//! Screens and widgets: the UI a mod owns, and the client's screen layer it cannot reach.
//!
//! A mod can build a whole screen here: screens with an id and a title, a stack of them that
//! pushes and pops and replaces, widgets with a kind, a label, a size and a parent, focus and
//! hover, a layout that *places* the widgets (a real vertical, horizontal or grid layout with
//! padding and spacing), a theme and style, a scale, a clip area, input capture, a cursor, a
//! tooltip, and context menus, modals and dialogs as declarations.
//!
//! What it cannot do is put any of it on the client's screen: `open_screen` and
//! `close_screen` would drive the client's own screen layer, which this host has no handle
//! on, and the loader's own panel is the loader's rather than a mod's. Those are refused with
//! their reasons, and the module says which parts are bookkeeping a mod can drive itself.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! ui::register_screen(Screen::new("hello:menu", "Hello").widget(
//!     Widget::new("hello:title", WidgetKind::Label, "Welcome"),
//! ));
//! ui::push_screen("hello:menu");
//! assert_eq!(ui::get_current_screen().as_deref(), Some("hello:menu"));
//! ui::set_layout("hello:menu", Layout::Vertical);
//! assert_eq!(ui::get_layout("hello:menu").map(|(placed, _)| placed.len()), Some(1));
//! assert!(!ui::open_screen("hello:menu"), "the client's screens are its own");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// What a widget is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WidgetKind {
    #[default]
    Label,
    Button,
    Toggle,
    Slider,
    TextBox,
    Dropdown,
    List,
    Panel,
    Image,
    ProgressBar,
    Spacer,
}

impl WidgetKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::Button => "button",
            Self::Toggle => "toggle",
            Self::Slider => "slider",
            Self::TextBox => "text_box",
            Self::Dropdown => "dropdown",
            Self::List => "list",
            Self::Panel => "panel",
            Self::Image => "image",
            Self::ProgressBar => "progress_bar",
            Self::Spacer => "spacer",
        }
    }

    /// Whether the widget takes the keyboard when it is focused.
    #[must_use]
    pub fn takes_focus(self) -> bool {
        matches!(
            self,
            Self::Button | Self::Toggle | Self::Slider | Self::TextBox | Self::Dropdown | Self::List
        )
    }
}

/// A placed widget: its id and the rectangle it landed in.
pub type Placement = (String, (f32, f32, f32, f32));

/// How a screen places its widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Layout {
    /// Every widget where it says it is.
    #[default]
    Absolute,
    /// One above another.
    Vertical,
    /// One beside another.
    Horizontal,
    /// A grid of a given width in columns.
    Grid(usize),
}

/// One widget, as a mod declares it.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    pub id: String,
    pub owner: String,
    pub kind: WidgetKind,
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// The widget it sits inside, when it has one.
    pub parent: Option<String>,
    pub visible: bool,
    pub enabled: bool,
    pub focusable: bool,
    pub focused: bool,
    pub hovered: bool,
    pub tooltip: String,
    /// Whether a mod has a handler for it, which nothing here calls.
    pub has_handler: bool,
}

impl Widget {
    /// A widget with a size a caller would expect for its kind.
    #[must_use]
    pub fn new(id: &str, kind: WidgetKind, text: &str) -> Self {
        let (width, height) = match kind {
            WidgetKind::Label => (0.0, 12.0),
            WidgetKind::Spacer => (0.0, 8.0),
            WidgetKind::ProgressBar => (120.0, 6.0),
            _ => (120.0, 20.0),
        };
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            kind,
            text: text.to_owned(),
            x: 0.0,
            y: 0.0,
            width,
            height,
            parent: None,
            visible: true,
            enabled: true,
            focusable: kind.takes_focus(),
            focused: false,
            hovered: false,
            tooltip: String::new(),
            has_handler: false,
        }
    }

    #[must_use]
    pub fn label(id: &str, text: &str) -> Self {
        Self::new(id, WidgetKind::Label, text)
    }

    #[must_use]
    pub fn button(id: &str, text: &str) -> Self {
        Self::new(id, WidgetKind::Button, text)
    }

    #[must_use]
    pub fn at(mut self, x: f32, y: f32) -> Self {
        self.x = x;
        self.y = y;
        self
    }

    #[must_use]
    pub fn sized(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    #[must_use]
    pub fn inside(mut self, parent: &str) -> Self {
        self.parent = Some(parent.to_owned());
        self
    }

    #[must_use]
    pub fn with_tooltip(mut self, tooltip: &str) -> Self {
        self.tooltip = tooltip.to_owned();
        self
    }

    /// Its rectangle: where it is and how big it is.
    #[must_use]
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        (self.x, self.y, self.width, self.height)
    }

    /// Whether a point is inside it.
    #[must_use]
    pub fn contains(&self, point: (f32, f32)) -> bool {
        point.0 >= self.x
            && point.0 <= self.x + self.width
            && point.1 >= self.y
            && point.1 <= self.y + self.height
    }
}

impl fmt::Display for Widget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} '{}' at ({:.0}, {:.0}) {:.0}x{:.0}{}",
            self.id,
            self.kind.as_str(),
            self.text,
            self.x,
            self.y,
            self.width,
            self.height,
            if self.focused { " focused" } else { "" }
        )
    }
}

/// How a screen is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub id: String,
    pub owner: String,
    pub title: String,
    pub widgets: Vec<Widget>,
    pub layout: Layout,
    pub theme: String,
    pub style: String,
    pub scale: f32,
    pub clip: Option<(f32, f32, f32, f32)>,
    pub padding: f32,
    pub spacing: f32,
    pub modal: bool,
    pub dialog: bool,
    pub context_menu: bool,
    pub captures_input: bool,
    pub tooltip: Option<(String, (f32, f32))>,
}

impl Screen {
    /// A screen with a title and no widgets.
    #[must_use]
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            title: title.to_owned(),
            widgets: Vec::new(),
            layout: Layout::Absolute,
            theme: "minecraft:default".to_owned(),
            style: String::new(),
            scale: 1.0,
            clip: None,
            padding: 4.0,
            spacing: 2.0,
            modal: false,
            dialog: false,
            context_menu: false,
            captures_input: false,
            tooltip: None,
        }
    }

    #[must_use]
    pub fn widget(mut self, widget: Widget) -> Self {
        self.widgets.retain(|held| held.id != widget.id);
        self.widgets.push(widget);
        self
    }

    #[must_use]
    pub fn layout(mut self, layout: Layout) -> Self {
        self.layout = layout;
        self
    }

    #[must_use]
    pub fn theme(mut self, theme: &str) -> Self {
        self.theme = theme.to_owned();
        self
    }

    #[must_use]
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale.max(0.0);
        self
    }

    #[must_use]
    pub fn modal(mut self) -> Self {
        self.modal = true;
        self
    }

    #[must_use]
    pub fn dialog(mut self) -> Self {
        self.dialog = true;
        self
    }

    /// A widget by id.
    #[must_use]
    pub fn widget_of(&self, id: &str) -> Option<&Widget> {
        self.widgets.iter().find(|widget| widget.id == id)
    }

    /// Places the widgets the way the layout says, and answers each one's rectangle.
    ///
    /// This is the layout a mod can rely on: absolute leaves every widget where it says it
    /// is, vertical stacks them down the screen with the padding and the spacing, horizontal
    /// lays them across, and a grid wraps them into columns.
    #[must_use]
    pub fn place(&self, size: (f32, f32)) -> Vec<Placement> {
        let mut placed = Vec::new();
        let mut cursor = (self.padding, self.padding);
        match self.layout {
            Layout::Absolute => {
                for widget in &self.widgets {
                    placed.push((widget.id.clone(), widget.bounds()));
                }
            }
            Layout::Vertical => {
                for widget in &self.widgets {
                    let width = if widget.width > 0.0 {
                        widget.width
                    } else {
                        (size.0 - self.padding * 2.0).max(0.0)
                    };
                    placed.push((widget.id.clone(), (cursor.0, cursor.1, width, widget.height)));
                    cursor.1 += widget.height + self.spacing;
                }
            }
            Layout::Horizontal => {
                for widget in &self.widgets {
                    let height = if widget.height > 0.0 {
                        widget.height
                    } else {
                        (size.1 - self.padding * 2.0).max(0.0)
                    };
                    placed.push((widget.id.clone(), (cursor.0, cursor.1, widget.width, height)));
                    cursor.0 += widget.width + self.spacing;
                }
            }
            Layout::Grid(columns) => {
                let columns = columns.max(1);
                // A screen smaller than its own padding leaves no room rather than a negative
                // one, which is what a caller would see as a rectangle off the screen.
                let cell = ((size.0 - self.padding * 2.0) / columns as f32).max(0.0);
                for (index, widget) in self.widgets.iter().enumerate() {
                    let column = index % columns;
                    let row = index / columns;
                    let x = self.padding + cell * column as f32;
                    let y = self.padding + (widget.height + self.spacing) * row as f32;
                    placed.push((
                        widget.id.clone(),
                        (x, y, (cell - self.spacing).max(0.0), widget.height),
                    ));
                }
            }
        }
        placed
    }

    /// Which widget holds a point, topmost last.
    #[must_use]
    pub fn widget_at(&self, size: (f32, f32), point: (f32, f32)) -> Option<String> {
        self.place(size)
            .into_iter()
            .rev()
            .find(|(_, (x, y, width, height))| {
                point.0 >= *x && point.0 <= x + width && point.1 >= *y && point.1 <= y + height
            })
            .map(|(id, _)| id)
    }
}

impl fmt::Display for Screen {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} '{}' with {} widget(s){}{}",
            self.id,
            self.title,
            self.widgets.len(),
            if self.modal { " (modal)" } else { "" },
            if self.dialog { " (dialog)" } else { "" }
        )
    }
}

/// The state of the mod's own screen stack.
#[derive(Debug, Default, Clone)]
pub struct UiState {
    stack: Vec<String>,
    previous: Option<String>,
    cursor: Option<String>,
    tooltip: Option<(String, (f32, f32))>,
    captures_input: bool,
}

impl UiState {
    /// The screen on top, if there is one.
    #[must_use]
    pub fn current(&self) -> Option<String> {
        self.stack.last().cloned()
    }

    /// The one under it.
    #[must_use]
    pub fn previous(&self) -> Option<String> {
        match self.stack.len() {
            0 | 1 => self.previous.clone(),
            _ => self.stack.get(self.stack.len() - 2).cloned(),
        }
    }

    /// The whole stack, bottom first.
    #[must_use]
    pub fn stack(&self) -> Vec<String> {
        self.stack.clone()
    }
}

fn screens() -> &'static Mutex<BTreeMap<String, Screen>> {
    static SCREENS: OnceLock<Mutex<BTreeMap<String, Screen>>> = OnceLock::new();
    SCREENS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_screens<T>(body: impl FnOnce(&mut BTreeMap<String, Screen>) -> T) -> T {
    let mut guard = screens().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn state() -> &'static Mutex<UiState> {
    static STATE: OnceLock<Mutex<UiState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(UiState::default()))
}

fn with_state<T>(body: impl FnOnce(&mut UiState) -> T) -> T {
    let mut guard = state().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

static LAYOUTS: AtomicU64 = AtomicU64::new(0);
static REBUILDS: AtomicU64 = AtomicU64::new(0);

/// Registers a screen.
pub fn register_screen(screen: Screen) -> bool {
    with_screens(|screens| screens.insert(screen.id.clone(), screen).is_none())
}

/// Forgets a screen.
pub fn unregister_screen(id: &str) -> bool {
    let removed = with_screens(|screens| screens.remove(id).is_some());
    with_state(|state| state.stack.retain(|held| held != id));
    removed
}

/// A screen by id.
#[must_use]
pub fn get_screen_by_id(id: &str) -> Option<Screen> {
    with_screens(|screens| screens.get(id).cloned())
}

/// Every screen a mod registered.
#[must_use]
pub fn get_screens() -> Vec<Screen> {
    with_screens(|screens| screens.values().cloned().collect())
}

/// Starts a screen, for a mod that wants to build one up.
#[must_use]
pub fn create_screen(id: &str, title: &str) -> Screen {
    Screen::new(id, title)
}

/// Forgets a screen, which is `unregister_screen` under the other name.
pub fn destroy_screen(id: &str) -> bool {
    unregister_screen(id)
}

/// Pushes a screen onto the mod's own stack, and answers whether it is registered.
pub fn push_screen(id: &str) -> bool {
    let known = get_screen_by_id(id).is_some();
    if !known {
        return false;
    }
    with_state(|state| {
        state.previous = state.current();
        state.stack.push(id.to_owned());
    });
    true
}

/// Takes the top screen off, and answers which one it was.
pub fn pop_screen() -> Option<String> {
    with_state(|state| {
        let popped = state.stack.pop();
        state.previous = popped.clone();
        popped
    })
}

/// The screen on top.
#[must_use]
pub fn get_current_screen() -> Option<String> {
    with_state(|state| state.current())
}

/// The one under it.
#[must_use]
pub fn get_previous_screen() -> Option<String> {
    with_state(|state| state.previous())
}

/// The whole stack.
#[must_use]
pub fn get_screen_stack() -> Vec<String> {
    with_state(|state| state.stack())
}

/// Adds a widget to a screen.
pub fn register_widget(screen: &str, widget: Widget) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            if screen.widgets.iter().any(|held| held.id == widget.id) {
                return false;
            }
            screen.widgets.push(widget);
            true
        }
        None => false,
    })
}

/// Takes a widget off a screen.
pub fn unregister_widget(screen: &str, id: &str) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            let before = screen.widgets.len();
            screen.widgets.retain(|widget| widget.id != id);
            screen.widgets.len() != before
        }
        None => false,
    })
}

/// Builds a widget, for a mod that wants to add one by hand.
#[must_use]
pub fn create_widget(id: &str, kind: WidgetKind, text: &str) -> Widget {
    Widget::new(id, kind, text)
}

/// A widget on a screen.
#[must_use]
pub fn get_widget(screen: &str, id: &str) -> Option<Widget> {
    get_screen_by_id(screen).and_then(|screen| screen.widget_of(id).cloned())
}

/// Removes a widget, which is `unregister_widget` under the other name.
pub fn remove_widget(screen: &str, id: &str) -> bool {
    unregister_widget(screen, id)
}

/// Finds widgets whose id or text contains something.
#[must_use]
pub fn find_widget(screen: &str, text: &str) -> Vec<Widget> {
    get_screen_by_id(screen)
        .map(|screen| {
            screen
                .widgets
                .into_iter()
                .filter(|widget| widget.id.contains(text) || widget.text.contains(text))
                .collect()
        })
        .unwrap_or_default()
}

/// The widget with focus.
#[must_use]
pub fn get_focused_widget() -> Option<String> {
    with_screens(|screens| {
        screens
            .values()
            .flat_map(|screen| screen.widgets.iter())
            .find(|widget| widget.focused)
            .map(|widget| widget.id.clone())
    })
}

/// Gives a widget focus, taking it from whatever had it.
pub fn set_focus(screen: &str, id: &str) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            let known = screen.widgets.iter().any(|widget| widget.id == id);
            if !known {
                return false;
            }
            for widget in &mut screen.widgets {
                widget.focused = widget.id == id;
            }
            true
        }
        None => false,
    })
}

/// Takes focus off everything.
pub fn clear_focus() -> usize {
    with_screens(|screens| {
        let mut cleared = 0;
        for screen in screens.values_mut() {
            for widget in &mut screen.widgets {
                if widget.focused {
                    widget.focused = false;
                    cleared += 1;
                }
            }
        }
        cleared
    })
}

/// The widget the pointer is over.
#[must_use]
pub fn get_hovered_widget() -> Option<String> {
    with_screens(|screens| {
        screens
            .values()
            .flat_map(|screen| screen.widgets.iter())
            .find(|widget| widget.hovered)
            .map(|widget| widget.id.clone())
    })
}

/// Marks the widget a point is over as hovered, and answers which one it was.
pub fn update_hover(screen: &str, size: (f32, f32), point: (f32, f32)) -> Option<String> {
    let hovered = get_screen_by_id(screen)
        .and_then(|screen| screen.widget_at(size, point));
    with_screens(|screens| {
        if let Some(screen) = screens.get_mut(screen) {
            for widget in &mut screen.widgets {
                widget.hovered = Some(&widget.id) == hovered.as_ref();
            }
        }
    });
    hovered
}

/// The widget everything else sits inside, which is the first one with no parent.
#[must_use]
pub fn get_root_widget(screen: &str) -> Option<String> {
    get_screen_by_id(screen)
        .and_then(|screen| screen.widgets.iter().find(|widget| widget.parent.is_none()).map(|widget| widget.id.clone()))
}

/// A screen's layout, and the rectangles it produced.
#[must_use]
pub fn get_layout(screen: &str) -> Option<(Vec<Placement>, Layout)> {
    get_screen_by_id(screen).map(|screen| (screen.place(screen_size(screen.scale)), screen.layout))
}

fn screen_size(scale: f32) -> (f32, f32) {
    let (width, height) = crate::hud::get_screen_size();
    let scale = if scale <= 0.0 { 1.0 } else { scale };
    (width as f32 / scale, height as f32 / scale)
}

/// Changes a screen's layout, and counts the layouts worked out again.
pub fn set_layout(screen: &str, layout: Layout) -> bool {
    let changed = with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            screen.layout = layout;
            true
        }
        None => false,
    });
    if changed {
        request_layout();
    }
    changed
}

/// A screen's theme.
#[must_use]
pub fn get_theme(screen: &str) -> Option<String> {
    get_screen_by_id(screen).map(|screen| screen.theme)
}

/// Changes it.
pub fn set_theme(screen: &str, theme: &str) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            screen.theme = theme.to_owned();
            true
        }
        None => false,
    })
}

/// A screen's style.
#[must_use]
pub fn get_style(screen: &str) -> Option<String> {
    get_screen_by_id(screen).map(|screen| screen.style)
}

/// Changes it.
pub fn set_style(screen: &str, style: &str) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            screen.style = style.to_owned();
            true
        }
        None => false,
    })
}

/// A screen's scale.
#[must_use]
pub fn get_scale(screen: &str) -> Option<f32> {
    get_screen_by_id(screen).map(|screen| screen.scale)
}

/// Changes it.
pub fn set_scale(screen: &str, scale: f32) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            screen.scale = scale.max(0.0);
            true
        }
        None => false,
    })
}

/// A screen's clip area.
#[must_use]
pub fn get_clip_area(screen: &str) -> Option<(f32, f32, f32, f32)> {
    get_screen_by_id(screen).and_then(|screen| screen.clip)
}

/// Sets it.
pub fn set_clip_area(screen: &str, area: (f32, f32, f32, f32)) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            screen.clip = Some(area);
            true
        }
        None => false,
    })
}

/// Records that the mod's UI takes the input, so a mod driving its own loop can check.
pub fn capture_input() -> bool {
    with_state(|state| {
        let was = state.captures_input;
        state.captures_input = true;
        !was
    })
}

/// Gives it back.
pub fn release_input() -> bool {
    with_state(|state| {
        let was = state.captures_input;
        state.captures_input = false;
        was
    })
}

/// Whether the mod's UI has the input.
#[must_use]
pub fn is_capturing_input() -> bool {
    with_state(|state| state.captures_input)
}

/// Sets the cursor a mod draws itself.
pub fn set_cursor(cursor: &str) -> bool {
    with_state(|state| {
        state.cursor = Some(cursor.to_owned());
        true
    })
}

/// The cursor a mod set.
#[must_use]
pub fn get_cursor() -> Option<String> {
    with_state(|state| state.cursor.clone())
}

/// Sets a tooltip.
pub fn set_tooltip(tooltip: &str) -> bool {
    with_state(|state| {
        state.tooltip = Some((tooltip.to_owned(), (0.0, 0.0)));
        true
    })
}

/// Shows one at a point.
pub fn show_tooltip(tooltip: &str, x: f32, y: f32) -> bool {
    with_state(|state| {
        state.tooltip = Some((tooltip.to_owned(), (x, y)));
        true
    })
}

/// Hides it.
pub fn hide_tooltip() -> bool {
    with_state(|state| state.tooltip.take().is_some())
}

/// The tooltip that is up.
#[must_use]
pub fn get_tooltip() -> Option<(String, (f32, f32))> {
    with_state(|state| state.tooltip.clone())
}

/// Records that a screen has a context menu, which the client would draw.
pub fn register_context_menu(screen: &str) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            screen.context_menu = true;
            true
        }
        None => false,
    })
}

/// Marks a screen as modal.
pub fn register_modal(screen: &str) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            screen.modal = true;
            true
        }
        None => false,
    })
}

/// Marks a screen as a dialog.
pub fn register_dialog(screen: &str) -> bool {
    with_screens(|screens| match screens.get_mut(screen) {
        Some(screen) => {
            screen.dialog = true;
            true
        }
        None => false,
    })
}

/// Asks for the layout to be worked out again.
pub fn request_layout() -> u64 {
    LAYOUTS.fetch_add(1, Ordering::Relaxed) + 1
}

/// The same, under the name the wishlist also uses.
pub fn invalidate() -> u64 {
    request_layout()
}

/// Rebuilds the mod's UI from its declarations, and answers how many screens it has.
pub fn rebuild() -> usize {
    REBUILDS.fetch_add(1, Ordering::Relaxed);
    get_screens().len()
}

/// How many layouts and rebuilds have been asked for.
#[must_use]
pub fn get_counts() -> (u64, u64) {
    (LAYOUTS.load(Ordering::Relaxed), REBUILDS.load(Ordering::Relaxed))
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about a screen: {}", why_unknown(what));
    false
}

/// Opens a screen on the client. `false`: the client's screen layer is its own.
pub fn open_screen(_id: &str) -> bool {
    refused("screen")
}

/// Closes it. `false`.
pub fn close_screen() -> bool {
    refused("screen")
}

/// Replaces it. `false`.
pub fn replace_screen(_id: &str) -> bool {
    refused("screen")
}

/// Lets a widget's handler be called by the client's UI. `false`.
pub fn register_widget_handler(_screen: &str, _id: &str) -> bool {
    refused("input")
}

/// Why a UI question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "screen" => {
            "the client's screen layer draws its own screens and takes its own input; this \
             host has no handle on it, and the loader's own panel is the loader's, not a \
             mod's"
                .to_owned()
        }
        "input" => {
            "a widget's handler is called by the client's UI as it dispatches input; a mod \
             driving its own screen stack calls it itself"
                .to_owned()
        }
        "cursor" => "the client draws its own cursor".to_owned(),
        "tooltip" => "a tooltip is drawn by the client's screen layer".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["screen", "input", "cursor", "tooltip"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_screens(|screens| screens.clear());
        with_state(|state| *state = UiState::default());
        crate::hud::set_screen_size((1280, 720));
    }

    fn menu() -> Screen {
        Screen::new("hello:menu", "Hello")
            .widget(Widget::label("hello:title", "Welcome").sized(200.0, 20.0))
            .widget(Widget::button("hello:play", "Play").sized(100.0, 20.0))
            .widget(Widget::label("hello:note", "note"))
    }

    #[test]
    fn a_screen_and_its_widgets_are_real_data() {
        let _serial = serial();
        fresh();
        assert!(register_screen(menu()));
        assert!(!register_screen(menu()), "it replaces");
        let screen = get_screen_by_id("hello:menu").expect("registered");
        assert_eq!(screen.title, "Hello");
        assert_eq!(screen.widgets.len(), 3);
        assert_eq!(get_screens().len(), 1);
        assert!(screen.to_string().contains("3 widget(s)"));
        assert!(register_widget("hello:menu", Widget::label("hello:extra", "extra")));
        assert!(!register_widget("hello:menu", Widget::label("hello:extra", "again")), "once");
        assert!(!register_widget("nothing", Widget::label("x", "x")));
        assert_eq!(get_widget("hello:menu", "hello:play").map(|widget| widget.kind), Some(WidgetKind::Button));
        assert!(unregister_widget("hello:menu", "hello:extra"));
        assert!(!unregister_widget("hello:menu", "hello:extra"));
        assert!(remove_widget("hello:menu", "hello:note"));
        assert_eq!(find_widget("hello:menu", "hello:").len(), 2);
        assert_eq!(find_widget("hello:menu", "Play").len(), 1);
        assert_eq!(get_root_widget("hello:menu"), Some("hello:title".to_owned()));
        assert_eq!(get_widget("hello:menu", "nothing"), None);
        assert_eq!(create_widget("w", WidgetKind::Slider, "s").kind, WidgetKind::Slider);
        assert_eq!(create_screen("s", "t").id, "s");
        assert!(destroy_screen("hello:menu"));
        assert!(!destroy_screen("hello:menu"));
    }

    #[test]
    fn a_widget_has_a_kind_and_a_size_and_a_hit_test() {
        let _serial = serial();
        let label = Widget::label("l", "text");
        assert_eq!(label.kind, WidgetKind::Label);
        assert_eq!(label.bounds(), (0.0, 0.0, 0.0, 12.0));
        let button = Widget::button("b", "press").at(10.0, 20.0).sized(100.0, 20.0).inside("panel");
        assert_eq!(button.bounds(), (10.0, 20.0, 100.0, 20.0));
        assert_eq!(button.parent.as_deref(), Some("panel"));
        assert!(button.contains((50.0, 30.0)));
        assert!(!button.contains((5.0, 30.0)));
        assert!(!button.contains((50.0, 50.0)));
        assert!(WidgetKind::Button.takes_focus());
        assert!(!WidgetKind::Label.takes_focus());
        assert_eq!(WidgetKind::ProgressBar.as_str(), "progress_bar");
        assert_eq!(
            Widget::new("x", WidgetKind::ProgressBar, "").bounds().3,
            6.0
        );
        assert_eq!(Widget::new("x", WidgetKind::Spacer, "").bounds().3, 8.0);
        assert!(button.to_string().contains("press"));
    }

    #[test]
    fn the_stack_pushes_pops_and_replaces_the_mods_own_screens() {
        let _serial = serial();
        fresh();
        register_screen(menu());
        register_screen(Screen::new("hello:settings", "Settings"));
        assert!(!push_screen("nothing"), "only a registered screen goes on the stack");
        assert!(push_screen("hello:menu"));
        assert_eq!(get_current_screen(), Some("hello:menu".to_owned()));
        assert!(push_screen("hello:settings"));
        assert_eq!(get_current_screen(), Some("hello:settings".to_owned()));
        assert_eq!(get_previous_screen(), Some("hello:menu".to_owned()));
        assert_eq!(get_screen_stack(), vec!["hello:menu".to_owned(), "hello:settings".to_owned()]);
        assert_eq!(pop_screen(), Some("hello:settings".to_owned()));
        assert_eq!(get_current_screen(), Some("hello:menu".to_owned()));
        assert_eq!(pop_screen(), Some("hello:menu".to_owned()));
        assert_eq!(pop_screen(), None);
        assert_eq!(get_current_screen(), None);
        assert!(push_screen("hello:menu"));
        assert!(unregister_screen("hello:menu"));
        assert_eq!(get_current_screen(), None, "unregistering takes it off the stack");
    }

    #[test]
    fn focus_hover_and_layout_work_on_the_widgets() {
        let _serial = serial();
        fresh();
        register_screen(menu());
        assert_eq!(get_focused_widget(), None);
        assert!(!set_focus("hello:menu", "nothing"));
        assert!(set_focus("hello:menu", "hello:play"));
        assert_eq!(get_focused_widget(), Some("hello:play".to_owned()));
        assert!(set_focus("hello:menu", "hello:title"));
        assert_eq!(get_focused_widget(), Some("hello:title".to_owned()), "focus moves");
        assert_eq!(clear_focus(), 1);
        assert_eq!(get_focused_widget(), None);

        // Hover is a hit test against the placed rectangles.
        let hovered = update_hover("hello:menu", (1280.0, 720.0), (50.0, 20.0));
        assert_eq!(hovered, Some("hello:play".to_owned()));
        assert_eq!(get_hovered_widget(), Some("hello:play".to_owned()));
        assert_eq!(update_hover("hello:menu", (1280.0, 720.0), (9999.0, 9999.0)), None);
        assert_eq!(get_hovered_widget(), None);

        // A vertical layout stacks them from the padding down.
        assert!(set_layout("hello:menu", Layout::Vertical));
        let (placed, layout) = get_layout("hello:menu").expect("it has one");
        assert_eq!(layout, Layout::Vertical);
        assert_eq!(placed.len(), 3);
        assert_eq!(placed[0].1 .1, 4.0, "the first is at the padding");
        assert_eq!(placed[1].1 .1, 4.0 + 20.0 + 2.0);
        assert_eq!(placed[2].1 .1, 4.0 + 20.0 + 2.0 + 20.0 + 2.0);
        assert!(placed[0].1 .2 > 0.0, "a widget with no width fills the screen");

        // A horizontal one lays them across.
        assert!(set_layout("hello:menu", Layout::Horizontal));
        let (placed, _) = get_layout("hello:menu").expect("it has one");
        assert_eq!(placed[0].1 .0, 4.0);
        assert_eq!(placed[1].1 .0, 4.0 + 200.0 + 2.0);

        // A grid wraps into columns.
        assert!(set_layout("hello:menu", Layout::Grid(2)));
        let (placed, _) = get_layout("hello:menu").expect("it has one");
        assert_eq!(placed[1].1 .0, placed[0].1 .0 + placed[0].1 .2 + 2.0);
        assert_eq!(placed[2].1 .1, placed[0].1 .1 + 12.0 + 2.0, "the third wraps to row two");
        assert!(set_layout("hello:menu", Layout::Absolute));
        let (placed, _) = get_layout("hello:menu").expect("it has one");
        assert_eq!(placed[0].1, (0.0, 0.0, 200.0, 20.0), "absolute is where it says");
        assert!(!set_layout("nothing", Layout::Vertical));
    }

    #[test]
    fn theme_style_scale_clip_input_and_tooltips_are_the_mods() {
        let _serial = serial();
        fresh();
        register_screen(menu().theme("hello:dark").scale(2.0));
        assert_eq!(get_theme("hello:menu"), Some("hello:dark".to_owned()));
        assert_eq!(get_scale("hello:menu"), Some(2.0));
        assert!(set_theme("hello:menu", "minecraft:default"));
        assert!(set_style("hello:menu", "flat"));
        assert_eq!(get_style("hello:menu"), Some("flat".to_owned()));
        assert!(set_scale("hello:menu", 1.0));
        assert!(set_clip_area("hello:menu", (0.0, 0.0, 100.0, 100.0)));
        assert_eq!(get_clip_area("hello:menu"), Some((0.0, 0.0, 100.0, 100.0)));
        assert!(!set_theme("nothing", "x"));
        assert!(!set_style("nothing", "x"));
        assert!(!set_scale("nothing", 1.0));
        assert!(!set_clip_area("nothing", (0.0, 0.0, 0.0, 0.0)));

        assert!(!is_capturing_input());
        assert!(capture_input());
        assert!(!capture_input(), "capturing twice says it already had it");
        assert!(is_capturing_input());
        assert!(release_input());
        assert!(!release_input());

        assert!(set_cursor("hello:hand"));
        assert_eq!(get_cursor(), Some("hello:hand".to_owned()));
        assert!(show_tooltip("press me", 10.0, 20.0));
        assert_eq!(get_tooltip(), Some(("press me".to_owned(), (10.0, 20.0))));
        assert!(set_tooltip("another"));
        assert!(hide_tooltip());
        assert!(!hide_tooltip());
        assert!(register_context_menu("hello:menu"));
        assert!(register_modal("hello:menu"));
        assert!(register_dialog("hello:menu"));
        let screen = get_screen_by_id("hello:menu").expect("registered");
        assert!(screen.context_menu && screen.modal && screen.dialog);
        assert!(screen.to_string().contains("modal"));
    }

    #[test]
    fn the_clients_screen_layer_is_refused() {
        let _serial = serial();
        fresh();
        register_screen(menu());
        assert!(!open_screen("hello:menu"));
        assert!(!close_screen());
        assert!(!replace_screen("hello:menu"));
        assert!(!register_widget_handler("hello:menu", "hello:play"));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("screen").contains("client's screen layer"));
        assert!(request_layout() > 0);
        assert!(invalidate() > 0);
        assert_eq!(rebuild(), 1);
        let (layouts, rebuilds) = get_counts();
        assert!(layouts >= 2 && rebuilds == 1);
    }
}
