use std::collections::HashMap;
use std::fs;
use std::path::Path;

use tengin::Engine;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("js") {
            out.push(p);
        }
    }
}

fn main() {
    let root = Path::new("vendor/test262/test");
    let mut files = Vec::new();
    walk(root, &mut files);

    let engine = Engine::new();
    let mut total = 0usize;
    let mut unsupported = 0usize;
    let mut oxc = 0usize;
    let mut skipped = 0usize;
    let mut categories: HashMap<String, usize> = HashMap::new();
    let mut samples: Vec<String> = Vec::new();
    let mut paths_by_cat: HashMap<String, Vec<String>> = HashMap::new();

    for f in &files {
        let src = match fs::read_to_string(f) {
            Ok(s) => s,
            Err(_) => continue,
        };
        total += 1;
        // Mimic the harness: skip `module` and `async` tests (not executed).
        if is_skipped(&src) {
            skipped += 1;
            continue;
        }
        // Non-strict parse attempt (most test262 parse in both modes identically).
        match engine.parse_source(&src) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("unsupported") || msg.contains("not supported") || msg.contains("not implemented") {
                    unsupported += 1;
                    // classify by first line
                    let first = msg.lines().next().unwrap_or("").to_string();
                    let cat = classify(&first);
                    *categories.entry(cat.clone()).or_insert(0) += 1;
                    if samples.len() < 40 && !samples.contains(&first) {
                        samples.push(first);
                    }
                    paths_by_cat
                        .entry(cat.clone())
                        .or_default()
                        .push(f.display().to_string());
                } else {
                    oxc += 1;
                }
            }
        }
    }

    println!("total files       : {total}");
    println!("skipped (async/mod): {skipped}");
    println!("unsupported errors: {unsupported}");
    println!("oxc real errors   : {oxc}");
    println!("--- unsupported categories ---");
    let mut v: Vec<_> = categories.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, c) in v {
        println!("{c:>8}  {k}");
    }
    println!("--- sample messages ---");
    for s in &samples {
        println!("  {}", s);
    }
    println!("--- assignment target files ---");
    for (k, v) in &paths_by_cat {
        if k.contains("assignment target") {
            for p in v.iter().take(40) {
                println!("  {p}");
            }
        }
    }
    println!("--- for-in/of files ---");
    for p in paths_by_cat.get("other: parse error: unsupported for-in/of left side").into_iter().flatten().take(15) {
        println!("  {p}");
    }
}

fn is_skipped(src: &str) -> bool {
    let Some(start) = src.find("/*---") else { return false; };
    let rest = &src[start + 5..];
    let Some(end) = rest.find("---*/") else { return false; };
    let block = &rest[..end];
    let mut in_flags = false;
    for line in block.lines() {
        let t = line.trim();
        if t == "flags:" {
            in_flags = true;
            continue;
        }
        if in_flags {
            if t.starts_with("- ") {
                let item = t[2..].trim();
                if item == "async" || item == "module" {
                    return true;
                }
            } else if !t.is_empty() && !t.starts_with('[') {
                in_flags = false;
            }
        }
        // inline list form: `flags: [module, async]`
        if let Some(after) = t.strip_prefix("flags:") {
            let s = after.trim();
            if let Some(inner) = s
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
            {
                for it in inner.split(',') {
                    let it = it.trim().trim_matches('"').trim_matches('\'').trim();
                    if it == "module" || it == "async" {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn classify(msg: &str) -> String {    let m = msg.to_lowercase();
    if m.contains("arrow") {
        "arrow functions".into()
    } else if m.contains("for-of") || m.contains("forof") {
        "for-of".into()
    } else if m.contains("do-while") || m.contains("while") {
        "while/do-while".into()
    } else if m.contains("label") {
        "labeled".into()
    } else if m.contains("with") {
        "with".into()
    } else if m.contains("spread") {
        "spread".into()
    } else if m.contains("destructur") {
        "destructuring".into()
    } else if m.contains("nullish") || m.contains("coalesc") {
        "nullish coalescing".into()
    } else if m.contains("optional") || m.contains("chain") {
        "optional chaining".into()
    } else if m.contains("bigint") {
        "BigInt".into()
    } else if m.contains("regex") {
        "RegExp".into()
    } else if m.contains("tagged") {
        "tagged template".into()
    } else if m.contains("private") {
        "private".into()
    } else if m.contains("bitwise") {
        "bitwise".into()
    } else if m.contains("exponent") {
        "exponentiation".into()
    } else if m.contains("await") {
        "await".into()
    } else if m.contains("yield") {
        "yield".into()
    } else if m.contains("module") || m.contains("import") || m.contains("export") {
        "modules".into()
    } else if m.contains("template") {
        "template".into()
    } else if m.contains("jsx") {
        "jsx".into()
    } else if m.contains("v8") {
        "v8 intrinsic".into()
    } else if m.contains("compound") {
        "compound assignment".into()
    } else {
        format!("other: {msg}")
    }
}
