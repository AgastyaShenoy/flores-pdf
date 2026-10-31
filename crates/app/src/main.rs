//! The reader's window: pages in a continuous column, drawn on the CPU and blitted with GDI
//! (softbuffer), the way Sumatra draws. No GPU, so no graphics driver in the process
//! (docs/architecture.md §5). The tabs, toolbar, panels and menu are drawn by `chrome`.
//!
//! usage: FloresPDF [files] [-page N] [-zoom fit-page|fit-width|PERCENT]
//!        FloresPDF -print-to "printer name" | -print-to-default files

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annots;
mod chrome;
mod favorites;
mod highlights;
mod fonts;
mod history;
mod install;
mod pdfium;
mod print;
mod watch;
mod settings;
mod thumbs;
mod update;
mod win;

use annots::{Markup, Piece, Shape, Tool};
use chrome::{
    AnnotBar, BLACK, Bar, Chrome, DARK, Editor, FieldLook, FieldShape, Hit, LIGHT, Menu, MenuEntry, OutlineRow, PANEL_WIDTH, Palette, Panel, Prompt, Props, Recent, Shown, Sidebar, Start,
    Tabs,
};
use engine::{Engine, Link, OutlineItem, PageSize, Target};
use history::{Entry, History};
use std::collections::HashMap;
use render::text::{self, TextWorker};
use render::{Frame, Layout, Look, RenderPool, Spread, Style, TileCache, Viewport};
use settings::{FIELDS, PageColors, Settings, StartZoom, Theme, Value};
use std::ffi::OsString;
use std::num::NonZeroU32;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

/// Scroll distance per arrow key press, in logical pixels.
const ARROW_STEP: f64 = 40.0;
/// Zoom levels for Ctrl+wheel and +/-, in percent. Same as Sumatra's.
const ZOOM_STEPS: [f32; 24] = [
    8.33, 12.5, 18.0, 25.0, 33.33, 50.0, 66.67, 75.0, 100.0, 125.0, 150.0, 200.0, 300.0, 400.0, 600.0, 800.0,
    1000.0, 1200.0, 1600.0, 2000.0, 2400.0, 3200.0, 4800.0, 6400.0,
];
const TOOLTIP_DELAY: Duration = Duration::from_millis(600);
/// Two clicks this close together, at nearly the same spot, select a word.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// Highlights, blended over the page: color and opacity out of 255.
const MATCH_TINT: (u32, u32) = (0x00FF_D400, 110);
const CURRENT_TINT: (u32, u32) = (0x00FF_7A00, 150);
const SELECTION_TINT: (u32, u32) = (0x0033_8CFF, 90);
/// The reader's highlights, in their own colors.
const HIGHLIGHT_ALPHA: u32 = 130;
/// How close to the sidebar's edge a press starts resizing it, in logical pixels.
const SIDEBAR_EDGE: f64 = 4.0;
/// Places kept for going back.
const HISTORY: usize = 100;
/// The scroll bar: width of the strip at the right edge that takes the mouse, and the thumb's
/// shortest length, in logical pixels.
const SCROLLBAR: f64 = 18.0;
const THUMB_MIN: f64 = 32.0;
/// Recent files the start page lists, at most.
const RECENT: usize = 20;
/// Closed tabs Ctrl+Shift+T can bring back.
const CLOSED: usize = 20;
/// How long a smooth scroll takes to arrive.
const GLIDE: Duration = Duration::from_millis(160);
/// A page image bigger than this many pixels is made at a lower resolution, to bound memory.
const MAX_IMAGE_PIXELS: f64 = 24e6;
/// Colors for annotations: the four highlight colors, red and black.
const ANNOT_COLORS: [u32; 6] = [0xFFE14D, 0x7CDB6E, 0x6EC6FF, 0xFF8AC8, 0xE53935, 0x1B1B1B];
/// Line widths and text sizes for the three width choices, in points.
const LINE_WIDTHS: [f32; 3] = [1.0, 2.5, 5.0];
const TEXT_SIZES: [f32; 3] = [10.0, 14.0, 20.0];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Zoom {
    FitPage,
    FitWidth,
    Percent(f32),
}

impl From<StartZoom> for Zoom {
    fn from(z: StartZoom) -> Self {
        match z {
            StartZoom::FitPage => Zoom::FitPage,
            StartZoom::FitWidth => Zoom::FitWidth,
            StartZoom::ActualSize => Zoom::Percent(100.0),
        }
    }
}

/// Wakes the event loop from elsewhere.
enum Wake {
    /// A tile or a page's text is ready.
    Redraw,
    /// Another copy of the reader handed over its command line.
    HandOver(Vec<OsString>),
    /// Printing has done this many pages of so many, or has finished.
    Printing(usize, usize),
    Printed(Result<usize, String>),
    /// An open file changed on disk.
    FileChanged(PathBuf),
    /// GitHub has a newer release; its setup has been downloaded (or couldn't be).
    Update(update::Release),
    UpdateReady(Result<PathBuf, String>),
}

/// An open file. The shown tab's document is in `App::doc` while it's shown.
struct Tab {
    path: PathBuf,
    name: String,
    /// A tab in the background. None for the shown tab, and for a tab from the last session that
    /// hasn't been looked at yet: it's read when it's first shown.
    doc: Option<Doc>,
}

/// What the right-click menus can do.
#[derive(Clone, PartialEq, Debug)]
enum Command {
    Copy,
    Highlight,
    Underline,
    Strikeout,
    /// The highlight at a point of a page, in a document that isn't a PDF.
    RemoveHighlight(usize, f32, f32),
    /// An annotation not saved yet, by its place in the document's marks: change its text, or
    /// remove it.
    EditMark(usize),
    DeleteMark(usize),
    /// An annotation in the file: its page, its place in the page's list, and where it is.
    DeleteSaved(usize, usize, [f32; 4]),
    UndoMark,
    Annotate,
    Save,
    Home,
    /// Setting `i` (in FIELDS) takes choice `k`, from its drop-down in the settings window.
    SetChoice(usize, usize),
    FindSelection,
    SearchWeb,
    CopyLink(String),
    ZoomIn,
    ZoomOut,
    FitPage,
    FitWidth,
    ActualSize,
    Print,
    SaveCopy,
    Open,
    CloseShownTab,
    FirstPage,
    LastPage,
    GoToPage,
    Back,
    Forward,
    Find,
    ToggleToolbar,
    ToggleSidebar,
    OpenSettings,
    About,
    FillForm,
    /// A drop-down field and the choice picked.
    PickOption(usize, usize),
    AddFavorite(usize),
    RemoveFavorite(usize),
    RenameBookmark(usize),
    CopyPageImage(usize),
    SavePageImage(usize),
    ShowInFolder(PathBuf),
    CopyPath(PathBuf),
    Properties,
    CloseTab(usize),
    CloseOtherTabs(usize),
    CloseTabsRight(usize),
    ReopenClosed,
    SetSpread(Spread),
    RotateLeft,
    RotateRight,
    Presentation,
    FullScreen,
    Separator,
}

impl Command {
    fn entry(&self) -> MenuEntry {
        match self {
            Command::Copy => MenuEntry::Item("Copy", "Ctrl+C"),
            Command::Highlight => MenuEntry::Item("Highlight", "Ctrl+H"),
            Command::Underline => MenuEntry::Item("Underline", "Ctrl+U"),
            Command::Strikeout => MenuEntry::Item("Strike out", ""),
            Command::RemoveHighlight(..) => MenuEntry::Item("Remove highlight", ""),
            Command::EditMark(_) => MenuEntry::Item("Edit text…", ""),
            Command::DeleteMark(_) | Command::DeleteSaved(..) => MenuEntry::Item("Delete annotation", ""),
            Command::UndoMark => MenuEntry::Item("Undo annotation", "Ctrl+Z"),
            Command::Annotate => MenuEntry::Item("Annotate", ""),
            Command::Save => MenuEntry::Item("Save", "Ctrl+S"),
            Command::Home => MenuEntry::Item("Home", "Alt+Home"),
            Command::SetChoice(i, k) => match &FIELDS[*i].kind {
                settings::Kind::Choice(options) => MenuEntry::Item(options.get(*k).map_or("", |o| o.1), ""),
                _ => MenuEntry::Separator,
            },
            Command::FindSelection => MenuEntry::Item("Find in document", ""),
            Command::SearchWeb => MenuEntry::Item("Search the web", ""),
            Command::CopyLink(_) => MenuEntry::Item("Copy link address", ""),
            Command::ZoomIn => MenuEntry::Item("Zoom in", "+"),
            Command::ZoomOut => MenuEntry::Item("Zoom out", "−"),
            Command::FitPage => MenuEntry::Item("Fit page", "Ctrl+0"),
            Command::FitWidth => MenuEntry::Item("Fit width", "Ctrl+2"),
            Command::ActualSize => MenuEntry::Item("Actual size", "Ctrl+1"),
            Command::Print => MenuEntry::Item("Print…", "Ctrl+P"),
            Command::Open => MenuEntry::Item("Open…", "Ctrl+O"),
            Command::CloseShownTab => MenuEntry::Item("Close", "Ctrl+W"),
            Command::FirstPage => MenuEntry::Item("First page", "Home"),
            Command::LastPage => MenuEntry::Item("Last page", "End"),
            Command::GoToPage => MenuEntry::Item("Go to page…", "Ctrl+G"),
            Command::Back => MenuEntry::Item("Back", "Alt+←"),
            Command::Forward => MenuEntry::Item("Forward", "Alt+→"),
            Command::Find => MenuEntry::Item("Find…", "Ctrl+F"),
            Command::ToggleToolbar => MenuEntry::Item("Toolbar", "F8"),
            Command::ToggleSidebar => MenuEntry::Item("Sidebar", "F12"),
            Command::OpenSettings => MenuEntry::Item("Settings", "Ctrl+,"),
            Command::About => MenuEntry::Item("About Flores PDF", ""),
            Command::SaveCopy => MenuEntry::Item("Save a copy…", "Ctrl+Shift+S"),
            Command::FillForm => MenuEntry::Item("Fill in form", ""),
            Command::PickOption(..) => MenuEntry::Item("", ""),
            Command::AddFavorite(_) => MenuEntry::Item("Bookmark this page", "Ctrl+D"),
            Command::RemoveFavorite(_) => MenuEntry::Item("Remove bookmark", "Ctrl+D"),
            Command::RenameBookmark(_) => MenuEntry::Item("Rename bookmark…", ""),
            Command::CopyPageImage(_) => MenuEntry::Item("Copy page as image", ""),
            Command::SavePageImage(_) => MenuEntry::Item("Save page as PNG…", ""),
            Command::ShowInFolder(_) => MenuEntry::Item("Show in folder", ""),
            Command::CopyPath(_) => MenuEntry::Item("Copy file path", ""),
            Command::Properties => MenuEntry::Item("Document properties", ""),
            Command::CloseTab(_) => MenuEntry::Item("Close tab", "Ctrl+W"),
            Command::CloseOtherTabs(_) => MenuEntry::Item("Close other tabs", ""),
            Command::CloseTabsRight(_) => MenuEntry::Item("Close tabs to the right", ""),
            Command::ReopenClosed => MenuEntry::Item("Reopen closed tab", "Ctrl+Shift+T"),
            Command::SetSpread(Spread::One) => MenuEntry::Item("One page", "Ctrl+6"),
            Command::SetSpread(Spread::Two) => MenuEntry::Item("Two pages", "Ctrl+7"),
            Command::SetSpread(Spread::Book) => MenuEntry::Item("Book view", "Ctrl+8"),
            Command::RotateLeft => MenuEntry::Item("Rotate left", "Ctrl+Shift+−"),
            Command::RotateRight => MenuEntry::Item("Rotate right", "Ctrl+Shift++"),
            Command::Presentation => MenuEntry::Item("Presentation", "F5"),
            Command::FullScreen => MenuEntry::Item("Full screen", "F11"),
            Command::Separator => MenuEntry::Separator,
        }
    }
}

/// A protected file waiting for its password, and how to open it once it comes.
struct PasswordPrompt {
    path: PathBuf,
    page: Option<usize>,
    zoom: Option<Zoom>,
    typed: String,
    /// The last try was wrong.
    wrong: bool,
}

/// A form field being filled in: what PDFium read, where it is on the page as shown, and whether
/// it's been changed.
struct FormEntry {
    field: pdfium::Field,
    /// x0, y0, x1, y1 in points from the top-left of the page as shown.
    rect: [f32; 4],
    edited: bool,
}

/// What the text box is for.
#[derive(Clone, Copy, PartialEq, Debug)]
enum EditFor {
    /// A new note or text box at a point of a page.
    NewNote(usize, [f32; 2]),
    NewText(usize, [f32; 2]),
    /// A note or text box not saved yet, by its place in the document's marks.
    Mark(usize),
    /// A bookmark's name, by its page.
    Bookmark(usize),
}

struct EditorState {
    purpose: EditFor,
    title: &'static str,
    text: String,
    /// The text is all selected, as it is when the box opens with text in it: typing replaces it.
    selected: bool,
}

/// An annotation under the mouse: one not saved yet, by its place in the document's marks, or one
/// in the file, with its page.
enum Found {
    Mark(usize),
    Saved(usize, engine::Annotation),
}

/// Why a file didn't open.
enum Failure {
    NeedsPassword,
    Other(String),
}

/// Presentation mode: one page (or spread) at a time, filling a black screen, no toolbar.
#[derive(Clone, Copy)]
struct Presenting {
    /// The page shown, and what to go back to afterwards.
    page: usize,
    zoom: Zoom,
    fullscreen: bool,
}

/// A smooth scroll on its way: from where it started to where it's going, in document pixels.
#[derive(Clone, Copy)]
struct Glide {
    from: (f64, f64),
    to: (f64, f64),
    start: Instant,
}

struct Doc {
    /// Absolute.
    path: PathBuf,
    /// The password it opened with, kept in memory only, to read it again after it changes.
    password: Option<String>,
    /// It's a PDF with a form; its fields once filling in starts, whether they're shown, and
    /// which one takes typing.
    has_form: bool,
    fields: Option<Vec<FormEntry>>,
    /// The fields' values when they were read.
    saved: Vec<(String, bool)>,
    /// Annotations made and not saved into the file yet.
    marks: Vec<annots::Mark>,
    /// The PDF with changes not written to its file yet (an annotation taken out), which the engine
    /// reads instead of the file.
    memory: Option<Vec<u8>>,
    /// The annotations in the file, by page, for the pages looked at so far.
    annotations: HashMap<usize, Vec<engine::Annotation>>,
    filling: bool,
    focus_field: Option<usize>,
    name: String,
    engine: Arc<dyn Engine>,
    sizes: Vec<PageSize>,
    /// None while the tab is in the background.
    pool: Option<RenderPool>,
    cache: TileCache,
    layout: Layout,
    zoom: Zoom,
    /// Quarter turns clockwise, and how pages sit side by side.
    rotation: u8,
    spread: Spread,
    /// Top-left of the view, in document pixels.
    x: f64,
    y: f64,
    /// 1 after scrolling down, -1 after scrolling up: which way to render ahead.
    ahead: i64,
    text: TextWorker,
    selection: Option<Selection>,
    /// The highlighted match, as (page, first character), since matches found later can come
    /// before it in the list.
    current: Option<(usize, usize)>,
    /// Go to the first match as soon as one is found.
    jump: bool,
    /// The search last run on this document, which the search box may have moved on from while
    /// the tab was in the background.
    query: String,
    /// Links of the pages looked at so far.
    links: HashMap<usize, Vec<Link>>,
    /// The outline, flattened in reading order.
    outline: Vec<Node>,
    sidebar_scroll: f64,
    /// Where links and jumps came from, for Alt+Left, and where going back came from.
    back: Vec<Spot>,
    forward: Vec<Spot>,
}

/// An outline entry, with its place in the tree.
struct Node {
    title: String,
    target: Option<Target>,
    depth: usize,
    children: bool,
    open: bool,
}

fn flatten(items: &[OutlineItem], depth: usize, out: &mut Vec<Node>) {
    for item in items {
        out.push(Node {
            title: item.title.clone(),
            target: item.target.clone(),
            depth,
            children: !item.children.is_empty(),
            open: item.open,
        });
        flatten(&item.children, depth + 1, out);
    }
}

/// A place in the document: a page and how far down it the top of the view is, in points.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Spot {
    page: usize,
    y: f32,
}

/// Selected text, from where the drag started to where it is now, each a page and a boundary
/// between characters of its text.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Selection {
    anchor: (usize, usize),
    focus: (usize, usize),
}

impl Selection {
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.focus { (self.anchor, self.focus) } else { (self.focus, self.anchor) }
    }

    /// The selected characters of `page`, whose text has `len` characters.
    fn on(&self, page: usize, len: usize) -> Option<Range<usize>> {
        let (a, b) = self.ordered();
        if page < a.0 || page > b.0 {
            return None;
        }
        let start = if page == a.0 { a.1 } else { 0 };
        let end = if page == b.0 { b.1 } else { len };
        (start < end).then_some(start..end.min(len))
    }
}

struct App {
    proxy: EventLoopProxy<Wake>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    /// The command line, acted on once the window exists.
    start: Option<Args>,
    /// Keeps other copies of the reader waiting until this one's window can take their files.
    start_lock: Option<win::StartLock>,
    tabs: Vec<Tab>,
    active: usize,
    /// The shown tab's document; None shows the start page.
    doc: Option<Doc>,
    /// Why the last file didn't open. Shown until the next click or key.
    error: Option<String>,
    /// How printing is going, shown the same way.
    status: Option<String>,
    /// Set to stop the print job that's running.
    printing: Option<Arc<AtomicBool>>,
    /// Watches the open files for changes.
    watcher: watch::Watcher,
    favorites: favorites::Favorites,
    highlights: highlights::Highlights,
    /// The sidebar shows bookmarks rather than the outline.
    sidebar_favorites: bool,
    /// The annotation tools' bar is shown, the tool picked, the colors picked (in ANNOT_COLORS) for
    /// highlights and for the rest, and the line width (in LINE_WIDTHS).
    annotating: bool,
    tool: Option<Tool>,
    annot_colors: [usize; 2],
    annot_width: usize,
    /// A drag drawing with a tool: the page, and the points so far (where it started first).
    drawing: Option<(Tool, usize, Vec<[f32; 2]>)>,
    /// The text box for a note, text or a bookmark's name, while it's open.
    editor: Option<EditorState>,
    /// The font text boxes are written in; annotating needs it.
    mark_font: Option<fonts::Font>,
    thumbs: thumbs::Thumbs,
    /// A newer release, once GitHub has said so, and whether its setup is downloading.
    update: Option<update::Release>,
    downloading: bool,
    history: History,
    settings: Settings,
    /// None if the UI font is missing: keyboard only then.
    chrome: Option<Chrome>,
    system_dark: bool,
    /// The right-click menu: where it opened and what's in it.
    menu: Option<(PhysicalPosition<f64>, Vec<Command>)>,
    /// The password box, while a protected file waits for its password.
    prompt: Option<PasswordPrompt>,
    /// The heading of the properties box: document properties, or about.
    props_title: &'static str,
    /// The document properties box's rows, while it's open.
    props: Option<Vec<(&'static str, String)>>,
    /// Files of closed tabs, the last closed at the end.
    closed: Vec<PathBuf>,
    glide: Option<Glide>,
    presenting: Option<Presenting>,
    /// The mouse is over the scroll bar or the one along the bottom, or dragging a thumb (along the
    /// bottom, or not) from this far into it.
    on_scrollbar: bool,
    on_hbar: bool,
    thumb_drag: Option<(bool, f64)>,
    /// Resizing the sidebar: where the drag started and the width then.
    side_drag: Option<(f64, i64)>,
    /// Dragging tab `n` to a new place.
    tab_drag: Option<usize>,
    /// A selection is being dragged past the window's edge, so the pages scroll on their own, and
    /// when they last did.
    autoscroll: bool,
    autoscroll_at: Option<Instant>,
    /// The right button is down, and the wheel zoomed while it was: then letting go doesn't open
    /// the menu.
    right_down: bool,
    right_zoomed: bool,
    modifiers: ModifiersState,
    cursor: PhysicalPosition<f64>,
    /// Cursor and scroll position where a drag that moves the pages started.
    drag: Option<(PhysicalPosition<f64>, (f64, f64))>,
    /// Dragging out a text selection.
    selecting: bool,
    last_click: Option<(Instant, PhysicalPosition<f64>)>,
    cursor_icon: CursorIcon,
    /// The search box's text, kept across documents.
    search: String,
    search_focus: bool,
    /// The next key typed replaces the search text, as when it's all selected.
    search_fresh: bool,
    /// The link under the mouse: where it is on screen and what it says it does.
    link_tip: Option<(kurbo::Rect, String)>,
    /// The link the mouse button went down on; it's followed if the button comes up there too.
    pressed_link: Option<Target>,
    hover: Option<Hit>,
    hover_since: Instant,
    pressed: Option<Hit>,
    panel_open: bool,
    panel_scroll: f64,
    /// The settings window's category, in `chrome::panel_sections()`.
    panel_tab: usize,
    /// Double-clicking a PDF opens this program; checked when the settings panel opens and when
    /// the window comes back from Windows' Default apps page.
    is_default: bool,
    /// The page number being typed into the toolbar's page box.
    editing: Option<String>,
    title: String,
}

impl App {
    fn window(&self) -> &Window {
        self.window.as_ref().expect("window is created on resume")
    }

    fn dpi(&self) -> f64 {
        self.window().scale_factor()
    }

    fn redraw_soon(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn dark(&self) -> bool {
        match self.settings.theme {
            Theme::System => self.system_dark,
            Theme::Light => false,
            Theme::Dark | Theme::Black => true,
        }
    }

    fn palette(&self) -> Palette {
        match self.settings.theme {
            Theme::Black => BLACK,
            _ if self.dark() => DARK,
            _ => LIGHT,
        }
    }

    /// The title bar's color when the theme sets one: black with the Black theme.
    fn caption(&self) -> Option<u32> {
        (self.settings.theme == Theme::Black).then_some(BLACK.bar)
    }

    fn look(&self) -> Look {
        match self.settings.page_colors {
            PageColors::Normal => Look::Normal,
            PageColors::Dark => Look::Dark,
            PageColors::Sepia => Look::Sepia,
            PageColors::Black => Look::Black,
        }
    }

    fn bar_height(&self) -> u32 {
        let shown = self.settings.show_toolbar && self.chrome.is_some() && self.presenting.is_none();
        if shown { Chrome::bar_height(self.dpi()) } else { 0 }
    }

    /// The tab strip shows once there are two tabs.
    fn tabs_height(&self) -> u32 {
        let shown = self.tabs.len() > 1 && self.chrome.is_some() && self.presenting.is_none();
        if shown { Chrome::tabs_height(self.dpi()) } else { 0 }
    }

    /// The annotation tools' bar, under the toolbar, while annotating a PDF.
    fn annot_bar_height(&self) -> u32 {
        let shown = self.annotating && self.is_pdf() && self.chrome.is_some() && self.presenting.is_none();
        if shown { Chrome::bar_height(self.dpi()) } else { 0 }
    }

    /// The tab strip and toolbars together.
    fn top_height(&self) -> u32 {
        self.tabs_height() + self.bar_height() + self.annot_bar_height()
    }

    /// The part of the window below the toolbar and right of the sidebar, where pages go.
    fn view_size(&self) -> PhysicalSize<u32> {
        let s = self.window().inner_size();
        PhysicalSize::new(s.width.saturating_sub(self.sidebar_width()), s.height.saturating_sub(self.top_height()))
    }

    fn has_outline(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| !d.outline.is_empty())
    }

    /// The document has an outline or favorites for the sidebar to show.
    fn has_sidebar(&self) -> bool {
        self.has_outline() || self.doc.as_ref().is_some_and(|d| !self.favorites.of(&d.path).is_empty())
    }

    fn is_favorite(&self, page: usize) -> bool {
        self.doc.as_ref().is_some_and(|d| self.favorites.has(&d.path, page))
    }

    /// Ctrl+D: bookmarks `page`, named after its section in the outline, and offers to rename it;
    /// or removes its bookmark.
    fn toggle_favorite(&mut self, page: usize) {
        let Some(doc) = &self.doc else { return };
        let path = doc.path.clone();
        if self.favorites.has(&path, page) {
            self.favorites.remove(&path, page);
            self.status = Some(format!("Removed the bookmark on page {}", page + 1));
        } else {
            let section = doc.outline.iter().rev().find(|n| matches!(n.target, Some(Target::Page { page: p, .. }) if p <= page));
            let name = section.map_or_else(|| format!("Page {}", page + 1), |n| n.title.clone());
            self.favorites.add(&path, page, &name);
            self.sidebar_favorites = true;
            self.open_editor(EditFor::Bookmark(page), name);
        }
        let _ = self.favorites.save();
        self.redraw_soon();
    }

    /// The right-click menu of bookmark `i` in the sidebar.
    fn open_bookmark_menu(&mut self, i: usize, pos: PhysicalPosition<f64>) {
        let Some(page) = self.doc.as_ref().and_then(|d| Some(self.favorites.of(&d.path).get(i)?.page)) else { return };
        self.show_menu(pos, vec![Command::RenameBookmark(page), Command::RemoveFavorite(page)]);
    }

    /// Opens the text box, for `purpose`, holding `text`.
    fn open_editor(&mut self, purpose: EditFor, text: String) {
        let title = match purpose {
            EditFor::NewNote(..) => "Note",
            EditFor::NewText(..) => "Text",
            EditFor::Bookmark(_) => "Bookmark name",
            EditFor::Mark(i) => match self.doc.as_ref().and_then(|d| d.marks.get(i)).map(|m| &m.shape) {
                Some(Shape::Note(..)) => "Note",
                _ => "Text",
            },
        };
        let selected = !text.is_empty();
        self.editor = Some(EditorState { purpose, title, text, selected });
        self.menu = None;
        self.redraw_soon();
    }

    /// Typing into the text box. Returns whether the key was used (all are, while it's open).
    fn editor_key(&mut self, event: &KeyEvent) -> bool {
        if self.editor.is_none() {
            return false;
        }
        let ctrl = self.modifiers.control_key();
        let pasted = if ctrl && event.logical_key == Key::Character("v".into()) { win::paste_text(self.window()) } else { None };
        let Some(ed) = &mut self.editor else { return false };
        let multiline = !matches!(ed.purpose, EditFor::Bookmark(_));
        let backspace = event.logical_key == Key::Named(NamedKey::Backspace);
        let typed = !ctrl && event.text.as_deref().is_some_and(|t| !t.chars().any(char::is_control));
        if std::mem::take(&mut ed.selected) && (pasted.is_some() || backspace || typed) {
            ed.text.clear();
            if backspace {
                self.redraw_soon();
                return true;
            }
        }
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.editor = None,
            Key::Named(NamedKey::Enter) if multiline && !ctrl => ed.text.push('\n'),
            Key::Named(NamedKey::Enter) => self.finish_editor(),
            Key::Named(NamedKey::Backspace) => {
                ed.text.pop();
            }
            _ => match (pasted, event.text.as_deref().filter(|t| !t.chars().any(char::is_control))) {
                (Some(p), _) => ed.text.push_str(&if multiline { p.replace("\r\n", "\n") } else { p.lines().collect::<Vec<_>>().join(" ") }),
                (None, Some(t)) if !ctrl => ed.text.push_str(t),
                _ => {}
            },
        }
        self.redraw_soon();
        true
    }

    /// OK in the text box: names the bookmark, or makes or changes the note or text box. An empty
    /// note or text box isn't made, and one emptied is removed.
    fn finish_editor(&mut self) {
        let Some(ed) = self.editor.take() else { return };
        let text = ed.text.trim_end().to_owned();
        let empty = text.trim().is_empty();
        let color = ANNOT_COLORS[self.annot_colors[1]];
        let size = TEXT_SIZES[self.annot_width];
        match ed.purpose {
            EditFor::Bookmark(page) => {
                if let Some(path) = self.doc.as_ref().map(|d| d.path.clone())
                    && !empty
                {
                    self.favorites.rename(&path, page, &text);
                    let _ = self.favorites.save();
                }
            }
            EditFor::NewNote(page, at) | EditFor::NewText(page, at) => {
                if let Some(doc) = self.doc.as_mut().filter(|_| !empty) {
                    // PDFium draws every note's icon yellow, so it's yellow here too
                    let (shape, color) = match ed.purpose {
                        EditFor::NewNote(..) => (Shape::Note(at, text), annots::NOTE_COLOR),
                        _ => (Shape::Text(at, text, size), color),
                    };
                    doc.marks.push(annots::Mark { page, color, shape, legacy: false });
                }
            }
            EditFor::Mark(i) => {
                if let Some(doc) = &mut self.doc
                    && let Some(m) = doc.marks.get_mut(i)
                {
                    match &mut m.shape {
                        Shape::Note(_, t) | Shape::Text(_, t, _) if !empty => *t = text,
                        _ => {
                            doc.marks.remove(i);
                        }
                    }
                }
            }
        }
        self.redraw_soon();
    }

    /// The outline sidebar's width, or 0 when it's hidden.
    fn sidebar_width(&self) -> u32 {
        if self.has_sidebar() && self.settings.show_outline && self.chrome.is_some() && self.presenting.is_none() {
            let w = self.window().inner_size().width;
            ((self.settings.sidebar_width as f64 * self.dpi()).round() as u32).min(w / 2)
        } else {
            0
        }
    }

    /// Where the pages' area starts in the window.
    fn view_origin(&self) -> (f64, f64) {
        (f64::from(self.sidebar_width()), f64::from(self.top_height()))
    }

    fn render_threads(&self) -> usize {
        match self.settings.render_threads {
            n if n > 0 => n as usize,
            _ => std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4),
        }
    }

    fn new_pool(&self, engine: Arc<dyn Engine>) -> RenderPool {
        let proxy = Mutex::new(self.proxy.clone());
        RenderPool::new(engine, self.render_threads(), move || {
            let _ = proxy.lock().unwrap().send_event(Wake::Redraw);
        })
    }

    /// Opens `path` in a new tab, or shows its tab if it's open already. It opens at `page` if
    /// given, otherwise where it was left last time.
    fn open(&mut self, path: &Path, page: Option<usize>, zoom: Option<Zoom>) {
        self.error = None;
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        if let Some(i) = self.tabs.iter().position(|t| history::same_file(&t.path, &path)) {
            self.activate(i);
            if let Some(z) = zoom {
                self.set_zoom(z, (0.0, 0.0));
            }
            if let Some(p) = page {
                self.jump_to_page(p);
            }
            return;
        }
        match self.load(&path, zoom, None) {
            Ok(doc) => self.add_tab(doc, page),
            Err(Failure::NeedsPassword) => self.prompt = Some(PasswordPrompt { path, page, zoom, typed: String::new(), wrong: false }),
            Err(Failure::Other(e)) => self.error = Some(e),
        }
        self.redraw_soon();
    }

    /// Shows a document just read in a new tab.
    fn add_tab(&mut self, doc: Doc, page: Option<usize>) {
        self.stash();
        self.tabs.push(Tab { path: doc.path.clone(), name: doc.name.clone(), doc: None });
        self.active = self.tabs.len() - 1;
        self.show_loaded(doc, page);
        self.save_history();
    }

    /// Tries the password typed into the password box.
    fn submit_password(&mut self) {
        let Some(prompt) = self.prompt.take() else { return };
        match self.load(&prompt.path, prompt.zoom, Some(&prompt.typed)) {
            Ok(doc) => self.add_tab(doc, prompt.page),
            Err(Failure::NeedsPassword) => self.prompt = Some(PasswordPrompt { typed: String::new(), wrong: true, ..prompt }),
            Err(Failure::Other(e)) => self.error = Some(e),
        }
        self.redraw_soon();
    }

    /// Typing into the password box. Returns whether the key was used.
    fn prompt_key(&mut self, event: &KeyEvent) -> bool {
        let Some(prompt) = &mut self.prompt else { return false };
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Enter) => self.submit_password(),
            Key::Named(NamedKey::Escape) => self.prompt = None,
            Key::Named(NamedKey::Backspace) => {
                prompt.typed.pop();
            }
            _ => {
                if let Some(t) = event.text.as_deref().filter(|t| !t.chars().any(char::is_control)) {
                    prompt.typed.push_str(t);
                }
            }
        }
        self.redraw_soon();
        true
    }

    /// Reads a document and starts its threads. Its zoom is `zoom`, else the one it had last time,
    /// else the setting for new documents.
    fn load(&self, path: &Path, zoom: Option<Zoom>, password: Option<&str>) -> Result<Doc, Failure> {
        self.load_from(path, zoom, password, None)
    }

    /// Reads a document as `load` does, or, given `data`, a PDF with changes not saved to its file.
    fn load_from(&self, path: &Path, zoom: Option<Zoom>, password: Option<&str>, data: Option<Vec<u8>>) -> Result<Doc, Failure> {
        let name = file_name(path);
        let opened = match (&data, password) {
            (Some(d), p) => engine::open_pdf_bytes(d.clone(), p.unwrap_or("")),
            (None, Some(p)) => engine::open_with_password(path, p),
            (None, None) => engine::open(path),
        };
        let engine = opened.map_err(|e| match e {
            engine::OpenError::NeedsPassword => Failure::NeedsPassword,
            e => Failure::Other(format!("Couldn't open {name}: {e}")),
        })?;
        let seen = self.history.find(path).and_then(|e| parse_zoom(&e.zoom));
        let (rotation, spread) = self.history.find(path).map_or((0, Spread::One), |e| parse_view(&e.view));
        let sizes = (0..engine.page_count()).map(|i| engine.page_size(i)).collect();
        let pool = Some(self.new_pool(engine.clone()));
        let proxy = Mutex::new(self.proxy.clone());
        let text = TextWorker::new(engine.clone(), move || {
            let _ = proxy.lock().unwrap().send_event(Wake::Redraw);
        });
        let mut outline = Vec::new();
        flatten(&engine.outline(), 0, &mut outline);
        // highlights kept in highlights.txt before they went into PDFs: shown, and saved into the
        // file the next time it's saved
        let marks = match (&data, engine.page_transform(0)) {
            (None, Some(_)) => self
                .highlights
                .of(path)
                .into_iter()
                .map(|h| annots::Mark { page: h.page, color: h.color, shape: Shape::Markup(Markup::Highlight, h.rects.clone()), legacy: true })
                .collect(),
            _ => Vec::new(),
        };
        Ok(Doc {
            path: path.to_owned(),
            password: password.map(str::to_owned),
            has_form: engine.has_form(),
            fields: None,
            saved: Vec::new(),
            marks,
            memory: data,
            annotations: HashMap::new(),
            filling: false,
            focus_field: None,
            name,
            engine,
            sizes,
            pool,
            cache: TileCache::default(),
            layout: Layout::new(&[], 0.0, 0, 0),
            zoom: zoom.or(seen).unwrap_or(self.settings.start_zoom.into()),
            rotation,
            spread,
            x: 0.0,
            y: 0.0,
            ahead: 1,
            text,
            selection: None,
            current: None,
            jump: false,
            query: String::new(),
            links: HashMap::new(),
            outline,
            sidebar_scroll: 0.0,
            back: Vec::new(),
            forward: Vec::new(),
        })
    }

    /// Shows a document just read: at `page` if given, else where it was left last time.
    fn show_loaded(&mut self, doc: Doc, page: Option<usize>) {
        let seen = self.history.find(&doc.path).map(|e| Spot { page: e.page, y: e.y });
        self.doc = Some(doc);
        self.relayout((0.0, 0.0));
        match (page, seen) {
            (Some(p), _) => self.go_to_page(p),
            (None, Some(spot)) => self.show_spot(spot),
            (None, None) => self.go_to_page(0),
        }
        self.run_search(false);
    }

    /// Puts the shown document back in its tab, letting go of what only a shown document needs:
    /// its tiles, render threads and the engine's caches.
    fn stash(&mut self) {
        self.reset_view_state();
        let Some(mut doc) = self.doc.take() else { return };
        if self.settings.remember_files {
            remember(&mut self.history, &doc);
        }
        doc.pool = None;
        doc.cache = TileCache::default();
        doc.engine.trim();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.doc = Some(doc);
        }
    }

    /// Shows tab `i`, reading its file first if this is the first look at it.
    fn activate(&mut self, i: usize) {
        if i >= self.tabs.len() || (i == self.active && self.doc.is_some()) {
            return;
        }
        self.stash();
        self.active = i;
        match self.tabs[i].doc.take() {
            Some(mut doc) => {
                doc.pool = Some(self.new_pool(doc.engine.clone()));
                let stale = doc.query != self.search;
                self.doc = Some(doc);
                // the window may have changed while the tab was in the background
                self.relayout((0.0, 0.0));
                if stale {
                    self.run_search(false);
                }
            }
            None => {
                let path = self.tabs[i].path.clone();
                match self.load(&path, None, None) {
                    Ok(doc) => self.show_loaded(doc, None),
                    Err(failure) => {
                        match failure {
                            // asked for again; it opens in a new tab once it comes
                            Failure::NeedsPassword => {
                                self.prompt = Some(PasswordPrompt { path, page: None, zoom: None, typed: String::new(), wrong: false })
                            }
                            // gone since last time, or on a drive that isn't there now
                            Failure::Other(e) => self.error = Some(e),
                        }
                        self.tabs.remove(i);
                        self.active = i.min(self.tabs.len().saturating_sub(1));
                        self.activate(self.active);
                    }
                }
            }
        }
        self.save_history();
        self.redraw_soon();
    }

    /// Closes tab `i`. The window stays, showing the next tab or the start page.
    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() || !self.confirm_close(i) {
            return;
        }
        let shown = i == self.active;
        if shown {
            self.reset_view_state();
            if let Some(doc) = self.doc.take()
                && self.settings.remember_files
            {
                remember(&mut self.history, &doc);
            }
        }
        self.remove_tab(i);
        if shown {
            self.activate(self.active);
        }
        self.save_history();
        self.redraw_soon();
    }

    /// Takes tab `i` out of the list, remembering its file for Ctrl+Shift+T.
    fn remove_tab(&mut self, i: usize) {
        let tab = self.tabs.remove(i);
        self.closed.retain(|p| !history::same_file(p, &tab.path));
        self.closed.push(tab.path);
        if self.closed.len() > CLOSED {
            self.closed.remove(0);
        }
        if i < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }

    /// Closes the tabs `close` picks, keeping tab `keep` and showing it.
    fn close_tabs_except(&mut self, keep: usize, close: impl Fn(usize) -> bool) {
        if keep >= self.tabs.len() {
            return;
        }
        // shown first, so no closed tab gets read just to be closed
        self.activate(keep);
        for i in (0..self.tabs.len()).rev() {
            if i != self.active && close(i) && self.confirm_close(i) {
                self.remove_tab(i);
            }
        }
        self.save_history();
        self.redraw_soon();
    }

    /// Ctrl+Shift+T: opens the last closed tab's file again, where it was left.
    fn reopen_closed(&mut self) {
        while let Some(path) = self.closed.pop() {
            if !self.tabs.iter().any(|t| history::same_file(&t.path, &path)) {
                self.open(&path, None, None);
                return;
            }
        }
    }

    /// Moves tab `from` to place `to`, keeping the same tab shown.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = match self.active {
            a if a == from => to,
            a if from < a && to >= a => a - 1,
            a if from > a && to <= a => a + 1,
            a => a,
        };
        self.tab_drag = Some(to);
        self.redraw_soon();
    }

    /// Ctrl+W: closes the shown tab, or the window if there's none.
    fn close_current(&mut self, el: &ActiveEventLoop) {
        if self.tabs.is_empty() {
            self.close(el);
        } else if self.doc.is_some() {
            self.close_tab(self.active);
        }
    }

    /// The toolbar's Update. The installed copy, after asking (and about changes not saved),
    /// downloads the new setup, which installs once this copy has closed and then starts the new one.
    /// A portable copy opens the release's page, to download it from there.
    fn start_update(&mut self) {
        let Some(release) = self.update.clone().filter(|_| !self.downloading) else { return };
        if !update::installed() || release.setup.is_none() {
            win::open_link(&release.page);
            return;
        }
        let ask = format!("Install Flores PDF {} now? Flores PDF closes, updates, and opens again.", release.version);
        if !matches!(win::ask(self.window(), &ask), win::Answer::Yes) {
            return;
        }
        for i in 0..self.tabs.len() {
            if !self.confirm_close(i) {
                return;
            }
        }
        self.downloading = true;
        self.status = Some(format!("Downloading Flores PDF {}…", release.version));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = proxy.send_event(Wake::UpdateReady(update::download(&release)));
        });
        self.redraw_soon();
    }

    /// The start page, with the tabs kept: the shown document goes back into its tab, and clicking
    /// the tab shows it again.
    fn go_home(&mut self) {
        if self.doc.is_some() {
            self.stash();
            self.save_history();
            self.redraw_soon();
        }
    }

    /// Moves to the next tab (`dir` 1) or the previous one (-1), wrapping around.
    fn next_tab(&mut self, dir: isize) {
        let n = self.tabs.len() as isize;
        if n > 1 {
            self.activate((self.active as isize + dir).rem_euclid(n) as usize);
        }
    }

    /// Forgets anything that points into the shown document: drags, hover, the page box, the menu.
    fn reset_view_state(&mut self) {
        self.editing = None;
        self.selecting = false;
        self.drag = None;
        self.thumb_drag = None;
        self.link_tip = None;
        self.pressed_link = None;
        self.menu = None;
        self.props = None;
        self.glide = None;
        self.drawing = None;
        self.editor = None;
    }

    /// The tabs open when the reader last closed, unread until each is shown.
    fn restore_session(&mut self) {
        if !self.settings.remember_files || !self.settings.restore_session {
            return;
        }
        let mut open: Vec<&Entry> = self.history.entries.iter().filter(|e| e.tab > 0).collect();
        open.sort_by_key(|e| e.tab);
        self.active = open.iter().position(|e| e.active).unwrap_or(0);
        self.tabs = open.into_iter().map(|e| Tab { path: e.path.clone(), name: file_name(&e.path), doc: None }).collect();
    }

    /// Writes the recent files, with where the reader is in the shown one and which are open.
    fn save_history(&mut self) {
        self.watch_tabs();
        if !self.settings.remember_files {
            return;
        }
        if let Some(doc) = &self.doc {
            remember(&mut self.history, doc);
        }
        for e in &mut self.history.entries {
            (e.tab, e.active) = (0, false);
        }
        for (i, t) in self.tabs.iter().enumerate() {
            if let Some(e) = self.history.entries.iter_mut().find(|e| history::same_file(&e.path, &t.path)) {
                (e.tab, e.active) = (i + 1, i == self.active);
            }
        }
        let _ = self.history.save();
    }

    /// Watches the open tabs' files, if reloading is on.
    fn watch_tabs(&self) {
        let paths = if self.settings.reload_changed { self.tabs.iter().map(|t| t.path.clone()).collect() } else { Vec::new() };
        self.watcher.watch(paths);
    }

    /// `path` changed on disk: reads it again if it's shown, keeping the view; a tab in the
    /// background reads it when it's next shown.
    fn file_changed(&mut self, path: &Path) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| history::same_file(&t.path, path)) {
            tab.doc = None;
        }
        let Some(doc) = self.doc.as_ref().filter(|d| history::same_file(&d.path, path)) else { return };
        if has_unsaved(doc) {
            // reading it again would lose what was typed into its form
            self.status = Some(format!("{} changed on disk; it's read again once your entries are saved or the tab closes", doc.name));
            self.redraw_soon();
            return;
        }
        // half-written, or gone for now: keep showing what was there, and try at the next change
        let _ = self.reload_shown(None);
    }

    /// Reads the shown document again, from its file or from `data` (the PDF with changes not saved
    /// yet), keeping the view: zoom, rotation, place, history, and form filling.
    fn reload_shown(&mut self, data: Option<Vec<u8>>) -> Result<(), String> {
        let Some(doc) = &self.doc else { return Ok(()) };
        let (path, zoom, rotation, spread, password, filling) = (doc.path.clone(), doc.zoom, doc.rotation, doc.spread, doc.password.clone(), doc.filling);
        let spot = self.spot_at(0.0);
        let mut new = self.load_from(&path, Some(zoom), password.as_deref(), data).map_err(|f| match f {
            Failure::NeedsPassword => "it needs a password".to_owned(),
            Failure::Other(e) => e,
        })?;
        (new.rotation, new.spread) = (rotation, spread);
        if let Some(old) = self.doc.take() {
            (new.back, new.forward, new.sidebar_scroll) = (old.back, old.forward, old.sidebar_scroll);
        }
        self.reset_view_state();
        self.doc = Some(new);
        self.relayout((0.0, 0.0));
        if let Some(spot) = spot {
            self.show_spot(spot);
        }
        self.run_search(false);
        if filling {
            self.toggle_filling();
        }
        self.redraw_soon();
        Ok(())
    }

    /// The start page's list.
    fn recent_files(&self) -> Vec<Recent> {
        if !self.settings.remember_files {
            return Vec::new();
        }
        let dpi = self.dpi();
        let (w, h) = ((chrome::THUMB_W * dpi).round() as u32, (chrome::THUMB_H * dpi).round() as u32);
        let entries = self.history.entries.iter().take(RECENT);
        entries
            .map(|e| Recent {
                name: file_name(&e.path),
                folder: e.path.parent().map_or_else(String::new, |p| p.display().to_string()),
                page: e.page + 1,
                thumb: self.thumbs.get(&e.path, w, h),
            })
            .collect()
    }

    fn open_dialog(&mut self) {
        if let Some(path) = win::open_dialog(self.window()) {
            self.open(&path, None, None);
        }
    }

    /// Rebuilds the layout if the zoom, view width, gap or DPI changed, keeping the document point
    /// under `anchor` (a position in the view) where it was.
    fn relayout(&mut self, anchor: (f64, f64)) {
        if self.window.is_none() {
            return;
        }
        let size = self.view_size();
        let dpi = self.dpi();
        // presenting, pages fill the screen, a screen apart, so only one shows at a time
        let (fit_gap, gap) = match self.presenting {
            Some(_) => (0, size.height),
            None => {
                let gap = (self.settings.page_gap as f64 * dpi).round() as u32;
                (gap, gap)
            }
        };
        let Some(doc) = &mut self.doc else { return };
        let scale = scale_for(doc.zoom, &doc.sizes, size, fit_gap, dpi, doc.rotation, doc.spread);
        let old = &doc.layout;
        let same_view = old.rotation == doc.rotation && old.spread == doc.spread;
        if scale == old.scale && gap == old.gap && size.width == old.view_width && same_view {
            return;
        }
        let new = Layout::arranged(&doc.sizes, scale, gap, size.width, doc.rotation, doc.spread);
        // a scroll on its way was aimed in the old layout's pixels
        self.glide = None;
        // a new rotation or spread moves pages about: the caller shows the page again
        if !old.pages.is_empty() && same_view {
            let (ax, ay) = (doc.x + anchor.0, doc.y + anchor.1);
            let i = old.page_at(ay as i64);
            let (p, q) = (old.pages[i], new.pages[i]);
            let k = f64::from(scale) / f64::from(old.scale);
            doc.x = q.x as f64 + (ax - p.x as f64) * k - anchor.0;
            doc.y = q.y as f64 + (ay - p.y as f64) * k - anchor.1;
        }
        doc.layout = new;
        clamp_scroll(doc, size);
    }

    /// Moves the view there at once, stopping any smooth scroll.
    fn scroll_to(&mut self, x: f64, y: f64) {
        self.glide = None;
        self.place(x, y);
    }

    fn place(&mut self, x: f64, y: f64) {
        // the link that was under the mouse has moved away; the next mouse move finds what's there
        self.link_tip = None;
        let size = self.view_size();
        let Some(doc) = &mut self.doc else { return };
        if y != doc.y {
            doc.ahead = if y > doc.y { 1 } else { -1 };
        }
        (doc.x, doc.y) = (x, y);
        clamp_scroll(doc, size);
        self.redraw_soon();
    }

    /// Scrolls by (dx, dy) from where a smooth scroll on its way is going, gliding there if
    /// smooth scrolling is on. Quick turns of the wheel add up and keep the glide going.
    fn scroll_by(&mut self, dx: f64, dy: f64) {
        let size = self.view_size();
        let Some(doc) = &self.doc else { return };
        let (x, y) = self.glide.map_or((doc.x, doc.y), |g| g.to);
        let max = ((doc.layout.width - i64::from(size.width)).max(0) as f64, (doc.layout.height - i64::from(size.height)).max(0) as f64);
        let to = ((x + dx).clamp(0.0, max.0), (y + dy).clamp(0.0, max.1));
        if !self.settings.smooth_scroll {
            self.scroll_to(to.0, to.1);
            return;
        }
        let from = (doc.x, doc.y);
        if to != from {
            self.glide = Some(Glide { from, to, start: Instant::now() });
            self.link_tip = None;
            self.redraw_soon();
        }
    }

    /// Moves a smooth scroll on to where it should be now: fast at first, easing in to the end.
    fn step_glide(&mut self) {
        let Some(g) = self.glide else { return };
        let t = g.start.elapsed().as_secs_f64() / GLIDE.as_secs_f64();
        let (x, y) = if t >= 1.0 {
            self.glide = None;
            g.to
        } else {
            let k = 1.0 - (1.0 - t).powi(3);
            (g.from.0 + (g.to.0 - g.from.0) * k, g.from.1 + (g.to.1 - g.from.1) * k)
        };
        self.place(x, y);
    }

    fn go_to_page(&mut self, page: usize) {
        let Some(doc) = &self.doc else { return };
        let Some(p) = doc.layout.pages.get(page.min(doc.layout.pages.len().saturating_sub(1))) else { return };
        let y = (p.y - i64::from(doc.layout.gap)) as f64;
        self.scroll_to(doc.x, y);
    }

    fn current_page(&self) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        Some(doc.layout.current_page(&viewport(doc, self.view_size())))
    }

    fn zoom_percent(&self) -> Option<f32> {
        let doc = self.doc.as_ref()?;
        Some(doc.layout.scale / actual_size(self.dpi()) * 100.0)
    }

    fn set_zoom(&mut self, zoom: Zoom, anchor: (f64, f64)) {
        if let Some(doc) = &mut self.doc {
            doc.zoom = zoom;
        }
        self.relayout(anchor);
        self.redraw_soon();
    }

    /// Moves to the next zoom level up (`dir` 1) or down (-1).
    fn zoom_step(&mut self, dir: i32, anchor: (f64, f64)) {
        let Some(cur) = self.zoom_percent() else { return };
        // skip levels within 5% of the current zoom: fit page can land at 66% and 66.67% is a level
        let next = if dir > 0 {
            ZOOM_STEPS.iter().copied().find(|&z| z > cur * 1.05)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|&z| z < cur / 1.05)
        };
        if let Some(z) = next {
            self.set_zoom(Zoom::Percent(z), anchor);
        }
    }

    /// Center of the view, in view coordinates.
    fn center(&self) -> (f64, f64) {
        let s = self.view_size();
        (f64::from(s.width) / 2.0, f64::from(s.height) / 2.0)
    }

    /// The page under a window position and the point on it, in points.
    fn page_point(&self, pos: PhysicalPosition<f64>) -> Option<(usize, f32, f32)> {
        let doc = self.doc.as_ref()?;
        let (left, top) = self.view_origin();
        if pos.y < top || pos.x < left || doc.layout.pages.is_empty() {
            return None;
        }
        let (dx, dy) = (doc.x + pos.x - left, doc.y + pos.y - top);
        let page = doc.layout.page_at_point(dx as i64, dy as i64);
        let (x, y) = from_doc(doc, page, dx, dy);
        Some((page, x, y))
    }

    /// Whether there's text under a window position.
    fn over_text(&self, pos: PhysicalPosition<f64>) -> bool {
        let Some((page, x, y)) = self.page_point(pos) else { return false };
        let text = self.doc.as_ref().and_then(|d| d.text.text(page));
        text.is_some_and(|t| text::char_at(&t, x, y).is_some())
    }

    /// Where the top of the view is, `down` pixels into the view.
    fn spot_at(&self, down: f64) -> Option<Spot> {
        doc_spot(self.doc.as_ref()?, down)
    }

    fn show_spot(&mut self, spot: Spot) {
        let Some(doc) = &self.doc else { return };
        let Some(p) = doc.layout.pages.get(spot.page) else { return };
        let y = p.y as f64 + f64::from(spot.y) * f64::from(doc.layout.scale);
        self.scroll_to(doc.x, y);
    }

    /// Goes to `to`, remembering where the view was for going back.
    fn jump(&mut self, to: Spot) {
        let here = self.spot_at(0.0);
        if let Some(doc) = &mut self.doc {
            if let Some(here) = here {
                doc.back.push(here);
                if doc.back.len() > HISTORY {
                    doc.back.remove(0);
                }
            }
            doc.forward.clear();
        }
        self.show_spot(to);
    }

    fn jump_to_page(&mut self, page: usize) {
        let Some(doc) = &self.doc else { return };
        let gap = f64::from(doc.layout.gap) / f64::from(doc.layout.scale);
        self.jump(Spot { page: page.min(doc.sizes.len().saturating_sub(1)), y: -gap as f32 });
    }

    /// Back to where the last jump came from (`back`), or forward again.
    fn go_history(&mut self, back: bool) {
        let here = self.spot_at(0.0);
        let Some(doc) = &mut self.doc else { return };
        let (from, to) = if back { (&mut doc.back, &mut doc.forward) } else { (&mut doc.forward, &mut doc.back) };
        let Some(spot) = from.pop() else { return };
        if let Some(here) = here {
            to.push(here);
        }
        self.show_spot(spot);
    }

    /// Follows a link or an outline entry.
    fn follow(&mut self, target: &Target) {
        match target {
            Target::Page { page, y: Some(y) } => {
                // a little room above the spot the PDF names, which is usually a heading's top
                let Some(spot) = self.doc.as_ref().and_then(|d| target_spot(d, *page, *y)) else { return };
                self.jump(Spot { y: spot.y - 12.0, ..spot });
            }
            Target::Page { page, y: None } => self.jump_to_page(*page),
            Target::Uri(uri) => {
                win::open_link(uri);
            }
        }
    }

    /// The link under a window position, and where it is in the window.
    fn link_at(&mut self, pos: PhysicalPosition<f64>) -> Option<(Link, kurbo::Rect)> {
        let (page, x, y) = self.page_point(pos)?;
        let (left, top) = self.view_origin();
        let doc = self.doc.as_mut()?;
        let engine = &doc.engine;
        let links = doc.links.entry(page).or_insert_with(|| engine.page_links(page));
        let link = links.iter().find(|l| l.rect[0] <= x && x <= l.rect[2] && l.rect[1] <= y && y <= l.rect[3])?.clone();
        let r = rect_to_doc(doc, page, link.rect) + kurbo::Vec2::new(left - doc.x, top - doc.y);
        Some((link, r))
    }

    /// The outline entries to show (those whose parents are all expanded), and the one for where
    /// the reader is.
    fn outline_rows(&self) -> (Vec<OutlineRow>, Option<usize>) {
        let Some(doc) = &self.doc else { return (Vec::new(), None) };
        // a quarter of the way down the view counts as where the reader is
        let here = self.spot_at(f64::from(self.view_size().height) / 4.0);
        let mut rows = Vec::new();
        let mut current = None;
        let mut hidden_below: Option<usize> = None;
        for (i, n) in doc.outline.iter().enumerate() {
            if let Some(d) = hidden_below {
                if n.depth > d {
                    continue;
                }
                hidden_below = None;
            }
            if let (Some(Target::Page { page, y }), Some(here)) = (&n.target, here)
                && let Some(at) = target_spot(doc, *page, y.unwrap_or(0.0))
                && (at.page, at.y) <= (here.page, here.y)
            {
                current = Some(i);
            }
            rows.push(OutlineRow { node: i, title: n.title.clone(), depth: n.depth, children: n.children, open: n.open });
            if n.children && !n.open {
                hidden_below = Some(n.depth);
            }
        }
        (rows, current)
    }

    /// Restarts the search after its text changed, from the current page.
    fn search_changed(&mut self) {
        self.run_search(true);
    }

    /// Runs the search box's text on the shown document, from the current page, going to the first
    /// match if `jump`.
    fn run_search(&mut self, jump: bool) {
        let page = self.current_page().unwrap_or(0);
        let query = self.search.clone();
        if let Some(doc) = &mut self.doc {
            doc.text.search(&query, page);
            doc.current = None;
            doc.jump = jump && !query.trim().is_empty();
            doc.query = query;
        }
        self.redraw_soon();
    }

    /// Moves to the next match (`dir` 1) or the previous one (-1), wrapping around. Without a
    /// current match, starts from the current page.
    fn step_match(&mut self, dir: i64) {
        let page = self.current_page().unwrap_or(0);
        let Some(doc) = &self.doc else { return };
        let target = doc.text.with_matches(|m| {
            if m.is_empty() {
                return None;
            }
            let len = m.len() as i64;
            let i = match doc.current {
                Some(key) => {
                    let at = m.partition_point(|x| (x.page, x.chars.start) < key);
                    let on_it = m.get(at).is_some_and(|x| (x.page, x.chars.start) == key);
                    match (on_it, dir > 0) {
                        (true, _) => at as i64 + dir,
                        (false, true) => at as i64,
                        (false, false) => at as i64 - 1,
                    }
                }
                None if dir > 0 => m.partition_point(|x| x.page < page) as i64,
                None => m.partition_point(|x| x.page <= page) as i64 - 1,
            };
            let found = &m[i.rem_euclid(len) as usize];
            let first = found.rects.first().copied().unwrap_or_default();
            let bounds = found.rects.iter().fold(first, |a, r| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]);
            Some(((found.page, found.chars.start), bounds))
        });
        let Some((key, bounds)) = target else { return };
        if let Some(doc) = &mut self.doc {
            doc.current = Some(key);
            doc.jump = false;
        }
        self.show_rect(key.0, bounds);
    }

    /// Scrolls so `rect` (points on `page`) is in view, if it isn't already.
    fn show_rect(&mut self, page: usize, rect: [f32; 4]) {
        let size = self.view_size();
        let Some(doc) = &self.doc else { return };
        if page >= doc.layout.pages.len() {
            return;
        }
        let r = rect_to_doc(doc, page, rect);
        let (x0, y0, x1, y1) = (r.x0, r.y0, r.x1, r.y1);
        let (w, h) = (f64::from(size.width), f64::from(size.height));
        let x = if x0 < doc.x || x1 > doc.x + w { x0 - w / 3.0 } else { doc.x };
        let y = if y0 < doc.y || y1 > doc.y + h { y0 - h / 3.0 } else { doc.y };
        self.scroll_to(x, y);
    }

    /// "3 of 17", "No results" or "Searching…" for the toolbar.
    fn search_status(&self) -> String {
        let Some(doc) = self.doc.as_ref().filter(|_| !self.search.trim().is_empty()) else { return String::new() };
        let p = doc.text.progress();
        let more = if p.finished() { "" } else { "…" };
        match (p.found, doc.current) {
            (0, _) if p.finished() => "No results".to_owned(),
            (0, _) => "Searching…".to_owned(),
            (n, Some(key)) => {
                let i = doc.text.with_matches(|m| m.partition_point(|x| (x.page, x.chars.start) < key));
                format!("{} of {n}{more}", i + 1)
            }
            (n, None) => format!("{n} found{more}"),
        }
    }

    /// The selected text, if there is any.
    fn selected_text(&self) -> Option<String> {
        let doc = self.doc.as_ref()?;
        let (a, b) = doc.selection?.ordered();
        let mut out = String::new();
        for page in a.0..=b.0 {
            // pages scrolled out of view may not have their text at hand anymore
            let text = doc.text.text(page).unwrap_or_else(|| Arc::new(doc.engine.page_text(page)));
            if let Some(range) = doc.selection?.on(page, text.chars.len()) {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&text::text_of(&text, range));
            }
        }
        (!out.trim().is_empty()).then_some(out)
    }

    /// Ctrl+H, Ctrl+U: highlights, underlines or strikes out the selected text in the color picked,
    /// and drops the selection. In a PDF the mark waits for Ctrl+S to go into the file; other
    /// documents can only be highlighted, and those highlights are kept in highlights.txt.
    fn mark_selection(&mut self, kind: Markup) {
        let pdf = self.is_pdf();
        // underlines and strikeouts in the drawing color: a highlight's pale one barely shows as a line
        let color = ANNOT_COLORS[self.annot_colors[usize::from(kind != Markup::Highlight)]];
        if !pdf && kind != Markup::Highlight {
            self.status = Some("Only PDFs can be underlined or struck out".to_owned());
            self.redraw_soon();
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        let Some(sel) = doc.selection.take() else { return };
        let (a, b) = sel.ordered();
        for page in a.0..=b.0 {
            let text = doc.text.text(page).unwrap_or_else(|| Arc::new(doc.engine.page_text(page)));
            if let Some(range) = sel.on(page, text.chars.len()) {
                let rects = text::line_rects(&text, range);
                if rects.is_empty() {
                    continue;
                }
                if pdf {
                    doc.marks.push(annots::Mark { page, color, shape: Shape::Markup(kind, rects), legacy: false });
                } else {
                    self.highlights.add(highlights::Highlight { path: doc.path.clone(), page, color, rects });
                }
            }
        }
        if !pdf {
            let _ = self.highlights.save();
        }
        self.redraw_soon();
    }

    /// Shows or hides the annotation tools.
    fn toggle_annotate(&mut self) {
        if !self.is_pdf() {
            self.status = Some("Only PDFs can be annotated; other documents can be highlighted (Ctrl+H)".to_owned());
        } else if self.mark_font.is_none() {
            self.error = Some("Annotating needs the Segoe UI or Arial font, and Windows has neither".to_owned());
        } else {
            self.annotating = !self.annotating;
            if !self.annotating {
                (self.tool, self.drawing) = (None, None);
            }
        }
        self.redraw_soon();
    }

    /// Picks tool `i` (in `Tool::ALL`), or puts it down if it's the one in hand.
    fn pick_tool(&mut self, i: usize) {
        let tool = Tool::ALL.get(i).copied();
        self.tool = if self.tool == tool { None } else { tool };
        self.drawing = None;
        self.redraw_soon();
    }

    /// Picks color `i` for the tool in hand: highlighting, or the rest. A highlight color is the one
    /// Ctrl+H uses too, and the settings remember it.
    fn pick_color(&mut self, i: usize) {
        let highlight = self.tool == Some(Tool::Highlight);
        self.annot_colors[usize::from(!highlight)] = i.min(ANNOT_COLORS.len() - 1);
        if highlight && i < 4 {
            let k = FIELDS.iter().position(|f| f.key == "highlight_color").unwrap();
            self.change(k, Value::Choice(i));
        }
        self.redraw_soon();
    }

    /// Ctrl+Z: takes back the last annotation not saved yet.
    fn undo_mark(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        match doc.marks.iter().rposition(|m| !m.legacy) {
            Some(i) => {
                doc.marks.remove(i);
            }
            None if doc.memory.is_some() => {
                self.status = Some("Removing an annotation can't be undone, but closing without saving keeps the file as it was".to_owned())
            }
            None => self.status = Some("Nothing to undo".to_owned()),
        }
        self.redraw_soon();
    }

    /// The annotations in the file on `page`, read once per page.
    fn saved_annotations(&mut self, page: usize) -> Vec<engine::Annotation> {
        let Some(doc) = &mut self.doc else { return Vec::new() };
        let engine = &doc.engine;
        doc.annotations.entry(page).or_insert_with(|| engine.page_annotations(page)).clone()
    }

    /// The annotation under a window position, the one on top: made last.
    fn annotation_at(&mut self, pos: PhysicalPosition<f64>) -> Option<Found> {
        let (page, x, y) = self.page_point(pos)?;
        let saved = if self.is_pdf() { self.saved_annotations(page) } else { Vec::new() };
        let font = self.mark_font.as_mut()?;
        let doc = self.doc.as_ref()?;
        if let Some(i) = doc.marks.iter().rposition(|m| m.page == page && m.contains(font, x, y)) {
            return Some(Found::Mark(i));
        }
        let inside = |a: &&engine::Annotation| a.rect[0] <= x && x <= a.rect[2] && a.rect[1] <= y && y <= a.rect[3];
        saved.iter().rev().find(inside).map(|a| Found::Saved(page, a.clone()))
    }

    /// What a note or an annotation's comment says, for a tooltip at the mouse.
    fn annotation_tip(&mut self, pos: PhysicalPosition<f64>) -> Option<(kurbo::Rect, String)> {
        let text = match self.annotation_at(pos)? {
            Found::Mark(i) => match &self.doc.as_ref()?.marks.get(i)?.shape {
                Shape::Note(_, t) => t.clone(),
                _ => return None,
            },
            Found::Saved(_, a) => a.contents,
        };
        let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = if line.chars().count() > 120 { format!("{}…", line.chars().take(120).collect::<String>()) } else { line };
        (!line.is_empty()).then(|| (kurbo::Rect::new(pos.x - 6.0, pos.y - 6.0, pos.x + 6.0, pos.y + 6.0), line))
    }

    /// Removes an annotation: one not saved yet at once; one in the file by writing the PDF again,
    /// in memory, until Ctrl+S.
    fn erase(&mut self, found: Found) {
        match found {
            Found::Mark(i) => {
                let Some(doc) = &mut self.doc else { return };
                if i >= doc.marks.len() {
                    return;
                }
                let m = doc.marks.remove(i);
                if let (true, Shape::Markup(_, rects)) = (m.legacy, &m.shape)
                    && let Some(r) = rects.first()
                {
                    self.highlights.remove_at(&doc.path, m.page, (r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
                    let _ = self.highlights.save();
                }
            }
            Found::Saved(page, a) => self.remove_saved(page, a.index, a.rect),
        }
        self.redraw_soon();
    }

    /// Takes annotation `index` of `page` (at `rect`, in points of the page as it is) out of the PDF.
    /// The PDF is written again in memory, with the annotations not saved yet put in too, and read
    /// from there until Ctrl+S writes it to the file.
    fn remove_saved(&mut self, page: usize, index: usize, rect: [f32; 4]) {
        let Some(t) = self.doc.as_ref().and_then(|d| d.engine.page_transform(page)) else { return };
        let removal = pdfium::Removal { page, index, rect: page_to_pdf(t, rect) };
        match self.rewrite(&[removal]).and_then(|bytes| self.reload_shown(Some(bytes))) {
            Ok(()) => self.status = Some("Annotation removed. Ctrl+S saves the change into the file".to_owned()),
            Err(e) => self.error = Some(format!("Couldn't remove the annotation: {e}")),
        }
    }

    /// The PDF with this document's changes made: form entries, the annotations not saved yet, and
    /// `remove` taken out.
    fn rewrite(&mut self, remove: &[pdfium::Removal]) -> Result<Vec<u8>, String> {
        let doc = self.doc.as_ref().ok_or("no document is open")?;
        let fields: Vec<pdfium::Field> = doc.fields.iter().flatten().filter(|e| e.edited).map(|e| e.field.clone()).collect();
        let mut add = Vec::new();
        if !doc.marks.is_empty() {
            let font = self.mark_font.as_mut().ok_or("annotating needs the Segoe UI or Arial font")?;
            for m in &doc.marks {
                if let Some(t) = doc.engine.page_transform(m.page) {
                    add.push(m.to_pdf(t, font));
                }
            }
        }
        let read;
        let base: &[u8] = match &doc.memory {
            Some(b) => b,
            None => {
                read = std::fs::read(&doc.path).map_err(|e| e.to_string())?;
                &read
            }
        };
        pdfium::get()?.apply(base, doc.password.as_deref(), &pdfium::Changes { fields: &fields, add: &add, remove })
    }

    /// The marks on the pages in view and the one being drawn, as pieces in window coordinates.
    /// Highlights aren't among them: they're tinted in with the search matches.
    fn mark_pieces(&mut self) -> Vec<Vec<Piece>> {
        let size = self.view_size();
        let (left, top) = self.view_origin();
        let drawing = self.drawing_mark();
        let Some(font) = self.mark_font.as_mut() else { return Vec::new() };
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let view = viewport(doc, size);
        let pages = doc.layout.pages_between(view.y, view.y + i64::from(view.height));
        let s = f64::from(doc.layout.scale);
        let mut out = Vec::new();
        for m in doc.marks.iter().chain(drawing.as_ref()) {
            if !pages.contains(&m.page) || matches!(m.shape, Shape::Markup(Markup::Highlight, _)) {
                continue;
            }
            let at = |x: f32, y: f32| {
                let (dx, dy) = to_doc(doc, m.page, x, y);
                (dx + left - doc.x, dy + top - doc.y)
            };
            let map = annots::affine(at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0));
            let pieces = m.pieces(font).into_iter().map(|p| {
                let stroke = if p.stroke > 0.0 { (p.stroke * s).max(1.0) } else { 0.0 };
                Piece { path: map * p.path, stroke, ..p }
            });
            out.push(pieces.collect());
        }
        out
    }

    /// The mark a drag with a tool is making.
    fn drawing_mark(&self) -> Option<annots::Mark> {
        let (tool, page, points) = self.drawing.as_ref()?;
        let color = ANNOT_COLORS[self.annot_colors[1]];
        annots::dragged(*tool, *page, color, LINE_WIDTHS[self.annot_width], *points.first()?, *points.last()?, points)
    }

    /// Where a window position is on `page`, even off its edges.
    fn point_on(&self, pos: PhysicalPosition<f64>, page: usize) -> Option<[f32; 2]> {
        let doc = self.doc.as_ref()?;
        if page >= doc.layout.pages.len() {
            return None;
        }
        let (left, top) = self.view_origin();
        let (x, y) = from_doc(doc, page, doc.x + pos.x - left, doc.y + pos.y - top);
        Some([x, y])
    }

    /// A press on the page with a tool that isn't for text: starts drawing, opens the text box for
    /// a note or text, or erases.
    fn tool_press(&mut self, tool: Tool, pos: PhysicalPosition<f64>) {
        let Some((page, x, y)) = self.page_point(pos) else { return };
        match tool {
            Tool::Note => self.open_editor(EditFor::NewNote(page, [x - annots::NOTE_SIZE / 2.0, y - annots::NOTE_SIZE / 2.0]), String::new()),
            Tool::Text => self.open_editor(EditFor::NewText(page, [x, y]), String::new()),
            Tool::Eraser => {
                if let Some(found) = self.annotation_at(pos) {
                    self.erase(found);
                }
            }
            _ => self.drawing = Some((tool, page, vec![[x, y]])),
        }
    }

    fn remove_highlight(&mut self, page: usize, x: f32, y: f32) {
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else { return };
        if self.highlights.remove_at(&path, page, x, y) {
            let _ = self.highlights.save();
        }
    }

    /// Puts the selected text on the clipboard.
    fn copy_selection(&self) {
        if let Some(text) = self.selected_text() {
            win::copy_text(self.window(), &text);
        }
    }

    /// The selection on one line, cut to a search's length.
    fn selection_query(&self) -> Option<String> {
        let text = self.selected_text()?;
        let words: Vec<&str> = text.split_whitespace().collect();
        Some(words.join(" ").chars().take(200).collect())
    }

    /// Searches the document for the selected text.
    fn find_selection(&mut self) {
        if let Some(query) = self.selection_query() {
            self.search = query;
            self.search_fresh = true;
            self.search_changed();
        }
    }

    /// Opens the web search the settings name for the selected text, in the browser.
    fn search_web(&self) {
        if let Some(query) = self.selection_query() {
            win::open_link(&format!("{}{}", self.settings.web_search.url(), url_encode(&query)));
        }
    }

    /// `page` drawn whole at the page image resolution, in its own colors, as 0x00RRGGBB pixels:
    /// width, height, resolution and pixels. Very big pages get a lower resolution.
    fn page_image(&self, page: usize) -> Option<(u32, u32, u32, Vec<u32>)> {
        let doc = self.doc.as_ref()?;
        let size = doc.sizes.get(page)?;
        let mut dpi = self.settings.image_dpi as f64;
        let points = f64::from(size.width) * f64::from(size.height);
        dpi = dpi.min((MAX_IMAGE_PIXELS / points).sqrt() * 72.0).min(65_000.0 * 72.0 / f64::from(size.width.max(size.height)));
        let scale = (dpi / 72.0) as f32;
        let (w, h) = (((size.width * scale).round() as u32).max(1), ((size.height * scale).round() as u32).max(1));
        let pixels = doc.engine.renderer().render(page, scale, engine::PixelRect { x: 0, y: 0, width: w, height: h });
        // the render threads would hold on to what this drew until they next go idle
        doc.engine.trim();
        Some((w, h, dpi.round() as u32, pixels))
    }

    fn copy_page_image(&mut self, page: usize) {
        let Some((w, h, dpi, pixels)) = self.page_image(page) else { return };
        if !win::copy_image(self.window(), w, h, dpi, &pixels) {
            self.error = Some("Couldn't copy the page: another program is using the clipboard".to_owned());
        }
    }

    fn save_page_image(&mut self, page: usize) {
        let Some(doc) = &self.doc else { return };
        let stem = doc.path.file_stem().map_or_else(|| "page".to_owned(), |s| s.to_string_lossy().into_owned());
        let name = format!("{stem} - page {}.png", page + 1);
        let Some(path) = win::save_dialog(self.window(), &name, "PNG image (*.png)\0*.png\0\0", "png") else { return };
        let Some((w, h, dpi, pixels)) = self.page_image(page) else { return };
        if let Err(e) = std::fs::write(&path, encode_png(w, h, dpi, &pixels)) {
            self.error = Some(format!("Couldn't save {}: {e}", file_name(&path)));
        }
    }

    /// The toolbar's pencil: starts filling in the form, reading its fields with PDFium the first
    /// time, or hides the fields again (what was typed stays until the tab closes).
    fn toggle_filling(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        if doc.filling {
            (doc.filling, doc.focus_field) = (false, None);
            self.redraw_soon();
            return;
        }
        if doc.fields.is_none() {
            let read = pdfium::get().and_then(|p| {
                let data = match &doc.memory {
                    Some(m) => m.clone(),
                    None => std::fs::read(&doc.path).map_err(|e| e.to_string())?,
                };
                p.read_fields(&data, doc.password.as_deref())
            });
            match read {
                Ok(fields) => {
                    let entries = fields
                        .into_iter()
                        .filter_map(|field| {
                            let rect = pdf_to_page(doc.engine.page_transform(field.page)?, field.rect);
                            Some(FormEntry { field, rect, edited: false })
                        })
                        .collect();
                    doc.fields = Some(entries);
                    doc.saved = field_values(doc);
                }
                Err(e) => {
                    self.error = Some(format!("Can't fill in forms: {e}"));
                    self.redraw_soon();
                    return;
                }
            }
        }
        doc.filling = true;
        let n = doc.fields.as_ref().map_or(0, Vec::len);
        self.status = Some(match n {
            0 => "This form has no fields that can be filled in here".to_owned(),
            _ => "Click a field to fill it in; Tab goes to the next. Ctrl+S saves your entries into the file".to_owned(),
        });
        self.redraw_soon();
    }

    /// The fields to draw over the pages in view, in window coordinates.
    fn field_looks(&self) -> Vec<FieldLook> {
        let Some(doc) = self.doc.as_ref().filter(|d| d.filling) else { return Vec::new() };
        let Some(entries) = &doc.fields else { return Vec::new() };
        let view = viewport(doc, self.view_size());
        let pages = doc.layout.pages_between(view.y, view.y + i64::from(view.height));
        let (left, top) = self.view_origin();
        let s = f64::from(doc.layout.scale);
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| pages.contains(&e.field.page))
            .map(|(i, e)| {
                let r = rect_to_doc(doc, e.field.page, e.rect) + kurbo::Vec2::new(left - doc.x, top - doc.y);
                let shape = match e.field.kind {
                    pdfium::FieldKind::Text { .. } => FieldShape::Text,
                    pdfium::FieldKind::Check => FieldShape::Check,
                    pdfium::FieldKind::Radio => FieldShape::Radio,
                    pdfium::FieldKind::Choice(_) => FieldShape::Choice,
                };
                // the text a size that fits a one-line field, and not larger than 12 points
                let one_line = f64::from(e.rect[3] - e.rect[1]).min(f64::from(e.rect[2] - e.rect[0]));
                let font = (one_line * 0.62).clamp(4.0, 12.0) * s;
                FieldLook { rect: r, kind: shape, text: e.field.value.clone(), checked: e.field.checked, focused: doc.focus_field == Some(i), font }
            })
            .collect()
    }

    /// A press while filling in: on a field, types into it, ticks it or opens its choices.
    /// Returns whether a field took the press.
    fn field_click(&mut self, pos: PhysicalPosition<f64>) -> bool {
        let point = self.page_point(pos);
        let Some(doc) = self.doc.as_mut().filter(|d| d.filling) else { return false };
        let Some(entries) = doc.fields.as_mut() else { return false };
        let hit = point.and_then(|(page, x, y)| {
            entries.iter().position(|e| e.field.page == page && e.rect[0] <= x && x <= e.rect[2] && e.rect[1] <= y && y <= e.rect[3])
        });
        let Some(i) = hit else {
            // a press elsewhere stops typing into a field
            return doc.focus_field.take().is_some();
        };
        doc.focus_field = None;
        match entries[i].field.kind.clone() {
            pdfium::FieldKind::Text { .. } => doc.focus_field = Some(i),
            pdfium::FieldKind::Check => {
                entries[i].field.checked = !entries[i].field.checked;
                entries[i].edited = true;
            }
            pdfium::FieldKind::Radio => {
                // one of a group: the group shares a name
                let name = entries[i].field.name.clone();
                for (j, e) in entries.iter_mut().enumerate() {
                    if e.field.name == name && matches!(e.field.kind, pdfium::FieldKind::Radio) {
                        e.field.checked = j == i;
                        e.edited = true;
                    }
                }
            }
            pdfium::FieldKind::Choice(options) => {
                let commands = (0..options.len()).map(|o| Command::PickOption(i, o)).collect();
                self.show_menu(pos, commands);
            }
        }
        true
    }

    fn pick_option(&mut self, field: usize, option: usize) {
        let Some(entry) = self.doc.as_mut().and_then(|d| d.fields.as_mut()?.get_mut(field)) else { return };
        if let pdfium::FieldKind::Choice(options) = &entry.field.kind
            && let Some(o) = options.get(option)
        {
            entry.field.value = o.clone();
            entry.edited = true;
        }
    }

    /// Typing into the field being filled in. Returns whether the key was used.
    fn field_key(&mut self, event: &KeyEvent) -> bool {
        let shift = self.modifiers.shift_key();
        if self.modifiers.control_key() {
            return false;
        }
        let Some(doc) = &mut self.doc else { return false };
        let (Some(i), Some(entries)) = (doc.focus_field, doc.fields.as_mut()) else { return false };
        let multiline = matches!(entries[i].field.kind, pdfium::FieldKind::Text { multiline: true });
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => doc.focus_field = None,
            Key::Named(NamedKey::Enter) if multiline => {
                entries[i].field.value.push('\n');
                entries[i].edited = true;
            }
            Key::Named(NamedKey::Enter) => doc.focus_field = None,
            Key::Named(NamedKey::Tab) => {
                // the next text field, in reading order: page, then top to bottom, left to right
                let order = |e: &FormEntry| (e.field.page, (e.rect[1] * 4.0) as i64, (e.rect[0] * 4.0) as i64);
                let mut texts: Vec<usize> = (0..entries.len()).filter(|&j| matches!(entries[j].field.kind, pdfium::FieldKind::Text { .. })).collect();
                texts.sort_by_key(|&j| order(&entries[j]));
                if let Some(k) = texts.iter().position(|&j| j == i) {
                    let n = texts.len();
                    let next = texts[if shift { (k + n - 1) % n } else { (k + 1) % n }];
                    doc.focus_field = Some(next);
                    let (page, rect) = (entries[next].field.page, entries[next].rect);
                    self.show_rect(page, rect);
                }
            }
            Key::Named(NamedKey::Backspace) => {
                entries[i].field.value.pop();
                entries[i].edited = true;
            }
            _ => match event.text.as_deref().filter(|t| !t.chars().any(char::is_control)) {
                Some(t) => {
                    entries[i].field.value.push_str(t);
                    entries[i].edited = true;
                }
                None => return false,
            },
        }
        self.redraw_soon();
        true
    }

    /// Whether the shown document is a PDF, which can be annotated and saved through PDFium.
    fn is_pdf(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.engine.page_transform(0).is_some())
    }

    /// Ctrl+S: writes the form entries and annotations into the PDF itself, through PDFium. Returns
    /// whether everything is saved now.
    fn save(&mut self) -> bool {
        let pdf = self.is_pdf();
        let Some(doc) = &self.doc else { return true };
        if !pdf {
            self.status = Some("Only PDFs can be saved with form entries and annotations".to_owned());
            self.redraw_soon();
            return true;
        }
        if !has_unsaved(doc) {
            self.status = Some("No changes to save".to_owned());
            self.redraw_soon();
            return true;
        }
        let legacy = doc.marks.iter().any(|m| m.legacy);
        let (path, name) = (doc.path.clone(), doc.name.clone());
        // read from memory while the file is replaced, so this reader isn't holding it open
        let shown = self.rewrite(&[]).and_then(|bytes| {
            self.reload_shown(Some(bytes.clone()))?;
            Ok(bytes)
        });
        let saved = shown.and_then(|bytes| write_in_place(&path, &bytes).map_err(|e| e.to_string()));
        self.redraw_soon();
        match saved {
            Ok(()) => {
                if legacy {
                    self.highlights.forget(&path);
                    let _ = self.highlights.save();
                }
                // the file now holds what's in memory: read it from there again
                let _ = self.reload_shown(None);
                if let Some(doc) = &mut self.doc {
                    doc.memory = None;
                }
                self.status = Some(format!("Saved {name}"));
                true
            }
            Err(e) => {
                self.error = Some(format!("Couldn't save {name}: {e}. The changes stay here until the tab closes; Save a copy (Ctrl+Shift+S) may work"));
                false
            }
        }
    }

    /// Ctrl+Shift+S: writes the PDF with its form entries and annotations to another file. The open
    /// one isn't changed, and what isn't saved in it stays that way.
    fn save_copy(&mut self) {
        let pdf = self.is_pdf();
        let Some(doc) = &self.doc else { return };
        if !pdf {
            self.status = Some("Only PDFs can be saved with form entries and annotations".to_owned());
            self.redraw_soon();
            return;
        }
        let stem = doc.path.file_stem().map_or_else(|| "document".to_owned(), |s| s.to_string_lossy().into_owned());
        let path = doc.path.clone();
        let Some(out) = win::save_dialog(self.window(), &format!("{stem} (copy).pdf"), "PDF document (*.pdf)\0*.pdf\0\0", "pdf") else { return };
        if history::same_file(&out, &path) {
            self.save();
            return;
        }
        match self.rewrite(&[]).and_then(|bytes| std::fs::write(&out, bytes).map_err(|e| e.to_string())) {
            Ok(()) => self.status = Some(format!("Saved a copy as {}", file_name(&out))),
            Err(e) => self.error = Some(format!("Couldn't save the copy: {e}")),
        }
        self.redraw_soon();
    }

    /// Before tab `i` closes: if it has changes not saved, asks whether to save them first. Returns
    /// whether to go on closing.
    fn confirm_close(&mut self, i: usize) -> bool {
        let doc = if i == self.active { self.doc.as_ref() } else { self.tabs.get(i).and_then(|t| t.doc.as_ref()) };
        let Some(doc) = doc.filter(|d| has_unsaved(d)) else { return true };
        let text = format!("\u{201C}{}\u{201D} has changes that aren't saved: form entries or annotations. Save them into the file before closing?", doc.name);
        match win::ask(self.window(), &text) {
            win::Answer::No => true,
            win::Answer::Cancel => false,
            win::Answer::Yes => {
                self.activate(i);
                self.save()
            }
        }
    }

    /// Ctrl+V: the clipboard's text into whatever takes typing. Returns whether something did.
    fn paste(&mut self) -> bool {
        let Some(text) = win::paste_text(self.window()) else { return false };
        if let Some(prompt) = &mut self.prompt {
            prompt.typed.push_str(text.lines().next().unwrap_or(""));
        } else if let Some(doc) = &mut self.doc
            && let (Some(i), Some(entries)) = (doc.focus_field, doc.fields.as_mut())
        {
            let multiline = matches!(entries[i].field.kind, pdfium::FieldKind::Text { multiline: true });
            let text = if multiline { text } else { text.lines().collect::<Vec<_>>().join(" ") };
            entries[i].field.value.push_str(&text);
            entries[i].edited = true;
        } else if self.search_focus {
            if self.search_fresh {
                self.search.clear();
            }
            self.search.push_str(&text.split_whitespace().collect::<Vec<_>>().join(" "));
            self.search_fresh = false;
            self.search_changed();
        } else if let Some(page) = &mut self.editing {
            page.extend(text.chars().filter(char::is_ascii_digit).take(6usize.saturating_sub(page.len())));
        } else {
            return false;
        }
        self.redraw_soon();
        true
    }

    /// Ctrl+P: asks which printer and pages, then prints on a thread of its own.
    fn print(&mut self) {
        if self.printing.is_some() {
            self.status = Some("Already printing. Esc stops it".to_owned());
            self.redraw_soon();
            return;
        }
        let Some((engine, name, pages)) = self.doc.as_ref().map(|d| (d.engine.clone(), d.name.clone(), d.sizes.len())) else { return };
        let current = self.current_page().unwrap_or(0);
        let Some(printer) = win::print_dialog(self.window(), pages, current) else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        self.printing = Some(cancel.clone());
        let proxy = self.proxy.clone();
        let started = std::thread::Builder::new().name("print".into()).stack_size(8 << 20).spawn(move || {
            let result = print::print(&printer, engine, &name, &cancel, |done, total| {
                let _ = proxy.send_event(Wake::Printing(done, total));
            });
            let _ = proxy.send_event(Wake::Printed(result));
        });
        if started.is_err() {
            self.printing = None;
            self.error = Some("Couldn't start printing".to_owned());
        }
        self.redraw_soon();
    }

    /// The properties box's rows for the shown document.
    fn properties(&self) -> Option<Vec<(&'static str, String)>> {
        let doc = self.doc.as_ref()?;
        let info = doc.engine.info();
        let page = self.current_page().unwrap_or(0);
        let mut rows = vec![("File", doc.name.clone())];
        if let Some(folder) = doc.path.parent() {
            rows.push(("Folder", folder.display().to_string()));
        }
        if let Ok(meta) = std::fs::metadata(&doc.path) {
            rows.push(("File size", file_size(meta.len())));
        }
        rows.push(("Format", info.format));
        rows.push(("Pages", doc.sizes.len().to_string()));
        if let Some(s) = doc.sizes.get(page) {
            let (w, h) = (f64::from(s.width), f64::from(s.height));
            let size = format!("{:.0} × {:.0} mm ({:.2} × {:.2} in)", w / 72.0 * 25.4, h / 72.0 * 25.4, w / 72.0, h / 72.0);
            rows.push(("Page size", if doc.sizes.len() > 1 { format!("{size}, page {}", page + 1) } else { size }));
        }
        let given = [
            ("Title", info.title),
            ("Author", info.author),
            ("Subject", info.subject),
            ("Keywords", info.keywords),
            ("Created", info.created),
            ("Modified", info.modified),
            ("Application", info.creator),
            ("PDF producer", info.producer),
        ];
        rows.extend(given.into_iter().filter_map(|(label, v)| Some((label, v?))));
        Some(rows)
    }

    /// Selects all text on the current page.
    fn select_page(&mut self) {
        let Some(page) = self.current_page() else { return };
        let Some(doc) = &mut self.doc else { return };
        if let Some(t) = doc.text.text(page) {
            doc.selection = Some(Selection { anchor: (page, 0), focus: (page, t.chars.len()) });
            self.redraw_soon();
        }
    }

    /// Typing into the search box. Returns whether the key was used.
    fn search_key(&mut self, event: &KeyEvent) -> bool {
        if !self.search_focus || self.modifiers.control_key() {
            return false;
        }
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Enter) => self.step_match(if self.modifiers.shift_key() { -1 } else { 1 }),
            Key::Named(NamedKey::Escape) => self.search_focus = false,
            Key::Named(NamedKey::Backspace) => {
                if self.search_fresh {
                    self.search.clear();
                } else {
                    self.search.pop();
                }
                self.search_changed();
            }
            _ => match event.text.as_deref() {
                Some(t) if !t.chars().any(char::is_control) => {
                    if self.search_fresh {
                        self.search.clear();
                    }
                    self.search.push_str(t);
                    self.search_changed();
                }
                _ => return false,
            },
        }
        self.search_fresh = false;
        self.redraw_soon();
        true
    }

    fn focus_search(&mut self) {
        if self.doc.is_some() {
            self.search_focus = true;
            self.search_fresh = true;
            self.editing = None;
            self.redraw_soon();
        }
    }

    fn toggle_outline(&mut self) {
        if self.has_sidebar() {
            let i = FIELDS.iter().position(|f| f.key == "show_outline").unwrap();
            self.change(i, Value::Toggle(!self.settings.show_outline));
        }
    }

    fn toggle_panel(&mut self) {
        self.panel_open = !self.panel_open;
        if self.panel_open {
            self.is_default = win::is_default_pdf_reader();
        } else if let Some(c) = &mut self.chrome {
            // the panel's pixels aren't needed until it opens again
            c.close_panel();
        }
        self.redraw_soon();
    }

    /// Sets field `i` of the settings table and applies the result.
    fn change(&mut self, i: usize, v: Value) {
        let mut s = self.settings.clone();
        (FIELDS[i].set)(&mut s, FIELDS[i].clamp(v));
        self.apply_settings(s);
    }

    fn apply_settings(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        let _ = self.settings.save();
        if old.theme != self.settings.theme {
            let theme = if self.dark() { winit::window::Theme::Dark } else { winit::window::Theme::Light };
            self.window().set_theme(Some(theme));
            win::caption_color(self.window(), self.caption());
        }
        if old.render_threads != self.settings.render_threads
            && let Some(engine) = self.doc.as_ref().map(|d| d.engine.clone())
        {
            let pool = self.new_pool(engine);
            if let Some(doc) = &mut self.doc {
                doc.pool = Some(pool);
            }
        }
        if old.reload_changed != self.settings.reload_changed {
            self.watch_tabs();
        }
        if old.remember_files != self.settings.remember_files {
            if self.settings.remember_files {
                self.save_history();
            } else {
                self.history.clear();
            }
        }
        // gap, toolbar and zoom changes reach the layout on the next frame
        self.redraw_soon();
    }

    fn click(&mut self, hit: Hit) {
        if hit != Hit::PageBox {
            self.editing = None;
        }
        if !matches!(hit, Hit::SearchBox | Hit::SearchPrev | Hit::SearchNext) {
            self.search_focus = false;
        }
        match hit {
            Hit::Tab(i) => self.activate(i),
            Hit::TabClose(i) => self.close_tab(i),
            Hit::NewTab => self.open_dialog(),
            Hit::Recent(i) => {
                if let Some(path) = self.history.entries.get(i).map(|e| e.path.clone()) {
                    self.open(&path, None, None);
                }
            }
            Hit::Forget(i) => {
                if let Some(path) = self.history.entries.get(i).map(|e| e.path.clone()) {
                    self.history.forget(&path);
                    self.save_history();
                }
            }
            // the password box's buttons are handled before clicks get here
            Hit::MenuItem(_) | Hit::Menu | Hit::Props | Hit::Prompt | Hit::PromptOk | Hit::PromptCancel => {}
            Hit::PropsClose => self.props = None,
            Hit::Outline => self.toggle_outline(),
            Hit::OutlineRow(i) => {
                let target = self.doc.as_ref().and_then(|d| d.outline.get(i)?.target.clone());
                if let Some(target) = target {
                    self.follow(&target);
                }
            }
            Hit::OutlineToggle(i) => {
                if let Some(n) = self.doc.as_mut().and_then(|d| d.outline.get_mut(i)) {
                    n.open = !n.open;
                }
            }
            Hit::Sidebar => {}
            Hit::SideTab(favorites) => {
                self.sidebar_favorites = favorites;
                if let Some(doc) = &mut self.doc {
                    doc.sidebar_scroll = 0.0;
                }
            }
            Hit::Favorite(i) => {
                let page = self.doc.as_ref().and_then(|d| Some(self.favorites.of(&d.path).get(i)?.page));
                if let Some(page) = page {
                    self.jump_to_page(page);
                }
            }
            Hit::Unfavorite(i) => {
                let page = self.doc.as_ref().and_then(|d| Some(self.favorites.of(&d.path).get(i)?.page));
                if let Some(page) = page {
                    self.toggle_favorite(page);
                }
            }
            Hit::Annotate => self.toggle_annotate(),
            Hit::Tool(i) => self.pick_tool(i),
            Hit::Color(i) => self.pick_color(i),
            Hit::Width(i) => self.annot_width = i.min(LINE_WIDTHS.len() - 1),
            Hit::Undo => self.undo_mark(),
            Hit::Save => {
                self.save();
            }
            // the text box's buttons are handled before clicks get here
            Hit::Editor | Hit::EditorOk | Hit::EditorCancel | Hit::EditorDelete => {}
            Hit::SearchBox => self.focus_search(),
            Hit::SearchPrev => self.step_match(-1),
            Hit::SearchNext => self.step_match(1),
            Hit::Open => self.open_dialog(),
            Hit::Prev => self.turn_page(-1),
            Hit::Next => self.turn_page(1),
            Hit::PageBox => self.editing = Some(String::new()),
            Hit::ZoomOut => self.zoom_step(-1, self.center()),
            Hit::ZoomIn => self.zoom_step(1, self.center()),
            Hit::FitPage => self.set_zoom(Zoom::FitPage, self.center()),
            Hit::FitWidth => self.set_zoom(Zoom::FitWidth, self.center()),
            Hit::View => self.open_view_menu(),
            Hit::MainMenu => self.open_main_menu(),
            Hit::FillForm => self.toggle_filling(),
            Hit::Settings | Hit::Close => self.toggle_panel(),
            Hit::Toggle(i) => {
                let on = (FIELDS[i].get)(&self.settings) == Value::Toggle(true);
                self.change(i, Value::Toggle(!on));
            }
            Hit::Step(i, dir) => {
                if let (Value::Number(n), settings::Kind::Number { step, .. }) = ((FIELDS[i].get)(&self.settings), &FIELDS[i].kind) {
                    self.change(i, Value::Number(n + dir * step));
                }
            }
            Hit::SettingsTab(k) => (self.panel_tab, self.panel_scroll) = (k, 0.0),
            Hit::Dropdown(i) => {
                if let settings::Kind::Choice(options) = &FIELDS[i].kind {
                    let commands = (0..options.len()).map(|k| Command::SetChoice(i, k)).collect();
                    // under the drop-down, at the card's right, its right edge on the box's
                    let dpi = self.dpi();
                    let at = match self.chrome.as_ref().and_then(|c| c.hit_rect(hit)) {
                        Some(r) => PhysicalPosition::new(r.x1 - 216.0 * dpi, r.center().y + 18.0 * dpi),
                        None => self.cursor,
                    };
                    self.show_menu(at, commands);
                }
            }
            Hit::AboutBox => (self.props_title, self.props) = ("About Flores PDF", Some(about())),
            Hit::Home => self.go_home(),
            Hit::Update => self.start_update(),
            Hit::OpenSettingsFile => {
                let _ = self.settings.save();
                win::open_in_default_program(&settings::path());
            }
            Hit::MakeDefault => {
                if !win::register_as_pdf_reader() {
                    self.error = Some("Couldn't register Flores PDF with Windows".to_owned());
                }
            }
            Hit::Reset => {
                let window = self.settings.window;
                self.apply_settings(Settings { window, ..Settings::default() });
            }
            Hit::Panel => {}
        }
        self.redraw_soon();
    }

    /// Typing into the page box. Returns whether the key was used.
    fn edit_key(&mut self, key: &Key) -> bool {
        let Some(text) = &mut self.editing else { return false };
        match key.as_ref() {
            Key::Character(c) if c.chars().all(|c| c.is_ascii_digit()) && text.len() < 6 => text.push_str(c),
            Key::Named(NamedKey::Backspace) => {
                text.pop();
            }
            Key::Named(NamedKey::Enter) => {
                if let Ok(n) = text.parse::<usize>() {
                    self.jump_to_page(n.saturating_sub(1));
                }
                self.editing = None;
            }
            Key::Named(NamedKey::Escape) => self.editing = None,
            _ => return false,
        }
        self.redraw_soon();
        true
    }

    fn toggle_fullscreen(&mut self) {
        let w = self.window();
        w.set_fullscreen(if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) });
    }

    /// Keys while presenting: forward and back like a slide show, Esc or F5 to stop.
    fn present_key(&mut self, key: &Key) {
        let shift = self.modifiers.shift_key();
        let last = self.doc.as_ref().map_or(0, |d| d.sizes.len().saturating_sub(1));
        match key.as_ref() {
            Key::Named(NamedKey::Space) if shift => self.present_step(-1),
            Key::Named(
                NamedKey::ArrowRight | NamedKey::ArrowDown | NamedKey::PageDown | NamedKey::Space | NamedKey::Enter,
            )
            | Key::Character("n") => self.present_step(1),
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp | NamedKey::PageUp | NamedKey::Backspace) | Key::Character("p") => {
                self.present_step(-1)
            }
            Key::Named(NamedKey::Home) | Key::Named(NamedKey::End) => {
                let to = if *key == Key::Named(NamedKey::Home) { 0 } else { last };
                if let (Some(p), Some(doc)) = (&mut self.presenting, &self.doc) {
                    p.page = row_step(&doc.layout, to, 0);
                }
                self.redraw_soon();
            }
            Key::Named(NamedKey::Escape | NamedKey::F5) => self.toggle_presentation(),
            _ => {}
        }
    }

    fn key(&mut self, el: &ActiveEventLoop, event: KeyEvent) {
        if self.error.take().is_some() | self.status.take().is_some() {
            self.redraw_soon();
        }
        // Esc stops printing
        if event.logical_key == Key::Named(NamedKey::Escape)
            && let Some(cancel) = &self.printing
        {
            cancel.store(true, Ordering::Relaxed);
            return;
        }
        // a key closes the menu, and does nothing else
        if self.menu.take().is_some() {
            self.redraw_soon();
            return;
        }
        if event.logical_key == Key::Named(NamedKey::Escape) && self.props.take().is_some() {
            self.redraw_soon();
            return;
        }
        if self.editor_key(&event) || self.prompt_key(&event) || self.field_key(&event) {
            return;
        }
        let key = event.logical_key.clone();
        if self.modifiers.control_key() && key == Key::Character("v".into()) && self.paste() {
            return;
        }
        if self.presenting.is_some() {
            self.present_key(&key);
            return;
        }
        if self.edit_key(&key) || self.search_key(&event) {
            return;
        }
        let has_selection = self.doc.as_ref().is_some_and(|d| d.selection.is_some());
        let has_search = !self.search.is_empty();
        let (ctrl, shift) = (self.modifiers.control_key(), self.modifiers.shift_key());
        let dpi = self.dpi();
        let size = self.view_size();
        let page_step = f64::from(size.height) * 0.9;
        // with nothing to scroll sideways, left and right turn pages
        let fits = self.doc.as_ref().is_none_or(|d| d.layout.width <= i64::from(size.width));
        match key.as_ref() {
            Key::Named(NamedKey::Tab) if ctrl => self.next_tab(if shift { -1 } else { 1 }),
            Key::Named(NamedKey::PageDown) if ctrl => self.next_tab(1),
            Key::Named(NamedKey::PageUp) if ctrl => self.next_tab(-1),
            Key::Named(NamedKey::F4) if ctrl => self.close_current(el),
            Key::Character("t" | "T") if ctrl && shift => self.reopen_closed(),
            Key::Character("p") if ctrl => self.print(),
            Key::Character("d") if ctrl => self.toggle_favorite(self.current_page().unwrap_or(0)),
            Key::Character("h") if ctrl => self.mark_selection(Markup::Highlight),
            Key::Character("u") if ctrl => self.mark_selection(Markup::Underline),
            Key::Character("z") if ctrl => self.undo_mark(),
            Key::Character("s" | "S") if ctrl && shift => self.save_copy(),
            Key::Character("s") if ctrl => {
                self.save();
            }
            Key::Named(NamedKey::Home) if self.modifiers.alt_key() => self.go_home(),
            Key::Named(NamedKey::ArrowLeft) if self.modifiers.alt_key() => self.go_history(true),
            Key::Named(NamedKey::ArrowRight) if self.modifiers.alt_key() => self.go_history(false),
            Key::Named(NamedKey::ArrowDown) => self.scroll_by(0.0, ARROW_STEP * dpi),
            Key::Named(NamedKey::ArrowUp) => self.scroll_by(0.0, -ARROW_STEP * dpi),
            Key::Named(NamedKey::ArrowRight) if fits => self.turn_page(1),
            Key::Named(NamedKey::ArrowLeft) if fits => self.turn_page(-1),
            Key::Named(NamedKey::ArrowRight) => self.scroll_by(ARROW_STEP * dpi, 0.0),
            Key::Named(NamedKey::ArrowLeft) => self.scroll_by(-ARROW_STEP * dpi, 0.0),
            Key::Named(NamedKey::PageDown) => self.scroll_by(0.0, page_step),
            Key::Named(NamedKey::PageUp) => self.scroll_by(0.0, -page_step),
            Key::Named(NamedKey::Space) => self.scroll_by(0.0, if shift { -page_step } else { page_step }),
            Key::Named(NamedKey::Home) => self.scroll_by(0.0, f64::NEG_INFINITY),
            Key::Named(NamedKey::End) => self.scroll_by(0.0, f64::INFINITY),
            Key::Named(NamedKey::F8) => {
                let i = FIELDS.iter().position(|f| f.key == "show_toolbar").unwrap();
                self.change(i, Value::Toggle(!self.settings.show_toolbar));
            }
            Key::Named(NamedKey::F11) => self.toggle_fullscreen(),
            Key::Named(NamedKey::F5) => self.toggle_presentation(),
            Key::Named(NamedKey::F10) => self.open_main_menu(),
            Key::Character("_" | "-") if ctrl && shift => self.rotate(-1),
            Key::Character("+" | "=") if ctrl && shift => self.rotate(1),
            Key::Character("6") if ctrl => self.set_spread(Spread::One),
            Key::Character("7") if ctrl => self.set_spread(Spread::Two),
            Key::Character("8") if ctrl => self.set_spread(Spread::Book),
            Key::Named(NamedKey::BrowserBack) => self.go_history(true),
            Key::Named(NamedKey::BrowserForward) => self.go_history(false),
            Key::Named(NamedKey::F12) => self.toggle_outline(),
            Key::Named(NamedKey::F3) => self.step_match(if shift { -1 } else { 1 }),
            // Esc closes one thing at a time: the tool in hand, the panel, the selection, the search,
            // full screen
            Key::Named(NamedKey::Escape) if self.tool.is_some() => {
                (self.tool, self.drawing) = (None, None);
                self.redraw_soon();
            }
            Key::Named(NamedKey::Escape) if self.panel_open => self.toggle_panel(),
            Key::Named(NamedKey::Escape) if has_selection => {
                if let Some(doc) = &mut self.doc {
                    doc.selection = None;
                }
                self.redraw_soon();
            }
            Key::Named(NamedKey::Escape) if has_search => {
                self.search.clear();
                self.search_changed();
            }
            Key::Named(NamedKey::Escape) => self.window().set_fullscreen(None),
            Key::Character("f") if ctrl => self.focus_search(),
            Key::Character("c") if ctrl => self.copy_selection(),
            Key::Character("a") if ctrl => self.select_page(),
            Key::Character("o") if ctrl => self.open_dialog(),
            Key::Character("g") if ctrl && self.doc.is_some() => {
                self.editing = Some(String::new());
                self.redraw_soon();
            }
            Key::Character(",") if ctrl => self.toggle_panel(),
            Key::Character("0") if ctrl => self.set_zoom(Zoom::FitPage, self.center()),
            Key::Character("1") if ctrl => self.set_zoom(Zoom::Percent(100.0), self.center()),
            Key::Character("2") if ctrl => self.set_zoom(Zoom::FitWidth, self.center()),
            Key::Character("w") if ctrl => self.close_current(el),
            Key::Character("n") => self.turn_page(1),
            Key::Character("p") => self.turn_page(-1),
            Key::Character("+" | "=") => self.zoom_step(1, self.center()),
            Key::Character("-") => self.zoom_step(-1, self.center()),
            Key::Character("q") => self.close(el),
            _ => {}
        }
    }

    /// Saves the open tabs and where the window is, if wanted, and quits, after asking about any
    /// form entries not saved.
    fn close(&mut self, el: &ActiveEventLoop) {
        for i in 0..self.tabs.len() {
            if !self.confirm_close(i) {
                return;
            }
        }
        self.save_history();
        if self.settings.remember_window
            && let Some(w) = &self.window
        {
            let maximized = w.is_maximized();
            let mut p = self.settings.window.unwrap_or(settings::Placement { x: 0, y: 0, width: 0, height: 0, maximized });
            if !maximized && w.fullscreen().is_none() {
                if let Ok(pos) = w.outer_position() {
                    (p.x, p.y) = (pos.x, pos.y);
                }
                let size = w.inner_size();
                (p.width, p.height) = (size.width, size.height);
            }
            p.maximized = maximized;
            if self.settings.window != Some(p) && p.width > 0 {
                self.settings.window = Some(p);
                let _ = self.settings.save();
            }
        }
        el.exit();
    }

    fn redraw(&mut self) {
        let size = self.window().inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        // the window was resized or moved to a screen with another DPI: keep the top of the view in place
        let view_size = self.view_size();
        self.relayout((f64::from(view_size.width) / 2.0, 0.0));
        self.step_glide();
        self.autoscroll = self.autoscroll_step();
        self.center_presented();
        let presenting = self.presenting.is_some();
        // a new search found its first match: show it
        if self.doc.as_ref().is_some_and(|d| d.jump && d.text.progress().found > 0) {
            self.step_match(1);
        }
        let (dpi, palette, look) = (self.dpi(), self.palette(), self.look());
        let (tabs_h, top_h) = (self.tabs_height(), self.top_height());
        let tabs = (tabs_h > 0).then(|| Tabs {
            width: size.width,
            dpi,
            palette,
            hover: self.hover.filter(|h| matches!(h, Hit::Tab(_) | Hit::TabClose(_) | Hit::NewTab)),
            names: self.tabs.iter().map(|t| t.name.clone()).collect(),
            // none, on the start page
            active: if self.doc.is_some() { self.active } else { usize::MAX },
        });
        let bar = (self.bar_height() > 0).then(|| self.bar_state(size.width, dpi, palette));
        let annot_bar = (self.annot_bar_height() > 0).then(|| AnnotBar {
            top: tabs_h + self.bar_height(),
            width: size.width,
            dpi,
            palette,
            hover: self.hover.filter(|h| matches!(h, Hit::Tool(_) | Hit::Color(_) | Hit::Width(_) | Hit::Undo | Hit::Save)),
            tool: self.tool.and_then(|t| Tool::ALL.iter().position(|x| *x == t)),
            colors: ANNOT_COLORS.to_vec(),
            color: self.annot_colors[usize::from(self.tool != Some(Tool::Highlight))],
            width_choice: self.annot_width,
            can_undo: self.doc.as_ref().is_some_and(|d| d.marks.iter().any(|m| !m.legacy)),
            unsaved: self.doc.as_ref().is_some_and(has_unsaved),
        });
        let marks = self.mark_pieces();
        let panel = self.panel_open.then(|| self.panel_state(size, dpi, palette));
        let side_w = self.sidebar_width();
        let sidebar = (side_w > 0).then(|| {
            let (rows, current) = self.outline_rows();
            Sidebar {
                top: top_h,
                width: side_w,
                height: size.height.saturating_sub(top_h),
                dpi,
                palette,
                hover: self.hover,
                scroll: self.doc.as_ref().map_or(0.0, |d| d.sidebar_scroll),
                rows,
                current,
                has_outline: self.has_outline(),
                showing_favorites: self.sidebar_favorites || !self.has_outline(),
                favorites: self.doc.as_ref().map_or_else(Vec::new, |d| {
                    self.favorites.of(&d.path).iter().map(|f| (f.name.clone(), f.page + 1)).collect()
                }),
            }
        });
        let recent = if self.doc.is_none() && self.chrome.is_some() { self.recent_files() } else { Vec::new() };
        if recent.is_empty() {
            self.thumbs.clear();
        }
        let start = (!recent.is_empty()).then(|| Start {
            left: side_w,
            top: top_h,
            width: view_size.width,
            height: view_size.height,
            dpi,
            palette,
            hover: self.hover,
            recent,
            look,
        });
        let menu = self.menu.as_ref().map(|(at, commands)| Menu {
            x: at.x,
            y: at.y,
            dpi,
            palette,
            hover: self.hover.filter(|h| matches!(h, Hit::MenuItem(_))),
            entries: commands.iter().map(|c| self.menu_entry(c)).collect(),
        });
        let prompt = self.prompt.as_ref().map(|p| Prompt {
            dpi,
            palette,
            hover: self.hover.filter(|h| matches!(h, Hit::PromptOk | Hit::PromptCancel)),
            name: file_name(&p.path),
            typed: p.typed.chars().count(),
            wrong: p.wrong,
        });
        let props = self.props.as_ref().map(|rows| Props {
            title: self.props_title,
            dpi,
            palette,
            hover: self.hover.filter(|h| *h == Hit::PropsClose),
            rows: rows.clone(),
        });
        let editor = self.editor.as_ref().map(|e| Editor {
            dpi,
            palette,
            hover: self.hover.filter(|h| matches!(h, Hit::EditorOk | Hit::EditorCancel | Hit::EditorDelete)),
            title: e.title.to_owned(),
            text: e.text.clone(),
            multiline: !matches!(e.purpose, EditFor::Bookmark(_)),
            can_delete: matches!(e.purpose, EditFor::Mark(_)),
            selected: e.selected,
        });
        let fields = self.field_looks();
        let scrollbar = self.scrollbar();
        let hot = self.on_scrollbar || matches!(self.thumb_drag, Some((false, _)));
        let hbar = self.hscrollbar();
        let hbar_hot = self.on_hbar || matches!(self.thumb_drag, Some((true, _)));
        let origin = self.view_origin();
        let waited = self.pressed.is_none() && self.menu.is_none() && self.hover_since.elapsed() >= TOOLTIP_DELAY;
        let tooltip = self.hover.filter(|_| waited).and_then(|h| Some((h, self.tooltip_text(h)?)));
        let link_tip = self.link_tip.clone().filter(|_| waited);
        let note = match (self.error.as_ref().or(self.status.as_ref()), &self.doc, &start) {
            (Some(e), _, _) => Some(e.clone()),
            (None, None, None) if self.prompt.is_none() => Some("Open a PDF with Ctrl+O, or drop one here".to_owned()),
            _ => None,
        };

        let surface = self.surface.as_mut().expect("surface is created on resume");
        surface.resize(w, h).expect("resizing the window buffer");
        let mut buffer = surface.buffer_mut().expect("getting the window buffer");
        {
            // the pages' area: below the tabs and toolbar, right of the sidebar
            let doc_rows = &mut buffer[(top_h * size.width + side_w) as usize..];
            let stride = size.width as usize;
            let mut frame = Frame { pixels: doc_rows, width: view_size.width, height: view_size.height, stride };
            match &mut self.doc {
                Some(doc) => {
                    for done in doc.pool.iter().flat_map(RenderPool::take_finished) {
                        doc.cache.insert(done.key, done.width, done.height, done.pixels);
                    }
                    clamp_scroll(doc, view_size);
                    let view = viewport(doc, view_size);
                    let screen = view_size.width as usize * view_size.height as usize * 4;
                    doc.cache.set_budget(self.settings.cache_screens as usize * screen);
                    doc.cache.begin_frame();
                    let style = match presenting {
                        true => Style { background: 0, border: None, look },
                        false => Style { background: palette.canvas, border: Some(palette.page_border), look },
                    };
                    render::compose(&mut frame, &doc.layout, &view, &doc.cache, &style);
                    if let Some(pool) = &doc.pool {
                        pool.set_jobs(render::wanted_tiles(&doc.layout, &view, doc.ahead, look, &doc.cache));
                    }
                    doc.cache.evict();
                    let visible: Vec<usize> = doc.layout.pages_between(view.y, view.y + i64::from(view.height)).collect();
                    doc.text.want(&visible);
                    draw_highlights(&mut frame, doc, &view, &self.highlights);

                    if let Some(bar) = scrollbar {
                        draw_scrollbar(&mut frame, bar, origin, dpi, hot, &palette);
                    }
                    if let Some(bar) = hbar {
                        draw_hscrollbar(&mut frame, bar, origin, dpi, hbar_hot, &palette);
                    }
                }
                None => render::fill_rect(&mut frame, 0, 0, view_size.width, view_size.height, palette.canvas),
            }
        }
        if let Some(chrome) = &mut self.chrome {
            match &start {
                Some(start) => chrome.draw_start(&mut buffer, size.width, start),
                None => chrome.close_start(),
            }
            if let Some(note) = &note {
                let mid = kurbo::Rect::new(0.0, 0.0, f64::from(size.width), f64::from(top_h) + f64::from(view_size.height) / 2.0);
                chrome.draw_tooltip(&mut buffer, size.width, size.height, mid, note, dpi, &palette);
            }
            let clip = kurbo::Rect::new(origin.0, origin.1, origin.0 + f64::from(view_size.width), origin.1 + f64::from(view_size.height));
            if !marks.is_empty() {
                chrome.draw_marks(&mut buffer, size.width, clip, &marks);
            }
            if !fields.is_empty() {
                chrome.draw_fields(&mut buffer, size.width, clip, &fields, dpi, &palette);
            }
            match &tabs {
                Some(tabs) => chrome.draw_tabs(&mut buffer, size.width, tabs),
                None => chrome.close_tabs(),
            }
            if let Some(bar) = &bar {
                chrome.draw_bar(&mut buffer, size.width, bar);
            }
            match &annot_bar {
                Some(b) => chrome.draw_annot_bar(&mut buffer, size.width, b),
                None => chrome.close_annot_bar(),
            }
            match &sidebar {
                Some(sidebar) => chrome.draw_sidebar(&mut buffer, size.width, sidebar),
                None => chrome.close_sidebar(),
            }
            if let Some(panel) = &panel {
                chrome.draw_panel(&mut buffer, size.width, panel);
            }
            match &props {
                Some(props) => chrome.draw_props(&mut buffer, size.width, size.height, props),
                None => chrome.close_props(),
            }
            match &prompt {
                Some(prompt) => chrome.draw_prompt(&mut buffer, size.width, size.height, prompt),
                None => chrome.close_prompt(),
            }
            match &editor {
                Some(e) => chrome.draw_editor(&mut buffer, size.width, size.height, e),
                None => chrome.close_editor(),
            }
            match &menu {
                Some(menu) => chrome.draw_menu(&mut buffer, size.width, size.height, menu),
                None => chrome.close_menu(),
            }
            if let Some((hit, text)) = &tooltip
                && let Some(anchor) = chrome.hit_rect(*hit)
            {
                chrome.draw_tooltip(&mut buffer, size.width, size.height, anchor, text, dpi, &palette);
            }
            if let Some((anchor, text)) = &link_tip {
                chrome.draw_tooltip(&mut buffer, size.width, size.height, *anchor, text, dpi, &palette);
            }
        }
        buffer.present().expect("presenting the window buffer");
        self.update_title();
        if self.glide.is_some() || self.autoscroll {
            // the next step of a smooth scroll or a selection's scrolling, on the screen's next frame
            win::wait_for_frame();
            self.redraw_soon();
        }
    }

    fn bar_state(&self, width: u32, dpi: f64, palette: Palette) -> Bar {
        let page = self.current_page().map_or(0, |p| p + 1);
        Bar {
            top: self.tabs_height(),
            width,
            dpi,
            palette,
            hover: self.hover,
            pressed: self.pressed,
            has_doc: self.doc.is_some(),
            page: self.editing.clone().unwrap_or_else(|| page.to_string()),
            editing: self.editing.is_some(),
            pages: self.doc.as_ref().map_or(0, |d| d.sizes.len()),
            zoom: self.zoom_percent().map_or_else(String::new, |z| format!("{z:.0}%")),
            panel_open: self.panel_open,
            search: self.search.clone(),
            search_focus: self.search_focus,
            search_status: self.search_status(),
            has_matches: self.doc.as_ref().is_some_and(|d| d.text.progress().found > 0),
            has_outline: self.has_sidebar(),
            outline_shown: self.sidebar_width() > 0,
            has_form: self.doc.as_ref().is_some_and(|d| d.has_form),
            filling: self.doc.as_ref().is_some_and(|d| d.filling),
            can_annotate: self.is_pdf(),
            annotating: self.annot_bar_height() > 0,
            update: self.update.as_ref().map(|r| match self.downloading {
                true => "Downloading…".to_owned(),
                false => format!("Update to {}", r.version),
            }),
        }
    }

    /// The settings window: centered, up to 860 × 640 logical pixels, and 24 short of the edges.
    fn panel_state(&self, size: PhysicalSize<u32>, dpi: f64, palette: Palette) -> Panel {
        let (w, h) = (f64::from(size.width), f64::from(size.height));
        let width = (PANEL_WIDTH * dpi).min(w - 48.0 * dpi).max((w - 8.0).min(320.0 * dpi)).round().max(1.0);
        let height = (640.0 * dpi).min(h - 48.0 * dpi).max((h - 8.0).min(240.0 * dpi)).round().max(1.0);
        Panel {
            x: ((w - width) / 2.0).max(0.0) as u32,
            y: ((h - height) / 2.0).max(0.0) as u32,
            width: width as u32,
            height: height as u32,
            dpi,
            palette,
            hover: self.hover,
            scroll: self.panel_scroll,
            tab: self.panel_tab,
            settings: self.settings.clone(),
            file: settings::path().display().to_string(),
            is_default: self.is_default,
        }
    }

    fn update_title(&mut self) {
        let title = match (&self.doc, &self.error) {
            (Some(doc), _) if doc.sizes.is_empty() => format!("{} – no pages", doc.name),
            (Some(doc), _) => {
                let percent = self.zoom_percent().unwrap_or(0.0);
                let page = self.current_page().unwrap_or(0) + 1;
                format!("{} – page {page} of {} – {percent:.0}%", doc.name, doc.sizes.len())
            }
            (None, Some(e)) => format!("{e} – Flores PDF"),
            (None, None) => "Flores PDF".to_owned(),
        };
        if title != self.title {
            self.window().set_title(&title);
            self.title = title;
        }
    }

    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if icon != self.cursor_icon {
            self.window().set_cursor(icon);
            self.cursor_icon = icon;
        }
    }

    /// Text for the tooltip of `hit`, if it has one.
    fn tooltip_text(&self, hit: Hit) -> Option<String> {
        match hit {
            Hit::Tab(i) => self.tabs.get(i).map(|t| t.path.display().to_string()),
            _ => hit.tooltip().map(str::to_owned),
        }
    }

    /// The scroll bar's strip at the right edge of the pages, and its thumb, in window
    /// coordinates. None when the document fits the window's height.
    fn scrollbar(&self) -> Option<(kurbo::Rect, kurbo::Rect)> {
        let doc = self.doc.as_ref().filter(|_| self.presenting.is_none())?;
        let size = self.view_size();
        let (total, shown) = (doc.layout.height as f64, f64::from(size.height));
        if total <= shown {
            return None;
        }
        let dpi = self.dpi();
        let (left, top) = self.view_origin();
        let right = left + f64::from(size.width);
        let track = kurbo::Rect::new(right - (SCROLLBAR * dpi).round(), top, right, top + shown);
        let h = (shown * shown / total).max(THUMB_MIN * dpi).min(shown);
        let y = top + (shown - h) * doc.y / (total - shown);
        Some((track, kurbo::Rect::new(track.x0, y, track.x1, y + h)))
    }

    /// The scroll bar along the bottom of the pages and its thumb, when they're wider than the view.
    fn hscrollbar(&self) -> Option<(kurbo::Rect, kurbo::Rect)> {
        let doc = self.doc.as_ref().filter(|_| self.presenting.is_none())?;
        let size = self.view_size();
        let (total, shown) = (doc.layout.width as f64, f64::from(size.width));
        if total <= shown {
            return None;
        }
        let dpi = self.dpi();
        let (left, top) = self.view_origin();
        let bottom = top + f64::from(size.height);
        // short of the vertical bar's corner
        let right = left + shown - if self.scrollbar().is_some() { (SCROLLBAR * dpi).round() } else { 0.0 };
        let track = kurbo::Rect::new(left, bottom - (SCROLLBAR * dpi).round(), right, bottom);
        let w = (track.width() * shown / total).max(THUMB_MIN * dpi).min(track.width());
        let x = left + (track.width() - w) * doc.x / (total - shown);
        Some((track, kurbo::Rect::new(x, track.y0, x + w, track.y1)))
    }

    /// Scrolls so the bottom thumb's left edge is `grab` pixels left of the mouse, which is at `x`.
    fn drag_hthumb(&mut self, x: f64, grab: f64) {
        let Some((track, thumb)) = self.hscrollbar() else { return };
        let Some(doc) = &self.doc else { return };
        let at = ((x - grab - track.x0) / (track.width() - thumb.width()).max(1.0)).clamp(0.0, 1.0);
        let shown = f64::from(self.view_size().width);
        self.scroll_to(at * (doc.layout.width as f64 - shown), doc.y);
    }

    /// Whether `pos` is on the sidebar's right edge, where dragging resizes it.
    fn on_sidebar_edge(&self, pos: PhysicalPosition<f64>) -> bool {
        let w = f64::from(self.sidebar_width());
        w > 0.0 && (pos.x - w).abs() <= SIDEBAR_EDGE * self.dpi() && pos.y >= self.view_origin().1
    }

    /// Moves a selection's loose end to the text nearest `pos`, held inside the pages' area.
    fn extend_selection(&mut self, pos: PhysicalPosition<f64>) {
        let (left, top) = self.view_origin();
        let size = self.view_size();
        let held = PhysicalPosition::new(pos.x.clamp(left, left + f64::from(size.width) - 1.0), pos.y.clamp(top, top + f64::from(size.height) - 1.0));
        if let Some((page, x, y)) = self.page_point(held)
            && let Some(doc) = &mut self.doc
            && let Some(text) = doc.text.text(page)
            && let Some(sel) = &mut doc.selection
        {
            sel.focus = (page, text::boundary_at(&text, x, y));
        }
    }

    /// While a selection is dragged above or below the pages, scrolls toward the mouse, faster the
    /// farther it is. Returns whether it scrolled, so the next frame comes.
    fn autoscroll_step(&mut self) -> bool {
        if !self.selecting {
            return false;
        }
        let (_, top) = self.view_origin();
        let bottom = top + f64::from(self.view_size().height);
        let past = if self.cursor.y < top { self.cursor.y - top } else { (self.cursor.y - bottom).max(0.0) };
        if past == 0.0 {
            self.autoscroll_at = None;
            return false;
        }
        // by time, not frames, which come at whatever rate the screen allows: ten times the
        // distance past the edge each second, up to a few screens
        let now = Instant::now();
        let dt = self.autoscroll_at.map_or(0.016, |t| now.duration_since(t).as_secs_f64().min(0.05));
        self.autoscroll_at = Some(now);
        let speed = (past * 10.0).clamp(-3000.0 * self.dpi(), 3000.0 * self.dpi());
        self.scroll_by_now(0.0, speed * dt);
        self.extend_selection(self.cursor);
        true
    }

    /// Scrolls so the thumb's top is `grab` pixels above the mouse, which is at `y`.
    fn drag_thumb(&mut self, y: f64, grab: f64) {
        let Some((track, thumb)) = self.scrollbar() else { return };
        let Some(doc) = &self.doc else { return };
        let at = ((y - grab - track.y0) / (track.height() - thumb.height()).max(1.0)).clamp(0.0, 1.0);
        self.scroll_to(doc.x, at * (doc.layout.height as f64 - track.height()));
    }

    /// Opens the right-click menu for the page at `pos`, with what applies there.
    fn open_menu(&mut self, pos: PhysicalPosition<f64>) {
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else { return };
        let mut commands = Vec::new();
        let pdf = self.is_pdf();
        if self.selected_text().is_some() {
            commands.extend([Command::Copy, Command::Highlight]);
            if pdf {
                commands.extend([Command::Underline, Command::Strikeout]);
            }
            commands.extend([Command::FindSelection, Command::SearchWeb]);
        }
        if !pdf
            && let Some((page, x, y)) = self.page_point(pos)
            && let Some(doc) = &self.doc
            && self.highlights.on(&doc.path, page).any(|h| h.contains(x, y))
        {
            commands.push(Command::RemoveHighlight(page, x, y));
        }
        match self.annotation_at(pos) {
            Some(Found::Mark(i)) => {
                if self.doc.as_ref().and_then(|d| d.marks.get(i)).is_some_and(|m| matches!(m.shape, Shape::Note(..) | Shape::Text(..))) {
                    commands.push(Command::EditMark(i));
                }
                commands.push(Command::DeleteMark(i));
            }
            Some(Found::Saved(page, a)) => commands.push(Command::DeleteSaved(page, a.index, a.rect)),
            None => {}
        }
        if let Some((link, _)) = self.link_at(pos)
            && let Target::Uri(uri) = link.target
        {
            commands.push(Command::CopyLink(uri));
        }
        if !commands.is_empty() {
            commands.push(Command::Separator);
        }
        let page = self.page_point(pos).map(|p| p.0).or(self.current_page()).unwrap_or(0);
        commands.extend([
            Command::ZoomIn,
            Command::ZoomOut,
            Command::FitPage,
            Command::FitWidth,
            Command::ActualSize,
            Command::Separator,
            if self.is_favorite(page) { Command::RemoveFavorite(page) } else { Command::AddFavorite(page) },
            Command::CopyPageImage(page),
            Command::SavePageImage(page),
            Command::Separator,
            Command::Print,
            Command::Save,
            Command::SaveCopy,
            Command::FillForm,
            Command::ShowInFolder(path.clone()),
            Command::CopyPath(path),
            Command::Properties,
        ]);
        if !pdf {
            commands.retain(|c| !matches!(c, Command::Save | Command::SaveCopy));
        }
        if !self.doc.as_ref().is_some_and(|d| d.has_form && !d.filling) {
            commands.retain(|c| *c != Command::FillForm);
        }
        self.show_menu(pos, commands);
    }

    /// Opens the right-click menu for tab `i`.
    fn open_tab_menu(&mut self, i: usize, pos: PhysicalPosition<f64>) {
        let Some(path) = self.tabs.get(i).map(|t| t.path.clone()) else { return };
        let mut commands = vec![Command::CloseTab(i), Command::CloseOtherTabs(i)];
        if i + 1 < self.tabs.len() {
            commands.push(Command::CloseTabsRight(i));
        }
        if !self.closed.is_empty() {
            commands.push(Command::ReopenClosed);
        }
        commands.extend([Command::Separator, Command::ShowInFolder(path.clone()), Command::CopyPath(path)]);
        self.show_menu(pos, commands);
    }

    /// Opens the main menu below its button, or at the window's top right without a toolbar.
    fn open_main_menu(&mut self) {
        let at = match self.chrome.as_ref().and_then(|c| c.hit_rect(Hit::MainMenu)).filter(|_| self.bar_height() > 0) {
            Some(r) => PhysicalPosition::new(r.x1, r.y1 + 2.0),
            None => PhysicalPosition::new(f64::from(self.window().inner_size().width), f64::from(self.top_height())),
        };
        let has_doc = self.doc.is_some();
        let mut commands = vec![Command::Open];
        if has_doc {
            commands.push(Command::Home);
        }
        if has_doc {
            if self.is_pdf() {
                commands.extend([Command::Save, Command::SaveCopy, Command::Annotate]);
            }
            if self.doc.as_ref().is_some_and(|d| d.marks.iter().any(|m| !m.legacy)) {
                commands.push(Command::UndoMark);
            }
            commands.extend([Command::Print, Command::Properties]);
            if let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) {
                commands.push(Command::ShowInFolder(path));
            }
        }
        if !self.closed.is_empty() {
            commands.push(Command::ReopenClosed);
        }
        if has_doc {
            commands.extend([
                Command::CloseShownTab,
                Command::Separator,
                Command::FirstPage,
                Command::LastPage,
                Command::GoToPage,
                Command::Back,
                Command::Forward,
                Command::Find,
                Command::Separator,
                Command::ToggleToolbar,
                Command::ToggleSidebar,
                Command::FullScreen,
                Command::Presentation,
            ]);
        }
        commands.extend([Command::Separator, Command::OpenSettings, Command::About]);
        self.show_menu(at, commands);
    }

    /// Opens the toolbar's view menu below its button.
    fn open_view_menu(&mut self) {
        let Some(r) = self.chrome.as_ref().and_then(|c| c.hit_rect(Hit::View)) else { return };
        let commands = vec![
            Command::SetSpread(Spread::One),
            Command::SetSpread(Spread::Two),
            Command::SetSpread(Spread::Book),
            Command::Separator,
            Command::RotateLeft,
            Command::RotateRight,
            Command::Separator,
            Command::Presentation,
            Command::FullScreen,
        ];
        self.show_menu(PhysicalPosition::new(r.x0, r.y1 + 2.0), commands);
    }

    /// How a menu shows `command`: with a check mark if it's a choice in effect.
    fn menu_entry(&self, command: &Command) -> MenuEntry {
        match (command, command.entry()) {
            (Command::PickOption(field, option), _) => {
                let entry = self.doc.as_ref().and_then(|d| d.fields.as_ref()?.get(*field));
                match entry.map(|e| &e.field) {
                    Some(pdfium::Field { kind: pdfium::FieldKind::Choice(options), value, .. }) => {
                        let label = options.get(*option).cloned().unwrap_or_default();
                        let on = *value == label;
                        MenuEntry::Choice(label, on)
                    }
                    _ => MenuEntry::Separator,
                }
            }
            (Command::SetChoice(i, k), MenuEntry::Item(label, keys)) => {
                MenuEntry::Check(label, keys, (FIELDS[*i].get)(&self.settings) == Value::Choice(*k))
            }
            (Command::SetSpread(s), MenuEntry::Item(label, keys)) => {
                MenuEntry::Check(label, keys, self.doc.as_ref().is_some_and(|d| d.spread == *s))
            }
            (_, entry) => entry,
        }
    }

    /// Turns the pages a quarter turn clockwise (`dir` 1) or the other way (-1), staying on the page.
    fn rotate(&mut self, dir: i16) {
        let page = self.current_page().unwrap_or(0);
        let Some(doc) = &mut self.doc else { return };
        doc.rotation = (i16::from(doc.rotation) + dir).rem_euclid(4) as u8;
        self.relayout((0.0, 0.0));
        self.go_to_page(page);
    }

    fn set_spread(&mut self, spread: Spread) {
        let page = self.current_page().unwrap_or(0);
        let Some(doc) = &mut self.doc else { return };
        doc.spread = spread;
        self.relayout((0.0, 0.0));
        self.go_to_page(page);
    }

    /// The next page (`dir` 1) or the previous one (-1); the next or previous pair in a spread.
    fn turn_page(&mut self, dir: i64) {
        let page = self.current_page().unwrap_or(0);
        if let Some(to) = self.doc.as_ref().map(|d| row_step(&d.layout, page, dir)) {
            self.go_to_page(to);
        }
    }

    /// F5: one page at a time on a black screen, or back to reading as before.
    fn toggle_presentation(&mut self) {
        if let Some(p) = self.presenting.take() {
            if let Some(doc) = &mut self.doc {
                doc.zoom = p.zoom;
            }
            if !p.fullscreen {
                self.window().set_fullscreen(None);
            }
            self.relayout((0.0, 0.0));
            self.go_to_page(p.page);
            return;
        }
        let Some(doc) = &self.doc else { return };
        let page = row_step(&doc.layout, self.current_page().unwrap_or(0), 0);
        let fullscreen = self.window().fullscreen().is_some();
        self.presenting = Some(Presenting { page, zoom: doc.zoom, fullscreen });
        if let Some(doc) = &mut self.doc {
            doc.zoom = Zoom::FitPage;
        }
        self.reset_view_state();
        self.panel_open = false;
        self.window().set_fullscreen(Some(Fullscreen::Borderless(None)));
        self.redraw_soon();
    }

    /// Presenting: shows the next page or pair (`dir` 1) or the previous one (-1).
    fn present_step(&mut self, dir: i64) {
        let Some(doc) = &self.doc else { return };
        if let Some(p) = &mut self.presenting {
            p.page = row_step(&doc.layout, p.page, dir);
        }
        self.redraw_soon();
    }

    /// Presenting: puts the shown page or pair in the middle of the screen. Called while drawing,
    /// so it moves the view without asking for another frame.
    fn center_presented(&mut self) {
        let size = self.view_size();
        let Some(p) = self.presenting else { return };
        let Some(doc) = &mut self.doc else { return };
        let Some(pos) = doc.layout.pages.get(p.page) else { return };
        doc.x = ((doc.layout.width - i64::from(size.width)) / 2).max(0) as f64;
        doc.y = (pos.y - (i64::from(size.height) - i64::from(pos.row_height)) / 2) as f64;
    }

    fn show_menu(&mut self, pos: PhysicalPosition<f64>, commands: Vec<Command>) {
        self.menu = Some((pos, commands));
        self.link_tip = None;
        self.hover = None;
        self.set_cursor_icon(CursorIcon::Default);
        self.redraw_soon();
    }

    /// Does what a menu entry says. Zooming keeps the point that was right-clicked where it was.
    fn run(&mut self, command: Command, at: PhysicalPosition<f64>) {
        let (left, top) = self.view_origin();
        let anchor = (at.x - left, at.y - top);
        match command {
            Command::Copy => self.copy_selection(),
            Command::CopyLink(uri) => {
                win::copy_text(self.window(), &uri);
            }
            Command::ZoomIn => self.zoom_step(1, anchor),
            Command::ZoomOut => self.zoom_step(-1, anchor),
            Command::FitPage => self.set_zoom(Zoom::FitPage, self.center()),
            Command::FitWidth => self.set_zoom(Zoom::FitWidth, anchor),
            Command::ActualSize => self.set_zoom(Zoom::Percent(100.0), anchor),
            Command::Highlight => self.mark_selection(Markup::Highlight),
            Command::Underline => self.mark_selection(Markup::Underline),
            Command::Strikeout => self.mark_selection(Markup::Strikeout),
            Command::EditMark(i) => {
                let text = self.doc.as_ref().and_then(|d| d.marks.get(i)).and_then(|m| match &m.shape {
                    Shape::Note(_, t) | Shape::Text(_, t, _) => Some(t.clone()),
                    _ => None,
                });
                if let Some(text) = text {
                    self.open_editor(EditFor::Mark(i), text);
                }
            }
            Command::DeleteMark(i) => self.erase(Found::Mark(i)),
            Command::DeleteSaved(page, index, rect) => self.remove_saved(page, index, rect),
            Command::UndoMark => self.undo_mark(),
            Command::Annotate => self.toggle_annotate(),
            Command::Home => self.go_home(),
            Command::SetChoice(i, k) => self.change(i, Value::Choice(k)),
            Command::Save => {
                self.save();
            }
            Command::RenameBookmark(page) => {
                let name = self.doc.as_ref().and_then(|d| self.favorites.of(&d.path).into_iter().find(|f| f.page == page).map(|f| f.name.clone()));
                if let Some(name) = name {
                    self.open_editor(EditFor::Bookmark(page), name);
                }
            }
            Command::RemoveHighlight(page, x, y) => self.remove_highlight(page, x, y),
            Command::FindSelection => self.find_selection(),
            Command::SearchWeb => self.search_web(),
            Command::Print => self.print(),
            Command::Open => self.open_dialog(),
            Command::CloseShownTab => {
                if !self.tabs.is_empty() {
                    self.close_tab(self.active);
                }
            }
            Command::FirstPage => self.jump_to_page(0),
            Command::LastPage => {
                let last = self.doc.as_ref().map_or(0, |d| d.sizes.len().saturating_sub(1));
                self.jump_to_page(last);
            }
            Command::GoToPage => {
                if self.doc.is_some() {
                    self.editing = Some(String::new());
                }
            }
            Command::Back => self.go_history(true),
            Command::Forward => self.go_history(false),
            Command::Find => self.focus_search(),
            Command::ToggleToolbar => {
                let i = FIELDS.iter().position(|f| f.key == "show_toolbar").unwrap();
                self.change(i, Value::Toggle(!self.settings.show_toolbar));
            }
            Command::ToggleSidebar => self.toggle_outline(),
            Command::OpenSettings => {
                if !self.panel_open {
                    self.toggle_panel();
                }
            }
            Command::About => (self.props_title, self.props) = ("About Flores PDF", Some(about())),
            Command::SaveCopy => self.save_copy(),
            Command::FillForm => self.toggle_filling(),
            Command::PickOption(field, option) => self.pick_option(field, option),
            Command::AddFavorite(page) | Command::RemoveFavorite(page) => self.toggle_favorite(page),
            Command::CopyPageImage(page) => self.copy_page_image(page),
            Command::SavePageImage(page) => self.save_page_image(page),
            Command::ShowInFolder(path) => win::show_in_folder(&path),
            Command::CopyPath(path) => {
                win::copy_text(self.window(), &path.display().to_string());
            }
            Command::Properties => (self.props_title, self.props) = ("Document properties", self.properties()),
            Command::CloseTab(i) => self.close_tab(i),
            Command::CloseOtherTabs(i) => self.close_tabs_except(i, |_| true),
            Command::CloseTabsRight(i) => self.close_tabs_except(i, |j| j > i),
            Command::ReopenClosed => self.reopen_closed(),
            Command::SetSpread(spread) => self.set_spread(spread),
            Command::RotateLeft => self.rotate(-1),
            Command::RotateRight => self.rotate(1),
            Command::Presentation => self.toggle_presentation(),
            Command::FullScreen => self.toggle_fullscreen(),
            Command::Separator => {}
        }
        self.redraw_soon();
    }

    fn mouse_moved(&mut self, el: &ActiveEventLoop, position: PhysicalPosition<f64>) {
        self.cursor = position;
        match self.thumb_drag {
            Some((true, grab)) => return self.drag_hthumb(position.x, grab),
            Some((false, grab)) => return self.drag_thumb(position.y, grab),
            None => {}
        }
        if let Some((start, width)) = self.side_drag {
            let w = width + ((position.x - start) / self.dpi()).round() as i64;
            self.settings.sidebar_width = w.clamp(160, 600);
            self.redraw_soon();
            return;
        }
        if let Some(i) = self.tab_drag {
            if let Some(Hit::Tab(j) | Hit::TabClose(j)) = self.chrome_hit(position)
                && j != i
            {
                self.move_tab(i, j);
            }
            return;
        }
        if let Some(page) = self.drawing.as_ref().map(|d| d.1) {
            let pen = self.drawing.as_ref().is_some_and(|d| d.0 == Tool::Pen);
            if let Some(p) = self.point_on(position, page)
                && let Some((_, _, points)) = &mut self.drawing
            {
                if !pen {
                    points.truncate(1);
                }
                // the pen keeps points half a point or more apart
                if points.last().is_none_or(|q| (q[0] - p[0]).hypot(q[1] - p[1]) >= 0.5) {
                    points.push(p);
                }
            }
            self.redraw_soon();
            return;
        }
        if self.selecting {
            self.extend_selection(position);
            self.redraw_soon();
            return;
        }
        if let Some((start, (x, y))) = self.drag {
            self.scroll_to(x - (position.x - start.x), y - (position.y - start.y));
            return;
        }
        // while the menu is open, only the menu reacts
        let menu_open = self.menu.is_some();
        let hit = self.chrome_hit(position).filter(|h| !menu_open || matches!(h, Hit::MenuItem(_) | Hit::Menu));
        let point = kurbo::Point::new(position.x, position.y);
        let on_scrollbar = hit.is_none() && !menu_open && self.scrollbar().is_some_and(|(t, _)| t.contains(point));
        let on_hbar = hit.is_none() && !menu_open && !on_scrollbar && self.hscrollbar().is_some_and(|(t, _)| t.contains(point));
        if (on_scrollbar, on_hbar) != (self.on_scrollbar, self.on_hbar) {
            (self.on_scrollbar, self.on_hbar) = (on_scrollbar, on_hbar);
            self.redraw_soon();
        }
        let on_edge = !menu_open && self.on_sidebar_edge(position);
        let on_pages = hit.is_none() && !on_scrollbar && !on_hbar && !menu_open && !on_edge && !self.panel_open;
        let link = if on_pages { self.link_at(position) } else { None };
        let link_tip = match &link {
            Some((l, r)) => Some((*r, describe(&l.target))),
            None if on_pages => self.annotation_tip(position),
            None => None,
        };
        let tip_changed = link_tip.as_ref().map(|t| &t.1) != self.link_tip.as_ref().map(|t| &t.1);
        if hit != self.hover || tip_changed {
            self.hover = hit;
            self.link_tip = link_tip;
            self.hover_since = Instant::now();
            if hit.is_some_and(|h| self.tooltip_text(h).is_some()) || self.link_tip.is_some() {
                el.set_control_flow(ControlFlow::WaitUntil(self.hover_since + TOOLTIP_DELAY));
            }
            self.redraw_soon();
        }
        let icon = match hit {
            _ if on_edge => CursorIcon::EwResize,
            Some(Hit::PageBox | Hit::SearchBox) => CursorIcon::Text,
            Some(Hit::Panel | Hit::Sidebar | Hit::Menu | Hit::Props | Hit::Prompt | Hit::Editor) => CursorIcon::Default,
            Some(_) => CursorIcon::Pointer,
            None if !on_pages => CursorIcon::Default,
            None if self.annotating && self.tool.is_some_and(|t| !t.marks_text() && t != Tool::Eraser) => CursorIcon::Crosshair,
            None if self.annotating && self.tool == Some(Tool::Eraser) => CursorIcon::Pointer,
            None if link.is_some() => CursorIcon::Pointer,
            None if self.over_text(position) => CursorIcon::Text,
            None => CursorIcon::Default,
        };
        self.set_cursor_icon(icon);
    }

    /// The part of the tabs, toolbar, panels, menu or start page under a window position.
    fn chrome_hit(&self, pos: PhysicalPosition<f64>) -> Option<Hit> {
        let shown = Shown {
            menu: self.menu.is_some(),
            props: self.props.is_some(),
            prompt: self.prompt.is_some(),
            panel: self.panel_open,
            sidebar: self.sidebar_width() > 0,
            tabs: self.tabs_height() > 0,
            bar: self.bar_height() > 0,
            annot_bar: self.annot_bar_height() > 0,
            editor: self.editor.is_some(),
            start: self.doc.is_none(),
        };
        let chrome = self.chrome.as_ref()?;
        // the settings window keeps the mouse to itself, apart from a menu opened from it
        let outside = chrome.panel_rect().filter(|_| self.panel_open).is_some_and(|r| !r.contains(kurbo::Point::new(pos.x, pos.y)));
        if outside && self.menu.is_none() && self.props.is_none() {
            return None;
        }
        chrome.hit(pos.x, pos.y, shown)
    }

    fn mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let pos = self.cursor;
        let pressed = state == ElementState::Pressed;
        if pressed && (self.error.take().is_some() | self.status.take().is_some()) {
            self.redraw_soon();
        }
        if button == MouseButton::Right && pressed {
            (self.right_down, self.right_zoomed) = (true, false);
        }
        // with the menu open, a press anywhere closes it; one on an entry does that entry first
        if pressed && self.menu.is_some() {
            let hit = self.chrome_hit(pos);
            if let Some((at, commands)) = self.menu.take()
                && button == MouseButton::Left
                && let Some(Hit::MenuItem(i)) = hit
                && let Some(command) = commands.into_iter().nth(i)
            {
                self.run(command, at);
            }
            self.hover = None;
            self.redraw_soon();
            return;
        }
        // so does the text box
        if self.editor.is_some() {
            match self.chrome_hit(pos).filter(|_| pressed && button == MouseButton::Left) {
                Some(Hit::EditorOk) => self.finish_editor(),
                Some(Hit::EditorCancel) => self.editor = None,
                Some(Hit::EditorDelete) => {
                    if let Some(EditorState { purpose: EditFor::Mark(i), .. }) = self.editor.take() {
                        self.erase(Found::Mark(i));
                    }
                }
                _ => {}
            }
            self.redraw_soon();
            return;
        }
        // the password box keeps the window's attention until it's answered
        if self.prompt.is_some() {
            match self.chrome_hit(pos).filter(|_| pressed && button == MouseButton::Left) {
                Some(Hit::PromptOk) => self.submit_password(),
                Some(Hit::PromptCancel) => self.prompt = None,
                _ => {}
            }
            self.redraw_soon();
            return;
        }
        // the settings window closes on a press outside it
        if pressed
            && self.panel_open
            && !self.chrome.as_ref().and_then(|c| c.panel_rect()).is_some_and(|r| r.contains(kurbo::Point::new(pos.x, pos.y)))
        {
            self.toggle_panel();
            return;
        }
        // the properties box closes on a press outside it
        if pressed && self.props.is_some() && !matches!(self.chrome_hit(pos), Some(Hit::Props | Hit::PropsClose)) {
            self.props = None;
            self.redraw_soon();
            return;
        }
        if self.presenting.is_some() {
            // a click goes on, a right-click back, as in a slide show
            match (pressed, button) {
                (true, MouseButton::Left | MouseButton::Forward) => self.present_step(1),
                (true, MouseButton::Right | MouseButton::Back) => self.present_step(-1),
                _ => {}
            }
            return;
        }
        if pressed && matches!(button, MouseButton::Back | MouseButton::Forward) {
            self.go_history(button == MouseButton::Back);
            return;
        }
        if button == MouseButton::Right {
            // the menu opens when the button comes up, as Windows' menus do, unless the wheel
            // zoomed while it was held
            if !pressed {
                self.right_down = false;
                let (left, top) = self.view_origin();
                let zoomed = std::mem::take(&mut self.right_zoomed);
                match self.chrome_hit(pos) {
                    _ if zoomed => {}
                    Some(Hit::Tab(i) | Hit::TabClose(i)) => self.open_tab_menu(i, pos),
                    Some(Hit::Favorite(i) | Hit::Unfavorite(i)) => self.open_bookmark_menu(i, pos),
                    None if self.doc.is_some() && pos.x >= left && pos.y >= top => self.open_menu(pos),
                    _ => {}
                }
            }
            return;
        }
        if state == ElementState::Released {
            self.thumb_drag = None;
            // a drag with a tool ends: what it drew is kept, if it's more than a click
            if self.drawing.is_some() {
                let mark = self.drawing_mark().filter(annots::worth_keeping);
                self.drawing = None;
                if let (Some(m), Some(doc)) = (mark, &mut self.doc) {
                    doc.marks.push(m);
                }
            }
            if self.side_drag.take().is_some() {
                let _ = self.settings.save();
            }
            if self.tab_drag.take().is_some() {
                self.save_history();
            }
            // a link is followed if the button comes up on the one it went down on
            if let Some(target) = self.pressed_link.take()
                && self.link_at(self.cursor).is_some_and(|(l, _)| l.target == target)
            {
                self.follow(&target);
            }
            self.pressed = None;
            if self.drag.take().is_some() {
                self.set_cursor_icon(CursorIcon::Default);
            }
            if std::mem::take(&mut self.selecting) {
                let markup = self.tool.filter(|_| self.annotating).and_then(|t| match t {
                    Tool::Highlight => Some(Markup::Highlight),
                    Tool::Underline => Some(Markup::Underline),
                    Tool::Strikeout => Some(Markup::Strikeout),
                    _ => None,
                });
                if let Some(doc) = &mut self.doc
                    && doc.selection.is_some_and(|s| s.anchor == s.focus)
                {
                    // a click, not a drag: nothing selected
                    doc.selection = None;
                } else if let Some(kind) = markup {
                    // with a tool for text, what's selected is marked at once
                    self.mark_selection(kind);
                }
            }
            self.redraw_soon();
            return;
        }
        let (left, top) = self.view_origin();
        let on_pages = pos.y >= top && pos.x >= left;
        if button == MouseButton::Middle {
            // a middle click closes a tab, as in browsers
            if let Some(Hit::Tab(i) | Hit::TabClose(i)) = self.chrome_hit(pos) {
                self.close_tab(i);
                return;
            }
            if let Some(doc) = self.doc.as_ref().filter(|_| on_pages) {
                self.drag = Some((pos, (doc.x, doc.y)));
                self.set_cursor_icon(CursorIcon::Grabbing);
            }
            return;
        }
        // where the click lands, not the last hover, which a layout change may have made stale
        if button == MouseButton::Left && self.on_sidebar_edge(pos) {
            self.side_drag = Some((pos.x, self.settings.sidebar_width));
            return;
        }
        if let Some(hit) = self.chrome_hit(pos) {
            self.pressed = Some(hit);
            self.click(hit);
            if let Hit::Tab(i) = hit {
                self.tab_drag = Some(i);
            }
            return;
        }
        self.editing = None;
        self.search_focus = false;
        // the scroll bar: drag the thumb, or press elsewhere on the strip to bring it there
        let point = kurbo::Point::new(pos.x, pos.y);
        if let Some((_, thumb)) = self.scrollbar().filter(|(track, _)| track.contains(point)) {
            let grab = if thumb.contains(point) { pos.y - thumb.y0 } else { thumb.height() / 2.0 };
            self.thumb_drag = Some((false, grab));
            self.drag_thumb(pos.y, grab);
            return;
        }
        if let Some((_, thumb)) = self.hscrollbar().filter(|(track, _)| track.contains(point)) {
            let grab = if thumb.contains(point) { pos.x - thumb.x0 } else { thumb.width() / 2.0 };
            self.thumb_drag = Some((true, grab));
            self.drag_hthumb(pos.x, grab);
            return;
        }
        if let Some(tool) = self.tool.filter(|t| self.annotating && !t.marks_text() && self.annot_bar_height() > 0) {
            self.tool_press(tool, pos);
            self.redraw_soon();
            return;
        }
        if self.field_click(pos) {
            self.redraw_soon();
            return;
        }
        if let Some((link, _)) = self.link_at(pos) {
            self.pressed_link = Some(link.target);
            return;
        }
        let double = self.last_click.is_some_and(|(t, p)| t.elapsed() < DOUBLE_CLICK && (p.x - pos.x).abs() < 4.0 && (p.y - pos.y).abs() < 4.0);
        self.last_click = Some((Instant::now(), pos));

        // on text: select, a word at a time on a double click
        let point = self.page_point(pos);
        if let Some((page, x, y)) = point
            && let Some(doc) = &mut self.doc
            && let Some(t) = doc.text.text(page)
            && let Some(i) = text::char_at(&t, x, y)
        {
            if double {
                let word = text::word_around(&t, i);
                doc.selection = Some(Selection { anchor: (page, word.start), focus: (page, word.end) });
            } else {
                let at = (page, text::boundary_at(&t, x, y));
                doc.selection = Some(Selection { anchor: at, focus: at });
                self.selecting = true;
            }
            self.redraw_soon();
            return;
        }
        // elsewhere: drop the selection and move the pages
        if let Some(doc) = self.doc.as_mut().filter(|_| on_pages) {
            doc.selection = None;
            self.drag = Some((pos, (doc.x, doc.y)));
            self.set_cursor_icon(CursorIcon::Grabbing);
        }
        self.redraw_soon();
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        // positive deltas move the content right and down
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                let step = self.settings.scroll_step as f64 * self.dpi();
                (f64::from(x) * step, f64::from(y) * step)
            }
            MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
        };
        if self.menu.take().is_some() {
            self.redraw_soon();
            return;
        }
        if self.presenting.is_some() {
            if dy != 0.0 {
                self.present_step(if dy < 0.0 { 1 } else { -1 });
            }
            return;
        }
        let (left, top) = self.view_origin();
        let panel = self.chrome.as_ref().and_then(|c| c.panel_rect()).filter(|_| self.panel_open);
        let over_panel = panel.is_some_and(|r| r.contains(kurbo::Point::new(self.cursor.x, self.cursor.y)));
        // the settings window keeps the wheel to itself
        if panel.is_some() && !over_panel {
            return;
        }
        let over_sidebar = self.cursor.x < left && self.cursor.y >= top;
        let point = kurbo::Point::new(self.cursor.x, self.cursor.y);
        let over_scrollbar = self.scrollbar().is_some_and(|(track, _)| track.contains(point));
        let over_hbar = self.hscrollbar().is_some_and(|(track, _)| track.contains(point));
        if over_panel {
            let max = self.chrome.as_ref().map_or(0.0, |c| c.panel_scroll_max());
            self.panel_scroll = (self.panel_scroll - dy).clamp(0.0, max);
            self.redraw_soon();
        } else if over_sidebar {
            let max = self.chrome.as_ref().map_or(0.0, |c| c.sidebar_scroll_max(self.view_size().height));
            if let Some(doc) = &mut self.doc {
                doc.sidebar_scroll = (doc.sidebar_scroll - dy).clamp(0.0, max);
            }
            self.redraw_soon();
        } else if self.modifiers.control_key() || self.right_down {
            // Ctrl+wheel, or the wheel with the right button held, zooms around the mouse
            if dy != 0.0 {
                let anchor = (self.cursor.x - left, self.cursor.y - top);
                self.zoom_step(if dy > 0.0 { 1 } else { -1 }, anchor);
                self.right_zoomed |= self.right_down;
            }
        } else if over_hbar {
            self.scroll_by(-dy, 0.0);
        } else if over_scrollbar {
            // the fast lane: over the scroll bar, a wheel notch moves a whole page
            let notch = self.settings.scroll_step as f64 * self.dpi();
            let page = self.current_page().unwrap_or(0);
            let step = self.doc.as_ref().and_then(|d| Some(f64::from(d.layout.pages.get(page)?.height + d.layout.gap)));
            if let Some(step) = step {
                self.scroll_by(0.0, -dy / notch * step);
            }
        } else {
            let (dx, dy) = if self.modifiers.shift_key() { (dy, 0.0) } else { (dx, dy) };
            match delta {
                // a touchpad already moves smoothly: follow it exactly
                MouseScrollDelta::PixelDelta(_) => self.scroll_by_now(-dx, -dy),
                MouseScrollDelta::LineDelta(..) => self.scroll_by(-dx, -dy),
            }
        }
    }

    fn scroll_by_now(&mut self, dx: f64, dy: f64) {
        if let Some(doc) = &self.doc {
            self.scroll_to(doc.x + dx, doc.y + dy);
        }
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
        if self.window.is_some() {
            return;
        }
        let theme = if self.dark() { winit::window::Theme::Dark } else { winit::window::Theme::Light };
        // the icon the build embedded, as icon group 1
        let icon = winit::window::Icon::from_resource(1, None).ok();
        let mut attrs = Window::default_attributes()
            .with_class_name(win::CLASS)
            .with_window_icon(icon.clone())
            .with_taskbar_icon(icon)
            .with_title("Flores PDF")
            .with_inner_size(LogicalSize::new(1000.0, 750.0))
            .with_min_inner_size(LogicalSize::new(480.0, 320.0))
            .with_theme(Some(theme));
        if self.settings.remember_window
            && let Some(p) = self.settings.window
        {
            // only if that spot is still on a screen
            let on_screen = el.available_monitors().any(|m| {
                let (pos, size) = (m.position(), m.size());
                (pos.x..pos.x + size.width as i32 - 64).contains(&p.x) && (pos.y..pos.y + size.height as i32 - 64).contains(&p.y)
            });
            if on_screen && p.width > 0 && p.height > 0 {
                attrs = attrs.with_position(PhysicalPosition::new(p.x, p.y)).with_inner_size(PhysicalSize::new(p.width, p.height));
            }
            attrs = attrs.with_maximized(p.maximized);
        }
        let window = Rc::new(el.create_window(attrs).expect("creating the window"));
        win::caption_color(&window, self.caption());
        let context = softbuffer::Context::new(window.clone()).expect("connecting to the display");
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).expect("creating the window buffer"));
        // files other copies of the reader hand over arrive as events; from here on they can
        let proxy = self.proxy.clone();
        win::listen_for_handover(&window, move |args| {
            let _ = proxy.send_event(Wake::HandOver(args));
        });
        self.start_lock = None;
        self.window = Some(window);
        self.restore_session();
        let args = self.start.take().unwrap_or_default();
        for path in &args.paths {
            self.open(path, args.page, args.zoom);
        }
        if self.doc.is_none() && !self.tabs.is_empty() {
            self.activate(self.active);
        }
        self.redraw_soon();
    }

    fn new_events(&mut self, el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            // time to show a tooltip
            el.set_control_flow(ControlFlow::Wait);
            self.redraw_soon();
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, wake: Wake) {
        match wake {
            Wake::Redraw => {}
            Wake::HandOver(args) => {
                let args = parse_args(args);
                for path in &args.paths {
                    self.open(path, args.page, args.zoom);
                }
                self.window().focus_window();
            }
            Wake::FileChanged(path) => self.file_changed(&path),
            Wake::Update(release) => self.update = Some(release),
            Wake::UpdateReady(setup) => {
                self.downloading = false;
                match setup.and_then(|path| update::install(&path)) {
                    // the setup waits for this copy to close
                    Ok(()) => self.close(el),
                    Err(e) => (self.status, self.error) = (None, Some(format!("Couldn't update: {e}"))),
                }
            }
            Wake::Printing(done, total) => self.status = Some(format!("Printing page {} of {total}… Esc stops it", done + 1)),
            Wake::Printed(result) => {
                self.printing = None;
                match result {
                    Ok(1) => self.status = Some("Sent 1 page to the printer".to_owned()),
                    Ok(n) => self.status = Some(format!("Sent {n} pages to the printer")),
                    Err(e) => (self.status, self.error) = (None, Some(e)),
                }
            }
        }
        self.redraw_soon();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close(el),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                // the menu and properties box were placed for the old size
                self.menu = None;
                self.props = None;
                self.redraw_soon();
            }
            WindowEvent::ThemeChanged(theme) => {
                self.system_dark = theme == winit::window::Theme::Dark;
                self.redraw_soon();
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Focused(true) if self.panel_open => {
                self.is_default = win::is_default_pdf_reader();
                self.redraw_soon();
            }
            WindowEvent::DroppedFile(path) => self.open(&path, None, None),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } if event.state == ElementState::Pressed => {
                self.key(el, event);
            }
            WindowEvent::CursorMoved { position, .. } => self.mouse_moved(el, position),
            WindowEvent::CursorLeft { .. } => {
                if self.hover.take().is_some() | std::mem::take(&mut self.on_scrollbar) | std::mem::take(&mut self.on_hbar) {
                    self.redraw_soon();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: button @ (MouseButton::Left | MouseButton::Middle | MouseButton::Right | MouseButton::Back | MouseButton::Forward),
                ..
            } => self.mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            _ => {}
        }
    }
}

/// Pixels per point at 100% zoom: 96 DPI times the screen's scale factor.
fn actual_size(dpi: f64) -> f32 {
    (dpi * 96.0 / 72.0) as f32
}

/// Pixels per point for `zoom` in a view of `view` pixels.
fn scale_for(zoom: Zoom, sizes: &[PageSize], view: PhysicalSize<u32>, gap: u32, dpi: f64, rotation: u8, spread: Spread) -> f32 {
    let actual = actual_size(dpi);
    // a spread is two pages wide, with a gap between them
    let across = if spread == Spread::One { 1 } else { 2 };
    let fit = |avail: u32, points: f32, n: u32| avail.saturating_sub((n + 1) * gap) as f32 / (points * n as f32);
    let turned = |s: &PageSize| if rotation % 2 == 1 { (s.height, s.width) } else { (s.width, s.height) };
    let widest = sizes.iter().fold(1.0f32, |m, s| m.max(turned(s).0));
    let tallest = sizes.iter().fold(1.0f32, |m, s| m.max(turned(s).1));
    let scale = match zoom {
        Zoom::Percent(p) => p / 100.0 * actual,
        Zoom::FitWidth => fit(view.width, widest, across),
        Zoom::FitPage => fit(view.width, widest, across).min(fit(view.height, tallest, 1)),
    };
    scale.clamp(actual * ZOOM_STEPS[0] / 100.0, actual * ZOOM_STEPS[ZOOM_STEPS.len() - 1] / 100.0)
}

fn viewport(doc: &Doc, size: PhysicalSize<u32>) -> Viewport {
    Viewport { x: doc.x.round() as i64, y: doc.y.round() as i64, width: size.width, height: size.height }
}

fn clamp_scroll(doc: &mut Doc, size: PhysicalSize<u32>) {
    doc.x = doc.x.clamp(0.0, (doc.layout.width - i64::from(size.width)).max(0) as f64);
    doc.y = doc.y.clamp(0.0, (doc.layout.height - i64::from(size.height)).max(0) as f64);
}

/// What a link does, for its tooltip.
fn describe(target: &Target) -> String {
    match target {
        Target::Page { page, .. } => format!("Go to page {} (Alt+← comes back)", page + 1),
        Target::Uri(uri) if uri.chars().count() > 80 => format!("{}…", uri.chars().take(80).collect::<String>()),
        Target::Uri(uri) => uri.clone(),
    }
}

/// Search matches and the selection on the pages in view.
fn draw_highlights(frame: &mut Frame, doc: &Doc, view: &Viewport, highlights: &highlights::Highlights) {
    let pages = doc.layout.pages_between(view.y, view.y + i64::from(view.height));
    let mut tint = |page: usize, r: [f32; 4], (color, alpha): (u32, u32)| {
        let r = rect_to_doc(doc, page, r);
        let (x0, y0) = (r.x0.floor() as i64 - view.x, r.y0.floor() as i64 - view.y);
        let (x1, y1) = (r.x1.ceil() as i64 - view.x, r.y1.ceil() as i64 - view.y);
        render::tint_rect(frame, x0, y0, (x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32, color, alpha);
    };
    // the reader's highlights first, under search matches and the selection: in a PDF, those not
    // saved into it yet (saved ones are part of the page); elsewhere, highlights.txt's
    let pdf = doc.engine.page_transform(0).is_some();
    for page in pages.clone() {
        if !pdf {
            for h in highlights.on(&doc.path, page) {
                for r in &h.rects {
                    tint(page, *r, (h.color, HIGHLIGHT_ALPHA));
                }
            }
        }
        for m in doc.marks.iter().filter(|m| m.page == page) {
            if let Shape::Markup(Markup::Highlight, rects) = &m.shape {
                for r in rects {
                    tint(page, *r, (m.color, HIGHLIGHT_ALPHA));
                }
            }
        }
    }
    doc.text.with_matches(|matches| {
        let first = matches.partition_point(|m| m.page < pages.start);
        for m in matches[first..].iter().take_while(|m| m.page < pages.end) {
            let shade = if doc.current == Some((m.page, m.chars.start)) { CURRENT_TINT } else { MATCH_TINT };
            for r in &m.rects {
                tint(m.page, *r, shade);
            }
        }
    });
    if let Some(sel) = doc.selection {
        for page in pages {
            if let Some(t) = doc.text.text(page)
                && let Some(range) = sel.on(page, t.chars.len())
            {
                for r in text::line_rects(&t, range) {
                    tint(page, r, SELECTION_TINT);
                }
            }
        }
    }
}

/// The scroll bar at the right edge of the pages: a thin thumb, or a wide one on a shaded strip
/// while the mouse is over it. `bar` is the strip and the thumb in window coordinates; `origin` is
/// where the frame sits in the window.
fn draw_scrollbar(frame: &mut Frame, bar: (kurbo::Rect, kurbo::Rect), origin: (f64, f64), dpi: f64, hot: bool, palette: &Palette) {
    let (track, thumb) = bar;
    let (y, h) = ((thumb.y0 - origin.1).round() as i64, thumb.height().round() as u32);
    if hot {
        let x = (track.x0 - origin.0).round() as i64;
        render::tint_rect(frame, x, 0, track.width().round() as u32, frame.height, palette.thumb, 60);
        let w = (10.0 * dpi).round();
        let x = (track.x0 - origin.0 + (track.width() - w) / 2.0).round() as i64;
        render::fill_rect(frame, x, y, w as u32, h, palette.dim);
    } else {
        let w = (6.0 * dpi).round();
        let x = (track.x1 - origin.0 - w - (3.0 * dpi).round()) as i64;
        render::fill_rect(frame, x, y, w as u32, h, palette.thumb);
    }
}

/// The scroll bar along the bottom of the pages, drawn like the one at the side.
fn draw_hscrollbar(frame: &mut Frame, bar: (kurbo::Rect, kurbo::Rect), origin: (f64, f64), dpi: f64, hot: bool, palette: &Palette) {
    let (track, thumb) = bar;
    let (x, w) = ((thumb.x0 - origin.0).round() as i64, thumb.width().round() as u32);
    if hot {
        let y = (track.y0 - origin.1).round() as i64;
        render::tint_rect(frame, 0, y, frame.width, track.height().round() as u32, palette.thumb, 60);
        let h = (10.0 * dpi).round();
        let y = (track.y0 - origin.1 + (track.height() - h) / 2.0).round() as i64;
        render::fill_rect(frame, x, y, w, h as u32, palette.dim);
    } else {
        let h = (6.0 * dpi).round();
        let y = (track.y1 - origin.1 - h - (3.0 * dpi).round()) as i64;
        render::fill_rect(frame, x, y, w, h as u32, palette.thumb);
    }
}

/// The first page of the row `dir` rows after `page`'s (before it, for a negative `dir`), staying
/// at the ends. With one page per row, that's simply the next or previous page.
fn row_step(layout: &Layout, page: usize, dir: i64) -> usize {
    let pages = &layout.pages;
    let Some(here) = pages.get(page).map(|p| p.y) else { return page };
    let row_start = |y: i64| pages.iter().position(|q| q.y == y).unwrap_or(page);
    match dir.signum() {
        1 => pages.iter().position(|q| q.y > here).unwrap_or_else(|| row_start(here)),
        -1 => pages.iter().rposition(|q| q.y < here).map_or_else(|| row_start(here), |i| row_start(pages[i].y)),
        _ => row_start(here),
    }
}

/// Where the top of `doc`'s view is, `down` pixels into the view.
fn doc_spot(doc: &Doc, down: f64) -> Option<Spot> {
    let y = doc.y + down;
    let page = doc.layout.page_at(y as i64);
    let p = doc.layout.pages.get(page)?;
    Some(Spot { page, y: ((y - p.y as f64) / f64::from(doc.layout.scale)) as f32 })
}

/// Notes where the reader is in `doc`, so it reopens there.
fn remember(history: &mut History, doc: &Doc) {
    let spot = doc_spot(doc, 0.0).unwrap_or(Spot { page: 0, y: 0.0 });
    let (zoom, view) = (zoom_name(doc.zoom), view_name(doc.rotation, doc.spread));
    history.remember(Entry { path: doc.path.clone(), page: spot.page, y: spot.y, zoom, view, tab: 0, active: false });
}

/// The About box: the version, and what's inside with whose license.
fn about() -> Vec<(&'static str, String)> {
    vec![
        ("Version", env!("CARGO_PKG_VERSION").to_owned()),
        ("Made by", "aga2522".to_owned()),
        ("Discord", "discord.gg/3xuJY8UXKu (a general server, all welcome)".to_owned()),
        ("PDF", "hayro, by Laurenz Stampfl (Apache-2.0 or MIT), whose renderer is vendored and changed here".to_owned()),
        ("Forms, saving", "PDFium build 7881 by Google and others (BSD-3-Clause), built in; written with its license texts to Local AppData, FloresPDF folder, when first used".to_owned()),
        ("DjVu", "djvu-rs (MIT)".to_owned()),
        ("EPUB", "our own reader and layout; Georgia, Consolas and Segoe UI from Windows".to_owned()),
        ("Comics", "UnRAR by Alexander Roshal (UnRAR license: reading only), sevenz-rust2 (Apache-2.0), our own zip reader".to_owned()),
        ("Pictures", "zune-jpeg, png, gif, image-webp (MIT or Apache-2.0)".to_owned()),
        ("Window", "winit, softbuffer, vello_cpu, skrifa (MIT or Apache-2.0); Segoe UI from Windows".to_owned()),
    ]
}

/// The form's values, to tell later whether they've changed.
fn field_values(doc: &Doc) -> Vec<(String, bool)> {
    doc.fields.iter().flatten().map(|e| (e.field.value.clone(), e.field.checked)).collect()
}

/// Whether the document has changes not saved into its file: form entries, annotations made, or
/// annotations taken out. Highlights from highlights.txt don't count: they're kept there meanwhile.
fn has_unsaved(doc: &Doc) -> bool {
    doc.memory.is_some()
        || doc.marks.iter().any(|m| !m.legacy)
        || doc.fields.as_ref().is_some_and(|f| f.iter().map(|e| (e.field.value.clone(), e.field.checked)).ne(doc.saved.iter().cloned()))
}

/// Replaces the file at `path` with `data`: written beside it first, then put in its place, so a
/// failure leaves the file as it was. The reader may still have the old file mapped for a moment
/// (a render thread finishing a tile), so the swap is tried for a few seconds.
fn write_in_place(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map_or_else(|| "document".into(), |n| n.to_string_lossy().into_owned());
    let part = path.with_file_name(format!("{name}.flores-saving"));
    std::fs::write(&part, data)?;
    for attempt in 0.. {
        match std::fs::rename(&part, path) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < 30 => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        }
    }
    unreachable!()
}

/// A rectangle in a page's own coordinates (left, bottom, right, top; y up) in points from the
/// top-left of the page as shown, through the page's transform.
fn pdf_to_page(t: [f64; 6], r: [f32; 4]) -> [f32; 4] {
    let map = |x: f32, y: f32| {
        let (x, y) = (f64::from(x), f64::from(y));
        ((t[0] * x + t[2] * y + t[4]) as f32, (t[1] * x + t[3] * y + t[5]) as f32)
    };
    let (a, b) = (map(r[0], r[1]), map(r[2], r[3]));
    [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
}

/// The reverse of `pdf_to_page`: a rectangle of the page as shown back in the page's own
/// coordinates (left, bottom, right, top).
fn page_to_pdf(t: [f64; 6], r: [f32; 4]) -> [f32; 4] {
    let det = t[0] * t[3] - t[1] * t[2];
    if det == 0.0 {
        return r;
    }
    let map = |x: f32, y: f32| {
        let (x, y) = (f64::from(x) - t[4], f64::from(y) - t[5]);
        (((t[3] * x - t[2] * y) / det) as f32, ((t[0] * y - t[1] * x) / det) as f32)
    };
    let (a, b) = (map(r[0], r[1]), map(r[2], r[3]));
    [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
}

/// Where point (x, y) of `page`, in points from the top-left of the page as it is, lands in
/// document pixels once the page is turned and placed.
fn to_doc(doc: &Doc, page: usize, x: f32, y: f32) -> (f64, f64) {
    let (p, s) = (doc.layout.pages[page], f64::from(doc.layout.scale));
    let size = doc.sizes[page];
    let (w, h, x, y) = (f64::from(size.width), f64::from(size.height), f64::from(x), f64::from(y));
    let (tx, ty) = match doc.layout.rotation % 4 {
        0 => (x, y),
        1 => (h - y, x),
        2 => (w - x, h - y),
        _ => (y, w - x),
    };
    (p.x as f64 + tx * s, p.y as f64 + ty * s)
}

/// The point of `page` under document pixel (dx, dy), in points from the top-left of the page as
/// it is: the reverse of `to_doc`.
fn from_doc(doc: &Doc, page: usize, dx: f64, dy: f64) -> (f32, f32) {
    let (p, s) = (doc.layout.pages[page], f64::from(doc.layout.scale));
    let size = doc.sizes[page];
    let (w, h) = (f64::from(size.width), f64::from(size.height));
    let (tx, ty) = ((dx - p.x as f64) / s, (dy - p.y as f64) / s);
    let (x, y) = match doc.layout.rotation % 4 {
        0 => (tx, ty),
        1 => (ty, h - tx),
        2 => (w - tx, h - ty),
        _ => (w - ty, tx),
    };
    (x as f32, y as f32)
}

/// A rectangle of `page` (x0, y0, x1, y1 in points of the page as it is) in document pixels.
fn rect_to_doc(doc: &Doc, page: usize, r: [f32; 4]) -> kurbo::Rect {
    let (ax, ay) = to_doc(doc, page, r[0], r[1]);
    let (bx, by) = to_doc(doc, page, r[2], r[3]);
    kurbo::Rect::new(ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))
}

/// A spot for a place the PDF names on `page`, `y` points from its top: for a turned page, where
/// that line of the page is in the turned page, or its top when the line runs down the screen.
fn target_spot(doc: &Doc, page: usize, y: f32) -> Option<Spot> {
    let p = doc.layout.pages.get(page)?;
    let (_, dy) = to_doc(doc, page, 0.0, y);
    let down = if doc.layout.rotation % 2 == 1 { 0.0 } else { (dy - p.y as f64) / f64::from(doc.layout.scale) };
    Some(Spot { page, y: down as f32 })
}

/// How a file was viewed, for the history: degrees turned, and "two" or "book" for spreads.
fn view_name(rotation: u8, spread: Spread) -> String {
    let deg = u32::from(rotation % 4) * 90;
    match spread {
        Spread::One => deg.to_string(),
        Spread::Two => format!("{deg}-two"),
        Spread::Book => format!("{deg}-book"),
    }
}

/// What `view_name` reads back; anything else is upright, one page per row.
fn parse_view(s: &str) -> (u8, Spread) {
    let (deg, spread) = s.split_once('-').unwrap_or((s, ""));
    let rotation = (deg.parse::<u32>().unwrap_or(0) / 90 % 4) as u8;
    let spread = match spread {
        "two" => Spread::Two,
        "book" => Spread::Book,
        _ => Spread::One,
    };
    (rotation, spread)
}

/// "8.2 MB (8,612,345 bytes)".
fn file_size(bytes: u64) -> String {
    let digits = bytes.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let short = match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / f64::from(1u32 << 30)),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / f64::from(1u32 << 20)),
        b => format!("{:.0} KB", (b as f64 / 1024.0).ceil()),
    };
    format!("{short} ({grouped} bytes)")
}

/// `text` made safe for a URL's query: letters, digits and `-._~` as they are, spaces as `+`, the
/// rest as %XX bytes of its UTF-8.
fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(char::from(b)),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A PNG of 0x00RRGGBB pixels, marked with its resolution.
fn encode_png(width: u32, height: u32, dpi: u32, pixels: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let per_meter = (f64::from(dpi) / 0.0254).round() as u32;
    encoder.set_pixel_dims(Some(png::PixelDimensions { xppu: per_meter, yppu: per_meter, unit: png::Unit::Meter }));
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    // writing to memory can't fail
    if let Ok(mut writer) = encoder.write_header() {
        let _ = writer.write_image_data(&rgb);
    }
    out
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// What the command line asks for: files, and Sumatra's flags `-page N` (from 1) and `-zoom` with
/// "fit page", "fit width" or a percentage.
#[derive(Clone, Default, Debug, PartialEq)]
struct Args {
    paths: Vec<PathBuf>,
    page: Option<usize>,
    zoom: Option<Zoom>,
    /// Print the files and quit, without a window: on the printer named, or the default one.
    print_to: Option<Option<String>>,
}

impl Args {
    /// The same request for another copy of the reader, whose current folder may differ.
    fn to_hand_over(&self) -> Vec<OsString> {
        let mut out = Vec::new();
        if let Some(page) = self.page {
            out.extend(["-page".into(), (page + 1).to_string().into()]);
        }
        if let Some(zoom) = self.zoom {
            out.extend(["-zoom".into(), zoom_name(zoom).into()]);
        }
        out.extend(self.paths.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()).into_os_string()));
        out
    }
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Args {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-page" | "--page") => {
                let n = args.next().and_then(|v| v.to_str()?.parse::<usize>().ok());
                out.page = Some(n.unwrap_or(1).saturating_sub(1));
            }
            Some("-print-to" | "--print-to") => out.print_to = Some(args.next().map(|v| v.to_string_lossy().into_owned())),
            Some("-print-to-default" | "--print-to-default") => out.print_to = Some(None),
            Some("-zoom" | "--zoom") => {
                if let Some(zoom) = args.next().and_then(|v| parse_zoom(v.to_str()?)) {
                    out.zoom = Some(zoom);
                }
            }
            _ => out.paths.push(PathBuf::from(arg)),
        }
    }
    out
}

/// "fit-page", "fit width", "150" or "150%".
fn parse_zoom(s: &str) -> Option<Zoom> {
    match s.to_ascii_lowercase().replace([' ', '-'], "").as_str() {
        "fitpage" => Some(Zoom::FitPage),
        "fitwidth" => Some(Zoom::FitWidth),
        p => p.trim_end_matches('%').parse().ok().filter(|p: &f32| p.is_finite() && *p > 0.0).map(Zoom::Percent),
    }
}

/// What `parse_zoom` reads back.
fn zoom_name(zoom: Zoom) -> String {
    match zoom {
        Zoom::FitPage => "fit-page".to_owned(),
        Zoom::FitWidth => "fit-width".to_owned(),
        Zoom::Percent(p) => p.to_string(),
    }
}

/// -print-to: prints each file on the printer named, or the default one, without a window.
fn print_files(paths: &[PathBuf], printer: Option<&str>) {
    for path in paths {
        let name = file_name(path);
        let result = engine::open(path).map_err(|e| format!("Couldn't open {name}: {e}")).and_then(|engine| {
            let what = printer.map_or_else(|| "the default printer".to_owned(), |p| format!("the printer \"{p}\""));
            let target = win::printer_named(printer, engine.page_count()).ok_or_else(|| format!("Couldn't find {what}"))?;
            print::print(&target, engine, &name, &AtomicBool::new(false), |_, _| {})
        });
        if let Err(e) = result {
            win::message_box(&e);
        }
    }
}

fn main() {
    // from setup, and from Settings' list of apps
    let raw: Vec<OsString> = std::env::args_os().skip(1).collect();
    if raw.first().is_some_and(|a| a == "--register") {
        std::process::exit(if install::register() { 0 } else { 1 });
    }
    if raw.first().is_some_and(|a| a == "--uninstall") {
        install::uninstall(raw.iter().any(|a| a == "--quiet"));
        return;
    }
    let args = parse_args(raw.into_iter());
    if let Some(printer) = &args.print_to {
        print_files(&args.paths, printer.as_deref());
        return;
    }
    settings::migrate();
    let settings = Settings::load();
    let mut start_lock = None;
    if settings.single_window {
        match win::hand_over(&args.to_hand_over()) {
            win::Start::HandedOver => return,
            win::Start::Alone(lock) => start_lock = lock,
        }
    }
    let event_loop = EventLoop::<Wake>::with_user_event().build().expect("starting the event loop");
    if settings.check_updates {
        // a few seconds in, so it doesn't compete with opening the first file
        let proxy = event_loop.create_proxy();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            if let Ok(Some(release)) = update::check() {
                let _ = proxy.send_event(Wake::Update(release));
            }
        });
    }
    let mut app = App {
        proxy: event_loop.create_proxy(),
        window: None,
        surface: None,
        start: Some(args),
        start_lock,
        tabs: Vec::new(),
        active: 0,
        doc: None,
        error: None,
        status: None,
        printing: None,
        watcher: {
            let proxy = event_loop.create_proxy();
            watch::Watcher::new(move |path| {
                let _ = proxy.send_event(Wake::FileChanged(path));
            })
        },
        history: History::load(),
        favorites: favorites::Favorites::load(),
        highlights: highlights::Highlights::load(),
        sidebar_favorites: false,
        annotating: false,
        tool: None,
        annot_colors: [settings.highlight_color as usize, 4],
        annot_width: 1,
        drawing: None,
        editor: None,
        mark_font: fonts::Font::load(&["segoeui.ttf", "arial.ttf"]),
        update: None,
        downloading: false,
        thumbs: {
            let proxy = Mutex::new(event_loop.create_proxy());
            thumbs::Thumbs::new(move || {
                let _ = proxy.lock().unwrap().send_event(Wake::Redraw);
            })
        },
        settings,
        chrome: Chrome::new(),
        system_dark: win::system_dark(),
        menu: None,
        prompt: None,
        props_title: "Document properties",
        props: None,
        closed: Vec::new(),
        glide: None,
        on_scrollbar: false,
        on_hbar: false,
        thumb_drag: None,
        side_drag: None,
        tab_drag: None,
        autoscroll: false,
        autoscroll_at: None,
        presenting: None,
        right_down: false,
        right_zoomed: false,
        modifiers: ModifiersState::empty(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        drag: None,
        selecting: false,
        last_click: None,
        cursor_icon: CursorIcon::Default,
        search: String::new(),
        search_focus: false,
        search_fresh: false,
        link_tip: None,
        pressed_link: None,
        hover: None,
        hover_since: Instant::now(),
        pressed: None,
        panel_open: false,
        panel_scroll: 0.0,
        panel_tab: 0,
        is_default: false,
        editing: None,
        title: String::new(),
    };
    event_loop.run_app(&mut app).expect("running the event loop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_coordinates_go_both_ways() {
        // an upright Letter page: y flips about the page's height
        let t = [1.0, 0.0, 0.0, -1.0, 0.0, 792.0];
        assert_eq!(pdf_to_page(t, [72.0, 700.0, 172.0, 720.0]), [72.0, 72.0, 172.0, 92.0]);
        assert_eq!(page_to_pdf(t, [72.0, 72.0, 172.0, 92.0]), [72.0, 700.0, 172.0, 720.0]);
    }

    #[test]
    fn small_formats() {
        assert_eq!(url_encode("large language models"), "large+language+models");
        assert_eq!(url_encode("a&b=c/ü"), "a%26b%3Dc%2F%C3%BC");
        assert_eq!(file_size(8_612_345), "8.2 MB (8,612,345 bytes)");
        assert_eq!(file_size(900), "1 KB (900 bytes)");
        let png = encode_png(2, 1, 150, &[0x00FF_0000, 0x0000_00FF]);
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn command_line() {
        let args = |s: &str| parse_args(s.split(' ').map(OsString::from));
        assert_eq!(args("a.pdf b.pdf"), Args { paths: vec!["a.pdf".into(), "b.pdf".into()], page: None, zoom: None, print_to: None });
        assert_eq!(args("-print-to-default a.pdf").print_to, Some(None));
        assert_eq!(args("-print-to Office a.pdf").print_to, Some(Some("Office".to_owned())));
        assert_eq!(args("-page 3 -zoom fit-width a.pdf").page, Some(2));
        assert_eq!(args("-zoom 150% a.pdf").zoom, Some(Zoom::Percent(150.0)));
        assert_eq!(args("-zoom nonsense a.pdf").zoom, None);
        for zoom in [Zoom::FitPage, Zoom::FitWidth, Zoom::Percent(66.67)] {
            assert_eq!(parse_zoom(&zoom_name(zoom)), Some(zoom));
        }
        // what one copy hands another reads back the same, with the path made absolute
        let given = args("-page 5 -zoom 200 x.pdf");
        let handed = parse_args(given.to_hand_over());
        assert_eq!((handed.page, handed.zoom), (given.page, given.zoom));
        assert!(handed.paths[0].is_absolute() && handed.paths[0].ends_with("x.pdf"));
    }
}
