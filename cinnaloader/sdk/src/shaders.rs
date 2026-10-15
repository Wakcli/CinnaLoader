//! Shaders: what a mod ships and what it can reflect on, and the pipeline it cannot reach.
//!
//! A shader is *source text* here, and that makes most of this section real: a mod registers a
//! shader with its name, its stage, its source, its defines, its variants and its includes; it
//! can look up a uniform, list the uniforms and attributes it declared, ask for the reflection
//! a compiler would give it (which is parsed from the source a mod wrote, not from a real
//! compiler), set a define or a variant, and register a material or a post-process pass with
//! the hook it belongs to.
//!
//! What cannot be done is the client's half: compiling and loading into its pipeline, applying
//! and removing a post-process pass, the active shader list, the compile errors of a real
//! compiler, the pipeline cache, and the features the client's backend supports. Those are
//! refused with their own reasons.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! shaders::register(Shader::new("hello:outline", ShaderStage::Fragment)
//!     .source("uniform mat4 u_proj;\nuniform vec4 u_color;\nvoid main() {}")
//!     .define("OUTLINE_WIDTH", "2"));
//! assert_eq!(shaders::get_uniforms("hello:outline").len(), 2);
//! assert!(shaders::get_uniform("hello:outline", "u_color").is_some());
//! assert!(!shaders::compile("hello:outline"));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// Which stage of the pipeline a shader is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Geometry,
    Compute,
    /// Bedrock's own, which is a whole material rather than one stage.
    Material,
}

impl ShaderStage {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vertex => "vertex",
            Self::Fragment => "fragment",
            Self::Geometry => "geometry",
            Self::Compute => "compute",
            Self::Material => "material",
        }
    }
}

impl fmt::Display for ShaderStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where a post-process pass runs, which is what the wishlist calls a shader hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShaderHook {
    BeforeWorldOpaque,
    AfterWorldOpaque,
    BeforeWorldTranslucent,
    AfterWorldTranslucent,
    BeforeEntityRender,
    AfterEntityRender,
    BeforeHandRender,
    AfterHandRender,
    BeforeHudRender,
    AfterHudRender,
    BeforePostProcessing,
    AfterPostProcessing,
    BeforeFinalComposite,
}

impl ShaderHook {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeWorldOpaque => "before_world_opaque",
            Self::AfterWorldOpaque => "after_world_opaque",
            Self::BeforeWorldTranslucent => "before_world_translucent",
            Self::AfterWorldTranslucent => "after_world_translucent",
            Self::BeforeEntityRender => "before_entity_render",
            Self::AfterEntityRender => "after_entity_render",
            Self::BeforeHandRender => "before_hand_render",
            Self::AfterHandRender => "after_hand_render",
            Self::BeforeHudRender => "before_hud_render",
            Self::AfterHudRender => "after_hud_render",
            Self::BeforePostProcessing => "before_post_processing",
            Self::AfterPostProcessing => "after_post_processing",
            Self::BeforeFinalComposite => "before_final_composite",
        }
    }

    /// Every hook, in the order a frame runs them.
    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::BeforeWorldOpaque,
            Self::AfterWorldOpaque,
            Self::BeforeWorldTranslucent,
            Self::AfterWorldTranslucent,
            Self::BeforeEntityRender,
            Self::AfterEntityRender,
            Self::BeforeHandRender,
            Self::AfterHandRender,
            Self::BeforeHudRender,
            Self::AfterHudRender,
            Self::BeforePostProcessing,
            Self::AfterPostProcessing,
            Self::BeforeFinalComposite,
        ]
    }
}

impl fmt::Display for ShaderHook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A uniform or attribute a shader declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderVariable {
    pub name: String,
    /// The type as written, such as `mat4` or `float`.
    pub kind: String,
    pub uniform: bool,
}

impl ShaderVariable {
    /// How many components the type has, which is what a caller binding it wants to know.
    #[must_use]
    pub fn components(&self) -> u32 {
        let kind = self.kind.as_str();
        match kind {
            "float" | "int" | "uint" | "bool" => 1,
            "vec2" | "ivec2" | "uvec2" | "bvec2" => 2,
            "vec3" | "ivec3" | "uvec3" | "bvec3" => 3,
            "vec4" | "ivec4" | "uvec4" | "bvec4" => 4,
            "mat2" => 4,
            "mat3" => 9,
            "mat4" => 16,
            _ => 0,
        }
    }
}

/// A shader a mod ships.
#[derive(Debug, Clone, PartialEq)]
pub struct Shader {
    pub id: String,
    pub owner: String,
    pub stage: ShaderStage,
    pub source: String,
    pub defines: BTreeMap<String, String>,
    /// The variant it is used as, such as `alpha_test` or `no_cull`.
    pub variant: String,
    pub includes: Vec<String>,
    /// A uniform value a mod set for its own bookkeeping.
    pub values: BTreeMap<String, String>,
}

impl Shader {
    /// A shader with no source yet.
    #[must_use]
    pub fn new(id: &str, stage: ShaderStage) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            stage,
            source: String::new(),
            defines: BTreeMap::new(),
            variant: String::new(),
            includes: Vec::new(),
            values: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn source(mut self, source: &str) -> Self {
        self.source = source.to_owned();
        self
    }

    /// Adds an include, which is another shader's source pasted in by name.
    #[must_use]
    pub fn include(mut self, id: &str) -> Self {
        self.includes.push(id.to_owned());
        self
    }

    #[must_use]
    pub fn define(mut self, name: &str, value: &str) -> Self {
        self.defines.insert(name.to_owned(), value.to_owned());
        self
    }

    #[must_use]
    pub fn variant(mut self, variant: &str) -> Self {
        self.variant = variant.to_owned();
        self
    }

    /// Every uniform the source declares, read out of the text rather than from a compiler.
    #[must_use]
    pub fn uniforms(&self) -> Vec<ShaderVariable> {
        self.variables().into_iter().filter(|v| v.uniform).collect()
    }

    /// Every attribute the source declares.
    #[must_use]
    pub fn attributes(&self) -> Vec<ShaderVariable> {
        self.variables().into_iter().filter(|v| !v.uniform).collect()
    }

    fn variables(&self) -> Vec<ShaderVariable> {
        let mut found = Vec::new();
        let mut source = self.source.clone();
        // Includes are pasted in, so a uniform declared in one is reflected too.
        for include in &self.includes {
            if let Some(other) = get(include) {
                source.push('\n');
                source.push_str(&other.source);
            }
        }
        let mut lines: Vec<&str> = source.lines().collect();
        lines.dedup();
        for line in lines {
            let line = line.trim();
            let (uniform, rest) = match line.strip_prefix("uniform ") {
                Some(rest) => (true, rest),
                None => match line.strip_prefix("in ") {
                    Some(rest) => (false, rest),
                    None => continue,
                },
            };
            let mut parts = rest.split_whitespace();
            let Some(kind) = parts.next() else { continue };
            let Some(name) = parts.next() else { continue };
            let name = name
                .trim_end_matches(';')
                .trim_end_matches('[')
                .trim_end_matches(']')
                .to_owned();
            found.push(ShaderVariable {
                name,
                kind: kind.to_owned(),
                uniform,
            });
        }
        found
    }

    /// What a compiler would tell a caller about this shader, which here is what its source
    /// says plus the defines it carries.
    #[must_use]
    pub fn reflection(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .variables()
            .into_iter()
            .map(|variable| {
                format!(
                    "{} {} {}",
                    if variable.uniform { "uniform" } else { "attribute" },
                    variable.kind,
                    variable.name
                )
            })
            .collect();
        for (name, value) in &self.defines {
            lines.push(format!("define {name}={value}"));
        }
        lines
    }
}

impl fmt::Display for Shader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}, {} byte(s), {} uniform(s))",
            self.id,
            self.stage,
            self.owner,
            self.source.len(),
            self.uniforms().len()
        )
    }
}

/// A material a mod declares: which shader it uses and with what defines.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub id: String,
    pub owner: String,
    pub shader: String,
    pub defines: BTreeMap<String, String>,
}

/// A post-process pass a mod declares.
#[derive(Debug, Clone, PartialEq)]
pub struct PostProcess {
    pub id: String,
    pub owner: String,
    pub shader: String,
    pub hook: ShaderHook,
    /// Where it sits among the passes on the same hook.
    pub order: i32,
    /// Whether it is in the chain right now.
    pub active: bool,
}

fn shaders() -> &'static Mutex<BTreeMap<String, Shader>> {
    static SHADERS: OnceLock<Mutex<BTreeMap<String, Shader>>> = OnceLock::new();
    SHADERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, Shader>) -> T) -> T {
    let mut guard = shaders().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn materials() -> &'static Mutex<BTreeMap<String, Material>> {
    static MATERIALS: OnceLock<Mutex<BTreeMap<String, Material>>> = OnceLock::new();
    MATERIALS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_materials<T>(body: impl FnOnce(&mut BTreeMap<String, Material>) -> T) -> T {
    let mut guard = materials().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn passes() -> &'static Mutex<Vec<PostProcess>> {
    static PASSES: OnceLock<Mutex<Vec<PostProcess>>> = OnceLock::new();
    PASSES.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_passes<T>(body: impl FnOnce(&mut Vec<PostProcess>) -> T) -> T {
    let mut guard = passes().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a shader.
pub fn register(shader: Shader) -> bool {
    with(|shaders| shaders.insert(shader.id.clone(), shader).is_none())
}

/// Forgets a shader.
pub fn unregister(id: &str) -> bool {
    with(|shaders| shaders.remove(id).is_some())
}

/// A shader by id.
#[must_use]
pub fn get(id: &str) -> Option<Shader> {
    with(|shaders| shaders.get(id).cloned())
}

/// Every shader a mod registered.
#[must_use]
pub fn get_all() -> Vec<Shader> {
    with(|shaders| shaders.values().cloned().collect())
}

/// Registers another shader's source as an include a shader can name.
pub fn register_include(id: &str, source: &str) -> bool {
    register(Shader::new(id, ShaderStage::Fragment).source(source))
}

/// Forgets an include.
pub fn unregister_include(id: &str) -> bool {
    unregister(id)
}

/// A uniform a shader declares.
#[must_use]
pub fn get_uniform(id: &str, name: &str) -> Option<ShaderVariable> {
    get(id).and_then(|shader| {
        shader
            .uniforms()
            .into_iter()
            .find(|variable| variable.name == name)
    })
}

/// Every uniform a shader declares.
#[must_use]
pub fn get_uniforms(id: &str) -> Vec<ShaderVariable> {
    get(id).map(|shader| shader.uniforms()).unwrap_or_default()
}

/// Every attribute a shader declares.
#[must_use]
pub fn get_attributes(id: &str) -> Vec<ShaderVariable> {
    get(id).map(|shader| shader.attributes()).unwrap_or_default()
}

/// What a compiler would say about it, as far as its source goes.
#[must_use]
pub fn get_reflection(id: &str) -> Vec<String> {
    get(id).map(|shader| shader.reflection()).unwrap_or_default()
}

/// Sets a uniform's value for the mod's own bookkeeping. Setting the *client's* uniform would
/// need its pipeline.
pub fn set_uniform(id: &str, name: &str, value: &str) -> bool {
    with(|shaders| match shaders.get_mut(id) {
        Some(shader) => {
            shader.values.insert(name.to_owned(), value.to_owned());
            true
        }
        None => false,
    })
}

/// The value a mod set for a uniform.
#[must_use]
pub fn get_uniform_value(id: &str, name: &str) -> Option<String> {
    get(id).and_then(|shader| shader.values.get(name).cloned())
}

/// Sets a define, which changes the source a shader would compile from.
pub fn set_define(id: &str, name: &str, value: &str) -> bool {
    with(|shaders| match shaders.get_mut(id) {
        Some(shader) => {
            shader.defines.insert(name.to_owned(), value.to_owned());
            true
        }
        None => false,
    })
}

/// Sets the variant a shader is used as.
pub fn set_variant(id: &str, variant: &str) -> bool {
    with(|shaders| match shaders.get_mut(id) {
        Some(shader) => {
            shader.variant = variant.to_owned();
            true
        }
        None => false,
    })
}

/// Declares a material.
pub fn register_material(material: Material) -> bool {
    with_materials(|materials| materials.insert(material.id.clone(), material).is_none())
}

/// Forgets a material.
pub fn unregister_material(id: &str) -> bool {
    with_materials(|materials| materials.remove(id).is_some())
}

/// A declared material.
#[must_use]
pub fn get_material(id: &str) -> Option<Material> {
    with_materials(|materials| materials.get(id).cloned())
}

/// Every declared material.
#[must_use]
pub fn get_materials() -> Vec<Material> {
    with_materials(|materials| materials.values().cloned().collect())
}

/// Declares a post-process pass, which is real data with a hook and an order.
pub fn register_post_process(pass: PostProcess) -> bool {
    with_passes(|passes| {
        if passes.iter().any(|held| held.id == pass.id) {
            return false;
        }
        passes.push(pass);
        passes.sort_by_key(|pass| (pass.hook, pass.order, pass.id.clone()));
        true
    })
}

/// Forgets a post-process pass.
pub fn unregister_post_process(id: &str) -> bool {
    with_passes(|passes| {
        let before = passes.len();
        passes.retain(|pass| pass.id != id);
        passes.len() != before
    })
}

/// Every declared pass, in the order a frame would run them.
#[must_use]
pub fn get_post_processes() -> Vec<PostProcess> {
    with_passes(|passes| passes.clone())
}

/// The passes declared for one hook.
#[must_use]
pub fn get_post_processes_for(hook: ShaderHook) -> Vec<PostProcess> {
    with_passes(|passes| {
        passes
            .iter()
            .filter(|pass| pass.hook == hook)
            .cloned()
            .collect()
    })
}

/// Turns a pass into the chain a mod owns, and answers whether it was there.
pub fn apply_post_process(id: &str) -> bool {
    with_passes(|passes| match passes.iter_mut().find(|pass| pass.id == id) {
        Some(pass) => {
            pass.active = true;
            true
        }
        None => false,
    })
}

/// Takes a pass out of that chain.
pub fn remove_post_process(id: &str) -> bool {
    with_passes(|passes| match passes.iter_mut().find(|pass| pass.id == id) {
        Some(pass) => {
            pass.active = false;
            true
        }
        None => false,
    })
}

/// Every pass a mod has turned on, which is not the client's active shader list.
#[must_use]
pub fn get_active_shaders() -> Vec<String> {
    with_passes(|passes| {
        passes
            .iter()
            .filter(|pass| pass.active)
            .map(|pass| pass.shader.clone())
            .collect()
    })
}

/// Registers a pipeline a mod would build from a shader, which is a ledger entry in section
/// 31's terms.
pub fn create_pipeline(id: &str) -> crate::graphics::ResourceHandle {
    crate::graphics::create_render_pipeline(id)
}

/// A pipeline cache, which only the client has. Empty.
#[must_use]
pub fn get_pipeline_cache() -> Vec<String> {
    Vec::new()
}

/// Clears it. `false`.
pub fn clear_pipeline_cache() -> bool {
    refused("cache")
}

/// Loads a shader into the client. `false`.
pub fn load(_id: &str) -> bool {
    refused("pipeline")
}

/// Compiles a shader. `false`: this host has no compiler.
pub fn compile(_id: &str) -> bool {
    refused("compile")
}

/// Compiles one variant of a shader. `false`.
pub fn compile_variant(_id: &str, _variant: &str) -> bool {
    refused("compile")
}

/// Compiles every registered shader. `false`.
pub fn compile_all() -> usize {
    refused("compile");
    0
}

/// Reloads the shaders. `false`.
pub fn reload() -> bool {
    refused("pipeline")
}

/// What a real compiler said, which this host never hears. Empty.
#[must_use]
pub fn get_compile_errors() -> Vec<String> {
    Vec::new()
}

/// What the client's backend supports. Empty.
#[must_use]
pub fn get_supported_features() -> Vec<String> {
    Vec::new()
}

/// Why a shader question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "compile" => {
            "compiling a shader is the client's pipeline's job; this host reflects on the \
             source a mod wrote instead of compiling it"
                .to_owned()
        }
        "pipeline" => {
            "loading a shader into the client, and its pipeline cache, need the client's GPU \
             device (section 31)"
                .to_owned()
        }
        "cache" => "the pipeline cache belongs to the client's device".to_owned(),
        "active" => {
            "the client's active shader list is inside its renderer; get_active_shaders \
             answers the passes a mod turned on instead"
                .to_owned()
        }
        "errors" => "a real compiler's errors are the client's; this host reports none rather than inventing them".to_owned(),
        "features" => "what the backend supports is the client's, from its own adapter".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["compile", "pipeline", "cache", "active", "errors", "features"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about shaders: {}", why_unknown(what));
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|shaders| shaders.clear());
        with_materials(|materials| materials.clear());
        with_passes(|passes| passes.clear());
    }

    fn outline() -> Shader {
        Shader::new("hello:outline", ShaderStage::Fragment)
            .source(
                "uniform mat4 u_proj;\nuniform vec4 u_color;\nin vec2 v_uv;\nvoid main() {}",
            )
            .define("OUTLINE_WIDTH", "2")
    }

    #[test]
    fn a_shader_is_registered_and_reflected_on() {
        let _serial = serial();
        fresh();
        assert!(register(outline()));
        let shader = get("hello:outline").expect("registered");
        assert_eq!(shader.stage, ShaderStage::Fragment);
        assert_eq!(shader.uniforms().len(), 2);
        assert_eq!(shader.attributes().len(), 1);
        assert_eq!(get_uniforms("hello:outline").len(), 2);
        assert_eq!(get_attributes("hello:outline").len(), 1);
        let uniform = get_uniform("hello:outline", "u_color").expect("declared");
        assert_eq!(uniform.kind, "vec4");
        assert_eq!(uniform.components(), 4);
        assert!(get_uniform("hello:outline", "nope").is_none());
        assert_eq!(get_uniform("hello:outline", "u_proj").map(|u| u.components()), Some(16));
        let reflection = get_reflection("hello:outline");
        assert!(reflection.iter().any(|line| line.contains("u_color")), "{reflection:?}");
        assert!(reflection.iter().any(|line| line.contains("OUTLINE_WIDTH")), "{reflection:?}");
        assert!(!register(outline()), "it replaces");
        assert!(shader.to_string().contains("fragment"), "{shader}");
        assert!(!shader.source.is_empty());
        assert!(unregister("hello:outline"));
        assert!(!unregister("hello:outline"));
        assert_eq!(ShaderStage::Vertex.to_string(), "vertex");
    }

    #[test]
    fn an_include_brings_its_uniforms_with_it() {
        let _serial = serial();
        fresh();
        register_include("hello:common", "uniform mat4 u_view;\nuniform float u_time;");
        register(outline().include("hello:common"));
        let uniforms = get_uniforms("hello:outline");
        assert_eq!(uniforms.len(), 4, "two of its own and two from the include");
        assert!(uniforms.iter().any(|uniform| uniform.name == "u_time"));
        assert!(unregister_include("hello:common"));
    }

    #[test]
    fn defines_variants_and_uniform_values_are_the_mods_own_data() {
        let _serial = serial();
        fresh();
        register(outline());
        assert!(set_define("hello:outline", "ALPHA_TEST", "1"));
        assert!(set_variant("hello:outline", "alpha_test"));
        assert!(set_uniform("hello:outline", "u_color", "1,0,0,1"));
        assert_eq!(
            get_uniform_value("hello:outline", "u_color"),
            Some("1,0,0,1".to_owned())
        );
        let shader = get("hello:outline").expect("registered");
        assert_eq!(shader.variant, "alpha_test");
        assert_eq!(shader.defines.get("ALPHA_TEST").map(String::as_str), Some("1"));
        assert!(!set_define("nothing", "X", "1"));
        assert!(!set_variant("nothing", "v"));
        assert!(!set_uniform("nothing", "u", "1"));
        assert_eq!(get_uniform_value("hello:outline", "nothing"), None);
    }

    #[test]
    fn a_material_and_a_post_process_pass_are_declared_data() {
        let _serial = serial();
        fresh();
        register(outline());
        assert!(register_material(Material {
            id: "hello:glow".to_owned(),
            owner: crate::metadata::id(),
            shader: "hello:outline".to_owned(),
            defines: BTreeMap::new(),
        }));
        assert!(!register_material(Material {
            id: "hello:glow".to_owned(),
            owner: crate::metadata::id(),
            shader: "hello:outline".to_owned(),
            defines: BTreeMap::new(),
        }));
        assert_eq!(get_materials().len(), 1);
        assert_eq!(
            get_material("hello:glow").map(|material| material.shader),
            Some("hello:outline".to_owned())
        );
        assert!(unregister_material("hello:glow"));

        assert!(register_post_process(PostProcess {
            id: "hello:bloom".to_owned(),
            owner: crate::metadata::id(),
            shader: "hello:outline".to_owned(),
            hook: ShaderHook::BeforeFinalComposite,
            order: 10,
            active: false,
        }));
        assert!(register_post_process(PostProcess {
            id: "hello:blur".to_owned(),
            owner: crate::metadata::id(),
            shader: "hello:outline".to_owned(),
            hook: ShaderHook::BeforeWorldOpaque,
            order: 0,
            active: false,
        }));
        assert!(!register_post_process(PostProcess {
            id: "hello:bloom".to_owned(),
            owner: crate::metadata::id(),
            shader: "hello:outline".to_owned(),
            hook: ShaderHook::BeforeFinalComposite,
            order: 10,
            active: false,
        }), "once");
        let passes = get_post_processes();
        assert_eq!(passes.len(), 2);
        assert_eq!(passes[0].id, "hello:blur", "hooks sort by the order a frame runs them");
        assert_eq!(get_post_processes_for(ShaderHook::BeforeFinalComposite).len(), 1);
        assert!(get_active_shaders().is_empty());
        assert!(apply_post_process("hello:bloom"));
        assert_eq!(get_active_shaders(), vec!["hello:outline".to_owned()]);
        assert!(remove_post_process("hello:bloom"));
        assert!(get_active_shaders().is_empty());
        assert!(!apply_post_process("nothing"));
        assert!(!remove_post_process("nothing"));
        assert!(unregister_post_process("hello:bloom"));
        assert!(!unregister_post_process("hello:bloom"));
        assert_eq!(ShaderHook::all().len(), 13);
        assert_eq!(ShaderHook::AfterHudRender.to_string(), "after_hud_render");
    }

    #[test]
    fn the_clients_pipeline_and_compiler_are_refused() {
        let _serial = serial();
        fresh();
        register(outline());
        assert!(!load("hello:outline"));
        assert!(!compile("hello:outline"));
        assert!(!compile_variant("hello:outline", "alpha_test"));
        assert_eq!(compile_all(), 0);
        assert!(!reload());
        assert!(get_compile_errors().is_empty());
        assert!(get_supported_features().is_empty());
        assert!(get_pipeline_cache().is_empty());
        assert!(!clear_pipeline_cache());
        let handle = create_pipeline("hello:pipeline");
        assert_eq!(
            crate::graphics::get_resource(handle).map(|resource| resource.kind),
            Some(crate::graphics::ResourceKind::RenderPipeline),
            "a pipeline a mod declares lands in section 31's ledger"
        );
        assert!(!crate::graphics::is_created_on_the_gpu(handle));
        assert_eq!(refusals().len(), 6);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn a_shader_variable_knows_how_big_it_is() {
        let variable = |kind: &str| ShaderVariable {
            name: "v".to_owned(),
            kind: kind.to_owned(),
            uniform: true,
        };
        assert_eq!(variable("float").components(), 1);
        assert_eq!(variable("vec3").components(), 3);
        assert_eq!(variable("mat4").components(), 16);
        assert_eq!(variable("sampler2D").components(), 0);
    }
}
