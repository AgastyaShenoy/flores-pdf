//! The few Windows calls the UI needs: the open-file dialog, the system's light/dark setting,
//! opening a file in its default program, and handing files to a reader window that's already open.

use std::cell::RefCell;
use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, FindWindowW, GWLP_WNDPROC, IsIconic, SMTO_ABORTIFHUNG, SW_RESTORE, SW_SHOWNORMAL, SendMessageTimeoutW,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, WM_COPYDATA, WNDPROC,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

fn hwnd(window: &Window) -> HWND {
    match window.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get() as HWND,
        _ => std::ptr::null_mut(),
    }
}

/// Asks for a document to open. Blocks until the dialog closes.
pub fn open_dialog(owner: &Window) -> Option<PathBuf> {
    let filter: Vec<u16> = concat!(
        "Documents, books, comics and pictures\0*.pdf;*.epub;*.djvu;*.djv;*.cbz;*.cbr;*.cb7;*.zip;*.rar;*.7z;*.jpg;*.jpeg;*.png;*.gif;*.webp\0",
        "PDF documents (*.pdf)\0*.pdf\0",
        "E-books (*.epub)\0*.epub\0",
        "DjVu documents (*.djvu)\0*.djvu;*.djv\0",
        "Comics (*.cbz, *.cbr, *.cb7)\0*.cbz;*.cbr;*.cb7;*.zip;*.rar;*.7z\0",
        "Pictures (*.jpg, *.png, *.gif, *.webp)\0*.jpg;*.jpeg;*.png;*.gif;*.webp\0",
        "All files (*.*)\0*.*\0\0"
    )
    .encode_utf16()
    .collect();
    let mut file = vec![0u16; 32 * 1024];
    // SAFETY: an all-zero OPENFILENAMEW is valid; the fields set below point at buffers that
    // outlive the call.
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = hwnd(owner);
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER;
    // SAFETY: see above.
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(0);
    Some(OsString::from_wide(&file[..len]).into())
}

/// Whether Windows is set to dark mode for apps.
pub fn system_dark() -> bool {
    let key = wide(OsStr::new("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"));
    let name = wide(OsStr::new("AppsUseLightTheme"));
    let mut light: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the strings are NUL-terminated and `light` has room for the DWORD asked for.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut light).cast::<c_void>(),
            &mut size,
        )
    };
    status == 0 && light == 0
}

/// Paints the title bar `rgb` (0xRRGGBB), or Windows' own color for the theme with `None`.
/// Windows 11 only; earlier versions ignore it.
pub fn caption_color(window: &Window, rgb: Option<u32>) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CAPTION_COLOR, DwmSetWindowAttribute};
    // a COLORREF is 0x00BBGGRR; 0xFFFFFFFF is DWMWA_COLOR_DEFAULT
    let color: u32 = rgb.map_or(0xFFFF_FFFF, |c| ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF));
    // SAFETY: the attribute takes a COLORREF, which `color` is, by pointer and size.
    unsafe {
        DwmSetWindowAttribute(hwnd(window), DWMWA_CAPTION_COLOR as u32, (&raw const color).cast::<c_void>(), size_of::<u32>() as u32)
    };
}

/// Puts `text` on the clipboard. Returns whether it worked (another program can hold the clipboard).
pub fn copy_text(owner: &Window, text: &str) -> bool {
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
    // Windows line breaks, NUL-terminated UTF-16
    let bytes: Vec<u8> = text.replace('\n', "\r\n").encode_utf16().chain([0]).flat_map(u16::to_ne_bytes).collect();
    set_clipboard(owner, u32::from(CF_UNICODETEXT), &bytes)
}

/// The text on the clipboard, if there is any.
pub fn paste_text(owner: &Window) -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
    // SAFETY: the clipboard's memory is only read while locked, and within its size.
    unsafe {
        if OpenClipboard(hwnd(owner)) == 0 {
            return None;
        }
        let mut text = None;
        let mem = GetClipboardData(u32::from(CF_UNICODETEXT));
        if !mem.is_null() {
            let data = GlobalLock(mem).cast::<u16>();
            if !data.is_null() {
                let units = std::slice::from_raw_parts(data, GlobalSize(mem) / 2);
                let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
                text = Some(String::from_utf16_lossy(&units[..end]).replace("\r\n", "\n"));
                GlobalUnlock(mem);
            }
        }
        CloseClipboard();
        text
    }
}

pub enum Answer {
    Yes,
    No,
    Cancel,
}

/// Asks a yes, no or cancel question in a Windows message box over the window.
pub fn ask(owner: &Window, text: &str) -> Answer {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDNO, IDYES, MB_ICONWARNING, MB_YESNOCANCEL, MessageBoxW};
    let (text, caption) = (wide(OsStr::new(text)), wide(OsStr::new("Flores PDF")));
    // SAFETY: NUL-terminated strings and our own window.
    match unsafe { MessageBoxW(hwnd(owner), text.as_ptr(), caption.as_ptr(), MB_YESNOCANCEL | MB_ICONWARNING) } {
        IDYES => Answer::Yes,
        IDNO => Answer::No,
        _ => Answer::Cancel,
    }
}

/// Puts an image of 0x00RRGGBB pixels on the clipboard, as a device-independent bitmap, which
/// every Windows program that takes pasted images understands.
pub fn copy_image(owner: &Window, width: u32, height: u32, dpi: u32, pixels: &[u32]) -> bool {
    use windows_sys::Win32::System::Ole::CF_DIB;
    set_clipboard(owner, u32::from(CF_DIB), &dib(width, height, dpi, pixels))
}

/// A BITMAPINFOHEADER, then 24-bit rows bottom to top, each padded to 4 bytes.
fn dib(width: u32, height: u32, dpi: u32, pixels: &[u32]) -> Vec<u8> {
    let row = (width as usize * 3).next_multiple_of(4);
    let per_meter = (f64::from(dpi) / 0.0254).round() as i32;
    let mut data = Vec::with_capacity(40 + row * height as usize);
    data.extend(40u32.to_le_bytes());
    data.extend((width as i32).to_le_bytes());
    data.extend((height as i32).to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(24u16.to_le_bytes());
    data.extend(0u32.to_le_bytes()); // BI_RGB
    data.extend(((row * height as usize) as u32).to_le_bytes());
    data.extend(per_meter.to_le_bytes());
    data.extend(per_meter.to_le_bytes());
    data.extend([0u8; 8]);
    data.extend(dib_rows(width, pixels));
    data
}

/// 0x00RRGGBB pixels as a 24-bit bitmap's rows: bottom to top, blue-green-red, each row padded to
/// 4 bytes.
fn dib_rows(width: u32, pixels: &[u32]) -> Vec<u8> {
    let row = (width as usize * 3).next_multiple_of(4);
    let mut data = Vec::with_capacity(row * pixels.len() / (width as usize).max(1));
    for line in pixels.chunks_exact(width as usize).rev() {
        let start = data.len();
        data.extend(line.iter().flat_map(|p| [*p as u8, (p >> 8) as u8, (p >> 16) as u8]));
        data.resize(start + row, 0);
    }
    data
}

/// A printer picked in the print dialog, ready to print on, and the pages asked for (from 0).
pub struct Printer {
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    pub pages: std::ops::RangeInclusive<usize>,
}

// SAFETY: a printer's device context isn't tied to the thread that made it; the print thread is the
// only one that uses it.
unsafe impl Send for Printer {}

/// Shows Windows' print dialog for a document of `pages` pages, on page `current` now.
pub fn print_dialog(owner: &Window, pages: usize, current: usize) -> Option<Printer> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        PD_CURRENTPAGE, PD_NOSELECTION, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW, PrintDlgW,
    };
    if pages == 0 {
        return None;
    }
    // SAFETY: an all-zero PRINTDLGW is valid; the fields set below are what the dialog needs.
    let mut pd: PRINTDLGW = unsafe { std::mem::zeroed() };
    pd.lStructSize = size_of::<PRINTDLGW>() as u32;
    pd.hwndOwner = hwnd(owner);
    pd.Flags = PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE | PD_NOSELECTION;
    let last = pages.min(usize::from(u16::MAX)) as u16;
    (pd.nMinPage, pd.nMaxPage, pd.nFromPage, pd.nToPage, pd.nCopies) = (1, last, 1, last, 1);
    // SAFETY: see above. The dialog allocates the devmode and names, which are freed here.
    let ok = unsafe { PrintDlgW(&mut pd) } != 0;
    unsafe {
        for h in [pd.hDevMode, pd.hDevNames] {
            if !h.is_null() {
                GlobalFree(h);
            }
        }
    }
    if !ok || pd.hDC.is_null() {
        return None;
    }
    let range = if pd.Flags & PD_PAGENUMS != 0 {
        let from = usize::from(pd.nFromPage.max(1)) - 1;
        from..=(usize::from(pd.nToPage.max(1)) - 1).max(from)
    } else if pd.Flags & PD_CURRENTPAGE != 0 {
        current..=current
    } else {
        0..=pages - 1
    };
    Some(Printer { dc: pd.hDC, pages: range })
}

/// The printer named `name`, or Windows' default printer, to print all of a document's `pages` on
/// without asking: for the -print-to command line flags.
pub fn printer_named(name: Option<&str>, pages: usize) -> Option<Printer> {
    use windows_sys::Win32::Graphics::Gdi::CreateDCW;
    let name = match name {
        Some(n) => n.to_owned(),
        None => default_printer()?,
    };
    let (driver, device) = (wide(OsStr::new("WINSPOOL")), wide(OsStr::new(&name)));
    // SAFETY: NUL-terminated names; no port or settings, so the printer's own are used.
    let dc = unsafe { CreateDCW(driver.as_ptr(), device.as_ptr(), std::ptr::null(), std::ptr::null()) };
    (!dc.is_null() && pages > 0).then(|| Printer { dc, pages: 0..=pages - 1 })
}

fn default_printer() -> Option<String> {
    use windows_sys::Win32::Graphics::Printing::GetDefaultPrinterW;
    let mut name = vec![0u16; 512];
    let mut len = name.len() as u32;
    // SAFETY: `len` is the buffer's length in characters.
    if unsafe { GetDefaultPrinterW(name.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    let end = name.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&name[..end]))
}

/// A message box, for errors when there's no reader window to show them in.
pub fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let (text, caption) = (wide(OsStr::new(text)), wide(OsStr::new("Flores PDF")));
    // SAFETY: NUL-terminated strings; no owner window.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR) };
}

impl Printer {
    /// The printable area in device pixels, and the printer's dots per inch across and down.
    pub fn area(&self) -> (u32, u32, u32, u32) {
        use windows_sys::Win32::Graphics::Gdi::{GetDeviceCaps, HORZRES, LOGPIXELSX, LOGPIXELSY, VERTRES};
        // SAFETY: a valid printer DC.
        let cap = |i| unsafe { GetDeviceCaps(self.dc, i as i32) }.max(1) as u32;
        (cap(HORZRES), cap(VERTRES), cap(LOGPIXELSX), cap(LOGPIXELSY))
    }

    /// Starts the print job, named `name` in the printer's queue.
    pub fn start(&self, name: &str) -> bool {
        use windows_sys::Win32::Storage::Xps::{DOCINFOW, StartDocW};
        let name = wide(OsStr::new(name));
        // SAFETY: an all-zero DOCINFOW is valid; the name outlives the call.
        let mut info: DOCINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<DOCINFOW>() as i32;
        info.lpszDocName = name.as_ptr();
        unsafe { StartDocW(self.dc, &info) > 0 }
    }

    pub fn start_page(&self) -> bool {
        // SAFETY: a valid printer DC with a job started.
        unsafe { windows_sys::Win32::Storage::Xps::StartPage(self.dc) > 0 }
    }

    /// Prints a block of 0x00RRGGBB pixels, `width` wide, stretched to `w` × `h` device pixels at
    /// (x, y): printers whose dots aren't square get the block stretched one way.
    pub fn draw(&self, x: i32, y: i32, w: i32, h: i32, width: u32, pixels: &[u32]) -> bool {
        use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, SRCCOPY, StretchDIBits};
        let height = (pixels.len() / width.max(1) as usize) as i32;
        let rows = dib_rows(width, pixels);
        // SAFETY: an all-zero BITMAPINFO is valid; the header describes `rows` exactly.
        let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: height,
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB,
            ..unsafe { std::mem::zeroed() }
        };
        let lines = unsafe {
            StretchDIBits(self.dc, x, y, w, h, 0, 0, width as i32, height, rows.as_ptr().cast(), &bmi, DIB_RGB_COLORS, SRCCOPY)
        };
        lines > 0
    }

    pub fn end_page(&self) -> bool {
        // SAFETY: a valid printer DC with a page started.
        unsafe { windows_sys::Win32::Storage::Xps::EndPage(self.dc) > 0 }
    }

    pub fn end(&self) -> bool {
        // SAFETY: a valid printer DC with a job started.
        unsafe { windows_sys::Win32::Storage::Xps::EndDoc(self.dc) > 0 }
    }

    /// Stops the job; what was sent so far is thrown away.
    pub fn abort(&self) {
        // SAFETY: a valid printer DC.
        unsafe { windows_sys::Win32::Storage::Xps::AbortDoc(self.dc) };
    }
}

impl Drop for Printer {
    fn drop(&mut self) {
        // SAFETY: the DC came from the print dialog and isn't used after this.
        unsafe { windows_sys::Win32::Graphics::Gdi::DeleteDC(self.dc) };
    }
}

fn set_clipboard(owner: &Window, format: u32, bytes: &[u8]) -> bool {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    // SAFETY: the memory is allocated with the size written to it, and handed to the clipboard,
    // which owns it from then on. Every call is checked before the next one relies on it.
    unsafe {
        if OpenClipboard(hwnd(owner)) == 0 {
            return false;
        }
        let mut ok = false;
        if EmptyClipboard() != 0 {
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if !mem.is_null() {
                let dst = GlobalLock(mem).cast::<u8>();
                if !dst.is_null() {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
                    GlobalUnlock(mem);
                    ok = !SetClipboardData(format, mem).is_null();
                }
                if !ok {
                    windows_sys::Win32::Foundation::GlobalFree(mem);
                }
            }
        }
        CloseClipboard();
        ok
    }
}

/// Asks where to save a file of the kind `filter` describes ("PNG image (*.png)\0*.png\0\0"),
/// suggesting `name` and adding `extension` if none is typed. Blocks until the dialog closes.
pub fn save_dialog(owner: &Window, name: &str, filter: &str, extension: &str) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{GetSaveFileNameW, OFN_NOREADONLYRETURN, OFN_OVERWRITEPROMPT};
    let filter: Vec<u16> = filter.encode_utf16().collect();
    let extension = wide(OsStr::new(extension));
    let mut file = vec![0u16; 32 * 1024];
    for (dst, src) in file.iter_mut().zip(name.encode_utf16().take(1000)) {
        *dst = src;
    }
    // SAFETY: as in `open_dialog`.
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = hwnd(owner);
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrDefExt = extension.as_ptr();
    ofn.Flags = OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT | OFN_NOREADONLYRETURN | OFN_EXPLORER;
    // SAFETY: see above.
    if unsafe { GetSaveFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(0);
    Some(OsString::from_wide(&file[..len]).into())
}

/// Opens an Explorer window on the folder holding `path`, with the file selected.
pub fn show_in_folder(path: &Path) {
    use std::os::windows::process::CommandExt;
    let windows = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    // Explorer reads `/select,"path"` itself rather than as a quoted argument; file names can't
    // contain quotes
    let _ = std::process::Command::new(windows.join("explorer.exe")).raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

/// Waits until the desktop shows its next frame, so steps of an animation land one per frame.
pub fn wait_for_frame() {
    // SAFETY: no arguments; it only blocks.
    unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmFlush();
    }
}

/// Opens a link from a document in the browser or mail program. Only web and mail links: a PDF
/// can point at files, programs and scripts too, and those aren't followed. A bare address such as
/// "example.org" is taken as a web link. Returns whether it opened anything.
pub fn open_link(uri: &str) -> bool {
    let Some(url) = web_link(uri) else { return false };
    open_in_default_program(Path::new(&url));
    true
}

/// The address to open for a link, if it's one that may be opened.
fn web_link(uri: &str) -> Option<String> {
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        Some(uri.to_owned())
    } else if !lower.contains(':') && !lower.contains(' ') && !lower.contains('\\') && lower.contains('.') {
        Some(format!("https://{uri}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{dib, web_link};

    #[test]
    fn clipboard_image_layout() {
        // 2 × 2: red, green on top; blue, white below. Rows of 6 bytes padded to 8, bottom first.
        let d = dib(2, 2, 150, &[0xFF0000, 0x00FF00, 0x0000FF, 0xFFFFFF]);
        assert_eq!(d.len(), 40 + 2 * 8);
        assert_eq!(&d[0..4], &40u32.to_le_bytes());
        assert_eq!(&d[4..12], &[2, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(&d[14..16], &24u16.to_le_bytes());
        assert_eq!(&d[24..28], &5906u32.to_le_bytes()); // 150 dpi in pixels per meter
        assert_eq!(&d[40..48], &[0xFF, 0, 0, 0xFF, 0xFF, 0xFF, 0, 0]);
        assert_eq!(&d[48..56], &[0, 0, 0xFF, 0, 0xFF, 0, 0, 0]);
    }

    #[test]
    fn only_web_and_mail_links_open() {
        assert_eq!(web_link("https://github.com/RUCAIBox/LLMSurvey").as_deref(), Some("https://github.com/RUCAIBox/LLMSurvey"));
        assert_eq!(web_link("lmbook-zh.github.io").as_deref(), Some("https://lmbook-zh.github.io"));
        assert_eq!(web_link("mailto:someone@example.org").as_deref(), Some("mailto:someone@example.org"));
        for blocked in ["file:///C:/Windows/notepad.exe", "javascript:alert(1)", "C:\\Windows\\notepad.exe", "\\\\server\\share\\x.exe", "notes"] {
            assert_eq!(web_link(blocked), None, "{blocked}");
        }
    }
}

/// Opens `path` the way double-clicking it would.
pub fn open_in_default_program(path: &Path) {
    let verb = wide(OsStr::new("open"));
    let file = wide(path.as_os_str());
    // SAFETY: the strings are NUL-terminated and live through the call.
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

/// The reader window's class name, which another copy of the reader looks for to hand files to.
pub const CLASS: &str = "FloresPDF";
/// Marks a WM_COPYDATA message as ours.
const HANDOVER: usize = 0x5044_4652;
/// The most a handover may carry, in UTF-16 units.
const MAX_HANDOVER: usize = 64 * 1024;

/// Held by a copy of the reader that's starting up, until its window can take files from others.
/// Opening several files from Explorer starts several copies at once, and without this each would
/// find no window yet and open its own.
pub struct StartLock(HANDLE);

impl Drop for StartLock {
    fn drop(&mut self) {
        // SAFETY: the handle is a mutex this thread owns.
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

pub enum Start {
    /// A reader window took the files: this copy can exit.
    HandedOver,
    /// No reader window is open, or it didn't answer: open one, and hold the lock until it listens.
    Alone(Option<StartLock>),
}

/// Gives the command line `args` (with absolute paths) to a reader window that's already open, and
/// brings that window to the front.
pub fn hand_over(args: &[OsString]) -> Start {
    let name = wide(OsStr::new(r"Local\FloresPDF-start"));
    // SAFETY: the name is NUL-terminated; no security attributes means the defaults.
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    let lock = if mutex.is_null() {
        None
    } else {
        // a copy starting up gets a few seconds to show its window; after that, go ahead regardless
        // SAFETY: a valid mutex handle.
        match unsafe { WaitForSingleObject(mutex, 5000) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Some(StartLock(mutex)),
            _ => {
                // SAFETY: as above, and not used after.
                unsafe { CloseHandle(mutex) };
                None
            }
        }
    };
    let class = wide(OsStr::new(CLASS));
    // SAFETY: a NUL-terminated class name, and no window title to match.
    let window = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if window.is_null() {
        return Start::Alone(lock);
    }
    let data: Vec<u16> = args.iter().flat_map(|a| a.encode_wide().chain([0])).collect();
    let message = COPYDATASTRUCT { dwData: HANDOVER, cbData: (data.len() * 2) as u32, lpData: data.as_ptr() as *mut c_void };
    let mut answer = 0;
    // SAFETY: `message` and `data` outlive the call, which copies them to the other process.
    let sent = unsafe {
        SendMessageTimeoutW(window, WM_COPYDATA, 0, &raw const message as LPARAM, SMTO_ABORTIFHUNG, 5000, &mut answer)
    };
    if sent == 0 || answer == 0 {
        // hung, or not a reader window after all
        return Start::Alone(lock);
    }
    // SAFETY: plain calls on a window handle; they fail harmlessly if it has just closed.
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window);
    }
    Start::HandedOver
}

thread_local! {
    static ON_HANDOVER: RefCell<Option<Box<dyn Fn(Vec<OsString>)>>> = const { RefCell::new(None) };
}
static PREVIOUS_PROC: AtomicIsize = AtomicIsize::new(0);

/// Calls `f` with the arguments another copy of the reader hands over. Call once, from the thread
/// that made the window.
pub fn listen_for_handover(window: &Window, f: impl Fn(Vec<OsString>) + 'static) {
    let hwnd = hwnd(window);
    if hwnd.is_null() {
        return;
    }
    ON_HANDOVER.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    // SAFETY: the window belongs to this thread, and `relay` passes every other message on to the
    // procedure it replaces.
    let previous = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, relay as *const () as isize) };
    PREVIOUS_PROC.store(previous, Ordering::Relaxed);
}

/// Window procedure in front of winit's: takes handovers, passes everything else on.
unsafe extern "system" fn relay(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        // SAFETY: for WM_COPYDATA, lparam points at a COPYDATASTRUCT that's valid during the call.
        let message = unsafe { &*(lparam as *const COPYDATASTRUCT) };
        let len = message.cbData as usize;
        if message.dwData == HANDOVER && len % 2 == 0 && len / 2 <= MAX_HANDOVER {
            let bytes = if len == 0 || message.lpData.is_null() {
                &[][..]
            } else {
                // SAFETY: the system copied `len` bytes there for this call.
                unsafe { std::slice::from_raw_parts(message.lpData as *const u8, len) }
            };
            let units: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_ne_bytes([b[0], b[1]])).collect();
            let args = units.split(|&c| c == 0).filter(|a| !a.is_empty()).map(OsString::from_wide).collect();
            ON_HANDOVER.with(|h| {
                if let Some(f) = &*h.borrow() {
                    f(args);
                }
            });
            return 1;
        }
    }
    // SAFETY: PREVIOUS_PROC holds the procedure `listen_for_handover` replaced.
    unsafe {
        let previous = std::mem::transmute::<isize, WNDPROC>(PREVIOUS_PROC.load(Ordering::Relaxed));
        CallWindowProcW(previous, hwnd, msg, wparam, lparam)
    }
}

/// The file types Flores PDF registers for, with the program identifier each gets and its name in
/// Explorer. Windows' Default apps lists them all under Flores PDF.
pub const FILE_TYPES: [(&str, &str, &str); 7] = [
    (".pdf", "FloresPDF.pdf", "PDF Document"),
    (".epub", "FloresPDF.epub", "EPUB Book"),
    (".djvu", "FloresPDF.djvu", "DjVu Document"),
    (".djv", "FloresPDF.djvu", "DjVu Document"),
    (".cbz", "FloresPDF.cbz", "Comic Book (zip)"),
    (".cbr", "FloresPDF.cbr", "Comic Book (RAR)"),
    (".cb7", "FloresPDF.cb7", "Comic Book (7-Zip)"),
];
const APP_NAME: &str = "Flores PDF";

/// The registry values, under HKEY_CURRENT_USER, that make Windows offer Flores PDF for its file
/// types: (key, value name, data); an empty name is the key's default value. Per user, so no admin.
pub fn association_values(exe: &str) -> Vec<(String, &'static str, String)> {
    let open = format!("\"{exe}\" \"%1\"");
    let classes = r"Software\Classes";
    let mut values = Vec::new();
    for (ext, prog_id, name) in FILE_TYPES {
        values.extend([
            (format!(r"{classes}\{prog_id}"), "", name.to_owned()),
            (format!(r"{classes}\{prog_id}\DefaultIcon"), "", format!("\"{exe}\",0")),
            // the name and icon Windows shows for this file type's program in its app lists
            (format!(r"{classes}\{prog_id}\Application"), "ApplicationName", APP_NAME.to_owned()),
            (format!(r"{classes}\{prog_id}\Application"), "ApplicationCompany", "aga2522".to_owned()),
            (format!(r"{classes}\{prog_id}\Application"), "ApplicationIcon", format!("\"{exe}\",0")),
            (format!(r"{classes}\{prog_id}\shell\open\command"), "", open.clone()),
            (format!(r"{classes}\{ext}\OpenWithProgids"), prog_id, String::new()),
            (format!(r"{classes}\Applications\FloresPDF.exe\SupportedTypes"), ext, String::new()),
            (r"Software\FloresPDF\Capabilities\FileAssociations".to_owned(), ext, prog_id.to_owned()),
        ]);
    }
    values.extend([
        (format!(r"{classes}\Applications\FloresPDF.exe"), "FriendlyAppName", APP_NAME.to_owned()),
        (format!(r"{classes}\Applications\FloresPDF.exe\shell\open\command"), "", open),
        (r"Software\FloresPDF\Capabilities".to_owned(), "ApplicationName", APP_NAME.to_owned()),
        (r"Software\FloresPDF\Capabilities".to_owned(), "ApplicationDescription", "A light, fast PDF reader".to_owned()),
        (r"Software\FloresPDF\Capabilities".to_owned(), "ApplicationIcon", format!("\"{exe}\",0")),
        (r"Software\RegisteredApplications".to_owned(), APP_NAME, r"Software\FloresPDF\Capabilities".to_owned()),
    ]);
    values
}

/// Whether a Flores PDF window is open, in this or another process.
pub fn other_window_open() -> bool {
    let class = wide(OsStr::new(CLASS));
    // SAFETY: a NUL-terminated class name, and no window title to match.
    !unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) }.is_null()
}

/// Tells Windows that Flores PDF opens PDFs, for the current user, then opens Windows' Default
/// apps page on Flores PDF, where the user makes it the default: Windows doesn't let a program do
/// that itself. Returns whether every value was written.
pub fn register_as_pdf_reader() -> bool {
    use windows_sys::Win32::System::Registry::{REG_SZ, RegSetKeyValueW};
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut ok = true;
    for (key, name, data) in association_values(&exe.display().to_string()) {
        let (key, name, data) = (wide(OsStr::new(&key)), wide(OsStr::new(name)), wide(OsStr::new(&data)));
        // SAFETY: NUL-terminated strings; the size given is the data's, terminator included.
        let status = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
        };
        ok &= status == 0;
    }
    // SAFETY: no items with this event.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
    open_in_default_program(Path::new(&format!("ms-settings:defaultapps?registeredAppUser={APP_NAME}")));
    ok
}

/// Whether double-clicking a PDF opens this copy of Flores PDF.
pub fn is_default_pdf_reader() -> bool {
    use windows_sys::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_EXECUTABLE, AssocQueryStringW};
    let (ext, verb) = (wide(OsStr::new(".pdf")), wide(OsStr::new("open")));
    let mut out = vec![0u16; 1024];
    let mut len = out.len() as u32;
    // SAFETY: NUL-terminated inputs; `len` is the output buffer's length in characters.
    let found = unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, ext.as_ptr(), verb.as_ptr(), out.as_mut_ptr(), &mut len) };
    if found != 0 {
        return false;
    }
    let end = out.iter().position(|&c| c == 0).unwrap_or(0);
    let program = PathBuf::from(OsString::from_wide(&out[..end]));
    std::env::current_exe().is_ok_and(|exe| crate::history::same_file(&exe, &program))
}

#[cfg(test)]
mod association_tests {
    #[test]
    fn registry_values_name_this_exe() {
        let values = super::association_values(r"C:\Apps\Flores\FloresPDF.exe");
        let command = values.iter().find(|(k, n, _)| k.ends_with(r"FloresPDF.pdf\shell\open\command") && n.is_empty()).unwrap();
        assert_eq!(command.2, r#""C:\Apps\Flores\FloresPDF.exe" "%1""#);
        assert!(values.iter().all(|(k, _, _)| k.starts_with(r"Software\")));
        assert!(values.iter().any(|(k, n, v)| k == r"Software\RegisteredApplications" && *n == "Flores PDF" && v.ends_with("Capabilities")));
        assert!(values.iter().any(|(k, n, v)| k.ends_with(r"FileAssociations") && *n == ".epub" && v == "FloresPDF.epub"));
    }
}
