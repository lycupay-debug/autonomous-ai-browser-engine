//! Rust-owned lifecycle wrappers over the V8 and SpiderMonkey C++ embedding APIs.

#[cfg(target_os = "freebsd")]
mod native {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::ptr::NonNull;

    unsafe extern "C" {
        fn aibe_v8_create() -> *mut c_void;
        fn aibe_v8_eval(
            handle: *mut c_void,
            source: *const c_char,
            output: *mut c_char,
            capacity: usize,
        ) -> c_int;
        fn aibe_v8_destroy(handle: *mut c_void);
        fn aibe_spidermonkey_create() -> *mut c_void;
        fn aibe_spidermonkey_eval(
            handle: *mut c_void,
            source: *const c_char,
            output: *mut c_char,
            capacity: usize,
        ) -> c_int;
        fn aibe_spidermonkey_destroy(handle: *mut c_void);
    }

    pub struct V8Runtime(NonNull<c_void>);
    pub struct SpiderMonkeyRuntime(NonNull<c_void>);

    impl V8Runtime {
        pub fn new() -> Result<Self, &'static str> {
            NonNull::new(unsafe { aibe_v8_create() })
                .map(Self)
                .ok_or("V8 initialization failed")
        }

        pub fn eval(&mut self, source: &str) -> Result<String, i32> {
            eval_with(self.0, source, aibe_v8_eval)
        }
    }

    impl SpiderMonkeyRuntime {
        pub fn new() -> Result<Self, &'static str> {
            NonNull::new(unsafe { aibe_spidermonkey_create() })
                .map(Self)
                .ok_or("SpiderMonkey initialization failed")
        }

        pub fn eval(&mut self, source: &str) -> Result<String, i32> {
            eval_with(self.0, source, aibe_spidermonkey_eval)
        }
    }

    fn eval_with(
        handle: NonNull<c_void>,
        source: &str,
        eval: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_char, usize) -> c_int,
    ) -> Result<String, i32> {
        let source = CString::new(source).map_err(|_| -10)?;
        let mut output = vec![0_i8; 4096];
        let status = unsafe {
            eval(
                handle.as_ptr(),
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

    impl Drop for V8Runtime {
        fn drop(&mut self) {
            unsafe { aibe_v8_destroy(self.0.as_ptr()) };
        }
    }

    impl Drop for SpiderMonkeyRuntime {
        fn drop(&mut self) {
            unsafe { aibe_spidermonkey_destroy(self.0.as_ptr()) };
        }
    }

    pub use SpiderMonkeyRuntime as PublicSpiderMonkeyRuntime;
    pub use V8Runtime as PublicV8Runtime;
}

#[cfg(target_os = "freebsd")]
pub use native::{PublicSpiderMonkeyRuntime as SpiderMonkeyRuntime, PublicV8Runtime as V8Runtime};

#[cfg(all(test, target_os = "freebsd"))]
mod tests {
    use super::{SpiderMonkeyRuntime, V8Runtime};

    #[test]
    fn rust_owns_both_native_runtime_lifecycles() {
        let mut v8 = V8Runtime::new().expect("V8 runtime initializes");
        assert_eq!(v8.eval("1 + 2").expect("V8 evaluates"), "3");
        drop(v8);

        let mut spider_monkey = SpiderMonkeyRuntime::new().expect("SpiderMonkey initializes");
        assert_eq!(
            spider_monkey.eval("1 + 2").expect("SpiderMonkey evaluates"),
            "3"
        );
        drop(spider_monkey);
    }
}
