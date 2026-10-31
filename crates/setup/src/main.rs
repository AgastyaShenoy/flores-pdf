//! FloresPDF-Setup.exe: installs Flores PDF for the current user, without administrator rights.
//! It writes FloresPDF.exe (packed inside it) to `%LOCALAPPDATA%\Programs\Flores PDF`, runs it with
//! `--register` so Windows knows it (file types, Default apps, Start menu, Settings' list of apps),
//! then offers Windows' Default apps page, where making it the PDF reader takes one click: Windows
//! doesn't let programs make themselves the default. Installing again over an installed copy
//! updates it, even while it's open.
//!
//! `/S` installs without asking anything. The reader's updater adds `--after PID` (wait for that
//! copy to close first) and `--run` (start the new one afterwards). `FLORES_INSTALL_TEST` works as in
//! the app's install.rs.

#![windows_subsystem = "windows"]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_YESNO, MessageBoxW};

static APP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/FloresPDF.exe.deflate"));

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let quiet = args.iter().any(|a| a.eq_ignore_ascii_case("/S") || a == "--quiet");
    let run = args.iter().any(|a| a == "--run");
    if let Some(pid) = args.iter().position(|a| a == "--after").and_then(|i| args.get(i + 1)?.parse().ok()) {
        wait_for(pid);
    }
    let Some(dir) = install_dir() else {
        message("Couldn't find your local app data folder to install into.", MB_ICONERROR);
        std::process::exit(1);
    };
    let ask = format!(
        "Install Flores PDF?\n\nIt goes in {}, for you only, with an entry in the Start menu. \
         You can remove it from Windows' Settings, under Apps.",
        dir.display()
    );
    if !quiet && message(&ask, MB_YESNO | MB_ICONQUESTION) != IDYES {
        return;
    }
    let exe = dir.join("FloresPDF.exe");
    if let Err(e) = write_app(&dir, &exe) {
        message(&format!("Couldn't install Flores PDF in {}:\n\n{e}", dir.display()), MB_ICONERROR);
        std::process::exit(1);
    }
    let registered = std::process::Command::new(&exe).arg("--register").status().is_ok_and(|s| s.success());
    if !registered {
        message("Flores PDF is installed, but Windows couldn't be told about all of it.", MB_ICONERROR);
    }
    if quiet {
        if run {
            let _ = std::process::Command::new(&exe).spawn();
        }
        return;
    }
    let ask = "Flores PDF is installed.\n\nMake it your default PDF reader? Windows' Default apps page \
               opens on Flores PDF: click \"Set default\" there.";
    if message(ask, MB_YESNO | MB_ICONQUESTION) == IDYES {
        open("ms-settings:defaultapps?registeredAppUser=Flores%20PDF");
    }
    let _ = std::process::Command::new(&exe).spawn();
}

/// Waits up to a minute for process `pid` to end: the reader being updated.
fn wait_for(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a handle opened here, waited on and closed once.
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            WaitForSingleObject(process, 60_000);
            CloseHandle(process);
        }
    }
}

fn install_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("FLORES_INSTALL_TEST") {
        Some(root) => PathBuf::from(root),
        None => PathBuf::from(std::env::var_os("LOCALAPPDATA")?),
    };
    Some(base.join("Programs").join("Flores PDF"))
}

/// Writes the app as `exe`. A copy that's open can't be overwritten but can be renamed, so an
/// installed one is moved aside first and deleted once it's no longer running.
fn write_app(dir: &Path, exe: &Path) -> std::io::Result<()> {
    let app = miniz_oxide::inflate::decompress_to_vec(APP).map_err(|_| std::io::Error::other("the program inside setup is damaged"))?;
    std::fs::create_dir_all(dir)?;
    let new = dir.join("FloresPDF.exe.new");
    std::fs::write(&new, app)?;
    let old = dir.join("FloresPDF.exe.old");
    let _ = std::fs::remove_file(&old);
    if exe.exists() {
        std::fs::rename(exe, &old)?;
    }
    std::fs::rename(&new, exe)?;
    let _ = std::fs::remove_file(&old);
    Ok(())
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain([0]).collect()
}

fn message(text: &str, style: u32) -> i32 {
    let (text, title) = (wide(text), wide("Flores PDF Setup"));
    // SAFETY: NUL-terminated strings; no owner window.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), style) }
}

fn open(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, target) = (wide("open"), wide(target));
    // SAFETY: NUL-terminated strings; no owner window, parameters or directory.
    unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}
