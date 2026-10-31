//! Embeds flores.manifest and the icon, flores.ico, into the exe (MSVC linker only), and PDFium
//! with its license texts, compressed, for the app to write out the first time it needs them.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=flores.manifest");
    println!("cargo:rerun-if-changed=flores.ico");
    println!("cargo:rerun-if-changed=../../third_party/pdfium");
    pdfium();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}\\flores.manifest");
    let ico = std::fs::read(Path::new(&dir).join("flores.ico")).expect("reading flores.ico");
    let res = Path::new(&std::env::var("OUT_DIR").unwrap()).join("flores.res");
    let mut data = icon_res(&ico);
    version_entry(&mut data, "Flores PDF", "FloresPDF.exe");
    std::fs::write(&res, data).expect("writing the icon resource");
    // the linker takes compiled resource files as they are
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// PDFium and its licenses, deflated, in OUT_DIR for `include_bytes!`; empty if PDFium isn't in
/// third_party, and the app then says so when it's needed.
fn pdfium() {
    let dir = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../third_party/pdfium");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).to_owned();
    let pack = |data: &[u8]| if data.is_empty() { Vec::new() } else { miniz_oxide::deflate::compress_to_vec(data, 9) };
    let dll = std::fs::read(dir.join("pdfium.dll")).unwrap_or_default();
    std::fs::write(out.join("pdfium.dll.deflate"), pack(&dll)).expect("writing PDFium");
    // the license texts one after another, each under its file's name
    let mut licenses = String::new();
    let mut files: Vec<_> = std::fs::read_dir(dir.join("licenses")).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    files.sort();
    files.insert(0, dir.join("LICENSE"));
    for f in files {
        if let Ok(text) = std::fs::read_to_string(&f) {
            let name = f.file_name().unwrap().to_string_lossy();
            licenses.push_str(&format!("==== {name} ====\n\n{}\n\n", text.trim_end()));
        }
    }
    let licenses = if dll.is_empty() { String::new() } else { licenses };
    std::fs::write(out.join("pdfium-licenses.txt.deflate"), pack(licenses.as_bytes())).expect("writing PDFium's licenses");
}

include!("res.rs");

