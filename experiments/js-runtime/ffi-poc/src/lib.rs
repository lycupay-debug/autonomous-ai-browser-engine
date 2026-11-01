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

/// Opaque, host-issued origin-group identifier. It is not a URL or origin string.
/// Only Realms explicitly created with the same handle share host-object access.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct JsOriginId {
    runtime_id: u64,
    origin_id: u64,
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
/// Object handles, engine-neutral callbacks, and browser-agent scheduling remain later work.
/// The V8 adapter has a separate opt-in Realm-scoped host-callback feasibility API.
///
/// No `Send` or `Sync` bound is imposed: the selected V8 isolate is thread-affine.
pub trait JsRuntime {
    fn create_origin(&mut self) -> Result<JsOriginId, JsRuntimeError>;
    fn create_realm(&mut self) -> Result<JsRealmId, JsRuntimeError>;
    fn create_realm_for_origin(&mut self, origin: JsOriginId) -> Result<JsRealmId, JsRuntimeError>;
    /// Initializes the minimal host Document surface for a browser Document Realm.
    /// Non-browser runtimes may keep the default no-op until they implement DOM bindings.
    fn initialize_document(&mut self, _realm: JsRealmId) -> Result<(), JsRuntimeError> {
        Ok(())
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsRuntimeError {
    pub code: i32,
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl std::fmt::Display for JsRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let (Some(line), Some(column)) = (self.line, self.column) {
            write!(formatter, "{} at {}:{}", self.message, line, column)
        } else {
            write!(formatter, "{}", self.message)
        }
    }
}

impl std::error::Error for JsRuntimeError {}

impl From<i32> for JsRuntimeError {
    fn from(code: i32) -> Self {
        Self {
            code,
            message: format!("JavaScript Runtime adapter error {code}"),
            line: None,
            column: None,
        }
    }
}

#[cfg(target_os = "freebsd")]
mod native {
    use std::cell::{Cell, RefCell};
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::marker::PhantomData;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr::{self, NonNull};
    use std::rc::Rc;
    use std::slice;
    use std::str;
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

    type HostCallbackFn = dyn FnMut(&str) -> Result<String, String>;

    struct HostCallbackState {
        callback: RefCell<Box<HostCallbackFn>>,
        active: Cell<bool>,
    }

    struct HostCallbackActiveGuard<'a>(&'a Cell<bool>);

    impl Drop for HostCallbackActiveGuard<'_> {
        fn drop(&mut self) {
            self.0.set(false);
        }
    }

    unsafe fn write_host_callback_output(
        output: *mut u8,
        output_capacity: usize,
        output_length: *mut usize,
        bytes: &[u8],
    ) -> bool {
        if output_length.is_null()
            || bytes.len() > output_capacity
            || (!bytes.is_empty() && output.is_null())
        {
            return false;
        }
        unsafe {
            *output_length = bytes.len();
            if !bytes.is_empty() {
                ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
            }
        }
        true
    }

    unsafe extern "C" fn host_callback_dispatch(
        user_data: *mut c_void,
        input: *const u8,
        input_length: usize,
        output: *mut u8,
        output_capacity: usize,
        output_length: *mut usize,
    ) -> c_int {
        if user_data.is_null() || (input.is_null() && input_length != 0) || output_length.is_null()
        {
            return -1;
        }
        unsafe {
            *output_length = 0;
        }
        let state = unsafe { &*(user_data.cast::<HostCallbackState>()) };
        if state.active.replace(true) {
            let message = b"reentrant host callback rejected";
            let written = unsafe {
                write_host_callback_output(output, output_capacity, output_length, message)
            };
            return if written { -3 } else { -4 };
        }
        let _active_guard = HostCallbackActiveGuard(&state.active);

        let input_bytes = if input_length == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(input, input_length) }
        };
        let input = match str::from_utf8(input_bytes) {
            Ok(value) => value,
            Err(_) => {
                let message = b"host callback input is not valid UTF-8";
                let written = unsafe {
                    write_host_callback_output(output, output_capacity, output_length, message)
                };
                return if written { -2 } else { -4 };
            }
        };

        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut callback = state
                .callback
                .try_borrow_mut()
                .map_err(|_| "reentrant host callback rejected".to_string())?;
            callback(input)
        }));
        let (status, message) = match result {
            Ok(Ok(value)) => (0, value),
            Ok(Err(error)) => (1, error),
            Err(_) => (2, "host callback panicked".to_string()),
        };
        if unsafe {
            write_host_callback_output(output, output_capacity, output_length, message.as_bytes())
        } {
            status
        } else {
            let fallback = b"host callback response exceeds the 16 KiB limit";
            let _ = unsafe {
                write_host_callback_output(output, output_capacity, output_length, fallback)
            };
            -4
        }
    }

    struct RawJsValueResult {
        status: i32,
        value_tag: c_int,
        boolean_value: c_int,
        number_value: f64,
        output: Vec<u8>,
        output_length: usize,
        error_line: u32,
        error_column: u32,
    }

    unsafe extern "C" {
        fn aibe_v8_create() -> *mut c_void;
        fn aibe_v8_eval(
            handle: *mut c_void,
            source: *const c_char,
            output: *mut c_char,
            capacity: usize,
            output_length: *mut usize,
        ) -> c_int;
        fn aibe_v8_create_realm(handle: *mut c_void, origin_id: u64, realm_id: *mut u64) -> c_int;
        #[cfg(test)]
        fn aibe_v8_origin_access_probe(
            handle: *mut c_void,
            source_realm_id: u64,
            same_origin_realm_id: u64,
            cross_origin_realm_id: u64,
        ) -> c_int;
        fn aibe_v8_destroy_realm(handle: *mut c_void, realm_id: u64) -> c_int;
        fn aibe_v8_install_host_callback(
            handle: *mut c_void,
            realm_id: u64,
            callback: unsafe extern "C" fn(
                *mut c_void,
                *const u8,
                usize,
                *mut u8,
                usize,
                *mut usize,
            ) -> c_int,
            user_data: *mut c_void,
        ) -> c_int;
        #[cfg(test)]
        fn aibe_v8_host_callback_cross_realm_probe(
            handle: *mut c_void,
            grant_realm_id: u64,
            caller_realm_id: u64,
        ) -> c_int;
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
            error_line: *mut u32,
            error_column: *mut u32,
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
            error_line: *mut u32,
            error_column: *mut u32,
        ) -> c_int;
        fn aibe_v8_run_microtasks(handle: *mut c_void) -> c_int;
        fn aibe_v8_force_gc(handle: *mut c_void) -> c_int;
        fn aibe_v8_finalized_dom_wrapper_count() -> usize;
        #[cfg(test)]
        pub(super) fn aibe_v8_cppgc_cycle_probe() -> c_int;
        #[cfg(test)]
        pub(super) fn aibe_v8_realm_wrapper_cache_probe(
            handle: *mut c_void,
            first_realm_id: u64,
            second_realm_id: u64,
        ) -> c_int;
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
        next_origin_id: u64,
        host_callback_state: Option<Box<HostCallbackState>>,
        host_callback_realm_id: Option<u64>,
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
                    next_origin_id: 1,
                    host_callback_state: None,
                    host_callback_realm_id: None,
                    _thread_affinity: PhantomData,
                })
                .ok_or("V8 initialization failed")
        }

        /// Creates a host-issued origin group. The opaque ID must not be
        /// derived from untrusted page-controlled URL or DOM data.
        pub fn create_origin(&mut self) -> Result<super::JsOriginId, super::JsRuntimeError> {
            if self.next_origin_id == 0 || self.next_origin_id == u64::MAX {
                return Err((-16).into());
            }
            let origin_id = self.next_origin_id;
            self.next_origin_id += 1;
            Ok(super::JsOriginId {
                runtime_id: self.runtime_id,
                origin_id,
            })
        }

        /// Creates a Realm with a host-issued origin-group capability.
        pub fn create_realm_for_origin(
            &mut self,
            origin: super::JsOriginId,
        ) -> Result<super::JsRealmId, super::JsRuntimeError> {
            if origin.runtime_id != self.runtime_id {
                return Err((-14).into());
            }
            let mut realm_id = 0_u64;
            let status = unsafe {
                aibe_v8_create_realm(self.handle.as_ptr(), origin.origin_id, &mut realm_id)
            };
            if status == 0 && realm_id != 0 {
                Ok(super::JsRealmId {
                    runtime_id: self.runtime_id,
                    realm_id,
                })
            } else {
                Err(if status == 0 {
                    (-13).into()
                } else {
                    status.into()
                })
            }
        }

        /// Creates a Realm in a fresh, isolated origin group.
        pub fn create_realm(&mut self) -> Result<super::JsRealmId, super::JsRuntimeError> {
            let origin = self.create_origin()?;
            self.create_realm_for_origin(origin)
        }

        pub fn destroy_realm(
            &mut self,
            realm: super::JsRealmId,
        ) -> Result<(), super::JsRuntimeError> {
            if realm.runtime_id != self.runtime_id {
                return Err((-14).into());
            }
            let status = unsafe { aibe_v8_destroy_realm(self.handle.as_ptr(), realm.realm_id) };
            if status == 0 {
                if self.host_callback_realm_id == Some(realm.realm_id) {
                    self.host_callback_state = None;
                    self.host_callback_realm_id = None;
                }
                Ok(())
            } else {
                Err(status.into())
            }
        }

        /// Explicitly grants one Realm a bounded host callback capability.
        /// The callback is not injected into the default/page Context unless
        /// that Realm is deliberately selected by the caller.
        pub fn install_host_callback<F>(
            &mut self,
            realm: super::JsRealmId,
            callback: F,
        ) -> Result<(), super::JsRuntimeError>
        where
            F: FnMut(&str) -> Result<String, String> + 'static,
        {
            if realm.runtime_id != self.runtime_id {
                return Err((-14).into());
            }
            if self.host_callback_state.is_some() {
                return Err((-15).into());
            }
            let mut state = Box::new(HostCallbackState {
                callback: RefCell::new(Box::new(callback)),
                active: Cell::new(false),
            });
            let user_data = (&mut *state as *mut HostCallbackState).cast::<c_void>();
            let status = unsafe {
                aibe_v8_install_host_callback(
                    self.handle.as_ptr(),
                    realm.realm_id,
                    host_callback_dispatch,
                    user_data,
                )
            };
            if status != 0 {
                return Err(status.into());
            }
            self.host_callback_state = Some(state);
            self.host_callback_realm_id = Some(realm.realm_id);
            Ok(())
        }

        #[cfg(test)]
        pub fn host_callback_cross_realm_probe(
            &mut self,
            grant: super::JsRealmId,
            caller: super::JsRealmId,
        ) -> i32 {
            if grant.runtime_id != self.runtime_id || caller.runtime_id != self.runtime_id {
                return -14;
            }
            unsafe {
                aibe_v8_host_callback_cross_realm_probe(
                    self.handle.as_ptr(),
                    grant.realm_id,
                    caller.realm_id,
                )
            }
        }

        #[cfg(test)]
        pub fn host_callback_state_pointer_for_test(&mut self) -> *mut c_void {
            self.host_callback_state
                .as_deref_mut()
                .map(|state| (state as *mut HostCallbackState).cast())
                .unwrap_or(ptr::null_mut())
        }

        pub fn eval_value_in_realm(
            &mut self,
            realm: super::JsRealmId,
            source: &str,
        ) -> Result<super::JsValue, super::JsRuntimeError> {
            if realm.runtime_id != self.runtime_id {
                return Err((-14).into());
            }
            let source = CString::new(source).map_err(|_| super::JsRuntimeError::from(-10))?;
            let mut value_tag = -1;
            let mut boolean_value = 0;
            let mut number_value = 0.0;
            let mut output = vec![0_u8; 1024 * 1024];
            let mut output_length = 0;
            let mut error_line = 0_u32;
            let mut error_column = u32::MAX;
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
                    &mut error_line,
                    &mut error_column,
                )
            };
            Self::decode_value_result(RawJsValueResult {
                status,
                value_tag,
                boolean_value,
                number_value,
                output,
                output_length,
                error_line,
                error_column,
            })
        }

        fn decode_value_result(
            result: RawJsValueResult,
        ) -> Result<super::JsValue, super::JsRuntimeError> {
            let RawJsValueResult {
                status,
                value_tag,
                boolean_value,
                number_value,
                output,
                output_length,
                error_line,
                error_column,
            } = result;
            if output_length > output.len() {
                return Err((-11).into());
            }
            if status != 0 {
                let message = String::from_utf8_lossy(&output[..output_length]).into_owned();
                return Err(super::JsRuntimeError {
                    code: status,
                    message: if message.is_empty() {
                        format!("JavaScript evaluation failed with status {status}")
                    } else {
                        message
                    },
                    line: (error_line > 0).then_some(error_line),
                    column: (error_column != u32::MAX).then_some(error_column),
                });
            }
            match value_tag {
                JS_VALUE_TAG_UNDEFINED => Ok(super::JsValue::Undefined),
                JS_VALUE_TAG_NULL => Ok(super::JsValue::Null),
                JS_VALUE_TAG_BOOLEAN => Ok(super::JsValue::Boolean(boolean_value != 0)),
                JS_VALUE_TAG_NUMBER => Ok(super::JsValue::Number(number_value)),
                JS_VALUE_TAG_STRING => String::from_utf8(output[..output_length].to_vec())
                    .map(super::JsValue::String)
                    .map_err(|_| (-12).into()),
                JS_VALUE_TAG_UNSUPPORTED => Ok(super::JsValue::Unsupported),
                _ => Err((-13).into()),
            }
        }

        pub fn eval_value(
            &mut self,
            source: &str,
        ) -> Result<super::JsValue, super::JsRuntimeError> {
            let source = CString::new(source).map_err(|_| super::JsRuntimeError::from(-10))?;
            let mut value_tag = -1;
            let mut boolean_value = 0;
            let mut number_value = 0.0;
            let mut output = vec![0_u8; 1024 * 1024];
            let mut output_length = 0;
            let mut error_line = 0_u32;
            let mut error_column = u32::MAX;
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
                    &mut error_line,
                    &mut error_column,
                )
            };
            Self::decode_value_result(RawJsValueResult {
                status,
                value_tag,
                boolean_value,
                number_value,
                output,
                output_length,
                error_line,
                error_column,
            })
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

        #[cfg(test)]
        pub fn origin_access_probe(
            &mut self,
            source: super::JsRealmId,
            same_origin: super::JsRealmId,
            cross_origin: super::JsRealmId,
        ) -> i32 {
            if [source, same_origin, cross_origin]
                .iter()
                .any(|realm| realm.runtime_id != self.runtime_id)
            {
                return -14;
            }
            unsafe {
                aibe_v8_origin_access_probe(
                    self.handle.as_ptr(),
                    source.realm_id,
                    same_origin.realm_id,
                    cross_origin.realm_id,
                )
            }
        }

        #[cfg(test)]
        pub fn realm_wrapper_cache_probe(
            &mut self,
            first: super::JsRealmId,
            second: super::JsRealmId,
        ) -> i32 {
            if first.runtime_id != self.runtime_id || second.runtime_id != self.runtime_id {
                return -14;
            }
            unsafe {
                aibe_v8_realm_wrapper_cache_probe(
                    self.handle.as_ptr(),
                    first.realm_id,
                    second.realm_id,
                )
            }
        }
    }

    #[cfg(test)]
    pub(super) fn invoke_host_callback_dispatch_for_test(
        user_data: *mut c_void,
        input: &str,
    ) -> (c_int, String) {
        let mut output = [0_u8; 128];
        let mut output_length = 0_usize;
        let status = unsafe {
            host_callback_dispatch(
                user_data,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut output_length,
            )
        };
        let message = String::from_utf8_lossy(&output[..output_length]).into_owned();
        (status, message)
    }

    impl Drop for V8Runtime {
        fn drop(&mut self) {
            // Safe Rust cannot move this wrapper to another thread.
            let _ = unsafe { aibe_v8_destroy(self.handle.as_ptr()) };
        }
    }

    impl super::JsRuntime for V8Runtime {
        fn create_origin(&mut self) -> Result<super::JsOriginId, super::JsRuntimeError> {
            V8Runtime::create_origin(self)
        }

        fn create_realm(&mut self) -> Result<super::JsRealmId, super::JsRuntimeError> {
            V8Runtime::create_realm(self)
        }

        fn create_realm_for_origin(
            &mut self,
            origin: super::JsOriginId,
        ) -> Result<super::JsRealmId, super::JsRuntimeError> {
            V8Runtime::create_realm_for_origin(self, origin)
        }

        fn initialize_document(
            &mut self,
            realm: super::JsRealmId,
        ) -> Result<(), super::JsRuntimeError> {
            self.eval_value_in_realm(
                realm,
                "globalThis.document = { nodeType: 9, documentElement: new Node() };",
            )
            .map(|_| ())
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
            self.eval_value(source)
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
        let syntax_error = runtime.eval_value("function (").unwrap_err();
        assert_eq!(syntax_error.code, -3);
        assert!(!syntax_error.message.is_empty());
        assert_eq!(syntax_error.line, Some(1));
        assert!(syntax_error.column.is_some());

        let runtime_error = runtime
            .eval_value("const valid = 1;\nthrow new Error('typed failure');")
            .unwrap_err();
        assert_eq!(runtime_error.code, -4);
        assert!(runtime_error.message.contains("typed failure"));
        assert_eq!(runtime_error.line, Some(2));
        assert!(runtime_error.column.is_some());
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
    fn v8_realm_host_callback_is_explicitly_granted_and_preserves_utf8_nul() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        let realm = runtime.create_realm().expect("Realm is created");
        runtime
            .install_host_callback(realm, |input| Ok(format!("reply:{input}")))
            .expect("host callback is granted to the selected Realm");

        assert_eq!(
            runtime
                .eval_value_in_realm(realm, "hostCall('A\\u0000B')")
                .unwrap(),
            super::JsValue::String("reply:A\0B".to_string())
        );
        assert_eq!(
            runtime.eval_value("typeof hostCall").unwrap(),
            super::JsValue::String("undefined".to_string()),
            "the default Context must not receive the host capability"
        );
        let other_realm = runtime.create_realm().expect("second Realm is created");
        assert_eq!(
            runtime
                .eval_value_in_realm(other_realm, "typeof hostCall")
                .unwrap(),
            super::JsValue::String("undefined".to_string()),
            "the callback is not ambient in other Realms"
        );
        assert_eq!(
            runtime
                .install_host_callback(other_realm, |_| Ok("second callback".to_string()))
                .unwrap_err()
                .code,
            -15,
            "a Runtime accepts only one active host callback capability"
        );
        let oversized_input = format!("hostCall('{}')", "x".repeat(16 * 1024 + 1));
        let input_limit_error = runtime
            .eval_value_in_realm(realm, &oversized_input)
            .unwrap_err();
        assert!(
            input_limit_error
                .message
                .contains("input exceeds the 16 KiB limit")
        );
        let argument_error = runtime
            .eval_value_in_realm(realm, "hostCall()")
            .unwrap_err();
        assert!(
            argument_error
                .message
                .contains("exactly one string argument")
        );

        runtime
            .destroy_realm(realm)
            .expect("destroying the granted Realm clears the callback capability");
        let replacement = runtime
            .create_realm()
            .expect("replacement Realm is created");
        runtime
            .install_host_callback(replacement, |input| Ok(input.to_uppercase()))
            .expect("callback state can be safely reinstalled after Realm teardown");
        assert_eq!(
            runtime
                .eval_value_in_realm(replacement, "hostCall('ready')")
                .unwrap(),
            super::JsValue::String("READY".to_string())
        );
    }

    #[test]
    fn v8_realm_host_callback_converts_errors_and_contains_panics() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        let realm = runtime.create_realm().expect("Realm is created");
        runtime
            .install_host_callback(realm, |input| match input {
                "failure" => Err("expected host callback failure".to_string()),
                "panic" => panic!("test panic must not cross the C ABI"),
                "oversize" => Ok("x".repeat(16 * 1024 + 1)),
                other => Ok(other.to_uppercase()),
            })
            .expect("host callback is installed");

        assert_eq!(
            runtime
                .eval_value_in_realm(
                    realm,
                    "try { hostCall('failure'); } catch (error) { error.message; }"
                )
                .unwrap(),
            super::JsValue::String("expected host callback failure".to_string())
        );
        assert_eq!(
            runtime
                .eval_value_in_realm(
                    realm,
                    "try { hostCall('panic'); } catch (error) { error.message; }"
                )
                .unwrap(),
            super::JsValue::String("host callback panicked".to_string())
        );
        assert_eq!(
            runtime
                .eval_value_in_realm(
                    realm,
                    "try { hostCall('oversize'); } catch (error) { error.message; }"
                )
                .unwrap(),
            super::JsValue::String("host callback response exceeds the 16 KiB limit".to_string())
        );
    }

    #[test]
    fn v8_realm_host_callback_function_cannot_be_transferred_to_another_realm() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        let shared_origin = runtime.create_origin().expect("host issues shared origin");
        let granted_realm = runtime
            .create_realm_for_origin(shared_origin)
            .expect("grant Realm is created");
        let caller_realm = runtime
            .create_realm_for_origin(shared_origin)
            .expect("same-origin caller Realm is created");
        runtime
            .install_host_callback(granted_realm, |_| Ok("must not be called".to_string()))
            .expect("host callback is granted only to the selected Realm");

        assert_eq!(
            runtime.host_callback_cross_realm_probe(granted_realm, caller_realm),
            0,
            "transferring the host function to another Realm must not transfer its capability"
        );
    }

    #[test]
    fn v8_realm_host_callback_rejects_reentrant_dispatch() {
        use std::cell::Cell;
        use std::rc::Rc;

        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        let realm = runtime.create_realm().expect("Realm is created");
        let state_pointer = Rc::new(Cell::new(std::ptr::null_mut()));
        let callback_state_pointer = Rc::clone(&state_pointer);
        runtime
            .install_host_callback(realm, move |input| {
                if input == "outer" {
                    let (status, message) = super::native::invoke_host_callback_dispatch_for_test(
                        callback_state_pointer.get(),
                        "inner",
                    );
                    Ok(format!("{status}:{message}"))
                } else {
                    Ok(input.to_string())
                }
            })
            .expect("host callback is installed");
        state_pointer.set(runtime.host_callback_state_pointer_for_test());

        assert_eq!(
            runtime
                .eval_value_in_realm(realm, "hostCall('outer')")
                .unwrap(),
            super::JsValue::String("-3:reentrant host callback rejected".to_string())
        );
    }

    #[test]
    fn v8_host_object_access_is_shared_only_by_realms_with_same_host_origin() {
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        let origin_a = runtime.create_origin().expect("origin A is issued by host");
        let origin_b = runtime.create_origin().expect("origin B is issued by host");
        let source = runtime
            .create_realm_for_origin(origin_a)
            .expect("source Realm created");
        let same_origin = runtime
            .create_realm_for_origin(origin_a)
            .expect("same-origin Realm created");
        let cross_origin = runtime
            .create_realm_for_origin(origin_b)
            .expect("cross-origin Realm created");

        assert_eq!(
            runtime.origin_access_probe(source, same_origin, cross_origin),
            0,
            "a native DOM wrapper may be used in another Realm only when the host grants the same origin group"
        );

        let mut other_runtime = V8Runtime::new().expect("independent Runtime initializes");
        assert_eq!(
            other_runtime
                .create_realm_for_origin(origin_a)
                .unwrap_err()
                .code,
            -14,
            "an origin capability from another Runtime must be rejected"
        );
    }

    #[test]
    fn v8_realms_are_isolated_and_destroyed_by_opaque_runtime_scoped_ids() {
        use super::JsRuntime;

        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        let first = runtime.create_realm().expect("first realm is created");
        let second = runtime.create_realm().expect("second realm is created");
        assert_ne!(first, second);
        assert_eq!(
            runtime.realm_wrapper_cache_probe(first, second),
            0,
            "the same native Node has stable per-Realm wrappers and cannot be used from another Realm"
        );

        // Every Context receives its own Node host binding. Wrapper identity is
        // stable within that Realm and does not depend on the default Context.
        assert_eq!(
            runtime
                .evaluate_value_in_realm(first, "typeof Node")
                .unwrap(),
            super::JsValue::String("function".to_string())
        );
        assert_eq!(
            runtime
                .evaluate_value_in_realm(second, "typeof Node")
                .unwrap(),
            super::JsValue::String("function".to_string())
        );
        let node_identity_script = "(() => { const root = new Node(); const child = new Node(); root.appendChild(child); return root.firstChild() === child && root.firstChild() === root.firstChild(); })()";
        assert_eq!(
            runtime
                .evaluate_value_in_realm(first, node_identity_script)
                .unwrap(),
            super::JsValue::Boolean(true)
        );
        assert_eq!(
            runtime
                .evaluate_value_in_realm(second, node_identity_script)
                .unwrap(),
            super::JsValue::Boolean(true)
        );
        assert_eq!(
            runtime
                .evaluate_value_in_realm(first, "globalThis.realmMarker = 17; realmMarker")
                .unwrap(),
            super::JsValue::Number(17.0)
        );
        assert_eq!(
            runtime
                .evaluate_value_in_realm(second, "typeof realmMarker")
                .unwrap(),
            super::JsValue::String("undefined".to_string())
        );

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
    fn v8_initializes_a_minimal_document_and_native_node_root_per_realm() {
        use super::{JsRuntime, JsValue};
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        let origin = runtime.create_origin().expect("origin created");
        let document_realm = runtime
            .create_realm_for_origin(origin)
            .expect("Document Realm created");
        runtime
            .initialize_document(document_realm)
            .expect("minimal Document surface initialized");
        assert_eq!(
            runtime
                .eval_value_in_realm(document_realm, "document.nodeType")
                .unwrap(),
            JsValue::Number(9.0)
        );
        assert_eq!(
            runtime
                .eval_value_in_realm(document_realm, "document.documentElement instanceof Node")
                .unwrap(),
            JsValue::Boolean(true)
        );
        assert_eq!(
            runtime
                .eval_value_in_realm(
                    document_realm,
                    "document.documentElement.appendChild(new Node()); document.documentElement.childCount()",
                )
                .unwrap(),
            JsValue::Number(1.0)
        );

        let worker_realm = runtime
            .create_realm_for_origin(origin)
            .expect("second Realm created");
        assert_eq!(
            runtime
                .eval_value_in_realm(worker_realm, "typeof document")
                .unwrap(),
            JsValue::String("undefined".into()),
            "a Realm does not receive a Document unless the host initializes it"
        );
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
                .eval("(() => { const parent = new Node(); const child = new Node(); parent.appendChild(child); const removed = parent.removeChild(child); return (removed === child) + ':' + parent.childCount(); })()")
                .expect("removeChild detaches and returns the child"),
            "true:0"
        );
        assert_eq!(
            runtime
                .eval("(() => { const parent = new Node(); const child = new Node(); try { parent.removeChild(child); return 'missing-allowed'; } catch (error) { return error.message; } })()")
                .expect("removeChild rejects a non-child"),
            "NotFoundError"
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
    fn v8_dom_detached_child_wrapper_is_released_after_gc() {
        let before = V8Runtime::finalized_dom_wrapper_count();
        let mut runtime = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(
            runtime
                .eval("globalThis.detachParent = new Node(); (() => { const detached = new Node(); detachParent.appendChild(detached); detachParent.removeChild(detached); })(); detachParent.childCount()")
                .expect("detached child is no longer retained by its former parent"),
            "0"
        );
        runtime
            .collect_garbage()
            .expect("detached child wrapper becomes collectible");
        assert!(
            V8Runtime::finalized_dom_wrapper_count() > before,
            "detached child wrapper should be finalized after its final native owner is released"
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
