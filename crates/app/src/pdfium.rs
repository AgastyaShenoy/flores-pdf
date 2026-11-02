//! Loading PDFium on Windows, for what hayro doesn't do: filling in forms, and writing form entries
//! and annotations into a PDF (or removing annotations from it). The rest is shared with the Android
//! app, in crates/pdfium-write. `pdfium.dll` (third_party/pdfium) is built into the exe, compressed,
//! and written to `%LOCALAPPDATA%\FloresPDF` the first time it's needed, with its license texts;
//! reading never loads it. A `pdfium.dll` beside the exe is used instead, if there is one.

pub use pdfium_write::*;
use std::path::PathBuf;
use std::sync::OnceLock;

/// PDFium and its license texts, deflated by the build script.
static DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pdfium.dll.deflate"));
static LICENSES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pdfium-licenses.txt.deflate"));
/// The PDFium build inside, which names the file it's written to.
const BUILD: &str = "7881";

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// PDFium, loaded on first use from `pdfium.dll` beside the exe.
pub fn get() -> Result<&'static Pdfium, String> {
    PDFIUM.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Pdfium, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let path = dll_path()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: a NUL-terminated absolute path, so no other folder is searched.
    let lib = unsafe { LoadLibraryW(wide.as_ptr()) };
    if lib.is_null() {
        return Err("pdfium.dll didn't load".to_owned());
    }
    let lookup = |name: &[u8]| unsafe { GetProcAddress(lib, name.as_ptr()) }.map_or(std::ptr::null(), |f| f as *const std::ffi::c_void);
    // SAFETY: the DLL is PDFium 7881, whose functions have the signatures pdfium-write expects.
    unsafe { Pdfium::new(&lookup) }
}

/// Where pdfium.dll is loaded from: beside the exe if it's there, otherwise the copy built into the
/// exe, written to `%LOCALAPPDATA%\FloresPDF` unless the file there already holds exactly it.
fn dll_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let beside = exe.with_file_name("pdfium.dll");
    if beside.exists() {
        return Ok(beside);
    }
    if DLL.is_empty() {
        return Err("this build of Flores PDF has no PDFium in it".to_owned());
    }
    let dll = miniz_oxide::inflate::decompress_to_vec(DLL).map_err(|_| "the PDFium inside Flores PDF is damaged".to_owned())?;
    let dir = cache_dir().ok_or("no local app data folder to put PDFium in")?;
    let path = dir.join(format!("pdfium-{BUILD}.dll"));
    if std::fs::read(&path).is_ok_and(|on_disk| on_disk == dll) {
        return Ok(path);
    }
    let failed = |e: std::io::Error| format!("couldn't write PDFium to {}: {e}", dir.display());
    std::fs::create_dir_all(&dir).map_err(failed)?;
    // written under another name first, so a half-written file is never loaded
    let part = dir.join(format!("pdfium-{BUILD}-{}.part", std::process::id()));
    std::fs::write(&part, &dll).map_err(failed)?;
    if let Ok(licenses) = miniz_oxide::inflate::decompress_to_vec(LICENSES) {
        let _ = std::fs::write(dir.join("pdfium-licenses.txt"), licenses);
    }
    match std::fs::rename(&part, &path) {
        Ok(()) => Ok(path),
        // another Flores PDF has a different file of that name loaded: use this one where it is
        Err(_) => Ok(part),
    }
}

/// `%LOCALAPPDATA%\FloresPDF`, for PDFium.
pub fn cache_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("FloresPDF"))
}
