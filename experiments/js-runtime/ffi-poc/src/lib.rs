//! Rust-owned lifecycle wrappers over native V8 and SpiderMonkey embedding APIs.
//!
//! V8 is the selected runtime. SpiderMonkey remains available only as a
//! comparison/fallback PoC until a future ADR changes that decision.

/// Opaque, runtime-scoped identifier for an isolated JavaScript Realm.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct JsRealmId {
    runtime_id: u64,
    realm_id: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JsValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Unsupported,
}

/// Project-owned JavaScript Runtime boundary. Implementations must not expose
/// engine-specific handles or types. The Phase 0 contract supports typed
/// primitive results, stringified evaluation, and explicit host checkpoints.
/// Object handles, callbacks, and browser-agent scheduling remain later work.
///
/// No `Send` or `Sync` bound is imposed: the selected V8 isolate is thread-affine.
pub trait JsRuntime {
    fn create_realm(&mut self) -> Result<JsRealmId, JsRuntimeError>;
    fn destroy_realm(&mut self, realm: JsRealmId) -> Result<(), JsRuntimeError>;
    fn evaluate_value_in_realm(
        &mut self,
        realm: JsRealmId,
        source: &str,
    ) -> Result<JsValue, JsRuntimeError>;
    fn evaluate_value(&mut self, source: &str) -> Result<JsValue, JsRuntimeError>;
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);

    fn allocate_runtime_id() -> Option<u64> {
        NEXT_RUNTIME_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .ok()
    }

    const JS_VALUE_TAG_UNDEFINED: c_int = 0;
    const JS_VALUE_TAG_NULL: c_int = 1;
    const JS_VALUE_TAG_BOOLEAN: c_int = 2;
    const JS_VALUE_TAG_NUMBER: c_int = 3;
    const JS_VALUE_TAG_STRING: c_int = 4;
    const JS_VALUE_TAG_UNSUPPORTED: c_int = 5;

    unsafe extern "C" {
        fn aibe_v8_create() -> *mut c_void;
        fn aibe_v8_eval(
            handle: *mut c_void,
            source: *const c_char,
            output: *mut c_char,
            capacity: usize,
            output_length: *mut usize,
        ) -> c_int;
        fn aibe_v8_create_realm(handle: *mut c_void, realm_id: *mut u64) -> c_int;
        fn aibe_v8_destroy_realm(handle: *mut c_void, realm_id: u64) -> c_int;
        fn aibe_v8_eval_value_in_realm(
            handle: *mut c_void,
            realm_id: u64,
            source: *const c_char,
            value_tag: *mut c_int,
            boolean_value: *mut c_int,
            number_value: *mut f64,
            output: *mut c_char,
            capacity: usize,
            output_length: *mut usize,
        ) -> c_int;
        fn aibe_v8_eval_value(
            handle: *mut c_void,
            source: *const c_char,
            value_tag: *mut c_int,
            boolean_value: *mut c_int,
            number_value: *mut f64,
            output: *mut c_char,
            capacity: usize,
            output_length: *mut usize,
        ) -> c_int;
        fn aibe_v8_run_microtasks(handle: *mut c_void) -> c_int;
        fn aibe_v8_force_gc(handle: *mut c_void) -> c_int;
        fn aibe_v8_finalized_dom_wrapper_count() -> usize;
        #[cfg(test)]
        pub(super) fn aibe_v8_cppgc_cycle_probe() -> c_int;
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
        runtime_id: u64,
        _thread_affinity: PhantomData<Rc<()>>,
    }

    /// Comparison-only SpiderMonkey wrapper. Not the selected production path.
    pub struct SpiderMonkeyRuntime(NonNull<c_void>);

    #[cfg(test)]
    pub fn cppgc_cycle_probe() -> i32 {
        unsafe { aibe_v8_cppgc_cycle_probe() }
    }

    impl V8Runtime {
        pub fn new() -> Result<Self, &'static str> {
            let runtime_id = allocate_runtime_id().ok_or("V8 runtime identity space exhausted")?;
            NonNull::new(unsafe { aibe_v8_create() })
                .map(|handle| Self {
                    handle,
                    runtime_id,
                    _thread_affinity: PhantomData,
                })
                .ok_or("V8 initialization failed")
        }

        pub fn create_realm(&mut self) -> Result<super::JsRealmId, super::JsRuntimeError> {
            let mut realm_id = 0_u64;
            let status = unsafe { aibe_v8_create_realm(self.handle.as_ptr(), &mut realm_id) };
            if status == 0 && realm_id != 0 {
                Ok(super::JsRealmId {
                    runtime_id: self.runtime_id,
                    realm_id,
                })
            } else {
                Err(super::JsRuntimeError {
                    code: if status == 0 { -13 } else { status },
                })
            }
        }

        pub fn destroy_realm(
            &mut self,
            realm: super::JsRealmId,
        ) -> Result<(), super::JsRuntimeError> {
            if realm.runtime_id != self.runtime_id {
                return Err(super::JsRuntimeError { code: -14 });
            }
            let status = unsafe { aibe_v8_destroy_realm(self.handle.as_ptr(), realm.realm_id) };
            if status == 0 {
                Ok(())
            } else {
                Err(status.into())
            }
        }

        pub fn eval_value_in_realm(
            &mut self,
            realm: super::JsRealmId,
            source: &str,
        ) -> Result<super::JsValue, super::JsRuntimeError> {
            if realm.runtime_id != self.runtime_id {
                return Err(super::JsRuntimeError { code: -14 });
            }
            let source = CString::new(source).map_err(|_| super::JsRuntimeError { code: -10 })?;
            let mut value_tag = -1;
            let mut boolean_value = 0;
            let mut number_value = 0.0;
            let mut output = vec![0_u8; 1024 * 1024];
            let mut output_length = 0;
            let status = unsafe {
                aibe_v8_eval_value_in_realm(
                    self.handle.as_ptr(),
                    realm.realm_id,
                    source.as_ptr(),
                    &mut value_tag,
                    &mut boolean_value,
                    &mut number_value,
                    output.as_mut_ptr().cast(),
                    output.len(),
                    &mut output_length,
                )
            };
            Self::decode_value_result(
                status,
                value_tag,
                boolean_value,
                number_value,
                output,
                output_length,
            )
            .map_err(Into::into)
        }

        fn decode_value_result(
            status: i32,
            value_tag: c_int,
            boolean_value: c_int,
            number_value: f64,
            output: Vec<u8>,
            output_length: usize,
        ) -> Result<super::JsValue, i32> {
            if status != 0 {
                return Err(status);
            }
            if output_length > output.len() {
                return Err(-11);
            }
            match value_tag {
                JS_VALUE_TAG_UNDEFINED => Ok(super::JsValue::Undefined),
                JS_VALUE_TAG_NULL => Ok(super::JsValue::Null),
                JS_VALUE_TAG_BOOLEAN => Ok(super::JsValue::Boolean(boolean_value != 0)),
                JS_VALUE_TAG_NUMBER => Ok(super::JsValue::Number(number_value)),
                JS_VALUE_TAG_STRING => String::from_utf8(output[..output_length].to_vec())
                    .map(super::JsValue::String)
                    .map_err(|_| -12),
                JS_VALUE_TAG_UNSUPPORTED => Ok(super::JsValue::Unsupported),
                _ => Err(-13),
            }
        }

        pub fn eval_value(&mut self, source: &str) -> Result<super::JsValue, i32> {
            let source = CString::new(source).map_err(|_| -10)?;
            let mut value_tag = -1;
            let mut boolean_value = 0;
            let mut number_value = 0.0;
            let mut output = vec![0_u8; 1024 * 1024];
            let mut output_length = 0;
            let status = unsafe {
                aibe_v8_eval_value(
                    self.handle.as_ptr(),
                    source.as_ptr(),
                    &mut value_tag,
                    &mut boolean_value,
                    &mut number_value,
                    output.as_mut_ptr().cast(),
                    output.len(),
                    &mut output_length,
                )
            };
            Self::decode_value_result(
                status,
                value_tag,
                boolean_value,
                number_value,
                output,
                output_length,
            )
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
        fn create_realm(&mut self) -> Result<super::JsRealmId, super::JsRuntimeError> {
            V8Runtime::create_realm(self)
        }

        fn destroy_realm(&mut self, realm: super::JsRealmId) -> Result<(), super::JsRuntimeError> {
            V8Runtime::destroy_realm(self, realm)
        }

        fn evaluate_value_in_realm(
            &mut self,
            realm: super::JsRealmId,
            source: &str,
        ) -> Result<super::JsValue, super::JsRuntimeError> {
            V8Runtime::eval_value_in_realm(self, realm, source)
        }

        fn evaluate_value(
            &mut self,
            source: &str,
        ) -> Result<super::JsValue, super::JsRuntimeError> {
            self.eval_value(source).map_err(Into::into)
        }

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

#[cfg(all(test, target_os = "freebsd"))]
pub(crate) use native::cppgc_cycle_probe;
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

        fn verify_realm_through_project_contract(runtime: &mut dyn JsRuntime) {
            let realm = runtime
                .create_realm()
                .expect("project contract creates Realm");
            assert_eq!(
                runtime
                    .evaluate_value_in_realm(realm, "6 * 7")
                    .expect("project contract evaluates in Realm"),
                super::JsValue::Number(42.0)
            );
            runtime
                .destroy_realm(realm)
                .expect("project contract destroys Realm");
        }

        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(evaluate_through_project_contract(&mut runtime), "42");
        assert_eq!(
            runtime
                .evaluate_value("6 * 7")
                .expect("typed primitive result"),
            super::JsValue::Number(42.0)
        );
        verify_realm_through_project_contract(&mut runtime);
        runtime
            .microtask_checkpoint()
            .expect("project contract performs microtask checkpoint");
        runtime
            .collect_garbage()
            .expect("project contract collects garbage");
    }

    #[test]
    fn v8_typed_primitive_values_preserve_javascript_types() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(
            runtime.eval_value("undefined").unwrap(),
            super::JsValue::Undefined
        );
        assert_eq!(runtime.eval_value("null").unwrap(), super::JsValue::Null);
        assert_eq!(
            runtime.eval_value("true").unwrap(),
            super::JsValue::Boolean(true)
        );
        assert_eq!(
            runtime.eval_value("false").unwrap(),
            super::JsValue::Boolean(false)
        );
        assert_eq!(
            runtime.eval_value("1.25").unwrap(),
            super::JsValue::Number(1.25)
        );
        assert_eq!(
            runtime.eval_value("'A\\u0000B'").unwrap(),
            super::JsValue::String("A\0B".to_string())
        );
        assert_eq!(
            runtime.eval_value("({ value: 1 })").unwrap(),
            super::JsValue::Unsupported
        );
        assert_eq!(
            runtime.eval_value("1n").unwrap(),
            super::JsValue::Unsupported
        );
        assert_eq!(
            runtime.eval_value("Symbol('x')").unwrap(),
            super::JsValue::Unsupported
        );
        assert_eq!(
            runtime.eval_value("(() => {})").unwrap(),
            super::JsValue::Unsupported
        );
        assert_eq!(
            runtime.eval_value("''").unwrap(),
            super::JsValue::String(String::new())
        );
        assert_eq!(runtime.eval_value("function (").unwrap_err(), -3);
        assert_eq!(
            runtime
                .eval_value("throw new Error('typed failure')")
                .unwrap_err(),
            -4
        );
        match runtime.eval_value("-0").unwrap() {
            super::JsValue::Number(value) => {
                assert_eq!(value.to_bits(), (-0.0_f64).to_bits())
            }
            other => panic!("expected Number(-0), received {other:?}"),
        }
        assert!(matches!(
            runtime.eval_value("NaN").unwrap(),
            super::JsValue::Number(value) if value.is_nan()
        ));
    }

    #[test]
    fn v8_realms_are_isolated_and_destroyed_by_opaque_runtime_scoped_ids() {
        use super::JsRuntime;

        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        let first = runtime.create_realm().expect("first realm is created");
        let second = runtime.create_realm().expect("second realm is created");
        assert_ne!(first, second);

        let mut other_runtime = V8Runtime::new().expect("second runtime initializes");
        assert_eq!(
            other_runtime
                .eval_value_in_realm(first, "1 + 1")
                .unwrap_err()
                .code,
            -14
        );
        assert_eq!(other_runtime.destroy_realm(first).unwrap_err().code, -14);

        assert_eq!(
            runtime
                .evaluate_value_in_realm(first, "globalThis.privateValue = 41; privateValue")
                .unwrap(),
            super::JsValue::Number(41.0)
        );
        assert_eq!(
            runtime
                .evaluate_value_in_realm(second, "typeof privateValue")
                .unwrap(),
            super::JsValue::String("undefined".to_string())
        );
        assert_eq!(
            runtime.evaluate_value("typeof privateValue").unwrap(),
            super::JsValue::String("undefined".to_string())
        );
        assert_eq!(
            runtime
                .evaluate_value_in_realm(first, "privateValue + 1")
                .unwrap(),
            super::JsValue::Number(42.0)
        );

        runtime
            .destroy_realm(first)
            .expect("first realm is destroyed");
        assert_eq!(
            runtime
                .evaluate_value_in_realm(first, "1 + 1")
                .unwrap_err()
                .code,
            -9
        );
        assert_eq!(
            runtime.evaluate_value_in_realm(second, "6 * 7").unwrap(),
            super::JsValue::Number(42.0)
        );
        runtime
            .destroy_realm(second)
            .expect("second realm is destroyed");
        assert_eq!(runtime.destroy_realm(second).unwrap_err().code, -9);
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
    fn v8_dom_wrapper_cache_preserves_identity_and_rewraps_native_nodes() {
        let before = V8Runtime::finalized_dom_wrapper_count();
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");

        assert_eq!(
            runtime
                .eval("(() => { const parent = new Node(); const child = new Node(); parent.appendChild(child); return parent.firstChild() === child && parent.firstChild() === parent.firstChild(); })()")
                .expect("cached wrapper preserves object identity"),
            "true"
        );
        assert_eq!(
            runtime
                .eval("new Node().firstChild() === null")
                .expect("empty parent has no first child"),
            "true"
        );

        runtime
            .eval("globalThis.rootNode = new Node(); (() => { const transientChild = new Node(); rootNode.appendChild(transientChild); })();")
            .expect("native parent retains child after JS wrapper becomes unreachable");
        runtime
            .collect_garbage()
            .expect("V8 collects unreachable child wrapper");
        assert!(
            V8Runtime::finalized_dom_wrapper_count() > before,
            "unreachable child wrapper should be finalized while native child remains attached"
        );
        assert_eq!(
            runtime
                .eval("rootNode.firstChild() === rootNode.firstChild()")
                .expect("native child can be wrapped again"),
            "true"
        );
        assert_eq!(
            runtime
                .eval("rootNode.firstChild().childCount()")
                .expect("recreated wrapper references the same native node"),
            "0"
        );
    }

    #[test]
    fn v8_dom_event_listeners_retain_callbacks_across_wrapper_collection() {
        let before = V8Runtime::finalized_dom_wrapper_count();
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");

        runtime
            .eval("globalThis.eventRoot = new Node(); globalThis.eventCalls = 0; (() => { const child = new Node(); const captured = 41; child.addEventListener('ping', type => { eventCalls = captured + (type === 'ping' ? 1 : 0); }); eventRoot.appendChild(child); })();")
            .expect("native node stores a callback closure");

        runtime
            .collect_garbage()
            .expect("unreachable child wrapper can be collected");
        assert!(
            V8Runtime::finalized_dom_wrapper_count() > before,
            "child JS wrapper should be collected while its native node remains attached"
        );

        assert_eq!(
            runtime
                .eval("eventRoot.firstChild().dispatchEvent('ping') + ':' + eventCalls")
                .expect("native listener callback survives wrapper collection"),
            "1:42"
        );

        assert_eq!(
            runtime
                .eval("(() => { const node = eventRoot.firstChild(); const callback = () => { eventCalls += 100; }; node.addEventListener('once', callback); node.removeEventListener('once', callback); return node.dispatchEvent('once'); })()")
                .expect("listener removal prevents future dispatch"),
            "0"
        );
    }

    #[test]
    fn v8_dom_event_dispatch_handles_reentrant_listener_mutation() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(
            runtime
                .eval("(() => { const node = new Node(); let removedCalls = 0; const removed = () => removedCalls++; node.addEventListener('remove', () => node.removeEventListener('remove', removed)); node.addEventListener('remove', removed); const removeCount = node.dispatchEvent('remove'); let lateCalls = 0; let added = false; const late = () => lateCalls++; const addDuringDispatch = () => { if (!added) { added = true; node.addEventListener('add', late); } }; node.addEventListener('add', addDuringDispatch); node.addEventListener('add', addDuringDispatch); const firstAddCount = node.dispatchEvent('add'); const secondAddCount = node.dispatchEvent('add'); return [removeCount, removedCalls, firstAddCount, secondAddCount, lateCalls].join(':'); })()")
                .expect("dispatch snapshots tolerate reentrant listener mutation"),
            "1:0:1:2:1"
        );
        assert_eq!(
            runtime
                .eval("(() => { const node = new Node(); node.addEventListener('error', () => { throw new Error('listener failure'); }); try { node.dispatchEvent('error'); return 'exception-swallowed'; } catch (error) { return error.message; } })()")
                .expect("listener exception remains visible to the script caller"),
            "listener failure"
        );
    }

    #[test]
    fn v8_cppgc_unifies_native_and_javascript_callback_cycles() {
        let finalized = super::cppgc_cycle_probe();
        assert!(
            finalized >= 2,
            "CppHeap should collect a native Node -> traced JS callback -> JS wrapper -> native Node cycle"
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
