//! Packs FloresPDF.exe, from the same target folder and profile, deflated, for setup to write out;
//! embeds setup.manifest and the app's icon (MSVC linker only). Build flores-pdf first:
//! `cargo build --release -p flores-pdf` then `cargo build --release -p flores-setup`.

use std::path::Path;

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = std::env::var("OUT_DIR").unwrap();
    // OUT_DIR is target/<profile>/build/<crate>/out
    let exe = Path::new(&out).ancestors().nth(3).unwrap().join("FloresPDF.exe");
    println!("cargo:rerun-if-changed={}", exe.display());
    println!("cargo:rerun-if-changed=setup.manifest");
    println!("cargo:rerun-if-changed=../app/flores.ico");
    let app = std::fs::read(&exe).unwrap_or_else(|_| panic!("{} isn't there: build flores-pdf first", exe.display()));
    std::fs::write(Path::new(&out).join("FloresPDF.exe.deflate"), miniz_oxide::deflate::compress_to_vec(&app, 9)).expect("writing the packed app");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}\\setup.manifest");
    let ico = std::fs::read(Path::new(&dir).join("../app/flores.ico")).expect("reading flores.ico");
    let res = Path::new(&out).join("setup.res");
    let mut data = icon_res(&ico);
    version_entry(&mut data, "Flores PDF Setup", "FloresPDF-Setup.exe");
    std::fs::write(&res, data).expect("writing the icon resource");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

include!("../app/res.rs");
