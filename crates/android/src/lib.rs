//! Flores PDF's native library for Android: the engine and renderer the Windows app uses, behind
//! the JNI functions of `io.github.agastyashenoy.flores.Native`. A document is a boxed `Viewer`
//! whose address the app holds as a handle; every call comes from the app's UI thread.
#![cfg(target_os = "android")]

mod gpu;
mod viewer;
mod window;

use jni::JNIEnv;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jfloat, jfloatArray, jint, jintArray, jlong, jstring, JNI_FALSE, JNI_TRUE};
use render::Look;
use std::fs::File;
use std::os::fd::FromRawFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use viewer::{Tap, Viewer};
use window::Window;

/// What `open` returns when it fails. A handle is an address, which can be negative as a jlong:
/// Android tags the top byte of heap pointers.
const UNSUPPORTED: jlong = -1;
const NEEDS_PASSWORD: jlong = -2;
const ENCRYPTED: jlong = -3;
const DAMAGED: jlong = -4;
const UNREADABLE: jlong = -5;

unsafe extern "C" {
    fn dup(fd: i32) -> i32;
}

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(priority: i32, tag: *const std::ffi::c_char, text: *const std::ffi::c_char) -> i32;
}

/// Writes a warning to the system log (logcat), under the tag Flores.
fn log(text: &str) {
    const WARN: i32 = 5;
    let text = std::ffi::CString::new(text.replace('\0', " ")).unwrap_or_default();
    unsafe { __android_log_write(WARN, c"Flores".as_ptr(), text.as_ptr()) };
}

/// Runs `f`, turning a panic into `fallback`: one must not unwind into Java.
fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// SAFETY: `h` is a handle from `open` that hasn't been closed.
unsafe fn viewer<'a>(h: jlong) -> &'a mut Viewer {
    unsafe { &mut *(h as *mut Viewer) }
}

fn string(env: &mut JNIEnv, s: &JString) -> String {
    if s.is_null() {
        return String::new();
    }
    env.get_string(s).map(String::from).unwrap_or_default()
}

fn new_string(env: &mut JNIEnv, s: &str) -> jstring {
    env.new_string(s).map_or(std::ptr::null_mut(), |s| s.into_raw())
}

/// Opens the file behind `fd`, a descriptor the app keeps open until `close`. `gap` is the space
/// around pages, in pixels.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_open(mut env: JNIEnv, _: JClass, fd: jint, password: JString, gap: jint) -> jlong {
    let password = string(&mut env, &password);
    guard(DAMAGED, || {
        // a copy of the descriptor, which the File closes; the app closes its own
        let copy = unsafe { dup(fd) };
        if copy < 0 {
            log(&format!("can't copy the file descriptor: {}", std::io::Error::last_os_error()));
            return UNREADABLE;
        }
        let file = unsafe { File::from_raw_fd(copy) };
        // formats read by name (7-Zip comics) open it again through this
        let path = format!("/proc/self/fd/{fd}");
        let opened = engine::open_file(file, Path::new(&path), &password);
        if let Err(e) = &opened {
            log(&format!("can't open the document: {e:?}"));
        }
        match opened {
            Ok(engine) if engine.page_count() > 0 => Box::into_raw(Box::new(Viewer::new(engine, gap.max(0) as u32))) as jlong,
            Ok(_) => DAMAGED,
            Err(engine::OpenError::Unsupported) => UNSUPPORTED,
            Err(engine::OpenError::NeedsPassword) => NEEDS_PASSWORD,
            Err(engine::OpenError::UnsupportedEncryption) => ENCRYPTED,
            Err(engine::OpenError::Damaged) => DAMAGED,
            Err(engine::OpenError::Io(_)) => UNREADABLE,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_close(_: JNIEnv, _: JClass, h: jlong) {
    // render threads finish the tile they're on and exit by themselves
    guard((), || drop(unsafe { Box::from_raw(h as *mut Viewer) }));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_pageCount(_: JNIEnv, _: JClass, h: jlong) -> jint {
    unsafe { viewer(h) }.page_count() as jint
}

/// The document's own title, or null.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_title(mut env: JNIEnv, _: JClass, h: jlong) -> jstring {
    match guard(None, || unsafe { viewer(h) }.title()) {
        Some(t) => new_string(&mut env, &t),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_outline(mut env: JNIEnv, _: JClass, h: jlong) -> jstring {
    let text = guard(String::new(), || unsafe { viewer(h) }.outline());
    new_string(&mut env, &text)
}

/// Points drawing at the view's surface, or at nothing when `surface` is null.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_setSurface(env: JNIEnv, _: JClass, h: jlong, surface: JObject, width: jint, height: jint) {
    let window = match surface.is_null() {
        true => None,
        false => unsafe { Window::from_surface(env.get_raw(), surface.as_raw()) },
    };
    unsafe { viewer(h) }.set_window(window, width.max(0) as u32, height.max(0) as u32);
}

/// Page colors (0 normal, 1 dark, 2 sepia, 3 black) and the color around pages, 0xRRGGBB.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_setLook(_: JNIEnv, _: JClass, h: jlong, look: jint, background: jint) {
    unsafe { viewer(h) }.set_look(Look::from_u8(look as u8), background as u32 & 0x00FF_FFFF);
}

/// Draws a frame if anything changed; true while more frames are wanted.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_draw(_: JNIEnv, _: JClass, h: jlong) -> jboolean {
    if guard(false, || unsafe { viewer(h) }.draw()) { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_scrollBy(_: JNIEnv, _: JClass, h: jlong, dx: jfloat, dy: jfloat) {
    unsafe { viewer(h) }.scroll_by(f64::from(dx), f64::from(dy));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_scrollTo(_: JNIEnv, _: JClass, h: jlong, x: jfloat, y: jfloat) {
    unsafe { viewer(h) }.scroll_to(f64::from(x), f64::from(y));
}

/// Room the app's bars take at the top and bottom of the view, in pixels.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_setInsets(_: JNIEnv, _: JClass, h: jlong, top: jfloat, bottom: jfloat) {
    unsafe { viewer(h) }.set_insets(f64::from(top), f64::from(bottom));
}

/// x, y, the largest x and y, and the smallest y, in pixels.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_scrollState(env: JNIEnv, _: JClass, h: jlong) -> jfloatArray {
    floats(env, &unsafe { viewer(h) }.scroll_state())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_zoomBy(_: JNIEnv, _: JClass, h: jlong, factor: jfloat, fx: jfloat, fy: jfloat) {
    unsafe { viewer(h) }.zoom_by(factor, f64::from(fx), f64::from(fy));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_zoom(_: JNIEnv, _: JClass, h: jlong) -> jfloat {
    unsafe { viewer(h) }.zoom()
}

/// A pinch or zoom animation starts (true) or ends.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_setGesture(_: JNIEnv, _: JClass, h: jlong, on: jboolean) {
    unsafe { viewer(h) }.set_gesture(on != JNI_FALSE);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_currentPage(_: JNIEnv, _: JClass, h: jlong) -> jint {
    unsafe { viewer(h) }.current_page() as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_goToPage(_: JNIEnv, _: JClass, h: jlong, page: jint) {
    unsafe { viewer(h) }.go_to_page(page.max(0) as usize);
}

/// Shows `page` from `y` points down it, or from its top when `y` is negative.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_goTo(_: JNIEnv, _: JClass, h: jlong, page: jint, y: jfloat) {
    unsafe { viewer(h) }.go_to(page.max(0) as usize, (y >= 0.0).then_some(y));
}

/// The page at the top of the view, points down it, and the zoom: what `restore` takes.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_position(env: JNIEnv, _: JClass, h: jlong) -> jfloatArray {
    let v = unsafe { viewer(h) };
    let (page, offset) = v.position();
    floats(env, &[page as f32, offset, v.zoom()])
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_restore(_: JNIEnv, _: JClass, h: jlong, page: jint, offset: jfloat, zoom: jfloat) {
    unsafe { viewer(h) }.restore(page.max(0) as usize, offset, zoom);
}

/// A tap at (x, y) in the view: null if it hit no link, "" if it followed one within the document,
/// or the address of a link out of it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_tap(mut env: JNIEnv, _: JClass, h: jlong, x: jfloat, y: jfloat) -> jstring {
    match guard(Tap::Nothing, || unsafe { viewer(h) }.tap(f64::from(x), f64::from(y))) {
        Tap::Nothing => std::ptr::null_mut(),
        Tap::Moved => new_string(&mut env, ""),
        Tap::Uri(uri) => new_string(&mut env, &uri),
    }
}

/// Back to where a link was followed from; false if there's nowhere to go back to.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_back(_: JNIEnv, _: JClass, h: jlong) -> jboolean {
    if unsafe { viewer(h) }.back() { JNI_TRUE } else { JNI_FALSE }
}

/// Searches the document from the current page; an empty query clears the search.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_search(mut env: JNIEnv, _: JClass, h: jlong, query: JString) {
    let query = string(&mut env, &query);
    unsafe { viewer(h) }.search(&query);
}

/// Matches found, pages searched, pages, and the current match (-1 for none).
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_searchStatus(env: JNIEnv, _: JClass, h: jlong) -> jintArray {
    let status = unsafe { viewer(h) }.search_status();
    match env.new_int_array(status.len() as i32) {
        Ok(a) => {
            let _ = env.set_int_array_region(&a, 0, &status);
            a.into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Moves to the next match (1) or the previous one (-1); false if there are none.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_agastyashenoy_flores_Native_searchStep(_: JNIEnv, _: JClass, h: jlong, dir: jint) -> jboolean {
    if unsafe { viewer(h) }.search_step(dir) { JNI_TRUE } else { JNI_FALSE }
}

fn floats(env: JNIEnv, values: &[f32]) -> jfloatArray {
    match env.new_float_array(values.len() as i32) {
        Ok(a) => {
            let _ = env.set_float_array_region(&a, 0, values);
            a.into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}
