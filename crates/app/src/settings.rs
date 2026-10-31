//! User settings: one table (`FIELDS`) describes every setting, and both the settings file and the
//! settings panel are generated from it. Adding a setting means a struct field and a table entry.
//!
//! The file is `FloresPDF-settings.toml` next to the exe if that exists (portable, like Sumatra),
//! otherwise `%APPDATA%\FloresPDF\settings.toml`. It's a flat subset of TOML: `key = value` lines.

use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    System,
    Light,
    Dark,
    /// Pure black, for OLED screens.
    Black,
}

/// How page content is colored. Applied to rendered tiles, so it costs nothing per frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageColors {
    Normal,
    /// Light text on dark pages, with colors kept recognizable.
    Dark,
    Sepia,
    /// Light text on pure black pages, for OLED screens.
    Black,
}

/// The color of new highlights: the first four of main.rs's ANNOT_COLORS.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HighlightColor {
    Yellow,
    Green,
    Blue,
    Pink,
}

/// Where "Search the web" in the right-click menu sends the selected text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WebSearch {
    Google,
    Bing,
    DuckDuckGo,
}

impl WebSearch {
    pub fn url(self) -> &'static str {
        match self {
            WebSearch::Google => "https://www.google.com/search?q=",
            WebSearch::Bing => "https://www.bing.com/search?q=",
            WebSearch::DuckDuckGo => "https://duckduckgo.com/?q=",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartZoom {
    FitPage,
    FitWidth,
    ActualSize,
}

/// Where and how big the window was, saved when "remember window" is on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub theme: Theme,
    pub page_colors: PageColors,
    pub show_toolbar: bool,
    pub show_outline: bool,
    /// In logical pixels; dragging the sidebar's edge sets it too.
    pub sidebar_width: i64,
    pub page_gap: i64,
    pub start_zoom: StartZoom,
    pub scroll_step: i64,
    pub remember_window: bool,
    pub smooth_scroll: bool,
    pub web_search: WebSearch,
    pub highlight_color: HighlightColor,
    pub single_window: bool,
    /// Pixels per inch for "Copy page as image" and "Save page as PNG".
    pub image_dpi: i64,
    pub remember_files: bool,
    pub restore_session: bool,
    pub reload_changed: bool,
    pub check_updates: bool,
    pub cache_screens: i64,
    pub render_threads: i64,
    pub window: Option<Placement>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            page_colors: PageColors::Normal,
            show_toolbar: true,
            show_outline: true,
            sidebar_width: 280,
            page_gap: 8,
            start_zoom: StartZoom::FitPage,
            scroll_step: 100,
            remember_window: true,
            smooth_scroll: true,
            web_search: WebSearch::Google,
            highlight_color: HighlightColor::Yellow,
            single_window: true,
            image_dpi: 150,
            remember_files: true,
            restore_session: true,
            reload_changed: true,
            check_updates: true,
            cache_screens: 1,
            render_threads: 0,
            window: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Value {
    /// Index into the field's options.
    Choice(usize),
    Toggle(bool),
    Number(i64),
}

pub enum Kind {
    /// (name in the file, label in the panel)
    Choice(&'static [(&'static str, &'static str)]),
    Toggle,
    /// `zero` is shown instead of 0, when 0 means something special.
    Number { min: i64, max: i64, step: i64, unit: &'static str, zero: Option<&'static str> },
}

pub struct Field {
    pub key: &'static str,
    pub section: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub get: fn(&Settings) -> Value,
    pub set: fn(&mut Settings, Value),
}

fn choice(v: Value) -> usize {
    match v {
        Value::Choice(i) => i,
        _ => 0,
    }
}

fn toggle(v: Value) -> bool {
    matches!(v, Value::Toggle(true))
}

fn number(v: Value) -> i64 {
    match v {
        Value::Number(n) => n,
        _ => 0,
    }
}

pub static FIELDS: &[Field] = &[
    Field {
        key: "theme",
        section: "Appearance",
        label: "Theme",
        help: "Colors of the toolbar, panels and the space around pages.",
        kind: Kind::Choice(&[("system", "System"), ("light", "Light"), ("dark", "Dark"), ("black", "Black")]),
        get: |s| Value::Choice(s.theme as usize),
        set: |s, v| s.theme = [Theme::System, Theme::Light, Theme::Dark, Theme::Black][choice(v).min(3)],
    },
    Field {
        key: "page_colors",
        section: "Appearance",
        label: "Page colors",
        help: "Dark turns white pages dark and text light. Sepia warms them for long reading. Black makes \
               pages pure black, which OLED screens show by turning those pixels off; with the Black theme \
               too, everything is.",
        kind: Kind::Choice(&[("normal", "Normal"), ("dark", "Dark"), ("sepia", "Sepia"), ("black", "Black")]),
        get: |s| Value::Choice(s.page_colors as usize),
        set: |s, v| {
            s.page_colors = [PageColors::Normal, PageColors::Dark, PageColors::Sepia, PageColors::Black][choice(v).min(3)]
        },
    },
    Field {
        key: "show_toolbar",
        section: "Appearance",
        label: "Toolbar",
        help: "F8 shows and hides it too.",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.show_toolbar),
        set: |s, v| s.show_toolbar = toggle(v),
    },
    Field {
        key: "show_outline",
        section: "Appearance",
        label: "Outline sidebar",
        help: "Shown when the document has an outline. F12 shows and hides it too.",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.show_outline),
        set: |s, v| s.show_outline = toggle(v),
    },
    Field {
        key: "sidebar_width",
        section: "Appearance",
        label: "Sidebar width",
        help: "Dragging the sidebar's edge changes it too.",
        kind: Kind::Number { min: 160, max: 600, step: 20, unit: "px", zero: None },
        get: |s| Value::Number(s.sidebar_width),
        set: |s, v| s.sidebar_width = number(v),
    },
    Field {
        key: "page_gap",
        section: "Appearance",
        label: "Space between pages",
        help: "Also the margin around them.",
        kind: Kind::Number { min: 0, max: 48, step: 4, unit: "px", zero: None },
        get: |s| Value::Number(s.page_gap),
        set: |s, v| s.page_gap = number(v),
    },
    Field {
        key: "start_zoom",
        section: "Reading",
        label: "Zoom for new documents",
        help: "Ctrl+0, Ctrl+1 and Ctrl+2 switch between these while reading.",
        kind: Kind::Choice(&[("fit-page", "Fit page"), ("fit-width", "Fit width"), ("actual-size", "100%")]),
        get: |s| Value::Choice(s.start_zoom as usize),
        set: |s, v| s.start_zoom = [StartZoom::FitPage, StartZoom::FitWidth, StartZoom::ActualSize][choice(v).min(2)],
    },
    Field {
        key: "scroll_step",
        section: "Reading",
        label: "Mouse wheel scrolls",
        help: "Distance per wheel notch.",
        kind: Kind::Number { min: 20, max: 400, step: 20, unit: "px", zero: None },
        get: |s| Value::Number(s.scroll_step),
        set: |s, v| s.scroll_step = number(v),
    },
    Field {
        key: "remember_window",
        section: "Reading",
        label: "Remember window size and position",
        help: "",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.remember_window),
        set: |s, v| s.remember_window = toggle(v),
    },
    Field {
        key: "smooth_scroll",
        section: "Reading",
        label: "Smooth scrolling",
        help: "The wheel and the keys glide the pages to where they're going instead of jumping there.",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.smooth_scroll),
        set: |s, v| s.smooth_scroll = toggle(v),
    },
    Field {
        key: "web_search",
        section: "Reading",
        label: "Search the web with",
        help: "For \"Search the web\" on selected text in the right-click menu.",
        kind: Kind::Choice(&[("google", "Google"), ("bing", "Bing"), ("duckduckgo", "DuckDuckGo")]),
        get: |s| Value::Choice(s.web_search as usize),
        set: |s, v| s.web_search = [WebSearch::Google, WebSearch::Bing, WebSearch::DuckDuckGo][choice(v).min(2)],
    },
    Field {
        key: "highlight_color",
        section: "Reading",
        label: "Highlight color",
        help: "For highlighting selected text: right-click it, or Ctrl+H. Highlights are kept by Flores PDF, not written into the file.",
        kind: Kind::Choice(&[("yellow", "Yellow"), ("green", "Green"), ("blue", "Blue"), ("pink", "Pink")]),
        get: |s| Value::Choice(s.highlight_color as usize),
        set: |s, v| {
            s.highlight_color = [HighlightColor::Yellow, HighlightColor::Green, HighlightColor::Blue, HighlightColor::Pink][choice(v).min(3)]
        },
    },
    Field {
        key: "single_window",
        section: "Files",
        label: "Open files as tabs in one window",
        help: "A PDF opened while the reader is running becomes a tab of its window instead of a new window.",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.single_window),
        set: |s, v| s.single_window = toggle(v),
    },
    Field {
        key: "remember_files",
        section: "Files",
        label: "Remember opened files",
        help: "Lists recent files on the start page and reopens each where you left off. Turning it off forgets the list.",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.remember_files),
        set: |s, v| s.remember_files = toggle(v),
    },
    Field {
        key: "restore_session",
        section: "Files",
        label: "Reopen last session's tabs",
        help: "Tabs open when the reader closed come back next time; each loads when you first look at it. Needs \"Remember opened files\".",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.restore_session),
        set: |s, v| s.restore_session = toggle(v),
    },
    Field {
        key: "check_updates",
        section: "System",
        label: "Check for updates",
        help: "When Flores PDF starts, it asks GitHub whether there's a newer release, and offers it in the toolbar. Nothing else is sent.",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.check_updates),
        set: |s, v| s.check_updates = toggle(v),
    },
    Field {
        key: "reload_changed",
        section: "Files",
        label: "Reload files when they change",
        help: "When another program rewrites an open PDF (LaTeX, a converter), it's read again at the same page.",
        kind: Kind::Toggle,
        get: |s| Value::Toggle(s.reload_changed),
        set: |s, v| s.reload_changed = toggle(v),
    },
    Field {
        key: "image_dpi",
        section: "Files",
        label: "Page image resolution",
        help: "For \"Copy page as image\" and \"Save page as PNG\". 150 suits documents and slides; 300 suits printing.",
        kind: Kind::Number { min: 72, max: 600, step: 25, unit: "dpi", zero: None },
        get: |s| Value::Number(s.image_dpi),
        set: |s, v| s.image_dpi = number(v),
    },
    Field {
        key: "cache_screens",
        section: "Memory and speed",
        label: "Page cache",
        help: "Rendered pages kept for scrolling back, in windowfuls. Text pages are compressed, so one holds several screens.",
        kind: Kind::Number { min: 1, max: 16, step: 1, unit: "screens", zero: None },
        get: |s| Value::Number(s.cache_screens),
        set: |s, v| s.cache_screens = number(v),
    },
    Field {
        key: "render_threads",
        section: "Memory and speed",
        label: "Render threads",
        help: "More renders heavy pages faster and uses more memory while busy. Automatic uses half the cores, up to 4.",
        kind: Kind::Number { min: 0, max: 16, step: 1, unit: "", zero: Some("Automatic") },
        get: |s| Value::Number(s.render_threads),
        set: |s, v| s.render_threads = number(v),
    },
];

impl Field {
    /// Steps a number up or down, or moves to another option.
    pub fn clamp(&self, v: Value) -> Value {
        match (&self.kind, v) {
            (Kind::Choice(options), Value::Choice(i)) => Value::Choice(i.min(options.len() - 1)),
            (Kind::Number { min, max, .. }, Value::Number(n)) => Value::Number(n.clamp(*min, *max)),
            (_, v) => v,
        }
    }

    pub fn display(&self, v: Value) -> String {
        match (&self.kind, v) {
            (Kind::Choice(options), Value::Choice(i)) => options[i.min(options.len() - 1)].1.to_owned(),
            (Kind::Toggle, Value::Toggle(on)) => if on { "On" } else { "Off" }.to_owned(),
            (Kind::Number { zero: Some(z), .. }, Value::Number(0)) => (*z).to_owned(),
            (Kind::Number { unit, .. }, Value::Number(n)) if unit.is_empty() => n.to_string(),
            // units are written plural: "1 screen", "2 screens"
            (Kind::Number { unit, .. }, Value::Number(1)) => format!("1 {}", unit.trim_end_matches('s')),
            (Kind::Number { unit, .. }, Value::Number(n)) => format!("{n} {unit}"),
            _ => String::new(),
        }
    }
}

/// Recent files go beside the settings: `FloresPDF-history.txt` for a portable copy, else
/// `history.txt` in the same folder.
pub fn history_path() -> PathBuf {
    let settings = path();
    let portable = settings.file_name().is_some_and(|n| n == "FloresPDF-settings.toml");
    settings.with_file_name(if portable { "FloresPDF-history.txt" } else { "history.txt" })
}

/// Highlights go beside the settings too.
pub fn highlights_path() -> PathBuf {
    let settings = path();
    let portable = settings.file_name().is_some_and(|n| n == "FloresPDF-settings.toml");
    settings.with_file_name(if portable { "FloresPDF-highlights.txt" } else { "highlights.txt" })
}

/// Favorite pages go beside the settings too.
pub fn favorites_path() -> PathBuf {
    let settings = path();
    let portable = settings.file_name().is_some_and(|n| n == "FloresPDF-settings.toml");
    settings.with_file_name(if portable { "FloresPDF-favorites.txt" } else { "favorites.txt" })
}

/// Copies the settings and recent files from the folder the reader used before it was named
/// Flores PDF, the first time it runs under the new name. The old folder is left as it was.
pub fn migrate() {
    let Some(base) = std::env::var_os("APPDATA").map(PathBuf::from) else { return };
    let (old, new) = (base.join("pdfreader"), base.join("FloresPDF"));
    // a portable copy keeps its settings beside the exe and never uses this folder
    if !path().starts_with(&new) || new.exists() || !old.join("settings.toml").exists() {
        return;
    }
    if std::fs::create_dir_all(&new).is_ok() {
        for name in ["settings.toml", "history.txt"] {
            let _ = std::fs::copy(old.join(name), new.join(name));
        }
    }
}

pub fn path() -> PathBuf {
    let portable = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("FloresPDF-settings.toml")));
    if let Some(p) = portable.filter(|p| p.exists()) {
        return p;
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("FloresPDF").join("settings.toml")
}

impl Settings {
    /// Reads the settings file. Missing keys and values that don't parse keep their defaults.
    pub fn load() -> Self {
        let mut s = Settings::default();
        let Ok(text) = std::fs::read_to_string(path()) else { return s };
        let mut window = (None, None, None, None, false);
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let unquoted = value.trim_matches('"');
            match key {
                "window_x" => window.0 = value.parse().ok(),
                "window_y" => window.1 = value.parse().ok(),
                "window_width" => window.2 = value.parse().ok(),
                "window_height" => window.3 = value.parse().ok(),
                "window_maximized" => window.4 = value == "true",
                _ => {}
            }
            let Some(field) = FIELDS.iter().find(|f| f.key == key) else { continue };
            let v = match &field.kind {
                Kind::Choice(options) => options.iter().position(|(name, _)| *name == unquoted).map(Value::Choice),
                Kind::Toggle => match value {
                    "true" => Some(Value::Toggle(true)),
                    "false" => Some(Value::Toggle(false)),
                    _ => None,
                },
                Kind::Number { .. } => value.parse().ok().map(Value::Number),
            };
            if let Some(v) = v {
                (field.set)(&mut s, field.clamp(v));
            }
        }
        if let (Some(x), Some(y), Some(width), Some(height), maximized) = window {
            s.window = Some(Placement { x, y, width, height, maximized });
        }
        s
    }

    /// Writes every setting with its description, so the file documents itself.
    pub fn save(&self) -> std::io::Result<()> {
        let mut out = String::from("# Flores PDF settings. Every one of these is also in the settings panel (Ctrl+,).\n");
        for field in FIELDS {
            let value = match (&field.kind, (field.get)(self)) {
                (Kind::Choice(options), Value::Choice(i)) => {
                    let names: Vec<_> = options.iter().map(|(n, _)| *n).collect();
                    let _ = write!(out, "\n# {}. One of: {}.", field.label, names.join(", "));
                    format!("\"{}\"", options[i].0)
                }
                (_, Value::Toggle(on)) => {
                    let _ = write!(out, "\n# {}. true or false.", field.label);
                    on.to_string()
                }
                (Kind::Number { min, max, .. }, Value::Number(n)) => {
                    let _ = write!(out, "\n# {}. {min} to {max}.", field.label);
                    n.to_string()
                }
                _ => continue,
            };
            if !field.help.is_empty() {
                let _ = write!(out, " {}", field.help);
            }
            let _ = writeln!(out, "\n{} = {value}", field.key);
        }
        if let Some(w) = self.window {
            let _ = write!(
                out,
                "\n# Window placement, saved on exit.\nwindow_x = {}\nwindow_y = {}\nwindow_width = {}\nwindow_height = {}\nwindow_maximized = {}\n",
                w.x, w.y, w.width, w.height, w.maximized
            );
        }
        let path = path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_round_trips_through_its_value() {
        let mut s = Settings::default();
        for f in FIELDS {
            let v = (f.get)(&s);
            (f.set)(&mut s, v);
        }
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn display_values() {
        let threads = FIELDS.iter().find(|f| f.key == "render_threads").unwrap();
        assert_eq!(threads.display(Value::Number(0)), "Automatic");
        assert_eq!(threads.display(Value::Number(3)), "3");
        let gap = FIELDS.iter().find(|f| f.key == "page_gap").unwrap();
        assert_eq!(gap.display(Value::Number(8)), "8 px");
        assert_eq!(gap.clamp(Value::Number(99)), Value::Number(48));
    }
}
