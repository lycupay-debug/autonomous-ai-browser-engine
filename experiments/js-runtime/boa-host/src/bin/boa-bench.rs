//! Small, reproducible Boa embedding microbenchmarks; not a browser benchmark.

use std::hint::black_box;
use std::time::{Duration, Instant};

use boa_engine::{Context, Source};

fn evaluate(context: &mut Context, source: &str) -> String {
    context
        .eval(Source::from_bytes(source))
        .expect("benchmark script evaluates")
        .display()
        .to_string()
}

fn run_case(label: &str, source: &str, warmups: usize, samples: usize) {
    let mut context = Context::default();
    for _ in 0..warmups {
        black_box(evaluate(&mut context, source));
    }

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        black_box(evaluate(&mut context, source));
        durations.push(start.elapsed());
    }
    durations.sort_unstable();
    let median = durations[durations.len() / 2];
    let total: Duration = durations.iter().copied().sum();
    let mean = total / samples as u32;
    println!(
        "case={label} samples={samples} median_us={} mean_us={}",
        median.as_micros(),
        mean.as_micros()
    );
}

fn main() {
    println!("runtime=boa_engine-0.22.0");
    println!("mode=release microbenchmark; no JIT or browser APIs assumed");
    run_case(
        "integer_loop_50000",
        "(() => { let n = 0; for (let i = 0; i < 50000; i++) n += i; return n; })()",
        2,
        9,
    );
    run_case(
        "object_array_10000",
        "(() => { const a = []; for (let i = 0; i < 10000; i++) a.push({x:i, y:i*2}); return a.length; })()",
        2,
        9,
    );
    run_case(
        "string_concat_5000",
        "(() => { let s = ''; for (let i = 0; i < 5000; i++) s += String(i); return s.length; })()",
        2,
        9,
    );

    let samples = 50;
    let start = Instant::now();
    for _ in 0..samples {
        let mut context = Context::default();
        black_box(evaluate(&mut context, "1 + 1"));
    }
    println!(
        "case=context_create_and_eval samples={samples} mean_us={}",
        (start.elapsed() / samples as u32).as_micros()
    );
}
