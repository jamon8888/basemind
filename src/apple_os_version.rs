//! Keeps std's Apple OS-version check symbols alive in LTO release builds.
//!
//! ONNX Runtime's prebuilt static library for `aarch64-apple-darwin` (ort-sys 2.0.0-rc.15 on) has CoreML
//! code compiled with Clang's `@available`, which calls `__isPlatformVersionAtLeast`. Clang normally gets
//! it from `libclang_rt.osx.a`, but rustc links with `-nodefaultlibs`, so the definition comes from `std`
//! (rust-lang/rust#138944, weak since #146343). Thin LTO only keeps what Rust code uses: the reference
//! from the native objects is invisible to it, so it drops std's definition and the release link fails
//! with `Undefined symbols for architecture arm64: "___isPlatformVersionAtLeast"`. Debug builds (no LTO)
//! link, which is why the `cargo test` legs stayed green.
//!
//! A `#[used]` static that points at each symbol is a Rust use: LTO keeps the definition. Nothing calls them.
//! It lives in the library crate, so a downstream binary that links basemind with LTO keeps them too.

unsafe extern "C" {
    fn __isPlatformVersionAtLeast(platform: u32, major: u32, minor: u32, subminor: u32) -> i32;
    fn __isOSVersionAtLeast(major: i32, minor: i32, subminor: i32) -> i32;
}

#[used]
static KEEP_IS_PLATFORM_VERSION_AT_LEAST: unsafe extern "C" fn(u32, u32, u32, u32) -> i32 = __isPlatformVersionAtLeast;

#[used]
static KEEP_IS_OS_VERSION_AT_LEAST: unsafe extern "C" fn(i32, i32, i32) -> i32 = __isOSVersionAtLeast;
