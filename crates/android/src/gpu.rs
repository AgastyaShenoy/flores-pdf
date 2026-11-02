//! Drawing frames with the GPU, through OpenGL ES 2. Tiles are still rendered on the CPU by the
//! engine and kept compressed in the tile cache; those on screen and near it are uploaded as
//! textures, and a frame is a few dozen textured rectangles. Scrolling then costs the CPU almost
//! nothing, which a 144 Hz tablet screen of 5 million pixels needs: composing every frame on the
//! CPU, as the Windows app does, took 10 ms there.

use crate::window::Window;
use render::{Tile, TileKey};
use std::collections::HashMap;
use std::ffi::{CString, c_char, c_void};

type Ptr = *mut c_void;

const EGL_NONE: i32 = 0x3038;
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_ES2_BIT: i32 = 0x0004;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_WINDOW_BIT: i32 = 0x0004;
const EGL_NATIVE_VISUAL_ID: i32 = 0x302E;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;

const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_WRAP_S: u32 = 0x2802;
const GL_TEXTURE_WRAP_T: u32 = 0x2803;
const GL_LINEAR: i32 = 0x2601;
const GL_CLAMP_TO_EDGE: i32 = 0x812F;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_FLOAT: u32 = 0x1406;
const GL_TRIANGLE_STRIP: u32 = 0x0005;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;
const GL_BLEND: u32 = 0x0BE2;
const GL_SRC_ALPHA: u32 = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetDisplay(id: Ptr) -> Ptr;
    fn eglInitialize(display: Ptr, major: *mut i32, minor: *mut i32) -> u32;
    fn eglChooseConfig(display: Ptr, attribs: *const i32, configs: *mut Ptr, size: i32, count: *mut i32) -> u32;
    fn eglGetConfigAttrib(display: Ptr, config: Ptr, attribute: i32, value: *mut i32) -> u32;
    fn eglCreateWindowSurface(display: Ptr, config: Ptr, window: Ptr, attribs: *const i32) -> Ptr;
    fn eglCreateContext(display: Ptr, config: Ptr, share: Ptr, attribs: *const i32) -> Ptr;
    fn eglMakeCurrent(display: Ptr, draw: Ptr, read: Ptr, context: Ptr) -> u32;
    fn eglSwapBuffers(display: Ptr, surface: Ptr) -> u32;
    fn eglDestroySurface(display: Ptr, surface: Ptr) -> u32;
    fn eglDestroyContext(display: Ptr, context: Ptr) -> u32;
    fn eglGetError() -> i32;
}

#[link(name = "GLESv2")]
unsafe extern "C" {
    fn glViewport(x: i32, y: i32, width: i32, height: i32);
    fn glClearColor(r: f32, g: f32, b: f32, a: f32);
    fn glClear(mask: u32);
    fn glCreateShader(kind: u32) -> u32;
    fn glShaderSource(shader: u32, count: i32, sources: *const *const c_char, lengths: *const i32);
    fn glCompileShader(shader: u32);
    fn glGetShaderiv(shader: u32, name: u32, value: *mut i32);
    fn glCreateProgram() -> u32;
    fn glAttachShader(program: u32, shader: u32);
    fn glLinkProgram(program: u32);
    fn glGetProgramiv(program: u32, name: u32, value: *mut i32);
    fn glUseProgram(program: u32);
    fn glGetAttribLocation(program: u32, name: *const c_char) -> i32;
    fn glGetUniformLocation(program: u32, name: *const c_char) -> i32;
    fn glUniform2f(location: i32, x: f32, y: f32);
    fn glUniform4f(location: i32, x: f32, y: f32, z: f32, w: f32);
    fn glGenTextures(n: i32, textures: *mut u32);
    fn glDeleteTextures(n: i32, textures: *const u32);
    fn glBindTexture(target: u32, texture: u32);
    fn glTexParameteri(target: u32, name: u32, value: i32);
    fn glTexImage2D(target: u32, level: i32, internal: i32, width: i32, height: i32, border: i32, format: u32, kind: u32, pixels: *const c_void);
    fn glPixelStorei(name: u32, value: i32);
    fn glEnableVertexAttribArray(index: u32);
    fn glVertexAttribPointer(index: u32, size: i32, kind: u32, normalized: u8, stride: i32, pointer: *const c_void);
    fn glDrawArrays(mode: u32, first: i32, count: i32);
    fn glEnable(cap: u32);
    fn glDisable(cap: u32);
    fn glBlendFunc(source: u32, destination: u32);
}

/// Positions in pixels from the view's top-left; texture coordinates for the image program.
const VERTEX: &str = "
attribute vec2 pos;
attribute vec2 uv;
uniform vec2 screen;
varying vec2 v;
void main() {
    gl_Position = vec4(pos.x / screen.x * 2.0 - 1.0, 1.0 - pos.y / screen.y * 2.0, 0.0, 1.0);
    v = uv;
}";
/// Tiles are uploaded as they're stored, 0x00RRGGBB words, which in memory are the bytes B G R 0:
/// the shader swaps them back.
const IMAGE: &str = "
precision mediump float;
uniform sampler2D image;
varying vec2 v;
void main() {
    gl_FragColor = vec4(texture2D(image, v).bgr, 1.0);
}";
const FILL: &str = "
precision mediump float;
uniform vec4 color;
void main() {
    gl_FragColor = color;
}";

struct Program {
    id: u32,
    pos: u32,
    uv: u32,
    screen: i32,
    color: i32,
}

struct Texture {
    id: u32,
    width: u32,
    height: u32,
    /// The frame it was last drawn in.
    used: u64,
}

pub struct Gpu {
    display: Ptr,
    surface: Ptr,
    context: Ptr,
    image: Program,
    fill: Program,
    textures: HashMap<TileKey, Texture>,
    bytes: usize,
    frame: u64,
    /// A tile's pixels, decoded for upload.
    scratch: Vec<u32>,
    /// Keeps the surface alive while the GPU draws into it.
    _window: Window,
}

impl Gpu {
    pub fn new(window: Window) -> Option<Self> {
        unsafe {
            let display = eglGetDisplay(std::ptr::null_mut());
            if display.is_null() || eglInitialize(display, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                return fail("eglInitialize");
            }
            let attribs = [
                EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
                EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE,
            ];
            let (mut config, mut count) = (std::ptr::null_mut(), 0);
            if eglChooseConfig(display, attribs.as_ptr(), &mut config, 1, &mut count) == 0 || count == 0 {
                return fail("eglChooseConfig");
            }
            let mut format = 0;
            eglGetConfigAttrib(display, config, EGL_NATIVE_VISUAL_ID, &mut format);
            window.set_format(format);
            let surface = eglCreateWindowSurface(display, config, window.raw(), [EGL_NONE].as_ptr());
            if surface.is_null() {
                return fail("eglCreateWindowSurface");
            }
            let context = eglCreateContext(display, config, std::ptr::null_mut(), [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE].as_ptr());
            if context.is_null() || eglMakeCurrent(display, surface, surface, context) == 0 {
                eglDestroySurface(display, surface);
                return fail("eglCreateContext");
            }
            let (Some(image), Some(fill)) = (program(IMAGE), program(FILL)) else {
                eglMakeCurrent(display, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
                eglDestroyContext(display, context);
                eglDestroySurface(display, surface);
                return None;
            };
            glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
            Some(Self { display, surface, context, image, fill, textures: HashMap::new(), bytes: 0, frame: 0, scratch: Vec::new(), _window: window })
        }
    }

    /// Starts a frame of `width` × `height` pixels, cleared to `background`.
    pub fn begin(&mut self, width: u32, height: u32, background: u32) -> bool {
        self.frame += 1;
        unsafe {
            if eglMakeCurrent(self.display, self.surface, self.surface, self.context) == 0 {
                return false;
            }
            glViewport(0, 0, width as i32, height as i32);
            let [r, g, b] = rgb(background);
            glClearColor(r, g, b, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            for p in [&self.image, &self.fill] {
                glUseProgram(p.id);
                glUniform2f(p.screen, width as f32, height as f32);
            }
        }
        true
    }

    /// Fills a rectangle with `color`, blended over what's there unless `alpha` is 255.
    pub fn fill(&self, x: f32, y: f32, width: f32, height: f32, color: u32, alpha: u32) {
        let [r, g, b] = rgb(color);
        unsafe {
            glUseProgram(self.fill.id);
            glUniform4f(self.fill.color, r, g, b, alpha as f32 / 255.0);
            if alpha < 255 {
                glEnable(GL_BLEND);
            }
            quad(&self.fill, x, y, width, height);
            if alpha < 255 {
                glDisable(GL_BLEND);
            }
        }
    }

    pub fn has(&self, key: &TileKey) -> bool {
        self.textures.contains_key(key)
    }

    /// Makes a texture of a rendered tile.
    pub fn upload(&mut self, key: TileKey, tile: &Tile) {
        let (w, h) = (tile.width, tile.height);
        self.scratch.resize(w as usize * h as usize, 0);
        for (y, row) in self.scratch.chunks_exact_mut(w as usize).enumerate() {
            tile.pixels.read_row(w, y as u32, 0, row);
        }
        let mut id = 0;
        unsafe {
            glGenTextures(1, &mut id);
            glBindTexture(GL_TEXTURE_2D, id);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA as i32, w as i32, h as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, self.scratch.as_ptr().cast());
        }
        self.bytes += w as usize * h as usize * 4;
        if let Some(old) = self.textures.insert(key, Texture { id, width: w, height: h, used: self.frame }) {
            self.bytes -= old.width as usize * old.height as usize * 4;
            unsafe { glDeleteTextures(1, &old.id) };
        }
    }

    /// Draws the texture of `key` stretched over the rectangle, if there is one.
    pub fn image(&mut self, key: &TileKey, x: f32, y: f32, width: f32, height: f32) {
        let Some(t) = self.textures.get_mut(key) else { return };
        t.used = self.frame;
        unsafe {
            glUseProgram(self.image.id);
            glBindTexture(GL_TEXTURE_2D, t.id);
            quad(&self.image, x, y, width, height);
        }
    }

    /// Textures of `page` at scales other than `scale`, with their sizes, to stretch over tiles
    /// not rendered yet.
    pub fn others(&self, page: usize, scale: u32, look: u8) -> Vec<(TileKey, u32, u32)> {
        self.textures
            .iter()
            .filter(|(k, _)| k.page == page && k.scale != scale && k.look == look)
            .map(|(k, t)| (*k, t.width, t.height))
            .collect()
    }

    /// Shows the frame, then drops the textures not drawn lately until they fit in `budget` bytes.
    pub fn finish(&mut self, budget: usize) {
        unsafe { eglSwapBuffers(self.display, self.surface) };
        if self.bytes <= budget {
            return;
        }
        let mut old: Vec<(u64, TileKey)> = self.textures.iter().filter(|(_, t)| t.used < self.frame).map(|(k, t)| (t.used, *k)).collect();
        old.sort_unstable_by_key(|&(used, _)| used);
        for (_, key) in old {
            if self.bytes <= budget {
                break;
            }
            if let Some(t) = self.textures.remove(&key) {
                self.bytes -= t.width as usize * t.height as usize * 4;
                unsafe { glDeleteTextures(1, &t.id) };
            }
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            if eglMakeCurrent(self.display, self.surface, self.surface, self.context) != 0 {
                let ids: Vec<u32> = self.textures.values().map(|t| t.id).collect();
                glDeleteTextures(ids.len() as i32, ids.as_ptr());
            }
            eglMakeCurrent(self.display, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            eglDestroyContext(self.display, self.context);
            eglDestroySurface(self.display, self.surface);
        }
    }
}

fn fail<T>(what: &str) -> Option<T> {
    crate::log(&format!("{what} failed: EGL error 0x{:x}", unsafe { eglGetError() }));
    None
}

fn rgb(c: u32) -> [f32; 3] {
    [(c >> 16 & 0xFF) as f32 / 255.0, (c >> 8 & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0]
}

/// A rectangle, the whole texture stretched over it for the image program.
unsafe fn quad(p: &Program, x: f32, y: f32, w: f32, h: f32) {
    let v: [f32; 16] = [x, y, 0.0, 0.0, x + w, y, 1.0, 0.0, x, y + h, 0.0, 1.0, x + w, y + h, 1.0, 1.0];
    unsafe {
        glEnableVertexAttribArray(p.pos);
        glVertexAttribPointer(p.pos, 2, GL_FLOAT, 0, 16, v.as_ptr().cast());
        if p.uv != u32::MAX {
            glEnableVertexAttribArray(p.uv);
            glVertexAttribPointer(p.uv, 2, GL_FLOAT, 0, 16, v[2..].as_ptr().cast());
        }
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    }
}

unsafe fn shader(kind: u32, source: &str) -> Option<u32> {
    let text = CString::new(source).ok()?;
    unsafe {
        let s = glCreateShader(kind);
        glShaderSource(s, 1, &text.as_ptr(), std::ptr::null());
        glCompileShader(s);
        let mut ok = 0;
        glGetShaderiv(s, GL_COMPILE_STATUS, &mut ok);
        if ok == 0 {
            crate::log("a shader didn't compile");
            return None;
        }
        Some(s)
    }
}

unsafe fn program(fragment: &str) -> Option<Program> {
    unsafe {
        let id = glCreateProgram();
        glAttachShader(id, shader(GL_VERTEX_SHADER, VERTEX)?);
        glAttachShader(id, shader(GL_FRAGMENT_SHADER, fragment)?);
        glLinkProgram(id);
        let mut ok = 0;
        glGetProgramiv(id, GL_LINK_STATUS, &mut ok);
        if ok == 0 {
            crate::log("a shader program didn't link");
            return None;
        }
        let attrib = |name: &str| {
            let name = CString::new(name).unwrap();
            // -1, when the program doesn't use it, becomes u32::MAX
            glGetAttribLocation(id, name.as_ptr()) as u32
        };
        let uniform = |name: &str| {
            let name = CString::new(name).unwrap();
            glGetUniformLocation(id, name.as_ptr())
        };
        Some(Program { id, pos: attrib("pos"), uv: attrib("uv"), screen: uniform("screen"), color: uniform("color") })
    }
}
