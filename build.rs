//! Incrusta assets/app.manifest en el ejecutable (enlazador de MSVC).
fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("app.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    }
}
