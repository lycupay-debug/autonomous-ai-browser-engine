use std::env;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "freebsd" {
        println!(
            "cargo:warning=Native V8/SpiderMonkey FFI PoC is FreeBSD-only; native link step skipped"
        );
        return;
    }

    let prefix =
        PathBuf::from(env::var("AIBE_RUNTIME_PREFIX").unwrap_or_else(|_| "/usr/local".into()));
    let include = prefix.join("include");
    let library = prefix.join("lib");

    let include_string = include.to_string_lossy().into_owned();
    let spider_monkey_include_string = include.join("mozjs-153").to_string_lossy().into_owned();

    let mut v8 = cc::Build::new();
    v8.cpp(true)
        .std("c++20")
        .flag("-Wall")
        .flag("-Wextra")
        .flag("-Werror")
        .flag("-isystem")
        .flag(&include_string)
        .define("V8_COMPRESS_POINTERS", None)
        .define("V8_31BIT_SMIS_ON_64BIT_ARCH", None)
        .define("V8_ENABLE_SANDBOX", None)
        .include(&include)
        .file("src/v8_ffi.cc")
        .compile("aibe_v8_ffi");

    let mut spider_monkey = cc::Build::new();
    spider_monkey
        .cpp(true)
        .std("c++20")
        .flag("-Wall")
        .flag("-Wextra")
        .flag("-Werror")
        .flag("-isystem")
        .flag(&spider_monkey_include_string)
        .define("XP_UNIX", None)
        .define("XP_FREEBSD", None)
        .file("src/spidermonkey_ffi.cpp")
        .compile("aibe_spidermonkey_ffi");

    println!("cargo:rustc-link-search=native={}", library.display());
    for library in [
        "v8",
        "v8_libplatform",
        "v8_libbase",
        "third_party_partition_alloc_src_partition_alloc_allocator_core",
        "third_party_partition_alloc_src_partition_alloc_allocator_shim",
        "third_party_partition_alloc_src_partition_alloc_allocator_base",
        "third_party_abseil-cpp_absl",
        "chrome_zlib",
        "icui18n",
        "icuuc",
        "icudata",
        "mozjs-153",
        "nspr4",
        "plc4",
        "plds4",
    ] {
        println!("cargo:rustc-link-lib=dylib={library}");
    }

    println!("cargo:rerun-if-env-changed=AIBE_RUNTIME_PREFIX");
    println!("cargo:rerun-if-env-changed=CXX");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/v8_ffi.cc");
    println!("cargo:rerun-if-changed=src/spidermonkey_ffi.cpp");
}
