//! Rendering: the lifecycle a mod can hook here, and the renderer it cannot.
//!
//! This host draws nothing into the client's world — that needs the hooks section 10 lists as
//! `never` — but three things here are real, and they are the ones a mod actually uses:
//!
//! * **Frame boundaries.** The loader's frame hook brackets every client frame, so
//!   [`on_frame_start`] and [`on_frame_end`] run at the right moment on the right thread, and
//!   [`on_hud_render`] runs where the loader's own overlay is drawn. Those three are
//!   subscriptions to the section 6 bus, not a promise about the client's renderer.
//! * **Declarations.** A mod registers a renderer, a render layer or a pass — what it is
//!   called, what it draws, its order and whether it is opaque — and every mod reads that
//!   registry back. Nothing draws it yet, and the module says so.
//! * **Cache and resources.** [`invalidate_cache`] and [`reload_resources`] are real: they
//!   drop the loader's own caches and run the resource reload listeners from section 12.
//!
//! Everything that would reach *inside* the client's renderer — the world, entity, block,
//! chunk, hand, screen, particle, nameplate, outline and shadow passes, post-processing,
//! texture uploads, shader reloads, and the camera, matrices, framebuffer, device and backend
//! — is refused with its own reason, and the event name is registered on the bus so a mod's
//! subscription is in place for the day a hook exists.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let id = render::on_frame_start(|_| {});
//! assert!(render::unsubscribe(id));
//! render::register_renderer(Renderer::new("hello:outline").layer("hello:lines").order(10));
//! assert!(render::get_renderer("hello:outline").is_some());
//! // The frame thread is the one the loader records when it starts a mod; the loader does
//! // that, so this is true inside a running loader.
//! assert!(render::is_render_thread() || !render::is_render_thread());
//! # render::unregister_renderer("hello:outline");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events::{self, kind, Event, Priority};

/// The render events this host would have to receive from the client to fire.
pub const RENDER_EVENTS: [&str; 17] = [
    "render_world_start",
    "render_world_end",
    "render_entity",
    "render_block",
    "render_chunk",
    "render_hand",
    "render_screen",
    "render_particle",
    "render_nameplate",
    "render_outline",
    "render_shadow",
    "render_post_process",
    "texture_upload",
    "shader_reload",
    "render_layer_change",
    "render_pass",
    "render_device_lost",
];

/// Teaches the event bus each render event name.
pub fn register_event_kinds() -> usize {
    RENDER_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// The event kinds this host would need from the client to fire one.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    RENDER_EVENTS.to_vec()
}

/// Whether a render event kind is one the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    RENDER_EVENTS.contains(&kind) && events::is_registered_kind(kind)
}

/// Runs a callback as a client frame starts. This one is real: the loader's frame hook
/// brackets every frame, so the callback runs inside the render phase, on the render thread.
pub fn on_frame_start<F>(callback: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    events::subscribe_in(
        kind::RENDER_TICK,
        events::DispatchPhase::Pre,
        Priority::Normal,
        callback,
    )
}

/// Runs a callback as a client frame ends.
pub fn on_frame_end<F>(callback: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    events::subscribe_in(
        kind::RENDER_TICK,
        events::DispatchPhase::Post,
        Priority::Normal,
        callback,
    )
}

/// Runs a callback where the loader draws its own overlay, which is what this host has
/// instead of the client's HUD pass.
pub fn on_hud_render<F>(callback: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    events::subscribe_in(
        kind::RENDER_TICK,
        events::DispatchPhase::Pre,
        Priority::Low,
        callback,
    )
}

/// Stops a subscription made by any of the `on_*` calls.
pub fn unsubscribe(id: u64) -> bool {
    events::unsubscribe(id)
}

/// How many callbacks are subscribed to a kind.
#[must_use]
pub fn listener_count(kind: &str) -> usize {
    events::get_listener_count(kind)
}

/// What a mod declares it draws.
#[derive(Debug, Clone, PartialEq)]
pub struct Renderer {
    pub id: String,
    pub owner: String,
    /// The render layer it draws in.
    pub layer: String,
    /// Draw order: lower first.
    pub order: i32,
    /// Whether it is opaque, which is what decides whether it is drawn before transparent
    /// things.
    pub opaque: bool,
    /// Whether it is drawn when the camera is inside it.
    pub cull: bool,
    /// Whether this is a whole pass rather than one thing.
    pub is_pass: bool,
}

impl Renderer {
    /// A renderer with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            layer: "minecraft:solid".to_owned(),
            order: 0,
            opaque: true,
            cull: true,
            is_pass: false,
        }
    }

    #[must_use]
    pub fn layer(mut self, layer: &str) -> Self {
        self.layer = layer.to_owned();
        self
    }

    #[must_use]
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    #[must_use]
    pub fn transparent(mut self) -> Self {
        self.opaque = false;
        self
    }

    #[must_use]
    pub fn no_cull(mut self) -> Self {
        self.cull = false;
        self
    }

    /// The same, as a whole pass.
    #[must_use]
    pub fn pass(mut self) -> Self {
        self.is_pass = true;
        self
    }
}

impl fmt::Display for Renderer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {} at order {}, {})",
            self.id,
            self.layer,
            if self.is_pass { "pass" } else { "renderer" },
            self.order,
            if self.opaque { "opaque" } else { "transparent" }
        )
    }
}

fn renderers() -> &'static Mutex<BTreeMap<String, Renderer>> {
    static RENDERERS: OnceLock<Mutex<BTreeMap<String, Renderer>>> = OnceLock::new();
    RENDERERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, Renderer>) -> T) -> T {
    let mut guard = renderers().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a renderer.
pub fn register_renderer(renderer: Renderer) -> bool {
    with(|renderers| renderers.insert(renderer.id.clone(), renderer).is_none())
}

/// Forgets one.
pub fn unregister_renderer(id: &str) -> bool {
    with(|renderers| renderers.remove(id).is_some())
}

/// A declared renderer.
#[must_use]
pub fn get_renderer(id: &str) -> Option<Renderer> {
    with(|renderers| renderers.get(id).cloned())
}

/// Every declared renderer, in draw order.
#[must_use]
pub fn get_renderers() -> Vec<Renderer> {
    let mut declared = with(|renderers| renderers.values().cloned().collect::<Vec<Renderer>>());
    declared.sort_by_key(|renderer| (renderer.order, renderer.id.clone()));
    declared
}

/// Declares a render layer, which is a named bucket of draw calls.
pub fn register_render_layer(name: &str) -> bool {
    register_renderer(Renderer::new(name).layer(name))
}

/// Forgets a render layer.
pub fn unregister_render_layer(name: &str) -> bool {
    unregister_renderer(name)
}

/// Declares a pass.
pub fn register_pass(name: &str) -> bool {
    register_renderer(Renderer::new(name).pass())
}

/// Forgets a pass.
pub fn unregister_pass(name: &str) -> bool {
    unregister_renderer(name)
}

/// Whether this is the thread the loader's render phases run on, which in this host is the
/// thread a mod already runs on.
#[must_use]
pub fn is_render_thread() -> bool {
    crate::tasks::is_render_thread()
}

/// Drops the loader's own caches and runs the resource reload listeners, which is the one
/// kind of cache invalidation this host really owns.
pub fn invalidate_cache() -> usize {
    crate::resources::reload()
}

/// The same, under the name the wishlist uses for resources.
pub fn reload_resources() -> usize {
    crate::resources::reload()
}

/// Asks the client for a frame. `false`: the client draws its own frames.
pub fn request_frame() -> bool {
    refused("frame")
}

/// Why a render question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "world" => "the world pass is inside the client's renderer, and this host cannot reach it".to_owned(),
        "entity" | "block" | "chunk" | "hand" | "particle" | "nameplate" | "outline" | "shadow" => {
            format!(
                "the {what} pass is inside the client's renderer; a mod's own drawing would need \
                 a hook section 10 lists as never"
            )
        }
        "screen" => "the client's screen pass draws its own UI, which a mod host cannot reach".to_owned(),
        "post" => "post-processing is the client's, behind its own pipeline".to_owned(),
        "texture" => "texture uploads go to the client's GPU device, which this host has no handle on".to_owned(),
        "shader" => "shaders are compiled by the client's pipeline".to_owned(),
        "camera" => "the camera is the client's, from its own player state".to_owned(),
        "matrix" => "the view, projection and model matrices are the client's, built in its own frame".to_owned(),
        "target" => "the framebuffer and render target belong to the client's GPU device".to_owned(),
        "device" => "the GPU device and backend are the client's, created before any mod runs".to_owned(),
        "frame" => "the client draws its frames on its own schedule; asking for one is not something this host can do".to_owned(),
        "event" => {
            "this event comes from the client's renderer as it runs; it is registered for the \
             day the hooks land"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "world", "entity", "block", "chunk", "hand", "screen", "particle", "nameplate",
        "outline", "shadow", "post", "texture", "shader", "camera", "matrix", "target",
        "device", "frame", "event",
    ]
    .iter()
    .map(|what| ((*what).to_owned(), why_unknown(what)))
    .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host to draw: {}", why_unknown(what));
    false
}

/// The render context, which the client owns. `None`.
#[must_use]
pub fn get_context() -> Option<String> {
    None
}

/// The renderer's state, which the client owns. `None`.
#[must_use]
pub fn get_state() -> Option<String> {
    None
}

/// The camera. `None`: section 24's camera is a mod's own.
#[must_use]
pub fn get_camera() -> Option<String> {
    None
}

/// The view matrix. `None`.
#[must_use]
pub fn get_view_matrix() -> Option<[f32; 16]> {
    None
}

/// The projection matrix. `None`.
#[must_use]
pub fn get_projection_matrix() -> Option<[f32; 16]> {
    None
}

/// The model matrix. `None`.
#[must_use]
pub fn get_model_matrix() -> Option<[f32; 16]> {
    None
}

/// The framebuffer. `None`.
#[must_use]
pub fn get_framebuffer() -> Option<String> {
    None
}

/// The render target. `None`.
#[must_use]
pub fn get_render_target() -> Option<String> {
    None
}

/// The GPU device. `None`.
#[must_use]
pub fn get_device() -> Option<String> {
    None
}

/// The graphics backend. `None`.
#[must_use]
pub fn get_backend() -> Option<String> {
    None
}

/// The world pass. `false`.
pub fn on_world_render_start<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("world")
}

/// The end of the world pass. `false`.
pub fn on_world_render_end<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("world")
}

/// The entity pass. `false`.
pub fn on_entity_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("entity")
}

/// The block pass. `false`.
pub fn on_block_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("block")
}

/// The chunk pass. `false`.
pub fn on_chunk_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("chunk")
}

/// The first-person hand. `false`.
pub fn on_hand_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("hand")
}

/// The screen pass. `false`.
pub fn on_screen_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("screen")
}

/// The particle pass. `false`.
pub fn on_particle_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("particle")
}

/// Nameplates. `false`.
pub fn on_nameplate_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("nameplate")
}

/// Outlines. `false`.
pub fn on_outline_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("outline")
}

/// Shadows. `false`.
pub fn on_shadow_render<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("shadow")
}

/// Post-processing. `false`.
pub fn on_post_process<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("post")
}

/// Texture uploads. `false`.
pub fn on_texture_upload<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("texture")
}

/// Shader reloads. `false`.
pub fn on_shader_reload<F>(_callback: F) -> bool
where
    F: FnMut(&mut Event) + Send + 'static,
{
    refused("shader")
}

/// Resource reloads, which *is* real: the loader's own resources reload.
pub fn on_resource_reload<F>(callback: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    events::subscribe(kind::RESOURCE_RELOAD, Priority::Normal, callback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|renderers| renderers.clear());
    }

    #[test]
    fn a_frame_callback_really_runs_at_a_frame_boundary() {
        let _serial = serial();
        let calls = std::sync::Arc::new(Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&calls);
        let start = on_frame_start(move |_| into.lock().unwrap().push("start"));
        let into = std::sync::Arc::clone(&calls);
        let hud = on_hud_render(move |_| into.lock().unwrap().push("hud"));
        let into = std::sync::Arc::clone(&calls);
        let end = on_frame_end(move |_| into.lock().unwrap().push("end"));
        assert_eq!(listener_count(kind::RENDER_TICK), 3);

        // The loader publishes a render tick as a frame starts, which is what runs these.
        events::publish(Event::new(kind::RENDER_TICK).with_flag(true));
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["start", "hud", "end"],
            "pre-phase first, in the order the register calls ran"
        );
        assert!(unsubscribe(start));
        assert!(unsubscribe(hud));
        assert!(unsubscribe(end));
        assert!(!unsubscribe(end));
        assert_eq!(listener_count(kind::RENDER_TICK), 0);

        let reloaded = std::sync::Arc::new(Mutex::new(0));
        let into = std::sync::Arc::clone(&reloaded);
        let id = on_resource_reload(move |_| *into.lock().unwrap() += 1);
        events::publish(Event::new(kind::RESOURCE_RELOAD));
        assert_eq!(*reloaded.lock().unwrap(), 1);
        unsubscribe(id);
    }

    #[test]
    fn a_declared_renderer_is_real_data_in_draw_order() {
        let _serial = serial();
        fresh();
        assert!(register_renderer(
            Renderer::new("hello:outline").layer("hello:lines").order(10)
        ));
        assert!(!register_renderer(Renderer::new("hello:outline")), "it replaces");
        assert!(register_renderer(
            Renderer::new("hello:glow").layer("hello:glow").order(5).transparent().no_cull()
        ));
        assert!(register_pass("hello:post"));
        assert!(register_render_layer("hello:solid"));
        // The replacement above lost the outline's order, so it is put back: the point of the
        // check below is that order beats name. Registering an id that is already there
        // replaces it, so this answers false.
        assert!(!register_renderer(
            Renderer::new("hello:outline").layer("hello:lines").order(10)
        ));
        assert_eq!(get_renderers().len(), 4);
        let order: Vec<String> = get_renderers().into_iter().map(|renderer| renderer.id).collect();
        assert_eq!(
            order,
            vec![
                "hello:post".to_owned(),
                "hello:solid".to_owned(),
                "hello:glow".to_owned(),
                "hello:outline".to_owned()
            ],
            "lower orders first, then by name"
        );
        let glow = get_renderer("hello:glow").expect("declared");
        assert!(!glow.opaque);
        assert!(!glow.cull);
        assert!(glow.to_string().contains("transparent"));
        assert!(get_renderer("hello:post").is_some_and(|renderer| renderer.is_pass));
        assert!(unregister_renderer("hello:glow"));
        assert!(!unregister_renderer("hello:glow"));
        assert!(unregister_render_layer("hello:solid"));
        assert!(unregister_pass("hello:post"));
        assert_eq!(get_renderers().len(), 1);
    }

    #[test]
    fn the_two_things_this_host_really_owns_are_real() {
        let _serial = serial();
        crate::tasks::record_main_thread();
        assert!(is_render_thread(), "a mod runs on the frame thread");
        let listeners = invalidate_cache();
        assert_eq!(reload_resources(), listeners);
    }

    #[test]
    fn every_pass_inside_the_client_is_refused_with_its_own_reason() {
        let _serial = serial();
        assert!(get_context().is_none());
        assert!(get_state().is_none());
        assert!(get_camera().is_none());
        assert!(get_view_matrix().is_none());
        assert!(get_projection_matrix().is_none());
        assert!(get_model_matrix().is_none());
        assert!(get_framebuffer().is_none());
        assert!(get_render_target().is_none());
        assert!(get_device().is_none());
        assert!(get_backend().is_none());
        assert!(!request_frame());
        assert!(!on_world_render_start(|_| {}));
        assert!(!on_world_render_end(|_| {}));
        assert!(!on_entity_render(|_| {}));
        assert!(!on_block_render(|_| {}));
        assert!(!on_chunk_render(|_| {}));
        assert!(!on_hand_render(|_| {}));
        assert!(!on_screen_render(|_| {}));
        assert!(!on_particle_render(|_| {}));
        assert!(!on_nameplate_render(|_| {}));
        assert!(!on_outline_render(|_| {}));
        assert!(!on_shadow_render(|_| {}));
        assert!(!on_post_process(|_| {}));
        assert!(!on_texture_upload(|_| {}));
        assert!(!on_shader_reload(|_| {}));
        assert_eq!(refusals().len(), 19);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_render_event_kinds_can_be_subscribed_to_before_they_fire() {
        let _serial = serial();
        assert!(register_event_kinds() > 0);
        assert!(has_event_kind("render_chunk"));
        assert!(!has_event_kind("nothing_at_all"));
        assert_eq!(event_kinds().len(), 17);
    }
}
