//! A command-line runner for the test262 conformance suite.
//!
//! Usage:
//!   tengin-test262 [filters...] [-v] [--test262 PATH] [--timeout N] [--jobs N] [--shard N/M]
//!
//! * `filters`   – if any are given, only tests whose path contains one of the
//!                 substrings are executed (handy for a single directory/file).
//! * `-v`        – print every failing (and skipped) test after the run.
//! * `--test262` – override the location of the test262 checkout
//!                 (defaults to `vendor/test262`).
//! * `--timeout` – per-test timeout in seconds (default 60); tests that exceed
//!                 it are reported as failures instead of hanging the run.
//! * `--jobs`    – maximum number of test processes to run concurrently
//!                 (default 2).
//! * `--shard`   – run one zero-based shard of the filtered test list, for
//!                 example `--shard 3/16`.
//!
//! Concurrency model
//! -----------------
//! A fixed pool of worker threads pulls test indices off a shared atomic cursor.
//! Each worker launches one test file in its own short-lived child process. The
//! child process boundary keeps per-test heap growth, leaks, panics, and crashes
//! isolated from the parent runner. A watchdog kills a child that exceeds the
//! configured per-test timeout, so the maximum number of in-flight engines is
//! exactly `--jobs`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tengin_test262::{
    collect_test_files, is_excluded, load_excludelist, run_test_file, Status, TestOutcome,
};

fn parse_shard_spec(value: &str) -> Option<(usize, usize)> {
    let (index, count) = value.split_once('/')?;
    let index = index.parse::<usize>().ok()?;
    let count = count.parse::<usize>().ok()?;
    if count == 0 || index >= count {
        return None;
    }
    Some((index, count))
}

fn parse_jobs(value: &str) -> Option<usize> {
    let jobs = value.parse::<usize>().ok()?;
    (jobs > 0).then_some(jobs)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Hidden worker mode used by the parent runner. Running one test in a
    // child process gives the timeout watchdog a reliable way to terminate a
    // runaway interpreter (Rust threads cannot be forcefully cancelled).
    if let Some(i) = args.iter().position(|a| a == "--single") {
        if let Some(path) = args.get(i + 1) {
            let outcomes = run_test_file(Path::new(path));
            let mut p = 0;
            let mut f = 0;
            let mut s = 0;
            for outcome in outcomes {
                match outcome.status {
                    Status::Pass => p += 1,
                    Status::Fail => f += 1,
                    Status::Skip => s += 1,
                }
            }
            // Encode the small per-file counts in the exit status so the
            // parent never has to drain a potentially noisy stdout pipe.
            std::process::exit(10 + p * 9 + f * 3 + s);
        }
    }

    let mut verbose = false;
    let mut test262_root = PathBuf::from("vendor/test262");
    let mut filters: Vec<String> = Vec::new();
    let mut timeout_secs: u64 = 60;
    let mut num_threads: usize = 2;
    let mut shard: Option<(usize, usize)> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" | "--verbose" => verbose = true,
            "--test262" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--test262 requires a path");
                    std::process::exit(2);
                }
                test262_root = PathBuf::from(&args[i]);
            }
            "--timeout" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--timeout requires a number of seconds");
                    std::process::exit(2);
                }
                match args[i].parse::<u64>() {
                    Ok(t) if t > 0 => timeout_secs = t,
                    _ => {
                        eprintln!("invalid --timeout value: {}", args[i]);
                        std::process::exit(2);
                    }
                }
            }
            "--jobs" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--jobs requires a positive integer");
                    std::process::exit(2);
                }
                match parse_jobs(&args[i]) {
                    Some(jobs) => num_threads = jobs,
                    None => {
                        eprintln!("invalid --jobs value: {}", args[i]);
                        std::process::exit(2);
                    }
                }
            }
            "--shard" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--shard requires N/M, for example 3/16");
                    std::process::exit(2);
                }
                match parse_shard_spec(&args[i]) {
                    Some(spec) => shard = Some(spec),
                    None => {
                        eprintln!(
                            "invalid --shard value: {} (expected zero-based N/M with N < M)",
                            args[i]
                        );
                        std::process::exit(2);
                    }
                }
            }
            other => {
                if let Some(v) = other.strip_prefix("--test262=") {
                    test262_root = PathBuf::from(v);
                } else if let Some(v) = other.strip_prefix("--timeout=") {
                    match v.parse::<u64>() {
                        Ok(t) if t > 0 => timeout_secs = t,
                        _ => {
                            eprintln!("invalid --timeout value: {v}");
                            std::process::exit(2);
                        }
                    }
                } else if let Some(v) = other.strip_prefix("--jobs=") {
                    match parse_jobs(v) {
                        Some(jobs) => num_threads = jobs,
                        None => {
                            eprintln!("invalid --jobs value: {v}");
                            std::process::exit(2);
                        }
                    }
                } else if let Some(v) = other.strip_prefix("--shard=") {
                    match parse_shard_spec(v) {
                        Some(spec) => shard = Some(spec),
                        None => {
                            eprintln!(
                                "invalid --shard value: {v} (expected zero-based N/M with N < M)"
                            );
                            std::process::exit(2);
                        }
                    }
                } else if other.starts_with('-') && other != "-" {
                    eprintln!("unknown option: {other}");
                    std::process::exit(2);
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

    // Stable ordering makes shard membership deterministic across machines.
    files.sort();

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

    let matched_files = files.len();
    if let Some((shard_index, shard_count)) = shard {
        files = files
            .into_iter()
            .enumerate()
            .filter_map(|(index, path)| {
                (index % shard_count == shard_index).then_some(path)
            })
            .collect();
    }

    if files.is_empty() {
        println!("no test files matched the given criteria");
        return;
    }

    if let Some((shard_index, shard_count)) = shard {
        println!(
            "Running {} test files from {:?} (shard {}/{}, {} matched before sharding, jobs={}) ...",
            files.len(),
            test262_root.display(),
            shard_index,
            shard_count,
            matched_files,
            num_threads
        );
    } else {
        println!(
            "Running {} test files from {:?} (jobs={}) ...",
            files.len(),
            test262_root.display(),
            num_threads
        );
    }

    let files = Arc::new(files);
    let cursor = Arc::new(AtomicUsize::new(0));
    let failures: Arc<Mutex<Vec<TestOutcome>>> = Arc::new(Mutex::new(Vec::new()));
    let pass = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicUsize::new(0));
    let skip = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));

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

        handles.push(thread::spawn(move || loop {
            let idx = cursor.fetch_add(1, Ordering::Relaxed);
            if idx >= files.len() {
                break;
            }

            let path = &files[idx];
            let outcomes = run_test_subprocess(path, timeout_secs);

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
    println!("files matched : {matched_files}");
    println!("files run     : {}", files.len());
    if let Some((shard_index, shard_count)) = shard {
        println!("shard         : {shard_index}/{shard_count}");
    }
    println!("jobs          : {num_threads}");
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

/// Execute one test in a child process and terminate it if it exceeds the
/// configured timeout. This prevents interpreter heap graphs from surviving
/// between test files and gives the parent a reliable kill boundary.
fn run_test_subprocess(path: &Path, timeout_secs: u64) -> Vec<TestOutcome> {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return vec![TestOutcome {
                path: path.to_string_lossy().to_string(),
                strict: false,
                status: Status::Fail,
                detail: format!("cannot locate test runner: {e}"),
            }]
        }
    };

    let mut child = match Command::new(exe)
        .arg("--single")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return vec![TestOutcome {
                path: path.to_string_lossy().to_string(),
                strict: false,
                status: Status::Fail,
                detail: format!("cannot spawn test runner: {e}"),
            }]
        }
    };

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return vec![TestOutcome {
                    path: path.to_string_lossy().to_string(),
                    strict: false,
                    status: Status::Fail,
                    detail: "timeout".to_string(),
                }];
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return vec![TestOutcome {
                    path: path.to_string_lossy().to_string(),
                    strict: false,
                    status: Status::Fail,
                    detail: format!("test runner wait failed: {e}"),
                }];
            }
        }
    };

    let Some(code) = status.code().filter(|code| (10..=37).contains(code)) else {
        return vec![TestOutcome {
            path: path.to_string_lossy().to_string(),
            strict: false,
            status: Status::Fail,
            detail: abnormal_exit_detail(&status),
        }];
    };

    let encoded = (code - 10) as usize;
    let pass = encoded / 9;
    let fail = (encoded % 9) / 3;
    let skip = encoded % 3;
    let mut outcomes = Vec::with_capacity(pass + fail + skip);

    for _ in 0..pass {
        outcomes.push(TestOutcome {
            path: path.to_string_lossy().to_string(),
            strict: false,
            status: Status::Pass,
            detail: String::new(),
        });
    }
    for _ in 0..fail {
        outcomes.push(TestOutcome {
            path: path.to_string_lossy().to_string(),
            strict: false,
            status: Status::Fail,
            detail: "failed in worker process".to_string(),
        });
    }
    for _ in 0..skip {
        outcomes.push(TestOutcome {
            path: path.to_string_lossy().to_string(),
            strict: false,
            status: Status::Skip,
            detail: String::new(),
        });
    }

    outcomes
}

fn abnormal_exit_detail(status: &ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return match signal {
                6 => "test runner aborted (SIGABRT)".to_string(),
                9 => "test runner killed (SIGKILL; possible OOM)".to_string(),
                11 => "test runner crashed (SIGSEGV)".to_string(),
                _ => format!("test runner terminated by signal {signal}"),
            };
        }
    }

    match status.code() {
        Some(code) => format!("test runner exited without a result (code {code})"),
        None => "test runner exited without a result".to_string(),
    }
}
