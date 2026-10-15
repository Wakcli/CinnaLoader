//! Generators: a declaration of what a mod makes, a template that renders it, and the files it
//! writes through [`crate::fs`], with an incremental check and a manifest.
//!
//! A [`Generator`] is a declaration — an id, an [`OutputKind`], a target path pattern, an
//! [`OutputFormat`] and the [`Template`] it renders. Registering one is registered once:
//! [`register`] refuses a second declaration under the same id, [`get`] and [`get_all`] read
//! them back. A template has a documented placeholder syntax, `{{name}}`; a placeholder with
//! no value is refused *by name* rather than left as `{{name}}`, and each value is escaped
//! for the generator's format (`Json` escapes a quote, `Html` an angle bracket, `Rust` a
//! backslash), so a value cannot break the document it lands in.
//!
//! [`run`] renders one output and [`run_all`] a batch over N inputs, both answering how many
//! files were written and their real byte sizes; an output path that leaves the generator's
//! own root, or that names the **client's** assets or a resource pack, is refused. An output
//! whose bytes already match what is on disk is skipped — [`invalidate`] forces the next run
//! to write it anyway — [`dry_run`] reports what would be written without writing it, and
//! [`generate_manifest`] records every output with its size and hash, writes it and reads it
//! back, with a [`Manifest::verify`] that names a file which no longer matches.
//!
//! ```
//! use cinnaloader::generators::{self, Generator, OutputKind, Template};
//!
//! # let root = std::env::temp_dir().join("cinnaloader-doc-generators");
//! # let _ = std::fs::remove_dir_all(&root);
//! # std::fs::create_dir_all(&root).unwrap();
//! # cinnaloader::fs::set_root(root);
//! generators::register_template(Template::text("greeting", "hello {{name}}"));
//! generators::register(
//!     Generator::new("greeter", OutputKind::Documentation)
//!         .root("generated/greeter")
//!         .target("{{name}}.txt")
//!         .template("greeting"),
//! );
//!
//! let done = generators::run("greeter", &[("name".to_owned(), "world".to_owned())]).unwrap();
//! assert_eq!(done.size, 11);
//! assert_eq!(cinnaloader::fs::read_text(&done.path).unwrap(), "hello world");
//!
//! let again = generators::run("greeter", &[("name".to_owned(), "world".to_owned())]).unwrap();
//! assert!(again.skipped, "the same bytes are already there");
//! # generators::unregister("greeter");
//! # generators::unregister_template("greeting");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::serialization::{self, Map, Value};

/// What a generator makes. It names the shape of the output, not its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputKind {
    Manifest,
    Registry,
    Documentation,
    Assets,
    Sources,
}

impl OutputKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::Registry => "registry",
            Self::Documentation => "documentation",
            Self::Assets => "assets",
            Self::Sources => "sources",
        }
    }
}

impl fmt::Display for OutputKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How an output is spelled, and therefore how the values that land in it are escaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    /// Plain text: a value goes in as it is.
    Text,
    /// A JSON string body: quotes, backslashes and control characters are escaped.
    Json,
    /// HTML text: `&`, `<`, `>`, `"` and `'` are escaped.
    Html,
    /// Inside a Rust string literal: quotes, backslashes and newlines are escaped.
    Rust,
}

impl OutputFormat {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Html => "html",
            Self::Rust => "rust",
        }
    }

    /// The file extension this format goes by.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Json => "json",
            Self::Html => "html",
            Self::Rust => "rs",
        }
    }

    /// The value as it may be written in this format.
    #[must_use]
    pub fn escape(self, value: &str) -> String {
        match self {
            Self::Text => value.to_owned(),
            Self::Json => escape_json(value),
            Self::Html => escape_html(value),
            Self::Rust => escape_rust(value),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn escape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", other as u32)),
            other => out.push(other),
        }
    }
    out
}

fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

fn escape_rust(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", other as u32)),
            other => out.push(other),
        }
    }
    out
}

/// A named template: a body with `{{name}}` placeholders, and the format it is written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub body: String,
    pub format: OutputFormat,
}

impl Template {
    #[must_use]
    pub fn new(name: &str, body: &str, format: OutputFormat) -> Self {
        Self {
            name: name.to_owned(),
            body: body.to_owned(),
            format,
        }
    }

    #[must_use]
    pub fn text(name: &str, body: &str) -> Self {
        Self::new(name, body, OutputFormat::Text)
    }

    #[must_use]
    pub fn json(name: &str, body: &str) -> Self {
        Self::new(name, body, OutputFormat::Json)
    }

    #[must_use]
    pub fn html(name: &str, body: &str) -> Self {
        Self::new(name, body, OutputFormat::Html)
    }

    #[must_use]
    pub fn rust(name: &str, body: &str) -> Self {
        Self::new(name, body, OutputFormat::Rust)
    }

    /// Renders it with the named values, in the format it was registered with.
    pub fn render(&self, values: &[(String, String)]) -> Result<String, GeneratorError> {
        self.render_with(&pairs(values), self.format)
    }

    fn render_with(
        &self,
        values: &BTreeMap<String, String>,
        format: OutputFormat,
    ) -> Result<String, GeneratorError> {
        substitute(&self.body, values, format)
    }
}

/// One generator: what it makes, where it writes, and the template it renders.
///
/// The target is a pattern rendered with the same values as the template, and `root` is the
/// one directory it may write under — a path that leaves it is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generator {
    pub id: String,
    pub kind: OutputKind,
    pub root: String,
    pub target: String,
    pub format: OutputFormat,
    pub template: String,
    pub inputs: Vec<String>,
    pub dependencies: Vec<String>,
    pub parameters: BTreeMap<String, String>,
}

impl Generator {
    #[must_use]
    pub fn new(id: &str, kind: OutputKind) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            root: format!("generated/{id}"),
            target: "output.txt".to_owned(),
            format: OutputFormat::Text,
            template: id.to_owned(),
            inputs: Vec::new(),
            dependencies: Vec::new(),
            parameters: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn root(mut self, root: &str) -> Self {
        self.root = root.to_owned();
        self
    }

    #[must_use]
    pub fn target(mut self, target: &str) -> Self {
        self.target = target.to_owned();
        self
    }

    #[must_use]
    pub fn format(mut self, format: OutputFormat) -> Self {
        self.format = format;
        self
    }

    #[must_use]
    pub fn template(mut self, template: &str) -> Self {
        self.template = template.to_owned();
        self
    }

    #[must_use]
    pub fn input(mut self, input: &str) -> Self {
        self.inputs.push(input.to_owned());
        self
    }

    #[must_use]
    pub fn depends_on(mut self, id: &str) -> Self {
        self.dependencies.push(id.to_owned());
        self
    }

    #[must_use]
    pub fn parameter(mut self, name: &str, value: &str) -> Self {
        self.parameters.insert(name.to_owned(), value.to_owned());
        self
    }
}

/// One file a run wrote, or would write: where, how big and the hash of its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    pub generator: String,
    pub path: String,
    pub size: u64,
    pub hash: u64,
    /// True when the bytes already on disk matched, so nothing was written.
    pub skipped: bool,
}

/// What a run over one or many inputs did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationReport {
    pub generator: String,
    pub written: usize,
    pub skipped: usize,
    /// The bytes actually written, which is the sum over the outputs that were not skipped.
    pub bytes: u64,
    pub dry_run: bool,
    pub outputs: Vec<Generated>,
}

/// One file in a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub hash: u64,
}

/// What was generated, with a real total, written as JSON and read back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
    pub total: u64,
}

impl Manifest {
    #[must_use]
    pub fn from_outputs(outputs: &[Generated]) -> Self {
        let entries: Vec<ManifestEntry> = outputs
            .iter()
            .map(|output| ManifestEntry {
                path: output.path.clone(),
                size: output.size,
                hash: output.hash,
            })
            .collect();
        let total = entries.iter().map(|entry| entry.size).sum();
        Self { entries, total }
    }

    #[must_use]
    pub fn entry(&self, path: &str) -> Option<&ManifestEntry> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    /// The paths whose file is gone or no longer holds the recorded size and bytes.
    #[must_use]
    pub fn verify(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| {
                crate::fs::file_size(&entry.path) != entry.size
                    || crate::fs::hash(&entry.path) != Some(entry.hash)
            })
            .map(|entry| entry.path.clone())
            .collect()
    }

    fn to_json(&self) -> Result<String, GeneratorError> {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let mut held = Map::new();
                held.insert("path", Value::str(&entry.path));
                held.insert("size", Value::Int(entry.size as i64));
                held.insert("hash", Value::Int(entry.hash as i64));
                Value::Map(held)
            })
            .collect();
        let mut map = Map::new();
        map.insert("generated", Value::List(entries));
        map.insert("total", Value::Int(self.total as i64));
        serialization::to_json(&Value::Map(map)).map_err(|error| GeneratorError::Malformed(error.to_string()))
    }

    /// Reads one back from the JSON a previous [`generate_manifest`] wrote.
    pub fn read(path: &str) -> Result<Self, GeneratorError> {
        let text = crate::fs::read_text(path).map_err(|error| GeneratorError::Io(error.to_string()))?;
        let value =
            serialization::from_json(&text).map_err(|error| GeneratorError::Malformed(error.to_string()))?;
        let map = value
            .as_map()
            .ok_or_else(|| GeneratorError::Malformed("a manifest is a JSON object".to_owned()))?;
        let mut entries = Vec::new();
        for held in map.get("generated").and_then(Value::as_list).unwrap_or_default() {
            let Some(held) = held.as_map() else {
                continue;
            };
            let Some(path) = held.get("path").and_then(Value::as_str) else {
                continue;
            };
            entries.push(ManifestEntry {
                path: path.to_owned(),
                size: held.get("size").and_then(Value::as_int).unwrap_or_default() as u64,
                hash: held.get("hash").and_then(Value::as_int).unwrap_or_default() as u64,
            });
        }
        let total = entries.iter().map(|entry| entry.size).sum();
        Ok(Self { entries, total })
    }
}

/// How far a generator got on its last run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Generated,
    Skipped,
    Failed,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Generated => "generated",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a generator could not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratorError {
    /// No generator is registered under that id.
    NoSuchGenerator(String),
    /// A generator is already registered under that id.
    AlreadyRegistered(String),
    /// No template is registered under that name.
    NoSuchTemplate(String),
    /// The template used a placeholder no value was given for.
    MissingValue { name: String },
    /// The template itself is not well formed.
    Malformed(String),
    /// The output path leaves the generator's own root.
    OutsideRoot(String),
    /// Writing there is refused, with the reason.
    Refused(String),
    /// The filesystem refused, with its message.
    Io(String),
}

impl fmt::Display for GeneratorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchGenerator(id) => write!(formatter, "no generator is registered as {id}"),
            Self::AlreadyRegistered(id) => {
                write!(formatter, "a generator is already registered as {id}")
            }
            Self::NoSuchTemplate(name) => write!(formatter, "no template is registered as {name}"),
            Self::MissingValue { name } => {
                write!(formatter, "the template has no value for `{name}`, and will not leave a placeholder behind")
            }
            Self::Malformed(why) => write!(formatter, "malformed template: {why}"),
            Self::OutsideRoot(path) => {
                write!(formatter, "{path} is outside the generator's own root")
            }
            Self::Refused(why) => formatter.write_str(why),
            Self::Io(why) => write!(formatter, "the filesystem refused: {why}"),
        }
    }
}

impl std::error::Error for GeneratorError {}

#[derive(Default)]
struct Generators {
    generators: Vec<Generator>,
    templates: BTreeMap<String, Template>,
    outputs: Vec<Generated>,
    status: BTreeMap<String, Status>,
    errors: Vec<String>,
    forced: Vec<String>,
}

fn generators() -> &'static Mutex<Generators> {
    static GENERATORS: OnceLock<Mutex<Generators>> = OnceLock::new();
    GENERATORS.get_or_init(|| Mutex::new(Generators::default()))
}

fn with<T>(body: impl FnOnce(&mut Generators) -> T) -> T {
    let mut guard = generators().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn pairs(values: &[(String, String)]) -> BTreeMap<String, String> {
    values.iter().cloned().collect()
}

fn substitute(
    body: &str,
    values: &BTreeMap<String, String>,
    format: OutputFormat,
) -> Result<String, GeneratorError> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    loop {
        let Some(start) = rest.find("{{") else {
            out.push_str(rest);
            return Ok(out);
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(GeneratorError::Malformed(
                "a placeholder opens with {{ and is never closed".to_owned(),
            ));
        };
        let name = after[..end].trim();
        if name.is_empty() {
            return Err(GeneratorError::Malformed("a placeholder with no name".to_owned()));
        }
        let Some(value) = values.get(name) else {
            return Err(GeneratorError::MissingValue { name: name.to_owned() });
        };
        out.push_str(&format.escape(value));
        rest = &after[end + 2..];
    }
}

/// Registers a generator, refusing a second declaration under the same id.
pub fn register(generator: Generator) -> bool {
    if generator.id.trim().is_empty() {
        return false;
    }
    with(|state| {
        if state.generators.iter().any(|held| held.id == generator.id) {
            crate::warn!("a generator is already registered as {}", generator.id);
            return false;
        }
        state.generators.push(generator);
        true
    })
}

/// Forgets a generator, and everything it left behind.
pub fn unregister(id: &str) -> bool {
    with(|state| {
        let before = state.generators.len();
        state.generators.retain(|held| held.id != id);
        let removed = state.generators.len() != before;
        if removed {
            state.status.remove(id);
            state.forced.retain(|held| held != id);
        }
        removed
    })
}

/// A generator by id.
#[must_use]
pub fn get(id: &str) -> Option<Generator> {
    with(|state| state.generators.iter().find(|held| held.id == id).cloned())
}

/// Every generator, in registration order.
#[must_use]
pub fn get_all() -> Vec<Generator> {
    with(|state| state.generators.clone())
}

/// The inputs a generator declared, which are the paths whose change forces a rerun.
#[must_use]
pub fn get_inputs(id: &str) -> Vec<String> {
    get(id).map(|generator| generator.inputs).unwrap_or_default()
}

/// The dependencies a generator declared, by id.
#[must_use]
pub fn get_dependencies(id: &str) -> Vec<String> {
    get(id)
        .map(|generator| generator.dependencies)
        .unwrap_or_default()
}

/// Every output written so far, across every generator.
#[must_use]
pub fn get_outputs() -> Vec<Generated> {
    with(|state| state.outputs.clone())
}

/// How far a generator got on its last run.
#[must_use]
pub fn get_status(id: &str) -> Status {
    with(|state| state.status.get(id).copied().unwrap_or_default())
}

/// What the failed runs left, newest last.
#[must_use]
pub fn get_errors() -> Vec<String> {
    with(|state| state.errors.clone())
}

/// Replaces a generator's parameters, which are the values its template falls back on.
pub fn set_parameters(id: &str, parameters: &[(String, String)]) -> bool {
    with(|state| {
        let Some(generator) = state.generators.iter_mut().find(|held| held.id == id) else {
            return false;
        };
        for (name, value) in parameters {
            generator.parameters.insert(name.clone(), value.clone());
        }
        true
    })
}

/// A generator's parameters, in name order.
#[must_use]
pub fn get_parameters(id: &str) -> Vec<(String, String)> {
    get(id)
        .map(|generator| generator.parameters.into_iter().collect())
        .unwrap_or_default()
}

/// Marks a generator's outputs as stale, so the next run writes them even when the bytes
/// already match, and answers how many outputs sit under its root.
pub fn invalidate(id: &str) -> usize {
    with(|state| {
        let Some(generator) = state.generators.iter().find(|held| held.id == id).cloned() else {
            return 0;
        };
        if !state.forced.iter().any(|held| held == id) {
            state.forced.push(id.to_owned());
        }
        let prefix = format!("{}/", generator.root.trim_end_matches('/'));
        state
            .outputs
            .iter()
            .filter(|output| output.path.starts_with(&prefix))
            .count()
    })
}

/// Registers a template, refusing a second one under the same name.
pub fn register_template(template: Template) -> bool {
    if template.name.trim().is_empty() {
        return false;
    }
    with(|state| {
        if state.templates.contains_key(&template.name) {
            crate::warn!("a template is already registered as {}", template.name);
            return false;
        }
        state.templates.insert(template.name.clone(), template);
        true
    })
}

/// Forgets a template.
pub fn unregister_template(name: &str) -> bool {
    with(|state| state.templates.remove(name).is_some())
}

/// Renders a registered template with the named values, refusing a value that is missing.
pub fn render_template(name: &str, values: &[(String, String)]) -> Result<String, GeneratorError> {
    with(|state| {
        let Some(template) = state.templates.get(name).cloned() else {
            return Err(GeneratorError::NoSuchTemplate(name.to_owned()));
        };
        template.render(values)
    })
}

fn plan_path(generator: &Generator, relative: &str) -> Result<String, GeneratorError> {
    let relative = relative.trim();
    let outside = || GeneratorError::OutsideRoot(format!("{}/{}", generator.root.trim_end_matches('/'), relative));
    if relative.is_empty() || Path::new(relative).is_absolute() || relative.contains('\\') {
        return Err(outside());
    }
    if relative.split('/').any(|part| part == ".." || part.is_empty()) {
        return Err(outside());
    }
    let full = format!("{}/{}", generator.root.trim_end_matches('/'), relative);
    if let Some(what) = client_reserved(&full) {
        let why = why_unknown(what);
        crate::warn!("a generator was refused writing {full}: {why}");
        return Err(GeneratorError::Refused(why));
    }
    Ok(full)
}

fn client_reserved(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    let under = |prefix: &str| lower == prefix || lower.starts_with(&format!("{prefix}/"));
    if under("client/assets") {
        return Some("client_assets");
    }
    if under("resourcepacks") || under("client/resourcepacks") {
        return Some("resource_pack");
    }
    None
}

fn merged_values(generator: &Generator, values: &[(String, String)]) -> BTreeMap<String, String> {
    let mut merged = generator.parameters.clone();
    for (name, value) in values {
        merged.insert(name.clone(), value.clone());
    }
    merged
}

fn store(
    state: &mut Generators,
    generator: &Generator,
    relative: &str,
    bytes: Vec<u8>,
    dry: bool,
) -> Result<Generated, GeneratorError> {
    let path = plan_path(generator, relative)?;
    let hash = crate::fs::hash_bytes(&bytes);
    let forced = state.forced.iter().any(|held| held == &generator.id);
    let skipped = !forced && crate::fs::is_file(&path) && crate::fs::hash(&path) == Some(hash);
    if !dry && !skipped {
        crate::fs::write(&path, &bytes).map_err(|error| GeneratorError::Io(error.to_string()))?;
    }
    let done = Generated {
        generator: generator.id.clone(),
        path,
        size: bytes.len() as u64,
        hash,
        skipped,
    };
    if !dry {
        state.forced.retain(|held| held != &generator.id);
        state.outputs.push(done.clone());
        let status = match skipped {
            true => Status::Skipped,
            false => Status::Generated,
        };
        state.status.insert(generator.id.clone(), status);
    }
    Ok(done)
}

fn run_one(
    state: &mut Generators,
    generator: &Generator,
    values: &BTreeMap<String, String>,
    dry: bool,
) -> Result<Generated, GeneratorError> {
    let relative = substitute(&generator.target, values, OutputFormat::Text)?;
    let template = state
        .templates
        .get(&generator.template)
        .cloned()
        .ok_or_else(|| GeneratorError::NoSuchTemplate(generator.template.clone()))?;
    let body = template.render_with(values, generator.format)?;
    store(state, generator, &relative, body.into_bytes(), dry)
}

fn found<'a>(state: &'a Generators, id: &str) -> Result<&'a Generator, GeneratorError> {
    state
        .generators
        .iter()
        .find(|held| held.id == id)
        .ok_or_else(|| GeneratorError::NoSuchGenerator(id.to_owned()))
}

fn failed(state: &mut Generators, id: &str, error: &GeneratorError) {
    state.errors.push(error.to_string());
    state.status.insert(id.to_owned(), Status::Failed);
}

fn report(generator: &str, outputs: &[Generated], dry_run: bool) -> GenerationReport {
    let written = outputs.iter().filter(|output| !output.skipped).count();
    let bytes = outputs
        .iter()
        .filter(|output| !output.skipped)
        .map(|output| output.size)
        .sum();
    GenerationReport {
        generator: generator.to_owned(),
        written,
        skipped: outputs.len() - written,
        bytes,
        dry_run,
        outputs: outputs.to_vec(),
    }
}

/// Renders one output for a generator and writes it, skipping it when the same bytes are
/// already there.
pub fn run(id: &str, values: &[(String, String)]) -> Result<Generated, GeneratorError> {
    with(|state| {
        let generator = match found(state, id) {
            Ok(generator) => generator.clone(),
            Err(error) => return Err(error),
        };
        let merged = merged_values(&generator, values);
        let result = run_one(state, &generator, &merged, false);
        if let Err(error) = &result {
            failed(state, id, error);
        }
        result
    })
}

/// Renders one output per input and writes them all, answering how many were written and their
/// real byte sizes.
pub fn run_all(id: &str, inputs: &[Vec<(String, String)>]) -> Result<GenerationReport, GeneratorError> {
    with(|state| {
        let generator = match found(state, id) {
            Ok(generator) => generator.clone(),
            Err(error) => return Err(error),
        };
        let mut outputs = Vec::with_capacity(inputs.len());
        for values in inputs {
            let merged = merged_values(&generator, values);
            match run_one(state, &generator, &merged, false) {
                Ok(done) => outputs.push(done),
                Err(error) => {
                    failed(state, id, &error);
                    return Err(error);
                }
            }
        }
        Ok(report(id, &outputs, false))
    })
}

/// The same as [`run_all`], but it writes nothing: the report says what would be written and
/// whether it would be skipped.
pub fn dry_run(id: &str, inputs: &[Vec<(String, String)>]) -> Result<GenerationReport, GeneratorError> {
    with(|state| {
        let generator = match found(state, id) {
            Ok(generator) => generator.clone(),
            Err(error) => return Err(error),
        };
        let mut outputs = Vec::with_capacity(inputs.len());
        for values in inputs {
            let merged = merged_values(&generator, values);
            outputs.push(run_one(state, &generator, &merged, true)?);
        }
        Ok(report(id, &outputs, true))
    })
}

/// Writes the manifest of everything generated so far under a generator's root, and reads it
/// straight back.
pub fn generate_manifest(id: &str) -> Result<Manifest, GeneratorError> {
    let path = with(|state| {
        let generator = found(state, id)?.clone();
        let manifest = Manifest::from_outputs(&state.outputs);
        let bytes = manifest.to_json()?;
        let done = store(state, &generator, "MANIFEST.json", bytes.into_bytes(), false)?;
        Ok(done.path)
    })?;
    Manifest::read(&path)
}

/// Writes a registry of every registered generator under a generator's root.
pub fn generate_registry(id: &str) -> Result<Generated, GeneratorError> {
    with(|state| {
        let generator = found(state, id)?.clone();
        let mut body = String::new();
        for held in &state.generators {
            body.push_str(&format!("{}\t{}\t{}\n", held.id, held.kind, held.target));
        }
        store(state, &generator, "registry.txt", body.into_bytes(), false)
    })
}

/// Writes a description of a generator under its own root.
pub fn generate_documentation(id: &str) -> Result<Generated, GeneratorError> {
    with(|state| {
        let generator = found(state, id)?.clone();
        let mut body = format!(
            "# {}\n\nkind: {}\ntarget: {}\nformat: {}\n",
            generator.id, generator.kind, generator.target, generator.format
        );
        for input in &generator.inputs {
            body.push_str(&format!("input: {input}\n"));
        }
        for dependency in &generator.dependencies {
            body.push_str(&format!("depends: {dependency}\n"));
        }
        for (name, value) in &generator.parameters {
            body.push_str(&format!("parameter: {name}={value}\n"));
        }
        store(state, &generator, "README.md", body.into_bytes(), false)
    })
}

/// Writes the asset index a generator declares under its own root.
pub fn generate_assets(id: &str) -> Result<Generated, GeneratorError> {
    with(|state| {
        let generator = found(state, id)?.clone();
        let mut body = String::from("assets:\n");
        for input in &generator.inputs {
            body.push_str(&format!("  - {}\n", OutputFormat::Text.escape(input)));
        }
        store(state, &generator, "assets.txt", body.into_bytes(), false)
    })
}

/// Writes a Rust source stub for a generator under its own root.
pub fn generate_sources(id: &str) -> Result<Generated, GeneratorError> {
    with(|state| {
        let generator = found(state, id)?.clone();
        let body = format!(
            "pub const GENERATOR: &str = \"{}\";\npub const TARGET: &str = \"{}\";\n",
            OutputFormat::Rust.escape(&generator.id),
            OutputFormat::Rust.escape(&generator.target),
        );
        store(state, &generator, "generated.rs", body.into_bytes(), false)
    })
}

/// Why writing into the client's own assets or its resource packs has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client_assets" => {
            "a mod cannot write into the client's own assets: that tree belongs to the game, not \
             to a mod (section 31)"
                .to_owned()
        }
        "resource_pack" => {
            "a client resource pack is packed and signed by the client, so this host has no \
             handle to write one"
                .to_owned()
        }
        _ => format!("{what} is not something this host can generate"),
    }
}

/// What this host cannot generate.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client_assets", "resource_pack"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|state| *state = Generators::default());
    }

    fn rooted(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cinnaloader-gen-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());
        root
    }

    fn page() -> Template {
        Template::text("page", "v={{value}}\n")
    }

    fn generator() -> Generator {
        Generator::new("pages", OutputKind::Documentation)
            .root("gen/pages")
            .target("{{name}}.txt")
            .template("page")
    }

    #[test]
    fn a_declaration_reads_back_and_a_duplicate_is_refused() {
        let _serial = serial();
        fresh();
        let held = generator().input("src/page.rs").depends_on("shared");
        assert!(register(held.clone()));
        assert!(!register(held.clone()), "the same id is refused");
        assert_eq!(get("pages"), Some(held.clone()));
        assert_eq!(get_all().len(), 1);
        assert_eq!(get_inputs("pages"), vec!["src/page.rs".to_owned()]);
        assert_eq!(get_dependencies("pages"), vec!["shared".to_owned()]);
        assert_eq!(get_status("pages"), Status::Idle);

        assert!(set_parameters("pages", &[("value".to_owned(), "7".to_owned())]));
        assert_eq!(get_parameters("pages"), vec![("value".to_owned(), "7".to_owned())]);
        assert!(get("pages").is_some_and(|held| held.parameters["value"] == "7"));
        assert!(!set_parameters("nothing", &[]));
        assert_eq!(get_inputs("nothing"), Vec::<String>::new());

        assert!(unregister("pages"));
        assert!(!unregister("pages"));
        assert_eq!(get("pages"), None);
    }

    #[test]
    fn a_template_refuses_a_missing_value_by_name_and_escapes_for_its_format() {
        let _serial = serial();
        fresh();
        assert!(register_template(page()));
        assert_eq!(
            render_template("page", &[("value".to_owned(), "1".to_owned())]).unwrap(),
            "v=1\n"
        );
        assert_eq!(
            render_template("page", &[]),
            Err(GeneratorError::MissingValue { name: "value".to_owned() })
        );
        assert!(render_template("nothing", &[]).is_err());
        assert!(register_template(Template::text("bad", "{{}}")));
        assert!(matches!(
            render_template("bad", &[]),
            Err(GeneratorError::Malformed(_))
        ));

        assert!(register_template(Template::json("blob", "{\"v\": \"{{value}}\"}")));
        assert_eq!(
            render_template("blob", &[("value".to_owned(), "a\"b\n".to_owned())]).unwrap(),
            "{\"v\": \"a\\\"b\\n\"}"
        );
        assert_eq!(OutputFormat::Json.escape("a\\b\tc"), "a\\\\b\\tc");
        assert_eq!(OutputFormat::Html.escape("<&>"), "&lt;&amp;&gt;");
        assert_eq!(OutputFormat::Rust.escape("a\"b"), "a\\\"b");
        assert_eq!(OutputFormat::Text.escape("as is"), "as is");
        assert_eq!(OutputFormat::Json.extension(), "json");

        assert!(!register_template(page()), "the same name is refused");
        assert!(unregister_template("page"));
        assert!(!unregister_template("page"));
    }

    #[test]
    fn a_run_writes_real_bytes_and_a_batch_reports_them() {
        let _serial = serial();
        fresh();
        let root = rooted("batch");
        register_template(page());
        assert!(register(generator()));

        let one = run("pages", &[("name".to_owned(), "alpha".to_owned()), ("value".to_owned(), "1".to_owned())])
            .expect("a run");
        assert!(!one.skipped);
        assert_eq!(one.path, "gen/pages/alpha.txt");
        assert_eq!(one.size, 4);
        assert_eq!(one.hash, crate::fs::hash_bytes(b"v=1\n"));
        assert_eq!(crate::fs::read_text("gen/pages/alpha.txt").unwrap(), "v=1\n");
        assert_eq!(get_status("pages"), Status::Generated);
        assert_eq!(get_outputs().len(), 1);

        let inputs: Vec<Vec<(String, String)>> = ["b", "c", "d"]
            .iter()
            .map(|name| {
                vec![
                    ("name".to_owned(), (*name).to_owned()),
                    ("value".to_owned(), (*name).to_owned()),
                ]
            })
            .collect();
        let batch = run_all("pages", &inputs).expect("a batch");
        assert_eq!(batch.written, 3);
        assert_eq!(batch.skipped, 0);
        assert_eq!(batch.bytes, 4 * 3);
        assert_eq!(batch.outputs.len(), 3);
        assert_eq!(batch.outputs[2].path, "gen/pages/d.txt");
        assert_eq!(get_outputs().len(), 4);

        assert_eq!(
            run("nothing", &[]),
            Err(GeneratorError::NoSuchGenerator("nothing".to_owned()))
        );
        assert_eq!(get_status("nothing"), Status::Idle);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_output_outside_the_generators_root_is_refused() {
        let _serial = serial();
        fresh();
        let root = rooted("escape");
        register_template(page());
        register(generator().target("../escape.txt"));
        assert!(matches!(
            run("pages", &[("name".to_owned(), "x".to_owned()), ("value".to_owned(), "1".to_owned())]),
            Err(GeneratorError::OutsideRoot(_))
        ));
        register(generator().root("gen/other").target("/absolute.txt"));
        assert!(matches!(
            run("pages", &[("value".to_owned(), "1".to_owned())]),
            Err(GeneratorError::OutsideRoot(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn incremental_skips_a_match_and_rewrites_a_change() {
        let _serial = serial();
        fresh();
        let root = rooted("incremental");
        register_template(page());
        register(generator());
        let values = |value: &str| {
            vec![("name".to_owned(), "one".to_owned()), ("value".to_owned(), value.to_owned())]
        };

        let first = run("pages", &values("1")).expect("a run");
        assert!(!first.skipped);
        let first_hash = first.hash;
        assert_eq!(first_hash, crate::fs::hash_bytes(b"v=1\n"));

        let same = run("pages", &values("1")).expect("a run");
        assert!(same.skipped, "the same bytes are already on disk");
        assert_eq!(same.hash, first_hash);
        assert_eq!(get_status("pages"), Status::Skipped);

        let changed = run("pages", &values("2")).expect("a run");
        assert!(!changed.skipped, "different bytes are written");
        assert_ne!(changed.hash, first_hash);
        assert_eq!(changed.hash, crate::fs::hash_bytes(b"v=2\n"));
        assert_eq!(crate::fs::read_text("gen/pages/one.txt").unwrap(), "v=2\n");

        assert_eq!(invalidate("pages"), 3, "the outputs under its root");
        let forced = run("pages", &values("2")).expect("a run");
        assert!(!forced.skipped, "invalidated, so it is written again");
        assert_eq!(forced.hash, changed.hash);
        assert_eq!(invalidate("nothing"), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_manifest_records_a_total_and_verify_names_a_changed_file() {
        let _serial = serial();
        fresh();
        let root = rooted("manifest");
        register_template(page());
        register(generator());
        run_all(
            "pages",
            &[
                vec![("name".to_owned(), "a".to_owned()), ("value".to_owned(), "aa".to_owned())],
                vec![("name".to_owned(), "b".to_owned()), ("value".to_owned(), "bbb".to_owned())],
            ],
        )
        .expect("a batch");

        let manifest = generate_manifest("pages").expect("a manifest");
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(manifest.total, 5 + 6);
        assert!(manifest.verify().is_empty(), "everything matches as written");

        let read_back = Manifest::read("gen/pages/MANIFEST.json").expect("read back");
        assert_eq!(read_back, manifest, "what was written reads back the same");

        let changed = &manifest.entries[0].path.clone();
        crate::fs::write_text(changed, "not the bytes it recorded").unwrap();
        assert_eq!(manifest.verify(), vec![changed.clone()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dry_run_reports_without_writing() {
        let _serial = serial();
        fresh();
        let root = rooted("dry");
        register_template(page());
        register(generator());
        let values = vec![("name".to_owned(), "z".to_owned()), ("value".to_owned(), "hi".to_owned())];

        let planned = dry_run("pages", std::slice::from_ref(&values)).expect("a dry run");
        assert!(planned.dry_run);
        assert_eq!(planned.written, 1);
        assert_eq!(planned.bytes, 5);
        assert_eq!(planned.outputs[0].path, "gen/pages/z.txt");
        assert!(!crate::fs::exists("gen/pages/z.txt"), "a dry run writes nothing");
        assert_eq!(get_status("pages"), Status::Idle);
        assert!(get_outputs().is_empty());

        run("pages", &values).expect("a run");
        assert!(crate::fs::exists("gen/pages/z.txt"));
        let again = dry_run("pages", std::slice::from_ref(&values)).expect("a dry run");
        assert_eq!(again.written, 0);
        assert_eq!(again.skipped, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn writing_into_the_clients_assets_is_refused() {
        let _serial = serial();
        fresh();
        let root = rooted("refused");
        register_template(page());
        register(
            Generator::new("client_assets", OutputKind::Assets)
                .root("client/assets")
                .target("{{name}}.txt")
                .template("page"),
        );
        let refused = run("client_assets", &[("name".to_owned(), "x".to_owned()), ("value".to_owned(), "1".to_owned())]);
        assert_eq!(refused, Err(GeneratorError::Refused(why_unknown("client_assets"))));
        assert!(!crate::fs::exists("client/assets/x.txt"));
        assert_eq!(get_status("client_assets"), Status::Failed);
        assert!(get_errors().iter().any(|error| error.contains("client's own assets")));

        assert_eq!(refusals().len(), 2);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can generate"));
        }
        assert!(why_unknown("resource_pack").contains("resource pack"));
        assert!(why_unknown("nonsense").contains("nonsense"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
