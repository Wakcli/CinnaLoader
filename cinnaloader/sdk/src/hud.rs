//! HUD and overlays: the elements a mod owns, and the one way this host can show them.
//!
//! A HUD element is real data — where it sits, how big it is, how it is aligned against the
//! screen, which anchor it grows from, its scale and opacity, which layer it is on, whether
//! it is visible and enabled, and its z index and priority — and so are layers, tooltips,
//! click handlers and animation hooks. The arithmetic is real too: [`Element::bounds`] places
//! an element on the screen from its alignment and anchor, `get_safe_area` insets the screen
//! by a margin, `get_screen_size` reads the window the loader was told about, and
//! `get_scaled_size` divides it by a scale.
//!
//! The drawing is where this host is honest. The loader has one way to put something in front
//! of the player — its overlay line — so [`flush`] draws the *text* a mod gave its visible
//! elements as overlay lines and counts the rest as elements it could not draw, with the
//! reason in [`why_unknown`]. A mod's FPS counter or coordinate display works; its minimap
//! does not, and it is told why.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! hud::register_element(HudElement::new("hello:coords", "0, 64, 0")
//!     .at(4.0, 4.0)
//!     .alignment(Alignment::TopLeft)
//!     .layer("hello:info"));
//! hud::set_screen_size((1280, 720));
//! assert_eq!(hud::get_bounds("hello:coords").map(|b| (b.0, b.1)), Some((4.0, 4.0)));
//! assert_eq!(hud::flush().0, 1, "the text goes on the overlay");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Where an element sits against the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Alignment {
    #[default]
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Alignment {
    /// The same, with the vertical fraction.
    #[must_use]
    pub fn fractions_of(self) -> (f32, f32) {
        let horizontal = match self {
            Self::TopLeft | Self::CenterLeft | Self::BottomLeft => 0.0,
            Self::TopCenter | Self::Center | Self::BottomCenter => 0.5,
            Self::TopRight | Self::CenterRight | Self::BottomRight => 1.0,
        };
        let vertical = match self {
            Self::TopLeft | Self::TopCenter | Self::TopRight => 0.0,
            Self::CenterLeft | Self::Center | Self::CenterRight => 0.5,
            Self::BottomLeft | Self::BottomCenter | Self::BottomRight => 1.0,
        };
        (horizontal, vertical)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TopLeft => "top_left",
            Self::TopCenter => "top_center",
            Self::TopRight => "top_right",
            Self::CenterLeft => "center_left",
            Self::Center => "center",
            Self::CenterRight => "center_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomCenter => "bottom_center",
            Self::BottomRight => "bottom_right",
        }
    }
}

/// Which corner an element grows from, for a caller that wants its own coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Anchor {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl Anchor {
    /// The fraction of the element's own size that its position counts from.
    #[must_use]
    pub fn fractions(self) -> (f32, f32) {
        match self {
            Self::TopLeft => (0.0, 0.0),
            Self::TopRight => (1.0, 0.0),
            Self::BottomLeft => (0.0, 1.0),
            Self::BottomRight => (1.0, 1.0),
            Self::Center => (0.5, 0.5),
        }
    }
}

/// One thing a mod wants on the HUD.
#[derive(Debug, Clone, PartialEq)]
pub struct HudElement {
    pub id: String,
    pub owner: String,
    /// The text it shows, when it shows text.
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub alignment: Alignment,
    pub anchor: Anchor,
    pub scale: f32,
    pub opacity: f32,
    pub layer: String,
    pub visible: bool,
    pub enabled: bool,
    pub z_index: i32,
    pub priority: i32,
    /// A tooltip shown when the pointer is over it.
    pub tooltip: String,
    /// Whether a mod has a click handler for it, which nothing here calls.
    pub has_click_handler: bool,
    /// Whether it has an animation, which nothing here runs.
    pub has_animation: bool,
}

impl HudElement {
    /// An element showing some text, with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, text: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            text: text.to_owned(),
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            alignment: Alignment::TopLeft,
            anchor: Anchor::TopLeft,
            scale: 1.0,
            opacity: 1.0,
            layer: "minecraft:default".to_owned(),
            visible: true,
            enabled: true,
            z_index: 0,
            priority: 0,
            tooltip: String::new(),
            has_click_handler: false,
            has_animation: false,
        }
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
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    #[must_use]
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    #[must_use]
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale.max(0.0);
        self
    }

    #[must_use]
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn layer(mut self, layer: &str) -> Self {
        self.layer = layer.to_owned();
        self
    }

    #[must_use]
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Puts it at a z index, which decides its order against elements on other layers.
    #[must_use]
    pub fn z(mut self, z_index: i32) -> Self {
        self.z_index = z_index;
        self
    }

    /// Where it lands on a screen, as `(x, y, width, height)`. The alignment places it among
    /// the free space and the anchor decides which corner the position counts from.
    #[must_use]
    pub fn bounds(&self, screen: (u32, u32)) -> (f32, f32, f32, f32) {
        let (width, height) = (self.width * self.scale, self.height * self.scale);
        let (horizontal, vertical) = self.alignment.fractions_of();
        let (anchor_x, anchor_y) = self.anchor.fractions();
        let base_x = (screen.0 as f32 - width) * horizontal;
        let base_y = (screen.1 as f32 - height) * vertical;
        (
            base_x + self.x - width * anchor_x,
            base_y + self.y - height * anchor_y,
            width,
            height,
        )
    }

    /// Whether the pointer is inside it.
    #[must_use]
    pub fn contains(&self, screen: (u32, u32), point: (f32, f32)) -> bool {
        let (x, y, width, height) = self.bounds(screen);
        point.0 >= x && point.0 <= x + width && point.1 >= y && point.1 <= y + height
    }

    /// Whether it would be drawn: visible, enabled and with something to draw.
    #[must_use]
    pub fn is_drawn(&self) -> bool {
        self.visible && self.enabled && !self.text.is_empty()
    }
}

impl fmt::Display for HudElement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} '{}' at ({:.0}, {:.0}) {} on {}{}",
            self.id,
            self.text,
            self.x,
            self.y,
            self.alignment.as_str(),
            self.layer,
            if self.visible { "" } else { " (hidden)" }
        )
    }
}

/// A layer of HUD elements, drawn in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudLayer {
    pub id: String,
    pub owner: String,
    pub z_index: i32,
    pub visible: bool,
}

fn elements() -> &'static Mutex<BTreeMap<String, HudElement>> {
    static ELEMENTS: OnceLock<Mutex<BTreeMap<String, HudElement>>> = OnceLock::new();
    ELEMENTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, HudElement>) -> T) -> T {
    let mut guard = elements().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn layers() -> &'static Mutex<BTreeMap<String, HudLayer>> {
    static LAYERS: OnceLock<Mutex<BTreeMap<String, HudLayer>>> = OnceLock::new();
    LAYERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_layers<T>(body: impl FnOnce(&mut BTreeMap<String, HudLayer>) -> T) -> T {
    let mut guard = layers().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The screen size this host was told about, which the launcher sets.
static SCREEN: OnceLock<Mutex<(u32, u32)>> = OnceLock::new();

/// Tells the HUD how big the screen is. The loader calls it from the window facts.
pub fn set_screen_size(size: (u32, u32)) {
    let slot = SCREEN.get_or_init(|| Mutex::new((0, 0)));
    if let Ok(mut held) = slot.lock() {
        *held = size;
    }
}

/// How big the screen is.
#[must_use]
pub fn get_screen_size() -> (u32, u32) {
    SCREEN
        .get()
        .and_then(|slot| slot.lock().ok().map(|held| *held))
        .filter(|size| size.0 > 0 && size.1 > 0)
        .unwrap_or_else(|| {
            crate::game::get_window()
                .map(|window| (window.width, window.height))
                .unwrap_or((0, 0))
        })
}

/// The screen size divided by a scale, which is what a caller laying out in scaled pixels
/// wants.
#[must_use]
pub fn get_scaled_size(scale: f32) -> (u32, u32) {
    let (width, height) = get_screen_size();
    let scale = if scale <= 0.0 { 1.0 } else { scale };
    (
        (width as f32 / scale) as u32,
        (height as f32 / scale) as u32,
    )
}

/// The area a mod may draw in, which is the screen inset by a margin.
#[must_use]
pub fn get_safe_area(margin: f32) -> (f32, f32, f32, f32) {
    let (width, height) = get_screen_size();
    let margin = margin.max(0.0);
    let (width, height) = (width as f32, height as f32);
    if margin * 2.0 >= width || margin * 2.0 >= height {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (
        margin,
        margin,
        width - margin * 2.0,
        height - margin * 2.0,
    )
}

/// Registers an element, or replaces the one with that id.
pub fn register_element(element: HudElement) -> bool {
    with(|elements| elements.insert(element.id.clone(), element).is_none())
}

/// Forgets an element.
pub fn unregister_element(id: &str) -> bool {
    with(|elements| elements.remove(id).is_some())
}

/// An element by id.
#[must_use]
pub fn get_element(id: &str) -> Option<HudElement> {
    with(|elements| elements.get(id).cloned())
}

/// Every element, ordered by layer, then z index, then priority.
#[must_use]
pub fn get_elements() -> Vec<HudElement> {
    let mut all: Vec<HudElement> = with(|elements| elements.values().cloned().collect());
    all.sort_by_key(|element| {
        (
            element.layer.clone(),
            element.z_index,
            -element.priority,
            element.id.clone(),
        )
    });
    all
}

/// Registers a layer.
pub fn register_layer(layer: HudLayer) -> bool {
    with_layers(|layers| layers.insert(layer.id.clone(), layer).is_none())
}

/// Forgets a layer.
pub fn unregister_layer(id: &str) -> bool {
    with_layers(|layers| layers.remove(id).is_some())
}

/// A layer by id.
#[must_use]
pub fn get_layer(id: &str) -> Option<HudLayer> {
    with_layers(|layers| layers.get(id).cloned())
}

/// Every layer, in z order.
#[must_use]
pub fn get_layers() -> Vec<HudLayer> {
    let mut all: Vec<HudLayer> = with_layers(|layers| layers.values().cloned().collect());
    all.sort_by_key(|layer| (layer.z_index, layer.id.clone()));
    all
}

/// Moves an element to another layer.
pub fn set_layer(id: &str, layer: &str) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.layer = layer.to_owned();
            true
        }
        None => false,
    })
}

/// Shows or hides an element.
pub fn set_visible(id: &str, visible: bool) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.visible = visible;
            true
        }
        None => false,
    })
}

/// Whether it is visible.
#[must_use]
pub fn is_visible(id: &str) -> bool {
    get_element(id).is_some_and(|element| element.visible)
}

/// Turns an element on or off, which is not the same as hiding it.
pub fn set_enabled(id: &str, enabled: bool) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.enabled = enabled;
            true
        }
        None => false,
    })
}

/// Whether it is enabled.
#[must_use]
pub fn is_enabled(id: &str) -> bool {
    get_element(id).is_some_and(|element| element.enabled)
}

/// Moves it.
pub fn set_position(id: &str, x: f32, y: f32) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.x = x;
            element.y = y;
            true
        }
        None => false,
    })
}

/// Resizes it.
pub fn set_size(id: &str, width: f32, height: f32) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.width = width;
            element.height = height;
            true
        }
        None => false,
    })
}

/// Changes how it is aligned against the screen.
pub fn set_alignment(id: &str, alignment: Alignment) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.alignment = alignment;
            true
        }
        None => false,
    })
}

/// Changes its scale.
pub fn set_scale(id: &str, scale: f32) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.scale = scale.max(0.0);
            true
        }
        None => false,
    })
}

/// Changes its opacity.
pub fn set_opacity(id: &str, opacity: f32) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.opacity = opacity.clamp(0.0, 1.0);
            true
        }
        None => false,
    })
}

/// Changes what it shows.
pub fn set_text(id: &str, text: &str) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.text = text.to_owned();
            true
        }
        None => false,
    })
}

/// Where an element lands on the screen.
#[must_use]
pub fn get_bounds(id: &str) -> Option<(f32, f32, f32, f32)> {
    get_element(id).map(|element| element.bounds(get_screen_size()))
}

/// The corner it grows from.
#[must_use]
pub fn get_anchor(id: &str) -> Option<Anchor> {
    get_element(id).map(|element| element.anchor)
}

/// Changes it.
pub fn set_anchor(id: &str, anchor: Anchor) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.anchor = anchor;
            true
        }
        None => false,
    })
}

/// Its z index.
#[must_use]
pub fn get_z_index(id: &str) -> Option<i32> {
    get_element(id).map(|element| element.z_index)
}

/// Changes it.
pub fn set_z_index(id: &str, z_index: i32) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.z_index = z_index;
            true
        }
        None => false,
    })
}

/// Its priority, which breaks ties on the same layer and z index.
#[must_use]
pub fn get_priority(id: &str) -> Option<i32> {
    get_element(id).map(|element| element.priority)
}

/// Changes it.
pub fn set_priority(id: &str, priority: i32) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.priority = priority;
            true
        }
        None => false,
    })
}

/// How many layouts have been thrown away, for a caller that watches that.
static LAYOUT: AtomicU64 = AtomicU64::new(0);
static RENDER: AtomicU64 = AtomicU64::new(0);
static REDRAWS: AtomicU64 = AtomicU64::new(0);

/// Notes that the layout has to be worked out again.
pub fn invalidate_layout() -> u64 {
    LAYOUT.fetch_add(1, Ordering::Relaxed) + 1
}

/// The same for the drawing.
pub fn invalidate_render() -> u64 {
    RENDER.fetch_add(1, Ordering::Relaxed) + 1
}

/// Asks for the next frame to draw the HUD again, which is the same as the render
/// invalidation plus a count of the requests.
pub fn request_redraw() -> u64 {
    RENDER.fetch_add(1, Ordering::Relaxed);
    REDRAWS.fetch_add(1, Ordering::Relaxed) + 1
}

/// How many times each has been asked for.
#[must_use]
pub fn get_invalidation_counts() -> (u64, u64, u64) {
    (
        LAYOUT.load(Ordering::Relaxed),
        RENDER.load(Ordering::Relaxed),
        REDRAWS.load(Ordering::Relaxed),
    )
}

/// Records that a mod has a click handler for an element, which nothing here calls.
pub fn register_click_handler(id: &str) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.has_click_handler = true;
            true
        }
        None => false,
    })
}

/// Sets an element's tooltip.
pub fn register_tooltip(id: &str, tooltip: &str) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.tooltip = tooltip.to_owned();
            true
        }
        None => false,
    })
}

/// Records that a mod has an animation for an element, which nothing here runs.
pub fn register_animation(id: &str) -> bool {
    with(|elements| match elements.get_mut(id) {
        Some(element) => {
            element.has_animation = true;
            true
        }
        None => false,
    })
}

/// Draws what this host can: the text of every element that would be drawn, on the overlay.
/// Answers how many were drawn and how many could not be.
pub fn flush() -> (u64, u64) {
    let (screen_width, screen_height) = get_screen_size();
    let mut drawn = 0;
    let mut undrawn = 0;
    for element in get_elements() {
        if !element.visible || !element.enabled {
            continue;
        }
        let (x, y, width, height) = element.bounds((screen_width, screen_height));
        if element.text.is_empty() {
            undrawn += 1;
            continue;
        }
        crate::overlay_line(&format!(
            "{} at {x:.0},{y:.0} ({width:.0}x{height:.0}, {:.2} alpha)",
            element.text, element.opacity
        ));
        drawn += 1;
    }
    (drawn, undrawn)
}

/// Why an element cannot be drawn here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "draw" => {
            "this host has no HUD of its own: the client draws its own, and the loader's \
             overlay line is the one place a mod host can put text (section 29)"
                .to_owned()
        }
        "image" => "an element that draws an image, a minimap or a graph needs the client's renderer".to_owned(),
        "input" => {
            "a click handler is called by the client's UI as it dispatches input; nothing \
             here calls one"
                .to_owned()
        }
        "animation" => "an element's animation is run by the client's UI".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["draw", "image", "input", "animation"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The elements a client would show, for a caller that wants to see the names the wishlist
/// lists.
pub const KNOWN_ELEMENTS: [&str; 21] = [
    "fps_counter",
    "tps_counter",
    "coordinates",
    "keystrokes",
    "armor_status",
    "durability",
    "potion_effects",
    "inventory_overlay",
    "target_info",
    "crosshair",
    "compass",
    "minimap",
    "waypoints",
    "boss_health",
    "chat_overlay",
    "scoreboard",
    "ping",
    "network_graph",
    "performance_graph",
    "notification_center",
    "hotbar",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|elements| elements.clear());
        with_layers(|layers| layers.clear());
        set_screen_size((1280, 720));
    }

    #[test]
    fn an_element_lands_where_its_alignment_and_anchor_say() {
        let _serial = serial();
        fresh();
        let corner = HudElement::new("a", "a").at(4.0, 4.0).sized(100.0, 20.0);
        assert_eq!(corner.bounds((1280, 720)), (4.0, 4.0, 100.0, 20.0));
        let centred = HudElement::new("b", "b")
            .sized(100.0, 20.0)
            .alignment(Alignment::Center);
        assert_eq!(centred.bounds((1280, 720)), (590.0, 350.0, 100.0, 20.0));
        let bottom_right = HudElement::new("c", "c")
            .sized(100.0, 20.0)
            .alignment(Alignment::BottomRight);
        assert_eq!(bottom_right.bounds((1280, 720)), (1180.0, 700.0, 100.0, 20.0));
        // The anchor takes the position off the element's own size.
        let anchored = HudElement::new("d", "d")
            .at(10.0, 10.0)
            .sized(100.0, 20.0)
            .anchor(Anchor::BottomRight);
        assert_eq!(anchored.bounds((1280, 720)), (-90.0, -10.0, 100.0, 20.0));
        let centred_anchor = HudElement::new("e", "e").sized(100.0, 20.0).anchor(Anchor::Center);
        assert_eq!(centred_anchor.bounds((1280, 720)), (-50.0, -10.0, 100.0, 20.0));
        // Scale multiplies the size, which moves a centred element too.
        let scaled = HudElement::new("f", "f")
            .sized(100.0, 20.0)
            .scale(2.0)
            .alignment(Alignment::Center);
        assert_eq!(scaled.bounds((1280, 720)), (540.0, 340.0, 200.0, 40.0));
        assert!(HudElement::new("g", "g").sized(10.0, 10.0).contains((1280, 720), (5.0, 5.0)));
        assert!(!HudElement::new("g", "g").sized(10.0, 10.0).contains((1280, 720), (50.0, 5.0)));
        assert_eq!(Alignment::BottomCenter.fractions_of(), (0.5, 1.0));
        assert_eq!(Alignment::CenterLeft.fractions_of(), (0.0, 0.5));
        assert_eq!(Anchor::Center.fractions(), (0.5, 0.5));
        assert_eq!(Alignment::TopRight.as_str(), "top_right");
    }

    #[test]
    fn the_screen_and_the_safe_area_come_from_what_the_loader_knows() {
        let _serial = serial();
        fresh();
        assert_eq!(get_screen_size(), (1280, 720));
        assert_eq!(get_scaled_size(2.0), (640, 360));
        assert_eq!(get_scaled_size(0.0), (1280, 720), "a zero scale is taken as one");
        assert_eq!(get_safe_area(8.0), (8.0, 8.0, 1264.0, 704.0));
        assert_eq!(get_safe_area(0.0), (0.0, 0.0, 1280.0, 720.0));
        assert_eq!(
            get_safe_area(400.0),
            (0.0, 0.0, 0.0, 0.0),
            "a margin bigger than the screen leaves nothing"
        );
    }

    #[test]
    fn elements_layers_and_their_order_are_real_data() {
        let _serial = serial();
        fresh();
        assert!(register_element(
            HudElement::new("hello:coords", "0, 64, 0")
                .at(4.0, 4.0)
                .layer("hello:info")
                .priority(5)
        ));
        assert!(!register_element(HudElement::new("hello:coords", "x")), "it replaces");
        assert!(register_element(
            HudElement::new("hello:fps", "60 fps").z(1).layer("hello:info").priority(1)
        ));
        assert!(register_element(HudElement::new("hello:other", "y").layer("aaa:first")));
        let order: Vec<String> = get_elements().into_iter().map(|element| element.id).collect();
        assert_eq!(
            order,
            vec!["hello:other".to_owned(), "hello:fps".to_owned(), "hello:coords".to_owned()],
            "layer, then z index, then priority"
        );
        assert!(register_layer(HudLayer {
            id: "hello:info".to_owned(),
            owner: crate::metadata::id(),
            z_index: 10,
            visible: true,
        }));
        assert!(!register_layer(HudLayer {
            id: "hello:info".to_owned(),
            owner: crate::metadata::id(),
            z_index: 10,
            visible: true,
        }));
        assert_eq!(get_layers().len(), 1);
        assert_eq!(get_layer("hello:info").map(|layer| layer.z_index), Some(10));
        assert!(unregister_layer("hello:info"));
        assert!(unregister_element("hello:other"));
        assert!(!unregister_element("hello:other"));
        assert_eq!(get_elements().len(), 2);
        let replaced = get_element("hello:coords").expect("it is there");
        assert_eq!(
            replaced.layer, "minecraft:default",
            "the replacement took the defaults with it"
        );
        assert!(replaced.to_string().contains("minecraft:default"));
        assert_eq!(HudElement::new("x", "x").bounds((0, 0)), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn everything_about_an_element_can_be_changed_and_read_back() {
        let _serial = serial();
        fresh();
        register_element(HudElement::new("hello:coords", "0, 64, 0").sized(100.0, 20.0));
        assert!(set_position("hello:coords", 10.0, 10.0));
        assert!(set_size("hello:coords", 200.0, 40.0));
        assert!(set_alignment("hello:coords", Alignment::BottomRight));
        assert!(set_anchor("hello:coords", Anchor::BottomRight));
        assert!(set_scale("hello:coords", 2.0));
        assert!(set_opacity("hello:coords", 2.0));
        assert!(set_z_index("hello:coords", 3));
        assert!(set_priority("hello:coords", 9));
        assert!(set_layer("hello:coords", "hello:other"));
        assert!(set_text("hello:coords", "1, 2, 3"));
        let element = get_element("hello:coords").expect("registered");
        assert_eq!((element.x, element.y), (10.0, 10.0));
        assert_eq!((element.width, element.height), (200.0, 40.0));
        assert_eq!(element.alignment, Alignment::BottomRight);
        assert_eq!(get_anchor("hello:coords"), Some(Anchor::BottomRight));
        assert_eq!(element.scale, 2.0);
        assert_eq!(element.opacity, 1.0, "clamped");
        assert_eq!(get_z_index("hello:coords"), Some(3));
        assert_eq!(get_priority("hello:coords"), Some(9));
        assert_eq!(element.layer, "hello:other");
        assert_eq!(element.text, "1, 2, 3");
        // 200x40 scaled by two, anchored at its own bottom right, at 10,10 in the corner.
        // Free space is 1280-400 by 720-80, the position adds 10,10, and the anchor takes
        // the element's own 400x80 off it.
        assert_eq!(
            get_bounds("hello:coords"),
            Some((490.0, 570.0, 400.0, 80.0))
        );
        assert!(set_visible("hello:coords", false));
        assert!(!is_visible("hello:coords"));
        assert!(set_enabled("hello:coords", false));
        assert!(!is_enabled("hello:coords"));
        for setter in [
            set_position("nothing", 1.0, 1.0),
            set_size("nothing", 1.0, 1.0),
            set_alignment("nothing", Alignment::Center),
            set_anchor("nothing", Anchor::Center),
            set_scale("nothing", 1.0),
            set_opacity("nothing", 1.0),
            set_z_index("nothing", 1),
            set_priority("nothing", 1),
            set_layer("nothing", "x"),
            set_text("nothing", "x"),
            set_visible("nothing", true),
            set_enabled("nothing", true),
        ] {
            assert!(!setter, "setting something that is not there says so");
        }
    }

    #[test]
    fn a_flush_draws_the_text_and_counts_what_it_could_not() {
        let _serial = serial();
        fresh();
        register_element(HudElement::new("hello:coords", "0, 64, 0").sized(100.0, 10.0));
        register_element(HudElement::new("hello:blank", "").sized(50.0, 50.0));
        register_element(HudElement::new("hello:hidden", "not shown").sized(10.0, 10.0));
        set_visible("hello:hidden", false);
        assert_eq!(flush(), (1, 1), "one text element drawn, one without text counted");
        register_element(HudElement::new("hello:fps", "60 fps"));
        assert_eq!(flush(), (2, 1));
        set_enabled("hello:fps", false);
        assert_eq!(flush(), (1, 1), "a disabled element is not drawn at all");
        assert!(invalidate_layout() > 0);
        assert!(invalidate_render() > 0);
        assert!(request_redraw() > 0);
        let (layout, render, redraws) = get_invalidation_counts();
        assert!(layout > 0 && render > 1 && redraws > 0);
    }

    #[test]
    fn click_handlers_tooltips_and_animations_are_recorded_and_never_called() {
        let _serial = serial();
        fresh();
        register_element(HudElement::new("hello:coords", "x").sized(10.0, 10.0));
        assert!(register_click_handler("hello:coords"));
        assert!(register_tooltip("hello:coords", "where you are"));
        assert!(register_animation("hello:coords"));
        let element = get_element("hello:coords").expect("registered");
        assert!(element.has_click_handler);
        assert!(element.has_animation);
        assert_eq!(element.tooltip, "where you are");
        assert!(!register_click_handler("nothing"));
        assert!(!register_tooltip("nothing", "x"));
        assert!(!register_animation("nothing"));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_eq!(KNOWN_ELEMENTS.len(), 21);
        assert!(KNOWN_ELEMENTS.contains(&"minimap"));
        assert!(KNOWN_ELEMENTS.contains(&"coordinates"));
    }
}
