/// Expose the compilation target triple as an environment variable at build time.
///
/// The `update` module uses `env!("TARGET")` to determine which release
/// asset to download for the current platform.
fn main() {
    println!(
        "cargo:rustc-env=TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
