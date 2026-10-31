//! Installing and removing Flores PDF for the current user. FloresPDF-Setup.exe puts the exe in
//! `%LOCALAPPDATA%\Programs\Flores PDF` and runs it with `--register`, which tells Windows about it:
//! it opens PDFs, it's offered in Default apps, it's in the Start menu, and it's in Settings' list
//! of apps, whose Uninstall runs it with `--uninstall`. Everything is per user, so no administrator
//! rights are needed.
//!
//! With the environment variable `FLORES_INSTALL_TEST` set to a folder, the program folder and the
//! Start menu entry go inside that folder and the registry values under `Software\FloresPDF-Test`,
//! where Windows doesn't look, so installing can be tested without changing the real ones.

use crate::win;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\FloresPDF";

enum Data {
    Text(String),
    Number(u32),
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain([0]).collect()
}

fn test_root() -> Option<PathBuf> {
    std::env::var_os("FLORES_INSTALL_TEST").map(PathBuf::from)
}

/// A key under HKEY_CURRENT_USER, moved aside when testing.
fn key(k: &str) -> String {
    match (test_root(), k.strip_prefix(r"Software\")) {
        (Some(_), Some(rest)) => format!(r"Software\FloresPDF-Test\{rest}"),
        _ => k.to_owned(),
    }
}

/// Where FloresPDF-Setup.exe installs to.
pub fn install_dir() -> Option<PathBuf> {
    let base = match test_root() {
        Some(root) => root,
        None => PathBuf::from(std::env::var_os("LOCALAPPDATA")?),
    };
    Some(base.join("Programs").join("Flores PDF"))
}

/// The Start menu entry.
fn shortcut_path() -> Option<PathBuf> {
    let programs = match test_root() {
        Some(root) => root.join("Start Menu"),
        None => PathBuf::from(std::env::var_os("APPDATA")?).join(r"Microsoft\Windows\Start Menu\Programs"),
    };
    Some(programs.join("Flores PDF.lnk"))
}

/// `--register`: tells Windows about this copy. Returns whether all of it was written.
pub fn register() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let dir = exe.parent().unwrap_or(Path::new("."));
    let exe_text = exe.display().to_string();
    let mut values: Vec<(String, &str, Data)> =
        win::association_values(&exe_text).into_iter().map(|(k, n, v)| (k, n, Data::Text(v))).collect();
    let size_kb = std::fs::metadata(&exe).map_or(0, |m| (m.len() / 1024) as u32);
    let uninstall = UNINSTALL_KEY.to_owned();
    values.extend([
        (uninstall.clone(), "DisplayName", Data::Text("Flores PDF".to_owned())),
        (uninstall.clone(), "DisplayIcon", Data::Text(format!("{exe_text},0"))),
        (uninstall.clone(), "DisplayVersion", Data::Text(env!("CARGO_PKG_VERSION").to_owned())),
        (uninstall.clone(), "Publisher", Data::Text("aga2522".to_owned())),
        (uninstall.clone(), "InstallLocation", Data::Text(dir.display().to_string())),
        (uninstall.clone(), "UninstallString", Data::Text(format!("\"{exe_text}\" --uninstall"))),
        (uninstall.clone(), "QuietUninstallString", Data::Text(format!("\"{exe_text}\" --uninstall --quiet"))),
        (uninstall.clone(), "NoModify", Data::Number(1)),
        (uninstall.clone(), "NoRepair", Data::Number(1)),
        (uninstall, "EstimatedSize", Data::Number(size_kb)),
    ]);
    let mut ok = values.iter().all(|(k, n, v)| set(k, n, v));
    ok &= shortcut_path().is_some_and(|lnk| {
        lnk.parent().is_some_and(|p| std::fs::create_dir_all(p).is_ok()) && create_shortcut(&lnk, &exe)
    });
    notify_associations();
    ok
}

/// `--uninstall`: asks first unless `quiet`, then takes back what `register` and setup did. The
/// program folder is emptied only when this is the installed copy; the files can't be deleted while
/// this process runs, so a hidden command prompt deletes them a moment after it ends.
pub fn uninstall(quiet: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO};
    if !quiet && message("Remove Flores PDF from this computer?", MB_YESNO | MB_ICONQUESTION) != IDYES {
        return;
    }
    while !quiet && win::other_window_open() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IDRETRY, MB_ICONWARNING, MB_RETRYCANCEL};
        if message("Flores PDF is still open. Close it, then press Retry.", MB_RETRYCANCEL | MB_ICONWARNING) != IDRETRY {
            return;
        }
    }
    let classes = r"Software\Classes";
    for (ext, prog_id, _) in win::FILE_TYPES {
        delete_tree(&key(&format!(r"{classes}\{prog_id}")));
        delete_value(&key(&format!(r"{classes}\{ext}\OpenWithProgids")), prog_id);
    }
    for k in [format!(r"{classes}\Applications\FloresPDF.exe"), r"Software\FloresPDF".to_owned(), UNINSTALL_KEY.to_owned()] {
        delete_tree(&key(&k));
    }
    delete_value(&key(r"Software\RegisteredApplications"), "Flores PDF");
    if let Some(lnk) = shortcut_path() {
        let _ = std::fs::remove_file(lnk);
    }
    if let Some(cache) = crate::pdfium::cache_dir().filter(|_| test_root().is_none()) {
        let _ = std::fs::remove_dir_all(cache);
    }
    notify_associations();
    if !quiet {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO};
        let data = crate::settings::path();
        let in_appdata = data.file_name().is_some_and(|n| n == "settings.toml");
        if in_appdata
            && let Some(folder) = data.parent()
            && folder.exists()
            && message(
                "Also delete your settings, recent files, favorites and highlights?",
                MB_YESNO | MB_ICONQUESTION,
            ) == IDYES
        {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let installed = install_dir().is_some_and(|d| exe.parent().is_some_and(|p| crate::history::same_file(p, &d)));
    if installed && let Some(dir) = exe.parent() {
        delete_after_exit(dir);
    }
    if !quiet {
        message("Flores PDF was removed.", 0);
    }
}

/// Deletes the program folder's files, then the folder, once this process has ended.
fn delete_after_exit(dir: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let d = dir.display();
    // only the files setup and the reader put there, so nothing else of the user's goes with them
    let script = format!(
        "/d /c ping -n 3 127.0.0.1 >nul & del /f /q \"{d}\\FloresPDF.exe\" \"{d}\\FloresPDF.exe.old\" \"{d}\\FloresPDF.exe.new\" & rd \"{d}\""
    );
    let temp = std::env::temp_dir();
    let _ = std::process::Command::new("cmd.exe").raw_arg(script).current_dir(temp).creation_flags(CREATE_NO_WINDOW).spawn();
}

fn message(text: &str, style: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
    let (text, title) = (wide(text), wide("Flores PDF"));
    // SAFETY: NUL-terminated strings; no owner window.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), style) }
}

fn set(k: &str, name: &str, data: &Data) -> bool {
    use windows_sys::Win32::System::Registry::{REG_DWORD, REG_SZ, RegSetKeyValueW};
    let (k, name) = (wide(&key(k)), wide(name));
    // SAFETY: NUL-terminated strings; each size is its data's, a string's terminator included.
    let status = unsafe {
        match data {
            Data::Text(s) => {
                let s = wide(s);
                RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), name.as_ptr(), REG_SZ, s.as_ptr().cast(), (s.len() * 2) as u32)
            }
            Data::Number(n) => RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), name.as_ptr(), REG_DWORD, (n as *const u32).cast(), 4),
        }
    };
    status == 0
}

fn delete_tree(k: &str) {
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;
    let k = wide(k);
    // SAFETY: a NUL-terminated key name under HKEY_CURRENT_USER.
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
}

fn delete_value(k: &str, name: &str) {
    use windows_sys::Win32::System::Registry::RegDeleteKeyValueW;
    let (k, name) = (wide(k), wide(name));
    // SAFETY: NUL-terminated strings.
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), name.as_ptr()) };
}

fn notify_associations() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    if test_root().is_none() {
        // SAFETY: no items with this event.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
    }
}

/// Writes a shortcut to `target` at `lnk` with the shell's ShellLink object, through its COM
/// interfaces called by their place in the vtable: windows-sys has no COM wrappers.
fn create_shortcut(lnk: &Path, target: &Path) -> bool {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
    use windows_sys::core::GUID;
    const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
    const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
    // IShellLinkW: SetDescription, SetWorkingDirectory, SetIconLocation, SetPath; IPersistFile: Save
    const SET_DESCRIPTION: usize = 7;
    const SET_WORKING_DIRECTORY: usize = 9;
    const SET_ICON_LOCATION: usize = 17;
    const SET_PATH: usize = 20;
    const SAVE: usize = 6;
    type SetText = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;
    type SetIcon = unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32;
    type Save = unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32;
    type QueryInterface = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    let (path, dir, desc, file) = (
        wide(&target.display().to_string()),
        wide(&target.parent().unwrap_or(Path::new(".")).display().to_string()),
        wide("A light, fast PDF reader"),
        wide(&lnk.display().to_string()),
    );
    // SAFETY: COM objects used through their documented vtables; each pointer is released once.
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut link: *mut c_void = std::ptr::null_mut();
        if CoCreateInstance(&CLSID_SHELL_LINK, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELL_LINK_W, &mut link) != 0 {
            return false;
        }
        let method = |object: *mut c_void, i: usize| *(*(object as *const *const usize)).add(i);
        let set_text = |i: usize, s: &[u16]| std::mem::transmute::<usize, SetText>(method(link, i))(link, s.as_ptr()) == 0;
        let mut ok = set_text(SET_PATH, &path) && set_text(SET_WORKING_DIRECTORY, &dir) && set_text(SET_DESCRIPTION, &desc);
        ok &= std::mem::transmute::<usize, SetIcon>(method(link, SET_ICON_LOCATION))(link, path.as_ptr(), 0) == 0;
        let mut persist: *mut c_void = std::ptr::null_mut();
        ok &= std::mem::transmute::<usize, QueryInterface>(method(link, 0))(link, &IID_IPERSIST_FILE, &mut persist) == 0;
        if !persist.is_null() {
            ok &= std::mem::transmute::<usize, Save>(method(persist, SAVE))(persist, file.as_ptr(), 1) == 0;
            std::mem::transmute::<usize, Release>(method(persist, 2))(persist);
        }
        std::mem::transmute::<usize, Release>(method(link, 2))(link);
        ok
    }
}
