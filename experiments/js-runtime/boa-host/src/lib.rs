//! Minimal host embedding experiment for the Boa JavaScript engine.
//!
//! This crate deliberately exposes no DOM, network, filesystem, or browser APIs.
//! Each evaluation gets a fresh Context so the initial isolation behavior can be tested.

use boa_engine::{Context, Source};

/// Evaluate one ECMAScript source string in a fresh Boa context.
pub fn evaluate(source: &str) -> Result<String, String> {
    let mut context = Context::default();
    context
        .eval(Source::from_bytes(source))
        .map(|value| value.display().to_string())
        .map_err(|error| format!("{error:?}"))
}

#[cfg(test)]
mod tests {
    use super::evaluate;
    use boa_engine::{Context, Source};

    #[test]
    fn evaluates_arithmetic() {
        assert_eq!(evaluate("1 + 2 * 3").expect("evaluation succeeds"), "7");
    }

    #[test]
    fn evaluates_functions_and_closures() {
        let result =
            evaluate("const make = (n) => (x) => x + n; make(4)(5)").expect("evaluation succeeds");
        assert_eq!(result, "9");
    }

    #[test]
    fn contexts_do_not_share_global_bindings() {
        evaluate("globalThis.phase0Secret = 123").expect("first context succeeds");
        let result = evaluate("typeof globalThis.phase0Secret").expect("second context succeeds");
        assert_eq!(result, "\"undefined\"");
    }

    #[test]
    fn promise_jobs_require_explicit_host_job_pump() {
        let mut context = Context::default();
        context
            .eval(Source::from_bytes(
                "globalThis.order = []; Promise.resolve().then(() => order.push('microtask')); order.push('sync');",
            ))
            .expect("script evaluates");

        let before_jobs = context
            .eval(Source::from_bytes("order.join(',')"))
            .expect("read synchronous state");
        assert_eq!(before_jobs.display().to_string(), "\"sync\"");

        context.run_jobs().expect("host drains Promise jobs");
        let after_jobs = context
            .eval(Source::from_bytes("order.join(',')"))
            .expect("read asynchronous state");
        assert_eq!(after_jobs.display().to_string(), "\"sync,microtask\"");
    }

    #[test]
    fn reports_syntax_errors() {
        assert!(evaluate("function (").is_err());
    }
}

#[cfg(test)]
mod dom_lifecycle;
