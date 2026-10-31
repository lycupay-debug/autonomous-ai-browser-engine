//! Small, deterministic Test262 subset runner for the Boa embedding PoC.
//!
//! It intentionally supports only synchronous scripts with no external includes,
//! no module/async/raw flags, and the shared `sta.js` + `assert.js` harness.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use boa_engine::{Context, Source};

fn collect_js_files(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_js_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "js") {
            files.push(path);
        }
    }
    Ok(())
}

fn frontmatter(source: &str) -> Option<&str> {
    let start = source.find("/*---")? + "/*---".len();
    let end = source[start..].find("---*/")? + start;
    Some(&source[start..end])
}

fn is_supported_synchronous_test(source: &str) -> bool {
    let Some(metadata) = frontmatter(source) else {
        return false;
    };

    let lines: Vec<&str> = metadata.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("includes:") {
            let value = value.trim();
            if !value.is_empty() && value != "[]" {
                return false;
            }
            if value.is_empty()
                && lines
                    .get(index + 1)
                    .is_some_and(|next| next.trim_start().starts_with('-'))
            {
                return false;
            }
        }
        if let Some(value) = trimmed.strip_prefix("flags:") {
            if value.contains("module") || value.contains("async") || value.contains("raw") {
                return false;
            }
            if value.contains("onlyStrict") {
                return false;
            }
            if value.trim().is_empty()
                && lines.get(index + 1).is_some_and(|next| {
                    let next = next.trim();
                    next.starts_with("- module")
                        || next.starts_with("- async")
                        || next.starts_with("- raw")
                        || next.starts_with("- onlyStrict")
                })
            {
                return false;
            }
        }
    }
    true
}

fn expected_negative_type(source: &str) -> Option<String> {
    let metadata = frontmatter(source)?;
    let lines: Vec<&str> = metadata.lines().collect();
    let negative_index = lines.iter().position(|line| line.trim() == "negative:")?;
    let mut phase = None;
    let mut error_type = None;
    for line in lines.iter().skip(negative_index + 1) {
        let trimmed = line.trim();
        if !line.starts_with(char::is_whitespace) && !trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("phase:") {
            phase = Some(value.trim());
        }
        if let Some(value) = trimmed.strip_prefix("type:") {
            error_type = Some(value.trim());
        }
    }
    match (phase, error_type) {
        (Some("parse" | "runtime"), Some(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let root = PathBuf::from(args.next().unwrap_or_else(|| "/tmp/aibe-test262".into()));
    let directories: Vec<PathBuf> = args.map(|path| root.join(path)).collect();
    if directories.is_empty() {
        eprintln!("usage: test262_subset <test262-root> <relative-test-directory> [...]");
        std::process::exit(2);
    }

    let harness = format!(
        "{}\n{}\n",
        fs::read_to_string(root.join("harness/sta.js"))?,
        fs::read_to_string(root.join("harness/assert.js"))?
    );

    let mut files = Vec::new();
    for directory in &directories {
        collect_js_files(directory, &mut files)?;
    }
    files.sort();

    let mut executed = 0usize;
    let mut passed = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut failures = Vec::new();

    for path in files {
        let source = fs::read_to_string(&path)?;
        if !is_supported_synchronous_test(&source) {
            skipped += 1;
            continue;
        }
        executed += 1;

        let mut context = Context::default();
        let outcome = context
            .eval(Source::from_bytes(&harness))
            .and_then(|_| context.eval(Source::from_bytes(&source)))
            .and_then(|_| context.run_jobs());

        let expected_error = expected_negative_type(&source);
        let test_result = match (expected_error, outcome) {
            (Some(expected), Err(error)) if error.to_string().contains(&expected) => Ok(()),
            (Some(expected), Err(error)) => Err(format!("expected {expected}, received {error}")),
            (Some(expected), Ok(())) => Err(format!("expected negative {expected} test to throw")),
            (None, Ok(())) => Ok(()),
            (None, Err(error)) => Err(error.to_string()),
        };

        match test_result {
            Ok(()) => passed += 1,
            Err(error) => {
                failed += 1;
                if failures.len() < 20 {
                    failures.push(format!("{}: {error}", path.display()));
                }
            }
        }
    }

    println!(
        "Test262 revision: {}",
        env::var("TEST262_REVISION").unwrap_or_else(|_| "unspecified".into())
    );
    println!("Discovered: {}", executed + skipped);
    println!("Executed: {executed}");
    println!("Passed: {passed}");
    println!("Failed: {failed}");
    println!("Skipped (unsupported harness/flags): {skipped}");
    for failure in failures {
        eprintln!("FAIL {failure}");
    }

    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
