//! Screen and menu builders: a fluent way to assemble a screen, and the client screen layer
//! it still cannot reach.
//!
//! A mod can build a screen out of widgets the way it writes Rust: [`ScreenBuilder`] starts
//! empty, each fluent method returns the builder, and [`ScreenBuilder::build`] checks the
//! declaration and registers a real [`crate::ui::Screen`] whose children are the widget
//! nodes the builder was handed, in the order they were added. The validation is the point:
//! an empty id, a size outside the range, a screen id already registered, two widgets sharing
//! an id, and a widget naming a parent that was never added are each refused with an error
//! that names the offending value.
//!
//! The geometry is real too: [`ScreenBuilder::place`] answers each child's rectangle through
//! the §36/37 layout maths on the size the builder declared, so a caller can act on the same
//! rectangles before anything is drawn. What the builder cannot do is *show* the screen on
//! the client — the client's screen layer draws its own screens and takes its own input, and
//! this host has no handle on it. [`ScreenBuilder::show`] is refused with its reason, and
//! [`why_unknown`] and [`refusals`] say so.
//!
//! ```
//! use cinnaloader::builders_ui::ScreenBuilder;
//! use cinnaloader::prelude::*;
//!
//! let builder = ScreenBuilder::new()
//!     .id("hello:menu")
//!     .title("Hello")
//!     .size(320, 240)
//!     .theme("hello:dark")
//!     .layout(Layout::Vertical)
//!     .add_text("hello:title", "Welcome")
//!     .add_button("hello:play", "Play")
//!     .on_open("hello:greet");
//! let screen = builder.build().expect("a real screen");
//! assert_eq!(screen.title, "Hello");
//! assert_eq!(screen.theme, "hello:dark");
//! assert_eq!(ui::get_screen_by_id("hello:menu").map(|held| held.title), Some("Hello".to_owned()));
//! assert_eq!(builder.place()[1].1, (4.0, 18.0, 120.0, 20.0));
//! assert!(!builder.show(), "the client owns its own screen layer");
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::ui::{self, Layout, Placement, Screen};
use crate::widget::{WidgetNode, WidgetType};

/// The size a builder starts with: 320 by 240 units.
pub const DEFAULT_SIZE: (u32, u32) = (320, 240);

/// The smallest side a built screen may have.
pub const MIN_SIZE: u32 = 1;

/// The largest side a built screen may have.
pub const MAX_SIZE: u32 = 4096;

/// The theme a builder starts with.
pub const DEFAULT_THEME: &str = "minecraft:default";

/// Why a screen declaration was refused at [`ScreenBuilder::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    MissingId,
    DuplicateScreen {
        id: String,
    },
    DuplicateWidget {
        id: String,
    },
    UnknownParent {
        widget: String,
        parent: String,
    },
    SizeOutOfRange {
        width: u32,
        height: u32,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingId => {
                write!(formatter, "a screen needs a non-empty id, got \"\"")
            }
            Self::DuplicateScreen { id } => {
                write!(formatter, "a screen called \"{id}\" is already registered")
            }
            Self::DuplicateWidget { id } => {
                write!(formatter, "two widgets on this screen share the id \"{id}\"")
            }
            Self::UnknownParent { widget, parent } => {
                write!(
                    formatter,
                    "widget \"{widget}\" sits inside \"{parent}\", which was never added"
                )
            }
            Self::SizeOutOfRange { width, height } => {
                write!(
                    formatter,
                    "a screen size of {width}x{height} is outside {MIN_SIZE}..={MAX_SIZE}"
                )
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// A fluent screen: what it is called, how big it is, and the widgets it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenBuilder {
    id: String,
    title: String,
    size: (u32, u32),
    background: String,
    layout: Layout,
    theme: String,
    widgets: Vec<WidgetNode>,
    handlers: Vec<(String, String)>,
}

impl Default for ScreenBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenBuilder {
    /// An empty builder: no id, no widgets, the default size and theme.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            size: DEFAULT_SIZE,
            background: String::new(),
            layout: Layout::Absolute,
            theme: DEFAULT_THEME.to_owned(),
            widgets: Vec::new(),
            handlers: Vec::new(),
        }
    }

    #[must_use]
    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }

    #[must_use]
    pub fn title(mut self, title: &str) -> Self {
        self.title = title.to_owned();
        self
    }

    #[must_use]
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = (width, height);
        self
    }

    /// The fill behind the widgets. The screen layer keeps no colour of its own, so it is
    /// stored as the screen's style.
    #[must_use]
    pub fn background(mut self, background: &str) -> Self {
        self.background = background.to_owned();
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
    pub fn add_widget(mut self, widget: WidgetNode) -> Self {
        self.widgets.push(widget);
        self
    }

    #[must_use]
    pub fn add_button(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::Button, text).sized(120.0, 20.0))
    }

    #[must_use]
    pub fn add_text(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::TextLabel, text).sized(0.0, 12.0))
    }

    #[must_use]
    pub fn add_image(self, id: &str, source: &str) -> Self {
        let mut node = WidgetNode::new(id, WidgetType::ImageView, "").sized(64.0, 64.0);
        node.icon = source.to_owned();
        self.add_widget(node)
    }

    #[must_use]
    pub fn add_input(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::TextInput, text).sized(120.0, 20.0))
    }

    #[must_use]
    pub fn add_slider(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::Slider, text).sized(120.0, 20.0))
    }

    #[must_use]
    pub fn add_checkbox(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::Checkbox, text).sized(120.0, 20.0))
    }

    #[must_use]
    pub fn add_dropdown(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::Dropdown, text).sized(120.0, 20.0))
    }

    #[must_use]
    pub fn add_list(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::ListView, text).sized(120.0, 60.0))
    }

    #[must_use]
    pub fn add_grid(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::GridView, text).sized(120.0, 60.0))
    }

    #[must_use]
    pub fn add_panel(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::Panel, text).sized(160.0, 120.0))
    }

    #[must_use]
    pub fn add_tab(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::Tab, text).sized(0.0, 20.0))
    }

    #[must_use]
    pub fn add_scroll_view(self, id: &str, text: &str) -> Self {
        self.add_widget(WidgetNode::new(id, WidgetType::ScrollView, text).sized(120.0, 80.0))
    }

    #[must_use]
    pub fn on_open(mut self, handler: &str) -> Self {
        self.note("open", handler);
        self
    }

    #[must_use]
    pub fn on_close(mut self, handler: &str) -> Self {
        self.note("close", handler);
        self
    }

    #[must_use]
    pub fn on_tick(mut self, handler: &str) -> Self {
        self.note("tick", handler);
        self
    }

    #[must_use]
    pub fn on_render(mut self, handler: &str) -> Self {
        self.note("render", handler);
        self
    }

    #[must_use]
    pub fn on_input(mut self, handler: &str) -> Self {
        self.note("input", handler);
        self
    }

    /// The size the builder declared, before any scaling.
    #[must_use]
    pub fn pixel_size(&self) -> (u32, u32) {
        self.size
    }

    /// Places the widgets through the declared layout on the declared size.
    #[must_use]
    pub fn place(&self) -> Vec<Placement> {
        self.to_screen()
            .place((self.size.0 as f32, self.size.1 as f32))
    }

    /// Checks the declaration and registers a real screen, answering it back.
    #[must_use = "a built screen is meant to be read back"]
    pub fn build(&self) -> Result<Screen, BuildError> {
        self.validate()?;
        let screen = self.to_screen();
        ui::register_screen(screen.clone());
        self.record_handlers();
        Ok(screen)
    }

    /// The wishlist's name for [`Self::build`].
    #[must_use = "a registered screen is meant to be read back"]
    pub fn register(&self) -> Result<Screen, BuildError> {
        self.build()
    }

    /// Shows the built screen on the client. `false`: the client's screen layer is its own.
    #[must_use]
    pub fn show(&self) -> bool {
        refused(&self.id, "show")
    }

    fn note(&mut self, kind: &str, handler: &str) {
        match self.handlers.iter_mut().find(|(held, _)| held == kind) {
            Some((_, name)) => *name = handler.to_owned(),
            None => self.handlers.push((kind.to_owned(), handler.to_owned())),
        }
    }

    fn validate(&self) -> Result<(), BuildError> {
        if self.id.trim().is_empty() {
            return Err(BuildError::MissingId);
        }
        if self.size.0 < MIN_SIZE
            || self.size.1 < MIN_SIZE
            || self.size.0 > MAX_SIZE
            || self.size.1 > MAX_SIZE
        {
            return Err(BuildError::SizeOutOfRange {
                width: self.size.0,
                height: self.size.1,
            });
        }
        if ui::get_screen_by_id(&self.id).is_some() {
            return Err(BuildError::DuplicateScreen {
                id: self.id.clone(),
            });
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for node in &self.widgets {
            if !seen.insert(node.id.as_str()) {
                return Err(BuildError::DuplicateWidget {
                    id: node.id.clone(),
                });
            }
        }
        for node in &self.widgets {
            if let Some(parent) = &node.parent {
                if !self.widgets.iter().any(|held| held.id == *parent) {
                    return Err(BuildError::UnknownParent {
                        widget: node.id.clone(),
                        parent: parent.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn to_screen(&self) -> Screen {
        let mut screen = Screen::new(&self.id, &self.title)
            .layout(self.layout)
            .theme(&self.theme);
        screen.style = self.background.clone();
        for node in &self.widgets {
            screen.widgets.push(node.to_widget());
        }
        screen
    }

    fn record_handlers(&self) {
        with_handlers(|held| {
            held.insert(self.id.clone(), self.handlers.clone());
        });
    }
}

type HandlerMap = BTreeMap<String, Vec<(String, String)>>;

fn handlers() -> &'static Mutex<HandlerMap> {
    static HANDLERS: OnceLock<Mutex<HandlerMap>> = OnceLock::new();
    HANDLERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_handlers<T>(body: impl FnOnce(&mut HandlerMap) -> T) -> T {
    let mut guard = handlers().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The handlers a built screen named, in the order they were added.
#[must_use]
pub fn get_handlers(screen: &str) -> Vec<(String, String)> {
    with_handlers(|held| held.get(screen).cloned().unwrap_or_default())
}

/// Forgets a built screen's handlers.
pub fn clear_handlers(screen: &str) -> bool {
    with_handlers(|held| held.remove(screen).is_some())
}

fn refused(id: &str, what: &str) -> bool {
    crate::warn!(
        "a mod asked this host to show the screen \"{id}\": {}",
        why_unknown(what)
    );
    false
}

/// Why showing a built screen on the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "show" | "screen" => {
            "the client's screen layer draws its own screens and takes its own input; this \
             host owns the declaration and the geometry a mod computes, not the showing"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["show"]
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

    #[test]
    fn a_builder_builds_and_registers_a_real_screen() {
        let _serial = serial();
        ui::unregister_screen("s79:menu");
        let builder = ScreenBuilder::new()
            .id("s79:menu")
            .title("Menu")
            .size(320, 240)
            .layout(Layout::Vertical)
            .theme("hello:dark")
            .background("hello:panel")
            .add_text("s79:title", "Welcome")
            .add_button("s79:play", "Play");
        let screen = builder.build().expect("it builds");
        assert_eq!(screen.title, "Menu");
        assert_eq!(builder.pixel_size(), (320, 240));
        assert_eq!(screen.theme, "hello:dark");
        assert_eq!(screen.style, "hello:panel");
        assert_eq!(screen.layout, Layout::Vertical);
        assert_eq!(
            screen.widgets.iter().map(|widget| widget.id.as_str()).collect::<Vec<_>>(),
            vec!["s79:title", "s79:play"]
        );
        assert_eq!(screen.widget_of("s79:play").map(|widget| widget.kind), Some(ui::WidgetKind::Button));
        assert_eq!(
            ui::get_screen_by_id("s79:menu").map(|held| held.title),
            Some("Menu".to_owned()),
            "it is really registered"
        );
        ui::unregister_screen("s79:menu");
    }

    #[test]
    fn the_layout_places_the_children_exactly() {
        let builder = ScreenBuilder::new()
            .id("s79:placed")
            .title("Placed")
            .size(320, 240)
            .layout(Layout::Vertical)
            .add_text("s79:a", "A")
            .add_button("s79:b", "B");
        let placed = builder.place();
        assert_eq!(placed.len(), 2);
        assert_eq!(placed[0], ("s79:a".to_owned(), (4.0, 4.0, 312.0, 12.0)));
        assert_eq!(placed[1], ("s79:b".to_owned(), (4.0, 18.0, 120.0, 20.0)));
    }

    #[test]
    fn an_empty_id_is_refused() {
        let error = ScreenBuilder::new().title("No id").build().unwrap_err();
        assert_eq!(error, BuildError::MissingId);
        assert!(error.to_string().contains("non-empty"));
        assert_eq!(
            ScreenBuilder::new().id("   ").title("Blank").build().unwrap_err(),
            BuildError::MissingId
        );
    }

    #[test]
    fn a_duplicate_screen_id_is_refused() {
        let _serial = serial();
        ui::unregister_screen("s79:dupe");
        ScreenBuilder::new().id("s79:dupe").title("One").build().expect("the first");
        let error = ScreenBuilder::new().id("s79:dupe").title("Two").build().unwrap_err();
        assert_eq!(error, BuildError::DuplicateScreen { id: "s79:dupe".to_owned() });
        assert!(error.to_string().contains("s79:dupe"));
        ui::unregister_screen("s79:dupe");
    }

    #[test]
    fn a_duplicate_widget_id_is_refused() {
        let _serial = serial();
        let error = ScreenBuilder::new()
            .id("s79:dup-widget")
            .title("Dup")
            .add_button("s79:a", "A")
            .add_text("s79:a", "A again")
            .build()
            .unwrap_err();
        assert_eq!(error, BuildError::DuplicateWidget { id: "s79:a".to_owned() });
        assert!(error.to_string().contains("s79:a"));
        assert_eq!(ui::get_screen_by_id("s79:dup-widget"), None, "nothing was registered");
    }

    #[test]
    fn a_widget_with_an_unknown_parent_is_refused_by_name() {
        let mut node = WidgetNode::new("s79:child", WidgetType::TextLabel, "inside");
        node.parent = Some("s79:panel".to_owned());
        let error = ScreenBuilder::new()
            .id("s79:orphan")
            .title("Orphan")
            .add_widget(node)
            .build()
            .unwrap_err();
        assert_eq!(
            error,
            BuildError::UnknownParent {
                widget: "s79:child".to_owned(),
                parent: "s79:panel".to_owned(),
            }
        );
        assert!(error.to_string().contains("s79:panel"), "it names the parent");
    }

    #[test]
    fn an_out_of_range_size_is_refused() {
        let error = ScreenBuilder::new().id("s79:size").title("Size").size(0, 240).build().unwrap_err();
        assert_eq!(error, BuildError::SizeOutOfRange { width: 0, height: 240 });
        assert!(error.to_string().contains("0x240"));
        let error = ScreenBuilder::new().id("s79:size").title("Size").size(320, 5000).build().unwrap_err();
        assert_eq!(error, BuildError::SizeOutOfRange { width: 320, height: 5000 });
    }

    #[test]
    fn the_handlers_are_registered_in_order() {
        let _serial = serial();
        ui::unregister_screen("s79:handlers");
        with_handlers(|held| held.remove("s79:handlers"));
        ScreenBuilder::new()
            .id("s79:handlers")
            .title("Handlers")
            .on_open("hello:greet")
            .on_close("hello:bye")
            .on_tick("hello:beat")
            .build()
            .expect("it builds");
        assert_eq!(
            get_handlers("s79:handlers"),
            vec![
                ("open".to_owned(), "hello:greet".to_owned()),
                ("close".to_owned(), "hello:bye".to_owned()),
                ("tick".to_owned(), "hello:beat".to_owned()),
            ]
        );
        assert!(clear_handlers("s79:handlers"));
        assert_eq!(get_handlers("s79:handlers"), Vec::<(String, String)>::new());
        ui::unregister_screen("s79:handlers");
    }

    #[test]
    fn showing_a_built_screen_on_the_client_is_refused() {
        let builder = ScreenBuilder::new().id("s79:show").title("Show");
        assert!(!builder.show());
        assert_eq!(refusals().len(), 1);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("show").contains("client's screen layer"));
        assert!(why_unknown("screen").contains("client's screen layer"));
    }
}
