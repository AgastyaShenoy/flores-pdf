//! The tab strip, toolbar, outline sidebar, settings panel, right-click menu, start page and
//! tooltips, drawn with vello_cpu into the window buffer.
//!
//! Each part is drawn into its own pixel buffer and kept until something it shows changes (hover,
//! page number, a setting), so scrolling a document costs one copy per part, not a redraw. Drawing
//! also records where the clickable parts are, which `hit` uses.

use crate::annots::{self, Piece, Tool};
use crate::settings::{FIELDS, Kind, Settings, Value};
use crate::fonts::Font;
use crate::thumbs::Thumb;
use kurbo::{Affine, Cap, Circle, Join, Rect, RoundedRect, Shape, Stroke};
use std::sync::Arc;
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};

/// In logical pixels.
pub const BAR_HEIGHT: f64 = 40.0;
pub const TAB_HEIGHT: f64 = 36.0;
/// The settings window's widest.
pub const PANEL_WIDTH: f64 = 860.0;
const TEXT_PX: f64 = 13.0;
const SMALL_PX: f64 = 12.0;
const ICON_PX: f64 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub bar: u32,
    pub panel: u32,
    pub text: u32,
    pub dim: u32,
    pub hover: u32,
    pub press: u32,
    pub line: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub field: u32,
    /// Around the pages.
    pub canvas: u32,
    pub page_border: u32,
    pub thumb: u32,
    /// Behind the tabs, and a tab under the mouse.
    pub strip: u32,
    pub tab_hover: u32,
}

pub const LIGHT: Palette = Palette {
    bar: 0xF3F3F3,
    panel: 0xF9F9F9,
    text: 0x1B1B1B,
    dim: 0x616161,
    hover: 0xE2E2E2,
    press: 0xD4D4D4,
    line: 0xD1D1D1,
    accent: 0x005FB8,
    on_accent: 0xFFFFFF,
    field: 0xFFFFFF,
    canvas: 0xCBCBCB,
    page_border: 0xA9A9A9,
    thumb: 0x8A8A8A,
    strip: 0xE4E4E4,
    tab_hover: 0xECECEC,
};

pub const DARK: Palette = Palette {
    bar: 0x2B2B2B,
    panel: 0x262626,
    text: 0xF2F2F2,
    dim: 0xA6A6A6,
    hover: 0x3B3B3B,
    press: 0x323232,
    line: 0x424242,
    accent: 0x60CDFF,
    on_accent: 0x000000,
    field: 0x1D1D1D,
    canvas: 0x161616,
    page_border: 0x050505,
    thumb: 0x6E6E6E,
    strip: 0x1F1F1F,
    tab_hover: 0x262626,
};

/// Pure black wherever there's no text, for OLED screens; lines and hovers stay just visible.
pub const BLACK: Palette = Palette {
    bar: 0x000000,
    panel: 0x000000,
    text: 0xE6E6E6,
    dim: 0x9A9A9A,
    hover: 0x1C1C1C,
    press: 0x141414,
    line: 0x2E2E2E,
    accent: 0x60CDFF,
    on_accent: 0x000000,
    field: 0x0E0E0E,
    canvas: 0x000000,
    page_border: 0x333333,
    thumb: 0x5C5C5C,
    strip: 0x000000,
    tab_hover: 0x141414,
};

/// Something drawn here that reacts to the mouse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// A tab, by position, and its close button.
    Tab(usize),
    TabClose(usize),
    NewTab,
    /// A file on the start page, by its place in the list, and its button to remove it.
    Recent(usize),
    Forget(usize),
    /// A right-click menu entry, by index, and the menu's background.
    MenuItem(usize),
    Menu,
    /// The document properties box, and its close button.
    Props,
    PropsClose,
    /// The password box, and its buttons.
    Prompt,
    PromptOk,
    PromptCancel,
    Outline,
    /// An outline entry, by its index in the document's flattened outline.
    OutlineRow(usize),
    /// The arrow that expands or collapses an outline entry.
    OutlineToggle(usize),
    /// The sidebar's background.
    Sidebar,
    /// The sidebar's Outline (false) or Bookmarks (true) heading.
    SideTab(bool),
    /// A bookmark, by its place in the list, and its button to remove it.
    Favorite(usize),
    Unfavorite(usize),
    /// The toolbar's button for the annotation tools, and the tools' bar: a tool (in `Tool::ALL`),
    /// a color, a line width, undo and save.
    Annotate,
    Tool(usize),
    Color(usize),
    Width(usize),
    Undo,
    Save,
    /// The text box for a note, a text box's text or a bookmark's name, and its buttons.
    Editor,
    EditorOk,
    EditorCancel,
    EditorDelete,
    Open,
    Prev,
    Next,
    PageBox,
    ZoomOut,
    ZoomIn,
    FitPage,
    FitWidth,
    /// The view menu: spreads, rotation, presentation.
    View,
    /// Fill in the document's form, or stop.
    FillForm,
    /// The main menu: everything the reader does, in one list.
    MainMenu,
    SearchBox,
    SearchPrev,
    SearchNext,
    Settings,
    Close,
    Toggle(usize),
    /// Field index, direction.
    Step(usize, i64),
    OpenSettingsFile,
    Reset,
    /// A category of the settings window, a setting's drop-down, and the About button.
    SettingsTab(usize),
    Dropdown(usize),
    AboutBox,
    /// The toolbar's button for the start page, and the one for a newer release.
    Home,
    Update,
    /// Register with Windows and open its Default apps page.
    MakeDefault,
    /// The panel's background, so clicks there don't reach the page underneath.
    Panel,
}

impl Hit {
    pub fn tooltip(self) -> Option<&'static str> {
        Some(match self {
            Hit::Outline => "Outline (F12)",
            Hit::Home => "Home: recent files (Alt+Home)",
            Hit::Update => "A newer Flores PDF is out",
            Hit::Open => "Open a file (Ctrl+O)",
            Hit::Prev => "Previous page (P)",
            Hit::Next => "Next page (N)",
            Hit::PageBox => "Go to page (Ctrl+G)",
            Hit::ZoomOut => "Zoom out (−)",
            Hit::ZoomIn => "Zoom in (+)",
            Hit::FitPage => "Fit page (Ctrl+0)",
            Hit::FitWidth => "Fit width (Ctrl+2)",
            Hit::View => "Page layout, rotation and presentation",
            Hit::FillForm => "Fill in this form (Ctrl+S saves it)",
            Hit::Annotate => "Annotate: highlight, draw, add notes and text",
            Hit::Tool(i) => annots::Tool::ALL.get(i).map_or("", |t| t.name()),
            Hit::Color(_) => "Color",
            Hit::Width(0) => "Thin lines, small text",
            Hit::Width(1) => "Medium lines and text",
            Hit::Width(_) => "Thick lines, large text",
            Hit::Undo => "Undo the last annotation (Ctrl+Z)",
            Hit::Save => "Save into the file (Ctrl+S)",
            Hit::MainMenu => "Menu (F10)",
            Hit::SearchBox => "Find in document (Ctrl+F)",
            Hit::SearchPrev => "Previous match (Shift+F3)",
            Hit::SearchNext => "Next match (F3)",
            Hit::Settings => "Settings (Ctrl+,)",
            Hit::TabClose(_) => "Close tab (Ctrl+W)",
            Hit::NewTab => "Open a file in a new tab (Ctrl+O)",
            Hit::Forget(_) => "Remove from the list",
            _ => return None,
        })
    }
}

// Segoe Fluent Icons / Segoe MDL2 Assets
const ICON_OPEN: char = '\u{E8E5}';
const ICON_UP: char = '\u{E70E}';
const ICON_DOWN: char = '\u{E70D}';
const ICON_ZOOM_OUT: char = '\u{E71F}';
const ICON_ZOOM_IN: char = '\u{E8A3}';
const ICON_FIT_PAGE: char = '\u{E9A6}';
const ICON_FIT_WIDTH: char = '\u{E799}';
const ICON_SETTINGS: char = '\u{E713}';
const ICON_CLOSE: char = '\u{E711}';
const ICON_SEARCH: char = '\u{E721}';
const ICON_OUTLINE: char = '\u{E700}';
const ICON_RIGHT: char = '\u{E76C}';
const ICON_ADD: char = '\u{E710}';
const ICON_CHECK: char = '\u{E73E}';
const ICON_VIEW: char = '\u{E89A}';
const ICON_EDIT: char = '\u{E70F}';
const ICON_MORE: char = '\u{E712}';
const ICON_HOME: char = '\u{E80F}';
const ICON_UPDATE: char = '\u{E896}';
const ICON_ANNOTATE: char = '\u{EDC6}';
const ICON_UNDO: char = '\u{E7A7}';
const ICON_SAVE: char = '\u{E74E}';
const ICON_DOC: char = '\u{E8A5}';
/// The annotation tools' icons, in `Tool::ALL`'s order, and letters for when the icon font is missing.
const TOOL_ICONS: [(char, &str); 11] = [
    ('\u{E7E6}', "H"),
    ('\u{E8DC}', "U"),
    ('\u{EDE0}', "S"),
    ('\u{E90A}', "N"),
    ('\u{ED63}', "P"),
    ('\u{E8D2}', "T"),
    ('\u{E739}', "R"),
    ('\u{EA3A}', "O"),
    ('\u{E738}', "L"),
    ('\u{E72A}', "A"),
    ('\u{E75C}', "E"),
];
/// Width of a menu's check mark column, in logical pixels.
const CHECK_GUTTER: f64 = 20.0;

fn has_checks(m: &Menu) -> bool {
    m.entries.iter().any(|e| matches!(e, MenuEntry::Check(..) | MenuEntry::Choice(..)))
}
/// Outline rows, in logical pixels.
const ROW: f64 = 28.0;
const INDENT: f64 = 16.0;
/// Right-click menu rows and padding, and start page rows, in logical pixels.
const MENU_ROW: f64 = 30.0;
const MENU_PAD: f64 = 4.0;
/// The start page's cards: the thumbnail's box, the card's width, and the room under the thumbnail.
pub const THUMB_W: f64 = 132.0;
pub const THUMB_H: f64 = 172.0;
const CARD_W: f64 = 156.0;
const CARD_TEXT: f64 = 46.0;
/// The properties box: its width, padding and label column, in logical pixels.
const PROPS_WIDTH: f64 = 540.0;
const PROPS_PAD: f64 = 20.0;
const PROPS_LABEL: f64 = 110.0;

/// What the toolbar shows. It's drawn again only when this changes.
#[derive(Clone, PartialEq, Debug)]
pub struct Bar {
    /// Where the toolbar starts: below the tabs, if they're shown.
    pub top: u32,
    pub width: u32,
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
    pub has_doc: bool,
    /// The page number, or the text being typed into the page box.
    pub page: String,
    pub editing: bool,
    pub pages: usize,
    pub zoom: String,
    pub panel_open: bool,
    pub search: String,
    pub search_focus: bool,
    /// "3 of 17", "No results", or empty.
    pub search_status: String,
    pub has_matches: bool,
    pub has_outline: bool,
    pub outline_shown: bool,
    /// The document has a form, and it's being filled in.
    pub has_form: bool,
    pub filling: bool,
    /// The document can be annotated (it's a PDF), and the annotation tools are shown.
    pub can_annotate: bool,
    pub annotating: bool,
    /// A newer release to offer ("Update to 0.2.0"), or what's happening with it.
    pub update: Option<String>,
}

/// The annotation tools' bar, under the toolbar.
#[derive(Clone, PartialEq, Debug)]
pub struct AnnotBar {
    pub top: u32,
    pub width: u32,
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    /// The tool picked, in `Tool::ALL`.
    pub tool: Option<usize>,
    pub colors: Vec<u32>,
    pub color: usize,
    pub width_choice: usize,
    pub can_undo: bool,
    pub unsaved: bool,
}

/// The box for typing a note, a text box's text or a bookmark's name.
#[derive(Clone, PartialEq, Debug)]
pub struct Editor {
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    pub title: String,
    pub text: String,
    /// Several lines can be typed (Enter starts a new one; Ctrl+Enter is OK).
    pub multiline: bool,
    pub can_delete: bool,
    /// The text is all selected: typing replaces it.
    pub selected: bool,
}

/// An outline entry as the sidebar shows it.
#[derive(Clone, PartialEq, Debug)]
pub struct OutlineRow {
    /// Index in the document's flattened outline.
    pub node: usize,
    pub title: String,
    pub depth: usize,
    pub children: bool,
    pub open: bool,
}

/// What the outline sidebar shows.
#[derive(Clone, PartialEq, Debug)]
pub struct Sidebar {
    pub top: u32,
    pub width: u32,
    pub height: u32,
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    pub scroll: f64,
    /// The entries whose parents are all expanded.
    pub rows: Vec<OutlineRow>,
    /// The entry for where the reader is.
    pub current: Option<usize>,
    pub has_outline: bool,
    /// The bookmarks tab is the one shown; its entries, with their pages (from 1).
    pub showing_favorites: bool,
    pub favorites: Vec<(String, usize)>,
}

/// What the settings panel shows.
#[derive(Clone, PartialEq, Debug)]
pub struct Panel {
    /// Where the settings window is in the main window, and its size.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    pub scroll: f64,
    /// The category shown, in `panel_sections()`.
    pub tab: usize,
    pub settings: Settings,
    pub file: String,
    /// Double-clicking a PDF opens this program.
    pub is_default: bool,
}

/// What the tab strip shows.
#[derive(Clone, PartialEq, Debug)]
pub struct Tabs {
    pub width: u32,
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    pub names: Vec<String>,
    pub active: usize,
}

/// A file on the start page.
#[derive(Clone, PartialEq, Debug)]
pub struct Recent {
    pub name: String,
    pub folder: String,
    /// From 1.
    pub page: usize,
    /// Its first page, small, once it's made.
    pub thumb: Option<Arc<Thumb>>,
}

/// The start page: recent files, in the pages' area while no document is open.
#[derive(Clone, PartialEq, Debug)]
pub struct Start {
    /// The pages' area, in window coordinates.
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    pub recent: Vec<Recent>,
    /// How pages are colored, for the thumbnails.
    pub look: render::Look,
}

#[derive(Clone, PartialEq, Debug)]
pub enum MenuEntry {
    /// A label and its shortcut.
    Item(&'static str, &'static str),
    /// One of a set of choices, with a check mark when it's the one in effect.
    Check(&'static str, &'static str, bool),
    /// A choice from the document, such as a drop-down field's, checked when it's the one picked.
    Choice(String, bool),
    Separator,
}

/// A right-click menu, opening at (x, y) in the window, or above or left of it if there's no room.
#[derive(Clone, PartialEq, Debug)]
pub struct Menu {
    pub x: f64,
    pub y: f64,
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    pub entries: Vec<MenuEntry>,
}

/// The document properties box: a label and a value per row.
#[derive(Clone, PartialEq, Debug)]
pub struct Props {
    pub title: &'static str,
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    pub rows: Vec<(&'static str, String)>,
}

/// The password box for a protected file.
#[derive(Clone, PartialEq, Debug)]
pub struct Prompt {
    pub dpi: f64,
    pub palette: Palette,
    pub hover: Option<Hit>,
    /// The file's name.
    pub name: String,
    /// Characters typed so far, shown as dots.
    pub typed: usize,
    /// The last password tried was wrong.
    pub wrong: bool,
}

/// A form field as drawn over the page while the form is filled in.
#[derive(Clone, PartialEq, Debug)]
pub struct FieldLook {
    /// In window coordinates.
    pub rect: Rect,
    pub kind: FieldShape,
    pub text: String,
    pub checked: bool,
    pub focused: bool,
    /// Text size in pixels.
    pub font: f64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FieldShape {
    Text,
    Check,
    Radio,
    Choice,
}

/// Which parts are on screen, for hit testing.
#[derive(Clone, Copy, Default)]
pub struct Shown {
    pub menu: bool,
    pub props: bool,
    pub prompt: bool,
    pub panel: bool,
    pub sidebar: bool,
    pub tabs: bool,
    pub bar: bool,
    pub annot_bar: bool,
    pub editor: bool,
    pub start: bool,
}

struct Drawn<S> {
    state: S,
    pixels: Vec<u32>,
    /// In window coordinates.
    hits: Vec<(Hit, Rect)>,
}

pub struct Chrome {
    text: Font,
    bold: Option<Font>,
    icons: Option<Font>,
    bar: Option<Drawn<Bar>>,
    annot_bar: Option<Drawn<AnnotBar>>,
    panel: Option<Drawn<Panel>>,
    sidebar: Option<Drawn<Sidebar>>,
    tabs: Option<Drawn<Tabs>>,
    /// The menu and the properties box, and where they went.
    menu: Option<(Drawn<Menu>, Rect)>,
    props: Option<(Drawn<Props>, Rect)>,
    /// The password box is small and changes with each key, so only where its buttons are is kept.
    prompt_hits: Vec<(Hit, Rect)>,
    editor_hits: Vec<(Hit, Rect)>,
    /// The start page's pixels aren't kept: it's big, and drawn only when there's nothing else.
    start_hits: Vec<(Hit, Rect)>,
    /// Height of everything in the panel, for scrolling.
    panel_content: f64,
    /// Height of the part of the settings window the cards scroll in.
    panel_view: f64,
    sidebar_content: f64,
    tooltip: Option<(String, f64, Palette, u32, u32, Vec<u32>)>,
}

/// Draws onto a vello_cpu context with the UI fonts and colors.
struct Canvas<'a> {
    ctx: &'a mut RenderContext,
    text: &'a mut Font,
    bold: Option<&'a mut Font>,
    icons: Option<&'a mut Font>,
    p: &'a Palette,
    s: f64,
}

fn rgb(c: u32) -> AlphaColor<Srgb> {
    AlphaColor::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}

impl Canvas<'_> {
    fn fill(&mut self, r: Rect, c: u32) {
        self.ctx.set_paint(rgb(c));
        self.ctx.fill_rect(&r);
    }

    fn round(&mut self, r: Rect, radius: f64, c: u32) {
        self.ctx.set_paint(rgb(c));
        self.ctx.fill_path(&RoundedRect::from_rect(r, radius * self.s).to_path(0.1));
    }

    fn outline(&mut self, r: Rect, radius: f64, width: f64, c: u32) {
        let inset = width * self.s / 2.0;
        self.ctx.set_paint(rgb(c));
        self.ctx.set_stroke(Stroke::new(width * self.s));
        self.ctx.stroke_path(&RoundedRect::from_rect(r.inset(-inset), radius * self.s).to_path(0.1));
    }

    fn line_px(&self) -> f64 {
        self.s.round().max(1.0)
    }

    fn px(&self, logical: f64) -> f32 {
        (logical * self.s) as f32
    }

    fn measure(&mut self, text: &str, size: f64) -> f64 {
        let px = self.px(size);
        f64::from(self.text.measure(text, px))
    }

    /// Text with its top at `y`.
    fn label(&mut self, text: &str, size: f64, x: f64, y: f64, c: u32, bold: bool) {
        let px = self.px(size);
        self.ctx.set_paint(rgb(c));
        let font = match (bold, self.bold.as_deref_mut()) {
            (true, Some(b)) => b,
            _ => &mut *self.text,
        };
        let (ascent, _) = font.line(px);
        font.draw(self.ctx, text, px, x, y + f64::from(ascent));
    }

    /// Text starting at `x`, centered vertically in `r`.
    fn label_middle(&mut self, text: &str, size: f64, x: f64, r: Rect, c: u32) {
        let (_, height) = self.text.line(self.px(size));
        self.label(text, size, x, r.center().y - f64::from(height) / 2.0, c, false);
    }

    fn centered(&mut self, text: &str, size: f64, r: Rect, c: u32) {
        let px = self.px(size);
        self.ctx.set_paint(rgb(c));
        self.text.draw_centered(self.ctx, text, px, (r.x0, r.y0, r.width(), r.height()));
    }

    fn icon(&mut self, icon: char, fallback: &str, r: Rect, c: u32) {
        self.icon_sized(icon, fallback, ICON_PX, r, c);
    }

    fn icon_sized(&mut self, icon: char, fallback: &str, size: f64, r: Rect, c: u32) {
        let px = self.px(size);
        self.ctx.set_paint(rgb(c));
        match self.icons.as_deref_mut() {
            Some(f) => f.draw_centered(self.ctx, &icon.to_string(), px, (r.x0, r.y0, r.width(), r.height())),
            None => self.text.draw_centered(self.ctx, fallback, px, (r.x0, r.y0, r.width(), r.height())),
        }
    }

    /// A square toolbar button.
    #[allow(clippy::too_many_arguments)]
    fn button(&mut self, hits: &mut Vec<(Hit, Rect)>, hit: Hit, r: Rect, icon: char, fallback: &str, enabled: bool, b: &Bar) {
        let active = (hit == Hit::Settings && b.panel_open)
            || (hit == Hit::Outline && b.outline_shown)
            || (hit == Hit::FillForm && b.filling)
            || (hit == Hit::Annotate && b.annotating);
        if enabled && (b.pressed == Some(hit) || active) {
            self.round(r, 4.0, self.p.press);
        } else if enabled && b.hover == Some(hit) {
            self.round(r, 4.0, self.p.hover);
        }
        let c = if enabled { self.p.text } else { self.p.line };
        self.icon(icon, fallback, r, c);
        if enabled {
            hits.push((hit, r));
        }
    }

    /// A square button outside the toolbar, which has no pressed state.
    #[allow(clippy::too_many_arguments)]
    fn button_plain(&mut self, hits: &mut Vec<(Hit, Rect)>, hit: Hit, r: Rect, icon: char, fallback: &str, enabled: bool, hover: Option<Hit>) {
        if enabled && hover == Some(hit) {
            self.round(r, 4.0, self.p.hover);
        }
        self.icon(icon, fallback, r, if enabled { self.p.text } else { self.p.line });
        if enabled {
            hits.push((hit, r));
        }
    }

    fn separator(&mut self, x: &mut f64, height: f64) {
        let h = (20.0 * self.s).round();
        let top = ((height - h) / 2.0).round();
        let x0 = (*x + 4.0 * self.s).round();
        self.fill(Rect::new(x0, top, x0 + self.line_px(), top + h), self.p.line);
        *x += (9.0 * self.s).round();
    }

    /// `text`, cut with an ellipsis if it's wider than `room`.
    fn fit(&mut self, text: &str, size: f64, room: f64) -> String {
        if self.measure(text, size) <= room {
            return text.to_owned();
        }
        let mut cut = text.to_owned();
        while !cut.is_empty() && self.measure(&format!("{cut}…"), size) > room {
            cut.pop();
        }
        format!("{}…", cut.trim_end())
    }

    fn wrap(&mut self, text: &str, size: f64, width: f64) -> Vec<String> {
        wrap(self.text, text, self.px(size), width)
    }
}

/// Splits `text` into lines no wider than `width` pixels, breaking after spaces and backslashes.
fn wrap(font: &mut Font, text: &str, px: f32, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for token in text.split_inclusive([' ', '\\']) {
        let candidate = format!("{line}{token}");
        if !line.is_empty() && f64::from(font.measure(candidate.trim_end(), px)) > width {
            lines.push(line.trim_end().to_owned());
            line = token.to_owned();
        } else {
            line = candidate;
        }
    }
    if !line.trim().is_empty() {
        lines.push(line.trim_end().to_owned());
    }
    lines
}

impl Chrome {
    /// None when Segoe UI can't be found; the reader then works by keyboard only.
    pub fn new() -> Option<Self> {
        Some(Self {
            text: Font::load(&["segoeui.ttf", "arial.ttf"])?,
            bold: Font::load(&["seguisb.ttf", "segoeuib.ttf", "arialbd.ttf"]),
            icons: Font::load(&["SegoeIcons.ttf", "segmdl2.ttf"]),
            bar: None,
            annot_bar: None,
            panel: None,
            sidebar: None,
            tabs: None,
            menu: None,
            props: None,
            prompt_hits: Vec::new(),
            editor_hits: Vec::new(),
            start_hits: Vec::new(),
            panel_content: 0.0,
            panel_view: 0.0,
            sidebar_content: 0.0,
            tooltip: None,
        })
    }

    /// Draws with `draw` into a w × h buffer of 0x00RRGGBB pixels.
    /// The context and target are made for each drawing and dropped after, since parts are only
    /// drawn when they change, and keeping them would hold scratch memory as big as the largest part.
    fn render(&mut self, w: u32, h: u32, palette: &Palette, dpi: f64, draw: impl FnOnce(&mut Canvas)) -> Vec<u32> {
        let (w16, h16) = (w as u16, h as u16);
        let mut ctx = RenderContext::new_with(w16, h16, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
        draw(&mut Canvas {
            ctx: &mut ctx,
            text: &mut self.text,
            bold: self.bold.as_mut(),
            icons: self.icons.as_mut(),
            p: palette,
            s: dpi,
        });
        ctx.flush();
        let mut pixmap = Pixmap::new(w16, h16);
        let black = AlphaColor::from_rgba8(0, 0, 0, 255);
        ctx.render_with(&mut pixmap, &mut Resources::default(), RasterizerSettings {
            target_init: TargetInit::Clear(black),
            ..Default::default()
        });
        pixmap.data_as_u8_slice().chunks_exact(4).map(|c| u32::from_be_bytes([0, c[0], c[1], c[2]])).collect()
    }

    /// Toolbar height in pixels.
    pub fn bar_height(dpi: f64) -> u32 {
        (BAR_HEIGHT * dpi).round() as u32
    }

    pub fn draw_bar(&mut self, frame: &mut [u32], frame_width: u32, b: &Bar) {
        let h = Self::bar_height(b.dpi);
        if self.bar.as_ref().is_none_or(|d| d.state != *b) {
            let mut hits = Vec::new();
            let pixels = self.render(b.width, h, &b.palette, b.dpi, |c| draw_bar(c, b, &mut hits));
            for (_, r) in &mut hits {
                *r = *r + kurbo::Vec2::new(0.0, f64::from(b.top));
            }
            self.bar = Some(Drawn { state: b.clone(), pixels, hits });
        }
        let d = self.bar.as_ref().unwrap();
        blit(frame, frame_width, 0, b.top, b.width, h, &d.pixels);
    }

    pub fn draw_annot_bar(&mut self, frame: &mut [u32], frame_width: u32, b: &AnnotBar) {
        let h = Self::bar_height(b.dpi);
        if self.annot_bar.as_ref().is_none_or(|d| d.state != *b) {
            let mut hits = Vec::new();
            let pixels = self.render(b.width, h, &b.palette, b.dpi, |c| draw_annot_bar(c, b, &mut hits));
            for (_, r) in &mut hits {
                *r = *r + kurbo::Vec2::new(0.0, f64::from(b.top));
            }
            self.annot_bar = Some(Drawn { state: b.clone(), pixels, hits });
        }
        let d = self.annot_bar.as_ref().unwrap();
        blit(frame, frame_width, 0, b.top, b.width, h, &d.pixels);
    }

    pub fn close_annot_bar(&mut self) {
        self.annot_bar = None;
    }

    /// Draws the text box for a note, text or a bookmark's name in the middle of the window.
    pub fn draw_editor(&mut self, frame: &mut [u32], frame_width: u32, frame_height: u32, e: &Editor) {
        let s = e.dpi;
        let w = (440.0 * s).min(f64::from(frame_width) - 32.0 * s).max(240.0 * s).round();
        let h = (if e.multiline { 300.0 } else { 170.0 } * s).min(f64::from(frame_height)).round();
        let mut hits = Vec::new();
        let pixels = self.render(w as u32, h as u32, &e.palette, s, |c| draw_editor(c, e, w, h, &mut hits));
        let x = ((f64::from(frame_width) - w) / 2.0).max(0.0).round();
        let y = ((f64::from(frame_height) - h) / 2.0).max(0.0).round();
        for (_, r) in &mut hits {
            *r = *r + kurbo::Vec2::new(x, y);
        }
        self.editor_hits = hits;
        blit(frame, frame_width, x as u32, y as u32, w as u32, h as u32, &pixels);
    }

    pub fn close_editor(&mut self) {
        self.editor_hits.clear();
    }

    /// Draws annotations over the pages, each a list of pieces in window coordinates, cut to `clip`
    /// (the pages' area). Each is drawn over what's there, in a box just big enough for it.
    pub fn draw_marks(&mut self, frame: &mut [u32], frame_width: u32, clip: Rect, marks: &[Vec<Piece>]) {
        for pieces in marks {
            let Some(bounds) = pieces.iter().map(|p| p.path.bounding_box().inflate(p.stroke / 2.0 + 1.0, p.stroke / 2.0 + 1.0)).reduce(|a, b| a.union(b))
            else {
                continue;
            };
            let area = bounds.intersect(clip);
            let area = Rect::new(area.x0.floor(), area.y0.floor(), area.x1.ceil(), area.y1.ceil());
            if area.width() < 1.0 || area.height() < 1.0 || area.width() > 16000.0 || area.height() > 16000.0 {
                continue;
            }
            let (w, h) = (area.width() as u16, area.height() as u16);
            let (x0, y0) = (area.x0 as usize, area.y0 as usize);
            let fw = frame_width as usize;
            let mut pixmap = Pixmap::new(w, h);
            for (row, line) in pixmap.data_as_u8_slice_mut().chunks_exact_mut(usize::from(w) * 4).enumerate() {
                let start = (y0 + row) * fw + x0;
                for (px, src) in line.chunks_exact_mut(4).zip(&frame[start..start + usize::from(w)]) {
                    px.copy_from_slice(&[(src >> 16) as u8, (src >> 8) as u8, *src as u8, 255]);
                }
            }
            let mut ctx = RenderContext::new_with(w, h, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
            ctx.set_transform(Affine::translate((-area.x0, -area.y0)));
            for p in pieces {
                let c = p.color;
                ctx.set_paint(AlphaColor::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, p.alpha));
                if p.stroke > 0.0 {
                    ctx.set_stroke(Stroke::new(p.stroke).with_caps(Cap::Round).with_join(Join::Round));
                    ctx.stroke_path(&p.path);
                } else {
                    ctx.fill_path(&p.path);
                }
            }
            ctx.flush();
            ctx.render_with(&mut pixmap, &mut Resources::default(), RasterizerSettings { target_init: TargetInit::SrcOver, ..Default::default() });
            for (row, line) in pixmap.data_as_u8_slice().chunks_exact(usize::from(w) * 4).enumerate() {
                let start = (y0 + row) * fw + x0;
                for (dst, px) in frame[start..start + usize::from(w)].iter_mut().zip(line.chunks_exact(4)) {
                    *dst = u32::from_be_bytes([0, px[0], px[1], px[2]]);
                }
            }
        }
    }

    /// Tab strip height in pixels.
    pub fn tabs_height(dpi: f64) -> u32 {
        (TAB_HEIGHT * dpi).round() as u32
    }

    /// Draws the tab strip at the top of the window.
    pub fn draw_tabs(&mut self, frame: &mut [u32], frame_width: u32, t: &Tabs) {
        let h = Self::tabs_height(t.dpi);
        if self.tabs.as_ref().is_none_or(|d| d.state != *t) {
            let mut hits = Vec::new();
            let pixels = self.render(t.width, h, &t.palette, t.dpi, |c| draw_tabs(c, t, &mut hits));
            self.tabs = Some(Drawn { state: t.clone(), pixels, hits });
        }
        let d = self.tabs.as_ref().unwrap();
        blit(frame, frame_width, 0, 0, t.width, h, &d.pixels);
    }

    pub fn close_tabs(&mut self) {
        self.tabs = None;
    }

    /// Draws the start page's list of recent files, centered in the pages' area.
    pub fn draw_start(&mut self, frame: &mut [u32], frame_width: u32, st: &Start) {
        let s = st.dpi;
        // as many cards across as fit, up to six
        let per_row = ((f64::from(st.width) - 32.0 * s) / (CARD_W * s)).floor().clamp(1.0, 6.0);
        let width = (per_row * CARD_W * s).round().min(f64::from(st.width) - 32.0 * s).max(0.0) as u32;
        let height = st.height.saturating_sub((48.0 * s) as u32);
        self.start_hits.clear();
        if width == 0 || height == 0 {
            return;
        }
        let mut hits = Vec::new();
        let mut boxes = Vec::new();
        let mut pixels = self.render(width, height, &st.palette, s, |c| boxes = draw_start(c, st, width, height, &mut hits));
        // the thumbnails, centered in their boxes, in the pages' colors
        for (i, b) in boxes {
            let Some(t) = st.recent.get(i).and_then(|r| r.thumb.as_ref()) else { continue };
            let mut image = t.pixels.clone();
            st.look.apply(&mut image);
            let x = (b.x0 + (b.width() - f64::from(t.width)) / 2.0).round().max(0.0) as u32;
            let y = (b.y0 + (b.height() - f64::from(t.height)) / 2.0).round().max(0.0) as u32;
            let w = t.width.min(width.saturating_sub(x));
            for row in 0..t.height.min(height.saturating_sub(y)) {
                let from = (row * t.width) as usize;
                let to = ((y + row) * width + x) as usize;
                pixels[to..to + w as usize].copy_from_slice(&image[from..from + w as usize]);
            }
        }
        let (x, y) = (st.left + (st.width - width) / 2, st.top + (32.0 * s) as u32);
        for (_, r) in &mut hits {
            *r = *r + kurbo::Vec2::new(f64::from(x), f64::from(y));
        }
        self.start_hits = hits;
        blit(frame, frame_width, x, y, width, height, &pixels);
    }

    pub fn close_start(&mut self) {
        self.start_hits.clear();
    }

    /// Draws the right-click menu.
    pub fn draw_menu(&mut self, frame: &mut [u32], frame_width: u32, frame_height: u32, m: &Menu) {
        if self.menu.as_ref().is_none_or(|(d, _)| d.state != *m) {
            let (w, h) = self.menu_size(m);
            let mut hits = Vec::new();
            let pixels = self.render(w, h, &m.palette, m.dpi, |c| draw_menu(c, m, w, h, &mut hits));
            // below and right of the click if there's room, otherwise above or left of it
            let (fw, fh, w64, h64) = (f64::from(frame_width), f64::from(frame_height), f64::from(w), f64::from(h));
            let x = if m.x + w64 <= fw { m.x } else { (m.x - w64).max(0.0) }.round();
            let y = if m.y + h64 <= fh { m.y } else { (m.y - h64).max(0.0) }.round();
            for (_, r) in &mut hits {
                *r = *r + kurbo::Vec2::new(x, y);
            }
            self.menu = Some((Drawn { state: m.clone(), pixels, hits }, Rect::new(x, y, x + w64, y + h64)));
        }
        let (d, at) = self.menu.as_ref().unwrap();
        blit(frame, frame_width, at.x0 as u32, at.y0 as u32, at.width() as u32, at.height() as u32, &d.pixels);
    }

    fn menu_size(&mut self, m: &Menu) -> (u32, u32) {
        let s = m.dpi;
        let px = (TEXT_PX * s) as f32;
        let mut widest = 0.0f64;
        let mut height = 2.0 * (MENU_PAD * s).round();
        for e in &m.entries {
            match e {
                MenuEntry::Item(label, keys) | MenuEntry::Check(label, keys, _) => {
                    widest = widest.max(f64::from(self.text.measure(label, px) + self.text.measure(keys, px)));
                    height += (MENU_ROW * s).round();
                }
                MenuEntry::Choice(label, _) => {
                    widest = widest.max(f64::from(self.text.measure(label, px)));
                    height += (MENU_ROW * s).round();
                }
                MenuEntry::Separator => height += (9.0 * s).round(),
            }
        }
        let gutter = if has_checks(m) { CHECK_GUTTER * s } else { 0.0 };
        ((widest + gutter + 72.0 * s).max(200.0 * s).ceil() as u32, height as u32)
    }

    pub fn close_menu(&mut self) {
        self.menu = None;
    }

    /// Draws the document properties box in the middle of the window.
    pub fn draw_props(&mut self, frame: &mut [u32], frame_width: u32, frame_height: u32, p: &Props) {
        let s = p.dpi;
        let (fw, fh) = (f64::from(frame_width), f64::from(frame_height));
        // placed for the window's size: a resize closes the box
        if self.props.as_ref().is_none_or(|(d, _)| d.state != *p) {
            let w = (PROPS_WIDTH * s).min(fw - 32.0 * s).max(200.0 * s).round();
            // the values, wrapped to their column
            let value_w = w - 2.0 * PROPS_PAD * s - PROPS_LABEL * s;
            let px = (TEXT_PX * s) as f32;
            let lines: Vec<Vec<String>> = p.rows.iter().map(|(_, v)| wrap(&mut self.text, v, px, value_w)).collect();
            let line_h = (18.0 * s).round();
            let rows_h: f64 = lines.iter().map(|l| l.len().max(1) as f64 * line_h + (8.0 * s).round()).sum();
            let h = ((56.0 + PROPS_PAD) * s + rows_h).round().min(fh.max(100.0));
            let mut hits = Vec::new();
            let pixels = self.render(w as u32, h as u32, &p.palette, s, |c| draw_props(c, p, &lines, w, h, &mut hits));
            let (x, y) = (((fw - w) / 2.0).max(0.0).round(), ((fh - h) / 2.0).max(0.0).round());
            for (_, r) in &mut hits {
                *r = *r + kurbo::Vec2::new(x, y);
            }
            self.props = Some((Drawn { state: p.clone(), pixels, hits }, Rect::new(x, y, x + w, y + h)));
        }
        let (d, at) = self.props.as_ref().unwrap();
        blit(frame, frame_width, at.x0 as u32, at.y0 as u32, at.width() as u32, at.height() as u32, &d.pixels);
    }

    /// Draws form fields over the pages, cut to `clip` (the pages' area). Fields are small and change
    /// with each key, so they're drawn each frame rather than kept.
    pub fn draw_fields(&mut self, frame: &mut [u32], frame_width: u32, clip: Rect, fields: &[FieldLook], dpi: f64, palette: &Palette) {
        for f in fields {
            let r = Rect::new(f.rect.x0.round(), f.rect.y0.round(), f.rect.x1.round(), f.rect.y1.round());
            if r.intersect(clip).area() <= 0.0 || r.width() < 2.0 || r.height() < 2.0 || r.width() > 8000.0 || r.height() > 8000.0 {
                continue;
            }
            let (w, h) = (r.width() as u32, r.height() as u32);
            let pixels = self.render(w, h, palette, dpi, |c| draw_field(c, f, f64::from(w), f64::from(h)));
            blit_clipped(frame, frame_width, r.x0 as i64, r.y0 as i64, w, h, &pixels, clip);
        }
    }

    /// Draws the password box in the middle of the window.
    pub fn draw_prompt(&mut self, frame: &mut [u32], frame_width: u32, frame_height: u32, p: &Prompt) {
        let s = p.dpi;
        let w = (420.0 * s).min(f64::from(frame_width) - 32.0 * s).max(240.0 * s).round();
        let text = format!("\u{201C}{}\u{201D} is protected. Type its password to open it.", p.name);
        let lines = wrap(&mut self.text, &text, (SMALL_PX * s) as f32, w - 2.0 * PROPS_PAD * s);
        let h = ((150.0 + 17.0 * lines.len() as f64 + if p.wrong { 20.0 } else { 0.0 }) * s).round();
        let mut hits = Vec::new();
        let pixels = self.render(w as u32, h as u32, &p.palette, s, |c| draw_prompt(c, p, &lines, w, h, &mut hits));
        let x = ((f64::from(frame_width) - w) / 2.0).max(0.0).round();
        let y = ((f64::from(frame_height) - h) / 2.0).max(0.0).round();
        for (_, r) in &mut hits {
            *r = *r + kurbo::Vec2::new(x, y);
        }
        self.prompt_hits = hits;
        blit(frame, frame_width, x as u32, y as u32, w as u32, h as u32, &pixels);
    }

    pub fn close_prompt(&mut self) {
        self.prompt_hits.clear();
    }

    pub fn close_props(&mut self) {
        self.props = None;
    }

    /// Draws the settings panel at the right edge, below the toolbar.
    /// Draws the settings window over the rest, which is dimmed.
    pub fn draw_panel(&mut self, frame: &mut [u32], frame_width: u32, p: &Panel) {
        if self.panel.as_ref().is_none_or(|d| d.state != *p) {
            let mut hits = Vec::new();
            let mut content = 0.0;
            let pixels = self.render(p.width, p.height, &p.palette, p.dpi, |c| content = draw_panel(c, p, &mut hits));
            for (_, r) in &mut hits {
                *r = *r + kurbo::Vec2::new(f64::from(p.x), f64::from(p.y));
            }
            self.panel_content = content;
            self.panel_view = f64::from(p.height) - (68.0 * p.dpi).round();
            self.panel = Some(Drawn { state: p.clone(), pixels, hits });
        }
        // everything else, darker, so the window stands out
        for px in frame.iter_mut() {
            let [_, r, g, b] = px.to_be_bytes();
            let dim = |c: u8| (u32::from(c) * 140 / 255) as u8;
            *px = u32::from_be_bytes([0, dim(r), dim(g), dim(b)]);
        }
        let d = self.panel.as_ref().unwrap();
        blit(frame, frame_width, p.x, p.y, p.width, p.height, &d.pixels);
    }

    /// How far the settings window's cards can scroll.
    pub fn panel_scroll_max(&self) -> f64 {
        (self.panel_content - self.panel_view).max(0.0)
    }

    /// Where the settings window is, while it's open.
    pub fn panel_rect(&self) -> Option<Rect> {
        let p = &self.panel.as_ref()?.state;
        Some(Rect::new(f64::from(p.x), f64::from(p.y), f64::from(p.x + p.width), f64::from(p.y + p.height)))
    }

    pub fn close_panel(&mut self) {
        self.panel = None;
    }

    /// Draws the outline sidebar at the left edge, below the toolbar.
    pub fn draw_sidebar(&mut self, frame: &mut [u32], frame_width: u32, s: &Sidebar) {
        let top = s.top;
        if self.sidebar.as_ref().is_none_or(|d| d.state != *s) {
            let mut hits = Vec::new();
            let mut content = 0.0;
            let pixels = self.render(s.width, s.height, &s.palette, s.dpi, |c| content = draw_sidebar(c, s, &mut hits));
            for (_, r) in &mut hits {
                *r = *r + kurbo::Vec2::new(0.0, f64::from(top));
            }
            self.sidebar_content = content;
            self.sidebar = Some(Drawn { state: s.clone(), pixels, hits });
        }
        let d = self.sidebar.as_ref().unwrap();
        blit(frame, frame_width, 0, top, s.width, s.height, &d.pixels);
    }

    /// How far the sidebar can scroll.
    pub fn sidebar_scroll_max(&self, height: u32) -> f64 {
        (self.sidebar_content - f64::from(height)).max(0.0)
    }

    pub fn close_sidebar(&mut self) {
        self.sidebar = None;
    }

    /// Draws `text` in a box below `anchor` (window coordinates), kept inside the window.
    pub fn draw_tooltip(&mut self, frame: &mut [u32], frame_width: u32, frame_height: u32, anchor: Rect, text: &str, dpi: f64, palette: &Palette) {
        let fresh = self.tooltip.as_ref().is_some_and(|t| t.0 == text && t.1 == dpi && t.2 == *palette);
        if !fresh {
            let px = (SMALL_PX * dpi) as f32;
            let w = (f64::from(self.text.measure(text, px)) + 16.0 * dpi).ceil() as u32;
            let h = (26.0 * dpi).round() as u32;
            let pixels = self.render(w, h, palette, dpi, |c| {
                let r = Rect::new(0.0, 0.0, f64::from(w), f64::from(h));
                c.fill(r, c.p.line);
                c.fill(r.inset(-c.line_px()), c.p.panel);
                c.centered(text, SMALL_PX, r, c.p.text);
            });
            self.tooltip = Some((text.to_owned(), dpi, *palette, w, h, pixels));
        }
        let (_, _, _, w, h, pixels) = self.tooltip.as_ref().unwrap();
        let x = (anchor.center().x - f64::from(*w) / 2.0).clamp(4.0, f64::from(frame_width.saturating_sub(*w + 4)));
        let y = anchor.y1 + 6.0 * dpi;
        if y + f64::from(*h) < f64::from(frame_height) {
            blit(frame, frame_width, x as u32, y as u32, *w, *h, pixels);
        }
    }

    /// What's under (x, y), looking at the parts on top first.
    pub fn hit(&self, x: f64, y: f64, shown: Shown) -> Option<Hit> {
        let point = kurbo::Point::new(x, y);
        let inside = |hits: &[(Hit, Rect)]| hits.iter().rev().find(|(_, r)| r.contains(point)).map(|(h, _)| *h);
        let menu = self.menu.as_ref().filter(|_| shown.menu).and_then(|(d, _)| inside(&d.hits));
        let prompt = if shown.prompt { inside(&self.prompt_hits) } else { None };
        let editor = if shown.editor { inside(&self.editor_hits) } else { None };
        editor
            .or(prompt)
            .or(menu)
            .or_else(|| self.props.as_ref().filter(|_| shown.props).and_then(|(d, _)| inside(&d.hits)))
            .or_else(|| self.panel.as_ref().filter(|_| shown.panel).and_then(|d| inside(&d.hits)))
            .or_else(|| self.sidebar.as_ref().filter(|_| shown.sidebar).and_then(|d| inside(&d.hits)))
            .or_else(|| self.tabs.as_ref().filter(|_| shown.tabs).and_then(|d| inside(&d.hits)))
            .or_else(|| self.bar.as_ref().filter(|_| shown.bar).and_then(|d| inside(&d.hits)))
            .or_else(|| self.annot_bar.as_ref().filter(|_| shown.annot_bar).and_then(|d| inside(&d.hits)))
            .or_else(|| if shown.start { inside(&self.start_hits) } else { None })
    }

    pub fn hit_rect(&self, hit: Hit) -> Option<Rect> {
        let bar = self.bar.iter().flat_map(|d| &d.hits);
        let annot_bar = self.annot_bar.iter().flat_map(|d| &d.hits);
        let tabs = self.tabs.iter().flat_map(|d| &d.hits);
        let panel = self.panel.iter().flat_map(|d| &d.hits);
        bar.chain(annot_bar).chain(tabs).chain(panel).chain(&self.start_hits).find(|(h, _)| *h == hit).map(|(_, r)| *r)
    }
}

/// Copies a w × h block of pixels to (x, y) in the frame, keeping to `clip`.
#[allow(clippy::too_many_arguments)]
fn blit_clipped(frame: &mut [u32], frame_width: u32, x: i64, y: i64, w: u32, h: u32, src: &[u32], clip: Rect) {
    let fw = i64::from(frame_width);
    let (x0, x1) = (x.max(clip.x0 as i64).max(0), (x + i64::from(w)).min(clip.x1 as i64).min(fw));
    if x0 >= x1 {
        return;
    }
    for row in 0..i64::from(h) {
        let fy = y + row;
        if fy < clip.y0 as i64 || fy >= clip.y1 as i64 || fy < 0 {
            continue;
        }
        let dst = (fy * fw + x0) as usize;
        let from = (row * i64::from(w) + (x0 - x)) as usize;
        let n = (x1 - x0) as usize;
        if dst + n > frame.len() {
            break;
        }
        frame[dst..dst + n].copy_from_slice(&src[from..from + n]);
    }
}

/// Copies a w × h block of pixels to (x, y) in the frame, clipped to it.
fn blit(frame: &mut [u32], frame_width: u32, x: u32, y: u32, w: u32, h: u32, src: &[u32]) {
    let (fw, x) = (frame_width as usize, x as usize);
    let n = (w as usize).min(fw.saturating_sub(x));
    for (row, line) in src.chunks_exact(w as usize).take(h as usize).enumerate() {
        let start = (y as usize + row) * fw + x;
        if start + n > frame.len() {
            break;
        }
        frame[start..start + n].copy_from_slice(&line[..n]);
    }
}

fn draw_bar(c: &mut Canvas, b: &Bar, hits: &mut Vec<(Hit, Rect)>) {
    let s = c.s;
    let (w, h) = (f64::from(b.width), f64::from(Chrome::bar_height(s)));
    c.fill(Rect::new(0.0, 0.0, w, h), c.p.bar);
    c.fill(Rect::new(0.0, h - c.line_px(), w, h), c.p.line);
    let size = (32.0 * s).round();
    let top = ((h - size) / 2.0).round();
    let gap = (2.0 * s).round();
    let mut x = (6.0 * s).round();
    let next = |x: &mut f64, width: f64| {
        let r = Rect::new(*x, top, *x + width, top + size);
        *x += width + gap;
        r
    };

    let r = next(&mut x, size);
    c.button(hits, Hit::Outline, r, ICON_OUTLINE, "=", b.has_outline, b);
    let r = next(&mut x, size);
    c.button(hits, Hit::Home, r, ICON_HOME, "H", b.has_doc, b);
    let r = next(&mut x, size);
    c.button(hits, Hit::Open, r, ICON_OPEN, "O", true, b);
    c.separator(&mut x, h);
    let r = next(&mut x, size);
    c.button(hits, Hit::Prev, r, ICON_UP, "<", b.has_doc, b);

    // page number box
    let field = next(&mut x, (52.0 * s).round()).inset((0.0, -3.0 * s));
    if b.has_doc {
        c.round(field, 4.0, c.p.field);
        if b.editing {
            c.outline(field, 4.0, 2.0, c.p.accent);
        } else {
            c.outline(field, 4.0, 1.0, if b.hover == Some(Hit::PageBox) { c.p.dim } else { c.p.line });
        }
        c.centered(&b.page, TEXT_PX, field, c.p.text);
        if b.editing {
            // caret after the text
            let tw = c.measure(&b.page, TEXT_PX);
            let cx = (field.center().x + tw / 2.0 + 1.0 * s).round();
            c.fill(Rect::new(cx, field.y0 + 5.0 * s, cx + c.line_px(), field.y1 - 5.0 * s), c.p.text);
        }
        hits.push((Hit::PageBox, field));
        let of = format!("of {}", b.pages);
        let ow = c.measure(&of, TEXT_PX);
        let label = next(&mut x, ow.ceil());
        c.centered(&of, TEXT_PX, label, c.p.dim);
    }
    let r = next(&mut x, size);
    c.button(hits, Hit::Next, r, ICON_DOWN, ">", b.has_doc, b);
    c.separator(&mut x, h);

    let r = next(&mut x, size);
    c.button(hits, Hit::ZoomOut, r, ICON_ZOOM_OUT, "-", b.has_doc, b);
    let label = next(&mut x, (52.0 * s).round());
    if b.has_doc {
        c.centered(&b.zoom, TEXT_PX, label, c.p.text);
    }
    let r = next(&mut x, size);
    c.button(hits, Hit::ZoomIn, r, ICON_ZOOM_IN, "+", b.has_doc, b);
    let r = next(&mut x, size);
    c.button(hits, Hit::FitPage, r, ICON_FIT_PAGE, "P", b.has_doc, b);
    let r = next(&mut x, size);
    c.button(hits, Hit::FitWidth, r, ICON_FIT_WIDTH, "W", b.has_doc, b);
    let r = next(&mut x, size);
    c.button(hits, Hit::View, r, ICON_VIEW, "V", b.has_doc, b);
    if b.has_form {
        let r = next(&mut x, size);
        c.button(hits, Hit::FillForm, r, ICON_EDIT, "F", true, b);
    }
    if b.can_annotate {
        let r = next(&mut x, size);
        c.button(hits, Hit::Annotate, r, ICON_ANNOTATE, "A", true, b);
    }
    let left_end = x;

    let gear_x = w - size - (6.0 * s).round();
    c.button(hits, Hit::Settings, Rect::new(gear_x, top, gear_x + size, top + size), ICON_SETTINGS, "S", true, b);
    let more_x = gear_x - size - gap;
    c.button(hits, Hit::MainMenu, Rect::new(more_x, top, more_x + size, top + size), ICON_MORE, "...", true, b);
    let mut right = more_x;
    if let Some(label) = &b.update {
        let uw = (c.measure(label, TEXT_PX) + 40.0 * s).ceil();
        let r = Rect::new(more_x - gap - uw, top + 2.0 * s, more_x - gap, top + size - 2.0 * s);
        c.round(r, 4.0, if b.hover == Some(Hit::Update) { c.p.text } else { c.p.accent });
        let icon = Rect::new(r.x0 + 6.0 * s, r.y0, r.x0 + 26.0 * s, r.y1);
        c.icon_sized(ICON_UPDATE, "", 13.0, icon, if b.hover == Some(Hit::Update) { c.p.bar } else { c.p.on_accent });
        c.label_middle(label, TEXT_PX, icon.x1 + 2.0 * s, r, if b.hover == Some(Hit::Update) { c.p.bar } else { c.p.on_accent });
        hits.push((Hit::Update, r));
        right = r.x0;
    }
    if b.has_doc {
        let mut sep = right - (11.0 * s).round();
        c.separator(&mut sep, h);
        draw_search(c, b, hits, left_end + (8.0 * s).round(), right - (11.0 * s).round(), top, size);
    }
}

/// The search box, match count and previous/next buttons, laid out right to left between `from`
/// and `to`. When there's no room, the count goes first, then the box shrinks, then the buttons go.
#[allow(clippy::too_many_arguments)]
fn draw_search(c: &mut Canvas, b: &Bar, hits: &mut Vec<(Hit, Rect)>, from: f64, to: f64, top: f64, size: f64) {
    let s = c.s;
    let gap = (2.0 * s).round();
    let avail = to - from;
    if avail < 80.0 * s {
        return;
    }
    let buttons = 2.0 * (size + gap);
    // room for the longest usual status, so the box doesn't shift while typing
    let mut status = if b.search_status.is_empty() {
        0.0
    } else {
        let widest = c.measure("No results", TEXT_PX).max(c.measure(&b.search_status, TEXT_PX));
        widest.ceil() + 10.0 * s
    };
    let mut field_w = (220.0 * s).round();
    let mut show_buttons = true;
    if field_w + status + buttons > avail {
        status = 0.0;
    }
    if field_w + buttons > avail {
        field_w = (avail - buttons).max(120.0 * s);
    }
    if field_w + buttons > avail {
        show_buttons = false;
        field_w = avail;
    }

    let mut x = to;
    if show_buttons {
        x -= size;
        c.button(hits, Hit::SearchNext, Rect::new(x, top, x + size, top + size), ICON_DOWN, ">", b.has_matches, b);
        x -= gap + size;
        c.button(hits, Hit::SearchPrev, Rect::new(x, top, x + size, top + size), ICON_UP, "<", b.has_matches, b);
        x -= gap;
    }
    if status > 0.0 {
        x -= status;
        c.label_middle(&b.search_status, TEXT_PX, x + 4.0 * s, Rect::new(x, top, x + status, top + size), c.p.dim);
    }
    x -= field_w + gap;

    let field = Rect::new(x, top + 3.0 * s, x + field_w, top + size - 3.0 * s);
    c.round(field, 4.0, c.p.field);
    if b.search_focus {
        c.outline(field, 4.0, 2.0, c.p.accent);
    } else {
        c.outline(field, 4.0, 1.0, if b.hover == Some(Hit::SearchBox) { c.p.dim } else { c.p.line });
    }
    let icon = Rect::new(field.x0, field.y0, field.x0 + 28.0 * s, field.y1);
    c.icon(ICON_SEARCH, "", icon, c.p.dim);
    let (text_x, text_end) = (icon.x1, field.x1 - 8.0 * s);
    if b.search.is_empty() && !b.search_focus {
        c.label_middle("Find in document", TEXT_PX, text_x, field, c.p.dim);
    } else {
        // the end of the text, if it's too long to show whole
        let mut shown = b.search.as_str();
        while !shown.is_empty() && c.measure(shown, TEXT_PX) > text_end - text_x {
            let next = shown.char_indices().nth(1).map_or(shown.len(), |(i, _)| i);
            shown = &shown[next..];
        }
        c.label_middle(shown, TEXT_PX, text_x, field, c.p.text);
        if b.search_focus {
            let cx = (text_x + c.measure(shown, TEXT_PX) + 1.0 * s).round();
            c.fill(Rect::new(cx, field.y0 + 6.0 * s, cx + c.line_px(), field.y1 - 6.0 * s), c.p.text);
        }
    }
    hits.push((Hit::SearchBox, field));
}

/// The settings window's categories: the settings table's sections, then System (the default PDF
/// reader, the settings file, resetting, about).
pub fn panel_sections() -> Vec<&'static str> {
    let mut sections: Vec<&'static str> = Vec::new();
    for f in FIELDS {
        if !sections.contains(&f.section) {
            sections.push(f.section);
        }
    }
    // last, with what isn't a setting
    sections.retain(|s| *s != "System");
    sections.push("System");
    sections
}

fn section_icon(name: &str) -> char {
    match name {
        "Appearance" => '\u{E790}',
        "Reading" => '\u{E736}',
        "Files" => '\u{E8B7}',
        "Memory and speed" => '\u{EC4A}',
        _ => '\u{E770}',
    }
}

/// The settings window: its categories down the left, and the chosen one's settings on the right,
/// each a card with its name, what it does, and its control. Returns the height of the cards.
fn draw_panel(c: &mut Canvas, p: &Panel, hits: &mut Vec<(Hit, Rect)>) -> f64 {
    let s = c.s;
    let (w, h) = (f64::from(p.width), f64::from(p.height));
    let all = Rect::new(0.0, 0.0, w, h);
    let nav_w = (220.0 * s).round().min((w * 0.4).round());
    let header = (68.0 * s).round();
    let pad = (24.0 * s).round();
    let sections = panel_sections();
    let section = sections.get(p.tab).copied().unwrap_or(sections[0]);
    c.fill(all, c.p.panel);
    hits.push((Hit::Panel, all));

    // the cards, scrolled; the header and the categories are drawn over them
    let (x0, x1) = (nav_w + pad, w - pad);
    let view = Rect::new(nav_w, header, w, h);
    let mut y = header - p.scroll;
    for (i, _) in FIELDS.iter().enumerate().filter(|(_, f)| f.section == section) {
        y = setting_card(c, p, i, x0, x1, y, view, hits);
    }
    if section == "System" {
        y = system_cards(c, p, x0, x1, y, view, hits);
    }
    let content = y + p.scroll - header + pad;

    c.fill(Rect::new(nav_w, 0.0, w, header), c.p.panel);
    c.label(section, 20.0, x0, (22.0 * s).round(), c.p.text, true);
    let close = Rect::new(w - pad - 28.0 * s, 18.0 * s, w - pad + 4.0 * s, 50.0 * s);
    if p.hover == Some(Hit::Close) {
        c.round(close, 4.0, c.p.hover);
    }
    c.icon(ICON_CLOSE, "x", close, c.p.text);
    hits.push((Hit::Panel, Rect::new(nav_w, 0.0, w, header)));
    hits.push((Hit::Close, close));

    c.fill(Rect::new(0.0, 0.0, nav_w, h), c.p.bar);
    c.fill(Rect::new(nav_w - c.line_px(), 0.0, nav_w, h), c.p.line);
    hits.push((Hit::Panel, Rect::new(0.0, 0.0, nav_w, h)));
    c.label("Settings", 20.0, (20.0 * s).round(), (22.0 * s).round(), c.p.text, true);
    let mut ny = header;
    for (k, name) in sections.iter().enumerate() {
        let r = Rect::new(8.0 * s, ny, nav_w - 8.0 * s, ny + (38.0 * s).round());
        let hit = Hit::SettingsTab(k);
        if k == p.tab {
            c.round(r, 4.0, c.p.press);
            c.round(Rect::new(r.x0, r.y0 + 10.0 * s, r.x0 + 3.0 * s, r.y1 - 10.0 * s), 1.5, c.p.accent);
        } else if p.hover == Some(hit) {
            c.round(r, 4.0, c.p.hover);
        }
        c.icon(section_icon(name), "", Rect::new(r.x0 + 8.0 * s, r.y0, r.x0 + 40.0 * s, r.y1), c.p.text);
        c.label_middle(name, TEXT_PX, r.x0 + 46.0 * s, r, c.p.text);
        hits.push((hit, r));
        ny = r.y1 + (4.0 * s).round();
    }
    c.outline(all, 0.0, 1.0, c.p.line);
    content
}

/// A card's frame, its name and what it does (wrapped to leave `control` pixels at the right), at
/// `y`. Returns the card; `hit` is where clicking it goes, if anywhere, kept to the part in view.
#[allow(clippy::too_many_arguments)]
fn card(c: &mut Canvas, p: &Panel, label: &str, help: &str, control: f64, x0: f64, x1: f64, y: f64, view: Rect, hit: Option<Hit>, hits: &mut Vec<(Hit, Rect)>) -> Rect {
    let s = c.s;
    let inner = (16.0 * s).round();
    let lines = if help.is_empty() { Vec::new() } else { c.wrap(help, SMALL_PX, x1 - x0 - 3.0 * inner - control) };
    let line_h = (17.0 * s).round();
    let text_h = (20.0 * s).round() + lines.len() as f64 * line_h;
    let r = Rect::new(x0, y, x1, y + (text_h + 2.0 * inner).max(64.0 * s).round());
    let hot = hit.is_some_and(|h| p.hover == Some(h));
    c.round(r, 6.0, if hot { c.p.hover } else { c.p.field });
    c.outline(r, 6.0, 1.0, c.p.line);
    let top = (r.center().y - text_h / 2.0).round();
    c.label(label, TEXT_PX, x0 + inner, top, c.p.text, false);
    for (k, line) in lines.iter().enumerate() {
        c.label(line, SMALL_PX, x0 + inner, top + (20.0 * s).round() + k as f64 * line_h, c.p.dim, false);
    }
    if let Some(hit) = hit {
        let shown = r.intersect(view);
        if shown.area() > 0.0 {
            hits.push((hit, shown));
        }
    }
    r
}

/// Setting `i` as a card: a switch, a number with − and +, or a drop-down of its choices.
/// Returns where the next card goes.
#[allow(clippy::too_many_arguments)]
fn setting_card(c: &mut Canvas, p: &Panel, i: usize, x0: f64, x1: f64, y: f64, view: Rect, hits: &mut Vec<(Hit, Rect)>) -> f64 {
    let s = c.s;
    let f = &FIELDS[i];
    let v = (f.get)(&p.settings);
    let control = match &f.kind {
        Kind::Toggle => 84.0 * s,
        Kind::Number { .. } => 132.0 * s,
        Kind::Choice(options) => options.iter().map(|(_, n)| c.measure(n, TEXT_PX)).fold(0.0, f64::max) + 52.0 * s,
    }
    .ceil();
    let hit = match f.kind {
        Kind::Toggle => Some(Hit::Toggle(i)),
        Kind::Choice(_) => Some(Hit::Dropdown(i)),
        Kind::Number { .. } => None,
    };
    let r = card(c, p, f.label, f.help, control, x0, x1, y, view, hit, hits);
    let (right, mid) = (x1 - (16.0 * s).round(), r.center().y);
    match &f.kind {
        Kind::Toggle => {
            let on = v == Value::Toggle(true);
            let track = Rect::new(right - 40.0 * s, mid - 10.0 * s, right, mid + 10.0 * s);
            let knob_x = if on { track.x1 - 10.0 * s } else { track.x0 + 10.0 * s };
            if on {
                c.round(track, 10.0, c.p.accent);
                c.ctx.set_paint(rgb(c.p.on_accent));
            } else {
                c.outline(track, 10.0, 1.0, c.p.dim);
                c.ctx.set_paint(rgb(c.p.dim));
            }
            c.ctx.fill_path(&Circle::new((knob_x, mid), 6.0 * s).to_path(0.1));
            let state = if on { "On" } else { "Off" };
            let sw = c.measure(state, TEXT_PX);
            c.label_middle(state, TEXT_PX, track.x0 - 10.0 * s - sw, r, c.p.text);
        }
        Kind::Number { min, max, .. } => {
            let n = match v {
                Value::Number(n) => n,
                _ => 0,
            };
            let bw = (30.0 * s).round();
            let up = Rect::new(right - bw, mid - bw / 2.0, right, mid + bw / 2.0);
            let value = Rect::new(up.x0 - (72.0 * s).round(), up.y0, up.x0, up.y1);
            let down = Rect::new(value.x0 - bw, up.y0, value.x0, up.y1);
            for (b, dir, enabled, glyph) in [(down, -1, n > *min, "−"), (up, 1, n < *max, "+")] {
                let hit = Hit::Step(i, dir);
                c.round(b, 4.0, if enabled && p.hover == Some(hit) { c.p.hover } else { c.p.panel });
                c.outline(b, 4.0, 1.0, c.p.line);
                c.centered(glyph, 15.0, b, if enabled { c.p.text } else { c.p.line });
                if enabled && b.intersect(view).area() > 0.0 {
                    hits.push((hit, b.intersect(view)));
                }
            }
            c.centered(&f.display(v), TEXT_PX, value, c.p.text);
        }
        Kind::Choice(options) => {
            let k = match v {
                Value::Choice(k) => k,
                _ => 0,
            };
            let b = Rect::new(right - control, mid - 16.0 * s, right, mid + 16.0 * s);
            c.round(b, 4.0, c.p.panel);
            c.outline(b, 4.0, 1.0, c.p.line);
            c.label_middle(options.get(k).map_or("", |o| o.1), TEXT_PX, b.x0 + 12.0 * s, b, c.p.text);
            c.icon_sized(ICON_DOWN, "v", 10.0, Rect::new(b.x1 - 30.0 * s, b.y0, b.x1 - 6.0 * s, b.y1), c.p.dim);
        }
    }
    r.y1 + (6.0 * s).round()
}

/// The System category: the default PDF reader, the settings file, resetting, and about.
#[allow(clippy::too_many_arguments)]
fn system_cards(c: &mut Canvas, p: &Panel, x0: f64, x1: f64, mut y: f64, view: Rect, hits: &mut Vec<(Hit, Rect)>) -> f64 {
    let s = c.s;
    let about = format!("Version {}, by aga2522. What's inside, with its licenses.", env!("CARGO_PKG_VERSION"));
    let default_help = if p.is_default {
        "Double-clicking a PDF opens it in Flores PDF."
    } else {
        "PDFs open in another program now. This adds Flores PDF to Windows' Default apps and opens that page, where you pick it for PDFs."
    };
    let reset = "Puts every setting back as it first was. Recent files, bookmarks and highlights stay.";
    let rows: [(&str, &str, Option<(&str, Hit)>); 4] = [
        ("Default PDF reader", default_help, (!p.is_default).then_some(("Make default…", Hit::MakeDefault))),
        ("Settings file", &p.file, Some(("Open", Hit::OpenSettingsFile))),
        ("Reset settings", reset, Some(("Reset", Hit::Reset))),
        ("About Flores PDF", &about, Some(("About", Hit::AboutBox))),
    ];
    for (label, help, button) in rows {
        let bw = button.map_or(90.0 * s, |(t, _)| c.measure(t, TEXT_PX) + 32.0 * s).ceil();
        let r = card(c, p, label, help, bw, x0, x1, y, view, None, hits);
        let (right, mid) = (x1 - (16.0 * s).round(), r.center().y);
        match button {
            Some((text, hit)) => {
                let b = Rect::new(right - bw, mid - 16.0 * s, right, mid + 16.0 * s);
                c.round(b, 4.0, if p.hover == Some(hit) { c.p.hover } else { c.p.panel });
                c.outline(b, 4.0, 1.0, c.p.line);
                c.centered(text, TEXT_PX, b, c.p.text);
                if b.intersect(view).area() > 0.0 {
                    hits.push((hit, b.intersect(view)));
                }
            }
            None => {
                let mark = Rect::new(right - 24.0 * s, mid - 12.0 * s, right, mid + 12.0 * s);
                c.icon(ICON_CHECK, "*", mark, c.p.accent);
            }
        }
        y = r.y1 + (6.0 * s).round();
    }
    y
}

/// Draws the outline sidebar and returns the height of its content.
fn draw_sidebar(c: &mut Canvas, sb: &Sidebar, hits: &mut Vec<(Hit, Rect)>) -> f64 {
    let s = c.s;
    let (w, h) = (f64::from(sb.width), f64::from(sb.height));
    c.fill(Rect::new(0.0, 0.0, w, h), c.p.panel);
    c.fill(Rect::new(w - c.line_px(), 0.0, w, h), c.p.line);
    hits.push((Hit::Sidebar, Rect::new(0.0, 0.0, w, h)));
    let pad = (12.0 * s).round();
    let row_h = (ROW * s).round();
    let header = (40.0 * s).round();
    // the headings, as tabs when there's an outline to switch to
    let top = (12.0 * s).round() - sb.scroll;
    let mut x = pad;
    let headings: &[(&str, bool)] = if sb.has_outline { &[("Outline", false), ("Bookmarks", true)] } else { &[("Bookmarks", true)] };
    for &(name, favorites) in headings {
        let shown = favorites == sb.showing_favorites;
        let tw = c.measure(name, 14.0).ceil();
        c.label(name, 14.0, x, top, if shown { c.p.text } else { c.p.dim }, true);
        if shown && sb.has_outline {
            c.fill(Rect::new(x, top + 22.0 * s, x + tw, top + 22.0 * s + (2.0 * s).round()), c.p.accent);
        }
        hits.push((Hit::SideTab(favorites), Rect::new(x - 4.0 * s, top - 4.0 * s, x + tw + 4.0 * s, top + 26.0 * s)));
        x += tw + (20.0 * s).round();
    }
    if sb.showing_favorites {
        return draw_favorites(c, sb, header, hits);
    }

    for (i, row) in sb.rows.iter().enumerate() {
        let y = header + row_h * i as f64 - sb.scroll;
        if y + row_h < 0.0 || y > h {
            continue;
        }
        let r = Rect::new(0.0, y, w - c.line_px(), y + row_h);
        let current = sb.current == Some(row.node);
        if current {
            c.fill(r, c.p.press);
            c.fill(Rect::new(0.0, y + 4.0 * s, (3.0 * s).round(), y + row_h - 4.0 * s), c.p.accent);
        } else if sb.hover == Some(Hit::OutlineRow(row.node)) || sb.hover == Some(Hit::OutlineToggle(row.node)) {
            c.fill(r, c.p.hover);
        }
        hits.push((Hit::OutlineRow(row.node), r));

        let x = pad + INDENT * s * row.depth as f64;
        let arrow = Rect::new(x, y, x + 18.0 * s, y + row_h);
        if row.children {
            let icon = if row.open { ICON_DOWN } else { ICON_RIGHT };
            c.icon_sized(icon, if row.open { "v" } else { ">" }, 10.0, arrow, c.p.dim);
            hits.push((Hit::OutlineToggle(row.node), arrow));
        }
        let title = c.fit(&row.title, TEXT_PX, w - pad - arrow.x1 - 2.0 * s);
        c.label_middle(&title, TEXT_PX, arrow.x1 + 2.0 * s, r, c.p.text);
    }
    header + row_h * sb.rows.len() as f64 + pad
}

fn draw_tabs(c: &mut Canvas, t: &Tabs, hits: &mut Vec<(Hit, Rect)>) {
    let s = c.s;
    let (w, h) = (f64::from(t.width), f64::from(Chrome::tabs_height(s)));
    c.fill(Rect::new(0.0, 0.0, w, h), c.p.strip);
    let top = (6.0 * s).round();
    let edge = (6.0 * s).round();
    let plus = (32.0 * s).round();
    // tabs share the width, 48 to 220 logical pixels each; ones that don't fit aren't drawn, and
    // Ctrl+Tab still reaches them
    let tab_w = ((w - 2.0 * edge - plus) / t.names.len().max(1) as f64).clamp(48.0 * s, 220.0 * s).floor();
    let mut x = edge;
    for (i, name) in t.names.iter().enumerate() {
        if x + tab_w > w - edge - plus {
            break;
        }
        let r = Rect::new(x, top, x + tab_w, h);
        let active = i == t.active;
        let hovered = matches!(t.hover, Some(Hit::Tab(j) | Hit::TabClose(j)) if j == i);
        if active || hovered {
            // the bottom corners fall below the strip, so only the top ones are round
            let color = if active { c.p.bar } else { c.p.tab_hover };
            c.round(Rect::new(r.x0, r.y0, r.x1, r.y1 + 8.0 * s), 6.0, color);
        } else if i + 1 != t.active && i + 1 < t.names.len() {
            let x1 = (r.x1 - c.line_px()).round();
            c.fill(Rect::new(x1, r.y0 + 8.0 * s, x1 + c.line_px(), r.y1 - 8.0 * s), c.p.line);
        }
        hits.push((Hit::Tab(i), r));
        let size = (24.0 * s).round();
        let mid = r.center().y;
        let close = Rect::new(r.x1 - size - 6.0 * s, mid - size / 2.0, r.x1 - 6.0 * s, mid + size / 2.0);
        let title = c.fit(name, TEXT_PX, close.x0 - r.x0 - 14.0 * s);
        c.label_middle(&title, TEXT_PX, r.x0 + 10.0 * s, r, if active { c.p.text } else { c.p.dim });
        if active || hovered {
            if t.hover == Some(Hit::TabClose(i)) {
                c.round(close, 4.0, c.p.hover);
            }
            c.icon_sized(ICON_CLOSE, "x", 10.0, close, c.p.text);
            hits.push((Hit::TabClose(i), close));
        }
        x += tab_w;
    }
    let r = Rect::new(x + 4.0 * s, top + 1.0 * s, x + plus, h - 3.0 * s);
    if t.hover == Some(Hit::NewTab) {
        c.round(r, 4.0, c.p.tab_hover);
    }
    c.icon_sized(ICON_ADD, "+", 12.0, r, c.p.text);
    hits.push((Hit::NewTab, r));
}

/// The start page: a heading, an open button, and a card for each recent file that fits, with its
/// first page, name and the page it was left at. Returns where each card's thumbnail goes.
fn draw_start(c: &mut Canvas, st: &Start, width: u32, height: u32, hits: &mut Vec<(Hit, Rect)>) -> Vec<(usize, Rect)> {
    let s = c.s;
    let (w, h) = (f64::from(width), f64::from(height));
    c.fill(Rect::new(0.0, 0.0, w, h), c.p.canvas);
    c.label("Recent files", 20.0, 8.0 * s, 0.0, c.p.text, true);
    let open = Rect::new(w - c.measure("Open a file", TEXT_PX).ceil() - 32.0 * s, 0.0, w - 8.0 * s, (32.0 * s).round());
    c.round(open, 4.0, if st.hover == Some(Hit::Open) { c.p.hover } else { c.p.field });
    c.outline(open, 4.0, 1.0, c.p.line);
    c.centered("Open a file", TEXT_PX, open, c.p.text);
    hits.push((Hit::Open, open));

    let (card_w, thumb_w, thumb_h) = ((CARD_W * s).round(), (THUMB_W * s).round(), (THUMB_H * s).round());
    let card_h = (thumb_h + (12.0 + CARD_TEXT) * s).round();
    let per_row = ((w / card_w).floor() as usize).max(1);
    let top = (52.0 * s).round();
    let mut boxes = Vec::new();
    for (i, f) in st.recent.iter().enumerate() {
        let (col, row) = (i % per_row, i / per_row);
        let (x, y) = (col as f64 * card_w, top + row as f64 * card_h);
        if y + card_h > h {
            break;
        }
        let card = Rect::new(x + 2.0 * s, y, x + card_w - 2.0 * s, y + card_h - 4.0 * s);
        let hovered = matches!(st.hover, Some(Hit::Recent(j) | Hit::Forget(j)) if j == i);
        if hovered {
            c.round(card, 6.0, c.p.hover);
        }
        hits.push((Hit::Recent(i), card));
        let thumb = Rect::new(card.center().x - thumb_w / 2.0, y + 8.0 * s, card.center().x + thumb_w / 2.0, y + 8.0 * s + thumb_h);
        match &f.thumb {
            // a frame around the page itself, which is drawn in afterwards
            Some(t) => {
                let (tw, th) = (f64::from(t.width), f64::from(t.height));
                let page = Rect::new(thumb.center().x - tw / 2.0, thumb.center().y - th / 2.0, thumb.center().x + tw / 2.0, thumb.center().y + th / 2.0);
                let page = Rect::new(page.x0.round(), page.y0.round(), page.x0.round() + tw, page.y0.round() + th);
                c.fill(page.inflate(c.line_px(), c.line_px()), c.p.page_border);
            }
            None => {
                c.fill(thumb, c.p.field);
                c.outline(thumb, 0.0, 1.0, c.p.line);
                c.icon_sized(ICON_DOC, "", 28.0, thumb, c.p.line);
            }
        }
        boxes.push((i, thumb));
        let text_x = card.x0 + 8.0 * s;
        let room = card.width() - 16.0 * s;
        let name = c.fit(&f.name, TEXT_PX, room);
        c.label(&name, TEXT_PX, text_x, thumb.y1 + 8.0 * s, c.p.text, false);
        let page = c.fit(&format!("Page {} · {}", f.page, f.folder), SMALL_PX, room);
        c.label(&page, SMALL_PX, text_x, thumb.y1 + 27.0 * s, c.p.dim, false);
        if hovered {
            let size = (26.0 * s).round();
            let forget = Rect::new(card.x1 - size - 4.0 * s, card.y0 + 4.0 * s, card.x1 - 4.0 * s, card.y0 + 4.0 * s + size);
            c.round(forget, 4.0, if st.hover == Some(Hit::Forget(i)) { c.p.press } else { c.p.panel });
            c.icon_sized(ICON_CLOSE, "x", 10.0, forget, c.p.text);
            hits.push((Hit::Forget(i), forget));
        }
    }
    boxes
}

/// The annotation tools, colors and line widths, then undo and save, with a hint at the right.
fn draw_annot_bar(c: &mut Canvas, b: &AnnotBar, hits: &mut Vec<(Hit, Rect)>) {
    let s = c.s;
    let (w, h) = (f64::from(b.width), f64::from(Chrome::bar_height(s)));
    c.fill(Rect::new(0.0, 0.0, w, h), c.p.bar);
    c.fill(Rect::new(0.0, h - c.line_px(), w, h), c.p.line);
    let size = (32.0 * s).round();
    let top = ((h - size) / 2.0).round();
    let gap = (2.0 * s).round();
    let mut x = (6.0 * s).round();
    let mut square = |c: &mut Canvas, x: &mut f64, hit: Hit, on: bool| {
        let r = Rect::new(*x, top, *x + size, top + size);
        *x += size + gap;
        if on {
            c.round(r, 4.0, c.p.press);
        } else if b.hover == Some(hit) {
            c.round(r, 4.0, c.p.hover);
        }
        hits.push((hit, r));
        r
    };
    for (i, (icon, letter)) in TOOL_ICONS.iter().enumerate() {
        let on = b.tool == Some(i);
        let r = square(c, &mut x, Hit::Tool(i), on);
        c.icon(*icon, letter, r, if on { c.p.accent } else { c.p.text });
        if i == 2 || i == 5 || i == 9 {
            c.separator(&mut x, h);
        }
    }
    c.separator(&mut x, h);
    for (i, color) in b.colors.iter().enumerate() {
        let r = square(c, &mut x, Hit::Color(i), false);
        let dot = Rect::from_center_size(r.center(), (16.0 * s, 16.0 * s));
        if i == b.color {
            c.ctx.set_paint(rgb(c.p.accent));
            c.ctx.fill_path(&Circle::new(dot.center(), 11.0 * s).to_path(0.1));
            c.ctx.set_paint(rgb(c.p.bar));
            c.ctx.fill_path(&Circle::new(dot.center(), 9.5 * s).to_path(0.1));
        }
        c.ctx.set_paint(rgb(c.p.line));
        c.ctx.fill_path(&Circle::new(dot.center(), 8.0 * s).to_path(0.1));
        c.ctx.set_paint(rgb(*color));
        c.ctx.fill_path(&Circle::new(dot.center(), 7.0 * s).to_path(0.1));
    }
    c.separator(&mut x, h);
    for i in 0..3 {
        let on = b.width_choice == i;
        let r = square(c, &mut x, Hit::Width(i), on);
        let line = [1.5, 3.0, 5.5][i] * s;
        let bar = Rect::from_center_size(r.center(), (16.0 * s, line));
        c.round(bar, 1.0, if on { c.p.accent } else { c.p.text });
    }
    c.separator(&mut x, h);
    let r = Rect::new(x, top, x + size, top + size);
    c.button_plain(hits, Hit::Undo, r, ICON_UNDO, "<", b.can_undo, b.hover);
    x += size + gap;
    let label = "Save";
    let sw = (c.measure(label, TEXT_PX) + 40.0 * s).ceil();
    let save = Rect::new(x + 4.0 * s, top, x + 4.0 * s + sw, top + size);
    if b.unsaved {
        c.round(save, 4.0, if b.hover == Some(Hit::Save) { c.p.text } else { c.p.accent });
        let icon = Rect::new(save.x0 + 4.0 * s, save.y0, save.x0 + 24.0 * s, save.y1);
        c.icon_sized(ICON_SAVE, "", 13.0, icon, c.p.on_accent);
        c.label_middle(label, TEXT_PX, icon.x1 + 2.0 * s, save, c.p.on_accent);
        hits.push((Hit::Save, save));
    } else {
        c.outline(save, 4.0, 1.0, c.p.line);
        let icon = Rect::new(save.x0 + 4.0 * s, save.y0, save.x0 + 24.0 * s, save.y1);
        c.icon_sized(ICON_SAVE, "", 13.0, icon, c.p.line);
        c.label_middle(label, TEXT_PX, icon.x1 + 2.0 * s, save, c.p.line);
    }
    let hint = match b.tool.and_then(|i| Tool::ALL.get(i)) {
        Some(t) if t.marks_text() => "Drag across text to mark it",
        Some(Tool::Note) => "Click where the note goes",
        Some(Tool::Text) => "Click where the text goes",
        Some(Tool::Eraser) => "Click an annotation to remove it",
        Some(_) => "Drag on the page to draw",
        None => "Pick a tool; right-click an annotation to change or remove it",
    };
    let room = w - save.x1 - 24.0 * s;
    if room > 60.0 * s {
        let hint = c.fit(hint, SMALL_PX, room);
        let hw = c.measure(&hint, SMALL_PX);
        c.label_middle(&hint, SMALL_PX, w - 12.0 * s - hw, Rect::new(0.0, 0.0, w, h), c.p.dim);
    }
}

/// The text box: a heading, the text typed so far with a caret after it, and the buttons.
fn draw_editor(c: &mut Canvas, e: &Editor, w: f64, h: f64, hits: &mut Vec<(Hit, Rect)>) {
    let s = c.s;
    let all = Rect::new(0.0, 0.0, w, h);
    c.fill(all, c.p.line);
    c.fill(all.inset(-c.line_px()), c.p.panel);
    hits.push((Hit::Editor, all));
    let pad = (PROPS_PAD * s).round();
    c.label(&e.title, 16.0, pad, (16.0 * s).round(), c.p.text, true);
    let by = (h - pad - 32.0 * s).round();
    let field = Rect::new(pad, (50.0 * s).round(), w - pad, by - (12.0 * s).round());
    c.round(field, 4.0, c.p.field);
    c.outline(field, 4.0, 2.0, c.p.accent);
    let line_h = (18.0 * s).round();
    // the text wrapped to the box, its last lines if it's too long
    let mut lines: Vec<String> = Vec::new();
    for para in e.text.split('\n') {
        let wrapped = c.wrap(para, TEXT_PX, field.width() - 20.0 * s);
        if wrapped.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(wrapped);
        }
    }
    let fits = ((field.height() - 12.0 * s) / line_h).floor().max(1.0) as usize;
    let first = lines.len().saturating_sub(fits);
    let mut y = field.y0 + 6.0 * s;
    for line in &lines[first..] {
        let x = field.x0 + 10.0 * s;
        if e.selected && !line.is_empty() {
            let w = c.measure(line, TEXT_PX);
            c.fill(Rect::new(x - 1.0 * s, y, (x + w + 1.0 * s).ceil(), y + line_h), c.p.accent);
            c.label(line, TEXT_PX, x, y, c.p.on_accent, false);
        } else {
            c.label(line, TEXT_PX, x, y, c.p.text, false);
        }
        y += line_h;
    }
    if !e.selected {
        let last = lines.last().map_or("", String::as_str);
        let cx = (field.x0 + 10.0 * s + c.measure(last, TEXT_PX) + 1.0 * s).round();
        let cy = y - line_h;
        c.fill(Rect::new(cx, cy + 2.0 * s, cx + c.line_px(), cy + line_h - 1.0 * s), c.p.text);
    }
    let mut x = w - pad;
    let mut buttons = vec![(Hit::EditorOk, "OK", true), (Hit::EditorCancel, "Cancel", false)];
    if e.can_delete {
        buttons.push((Hit::EditorDelete, "Delete", false));
    }
    for (hit, label, primary) in buttons {
        let bw = c.measure(label, TEXT_PX).ceil() + 32.0 * s;
        let r = Rect::new(x - bw, by, x, by + (32.0 * s).round());
        if primary {
            c.round(r, 4.0, c.p.accent);
        } else {
            c.round(r, 4.0, if e.hover == Some(hit) { c.p.hover } else { c.p.field });
            c.outline(r, 4.0, 1.0, c.p.line);
        }
        c.centered(label, TEXT_PX, r, if primary { c.p.on_accent } else { c.p.text });
        hits.push((hit, r));
        x = r.x0 - (8.0 * s).round();
    }
    if e.multiline {
        c.label("Enter starts a new line; Ctrl+Enter is OK", SMALL_PX, pad, by + 8.0 * s, c.p.dim, false);
    }
}

fn draw_menu(c: &mut Canvas, m: &Menu, width: u32, height: u32, hits: &mut Vec<(Hit, Rect)>) {
    let s = c.s;
    let all = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
    c.fill(all, c.p.line);
    c.fill(all.inset(-c.line_px()), c.p.panel);
    hits.push((Hit::Menu, all));
    let pad = (MENU_PAD * s).round();
    let row_h = (MENU_ROW * s).round();
    // room on the left for check marks, if any entry can have one
    let gutter = if has_checks(m) { CHECK_GUTTER * s } else { 0.0 };
    let mut y = pad;
    for (i, e) in m.entries.iter().enumerate() {
        match e {
            MenuEntry::Choice(label, on) => {
                let r = Rect::new(pad, y, all.x1 - pad, y + row_h);
                if m.hover == Some(Hit::MenuItem(i)) {
                    c.round(r, 4.0, c.p.hover);
                }
                if *on {
                    let mark = Rect::new(r.x0 + 4.0 * s, r.y0, r.x0 + 4.0 * s + gutter, r.y1);
                    c.icon_sized(ICON_CHECK, "*", 12.0, mark, c.p.accent);
                }
                c.label_middle(label, TEXT_PX, r.x0 + 12.0 * s + gutter, r, c.p.text);
                hits.push((Hit::MenuItem(i), r));
                y += row_h;
            }
            MenuEntry::Item(label, keys) | MenuEntry::Check(label, keys, _) => {
                let r = Rect::new(pad, y, all.x1 - pad, y + row_h);
                if m.hover == Some(Hit::MenuItem(i)) {
                    c.round(r, 4.0, c.p.hover);
                }
                if let MenuEntry::Check(_, _, true) = e {
                    let mark = Rect::new(r.x0 + 4.0 * s, r.y0, r.x0 + 4.0 * s + gutter, r.y1);
                    c.icon_sized(ICON_CHECK, "*", 12.0, mark, c.p.accent);
                }
                c.label_middle(label, TEXT_PX, r.x0 + 12.0 * s + gutter, r, c.p.text);
                let keys_w = c.measure(keys, TEXT_PX);
                c.label_middle(keys, TEXT_PX, r.x1 - 12.0 * s - keys_w, r, c.p.dim);
                hits.push((Hit::MenuItem(i), r));
                y += row_h;
            }
            MenuEntry::Separator => {
                let line = (y + 4.0 * s).round();
                c.fill(Rect::new(pad, line, all.x1 - pad, line + c.line_px()), c.p.line);
                y += (9.0 * s).round();
            }
        }
    }
}

/// The properties box: a heading, a close button, and the rows with their values in `lines`.
fn draw_props(c: &mut Canvas, p: &Props, lines: &[Vec<String>], w: f64, h: f64, hits: &mut Vec<(Hit, Rect)>) {
    let s = c.s;
    let all = Rect::new(0.0, 0.0, w, h);
    c.fill(all, c.p.line);
    c.fill(all.inset(-c.line_px()), c.p.panel);
    hits.push((Hit::Props, all));
    let pad = (PROPS_PAD * s).round();
    c.label(p.title, 16.0, pad, (16.0 * s).round(), c.p.text, true);
    let close = Rect::new(w - pad - 24.0 * s, 12.0 * s, w - pad + 8.0 * s, 44.0 * s);
    if p.hover == Some(Hit::PropsClose) {
        c.round(close, 4.0, c.p.hover);
    }
    c.icon(ICON_CLOSE, "x", close, c.p.text);
    hits.push((Hit::PropsClose, close));
    let line_h = (18.0 * s).round();
    let value_x = pad + PROPS_LABEL * s;
    let mut y = (56.0 * s).round();
    for ((label, _), value) in p.rows.iter().zip(lines) {
        c.label(label, TEXT_PX, pad, y, c.p.dim, false);
        for (i, line) in value.iter().enumerate() {
            c.label(line, TEXT_PX, value_x, y + i as f64 * line_h, c.p.text, false);
        }
        y += value.len().max(1) as f64 * line_h + (8.0 * s).round();
    }
}

/// The password box: a heading, which file, the field with a dot per character, and its buttons.
fn draw_prompt(c: &mut Canvas, p: &Prompt, lines: &[String], w: f64, h: f64, hits: &mut Vec<(Hit, Rect)>) {
    let s = c.s;
    let all = Rect::new(0.0, 0.0, w, h);
    c.fill(all, c.p.line);
    c.fill(all.inset(-c.line_px()), c.p.panel);
    hits.push((Hit::Prompt, all));
    let pad = (PROPS_PAD * s).round();
    c.label("Password needed", 16.0, pad, (16.0 * s).round(), c.p.text, true);
    let mut y = (50.0 * s).round();
    for line in lines {
        c.label(line, SMALL_PX, pad, y, c.p.dim, false);
        y += (17.0 * s).round();
    }
    if p.wrong {
        c.label("That password isn't right. Try again.", SMALL_PX, pad, y + 2.0 * s, c.p.accent, false);
        y += (20.0 * s).round();
    }
    y += (8.0 * s).round();
    let field = Rect::new(pad, y, w - pad, y + (32.0 * s).round());
    c.round(field, 4.0, c.p.field);
    c.outline(field, 4.0, 2.0, c.p.accent);
    let dots = "\u{25CF}".repeat(p.typed.min(40));
    c.label_middle(&dots, SMALL_PX, field.x0 + 10.0 * s, field, c.p.text);
    let cx = (field.x0 + 10.0 * s + c.measure(&dots, SMALL_PX) + 1.0 * s).round();
    c.fill(Rect::new(cx, field.y0 + 7.0 * s, cx + c.line_px(), field.y1 - 7.0 * s), c.p.text);
    let by = (h - pad - 32.0 * s).round();
    let mut x = w - pad;
    for (hit, label, primary) in [(Hit::PromptOk, "Open", true), (Hit::PromptCancel, "Cancel", false)] {
        let bw = c.measure(label, TEXT_PX).ceil() + 32.0 * s;
        let r = Rect::new(x - bw, by, x, by + (32.0 * s).round());
        if primary {
            c.round(r, 4.0, c.p.accent);
        } else {
            c.round(r, 4.0, if p.hover == Some(hit) { c.p.hover } else { c.p.field });
            c.outline(r, 4.0, 1.0, c.p.line);
        }
        c.centered(label, TEXT_PX, r, if primary { c.p.on_accent } else { c.p.text });
        hits.push((hit, r));
        x = r.x0 - (8.0 * s).round();
    }
}

/// The sidebar's bookmarks: name and page, a button to remove each, and a hint when there are none.
fn draw_favorites(c: &mut Canvas, sb: &Sidebar, header: f64, hits: &mut Vec<(Hit, Rect)>) -> f64 {
    let s = c.s;
    let w = f64::from(sb.width);
    let (pad, row_h) = ((12.0 * s).round(), (ROW * s).round());
    if sb.favorites.is_empty() {
        let mut y = header + 4.0 * s - sb.scroll;
        for line in c.wrap("No bookmarks in this document. Ctrl+D adds the page you're on; right-click one to rename it.", SMALL_PX, w - 2.0 * pad) {
            c.label(&line, SMALL_PX, pad, y, c.p.dim, false);
            y += (17.0 * s).round();
        }
        return y + sb.scroll + pad;
    }
    for (i, (name, page)) in sb.favorites.iter().enumerate() {
        let y = header + row_h * i as f64 - sb.scroll;
        let r = Rect::new(0.0, y, w - c.line_px(), y + row_h);
        let hovered = matches!(sb.hover, Some(Hit::Favorite(j) | Hit::Unfavorite(j)) if j == i);
        if hovered {
            c.fill(r, c.p.hover);
        }
        hits.push((Hit::Favorite(i), r));
        let size = (22.0 * s).round();
        let remove = Rect::new(r.x1 - size - 6.0 * s, r.center().y - size / 2.0, r.x1 - 6.0 * s, r.center().y + size / 2.0);
        let label = format!("{page}");
        let page_x = remove.x0 - c.measure(&label, SMALL_PX).ceil() - 6.0 * s;
        let title = c.fit(name, TEXT_PX, page_x - pad - 8.0 * s);
        c.label_middle(&title, TEXT_PX, pad, r, c.p.text);
        c.label_middle(&label, SMALL_PX, page_x, r, c.p.dim);
        if hovered {
            if sb.hover == Some(Hit::Unfavorite(i)) {
                c.round(remove, 4.0, c.p.press);
            }
            c.icon_sized(ICON_CLOSE, "x", 9.0, remove, c.p.text);
            hits.push((Hit::Unfavorite(i), remove));
        }
    }
    header + row_h * sb.favorites.len() as f64 + pad
}

/// One form field: a pale box with what's in it, outlined in the accent color while typing into it.
fn draw_field(c: &mut Canvas, f: &FieldLook, w: f64, h: f64) {
    let s = c.s;
    let all = Rect::new(0.0, 0.0, w, h);
    let ink = 0x1B1B1B;
    c.fill(all, if f.focused { c.p.accent } else { 0x8AB4F8 });
    let border = if f.focused { (2.0 * s).round() } else { 1.0 };
    c.fill(all.inset(-border), 0xE8F0FE);
    let size = f.font / s;
    match f.kind {
        FieldShape::Check => {
            if f.checked {
                c.icon_sized(ICON_CHECK, "x", (h * 0.75 / s).max(6.0), all, ink);
            }
        }
        FieldShape::Radio => {
            if f.checked {
                c.ctx.set_paint(rgb(ink));
                c.ctx.fill_path(&Circle::new(all.center(), w.min(h) * 0.25).to_path(0.1));
            }
        }
        FieldShape::Text | FieldShape::Choice => {
            let pad = (3.0 * s).round().max(2.0);
            let line = (f.font * 1.25).max(1.0);
            let lines: Vec<&str> = f.text.split('\n').collect();
            // one line sits in the middle; several start at the top
            let mut y = if lines.len() == 1 { (h - line) / 2.0 } else { pad };
            for text in &lines {
                c.label(text, size, pad, y, ink, false);
                y += line;
            }
            if f.focused {
                let last = lines.last().copied().unwrap_or("");
                let cx = (pad + c.measure(last, size) + 1.0).round();
                let top = y - line;
                c.fill(Rect::new(cx, top + line * 0.1, cx + c.line_px(), top + line * 0.9), ink);
            }
            if f.kind == FieldShape::Choice {
                let arrow = Rect::new(w - h, 0.0, w, h);
                c.icon_sized(ICON_DOWN, "v", (h * 0.45 / s).max(6.0), arrow, ink);
            }
        }
    }
}
