//! The screen surface the pages are drawn into: an `ANativeWindow` from the app's SurfaceView, which
//! the GPU draws into (`gpu.rs`).

use std::ffi::c_void;

#[link(name = "android")]
unsafe extern "C" {
    fn ANativeWindow_fromSurface(env: *mut jni::sys::JNIEnv, surface: jni::sys::jobject) -> *mut c_void;
    fn ANativeWindow_release(window: *mut c_void);
    fn ANativeWindow_setBuffersGeometry(window: *mut c_void, width: i32, height: i32, format: i32) -> i32;
}

/// A reference to the surface, released when dropped.
pub struct Window(*mut c_void);

impl Window {
    /// SAFETY: `env` is the calling thread's JNI environment and `surface` an android.view.Surface.
    pub unsafe fn from_surface(env: *mut jni::sys::JNIEnv, surface: jni::sys::jobject) -> Option<Self> {
        let w = unsafe { ANativeWindow_fromSurface(env, surface) };
        (!w.is_null()).then_some(Self(w))
    }

    pub fn raw(&self) -> *mut c_void {
        self.0
    }

    /// Sets the buffers' pixel format, keeping the surface's own size.
    pub fn set_format(&self, format: i32) {
        unsafe { ANativeWindow_setBuffersGeometry(self.0, 0, 0, format) };
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe { ANativeWindow_release(self.0) };
    }
}
