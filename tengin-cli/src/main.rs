use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use std::env;
use std::path::PathBuf;
use std::process;

use tengin_test262::{run_files, summarize, Status, TestOutcome};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: {} <test.js> [test.js ...]", args[0]);
        process::exit(2);
    }

    let paths: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
    let outcomes = run_files(&paths);

    let mut overall = true;
    for o in &outcomes {
        print_outcome(o);
        if o.status == Status::Fail {
            overall = false;
        }
    }

    let (pass, fail, skip) = summarize(&outcomes);
    println!("---");
    println!("pass={pass} fail={fail} skip={skip}");

    process::exit(if overall { 0 } else { 1 });
}

fn print_outcome(o: &TestOutcome) {
    let tag = match o.status {
        Status::Pass => "PASS",
        Status::Fail => "FAIL",
        Status::Skip => "SKIP",
    };
    let detail = if o.detail.is_empty() {
        String::new()
    } else {
        format!(" ({})", o.detail)
    };
    println!(
        "{} (strict={}) {}{}",
        tag,
        o.strict,
        o.path,
        detail
    );
}
