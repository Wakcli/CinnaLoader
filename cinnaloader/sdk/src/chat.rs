//! Chat and messages: building a message is real work, the client's chat is not.
//!
//! A message here is real data. [`create_text`] and [`create_translation`] build components,
//! [`Style`] carries a colour (a name or a hex), bold, italic, underlined, strikethrough and
//! obfuscated, [`create_click_event`] and [`create_hover_event`] attach an action with its
//! payload, and spans concatenate into one message with [`Message::then`]. Flattening is real
//! too: [`Message::plain_text`] renders the text, and [`Message::spans`] keeps the styling and
//! the events that were on the way down, so a caller can assert on both. A translation
//! substitutes its arguments in order and leaves a placeholder it has no argument for visible
//! rather than dropping it.
//!
//! History is a ring a mod owns: [`MessageHistory`] has a cap and drops the oldest entry one
//! over it, and [`push_history`] keeps one for the mod. Delivery is real in process: a handler
//! registered with [`register_message_handler`] is called with the message, and [`broadcast`]
//! reaches every one of them.
//!
//! What this host cannot do is touch the client's chat. Sending into it, reading its history
//! or its last and selected messages, taking over its HUD or its formatting, and anything that
//! needs its chat screen are refused with a reason in [`why_unknown`], listed by [`refusals`],
//! and never answered with a fake success.
//!
//! ```
//! use cinnaloader::chat::{self, TextColor};
//!
//! let mut style = chat::create_style();
//! chat::set_color(&mut style, TextColor::named("red"));
//! chat::set_bold(&mut style, true);
//!
//! let message = chat::create_text("Hi")
//!     .style(style)
//!     .then(chat::create_text(" there").italic(true));
//! assert_eq!(message.plain_text(), "Hi there");
//! assert_eq!(
//!     message.spans()[1].annotations(),
//!     vec!["color=red".to_owned(), "bold".to_owned(), "italic".to_owned()]
//! );
//! assert!(!chat::send_message(&message), "the client's chat is its own");
//! ```

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// How many messages a mod's default history keeps.
pub const DEFAULT_HISTORY_CAP: usize = 100;

/// A colour a message can be drawn in: one of the named ones, or a packed `0xRRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextColor {
    Named(String),
    Rgb(u32),
}

impl TextColor {
    /// A named colour such as `red` or `dark_purple`.
    #[must_use]
    pub fn named(name: &str) -> Self {
        Self::Named(name.trim().to_ascii_lowercase())
    }

    /// A packed colour, `0xRRGGBB`.
    #[must_use]
    pub fn hex(value: u32) -> Self {
        Self::Rgb(value & 0x00ff_ffff)
    }

    /// The same from three channels.
    #[must_use]
    pub fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::Rgb((u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue))
    }

    /// Reads `#rrggbb` as a hex colour and anything else as a name.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text.strip_prefix('#').and_then(|hex| u32::from_str_radix(hex, 16).ok()) {
            Some(value) => Self::hex(value),
            None => Self::named(text),
        }
    }

    /// Its text form, a name or `#rrggbb`.
    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            Self::Named(name) => name.clone(),
            Self::Rgb(value) => format!("#{value:06x}"),
        }
    }
}

impl fmt::Display for TextColor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_str())
    }
}

/// How a message is drawn, field by field. `None` means "take it from the parent".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Style {
    pub color: Option<TextColor>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underlined: Option<bool>,
    pub strikethrough: Option<bool>,
    pub obfuscated: Option<bool>,
    /// Text a shift-click inserts, when the message has a click event.
    pub insertion: Option<String>,
}

impl Style {
    /// The flags on this style as annotations, in a fixed order.
    #[must_use]
    pub fn annotations(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(color) = &self.color {
            out.push(format!("color={}", color.as_str()));
        }
        for (name, flag) in [
            ("bold", self.bold),
            ("italic", self.italic),
            ("underlined", self.underlined),
            ("strikethrough", self.strikethrough),
            ("obfuscated", self.obfuscated),
        ] {
            if flag == Some(true) {
                out.push(name.to_owned());
            }
        }
        if let Some(insertion) = &self.insertion {
            out.push(format!("insertion={insertion}"));
        }
        out
    }

    fn or(&self, parent: &Self) -> Self {
        Self {
            color: self.color.clone().or_else(|| parent.color.clone()),
            bold: self.bold.or(parent.bold),
            italic: self.italic.or(parent.italic),
            underlined: self.underlined.or(parent.underlined),
            strikethrough: self.strikethrough.or(parent.strikethrough),
            obfuscated: self.obfuscated.or(parent.obfuscated),
            insertion: self.insertion.clone().or_else(|| parent.insertion.clone()),
        }
    }

    fn write_json(&self, out: &mut String) {
        if let Some(color) = &self.color {
            out.push_str(",\"color\":");
            write_json_string(out, &color.as_str());
        }
        for (name, flag) in [
            ("bold", self.bold),
            ("italic", self.italic),
            ("underlined", self.underlined),
            ("strikethrough", self.strikethrough),
            ("obfuscated", self.obfuscated),
        ] {
            if let Some(flag) = flag {
                out.push(',');
                write_json_string(out, name);
                out.push(':');
                out.push_str(if flag { "true" } else { "false" });
            }
        }
        if let Some(insertion) = &self.insertion {
            out.push_str(",\"insertion\":");
            write_json_string(out, insertion);
        }
    }
}

/// What clicking a message does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClickAction {
    OpenUrl,
    RunCommand,
    SuggestCommand,
    ChangePage,
    CopyToClipboard,
    ShowDialog,
    OpenFile,
    Custom(String),
}

impl ClickAction {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::OpenUrl => "open_url",
            Self::RunCommand => "run_command",
            Self::SuggestCommand => "suggest_command",
            Self::ChangePage => "change_page",
            Self::CopyToClipboard => "copy_to_clipboard",
            Self::ShowDialog => "show_dialog",
            Self::OpenFile => "open_file",
            Self::Custom(name) => name.as_str(),
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "open_url" => Self::OpenUrl,
            "run_command" => Self::RunCommand,
            "suggest_command" => Self::SuggestCommand,
            "change_page" => Self::ChangePage,
            "copy_to_clipboard" => Self::CopyToClipboard,
            "show_dialog" => Self::ShowDialog,
            "open_file" => Self::OpenFile,
            other => Self::Custom(other.to_owned()),
        }
    }
}

/// A click action with its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClickEvent {
    pub action: ClickAction,
    pub value: String,
}

/// What hovering a message shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoverAction {
    ShowText,
    ShowItem,
    ShowEntity,
}

impl HoverAction {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ShowText => "show_text",
            Self::ShowItem => "show_item",
            Self::ShowEntity => "show_entity",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "show_item" => Self::ShowItem,
            "show_entity" => Self::ShowEntity,
            _ => Self::ShowText,
        }
    }
}

/// A hover action with its payload, text or an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverEvent {
    pub action: HoverAction,
    pub value: String,
}

impl HoverEvent {
    /// A hover showing text.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self {
            action: HoverAction::ShowText,
            value: text.to_owned(),
        }
    }

    /// A hover showing an item.
    #[must_use]
    pub fn item(item: &str) -> Self {
        Self {
            action: HoverAction::ShowItem,
            value: item.to_owned(),
        }
    }
}

/// What a message carries before its styling: literal text or a translation key.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    Translate {
        key: String,
        args: Vec<Message>,
        /// Named arguments, which are not a vanilla field; the key keeps a note.
        named: Vec<(String, Message)>,
    },
}

/// One piece of a chat message, with its styling, its events and its children.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub content: Content,
    pub style: Style,
    pub click: Option<ClickEvent>,
    pub hover: Option<HoverEvent>,
    pub children: Vec<Message>,
}

impl Message {
    /// A message of literal text.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self::new(Content::Text(text.to_owned()))
    }

    /// A message of a translation key with positional arguments.
    #[must_use]
    pub fn translation(key: &str, args: &[Message]) -> Self {
        Self::new(Content::Translate {
            key: key.to_owned(),
            args: args.to_vec(),
            named: Vec::new(),
        })
    }

    /// A message from any content.
    #[must_use]
    pub fn new(content: Content) -> Self {
        Self {
            content,
            style: Style::default(),
            click: None,
            hover: None,
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    #[must_use]
    pub fn color(mut self, color: TextColor) -> Self {
        self.style.color = Some(color);
        self
    }

    #[must_use]
    pub fn bold(mut self, on: bool) -> Self {
        self.style.bold = Some(on);
        self
    }

    #[must_use]
    pub fn italic(mut self, on: bool) -> Self {
        self.style.italic = Some(on);
        self
    }

    #[must_use]
    pub fn underlined(mut self, on: bool) -> Self {
        self.style.underlined = Some(on);
        self
    }

    #[must_use]
    pub fn strikethrough(mut self, on: bool) -> Self {
        self.style.strikethrough = Some(on);
        self
    }

    #[must_use]
    pub fn obfuscated(mut self, on: bool) -> Self {
        self.style.obfuscated = Some(on);
        self
    }

    #[must_use]
    pub fn click(mut self, event: ClickEvent) -> Self {
        self.click = Some(event);
        self
    }

    #[must_use]
    pub fn hover(mut self, event: HoverEvent) -> Self {
        self.hover = Some(event);
        self
    }

    /// Appends another span to this message.
    #[must_use]
    pub fn then(mut self, other: Message) -> Self {
        self.children.push(other);
        self
    }

    /// Flattens the message into runs, each with the styling and events it inherits.
    #[must_use]
    pub fn spans(&self) -> Vec<Span> {
        let mut out = Vec::new();
        self.flatten_into(&Style::default(), None, None, &mut out);
        out
    }

    /// The plain text of the whole message, with nothing but the characters.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.spans().into_iter().map(|span| span.text).collect()
    }

    /// Every annotation the message carries, span after span.
    #[must_use]
    pub fn annotations(&self) -> Vec<String> {
        self.spans()
            .into_iter()
            .flat_map(|span| span.annotations())
            .collect()
    }

    /// The message as a JSON component, the form a client reads.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    /// Builds a message from a JSON component.
    #[must_use]
    pub fn from_json(text: &str) -> Option<Self> {
        let value = parse_json(text)?;
        Self::read_json(&value)
    }

    fn flatten_into(
        &self,
        parent: &Style,
        parent_click: Option<&ClickEvent>,
        parent_hover: Option<&HoverEvent>,
        out: &mut Vec<Span>,
    ) {
        let style = self.style.or(parent);
        let click = self.click.clone().or_else(|| parent_click.cloned());
        let hover = self.hover.clone().or_else(|| parent_hover.cloned());
        let text = match &self.content {
            Content::Text(text) => text.clone(),
            Content::Translate { key, args, named } => resolve_translation(key, args, named),
        };
        if !text.is_empty() {
            out.push(Span {
                text,
                style: style.clone(),
                click: click.clone(),
                hover: hover.clone(),
            });
        }
        for child in &self.children {
            child.flatten_into(&style, click.as_ref(), hover.as_ref(), out);
        }
    }

    fn write_json(&self, out: &mut String) {
        out.push('{');
        match &self.content {
            Content::Text(text) => {
                out.push_str("\"text\":");
                write_json_string(out, text);
            }
            Content::Translate { key, args, named } => {
                out.push_str("\"translate\":");
                write_json_string(out, key);
                if !args.is_empty() {
                    out.push_str(",\"with\":[");
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            out.push(',');
                        }
                        arg.write_json(out);
                    }
                    out.push(']');
                }
                if !named.is_empty() {
                    out.push_str(",\"with_named\":{");
                    for (index, (name, arg)) in named.iter().enumerate() {
                        if index > 0 {
                            out.push(',');
                        }
                        write_json_string(out, name);
                        out.push(':');
                        arg.write_json(out);
                    }
                    out.push('}');
                }
            }
        }
        self.style.write_json(out);
        if let Some(click) = &self.click {
            out.push_str(",\"clickEvent\":{\"action\":");
            write_json_string(out, click.action.as_str());
            out.push_str(",\"value\":");
            write_json_string(out, &click.value);
            out.push('}');
        }
        if let Some(hover) = &self.hover {
            out.push_str(",\"hoverEvent\":{\"action\":");
            write_json_string(out, hover.action.as_str());
            out.push_str(",\"value\":");
            write_json_string(out, &hover.value);
            out.push('}');
        }
        if !self.children.is_empty() {
            out.push_str(",\"extra\":[");
            for (index, child) in self.children.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                child.write_json(out);
            }
            out.push(']');
        }
        out.push('}');
    }

    fn read_json(value: &Json) -> Option<Self> {
        if !matches!(value, Json::Object(_)) {
            return None;
        }
        let content = match value.get("text").and_then(Json::as_str) {
            Some(text) => Content::Text(text.to_owned()),
            None => match value.get("translate").and_then(Json::as_str) {
                Some(key) => Content::Translate {
                    key: key.to_owned(),
                    args: value
                        .get("with")
                        .and_then(Json::as_array)
                        .map(|items| items.iter().filter_map(Self::read_json).collect())
                        .unwrap_or_default(),
                    named: match value.get("with_named") {
                        Some(Json::Object(entries)) => entries
                            .iter()
                            .filter_map(|(name, arg)| {
                                Self::read_json(arg).map(|message| (name.clone(), message))
                            })
                            .collect(),
                        _ => Vec::new(),
                    },
                },
                None => Content::Text(String::new()),
            },
        };
        let mut style = Style::default();
        if let Some(color) = value.get("color").and_then(Json::as_str) {
            style.color = Some(TextColor::parse(color));
        }
        for (name, slot) in [
            ("bold", &mut style.bold),
            ("italic", &mut style.italic),
            ("underlined", &mut style.underlined),
            ("strikethrough", &mut style.strikethrough),
            ("obfuscated", &mut style.obfuscated),
        ] {
            if let Some(flag) = value.get(name).and_then(Json::as_bool) {
                *slot = Some(flag);
            }
        }
        if let Some(insertion) = value.get("insertion").and_then(Json::as_str) {
            style.insertion = Some(insertion.to_owned());
        }
        let click = value.get("clickEvent").and_then(|event| {
            Some(ClickEvent {
                action: ClickAction::parse(event.get("action")?.as_str()?),
                value: event.get("value")?.as_str()?.to_owned(),
            })
        });
        let hover = value.get("hoverEvent").and_then(|event| {
            Some(HoverEvent {
                action: HoverAction::parse(event.get("action")?.as_str()?),
                value: event.get("value")?.as_str()?.to_owned(),
            })
        });
        let children = value
            .get("extra")
            .and_then(Json::as_array)
            .map(|items| items.iter().filter_map(Self::read_json).collect())
            .unwrap_or_default();
        Some(Self {
            content,
            style,
            click,
            hover,
            children,
        })
    }
}

impl fmt::Display for Message {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.plain_text())
    }
}

/// One flattened run of a message: its text and everything drawn with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub click: Option<ClickEvent>,
    pub hover: Option<HoverEvent>,
}

impl Span {
    /// The styling and events of this run as annotations.
    #[must_use]
    pub fn annotations(&self) -> Vec<String> {
        let mut out = self.style.annotations();
        if let Some(click) = &self.click {
            out.push(format!("click={}:{}", click.action.as_str(), click.value));
        }
        if let Some(hover) = &self.hover {
            out.push(format!("hover={}:{}", hover.action.as_str(), hover.value));
        }
        out
    }
}

/// A bounded history a mod owns. A push one past the cap drops the oldest entry.
#[derive(Debug, Clone)]
pub struct MessageHistory {
    capacity: usize,
    entries: VecDeque<Message>,
}

impl MessageHistory {
    /// An empty history that keeps at most `capacity` messages.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
        }
    }

    /// Appends a message, answering with the one it dropped when it was full.
    pub fn push(&mut self, message: Message) -> Option<Message> {
        let dropped = if self.entries.len() >= self.capacity {
            self.entries.pop_front()
        } else {
            None
        };
        self.entries.push_back(message);
        dropped
    }

    /// A message by position, oldest first.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Message> {
        self.entries.get(index).cloned()
    }

    /// Every message, oldest first.
    #[must_use]
    pub fn get_all(&self) -> Vec<Message> {
        self.entries.iter().cloned().collect()
    }

    /// A copy of the message at a position.
    #[must_use]
    pub fn copy_message(&self, index: usize) -> Option<Message> {
        self.get(index)
    }

    /// The newest message.
    #[must_use]
    pub fn last(&self) -> Option<Message> {
        self.entries.back().cloned()
    }

    /// Messages whose plain text contains `needle`.
    #[must_use]
    pub fn find(&self, needle: &str) -> Vec<Message> {
        self.entries
            .iter()
            .filter(|message| message.plain_text().contains(needle))
            .cloned()
            .collect()
    }

    /// How many messages it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many it keeps.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Drops everything, answering how many went.
    pub fn clear(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        count
    }
}

/// A message handler, keyed by the channel it registered under.
type Handler = Box<dyn FnMut(&Message) + Send>;

fn history() -> &'static Mutex<MessageHistory> {
    static HISTORY: OnceLock<Mutex<MessageHistory>> = OnceLock::new();
    HISTORY.get_or_init(|| Mutex::new(MessageHistory::new(DEFAULT_HISTORY_CAP)))
}

fn with_history<T>(body: impl FnOnce(&mut MessageHistory) -> T) -> T {
    let mut guard = history().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn handlers() -> &'static Mutex<BTreeMap<String, Handler>> {
    static HANDLERS: OnceLock<Mutex<BTreeMap<String, Handler>>> = OnceLock::new();
    HANDLERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Builds a message of literal text.
#[must_use]
pub fn create_text(text: &str) -> Message {
    Message::text(text)
}

/// Builds a translation, substituting its positional arguments in order.
#[must_use]
pub fn create_translation(key: &str, args: &[Message]) -> Message {
    Message::translation(key, args)
}

/// Builds a translation with named arguments substituted by `{name}`.
#[must_use]
pub fn create_translation_named(key: &str, args: &[(&str, Message)]) -> Message {
    Message::new(Content::Translate {
        key: key.to_owned(),
        args: Vec::new(),
        named: args
            .iter()
            .map(|(name, message)| ((*name).to_owned(), message.clone()))
            .collect(),
    })
}

/// Builds a translation with no arguments, which is the whole of a key.
#[must_use]
pub fn translate(key: &str) -> Message {
    create_translation(key, &[])
}

/// An empty style, which inherits everything from its parent.
#[must_use]
pub fn create_style() -> Style {
    Style::default()
}

/// A click action and its payload.
#[must_use]
pub fn create_click_event(action: ClickAction, value: &str) -> ClickEvent {
    ClickEvent {
        action,
        value: value.to_owned(),
    }
}

/// A hover action and its payload, text or an item.
#[must_use]
pub fn create_hover_event(action: HoverAction, value: &str) -> HoverEvent {
    HoverEvent {
        action,
        value: value.to_owned(),
    }
}

/// Sets the colour.
pub fn set_color(style: &mut Style, color: TextColor) {
    style.color = Some(color);
}

/// Sets bold.
pub fn set_bold(style: &mut Style, on: bool) {
    style.bold = Some(on);
}

/// Sets italic.
pub fn set_italic(style: &mut Style, on: bool) {
    style.italic = Some(on);
}

/// Sets underlined.
pub fn set_underlined(style: &mut Style, on: bool) {
    style.underlined = Some(on);
}

/// Sets strikethrough.
pub fn set_strikethrough(style: &mut Style, on: bool) {
    style.strikethrough = Some(on);
}

/// Sets obfuscated.
pub fn set_obfuscated(style: &mut Style, on: bool) {
    style.obfuscated = Some(on);
}

/// Sets the text a shift-click inserts.
pub fn set_insertion(style: &mut Style, insertion: &str) {
    style.insertion = Some(insertion.to_owned());
}

/// Serializes a message to its JSON component.
#[must_use]
pub fn format(message: &Message) -> String {
    message.to_json()
}

/// Parses a JSON component back into a message.
#[must_use]
pub fn parse(text: &str) -> Option<Message> {
    Message::from_json(text)
}

/// The component tree under a message, the message itself first.
#[must_use]
pub fn get_component_tree(message: &Message) -> Vec<Message> {
    let mut out = Vec::new();
    collect_tree(message, &mut out);
    out
}

fn collect_tree(message: &Message, out: &mut Vec<Message>) {
    out.push(message.clone());
    for child in &message.children {
        collect_tree(child, out);
    }
}

/// A copy of a message.
#[must_use]
pub fn copy_message(message: &Message) -> Message {
    message.clone()
}

/// Appends to the mod's history, which drops the oldest entry past the cap.
pub fn push_history(message: &Message) -> usize {
    with_history(|history| {
        history.push(message.clone());
        history.len()
    })
}

/// Every message in the mod's history, oldest first.
#[must_use]
pub fn get_history() -> Vec<Message> {
    with_history(|history| history.get_all())
}

/// A message in the mod's history by position.
#[must_use]
pub fn get_message(index: usize) -> Option<Message> {
    with_history(|history| history.get(index))
}

/// Messages in the mod's history whose text contains `needle`.
#[must_use]
pub fn find_history(needle: &str) -> Vec<Message> {
    with_history(|history| history.find(needle))
}

/// Drops the mod's history, answering how many went.
pub fn clear_history() -> usize {
    with_history(MessageHistory::clear)
}

/// Registers a handler for a channel, or replaces the one with that name.
pub fn register_message_handler(
    channel: &str,
    handler: impl FnMut(&Message) + Send + 'static,
) -> bool {
    handlers()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(channel.to_owned(), Box::new(handler))
        .is_none()
}

/// Forgets a channel's handler.
pub fn unregister_message_handler(channel: &str) -> bool {
    handlers()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(channel)
        .is_some()
}

/// Sends to the in-process handler on one channel, and records it. Answers how many heard it.
pub fn send_local(channel: &str, message: &Message) -> usize {
    push_history(message);
    let handler = handlers()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(channel);
    match handler {
        Some(mut handler) => {
            handler(message);
            handlers()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(channel.to_owned(), handler);
            1
        }
        None => 0,
    }
}

/// Sends to every in-process handler, and records it. Answers how many heard it.
pub fn broadcast(message: &Message) -> usize {
    push_history(message);
    let taken = {
        let mut guard = handlers().lock().unwrap_or_else(|error| error.into_inner());
        std::mem::take(&mut *guard)
    };
    let count = taken.len();
    let mut returned = taken;
    for handler in returned.values_mut() {
        handler(message);
    }
    handlers()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .extend(returned);
    count
}

fn refused(what: &str) {
    crate::warn!(
        "a mod asked this host about the client's chat: {}",
        why_unknown(what)
    );
}

/// Sends into the client's chat. `false`.
pub fn send_message(_message: &Message) -> bool {
    refused("send");
    false
}

/// Sends a system line into the client's chat. `false`.
pub fn send_system_message(_message: &Message) -> bool {
    refused("send");
    false
}

/// Sends into the client's action bar. `false`.
pub fn send_action_bar(_message: &Message) -> bool {
    refused("hud");
    false
}

/// Shows a title on the client. `false`.
pub fn send_title(_message: &Message) -> bool {
    refused("hud");
    false
}

/// Shows a subtitle on the client. `false`.
pub fn send_subtitle(_message: &Message) -> bool {
    refused("hud");
    false
}

/// Shows a toast on the client. `false`.
pub fn send_toast(_message: &Message) -> bool {
    refused("hud");
    false
}

/// Sends over the client's connection. `false`.
pub fn send_network(_target: &str, _message: &Message) -> bool {
    refused("send");
    false
}

/// Registers a formatter with the client's chat. `false`.
pub fn register_formatter(_name: &str) -> bool {
    refused("format");
    false
}

/// Forgets a formatter. `false`.
pub fn unregister_formatter(_name: &str) -> bool {
    refused("format");
    false
}

/// The client's last message. `None`.
#[must_use]
pub fn get_last_message() -> Option<Message> {
    refused("history");
    None
}

/// The message selected in the client's chat screen. `None`.
#[must_use]
pub fn get_selected_message() -> Option<Message> {
    refused("screen");
    None
}

/// Why a chat question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "send" => {
            "sending into the client's chat is the client's own; this host has no handle on \
             its chat layer, so nothing is delivered and no success is faked"
                .to_owned()
        }
        "history" => {
            "the client's chat history is kept by the client; a mod that wants one keeps its \
             own ring with `push_history`"
                .to_owned()
        }
        "hud" => {
            "the client's chat HUD, its action bar and its toasts are drawn by the client; a \
             mod may put a line on the loader's overlay instead"
                .to_owned()
        }
        "format" => {
            "the client's chat formatting is applied by the client as it renders a message; a \
             mod's style is data the client would read, not something set here"
                .to_owned()
        }
        "screen" => {
            "the client's chat screen owns its selection and its input; this host never sees \
             it"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["send", "history", "hud", "format", "screen"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn resolve_translation(key: &str, args: &[Message], named: &[(String, Message)]) -> String {
    let mut text = crate::i18n::translate(key);
    for (index, arg) in args.iter().enumerate() {
        text = text.replace(&format!("{{{index}}}"), &arg.plain_text());
    }
    for (name, arg) in named {
        text = text.replace(&format!("{{{name}}}"), &arg.plain_text());
    }
    text
}

fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control < ' ' => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// A value the JSON parser produced, enough for a component.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(entries) => entries.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }
}

fn parse_json(text: &str) -> Option<Json> {
    let mut parser = JsonParser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.pos == parser.bytes.len() {
        Some(value)
    } else {
        None
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl JsonParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Option<Json> {
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Json::Text),
            b't' => self.literal("true").map(|()| Json::Bool(true)),
            b'f' => self.literal("false").map(|()| Json::Bool(false)),
            b'n' => self.literal("null").map(|()| Json::Null),
            _ => self.number(),
        }
    }

    fn literal(&mut self, word: &str) -> Option<()> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Some(())
        } else {
            None
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if self.pos == start {
            return None;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()?
            .parse()
            .ok()
            .map(Json::Number)
    }

    fn string(&mut self) -> Option<String> {
        if self.bump()? != b'"' {
            return None;
        }
        let mut out = String::new();
        loop {
            match self.bump()? {
                b'"' => return Some(out),
                b'\\' => match self.bump()? {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hex = std::str::from_utf8(self.bytes.get(self.pos..self.pos + 4)?).ok()?;
                        let code = u32::from_str_radix(hex, 16).ok()?;
                        self.pos += 4;
                        out.push(char::from_u32(code)?);
                    }
                    _ => return None,
                },
                byte if byte < 0x80 => out.push(byte as char),
                _ => {
                    let start = self.pos - 1;
                    let width = match self.bytes[start] {
                        0xf0..=0xf7 => 4,
                        0xe0..=0xef => 3,
                        _ => 2,
                    };
                    let slice = self.bytes.get(start..start + width)?;
                    self.pos = start + width;
                    out.push_str(std::str::from_utf8(slice).ok()?);
                }
            }
        }
    }

    fn object(&mut self) -> Option<Json> {
        if self.bump()? != b'{' {
            return None;
        }
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Some(Json::Object(entries));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            if self.bump()? != b':' {
                return None;
            }
            self.skip_ws();
            let value = self.value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.bump()? {
                b',' => continue,
                b'}' => return Some(Json::Object(entries)),
                _ => return None,
            }
        }
    }

    fn array(&mut self) -> Option<Json> {
        if self.bump()? != b'[' {
            return None;
        }
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Some(Json::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.bump()? {
                b',' => continue,
                b']' => return Some(Json::Array(items)),
                _ => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn translations(entries: &[(&str, &str)]) {
        let current = crate::i18n::get_current_language();
        let fallback = crate::i18n::get_fallback_language();
        crate::i18n::register_translations(&current, entries);
        if fallback != current {
            crate::i18n::register_translations(&fallback, entries);
        }
    }

    fn fresh() {
        with_history(|history| *history = MessageHistory::new(DEFAULT_HISTORY_CAP));
        handlers()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    #[test]
    fn spans_concatenate_into_one_message_and_flatten_back() {
        let _serial = serial();
        fresh();
        let message = create_text("Hello")
            .color(TextColor::named("red"))
            .then(create_text(" world").bold(true));
        assert_eq!(message.plain_text(), "Hello world");
        assert_eq!(message.to_string(), "Hello world");
        let spans = message.spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "Hello");
        assert_eq!(spans[1].text, " world");
        assert_eq!(spans[1].style.bold, Some(true));
        assert_eq!(get_component_tree(&message).len(), 2);
        assert_eq!(copy_message(&message), message);
    }

    #[test]
    fn a_nested_styled_span_keeps_both_styles() {
        let _serial = serial();
        fresh();
        let mut outer = create_style();
        set_color(&mut outer, TextColor::named("gold"));
        set_underlined(&mut outer, true);
        let mut inner = create_style();
        set_bold(&mut inner, true);
        set_italic(&mut inner, true);
        let message = create_text("a").style(outer).then(create_text("b").style(inner));
        let spans = message.spans();
        assert_eq!(
            spans[0].annotations(),
            vec!["color=gold".to_owned(), "underlined".to_owned()]
        );
        assert_eq!(
            spans[1].annotations(),
            vec![
                "color=gold".to_owned(),
                "bold".to_owned(),
                "italic".to_owned(),
                "underlined".to_owned()
            ]
        );
        let mut extra = create_style();
        set_strikethrough(&mut extra, true);
        set_obfuscated(&mut extra, true);
        set_insertion(&mut extra, "copy me");
        assert_eq!(
            create_text("z").style(extra).annotations(),
            vec![
                "strikethrough".to_owned(),
                "obfuscated".to_owned(),
                "insertion=copy me".to_owned()
            ]
        );
    }

    #[test]
    fn a_translation_substitutes_its_arguments_in_order() {
        let _serial = serial();
        fresh();
        translations(&[("greet", "Hello, {0}! Welcome, {1}."), ("welcome", "Welcome, {player}!")]);
        let message = create_translation("greet", &[create_text("world"), create_text("friend")]);
        assert_eq!(message.plain_text(), "Hello, world! Welcome, friend.");
        assert_eq!(message.to_string(), "Hello, world! Welcome, friend.");
        let named = create_translation_named("welcome", &[("player", create_text("Alex"))]);
        assert_eq!(named.plain_text(), "Welcome, Alex!");
        assert_eq!(translate("welcome").plain_text(), "Welcome, {player}!");
    }

    #[test]
    fn an_unknown_key_and_an_unknown_placeholder_stay_visible() {
        let _serial = serial();
        fresh();
        assert_eq!(create_translation("no.such.key", &[]).plain_text(), "no.such.key");
        translations(&[("hi.two", "Hi {0} and {1}")]);
        assert_eq!(
            create_translation("hi.two", &[create_text("you")]).plain_text(),
            "Hi you and {1}"
        );
    }

    #[test]
    fn a_named_colour_and_a_hex_colour_both_resolve() {
        let _serial = serial();
        fresh();
        assert_eq!(
            create_text("x").color(TextColor::named("dark_purple")).annotations(),
            vec!["color=dark_purple".to_owned()]
        );
        assert_eq!(
            create_text("y").color(TextColor::hex(0xff5555)).annotations(),
            vec!["color=#ff5555".to_owned()]
        );
        assert_eq!(TextColor::rgb(0x55, 0xff, 0x00).as_str(), "#55ff00");
        assert_eq!(TextColor::parse("#55ff00"), TextColor::Rgb(0x55ff00));
        assert_eq!(TextColor::parse("red"), TextColor::Named("red".to_owned()));
        assert_eq!(TextColor::named("Red").to_string(), "red");
    }

    #[test]
    fn a_click_and_a_hover_survive_the_flatten() {
        let _serial = serial();
        fresh();
        let message = create_text("click me")
            .click(create_click_event(ClickAction::RunCommand, "/say hi"))
            .hover(create_hover_event(HoverAction::ShowText, "a tooltip"))
            .then(create_text(" now"));
        let spans = message.spans();
        assert_eq!(spans.len(), 2);
        for span in &spans {
            assert_eq!(
                span.click,
                Some(ClickEvent {
                    action: ClickAction::RunCommand,
                    value: "/say hi".to_owned()
                })
            );
        }
        assert!(spans[0].annotations().contains(&"click=run_command:/say hi".to_owned()));
        assert!(spans[0].annotations().contains(&"hover=show_text:a tooltip".to_owned()));
        assert_eq!(HoverEvent::item("minecraft:diamond").action, HoverAction::ShowItem);
        assert_eq!(create_hover_event(HoverAction::ShowEntity, "id").action, HoverAction::ShowEntity);
    }

    #[test]
    fn the_history_is_a_ring_that_drops_exactly_one_at_its_cap() {
        let _serial = serial();
        fresh();
        let mut history = MessageHistory::new(4);
        assert_eq!(history.capacity(), 4);
        for index in 0..4 {
            assert_eq!(
                history.push(create_text(&format!("m{index}"))),
                None,
                "nothing drops while it fits"
            );
        }
        assert_eq!(history.len(), 4);
        let dropped = history.push(create_text("m4")).expect("one over the cap drops the oldest");
        assert_eq!(dropped.plain_text(), "m0");
        assert_eq!(history.len(), 4);
        assert_eq!(history.get(0).map(|message| message.plain_text()), Some("m1".to_owned()));
        assert_eq!(history.get_all().len(), 4);
        assert_eq!(history.last().map(|message| message.plain_text()), Some("m4".to_owned()));
        assert_eq!(history.find("m3").len(), 1);
        assert_eq!(
            history.copy_message(0).map(|message| message.plain_text()),
            Some("m1".to_owned())
        );
        assert_eq!(history.clear(), 4);
        assert!(history.is_empty());

        push_history(&create_text("a"));
        push_history(&create_text("b"));
        assert_eq!(get_history().len(), 2);
        assert_eq!(get_message(0).map(|message| message.plain_text()), Some("a".to_owned()));
        assert_eq!(find_history("b").len(), 1);
        assert_eq!(clear_history(), 2);
    }

    #[test]
    fn a_broadcast_reaches_every_subscriber_and_a_targeted_send_one() {
        let _serial = serial();
        fresh();
        let heard = std::sync::Arc::new(Mutex::new(Vec::new()));
        for name in ["a", "b", "c"] {
            let into = std::sync::Arc::clone(&heard);
            assert!(register_message_handler(name, move |message: &Message| {
                into.lock().unwrap().push(message.clone());
            }));
        }
        let message = create_text("hello").color(TextColor::named("green"));
        assert_eq!(send_local("b", &message), 1);
        assert_eq!(broadcast(&message), 3);
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 4);
        assert!(heard.iter().all(|held| held.plain_text() == "hello"));
        assert!(heard
            .iter()
            .all(|held| held.style.color == Some(TextColor::named("green"))));
        drop(heard);
        assert!(unregister_message_handler("b"));
        assert!(!unregister_message_handler("b"));
        assert_eq!(broadcast(&message), 2);
        assert_eq!(send_local("b", &message), 0);
    }

    #[test]
    fn format_and_parse_are_a_real_round_trip() {
        let _serial = serial();
        fresh();
        translations(&[("greet", "Hello, {0}!")]);
        let message = create_text("Hi")
            .color(TextColor::hex(0x55ff55))
            .bold(true)
            .click(create_click_event(ClickAction::OpenUrl, "https://example.invalid"))
            .then(create_translation("greet", &[create_text("there")]));
        let json = format(&message);
        assert!(json.contains("\"text\":\"Hi\""));
        assert!(json.contains("\"color\":\"#55ff55\""));
        assert!(json.contains("\"bold\":true"));
        assert!(json.contains("\"clickEvent\""));
        let parsed = parse(&json).expect("it round trips");
        assert_eq!(parsed, message);
        assert_eq!(parsed.plain_text(), "HiHello, there!");
        assert!(parse("not json").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn the_clients_chat_is_refused_with_a_reason_each() {
        let _serial = serial();
        fresh();
        let message = create_text("hi");
        assert!(!send_message(&message));
        assert!(!send_system_message(&message));
        assert!(!send_action_bar(&message));
        assert!(!send_title(&message));
        assert!(!send_subtitle(&message));
        assert!(!send_toast(&message));
        assert!(!send_network("server", &message));
        assert!(!register_formatter("hello:fmt"));
        assert!(!unregister_formatter("hello:fmt"));
        assert_eq!(get_last_message(), None);
        assert_eq!(get_selected_message(), None);
        assert_eq!(refusals().len(), 5);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("send").contains("client"));
        assert!(why_unknown("screen").contains("chat screen"));
    }
}
