//! PDFium, for what hayro doesn't do: filling in forms, and writing form entries and annotations
//! into a PDF (or removing annotations from it). `pdfium.dll` (third_party/pdfium) is built into the exe, compressed,
//! and written to `%LOCALAPPDATA%\FloresPDF` the first time it's needed, with its license texts;
//! reading never loads it. A `pdfium.dll` beside the exe is used instead, if there is one. Only the
//! functions used are looked up.
//!
//! Values are set through PDFium's own form filling (focus a field, replace its text; click a check
//! box), the way its viewer does, so each field gets a proper appearance that every reader shows.
//! Annotations get theirs from PDFium too, made when the page is drawn once with annotations, except
//! text boxes, which PDFium doesn't make appearances for: theirs is given (`Shape::FreeText`).
//! Coordinates here are the page's own: points, y up.

use std::ffi::{CString, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

/// PDFium and its license texts, deflated by the build script.
static DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pdfium.dll.deflate"));
static LICENSES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pdfium-licenses.txt.deflate"));
/// The PDFium build inside, which names the file it's written to.
const BUILD: &str = "7881";

type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RectF {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

#[repr(C)]
struct QuadPointsF([f32; 8]);

#[repr(C)]
#[derive(Clone, Copy)]
struct PointF {
    x: f32,
    y: f32,
}

/// FPDF_FILEWRITE, with the buffer it writes to after the fields PDFium knows.
#[repr(C)]
struct FileWrite {
    version: i32,
    write_block: unsafe extern "C" fn(*mut FileWrite, *const c_void, u32) -> i32,
    out: Vec<u8>,
}

unsafe extern "C" fn write_block(this: *mut FileWrite, data: *const c_void, size: u32) -> i32 {
    // SAFETY: PDFium passes back the FileWrite given to FPDF_SaveAsCopy, and `size` bytes at `data`.
    unsafe { (*this).out.extend_from_slice(std::slice::from_raw_parts(data.cast::<u8>(), size as usize)) };
    1
}

/// FPDF_FORMFILLINFO, version 1, with every callback left out: PDFium checks before calling each.
#[repr(C)]
struct FormFillInfo {
    version: i32,
    callbacks: [usize; 48],
}

macro_rules! functions {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        #[allow(non_snake_case)]
        struct Functions {
            $($name: unsafe extern "system" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Functions {
            /// SAFETY: `lib` is a loaded PDFium whose functions have these signatures.
            unsafe fn load(lib: windows_sys::Win32::Foundation::HMODULE) -> Result<Self, String> {
                use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
                Ok(Self {
                    $($name: unsafe {
                        let f = GetProcAddress(lib, concat!(stringify!($name), "\0").as_ptr())
                            .ok_or(concat!("pdfium.dll has no ", stringify!($name)))?;
                        std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn($($arg),*) $(-> $ret)?>(f)
                    },)*
                })
            }
        }
    };
}

functions! {
    FPDF_InitLibrary: fn();
    FPDF_LoadMemDocument64: fn(*const c_void, usize, *const u8) -> Handle;
    FPDF_CloseDocument: fn(Handle);
    FPDF_GetLastError: fn() -> u32;
    FPDF_GetPageCount: fn(Handle) -> i32;
    FPDF_LoadPage: fn(Handle, i32) -> Handle;
    FPDF_ClosePage: fn(Handle);
    FPDFDOC_InitFormFillEnvironment: fn(Handle, *mut FormFillInfo) -> Handle;
    FPDFDOC_ExitFormFillEnvironment: fn(Handle);
    FORM_OnAfterLoadPage: fn(Handle, Handle);
    FORM_OnBeforeClosePage: fn(Handle, Handle);
    FPDFPage_GetAnnotCount: fn(Handle) -> i32;
    FPDFPage_GetAnnot: fn(Handle, i32) -> Handle;
    FPDFPage_CloseAnnot: fn(Handle);
    FPDFAnnot_GetSubtype: fn(Handle) -> i32;
    FPDFAnnot_GetRect: fn(Handle, *mut RectF) -> i32;
    FPDFAnnot_GetFormFieldType: fn(Handle, Handle) -> i32;
    FPDFAnnot_GetFormFieldName: fn(Handle, Handle, *mut u16, u32) -> u32;
    FPDFAnnot_GetFormFieldValue: fn(Handle, Handle, *mut u16, u32) -> u32;
    FPDFAnnot_GetFormFieldFlags: fn(Handle, Handle) -> i32;
    FPDFAnnot_IsChecked: fn(Handle, Handle) -> i32;
    FPDFAnnot_GetOptionCount: fn(Handle, Handle) -> i32;
    FPDFAnnot_GetOptionLabel: fn(Handle, Handle, i32, *mut u16, u32) -> u32;
    FORM_SetFocusedAnnot: fn(Handle, Handle) -> i32;
    FORM_SelectAllText: fn(Handle, Handle) -> i32;
    FORM_ReplaceSelection: fn(Handle, Handle, *const u16);
    FORM_ForceToKillFocus: fn(Handle) -> i32;
    FORM_OnLButtonDown: fn(Handle, Handle, i32, f64, f64) -> i32;
    FORM_OnLButtonUp: fn(Handle, Handle, i32, f64, f64) -> i32;
    FORM_SetIndexSelected: fn(Handle, Handle, i32, i32) -> i32;
    FPDFPage_CreateAnnot: fn(Handle, i32) -> Handle;
    FPDFAnnot_SetColor: fn(Handle, i32, u32, u32, u32, u32) -> i32;
    FPDFAnnot_AppendAttachmentPoints: fn(Handle, *const QuadPointsF) -> i32;
    FPDFAnnot_SetRect: fn(Handle, *const RectF) -> i32;
    FPDFAnnot_SetBorder: fn(Handle, f32, f32, f32) -> i32;
    FPDFAnnot_SetFlags: fn(Handle, i32) -> i32;
    FPDFAnnot_SetStringValue: fn(Handle, *const u8, *const u16) -> i32;
    FPDFAnnot_SetAP: fn(Handle, i32, *const u16) -> i32;
    FPDFAnnot_AddInkStroke: fn(Handle, *const PointF, usize) -> i32;
    FPDFPage_RemoveAnnot: fn(Handle, i32) -> i32;
    FPDFBitmap_Create: fn(i32, i32, i32) -> Handle;
    FPDFBitmap_Destroy: fn(Handle);
    FPDF_RenderPageBitmap: fn(Handle, Handle, i32, i32, i32, i32, i32, i32);
    FPDF_SaveAsCopy: fn(Handle, *mut FileWrite, u32) -> i32;
}

pub const ANNOT_NOTE: i32 = 1;
const ANNOT_FREE_TEXT: i32 = 3;
const ANNOT_SQUARE: i32 = 5;
const ANNOT_CIRCLE: i32 = 6;
pub const ANNOT_HIGHLIGHT: i32 = 9;
pub const ANNOT_UNDERLINE: i32 = 10;
pub const ANNOT_STRIKEOUT: i32 = 12;
const ANNOT_INK: i32 = 15;
const ANNOT_WIDGET: i32 = 20;
/// Annotation flag: printed with the page.
const FLAG_PRINT: i32 = 4;
const APPEARANCE_NORMAL: i32 = 0;
const FIELD_CHECKBOX: i32 = 2;
const FIELD_RADIO: i32 = 3;
const FIELD_COMBOBOX: i32 = 4;
const FIELD_LISTBOX: i32 = 5;
const FIELD_TEXT: i32 = 6;
const TEXT_MULTILINE: i32 = 1 << 12;
/// FPDF_RenderPageBitmap's flag to draw annotations.
const RENDER_ANNOTATIONS: i32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Text { multiline: bool },
    Check,
    Radio,
    /// A drop-down or list box, and its choices.
    Choice(Vec<String>),
}

/// A form field's widget on a page: where it is and what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub page: usize,
    /// The widget's place among the page's annotations, to find it again when saving.
    pub annot: usize,
    /// left, bottom, right, top in the page's own coordinates.
    pub rect: [f32; 4],
    /// The field's full name; radio buttons of a group share it.
    pub name: String,
    pub kind: FieldKind,
    pub value: String,
    pub checked: bool,
}

/// An annotation to add to a page. Rectangles are left, bottom, right, top.
pub struct NewAnnot {
    pub page: usize,
    /// 0x00RRGGBB.
    pub color: u32,
    pub shape: Shape,
}

pub enum Shape {
    /// A highlight, underline or strikeout (`ANNOT_HIGHLIGHT`…) over lines of text.
    Markup(i32, Vec<[f32; 4]>),
    /// A note's icon and its text.
    Note([f32; 4], String),
    /// Lines drawn freehand (an arrow is drawn this way too), and their width in points.
    Ink(Vec<Vec<[f32; 2]>>, f32),
    /// A rectangle or (`true`) an ellipse's outline, and its width.
    Square([f32; 4], f32, bool),
    /// A text box: where, its text, and its appearance as a content stream in page coordinates.
    FreeText([f32; 4], String, String),
}

/// An annotation to take out of a page: its place in the page's list, and where it is, to make
/// sure it's the one meant.
pub struct Removal {
    pub page: usize,
    pub index: usize,
    pub rect: [f32; 4],
}

/// What to change in a document.
#[derive(Default)]
pub struct Changes<'a> {
    pub fields: &'a [Field],
    pub add: &'a [NewAnnot],
    pub remove: &'a [Removal],
}

pub struct Pdfium {
    f: Functions,
}

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// PDFium, loaded on first use from `pdfium.dll` beside the exe.
pub fn get() -> Result<&'static Pdfium, String> {
    PDFIUM.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Pdfium, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    let path = dll_path()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: a NUL-terminated absolute path, so no other folder is searched.
    let lib = unsafe { LoadLibraryW(wide.as_ptr()) };
    if lib.is_null() {
        return Err("pdfium.dll didn't load".to_owned());
    }
    // SAFETY: the DLL is PDFium 7881, whose functions have the signatures above.
    let f = unsafe { Functions::load(lib)? };
    unsafe { (f.FPDF_InitLibrary)() };
    Ok(Pdfium { f })
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

/// An open document with its form environment, closed on drop. Borrows the file's bytes, which
/// PDFium reads from while it's open.
struct Document<'a> {
    p: &'a Pdfium,
    doc: Handle,
    form: Handle,
    _info: Box<FormFillInfo>,
    _data: &'a [u8],
}

impl Drop for Document<'_> {
    fn drop(&mut self) {
        // SAFETY: handles from this document, closed once, form before document.
        unsafe {
            if !self.form.is_null() {
                (self.p.f.FPDFDOC_ExitFormFillEnvironment)(self.form);
            }
            (self.p.f.FPDF_CloseDocument)(self.doc);
        }
    }
}

/// A loaded page, set up for form filling, closed on drop.
struct Page<'a, 'b> {
    d: &'b Document<'a>,
    page: Handle,
}

impl Drop for Page<'_, '_> {
    fn drop(&mut self) {
        // SAFETY: a page of this document, closed once.
        unsafe {
            (self.d.p.f.FORM_OnBeforeClosePage)(self.page, self.d.form);
            (self.d.p.f.FPDF_ClosePage)(self.page);
        }
    }
}

/// An annotation of a page, closed on drop.
struct Annot<'p> {
    f: &'p Functions,
    a: Handle,
}

impl Drop for Annot<'_> {
    fn drop(&mut self) {
        // SAFETY: an annotation handle, closed once.
        unsafe { (self.f.FPDFPage_CloseAnnot)(self.a) };
    }
}

impl Pdfium {
    fn open<'a>(&'a self, data: &'a [u8], password: Option<&str>) -> Result<Document<'a>, String> {
        let password = CString::new(password.unwrap_or("")).map_err(|_| "The password has a NUL in it".to_owned())?;
        // SAFETY: `data` outlives the document (it's borrowed for its lifetime); the password is
        // NUL-terminated.
        let doc = unsafe { (self.f.FPDF_LoadMemDocument64)(data.as_ptr().cast(), data.len(), password.as_ptr().cast()) };
        if doc.is_null() {
            let code = unsafe { (self.f.FPDF_GetLastError)() };
            return Err(format!("PDFium couldn't read the file (error {code})"));
        }
        let mut info = Box::new(FormFillInfo { version: 1, callbacks: [0; 48] });
        // SAFETY: a valid document; the info lives as long as the environment, in the Document.
        let form = unsafe { (self.f.FPDFDOC_InitFormFillEnvironment)(doc, &mut *info) };
        Ok(Document { p: self, doc, form, _info: info, _data: data })
    }

    /// The form fields of a PDF, page by page.
    pub fn read_fields(&self, data: &[u8], password: Option<&str>) -> Result<Vec<Field>, String> {
        let d = self.open(data, password)?;
        let f = &self.f;
        let pages = unsafe { (f.FPDF_GetPageCount)(d.doc) }.max(0) as usize;
        let mut fields = Vec::new();
        for page in 0..pages {
            let Some(p) = d.page(page) else { continue };
            let count = unsafe { (f.FPDFPage_GetAnnotCount)(p.page) }.max(0) as usize;
            for annot in 0..count {
                let Some(a) = p.annot(annot) else { continue };
                if unsafe { (f.FPDFAnnot_GetSubtype)(a.a) } != ANNOT_WIDGET {
                    continue;
                }
                let kind = match unsafe { (f.FPDFAnnot_GetFormFieldType)(d.form, a.a) } {
                    FIELD_TEXT => {
                        let flags = unsafe { (f.FPDFAnnot_GetFormFieldFlags)(d.form, a.a) };
                        FieldKind::Text { multiline: flags & TEXT_MULTILINE != 0 }
                    }
                    FIELD_CHECKBOX => FieldKind::Check,
                    FIELD_RADIO => FieldKind::Radio,
                    // a list box is picked from like a drop-down
                    FIELD_COMBOBOX | FIELD_LISTBOX => {
                        let n = unsafe { (f.FPDFAnnot_GetOptionCount)(d.form, a.a) }.max(0);
                        FieldKind::Choice((0..n).map(|i| wide_string(|b, l| unsafe { (f.FPDFAnnot_GetOptionLabel)(d.form, a.a, i, b, l) })).collect())
                    }
                    // buttons and signatures aren't filled in here
                    _ => continue,
                };
                let mut r = RectF::default();
                if unsafe { (f.FPDFAnnot_GetRect)(a.a, &mut r) } == 0 {
                    continue;
                }
                fields.push(Field {
                    page,
                    annot,
                    rect: [r.left.min(r.right), r.bottom.min(r.top), r.left.max(r.right), r.bottom.max(r.top)],
                    name: wide_string(|b, l| unsafe { (f.FPDFAnnot_GetFormFieldName)(d.form, a.a, b, l) }),
                    value: wide_string(|b, l| unsafe { (f.FPDFAnnot_GetFormFieldValue)(d.form, a.a, b, l) }),
                    checked: unsafe { (f.FPDFAnnot_IsChecked)(d.form, a.a) } != 0,
                    kind,
                });
            }
        }
        Ok(fields)
    }

    /// The PDF with `changes` made: fields (as edited) filled in, annotations removed, then others
    /// added. Returns the whole new file.
    pub fn apply(&self, data: &[u8], password: Option<&str>, changes: &Changes) -> Result<Vec<u8>, String> {
        let d = self.open(data, password)?;
        let f = &self.f;
        let mut pages: Vec<usize> = changes.fields.iter().map(|x| x.page).chain(changes.add.iter().map(|a| a.page)).collect();
        pages.extend(changes.remove.iter().map(|r| r.page));
        pages.sort_unstable();
        pages.dedup();
        for page in pages {
            let p = d.page(page).ok_or_else(|| format!("PDFium couldn't load page {}", page + 1))?;
            for field in changes.fields.iter().filter(|x| x.page == page) {
                let Some(a) = p.annot(field.annot) else { continue };
                p.fill(&a, field);
            }
            // from the last, so the places of the others stay the same
            let mut removals: Vec<&Removal> = changes.remove.iter().filter(|r| r.page == page).collect();
            removals.sort_by_key(|r| std::cmp::Reverse(r.index));
            for r in removals {
                if let Some(i) = p.find(r.index, r.rect) {
                    unsafe { (f.FPDFPage_RemoveAnnot)(p.page, i as i32) };
                }
            }
            let mut added = false;
            for new in changes.add.iter().filter(|a| a.page == page) {
                p.add(new)?;
                added = true;
            }
            if added {
                // drawing the page with its annotations makes PDFium write their appearances into
                // the document, so every reader shows them
                unsafe {
                    let bitmap = (f.FPDFBitmap_Create)(1, 1, 0);
                    if !bitmap.is_null() {
                        (f.FPDF_RenderPageBitmap)(bitmap, p.page, 0, 0, 1, 1, 0, RENDER_ANNOTATIONS);
                        (f.FPDFBitmap_Destroy)(bitmap);
                    }
                }
            }
        }
        let mut writer = FileWrite { version: 1, write_block, out: Vec::new() };
        // SAFETY: the writer outlives the call; flags 0 writes the whole file.
        if unsafe { (f.FPDF_SaveAsCopy)(d.doc, &mut writer, 0) } == 0 {
            return Err("PDFium couldn't write the file".to_owned());
        }
        Ok(writer.out)
    }
}

impl<'a> Document<'a> {
    fn page(&self, index: usize) -> Option<Page<'a, '_>> {
        // SAFETY: a valid document; the page is closed by Page's drop.
        let page = unsafe { (self.p.f.FPDF_LoadPage)(self.doc, index as i32) };
        if page.is_null() {
            return None;
        }
        unsafe { (self.p.f.FORM_OnAfterLoadPage)(page, self.form) };
        Some(Page { d: self, page })
    }
}

impl<'a> Page<'a, '_> {
    fn annot(&self, index: usize) -> Option<Annot<'a>> {
        // SAFETY: a valid page; the annotation is closed by Annot's drop.
        let a = unsafe { (self.d.p.f.FPDFPage_GetAnnot)(self.page, index as i32) };
        (!a.is_null()).then_some(Annot { f: &self.d.p.f, a })
    }

    /// Which annotation to remove: the one at `index` if it's still at `rect`, else the first one
    /// that is, in case the list has changed.
    fn find(&self, index: usize, rect: [f32; 4]) -> Option<usize> {
        let f = &self.d.p.f;
        let count = unsafe { (f.FPDFPage_GetAnnotCount)(self.page) }.max(0) as usize;
        let at = |i: usize| {
            let a = self.annot(i)?;
            let mut r = RectF::default();
            (unsafe { (f.FPDFAnnot_GetRect)(a.a, &mut r) } != 0).then_some(r)
        };
        let close = |r: RectF| {
            let got = [r.left.min(r.right), r.bottom.min(r.top), r.left.max(r.right), r.bottom.max(r.top)];
            got.iter().zip(rect).all(|(a, b)| (a - b).abs() < 1.0)
        };
        if index < count && at(index).is_some_and(close) {
            return Some(index);
        }
        (0..count).find(|&i| at(i).is_some_and(close))
    }

    /// Adds an annotation, printed with the page.
    fn add(&self, new: &NewAnnot) -> Result<(), String> {
        let f = &self.d.p.f;
        let subtype = match &new.shape {
            Shape::Markup(kind, _) => *kind,
            Shape::Note(..) => ANNOT_NOTE,
            Shape::Ink(..) => ANNOT_INK,
            Shape::Square(.., false) => ANNOT_SQUARE,
            Shape::Square(.., true) => ANNOT_CIRCLE,
            Shape::FreeText(..) => ANNOT_FREE_TEXT,
        };
        // SAFETY: a valid page; the annotation is closed by Annot's drop.
        let a = unsafe { (f.FPDFPage_CreateAnnot)(self.page, subtype) };
        if a.is_null() {
            return Err("PDFium couldn't add an annotation".to_owned());
        }
        let a = Annot { f, a };
        let (r, g, b) = (new.color >> 16 & 0xFF, new.color >> 8 & 0xFF, new.color & 0xFF);
        let set_rect = |q: [f32; 4]| {
            let rect = RectF { left: q[0], top: q[3], right: q[2], bottom: q[1] };
            unsafe { (f.FPDFAnnot_SetRect)(a.a, &rect) };
        };
        let set_text = |key: &str, text: &str| {
            let key = CString::new(key).expect("no NUL in keys");
            let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
            unsafe { (f.FPDFAnnot_SetStringValue)(a.a, key.as_ptr().cast(), wide.as_ptr()) };
        };
        // SAFETY: a valid annotation; every pointer is to data that outlives its call.
        unsafe {
            (f.FPDFAnnot_SetFlags)(a.a, FLAG_PRINT);
            (f.FPDFAnnot_SetColor)(a.a, 0, r, g, b, 255);
        }
        match &new.shape {
            Shape::Markup(_, rects) => {
                let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                for q in rects {
                    // top-left, top-right, bottom-left, bottom-right, as readers expect
                    let points = QuadPointsF([q[0], q[3], q[2], q[3], q[0], q[1], q[2], q[1]]);
                    unsafe { (f.FPDFAnnot_AppendAttachmentPoints)(a.a, &points) };
                    bounds = [bounds[0].min(q[0]), bounds[1].min(q[1]), bounds[2].max(q[2]), bounds[3].max(q[3])];
                }
                set_rect(bounds);
            }
            Shape::Note(rect, text) => {
                set_rect(*rect);
                set_text("Contents", text);
                set_text("Name", "Comment");
            }
            Shape::Ink(strokes, width) => {
                let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                for stroke in strokes {
                    let points: Vec<PointF> = stroke.iter().map(|p| PointF { x: p[0], y: p[1] }).collect();
                    unsafe { (f.FPDFAnnot_AddInkStroke)(a.a, points.as_ptr(), points.len()) };
                    for p in stroke {
                        bounds = [bounds[0].min(p[0]), bounds[1].min(p[1]), bounds[2].max(p[0]), bounds[3].max(p[1])];
                    }
                }
                set_rect([bounds[0] - width, bounds[1] - width, bounds[2] + width, bounds[3] + width]);
                unsafe { (f.FPDFAnnot_SetBorder)(a.a, 0.0, 0.0, *width) };
            }
            Shape::Square(rect, width, _) => {
                set_rect(*rect);
                unsafe { (f.FPDFAnnot_SetBorder)(a.a, 0.0, 0.0, *width) };
            }
            Shape::FreeText(rect, text, appearance) => {
                set_rect(*rect);
                set_text("Contents", text);
                set_text("DA", &format!("/Helv 12 Tf {:.3} {:.3} {:.3} rg", r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0));
                unsafe { (f.FPDFAnnot_SetBorder)(a.a, 0.0, 0.0, 0.0) };
                let wide: Vec<u16> = appearance.encode_utf16().chain([0]).collect();
                unsafe { (f.FPDFAnnot_SetAP)(a.a, APPEARANCE_NORMAL, wide.as_ptr()) };
            }
        }
        Ok(())
    }

    /// Gives a field its value through PDFium's form filling, as typing or clicking would.
    fn fill(&self, a: &Annot, field: &Field) {
        let (f, form, page) = (&self.d.p.f, self.d.form, self.page);
        // SAFETY: handles of this page and document.
        unsafe {
            match &field.kind {
                FieldKind::Text { .. } => {
                    let text: Vec<u16> = field.value.encode_utf16().chain([0]).collect();
                    (f.FORM_SetFocusedAnnot)(form, a.a);
                    (f.FORM_SelectAllText)(form, page);
                    (f.FORM_ReplaceSelection)(form, page, text.as_ptr());
                    (f.FORM_ForceToKillFocus)(form);
                }
                FieldKind::Check | FieldKind::Radio => {
                    // a click in the middle flips it; a radio button is only ever clicked on
                    let now = (f.FPDFAnnot_IsChecked)(form, a.a) != 0;
                    if now != field.checked && (field.checked || field.kind == FieldKind::Check) {
                        let (x, y) = ((field.rect[0] + field.rect[2]) / 2.0, (field.rect[1] + field.rect[3]) / 2.0);
                        (f.FORM_OnLButtonDown)(form, page, 0, f64::from(x), f64::from(y));
                        (f.FORM_OnLButtonUp)(form, page, 0, f64::from(x), f64::from(y));
                        (f.FORM_ForceToKillFocus)(form);
                    }
                }
                FieldKind::Choice(options) => {
                    if let Some(i) = options.iter().position(|o| *o == field.value) {
                        (f.FORM_SetFocusedAnnot)(form, a.a);
                        (f.FORM_SetIndexSelected)(form, page, i as i32, 1);
                        (f.FORM_ForceToKillFocus)(form);
                    }
                }
            }
        }
    }
}

/// A UTF-16 string from a PDFium getter that takes a buffer and its size in bytes, and returns the
/// size needed, terminator included.
fn wide_string(get: impl Fn(*mut u16, u32) -> u32) -> String {
    let bytes = get(std::ptr::null_mut(), 0) as usize;
    if bytes < 2 {
        return String::new();
    }
    let mut buf = vec![0u16; bytes / 2];
    get(buf.as_mut_ptr(), bytes as u32);
    String::from_utf16_lossy(&buf[..buf.len() - 1])
}
