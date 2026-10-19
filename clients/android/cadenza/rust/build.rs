//! Android's 16 KB memory pages (Android 15+ devices): every LOAD segment of libencore_cadenza.so is aligned to
//! 16 KB. NDK 29 with cargo-ndk already does this by default; the flag makes it explicit and independent of the
//! NDK and of RUSTFLAGS (a cdylib link argument is always passed). CI checks the result with `llvm-readelf -l`
//! (.github/workflows/apps.yml, android-native). Host builds (`cargo test`) are left alone.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-z,max-page-size=16384");
    }
}
