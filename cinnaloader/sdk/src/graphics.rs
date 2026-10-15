//! GPU resources: a ledger a mod keeps, and the device it cannot touch.
//!
//! The client creates and owns the GPU device, its adapter, its backend and every buffer,
//! texture, pipeline and framebuffer on it. This host has no handle on any of that, so
//! everything that would *create*, *submit*, *read* or *resize* on the GPU is refused with
//! its own reason.
//!
//! What is real, and useful for a mod with a graphics budget, is the **ledger**: a mod
//! records the resources it would ask for — a vertex buffer of so many bytes, a texture of a
//! given format and size, a pipeline for a kind of work — and gets a handle back. The ledger
//! then answers how many there are, how many bytes they would take, and what a mod's own
//! validation found. Nothing is created on the GPU, and [`is_created_on_the_gpu`] says so
//! plainly rather than pretending a handle is a resource.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let vertex = graphics::create_vertex_buffer("hello", 1024, "vertex");
//! assert_eq!(graphics::get_resource_count(), 1);
//! assert_eq!(graphics::get_memory_usage(), 1024);
//! assert!(!graphics::is_created_on_the_gpu(vertex));
//! assert!(graphics::release_resource(vertex));
//! assert!(graphics::get_device().is_none());
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// What kind of GPU resource a mod is describing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ResourceKind {
    Buffer,
    VertexBuffer,
    IndexBuffer,
    UniformBuffer,
    StorageBuffer,
    Texture,
    TextureView,
    Sampler,
    Shader,
    Pipeline,
    RenderPipeline,
    ComputePipeline,
    BindGroup,
    BindGroupLayout,
    Framebuffer,
    RenderTarget,
    DepthBuffer,
    CommandBuffer,
}

impl ResourceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buffer => "buffer",
            Self::VertexBuffer => "vertex_buffer",
            Self::IndexBuffer => "index_buffer",
            Self::UniformBuffer => "uniform_buffer",
            Self::StorageBuffer => "storage_buffer",
            Self::Texture => "texture",
            Self::TextureView => "texture_view",
            Self::Sampler => "sampler",
            Self::Shader => "shader",
            Self::Pipeline => "pipeline",
            Self::RenderPipeline => "render_pipeline",
            Self::ComputePipeline => "compute_pipeline",
            Self::BindGroup => "bind_group",
            Self::BindGroupLayout => "bind_group_layout",
            Self::Framebuffer => "framebuffer",
            Self::RenderTarget => "render_target",
            Self::DepthBuffer => "depth_buffer",
            Self::CommandBuffer => "command_buffer",
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A handle to a resource a mod described. It is a ledger entry, not a GPU object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceHandle(pub u64);

impl fmt::Display for ResourceHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{}", self.0)
    }
}

/// What a mod said about a resource it would ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDesc {
    pub handle: ResourceHandle,
    pub kind: ResourceKind,
    pub label: String,
    pub size: u64,
    /// The format, for a texture or a buffer that has one.
    pub format: String,
    /// What it would be used for, as the backend names it.
    pub usage: String,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    /// Whether the mod released it.
    pub released: bool,
}

impl fmt::Display for ResourceDesc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} '{}' of {} bytes{}",
            self.kind,
            self.label,
            self.size,
            if self.released { " (released)" } else { "" }
        )
    }
}

/// The ledger of what a mod would ask the GPU for.
#[derive(Debug, Default, Clone)]
pub struct Ledger {
    resources: BTreeMap<u64, ResourceDesc>,
    validation_errors: Vec<String>,
    device_lost: Option<String>,
}

impl Ledger {
    /// How many resources are in it, released ones included.
    #[must_use]
    pub fn count(&self) -> usize {
        self.resources.values().filter(|resource| !resource.released).count()
    }

    /// How many bytes the live ones would take.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.resources
            .values()
            .filter(|resource| !resource.released)
            .map(|resource| resource.size)
            .sum()
    }

    /// Every resource, released ones included.
    #[must_use]
    pub fn all(&self) -> Vec<ResourceDesc> {
        self.resources.values().cloned().collect()
    }

    /// One resource.
    #[must_use]
    pub fn get(&self, handle: ResourceHandle) -> Option<ResourceDesc> {
        self.resources.get(&handle.0).cloned()
    }

    /// Records what a mod's own validation found.
    pub fn note_validation_error(&mut self, error: &str) {
        self.validation_errors.push(error.to_owned());
    }

    /// What validation found.
    #[must_use]
    pub fn validation_errors(&self) -> Vec<String> {
        self.validation_errors.clone()
    }

    /// Records that the GPU device was lost, which only the client can really know.
    pub fn note_device_lost(&mut self, reason: &str) {
        self.device_lost = Some(reason.to_owned());
    }

    #[must_use]
    pub fn device_lost_reason(&self) -> Option<String> {
        self.device_lost.clone()
    }
}

fn ledger() -> &'static Mutex<Ledger> {
    static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(Ledger::default()))
}

fn with<T>(body: impl FnOnce(&mut Ledger) -> T) -> T {
    let mut guard = ledger().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn next_handle() -> ResourceHandle {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    ResourceHandle(NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Records a resource of a kind, and answers its handle.
pub fn describe(kind: ResourceKind, label: &str, size: u64) -> ResourceHandle {
    let handle = next_handle();
    with(|ledger| {
        ledger.resources.insert(
            handle.0,
            ResourceDesc {
                handle,
                kind,
                label: label.to_owned(),
                size,
                format: String::new(),
                usage: String::new(),
                width: 0,
                height: 0,
                depth: 0,
                released: false,
            },
        );
    });
    handle
}

/// Records a two-dimensional resource, such as a texture.
pub fn describe_2d(
    kind: ResourceKind,
    label: &str,
    format: &str,
    width: u32,
    height: u32,
) -> ResourceHandle {
    let handle = describe(kind, label, u64::from(width) * u64::from(height) * 4);
    with(|ledger| {
        if let Some(resource) = ledger.resources.get_mut(&handle.0) {
            resource.format = format.to_owned();
            resource.width = width;
            resource.height = height;
            resource.depth = 1;
        }
    });
    handle
}

fn record(kind: ResourceKind, label: &str, size: u64, usage: &str) -> ResourceHandle {
    let handle = describe(kind, label, size);
    with(|ledger| {
        if let Some(resource) = ledger.resources.get_mut(&handle.0) {
            resource.usage = usage.to_owned();
        }
    });
    handle
}

/// Records a buffer.
pub fn create_buffer(label: &str, size: u64, usage: &str) -> ResourceHandle {
    record(ResourceKind::Buffer, label, size, usage)
}

/// Records a vertex buffer.
pub fn create_vertex_buffer(label: &str, size: u64, usage: &str) -> ResourceHandle {
    record(ResourceKind::VertexBuffer, label, size, usage)
}

/// Records an index buffer.
pub fn create_index_buffer(label: &str, size: u64, usage: &str) -> ResourceHandle {
    record(ResourceKind::IndexBuffer, label, size, usage)
}

/// Records a uniform buffer.
pub fn create_uniform_buffer(label: &str, size: u64, usage: &str) -> ResourceHandle {
    record(ResourceKind::UniformBuffer, label, size, usage)
}

/// Records a storage buffer.
pub fn create_storage_buffer(label: &str, size: u64, usage: &str) -> ResourceHandle {
    record(ResourceKind::StorageBuffer, label, size, usage)
}

/// Records a texture.
pub fn create_texture(label: &str, format: &str, width: u32, height: u32) -> ResourceHandle {
    describe_2d(ResourceKind::Texture, label, format, width, height)
}

/// Records a texture view.
pub fn create_texture_view(label: &str, format: &str, width: u32, height: u32) -> ResourceHandle {
    describe_2d(ResourceKind::TextureView, label, format, width, height)
}

/// Records a sampler.
pub fn create_sampler(label: &str) -> ResourceHandle {
    describe(ResourceKind::Sampler, label, 0)
}

/// Records a shader.
pub fn create_shader(label: &str, size: u64) -> ResourceHandle {
    describe(ResourceKind::Shader, label, size)
}

/// Records a pipeline.
pub fn create_pipeline(label: &str) -> ResourceHandle {
    describe(ResourceKind::Pipeline, label, 0)
}

/// Records a render pipeline.
pub fn create_render_pipeline(label: &str) -> ResourceHandle {
    describe(ResourceKind::RenderPipeline, label, 0)
}

/// Records a compute pipeline.
pub fn create_compute_pipeline(label: &str) -> ResourceHandle {
    describe(ResourceKind::ComputePipeline, label, 0)
}

/// Records a bind group.
pub fn create_bind_group(label: &str) -> ResourceHandle {
    describe(ResourceKind::BindGroup, label, 0)
}

/// Records a bind group layout.
pub fn create_bind_group_layout(label: &str) -> ResourceHandle {
    describe(ResourceKind::BindGroupLayout, label, 0)
}

/// Records a framebuffer.
pub fn create_framebuffer(label: &str, width: u32, height: u32) -> ResourceHandle {
    describe_2d(ResourceKind::Framebuffer, label, "rgba8", width, height)
}

/// Records a render target.
pub fn create_render_target(label: &str, width: u32, height: u32) -> ResourceHandle {
    describe_2d(ResourceKind::RenderTarget, label, "rgba8", width, height)
}

/// Records a depth buffer.
pub fn create_depth_buffer(label: &str, width: u32, height: u32) -> ResourceHandle {
    describe_2d(ResourceKind::DepthBuffer, label, "depth32", width, height)
}

/// Records a command buffer.
pub fn create_command_buffer(label: &str) -> ResourceHandle {
    describe(ResourceKind::CommandBuffer, label, 0)
}

/// Lets a resource go from the ledger, and answers whether it was there.
pub fn release_resource(handle: ResourceHandle) -> bool {
    with(|ledger| match ledger.resources.get_mut(&handle.0) {
        Some(resource) if !resource.released => {
            resource.released = true;
            true
        }
        _ => false,
    })
}

/// How many live resources the ledger holds.
#[must_use]
pub fn get_resource_count() -> usize {
    with(|ledger| ledger.count())
}

/// How many bytes they would take.
#[must_use]
pub fn get_memory_usage() -> u64 {
    with(|ledger| ledger.bytes())
}

/// Every resource in the ledger.
#[must_use]
pub fn get_resources() -> Vec<ResourceDesc> {
    with(|ledger| ledger.all())
}

/// One resource.
#[must_use]
pub fn get_resource(handle: ResourceHandle) -> Option<ResourceDesc> {
    with(|ledger| ledger.get(handle))
}

/// Whether a resource was really created on the GPU, which it never was.
#[must_use]
pub fn is_created_on_the_gpu(_handle: ResourceHandle) -> bool {
    false
}

/// What a mod's own validation found.
#[must_use]
pub fn get_validation_errors() -> Vec<String> {
    with(|ledger| ledger.validation_errors())
}

/// Notes that a mod's own check failed.
pub fn note_validation_error(error: &str) {
    with(|ledger| ledger.note_validation_error(error));
}

/// Why the GPU device was lost, when it was: only the client can really know, so this is what
/// a mod recorded.
#[must_use]
pub fn get_device_lost_reason() -> Option<String> {
    with(|ledger| ledger.device_lost_reason())
}

/// Records that the device was lost.
pub fn note_device_lost(reason: &str) {
    with(|ledger| ledger.note_device_lost(reason));
}

/// What a backend the client might be using is called, for a caller that wants to see the
/// names this host would ask for.
pub const BACKENDS: [&str; 4] = ["vulkan", "metal", "dx12", "gl"];

/// The GPU device. `None`: the client owns it.
#[must_use]
pub fn get_device() -> Option<()> {
    None
}

/// The adapter. `None`: the client chose it before any mod ran.
#[must_use]
pub fn get_adapter() -> Option<String> {
    None
}

/// The backend. `None`: the client does not tell a mod, and this host has no handle on it.
#[must_use]
pub fn get_backend() -> Option<String> {
    None
}

/// What the backend can do. Empty.
#[must_use]
pub fn get_capabilities() -> Vec<(String, String)> {
    Vec::new()
}

/// The backend's limits. Empty.
#[must_use]
pub fn get_limits() -> Vec<(String, u64)> {
    Vec::new()
}

/// Submits work to the GPU. `false`.
pub fn submit() -> bool {
    refused("submit")
}

/// Reads a buffer back from the GPU. `None`.
#[must_use]
pub fn read_buffer(_handle: ResourceHandle) -> Option<Vec<u8>> {
    refused_read("read")
}

/// Writes a buffer on the GPU. `false`.
pub fn write_buffer(_handle: ResourceHandle, _bytes: &[u8]) -> bool {
    refused("write")
}

/// Reads a texture back. `None`.
#[must_use]
pub fn read_texture(_handle: ResourceHandle) -> Option<Vec<u8>> {
    refused_read("read")
}

/// Writes a texture. `false`.
pub fn write_texture(_handle: ResourceHandle, _bytes: &[u8]) -> bool {
    refused("write")
}

/// Resizes a texture. `false`.
pub fn resize_texture(_handle: ResourceHandle, _width: u32, _height: u32) -> bool {
    refused("resize")
}

/// Resizes a render target. `false`.
pub fn resize_render_target(_handle: ResourceHandle, _width: u32, _height: u32) -> bool {
    refused("resize")
}

/// Why a GPU question has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "device" => {
            "the GPU device belongs to the client, created before any mod runs, and this host \
             has no handle on it"
                .to_owned()
        }
        "backend" => {
            "which backend the client uses is its own choice, and it does not tell a mod; \
             these APIs are deliberately not tied to one"
                .to_owned()
        }
        "create" => {
            "creating a GPU resource would need the client's device; a mod's ledger entry is \
             real data, and is_created_on_the_gpu answers false for every one of them"
                .to_owned()
        }
        "submit" => "submitting work goes to the client's queue, which this host cannot reach".to_owned(),
        "read" => "reading back from the GPU needs the client's device".to_owned(),
        "write" => "writing to the GPU needs the client's device".to_owned(),
        "resize" => "resizing a GPU resource needs the client's device".to_owned(),
        "lost" => {
            "only the client knows when and why its device was lost; this answers what a mod \
             recorded itself"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["device", "backend", "create", "submit", "read", "write", "resize", "lost"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about the GPU: {}", why_unknown(what));
    false
}

fn refused_read(what: &str) -> Option<Vec<u8>> {
    crate::warn!("a mod asked this host about the GPU: {}", why_unknown(what));
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|ledger| *ledger = Ledger::default());
    }

    #[test]
    fn a_ledger_records_what_a_mod_would_ask_for() {
        let _serial = serial();
        fresh();
        let vertex = create_vertex_buffer("hello:vertices", 1024, "vertex");
        let texture = create_texture("hello:atlas", "rgba8", 16, 16);
        let _shader = create_shader("hello:shader", 2048);
        assert_eq!(get_resource_count(), 3);
        assert_eq!(get_memory_usage(), 1024 + 16 * 16 * 4 + 2048);
        assert_eq!(get_resources().len(), 3);
        assert!(!is_created_on_the_gpu(vertex), "nothing is created on the GPU here");

        let described = get_resource(texture).expect("it is in the ledger");
        assert_eq!(described.kind, ResourceKind::Texture);
        assert_eq!(described.format, "rgba8");
        assert_eq!(described.width, 16);
        assert_eq!((described.height, described.depth), (16, 1));
        assert!(described.to_string().contains("texture"), "{described}");
        let described = get_resource(vertex).expect("it is in the ledger");
        assert_eq!(described.usage, "vertex");
        assert_eq!(described.size, 1024);
        assert!(!described.released);
        assert_eq!(ResourceHandle(vertex.0).to_string(), format!("#{}", vertex.0));

        assert!(release_resource(vertex));
        assert!(!release_resource(vertex), "releasing twice does nothing");
        assert_eq!(get_resource_count(), 2);
        assert!(get_resource(vertex).is_some_and(|resource| resource.released));
        assert!(!release_resource(ResourceHandle(9999)));
    }

    #[test]
    fn every_kind_of_resource_can_be_described() {
        let _serial = serial();
        fresh();
        for (kind, handle) in [
            (ResourceKind::Buffer, create_buffer("a", 16, "uniform")),
            (ResourceKind::IndexBuffer, create_index_buffer("b", 16, "index")),
            (ResourceKind::UniformBuffer, create_uniform_buffer("c", 16, "uniform")),
            (ResourceKind::StorageBuffer, create_storage_buffer("d", 16, "storage")),
            (ResourceKind::TextureView, create_texture_view("e", "rgba8", 4, 4)),
            (ResourceKind::Sampler, create_sampler("f")),
            (ResourceKind::Pipeline, create_pipeline("g")),
            (ResourceKind::RenderPipeline, create_render_pipeline("h")),
            (ResourceKind::ComputePipeline, create_compute_pipeline("i")),
            (ResourceKind::BindGroup, create_bind_group("j")),
            (ResourceKind::BindGroupLayout, create_bind_group_layout("k")),
            (ResourceKind::Framebuffer, create_framebuffer("l", 8, 8)),
            (ResourceKind::RenderTarget, create_render_target("m", 8, 8)),
            (ResourceKind::DepthBuffer, create_depth_buffer("n", 8, 8)),
            (ResourceKind::CommandBuffer, create_command_buffer("o")),
        ] {
            assert_eq!(
                get_resource(handle).map(|resource| resource.kind),
                Some(kind),
                "{kind} did not land"
            );
        }
        assert_eq!(get_resource_count(), 15);
        assert_eq!(ResourceKind::VertexBuffer.as_str(), "vertex_buffer");
        assert!(BACKENDS.contains(&"vulkan"));
        assert!(BACKENDS.contains(&"dx12"));
    }

    #[test]
    fn validation_errors_and_a_lost_device_are_what_a_mod_recorded() {
        let _serial = serial();
        fresh();
        assert!(get_validation_errors().is_empty());
        assert!(get_device_lost_reason().is_none());
        note_validation_error("a pipeline without a shader");
        assert_eq!(get_validation_errors().len(), 1);
        note_device_lost("the probe lost it");
        assert_eq!(
            get_device_lost_reason(),
            Some("the probe lost it".to_owned())
        );
    }

    #[test]
    fn the_device_and_everything_on_it_are_refused() {
        let _serial = serial();
        fresh();
        assert!(get_device().is_none());
        assert!(get_adapter().is_none());
        assert!(get_backend().is_none());
        assert!(get_capabilities().is_empty());
        assert!(get_limits().is_empty());
        assert!(!submit());
        let handle = create_buffer("hello:buffer", 8, "uniform");
        assert!(read_buffer(handle).is_none());
        assert!(!write_buffer(handle, &[1, 2, 3]));
        assert!(read_texture(handle).is_none());
        assert!(!write_texture(handle, &[1, 2, 3]));
        assert!(!resize_texture(handle, 4, 4));
        assert!(!resize_render_target(handle, 4, 4));
        assert_eq!(refusals().len(), 8);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
