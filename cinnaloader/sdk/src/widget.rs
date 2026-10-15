//! GUI widgets: the node tree a mod builds, and the client's screen that would draw it.
//!
//! A mod can build a whole widget tree here. A node has an id, an owner, a kind from the
//! catalogue in [`WidgetType`], a label, a value, an icon, a tooltip, a flag per trait
//! (visible, enabled, focusable, draggable, resizable, clippable), a position and a size
//! clamped by [`WidgetNode::min_size`] and [`WidgetNode::max_size`], a margin, a padding, a
//! border, a background, an opacity, a rotation, a scale, a cursor, a layout and constraints.
//! The tree is real: [`add_child`] and [`remove_child`] keep the parent link and the child
//! list in step, [`remove_all_children`] clears one side of it, [`unregister`] detaches a
//! node that goes away, and a cycle is refused rather than created. The margin, border and
//! padding really move the node's rectangles, and the layout really places its children.
//! [`set_data`] is a keyed store and [`bind`] makes one key read through to another node.
//!
//! The twelve events are real registrations with a dispatch path a mod can call itself:
//! [`on_click`] names a handler, [`dispatch`] runs the handlers for an event and records
//! what it ran, and [`set_value`] raises the change event only when the value moves. What a
//! mod cannot do is put any of it on the client's screen: drawing a widget, the client's
//! pointer and keyboard delivery, its theme, its native dialogs and file pickers, its input
//! method editor and its scrolling are all inside the client. Those are refused with their
//! reasons, and the module says which parts are bookkeeping a mod drives itself.
//!
//! ```
//! use cinnaloader::widget::{self, WidgetNode, WidgetType};
//!
//! assert!(widget::register(
//!     WidgetNode::new("hello:panel", WidgetType::Panel, "").sized(200.0, 100.0)
//! ));
//! assert!(widget::register(WidgetNode::new("hello:label", WidgetType::TextLabel, "Hello")));
//! assert!(widget::add_child("hello:panel", "hello:label"));
//! assert_eq!(widget::get_children("hello:panel"), vec!["hello:label".to_owned()]);
//! assert_eq!(widget::get_parent("hello:label"), Some("hello:panel".to_owned()));
//! assert!(widget::set_min_size("hello:label", 40.0, 10.0));
//! assert!(widget::set_size("hello:label", 10.0, 4.0));
//! assert_eq!(widget::get_node("hello:label").map(|node| node.size), Some((40.0, 10.0)));
//! assert!(widget::on_click("hello:label", "hello:greet"));
//! assert_eq!(widget::dispatch("hello:label", widget::WidgetEvent::Click, ""), 1);
//! assert!(!widget::draw("hello:label"), "the client draws widgets");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::ui::Layout;

/// A rectangle, as `(x, y, width, height)`.
pub type Rect = (f32, f32, f32, f32);

/// Every kind of widget the library names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum WidgetType {
    #[default]
    Widget,
    Container,
    Panel,
    Window,
    Modal,
    Dialog,
    Popup,
    Tooltip,
    ContextMenu,
    MenuBar,
    MenuItem,
    Toolbar,
    TabView,
    Tab,
    ScrollView,
    ListView,
    GridView,
    TreeView,
    TableView,
    DataGrid,
    TextLabel,
    RichText,
    TextInput,
    MultilineInput,
    PasswordInput,
    NumberInput,
    Slider,
    ProgressBar,
    CircularProgress,
    Button,
    IconButton,
    ToggleButton,
    Checkbox,
    RadioButton,
    RadioGroup,
    Dropdown,
    ComboBox,
    SearchBox,
    ColorPicker,
    DatePicker,
    TimePicker,
    KeybindInput,
    FilePicker,
    DirectoryPicker,
    ImageView,
    ImageButton,
    SpriteView,
    ItemView,
    BlockView,
    EntityPreview,
    ModelViewer,
    CodeEditor,
    Console,
    ChatBox,
    Notification,
    Badge,
    Separator,
    Spacer,
    LoadingSpinner,
    Accordion,
    CollapsiblePanel,
    Breadcrumbs,
    Pagination,
    VirtualList,
    DragHandle,
    ResizeHandle,
}

impl WidgetType {
    /// The whole catalogue, in the order the wishlist names it.
    pub const ALL: [Self; 66] = [
        Self::Widget,
        Self::Container,
        Self::Panel,
        Self::Window,
        Self::Modal,
        Self::Dialog,
        Self::Popup,
        Self::Tooltip,
        Self::ContextMenu,
        Self::MenuBar,
        Self::MenuItem,
        Self::Toolbar,
        Self::TabView,
        Self::Tab,
        Self::ScrollView,
        Self::ListView,
        Self::GridView,
        Self::TreeView,
        Self::TableView,
        Self::DataGrid,
        Self::TextLabel,
        Self::RichText,
        Self::TextInput,
        Self::MultilineInput,
        Self::PasswordInput,
        Self::NumberInput,
        Self::Slider,
        Self::ProgressBar,
        Self::CircularProgress,
        Self::Button,
        Self::IconButton,
        Self::ToggleButton,
        Self::Checkbox,
        Self::RadioButton,
        Self::RadioGroup,
        Self::Dropdown,
        Self::ComboBox,
        Self::SearchBox,
        Self::ColorPicker,
        Self::DatePicker,
        Self::TimePicker,
        Self::KeybindInput,
        Self::FilePicker,
        Self::DirectoryPicker,
        Self::ImageView,
        Self::ImageButton,
        Self::SpriteView,
        Self::ItemView,
        Self::BlockView,
        Self::EntityPreview,
        Self::ModelViewer,
        Self::CodeEditor,
        Self::Console,
        Self::ChatBox,
        Self::Notification,
        Self::Badge,
        Self::Separator,
        Self::Spacer,
        Self::LoadingSpinner,
        Self::Accordion,
        Self::CollapsiblePanel,
        Self::Breadcrumbs,
        Self::Pagination,
        Self::VirtualList,
        Self::DragHandle,
        Self::ResizeHandle,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Widget => "widget",
            Self::Container => "container",
            Self::Panel => "panel",
            Self::Window => "window",
            Self::Modal => "modal",
            Self::Dialog => "dialog",
            Self::Popup => "popup",
            Self::Tooltip => "tooltip",
            Self::ContextMenu => "context_menu",
            Self::MenuBar => "menu_bar",
            Self::MenuItem => "menu_item",
            Self::Toolbar => "toolbar",
            Self::TabView => "tab_view",
            Self::Tab => "tab",
            Self::ScrollView => "scroll_view",
            Self::ListView => "list_view",
            Self::GridView => "grid_view",
            Self::TreeView => "tree_view",
            Self::TableView => "table_view",
            Self::DataGrid => "data_grid",
            Self::TextLabel => "text_label",
            Self::RichText => "rich_text",
            Self::TextInput => "text_input",
            Self::MultilineInput => "multiline_input",
            Self::PasswordInput => "password_input",
            Self::NumberInput => "number_input",
            Self::Slider => "slider",
            Self::ProgressBar => "progress_bar",
            Self::CircularProgress => "circular_progress",
            Self::Button => "button",
            Self::IconButton => "icon_button",
            Self::ToggleButton => "toggle_button",
            Self::Checkbox => "checkbox",
            Self::RadioButton => "radio_button",
            Self::RadioGroup => "radio_group",
            Self::Dropdown => "dropdown",
            Self::ComboBox => "combo_box",
            Self::SearchBox => "search_box",
            Self::ColorPicker => "color_picker",
            Self::DatePicker => "date_picker",
            Self::TimePicker => "time_picker",
            Self::KeybindInput => "keybind_input",
            Self::FilePicker => "file_picker",
            Self::DirectoryPicker => "directory_picker",
            Self::ImageView => "image_view",
            Self::ImageButton => "image_button",
            Self::SpriteView => "sprite_view",
            Self::ItemView => "item_view",
            Self::BlockView => "block_view",
            Self::EntityPreview => "entity_preview",
            Self::ModelViewer => "model_viewer",
            Self::CodeEditor => "code_editor",
            Self::Console => "console",
            Self::ChatBox => "chat_box",
            Self::Notification => "notification",
            Self::Badge => "badge",
            Self::Separator => "separator",
            Self::Spacer => "spacer",
            Self::LoadingSpinner => "loading_spinner",
            Self::Accordion => "accordion",
            Self::CollapsiblePanel => "collapsible_panel",
            Self::Breadcrumbs => "breadcrumbs",
            Self::Pagination => "pagination",
            Self::VirtualList => "virtual_list",
            Self::DragHandle => "drag_handle",
            Self::ResizeHandle => "resize_handle",
        }
    }

    /// Whether the kind shows a label a caller sets with [`set_text`].
    #[must_use]
    pub fn accepts_text(self) -> bool {
        matches!(
            self,
            Self::Widget
                | Self::Panel
                | Self::Window
                | Self::Modal
                | Self::Dialog
                | Self::Tooltip
                | Self::MenuItem
                | Self::Tab
                | Self::TextLabel
                | Self::RichText
                | Self::TextInput
                | Self::MultilineInput
                | Self::PasswordInput
                | Self::NumberInput
                | Self::Button
                | Self::IconButton
                | Self::ToggleButton
                | Self::Checkbox
                | Self::RadioButton
                | Self::Dropdown
                | Self::ComboBox
                | Self::SearchBox
                | Self::ColorPicker
                | Self::DatePicker
                | Self::TimePicker
                | Self::KeybindInput
                | Self::FilePicker
                | Self::DirectoryPicker
                | Self::CodeEditor
                | Self::Console
                | Self::ChatBox
                | Self::Notification
                | Self::Badge
                | Self::Breadcrumbs
                | Self::Pagination
        )
    }

    /// Whether the kind holds other nodes.
    #[must_use]
    pub fn accepts_children(self) -> bool {
        matches!(
            self,
            Self::Widget
                | Self::Container
                | Self::Panel
                | Self::Window
                | Self::Modal
                | Self::Dialog
                | Self::Popup
                | Self::Tooltip
                | Self::ContextMenu
                | Self::MenuBar
                | Self::Toolbar
                | Self::TabView
                | Self::Tab
                | Self::ScrollView
                | Self::ListView
                | Self::GridView
                | Self::TreeView
                | Self::TableView
                | Self::DataGrid
                | Self::RadioGroup
                | Self::ComboBox
                | Self::Accordion
                | Self::CollapsiblePanel
                | Self::VirtualList
                | Self::Breadcrumbs
                | Self::Pagination
        )
    }

    /// Whether the kind carries a value a caller sets with [`set_value`].
    #[must_use]
    pub fn takes_value(self) -> bool {
        matches!(
            self,
            Self::Slider
                | Self::NumberInput
                | Self::TextInput
                | Self::MultilineInput
                | Self::PasswordInput
                | Self::Dropdown
                | Self::ComboBox
                | Self::SearchBox
                | Self::ColorPicker
                | Self::DatePicker
                | Self::TimePicker
                | Self::KeybindInput
                | Self::Checkbox
                | Self::ToggleButton
                | Self::RadioButton
                | Self::ProgressBar
                | Self::CircularProgress
                | Self::ScrollView
                | Self::TabView
                | Self::Pagination
        )
    }

    /// The coarse kind the screen layer in [`crate::ui`] understands.
    #[must_use]
    pub fn ui_kind(self) -> crate::ui::WidgetKind {
        match self {
            Self::Button | Self::IconButton | Self::ImageButton => crate::ui::WidgetKind::Button,
            Self::ToggleButton | Self::Checkbox | Self::RadioButton => crate::ui::WidgetKind::Toggle,
            Self::Slider | Self::NumberInput => crate::ui::WidgetKind::Slider,
            Self::TextInput
            | Self::MultilineInput
            | Self::PasswordInput
            | Self::SearchBox
            | Self::KeybindInput
            | Self::CodeEditor
            | Self::Console
            | Self::ChatBox => crate::ui::WidgetKind::TextBox,
            Self::Dropdown | Self::ComboBox => crate::ui::WidgetKind::Dropdown,
            Self::ListView
            | Self::GridView
            | Self::TreeView
            | Self::TableView
            | Self::DataGrid
            | Self::VirtualList
            | Self::ScrollView => crate::ui::WidgetKind::List,
            Self::Panel
            | Self::Container
            | Self::Window
            | Self::Modal
            | Self::Dialog
            | Self::Popup
            | Self::TabView
            | Self::Tab
            | Self::Accordion
            | Self::CollapsiblePanel => crate::ui::WidgetKind::Panel,
            Self::ImageView
            | Self::SpriteView
            | Self::ItemView
            | Self::BlockView
            | Self::EntityPreview
            | Self::ModelViewer => crate::ui::WidgetKind::Image,
            Self::ProgressBar | Self::CircularProgress | Self::LoadingSpinner => {
                crate::ui::WidgetKind::ProgressBar
            }
            Self::Spacer | Self::Separator | Self::DragHandle | Self::ResizeHandle => {
                crate::ui::WidgetKind::Spacer
            }
            _ => crate::ui::WidgetKind::Label,
        }
    }
}

/// The four edges of a box, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Edges {
    #[must_use]
    pub fn zero() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn all(value: f32) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }

    #[must_use]
    pub fn sides(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    #[must_use]
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    #[must_use]
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// What a node will not shrink below or grow past, and how much of the spare room it wants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Constraints {
    pub min: (f32, f32),
    pub max: (f32, f32),
    pub weight: f32,
}

impl Default for Constraints {
    fn default() -> Self {
        Self {
            min: (0.0, 0.0),
            max: (f32::MAX, f32::MAX),
            weight: 0.0,
        }
    }
}

impl Constraints {
    #[must_use]
    pub fn new(min: (f32, f32), max: (f32, f32)) -> Self {
        Self {
            min,
            max,
            weight: 0.0,
        }
    }

    #[must_use]
    pub fn fixed(width: f32, height: f32) -> Self {
        Self {
            min: (width, height),
            max: (width, height),
            weight: 0.0,
        }
    }

    #[must_use]
    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }

    /// Holds a size inside the bounds, with the upper bound raised to meet a lower one.
    #[must_use]
    pub fn clamp(&self, size: (f32, f32)) -> (f32, f32) {
        let x = size.0.clamp(self.min.0, self.max.0.max(self.min.0));
        let y = size.1.clamp(self.min.1, self.max.1.max(self.min.1));
        (x, y)
    }
}

/// An event a widget can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WidgetEvent {
    Click,
    DoubleClick,
    RightClick,
    Hover,
    Focus,
    Blur,
    Change,
    Submit,
    DragStart,
    Drag,
    DragEnd,
    Scroll,
}

impl WidgetEvent {
    /// The twelve events, in the order the wishlist names them.
    pub const ALL: [Self; 12] = [
        Self::Click,
        Self::DoubleClick,
        Self::RightClick,
        Self::Hover,
        Self::Focus,
        Self::Blur,
        Self::Change,
        Self::Submit,
        Self::DragStart,
        Self::Drag,
        Self::DragEnd,
        Self::Scroll,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::DoubleClick => "double_click",
            Self::RightClick => "right_click",
            Self::Hover => "hover",
            Self::Focus => "focus",
            Self::Blur => "blur",
            Self::Change => "change",
            Self::Submit => "submit",
            Self::DragStart => "drag_start",
            Self::Drag => "drag",
            Self::DragEnd => "drag_end",
            Self::Scroll => "scroll",
        }
    }

    /// The matching `on_*` registration, for a caller that walks the list.
    #[must_use]
    pub fn handler_name(self) -> &'static str {
        match self {
            Self::Click => "on_click",
            Self::DoubleClick => "on_double_click",
            Self::RightClick => "on_right_click",
            Self::Hover => "on_hover",
            Self::Focus => "on_focus",
            Self::Blur => "on_blur",
            Self::Change => "on_change",
            Self::Submit => "on_submit",
            Self::DragStart => "on_drag_start",
            Self::Drag => "on_drag",
            Self::DragEnd => "on_drag_end",
            Self::Scroll => "on_scroll",
        }
    }
}

/// One node in a mod's widget tree.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetNode {
    pub id: String,
    pub owner: String,
    pub kind: WidgetType,
    pub text: String,
    pub value: String,
    pub icon: String,
    pub tooltip: String,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub position: (f32, f32),
    pub size: (f32, f32),
    pub min_size: (f32, f32),
    pub max_size: (f32, f32),
    pub margin: Edges,
    pub padding: Edges,
    pub border: Edges,
    pub background: String,
    pub opacity: f32,
    pub rotation: f32,
    pub scale: f32,
    pub cursor: String,
    pub visible: bool,
    pub enabled: bool,
    pub focusable: bool,
    pub draggable: bool,
    pub resizable: bool,
    pub clippable: bool,
    pub layout: Layout,
    pub spacing: f32,
    pub constraints: Constraints,
    pub data: BTreeMap<String, String>,
    pub bindings: BTreeMap<String, String>,
    pub handlers: BTreeMap<WidgetEvent, Vec<String>>,
}

impl WidgetNode {
    #[must_use]
    pub fn new(id: &str, kind: WidgetType, text: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            kind,
            text: text.to_owned(),
            value: String::new(),
            icon: String::new(),
            tooltip: String::new(),
            parent: None,
            children: Vec::new(),
            position: (0.0, 0.0),
            size: (0.0, 0.0),
            min_size: (0.0, 0.0),
            max_size: (f32::MAX, f32::MAX),
            margin: Edges::zero(),
            padding: Edges::zero(),
            border: Edges::zero(),
            background: String::new(),
            opacity: 1.0,
            rotation: 0.0,
            scale: 1.0,
            cursor: "default".to_owned(),
            visible: true,
            enabled: true,
            focusable: matches!(kind, WidgetType::Button | WidgetType::TextInput | WidgetType::Slider),
            draggable: false,
            resizable: false,
            clippable: false,
            layout: Layout::Absolute,
            spacing: 0.0,
            constraints: Constraints::default(),
            data: BTreeMap::new(),
            bindings: BTreeMap::new(),
            handlers: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn at(mut self, x: f32, y: f32) -> Self {
        self.position = (x, y);
        self
    }

    #[must_use]
    pub fn sized(mut self, width: f32, height: f32) -> Self {
        self.size = self.clamp_size((width, height));
        self
    }

    #[must_use]
    pub fn with_margin(mut self, margin: Edges) -> Self {
        self.margin = margin;
        self
    }

    #[must_use]
    pub fn with_padding(mut self, padding: Edges) -> Self {
        self.padding = padding;
        self
    }

    #[must_use]
    pub fn with_border(mut self, border: Edges) -> Self {
        self.border = border;
        self
    }

    /// A size held between the node's own bounds and its constraints.
    #[must_use]
    pub fn clamp_size(&self, size: (f32, f32)) -> (f32, f32) {
        let min = (
            self.min_size.0.max(self.constraints.min.0),
            self.min_size.1.max(self.constraints.min.1),
        );
        let max = (
            self.max_size.0.min(self.constraints.max.0),
            self.max_size.1.min(self.constraints.max.1),
        );
        (
            size.0.clamp(min.0, max.0.max(min.0)),
            size.1.clamp(min.1, max.1.max(min.1)),
        )
    }

    /// The top-left of the content box, in the parent's coordinates.
    #[must_use]
    pub fn content_origin(&self) -> (f32, f32) {
        (
            self.position.0 + self.border.left + self.padding.left,
            self.position.1 + self.border.top + self.padding.top,
        )
    }

    /// The size of the content box, never negative.
    #[must_use]
    pub fn content_size(&self) -> (f32, f32) {
        (
            (self.size.0 - self.border.horizontal() - self.padding.horizontal()).max(0.0),
            (self.size.1 - self.border.vertical() - self.padding.vertical()).max(0.0),
        )
    }

    /// The node's own box: where it sits and how big it is.
    #[must_use]
    pub fn border_rect(&self) -> Rect {
        (self.position.0, self.position.1, self.size.0, self.size.1)
    }

    /// The box with the margin added around it.
    #[must_use]
    pub fn margin_rect(&self) -> Rect {
        (
            self.position.0 - self.margin.left,
            self.position.1 - self.margin.top,
            self.size.0 + self.margin.horizontal(),
            self.size.1 + self.margin.vertical(),
        )
    }

    /// The screen-layer widget this node turns into, for a mod that puts it on a screen.
    #[must_use]
    pub fn to_widget(&self) -> crate::ui::Widget {
        let mut widget = crate::ui::Widget::new(&self.id, self.kind.ui_kind(), &self.text);
        widget.owner = self.owner.clone();
        widget.x = self.position.0;
        widget.y = self.position.1;
        widget.width = self.size.0;
        widget.height = self.size.1;
        widget.parent = self.parent.clone();
        widget.visible = self.visible;
        widget.enabled = self.enabled;
        widget.focusable = self.focusable || self.kind.ui_kind().takes_focus();
        widget.tooltip = self.tooltip.clone();
        widget
    }
}

impl fmt::Display for WidgetNode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} '{}' {:.0}x{:.0}{}",
            self.id,
            self.kind.as_str(),
            self.text,
            self.size.0,
            self.size.1,
            if self.visible { "" } else { " hidden" }
        )
    }
}

/// One handler a dispatch ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
    pub node: String,
    pub event: WidgetEvent,
    pub handler: String,
    pub value: String,
}

fn nodes() -> &'static Mutex<BTreeMap<String, WidgetNode>> {
    static NODES: OnceLock<Mutex<BTreeMap<String, WidgetNode>>> = OnceLock::new();
    NODES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_nodes<T>(body: impl FnOnce(&mut BTreeMap<String, WidgetNode>) -> T) -> T {
    let mut guard = nodes().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn dispatches() -> &'static Mutex<Vec<Dispatch>> {
    static DISPATCHES: OnceLock<Mutex<Vec<Dispatch>>> = OnceLock::new();
    DISPATCHES.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_dispatches<T>(body: impl FnOnce(&mut Vec<Dispatch>) -> T) -> T {
    let mut guard = dispatches().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Puts a node in the tree.
pub fn register(node: WidgetNode) -> bool {
    with_nodes(|nodes| nodes.insert(node.id.clone(), node).is_none())
}

/// Takes a node out, detaching it from its parent and orphaning its children.
pub fn unregister(id: &str) -> bool {
    with_nodes(|nodes| {
        let Some(node) = nodes.remove(id) else {
            return false;
        };
        if let Some(parent) = node.parent {
            if let Some(held) = nodes.get_mut(&parent) {
                held.children.retain(|child| child != id);
            }
        }
        for child in &node.children {
            if let Some(held) = nodes.get_mut(child) {
                if held.parent.as_deref() == Some(id) {
                    held.parent = None;
                }
            }
        }
        true
    })
}

/// A node by id.
#[must_use]
pub fn get_node(id: &str) -> Option<WidgetNode> {
    with_nodes(|nodes| nodes.get(id).cloned())
}

/// Builds a node, for a mod that wants to register one by hand.
#[must_use]
pub fn create(id: &str, kind: WidgetType, text: &str) -> WidgetNode {
    WidgetNode::new(id, kind, text)
}

/// A node's id, when the node exists.
#[must_use]
pub fn get_id(id: &str) -> Option<String> {
    with_nodes(|nodes| nodes.get(id).map(|node| node.id.clone()))
}

/// The node it sits inside.
#[must_use]
pub fn get_parent(id: &str) -> Option<String> {
    with_nodes(|nodes| nodes.get(id).and_then(|node| node.parent.clone()))
}

/// The nodes that sit inside it, in the order they were added.
#[must_use]
pub fn get_children(id: &str) -> Vec<String> {
    with_nodes(|nodes| {
        nodes
            .get(id)
            .map(|node| node.children.clone())
            .unwrap_or_default()
    })
}

/// Makes one node the parent of another, refusing a link that would make a cycle.
pub fn add_child(parent: &str, child: &str) -> bool {
    with_nodes(|nodes| {
        if parent == child {
            return false;
        }
        // walking up from the parent catches both a self-parent and an ancestor, which is
        // exactly the link that would make a cycle.
        let mut walk = Some(parent.to_owned());
        let mut steps = 0;
        while let Some(id) = walk {
            if id == child {
                return false;
            }
            steps += 1;
            if steps > 4096 {
                return false;
            }
            walk = nodes.get(&id).and_then(|node| node.parent.clone());
        }
        let Some(held) = nodes.get(parent) else {
            return false;
        };
        if held.children.iter().any(|existing| existing == child) {
            return false;
        }
        if !nodes.contains_key(child) {
            return false;
        }
        if let Some(previous) = nodes.get(child).and_then(|node| node.parent.clone()) {
            if let Some(held) = nodes.get_mut(&previous) {
                held.children.retain(|held| held != child);
            }
        }
        if let Some(held) = nodes.get_mut(parent) {
            held.children.push(child.to_owned());
        }
        if let Some(held) = nodes.get_mut(child) {
            held.parent = Some(parent.to_owned());
        }
        true
    })
}

/// Takes a child out of a parent, clearing the link both ways.
pub fn remove_child(parent: &str, child: &str) -> bool {
    with_nodes(|nodes| {
        let Some(held) = nodes.get_mut(parent) else {
            return false;
        };
        let before = held.children.len();
        held.children.retain(|existing| existing != child);
        let removed = held.children.len() != before;
        if removed {
            if let Some(node) = nodes.get_mut(child) {
                if node.parent.as_deref() == Some(parent) {
                    node.parent = None;
                }
            }
        }
        removed
    })
}

/// Empties a parent, and answers how many children it let go.
pub fn remove_all_children(parent: &str) -> usize {
    with_nodes(|nodes| {
        let Some(held) = nodes.get_mut(parent) else {
            return 0;
        };
        let children: Vec<String> = std::mem::take(&mut held.children);
        let count = children.len();
        for child in &children {
            if let Some(node) = nodes.get_mut(child) {
                if node.parent.as_deref() == Some(parent) {
                    node.parent = None;
                }
            }
        }
        count
    })
}

/// A node's rectangle on the screen, with every ancestor's content offset and its own margin
/// counted in.
#[must_use]
pub fn get_rect(id: &str) -> Option<Rect> {
    with_nodes(|nodes| {
        let node = nodes.get(id)?;
        let (mut x, mut y) = (node.position.0, node.position.1);
        let mut walk = node.parent.clone();
        let mut steps = 0;
        while let Some(parent) = walk {
            steps += 1;
            if steps > 4096 {
                break;
            }
            let Some(held) = nodes.get(&parent) else {
                break;
            };
            let (ox, oy) = held.content_origin();
            x += ox;
            y += oy;
            walk = held.parent.clone();
        }
        Some((
            x - node.margin.left,
            y - node.margin.top,
            node.size.0 + node.margin.horizontal(),
            node.size.1 + node.margin.vertical(),
        ))
    })
}

/// Places a node's children by its layout and answers each child's box, in the parent's
/// coordinates.
#[must_use]
pub fn place(parent: &str) -> Vec<(String, Rect)> {
    with_nodes(|nodes| {
        let Some(held) = nodes.get(parent) else {
            return Vec::new();
        };
        let (ox, oy) = held.content_origin();
        let (cw, ch) = held.content_size();
        let layout = held.layout;
        let spacing = held.spacing;
        let kids: Vec<&WidgetNode> = held
            .children
            .iter()
            .filter_map(|id| nodes.get(id))
            .collect();
        let mut placed = Vec::new();
        match layout {
            Layout::Absolute => {
                for kid in &kids {
                    placed.push((
                        kid.id.clone(),
                        (ox + kid.position.0, oy + kid.position.1, kid.size.0, kid.size.1),
                    ));
                }
            }
            Layout::Vertical => {
                let total: f32 = kids.iter().map(|kid| kid.constraints.weight.max(0.0)).sum();
                let used = kids.iter().map(|kid| kid.size.1).sum::<f32>()
                    + spacing * kids.len().saturating_sub(1) as f32;
                let spare = (ch - used).max(0.0);
                let mut cursor = oy;
                for kid in &kids {
                    let width = if kid.size.0 > 0.0 { kid.size.0 } else { cw };
                    let mut height = kid.size.1;
                    if total > 0.0 && kid.constraints.weight > 0.0 {
                        height += spare * kid.constraints.weight / total;
                    }
                    placed.push((kid.id.clone(), (ox, cursor, width, height)));
                    cursor += height + spacing;
                }
            }
            Layout::Horizontal => {
                let total: f32 = kids.iter().map(|kid| kid.constraints.weight.max(0.0)).sum();
                let used = kids.iter().map(|kid| kid.size.0).sum::<f32>()
                    + spacing * kids.len().saturating_sub(1) as f32;
                let spare = (cw - used).max(0.0);
                let mut cursor = ox;
                for kid in &kids {
                    let height = if kid.size.1 > 0.0 { kid.size.1 } else { ch };
                    let mut width = kid.size.0;
                    if total > 0.0 && kid.constraints.weight > 0.0 {
                        width += spare * kid.constraints.weight / total;
                    }
                    placed.push((kid.id.clone(), (cursor, oy, width, height)));
                    cursor += width + spacing;
                }
            }
            Layout::Grid(columns) => {
                let columns = columns.max(1);
                let cell = (cw / columns as f32).max(0.0);
                for (index, kid) in kids.iter().enumerate() {
                    let column = index % columns;
                    let row = index / columns;
                    let x = ox + cell * column as f32;
                    let y = oy + (kid.size.1 + spacing) * row as f32;
                    placed.push((
                        kid.id.clone(),
                        (x, y, (cell - spacing).max(0.0), kid.size.1),
                    ));
                }
            }
        }
        placed
    })
}

/// Sets the label a kind that [`WidgetType::accepts_text`] shows.
pub fn set_text(id: &str, text: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.text = text.to_owned();
            true
        }
        None => false,
    })
}

/// The label.
#[must_use]
pub fn get_text(id: &str) -> Option<String> {
    with_nodes(|nodes| nodes.get(id).map(|node| node.text.clone()))
}

/// Sets the icon name a node draws beside its label.
pub fn set_icon(id: &str, icon: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.icon = icon.to_owned();
            true
        }
        None => false,
    })
}

/// Sets the tooltip text.
pub fn set_tooltip(id: &str, tooltip: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.tooltip = tooltip.to_owned();
            true
        }
        None => false,
    })
}

/// Shows or hides a node.
pub fn set_visible(id: &str, visible: bool) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.visible = visible;
            true
        }
        None => false,
    })
}

/// Enables or disables it.
pub fn set_enabled(id: &str, enabled: bool) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.enabled = enabled;
            true
        }
        None => false,
    })
}

/// The same as `set_enabled(id, !disabled)`.
pub fn set_disabled(id: &str, disabled: bool) -> bool {
    set_enabled(id, !disabled)
}

/// Moves a node, in its parent's coordinates.
pub fn set_position(id: &str, x: f32, y: f32) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.position = (x, y);
            true
        }
        None => false,
    })
}

/// Sizes a node, held between its min and max.
pub fn set_size(id: &str, width: f32, height: f32) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.size = node.clamp_size((width, height));
            true
        }
        None => false,
    })
}

/// Raises the floor a size request is held above, and re-holds the current size.
pub fn set_min_size(id: &str, width: f32, height: f32) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.min_size = (width.max(0.0), height.max(0.0));
            node.size = node.clamp_size(node.size);
            true
        }
        None => false,
    })
}

/// Lowers the ceiling a size request is held under, and re-holds the current size.
pub fn set_max_size(id: &str, width: f32, height: f32) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.max_size = (width.max(0.0), height.max(0.0));
            node.size = node.clamp_size(node.size);
            true
        }
        None => false,
    })
}

/// Sets the space around a node, outside its border.
pub fn set_margin(id: &str, margin: Edges) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.margin = margin;
            true
        }
        None => false,
    })
}

/// Sets the space inside a node's border, around its content.
pub fn set_padding(id: &str, padding: Edges) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.padding = padding;
            true
        }
        None => false,
    })
}

/// Sets the width of the border between the padding and the margin.
pub fn set_border(id: &str, border: Edges) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.border = border;
            true
        }
        None => false,
    })
}

/// Sets the background colour, as a name the client would resolve.
pub fn set_background(id: &str, background: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.background = background.to_owned();
            true
        }
        None => false,
    })
}

/// Sets how opaque a node is, held between 0 and 1.
pub fn set_opacity(id: &str, opacity: f32) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.opacity = opacity.clamp(0.0, 1.0);
            true
        }
        None => false,
    })
}

/// Turns a node, in degrees.
pub fn set_rotation(id: &str, degrees: f32) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.rotation = degrees;
            true
        }
        None => false,
    })
}

/// Scales a node about its origin, never below zero.
pub fn set_scale(id: &str, scale: f32) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.scale = scale.max(0.0);
            true
        }
        None => false,
    })
}

/// Sets the cursor name a caller draws while it is over the node.
pub fn set_cursor(id: &str, cursor: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.cursor = cursor.to_owned();
            true
        }
        None => false,
    })
}

/// Sets whether the node can hold focus.
pub fn set_focusable(id: &str, focusable: bool) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.focusable = focusable;
            true
        }
        None => false,
    })
}

/// Sets whether the node can be dragged.
pub fn set_draggable(id: &str, draggable: bool) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.draggable = draggable;
            true
        }
        None => false,
    })
}

/// Sets whether the node can be resized.
pub fn set_resizable(id: &str, resizable: bool) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.resizable = resizable;
            true
        }
        None => false,
    })
}

/// Sets whether the node clips its children.
pub fn set_clippable(id: &str, clippable: bool) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.clippable = clippable;
            true
        }
        None => false,
    })
}

/// Sets how the node places its children.
pub fn set_layout(id: &str, layout: Layout) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.layout = layout;
            true
        }
        None => false,
    })
}

/// Sets the gap the node leaves between the children it places.
pub fn set_spacing(id: &str, spacing: f32) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.spacing = spacing.max(0.0);
            true
        }
        None => false,
    })
}

/// Sets what the node will not shrink below or grow past, and re-holds the current size.
pub fn set_constraints(id: &str, constraints: Constraints) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.constraints = constraints;
            node.size = node.clamp_size(node.size);
            true
        }
        None => false,
    })
}

/// Stores a value under a key on the node.
pub fn set_data(id: &str, key: &str, value: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.data.insert(key.to_owned(), value.to_owned());
            true
        }
        None => false,
    })
}

/// A value on the node, following its binding when one is set.
#[must_use]
pub fn get_data(id: &str, key: &str) -> Option<String> {
    with_nodes(|nodes| -> Option<String> {
        let node = nodes.get(id)?;
        if let Some(value) = node.data.get(key) {
            return Some(value.clone());
        }
        let source = node.bindings.get(key).and_then(|source| nodes.get(source))?;
        if let Some(value) = source.data.get(key) {
            return Some(value.clone());
        }
        if source.kind.takes_value() && !source.value.is_empty() {
            return Some(source.value.clone());
        }
        if source.text.is_empty() {
            return None;
        }
        Some(source.text.clone())
    })
}

/// Links a key on one node to read through to another node.
pub fn bind(id: &str, key: &str, source: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            node.bindings.insert(key.to_owned(), source.to_owned());
            true
        }
        None => false,
    })
}

/// Breaks a binding, and answers whether there was one.
pub fn unbind(id: &str, key: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => node.bindings.remove(key).is_some(),
        None => false,
    })
}

/// Sets a node's value, raising the change event only when it actually moves.
pub fn set_value(id: &str, value: &str) -> bool {
    let changed = with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            if node.value == value {
                false
            } else {
                node.value = value.to_owned();
                true
            }
        }
        None => false,
    });
    if changed {
        dispatch(id, WidgetEvent::Change, value);
    }
    changed
}

/// A node's value.
#[must_use]
pub fn get_value(id: &str) -> Option<String> {
    with_nodes(|nodes| nodes.get(id).map(|node| node.value.clone()))
}

fn on(id: &str, event: WidgetEvent, handler: &str) -> bool {
    with_nodes(|nodes| match nodes.get_mut(id) {
        Some(node) => {
            let list = node.handlers.entry(event).or_default();
            if !list.iter().any(|held| held == handler) {
                list.push(handler.to_owned());
            }
            true
        }
        None => false,
    })
}

/// Names a handler for a click.
pub fn on_click(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::Click, handler)
}

/// Names one for a double click.
pub fn on_double_click(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::DoubleClick, handler)
}

/// Names one for a right click.
pub fn on_right_click(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::RightClick, handler)
}

/// Names one for the pointer moving onto the node.
pub fn on_hover(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::Hover, handler)
}

/// Names one for the node taking focus.
pub fn on_focus(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::Focus, handler)
}

/// Names one for it losing focus.
pub fn on_blur(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::Blur, handler)
}

/// Names one for the value changing.
pub fn on_change(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::Change, handler)
}

/// Names one for the node being submitted.
pub fn on_submit(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::Submit, handler)
}

/// Names one for a drag starting.
pub fn on_drag_start(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::DragStart, handler)
}

/// Names one for a drag.
pub fn on_drag(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::Drag, handler)
}

/// Names one for a drag ending.
pub fn on_drag_end(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::DragEnd, handler)
}

/// Names one for a scroll.
pub fn on_scroll(id: &str, handler: &str) -> bool {
    on(id, WidgetEvent::Scroll, handler)
}

/// Runs every handler a node has for an event, records each one, and answers how many ran.
///
/// The client would call this as it dispatches input; a mod driving its own UI calls it
/// itself.
pub fn dispatch(id: &str, event: WidgetEvent, value: &str) -> usize {
    let handlers = with_nodes(|nodes| {
        nodes
            .get(id)
            .and_then(|node| node.handlers.get(&event))
            .cloned()
            .unwrap_or_default()
    });
    if handlers.is_empty() {
        return 0;
    }
    with_dispatches(|log| {
        for handler in &handlers {
            log.push(Dispatch {
                node: id.to_owned(),
                event,
                handler: handler.clone(),
                value: value.to_owned(),
            });
        }
    });
    handlers.len()
}

/// Submits a node, raising the submit event with its value.
pub fn submit(id: &str) -> bool {
    let value = with_nodes(|nodes| nodes.get(id).map(|node| node.value.clone()));
    match value {
        Some(value) => {
            dispatch(id, WidgetEvent::Submit, &value);
            true
        }
        None => false,
    }
}

/// The dispatches that have run, oldest first.
#[must_use]
pub fn get_dispatches() -> Vec<Dispatch> {
    with_dispatches(|log| log.clone())
}

/// Takes the dispatch log, clearing it.
#[must_use]
pub fn take_dispatches() -> Vec<Dispatch> {
    with_dispatches(std::mem::take)
}

/// Forgets the dispatch log.
pub fn clear_dispatches() {
    with_dispatches(|log| log.clear());
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about a widget: {}", why_unknown(what));
    false
}

/// Draws a widget on the client's screen. `false`: the client's renderer paints it.
pub fn draw(_id: &str) -> bool {
    refused("draw")
}

/// Delivers a real pointer or keyboard event to a widget. `false`.
pub fn deliver_input(_id: &str, _event: WidgetEvent) -> bool {
    refused("input")
}

/// Applies the client's UI theme. `false`.
pub fn apply_client_theme(_theme: &str) -> bool {
    refused("theme")
}

/// Opens a native dialog or file picker. `false`.
pub fn open_native_dialog(_id: &str) -> bool {
    refused("dialog")
}

/// Starts text composition through the client's input method editor. `false`.
pub fn start_ime(_id: &str) -> bool {
    refused("ime")
}

/// Scrolls a view through the client's renderer. `false`.
pub fn scroll_by(_id: &str, _dx: f32, _dy: f32) -> bool {
    refused("scroll")
}

/// Why a widget question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "draw" => {
            "a widget reaches the screen through the client's renderer; this host owns the \
             node tree and the geometry a mod computes, not the painting"
                .to_owned()
        }
        "input" => {
            "real pointer and keyboard events are delivered by the client as it dispatches \
             them; a mod driving its own UI calls dispatch itself"
                .to_owned()
        }
        "theme" => {
            "the client's UI theme belongs to the client; a node's own background, opacity \
             and cursor are the mod's"
                .to_owned()
        }
        "dialog" => {
            "a native dialog or file picker is an operating-system window the client opens, \
             which this host cannot raise"
                .to_owned()
        }
        "ime" => "text composition through an input method editor is run by the client's text field".to_owned(),
        "scroll" => {
            "scrolling a view is performed by the client's renderer; a mod's own loop moves \
             its own offsets"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["draw", "input", "theme", "dialog", "ime", "scroll"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Layout;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_nodes(|nodes| nodes.clear());
        with_dispatches(|log| log.clear());
    }

    fn label(id: &str, text: &str) -> WidgetNode {
        WidgetNode::new(id, WidgetType::TextLabel, text)
    }

    #[test]
    fn a_tree_links_parent_and_children_both_ways() {
        let _serial = serial();
        fresh();
        assert!(register(WidgetNode::new("panel", WidgetType::Panel, "")));
        assert!(register(label("a", "a")));
        assert!(register(label("b", "b")));
        assert!(add_child("panel", "a"));
        assert!(add_child("panel", "b"));
        assert_eq!(get_children("panel"), vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(get_parent("a"), Some("panel".to_owned()));
        assert_eq!(get_id("a"), Some("a".to_owned()));
        assert!(!add_child("panel", "a"), "it is already a child");
        assert!(!add_child("panel", "missing"), "there is no such child");
        assert!(remove_child("panel", "a"));
        assert_eq!(get_parent("a"), None, "a removed child reports no parent");
        assert!(!remove_child("panel", "a"));
        assert_eq!(remove_all_children("panel"), 1);
        assert_eq!(get_children("panel"), Vec::<String>::new());
        assert_eq!(get_parent("b"), None);

        assert!(add_child("panel", "a"));
        assert!(unregister("a"), "deleting a node detaches it");
        assert_eq!(get_children("panel"), Vec::<String>::new());
        assert_eq!(get_parent("a"), None);
        assert_eq!(get_node("a"), None);
    }

    #[test]
    fn a_cycle_cannot_be_created() {
        let _serial = serial();
        fresh();
        assert!(register(label("a", "a")));
        assert!(register(label("b", "b")));
        assert!(register(label("c", "c")));
        assert!(add_child("a", "b"));
        assert!(add_child("b", "c"));
        assert!(!add_child("c", "a"), "a is an ancestor of c");
        assert!(!add_child("a", "a"), "a node is not its own parent");
        assert_eq!(get_parent("a"), None);
        assert_eq!(get_children("a"), vec!["b".to_owned()]);
        assert_eq!(get_children("c"), Vec::<String>::new());
        assert_eq!(get_parent("c"), Some("b".to_owned()));

        // A node can move to another parent without losing its links.
        assert!(register(label("other", "other")));
        assert!(add_child("other", "c"));
        assert_eq!(get_parent("c"), Some("other".to_owned()));
        assert_eq!(get_children("b"), Vec::<String>::new());
    }

    #[test]
    fn a_size_is_clamped_by_min_max_and_constraints() {
        let _serial = serial();
        fresh();
        assert!(register(label("n", "n")));
        assert!(set_min_size("n", 40.0, 10.0));
        assert!(set_size("n", 10.0, 4.0));
        assert_eq!(get_node("n").map(|node| node.size), Some((40.0, 10.0)));
        assert!(set_max_size("n", 100.0, 50.0));
        assert!(set_size("n", 999.0, 999.0));
        assert_eq!(get_node("n").map(|node| node.size), Some((100.0, 50.0)));
        assert!(set_max_size("n", 5.0, 5.0), "a max under the min does not panic");
        assert_eq!(get_node("n").map(|node| node.size), Some((40.0, 10.0)));
        assert!(set_constraints("n", Constraints::fixed(20.0, 20.0)));
        assert_eq!(get_node("n").map(|node| node.size), Some((40.0, 20.0)));
        assert!(!set_min_size("missing", 1.0, 1.0));
        assert!(!set_max_size("missing", 1.0, 1.0));
        assert!(!set_size("missing", 1.0, 1.0));
    }

    #[test]
    fn margin_border_and_padding_move_the_rectangles() {
        let _serial = serial();
        fresh();
        let mut panel = WidgetNode::new("panel", WidgetType::Panel, "");
        panel.size = (100.0, 100.0);
        panel.border = Edges::all(2.0);
        panel.padding = Edges::all(4.0);
        assert!(register(panel));
        let held = get_node("panel").expect("registered");
        assert_eq!(held.content_origin(), (6.0, 6.0));
        assert_eq!(held.content_size(), (88.0, 88.0));
        assert_eq!(held.border_rect(), (0.0, 0.0, 100.0, 100.0));

        assert!(register(label("child", "child").sized(10.0, 5.0)));
        assert!(set_margin("child", Edges::all(1.0)));
        assert!(add_child("panel", "child"));
        assert_eq!(
            place("panel"),
            vec![("child".to_owned(), (6.0, 6.0, 10.0, 5.0))],
            "the child sits at the parent's content origin"
        );
        assert_eq!(
            get_rect("child"),
            Some((5.0, 5.0, 12.0, 7.0)),
            "border plus padding plus the child's own margin"
        );

        assert!(set_layout("panel", Layout::Vertical));
        assert!(set_spacing("panel", 3.0));
        assert!(register(label("second", "second").sized(10.0, 5.0)));
        assert!(add_child("panel", "second"));
        let placed = place("panel");
        assert_eq!(placed[0].1 .1, 6.0);
        assert_eq!(placed[1].1 .1, 6.0 + 5.0 + 3.0, "the second stacks below, past the spacing");
    }

    #[test]
    fn data_is_a_keyed_store() {
        let _serial = serial();
        fresh();
        assert!(register(label("n", "n")));
        assert!(set_data("n", "owner", "hello"));
        assert_eq!(get_data("n", "owner"), Some("hello".to_owned()));
        assert!(set_data("n", "owner", "other"));
        assert_eq!(get_data("n", "owner"), Some("other".to_owned()));
        assert_eq!(get_data("n", "missing"), None);
        assert!(!set_data("ghost", "k", "v"));
    }

    #[test]
    fn a_binding_reads_through_to_its_source_until_it_is_unbound() {
        let _serial = serial();
        fresh();
        assert!(register(label("source", "hi")));
        assert!(register(label("n", "n")));
        assert!(bind("n", "caption", "source"));
        assert_eq!(get_data("n", "caption"), Some("hi".to_owned()));
        assert!(set_text("source", "bye"));
        assert_eq!(get_data("n", "caption"), Some("bye".to_owned()), "it follows the source");
        assert!(set_data("source", "caption", "exact"));
        assert_eq!(get_data("n", "caption"), Some("exact".to_owned()), "a key on the source wins");
        assert!(unbind("n", "caption"));
        assert_eq!(get_data("n", "caption"), None);
        assert!(!unbind("n", "caption"));
    }

    #[test]
    fn a_handler_fires_for_its_event_and_a_change_only_when_the_value_moves() {
        let _serial = serial();
        fresh();
        assert!(register(WidgetNode::new("n", WidgetType::TextInput, "")));
        assert!(on_change("n", "hello:changed"));
        assert!(on_click("n", "hello:clicked"));
        clear_dispatches();

        assert!(!set_value("n", ""), "the value did not move");
        assert_eq!(get_dispatches().len(), 0, "so no change event");
        assert!(set_value("n", "abc"));
        let mut fired = get_dispatches();
        assert_eq!(fired.len(), 1);
        let change = fired.pop().expect("one ran");
        assert_eq!(change.event, WidgetEvent::Change);
        assert_eq!(change.handler, "hello:changed");
        assert_eq!(change.value, "abc");
        clear_dispatches();
        assert!(!set_value("n", "abc"));
        assert_eq!(get_dispatches().len(), 0, "the same value raises nothing");

        assert_eq!(dispatch("n", WidgetEvent::Click, ""), 1);
        assert_eq!(dispatch("n", WidgetEvent::Scroll, ""), 0, "no scroll handler");
        assert!(on_scroll("n", "hello:scrolled"));
        assert_eq!(dispatch("n", WidgetEvent::Scroll, "2"), 1);
        assert_eq!(get_value("n"), Some("abc".to_owned()));
        assert!(on_submit("n", "hello:submitted"));
        assert!(submit("n"));
        assert_eq!(
            get_dispatches().last().map(|held| held.event),
            Some(WidgetEvent::Submit)
        );
        assert!(!on_click("missing", "h"));
    }

    #[test]
    fn a_widget_type_knows_what_it_accepts() {
        let _serial = serial();
        assert_eq!(WidgetType::ALL.len(), 66);
        assert_eq!(WidgetType::default(), WidgetType::Widget);
        assert_eq!(WidgetType::Panel.as_str(), "panel");
        assert_eq!(WidgetType::ContextMenu.as_str(), "context_menu");
        assert_eq!(WidgetType::ResizeHandle.as_str(), "resize_handle");
        assert!(WidgetType::TextLabel.accepts_text());
        assert!(!WidgetType::TextLabel.accepts_children());
        assert!(WidgetType::Container.accepts_children());
        assert!(!WidgetType::Container.accepts_text());
        assert!(WidgetType::Slider.takes_value());
        assert!(!WidgetType::Spacer.takes_value());
        assert_eq!(WidgetType::ALL.len(), WidgetType::ALL.iter().collect::<std::collections::BTreeSet<_>>().len(), "every kind is distinct");
        assert_eq!(
            WidgetNode::new("b", WidgetType::Button, "Go").to_widget().kind,
            crate::ui::WidgetKind::Button
        );
        assert_eq!(WidgetEvent::ALL.len(), 12);
        assert_eq!(WidgetEvent::Change.as_str(), "change");
        assert_eq!(WidgetEvent::Change.handler_name(), "on_change");
    }

    #[test]
    fn the_clients_drawing_input_theme_dialogs_and_scrolling_are_refused() {
        let _serial = serial();
        fresh();
        assert!(!draw("n"));
        assert!(!deliver_input("n", WidgetEvent::Click));
        assert!(!apply_client_theme("minecraft:dark"));
        assert!(!open_native_dialog("n"));
        assert!(!start_ime("n"));
        assert!(!scroll_by("n", 0.0, 3.0));
        assert_eq!(refusals().len(), 6);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("draw").contains("client"));
        assert!(why_unknown("scroll").contains("renderer"));
    }
}
