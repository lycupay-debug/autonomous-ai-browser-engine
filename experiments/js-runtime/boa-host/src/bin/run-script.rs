//! Evaluate a JavaScript source file in a fresh Boa Context.
use std::env;
use std::fs;
use std::process;

use boa_engine::{Context, Source};

fn main() {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: run-script <javascript-file>");
        process::exit(2);
    };
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read JavaScript source: {error}");
            process::exit(3);
        }
    };
    let mut context = Context::default();
    match context.eval(Source::from_bytes(&source)) {
        Ok(value) => println!("{}", value.display()),
        Err(error) => {
            eprintln!("JavaScript evaluation failed: {error}");
            process::exit(1);
        }
    }
}
