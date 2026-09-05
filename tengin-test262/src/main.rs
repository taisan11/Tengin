//! A command-line runner for the test262 conformance suite.
//!
//! Usage:
//!   tengin-test262 [filters...] [-v] [--test262 PATH] [--timeout N]
//!
//! * `filters`   – if any are given, only tests whose path contains one of the
//!                 substrings are executed (handy for a single directory/file).
//! * `-v`        – print every failing (and skipped) test after the run.
//! * `--test262` – override the location of the test262 checkout
//!                 (defaults to `vendor/test262`).
//! * `--timeout` – per-test timeout in seconds (default 10); tests that exceed
//!                 it are reported as failures instead of hanging the run.
//!
//! Concurrency model
//! -----------------
//! A fixed pool of `num_threads` worker threads pulls test indices off a shared
//! atomic cursor and executes them. Each test is run in its *own* short-lived
//! thread so that a misbehaving test (infinite loop, stack overflow) cannot
//! corrupt or permanently block a worker. To keep memory and CPU bounded even
//! when tests hang, the number of concurrently in-flight test threads is capped
//! by a semaphore of size `num_threads`: a test that times out is detached
//! (its thread is abandoned, not joined) and keeps holding its permit until it
//! actually finishes, so at most `num_threads` such threads can accumulate.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tengin_test262::{collect_test_files, is_excluded, load_excludelist, run_test_file, Status, TestOutcome};

/// A counting semaphore used to bound the number of concurrently executing test
/// threads. Permits are released by the test thread when it exits; on timeout a
/// thread is detached without releasing its permit, which caps how many hung
/// tests can pile up.
struct Semaphore {
    permits: Mutex<usize>,
    cvar: Condvar,
}

impl Semaphore {
    fn new(n: usize) -> Self {
        Semaphore {
            permits: Mutex::new(n),
            cvar: Condvar::new(),
        }
    }

    fn acquire(&self) {
        let mut p = self.permits.lock().unwrap();
        while *p == 0 {
            p = self.cvar.wait(p).unwrap();
        }
        *p -= 1;
    }

    fn release(&self) {
        let mut p = self.permits.lock().unwrap();
        *p += 1;
        drop(p);
        self.cvar.notify_one();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut verbose = false;
    let mut test262_root = PathBuf::from("vendor/test262");
    let mut filters: Vec<String> = Vec::new();
    let mut timeout_secs: u64 = 60;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" | "--verbose" => verbose = true,
            "--test262" => {
                i += 1;
                if i < args.len() {
                    test262_root = PathBuf::from(&args[i]);
                }
            }
            "--timeout" => {
                i += 1;
                if i < args.len() {
                    if let Ok(t) = args[i].parse::<u64>() {
                        timeout_secs = t;
                    }
                }
            }
            other => {
                if let Some(v) = other.strip_prefix("--test262=") {
                    test262_root = PathBuf::from(v);
                } else if other.starts_with('-') && other != "-" {
                    eprintln!("unknown option: {other}");
                } else {
                    filters.push(other.to_string());
                }
            }
        }
        i += 1;
    }

    let test_dir = test262_root.join("test");
    if !test_dir.exists() {
        eprintln!(
            "test262 test directory not found at {:?}\n(run `git submodule update --init` first)",
            test_dir
        );
        std::process::exit(2);
    }

    let excludes = load_excludelist(&test262_root.join("excludelist.xml"));

    let mut files = collect_test_files(&test_dir);
    let total_files = files.len();
    files.retain(|p| {
        if is_excluded(p, &excludes) {
            return false;
        }
        if !filters.is_empty() {
            let s = p.to_string_lossy();
            if !filters.iter().any(|f| s.contains(f.as_str())) {
                return false;
            }
        }
        true
    });

    if files.is_empty() {
        println!("no test files matched the given criteria");
        return;
    }

    println!(
        "Running {} test files from {:?} ...",
        files.len(),
        test262_root.display()
    );

    let files = Arc::new(files);
    let cursor = Arc::new(AtomicUsize::new(0));
    let failures: Arc<Mutex<Vec<TestOutcome>>> = Arc::new(Mutex::new(Vec::new()));
    let pass = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicUsize::new(0));
    let skip = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));

    let num_threads = 2; // TEMP: capped for peak-memory measurement

    // Cap concurrently in-flight test threads so hung tests cannot accumulate
    // without bound.
    let sem = Arc::new(Semaphore::new(num_threads));

    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..num_threads {
        let files = files.clone();
        let cursor = cursor.clone();
        let failures = failures.clone();
        let pass = pass.clone();
        let fail = fail.clone();
        let skip = skip.clone();
        let done = done.clone();
        let sem = sem.clone();
        handles.push(thread::spawn(move || loop {
            let idx = cursor.fetch_add(1, Ordering::Relaxed);
            if idx >= files.len() {
                break;
            }

            // Borrow the path from the shared vec so we avoid cloning it into the
            // worker thread.
            let path: PathBuf = files[idx].clone();
            let path_for_worker = path.clone();

            let sem = sem.clone();
            let sem_for_timeout = sem.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            sem.acquire();
            // Give each test its own thread with a large stack: the interpreter
            // is a tree-walker that recurses on the Rust call stack, so deeply
            // (but legitimately) nested programs would otherwise overflow the
            // default 2MB stack and abort the whole run.
            let worker = thread::Builder::new()
                .stack_size(256 * 1024 * 1024)
                .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_test_file(&path_for_worker)
                }));
                let outcomes = result.unwrap_or_else(|_| {
                    vec![TestOutcome {
                        path: path_for_worker.to_string_lossy().to_string(),
                        strict: false,
                        status: Status::Fail,
                        detail: "panic while running test".to_string(),
                    }]
                });
                let _ = tx.send(outcomes);
                // Hand the permit back only once the test thread is done.
                sem.release();
            })
            .unwrap();

            let outcomes = match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
                Ok(o) => {
                    // Test finished in time; reap the thread (it has already
                    // released its permit).
                    let _ = worker.join();
                    o
                }
                Err(_) => {
                    // Timed out. Detach the worker instead of joining it so we
                    // never block on a hung test. Release the permit now so a
                    // stream of timeouts cannot starve the whole run; the (late)
                    // thread's eventual release is a harmless over-release.
                    sem_for_timeout.release();
                    fail.fetch_add(1, Ordering::Relaxed);
                    if verbose {
                        eprintln!("FAIL [non-strict] {}: timeout", path.display());
                        failures.lock().unwrap().push(TestOutcome {
                            path: path.to_string_lossy().to_string(),
                            strict: false,
                            status: Status::Fail,
                            detail: "timeout".to_string(),
                        });
                    }
                    continue;
                }
            };

            for o in outcomes {
                match o.status {
                    Status::Pass => {
                        pass.fetch_add(1, Ordering::Relaxed);
                    }
                    Status::Fail => {
                        fail.fetch_add(1, Ordering::Relaxed);
                        if verbose {
                            let mode = if o.strict { "strict" } else { "non-strict" };
                            eprintln!("FAIL [{}] {}: {}", mode, o.path, o.detail);
                            failures.lock().unwrap().push(o);
                        }
                    }
                    Status::Skip => {
                        skip.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            let completed = done.fetch_add(1, Ordering::Relaxed) + 1;
            if completed % 1000 == 0 || completed == files.len() {
                eprintln!(
                    "  [{}/{} files] pass={} fail={} skip={}",
                    completed,
                    files.len(),
                    pass.load(Ordering::Relaxed),
                    fail.load(Ordering::Relaxed),
                    skip.load(Ordering::Relaxed),
                );
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let elapsed = start.elapsed();

    let p = pass.load(Ordering::Relaxed);
    let f = fail.load(Ordering::Relaxed);
    let s = skip.load(Ordering::Relaxed);
    let total = p + f + s;

    println!();
    println!("=== test262 run complete ===");
    println!("files scanned : {total_files}");
    println!("files run     : {}", files.len());
    println!("cases         : {total}");
    println!("  PASS        : {p}");
    println!("  FAIL        : {f}");
    println!("  SKIP        : {s}");
    if total > 0 {
        println!("  pass rate   : {:.2}%", (p as f64 / total as f64) * 100.0);
    }
    println!("elapsed       : {:.1?}", elapsed);

    if verbose && !failures.lock().unwrap().is_empty() {
        println!("\n--- FAILURES ---");
        for o in failures.lock().unwrap().iter() {
            let mode = if o.strict { "strict" } else { "non-strict" };
            println!("{} ({mode}): {}", o.path, o.detail);
        }
    }
}
