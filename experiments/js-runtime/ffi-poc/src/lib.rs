//! Rust-owned lifecycle wrappers over native V8 and SpiderMonkey embedding APIs.
//!
//! V8 is the selected runtime. SpiderMonkey remains available only as a
//! comparison/fallback PoC until a future ADR changes that decision.

/// Project-owned JavaScript Runtime boundary. Implementations must not expose
/// engine-specific handles or types. The initial Phase 0 contract deliberately
/// supports stringified evaluation and explicit host checkpoints only; typed
/// values, callbacks, realms, and browser-agent scheduling are later work.
///
/// No `Send` or `Sync` bound is imposed: the selected V8 isolate is thread-affine.
pub trait JsRuntime {
    fn evaluate_to_string(&mut self, source: &str) -> Result<String, JsRuntimeError>;
    fn microtask_checkpoint(&mut self) -> Result<(), JsRuntimeError>;
    fn collect_garbage(&mut self) -> Result<(), JsRuntimeError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsRuntimeError {
    pub code: i32,
}

impl std::fmt::Display for JsRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "JavaScript Runtime adapter error {}", self.code)
    }
}

impl std::error::Error for JsRuntimeError {}

impl From<i32> for JsRuntimeError {
    fn from(code: i32) -> Self {
        Self { code }
    }
}

#[cfg(target_os = "freebsd")]
mod native {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::marker::PhantomData;
    use std::ptr::NonNull;
    use std::rc::Rc;

    unsafe extern "C" {
        fn aibe_v8_create() -> *mut c_void;
        fn aibe_v8_eval(
            handle: *mut c_void,
            source: *const c_char,
            output: *mut c_char,
            capacity: usize,
            output_length: *mut usize,
        ) -> c_int;
        fn aibe_v8_run_microtasks(handle: *mut c_void) -> c_int;
        fn aibe_v8_force_gc(handle: *mut c_void) -> c_int;
        fn aibe_v8_finalized_dom_wrapper_count() -> usize;
        fn aibe_v8_destroy(handle: *mut c_void) -> c_int;
        fn aibe_spidermonkey_create() -> *mut c_void;
        fn aibe_spidermonkey_eval(
            handle: *mut c_void,
            source: *const c_char,
            output: *mut c_char,
            capacity: usize,
        ) -> c_int;
        fn aibe_spidermonkey_destroy(handle: *mut c_void);
    }

    /// One V8 isolate/context. Deliberately !Send + !Sync: V8 isolate access
    /// and destruction must remain on the thread that created the instance.
    pub struct V8Runtime {
        handle: NonNull<c_void>,
        _thread_affinity: PhantomData<Rc<()>>,
    }

    /// Comparison-only SpiderMonkey wrapper. Not the selected production path.
    pub struct SpiderMonkeyRuntime(NonNull<c_void>);

    impl V8Runtime {
        pub fn new() -> Result<Self, &'static str> {
            NonNull::new(unsafe { aibe_v8_create() })
                .map(|handle| Self {
                    handle,
                    _thread_affinity: PhantomData,
                })
                .ok_or("V8 initialization failed")
        }

        pub fn eval(&mut self, source: &str) -> Result<String, i32> {
            let source = CString::new(source).map_err(|_| -10)?;
            let mut output = vec![0_u8; 1024 * 1024];
            let mut output_length = 0;
            let status = unsafe {
                aibe_v8_eval(
                    self.handle.as_ptr(),
                    source.as_ptr(),
                    output.as_mut_ptr().cast(),
                    output.len(),
                    &mut output_length,
                )
            };
            if status != 0 {
                return Err(status);
            }
            if output_length > output.len() {
                return Err(-11);
            }
            String::from_utf8(output[..output_length].to_vec()).map_err(|_| -12)
        }

        pub fn run_microtasks(&mut self) -> Result<(), i32> {
            let status = unsafe { aibe_v8_run_microtasks(self.handle.as_ptr()) };
            if status == 0 { Ok(()) } else { Err(status) }
        }

        pub fn collect_garbage(&mut self) -> Result<(), i32> {
            let status = unsafe { aibe_v8_force_gc(self.handle.as_ptr()) };
            if status == 0 { Ok(()) } else { Err(status) }
        }

        pub fn finalized_dom_wrapper_count() -> usize {
            unsafe { aibe_v8_finalized_dom_wrapper_count() }
        }
    }

    impl Drop for V8Runtime {
        fn drop(&mut self) {
            // Safe Rust cannot move this wrapper to another thread.
            let _ = unsafe { aibe_v8_destroy(self.handle.as_ptr()) };
        }
    }

    impl super::JsRuntime for V8Runtime {
        fn evaluate_to_string(&mut self, source: &str) -> Result<String, super::JsRuntimeError> {
            self.eval(source).map_err(Into::into)
        }

        fn microtask_checkpoint(&mut self) -> Result<(), super::JsRuntimeError> {
            self.run_microtasks().map_err(Into::into)
        }

        fn collect_garbage(&mut self) -> Result<(), super::JsRuntimeError> {
            self.collect_garbage().map_err(Into::into)
        }
    }

    impl SpiderMonkeyRuntime {
        pub fn new() -> Result<Self, &'static str> {
            NonNull::new(unsafe { aibe_spidermonkey_create() })
                .map(Self)
                .ok_or("SpiderMonkey initialization failed")
        }

        pub fn eval(&mut self, source: &str) -> Result<String, i32> {
            let source = CString::new(source).map_err(|_| -10)?;
            let mut output = vec![0_i8; 4096];
            let status = unsafe {
                aibe_spidermonkey_eval(
                    self.0.as_ptr(),
                    source.as_ptr(),
                    output.as_mut_ptr(),
                    output.len(),
                )
            };
            if status != 0 {
                return Err(status);
            }
            let output = unsafe { CStr::from_ptr(output.as_ptr()) };
            Ok(output.to_string_lossy().into_owned())
        }
    }

    impl Drop for SpiderMonkeyRuntime {
        fn drop(&mut self) {
            unsafe { aibe_spidermonkey_destroy(self.0.as_ptr()) };
        }
    }
}

#[cfg(target_os = "freebsd")]
pub use native::{SpiderMonkeyRuntime, V8Runtime};

#[cfg(all(test, target_os = "freebsd"))]
mod tests {
    use super::{SpiderMonkeyRuntime, V8Runtime};

    #[test]
    fn selected_v8_implements_the_engine_agnostic_runtime_contract() {
        use super::JsRuntime;

        fn evaluate_through_project_contract(runtime: &mut dyn JsRuntime) -> String {
            runtime
                .evaluate_to_string("6 * 7")
                .expect("project Runtime contract evaluates")
        }

        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(evaluate_through_project_contract(&mut runtime), "42");
        runtime
            .microtask_checkpoint()
            .expect("project contract performs microtask checkpoint");
        runtime
            .collect_garbage()
            .expect("project contract collects garbage");
    }

    #[test]
    fn v8_rust_owned_lifecycle_and_basic_evaluation() {
        let mut v8 = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(v8.eval("1 + 2").expect("V8 evaluates"), "3");
        assert_eq!(v8.eval("'AIBE'").expect("V8 evaluates string"), "AIBE");
    }

    #[test]
    fn v8_reports_syntax_and_runtime_errors() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(runtime.eval("function (").unwrap_err(), -3);
        assert_eq!(runtime.eval("throw new Error('expected')").unwrap_err(), -4);
    }

    #[test]
    fn v8_instances_have_isolated_contexts_and_independent_lifetimes() {
        let mut first = V8Runtime::new().expect("first V8 runtime initializes");
        let mut second = V8Runtime::new().expect("second V8 runtime initializes");
        first
            .eval("globalThis.privateValue = 41")
            .expect("first context writes");
        second
            .eval("globalThis.privateValue = 99")
            .expect("second context writes");
        assert_eq!(first.eval("privateValue").expect("first reads"), "41");
        assert_eq!(second.eval("privateValue").expect("second reads"), "99");
        drop(first);
        assert_eq!(
            second
                .eval("privateValue + 1")
                .expect("second remains alive"),
            "100"
        );
    }

    #[test]
    fn v8_promise_jobs_require_the_host_microtask_checkpoint() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(
            runtime
                .eval("globalThis.events = []; Promise.resolve().then(() => events.push('microtask')); events.push('sync'); events.join(',')")
                .expect("script evaluates"),
            "sync"
        );
        runtime
            .run_microtasks()
            .expect("host runs microtask checkpoint");
        assert_eq!(
            runtime.eval("events.join(',')").expect("read async state"),
            "sync,microtask"
        );
    }

    #[test]
    fn v8_dom_wrapper_moves_children_and_rejects_ancestor_cycles() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(
            runtime
                .eval("(() => { const first = new Node(); const second = new Node(); const child = new Node(); first.appendChild(child); second.appendChild(child); return first.childCount() + ':' + second.childCount(); })()")
                .expect("native node can be moved between parents"),
            "0:1"
        );
        assert_eq!(
            runtime
                .eval("(() => { const parent = new Node(); const child = new Node(); parent.appendChild(child); try { child.appendChild(parent); return 'cycle-allowed'; } catch (error) { return error.message; } })()")
                .expect("ancestor cycle is rejected"),
            "HierarchyRequestError"
        );
    }

    #[test]
    fn v8_string_ffi_preserves_embedded_nul_bytes() {
        let mut v8 = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(
            v8.eval("'left\\u0000right'").expect("V8 returns string"),
            "left\0right"
        );
    }

    #[test]
    fn v8_dom_wrappers_support_native_tree_and_gc_cycle_finalization() {
        let before = V8Runtime::finalized_dom_wrapper_count();
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(
            runtime
                .eval("(() => { const parent = new Node(); const child = new Node(); parent.appendChild(child); return parent.childCount(); })()")
                .expect("native Node wrapper works"),
            "1"
        );
        runtime
            .eval("globalThis.parent = new Node(); globalThis.child = new Node(); parent.appendChild(child); child.self = child; parent.self = parent; delete globalThis.parent; delete globalThis.child;")
            .expect("JS/native wrapper cycles are created and unrooted");
        for _ in 0..5 {
            runtime.collect_garbage().expect("V8 GC completes");
        }
        assert!(
            V8Runtime::finalized_dom_wrapper_count() >= before + 4,
            "unreachable V8 DOM wrappers should be reclaimed"
        );
    }

    #[test]
    fn spidermonkey_remains_a_comparison_fallback() {
        let mut runtime = SpiderMonkeyRuntime::new().expect("SpiderMonkey initializes");
        assert_eq!(runtime.eval("1 + 2").expect("SpiderMonkey evaluates"), "3");
    }
}
