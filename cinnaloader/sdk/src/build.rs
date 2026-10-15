//! Building a mod: the description of a build this host can produce, and the compiling it
//! will not do.
//!
//! A build is a [`BuildConfig`] — a [`Profile`] (debug or release, with the real flags a
//! compiler reads: opt level, debug info, lto, panic strategy) and a [`Target`] (a triple,
//! its features, and the loader ABI it is built for). From those [`plan`] derives the
//! [`Invocation`]s a build would run: the program, its arguments, the working directory and
//! the environment, as data a caller prints. The rule is fixed — program is the registered
//! compiler or `cargo`; the profile contributes `--profile <cargo's name for the mode>` and
//! the `RUSTFLAGS` environment; the target contributes `--target <triple>`, `--features` and
//! `CINNALOADER_ABI` — so a known profile gives a known argument list.
//!
//! Everything after the plan is real and needs no compiler: [`artifacts`] turns
//! caller-supplied bytes into an [`Artifact`] list with sizes and hashes and a total, a
//! [`BuildLog`] totals caller-supplied step durations and names the slowest, and
//! [`update_inputs`] decides from input hashes which registered steps rerun and which are
//! reused. [`report`] gathers the whole description through [`crate::serialization`].
//!
//! What this host will not do is compile. It ships no compiler, so [`compile`] and
//! [`rebuild`] refuse with [`why_unknown`], building the **client** is refused for the same
//! reason as any other game code, and a caller runs the plan itself.
//!
//! ```
//! use cinnaloader::build::{self, BuildConfig, Profile, Target};
//!
//! let profile = Profile::release();
//! assert_eq!(profile.opt_level, 3);
//! assert!(profile.flags().contains(&"lto".to_owned()));
//!
//! let target = Target::new("x86_64-unknown-linux-gnu");
//! assert!(target.is_compatible(), "built for this host's ABI");
//!
//! let plan = build::plan(&BuildConfig::new(profile, target));
//! assert_eq!(plan.invocations[0].program, "cargo");
//! assert_eq!(plan.invocations[0].args[0], "build");
//!
//! let artifacts = build::artifacts(&[("mod.so".to_owned(), vec![1, 2, 3])]);
//! assert_eq!(artifacts[0].size, 3);
//! assert_eq!(build::total_size(&artifacts), 3);
//!
//! assert!(build::compile().is_err(), "this host has no compiler");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::serialization::{self, Map, Value};

/// Which of the two builds a profile describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Debug,
    Release,
}

impl Mode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }

    /// What cargo calls this mode on the command line: the debug profile is `dev`.
    #[must_use]
    pub fn cargo_name(self) -> &'static str {
        match self {
            Self::Debug => "dev",
            Self::Release => "release",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a panic does inside the built mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicStrategy {
    Unwind,
    Abort,
}

impl PanicStrategy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unwind => "unwind",
            Self::Abort => "abort",
        }
    }
}

impl fmt::Display for PanicStrategy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A named build profile with the flags a compiler reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub mode: Mode,
    pub opt_level: u8,
    pub debug_info: u8,
    pub lto: bool,
    pub panic: PanicStrategy,
}

impl Profile {
    /// A profile named after its mode, with that mode's usual flags.
    #[must_use]
    pub fn new(name: impl Into<String>, mode: Mode) -> Self {
        Self {
            name: name.into(),
            mode,
            opt_level: match mode {
                Mode::Debug => 0,
                Mode::Release => 3,
            },
            debug_info: match mode {
                Mode::Debug => 2,
                Mode::Release => 0,
            },
            lto: matches!(mode, Mode::Release),
            panic: match mode {
                Mode::Debug => PanicStrategy::Unwind,
                Mode::Release => PanicStrategy::Abort,
            },
        }
    }

    #[must_use]
    pub fn debug() -> Self {
        Self::new("debug", Mode::Debug)
    }

    #[must_use]
    pub fn release() -> Self {
        Self::new("release", Mode::Release)
    }

    #[must_use]
    pub fn opt_level(mut self, level: u8) -> Self {
        self.opt_level = level.min(3);
        self
    }

    #[must_use]
    pub fn debug_info(mut self, level: u8) -> Self {
        self.debug_info = level.min(2);
        self
    }

    #[must_use]
    pub fn lto(mut self, on: bool) -> Self {
        self.lto = on;
        self
    }

    #[must_use]
    pub fn panic(mut self, strategy: PanicStrategy) -> Self {
        self.panic = strategy;
        self
    }

    /// The rustc flags this profile stands for, in order.
    #[must_use]
    pub fn flags(&self) -> Vec<String> {
        let mut flags = vec![
            "-C".to_owned(),
            format!("opt-level={}", self.opt_level),
            "-C".to_owned(),
            format!("debuginfo={}", self.debug_info),
            "-C".to_owned(),
            format!("panic={}", self.panic),
        ];
        if self.lto {
            flags.push("-C".to_owned());
            flags.push("lto".to_owned());
        }
        flags
    }
}

/// The platform and features a build targets, and the loader ABI it is built for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub triple: String,
    pub features: Vec<String>,
    pub abi: u32,
}

impl Target {
    /// A target for this host's loader ABI, with no extra features.
    #[must_use]
    pub fn new(triple: impl Into<String>) -> Self {
        Self {
            triple: triple.into(),
            features: Vec::new(),
            abi: crate::ABI_VERSION,
        }
    }

    #[must_use]
    pub fn feature(mut self, feature: impl Into<String>) -> Self {
        self.features.push(feature.into());
        self
    }

    #[must_use]
    pub fn abi(mut self, abi: u32) -> Self {
        self.abi = abi;
        self
    }

    /// This target's ABI against the one this host runs.
    #[must_use]
    pub fn compatibility(&self) -> Compatibility {
        if self.abi == crate::ABI_VERSION {
            Compatibility::Compatible { abi: self.abi }
        } else {
            Compatibility::Mismatch {
                built_for: self.abi,
                host: crate::ABI_VERSION,
            }
        }
    }

    #[must_use]
    pub fn is_compatible(&self) -> bool {
        matches!(self.compatibility(), Compatibility::Compatible { .. })
    }
}

/// Whether a target's ABI is this build's, naming both numbers when it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compatibility {
    Compatible { abi: u32 },
    Mismatch { built_for: u32, host: u32 },
}

impl Compatibility {
    #[must_use]
    pub fn is_compatible(self) -> bool {
        matches!(self, Self::Compatible { .. })
    }

    /// A sentence that names both versions whichever way it went.
    #[must_use]
    pub fn sentence(self) -> String {
        match self {
            Self::Compatible { abi } => {
                format!("built for loader ABI {abi}, which is this host's ABI")
            }
            Self::Mismatch { built_for, host } => {
                format!("built for loader ABI {built_for}, and this host is loader ABI {host}")
            }
        }
    }
}

/// Everything a build needs besides its steps: a profile, a target and where output goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildConfig {
    pub profile: Profile,
    pub target: Target,
    pub output: String,
    pub cache: String,
}

impl BuildConfig {
    #[must_use]
    pub fn new(profile: Profile, target: Target) -> Self {
        Self {
            profile,
            target,
            output: "target/mod".to_owned(),
            cache: "target/cache".to_owned(),
        }
    }

    #[must_use]
    pub fn output(mut self, dir: impl Into<String>) -> Self {
        self.output = dir.into();
        self
    }

    #[must_use]
    pub fn cache(mut self, dir: impl Into<String>) -> Self {
        self.cache = dir.into();
        self
    }
}

/// One command a build would run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
}

impl Invocation {
    /// The command line, program then arguments, as a caller prints it.
    #[must_use]
    pub fn line(&self) -> String {
        if self.args.is_empty() {
            return self.program.clone();
        }
        format!("{} {}", self.program, self.args.join(" "))
    }
}

/// The command lines a build would need, in the order it would run them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub invocations: Vec<Invocation>,
}

impl BuildPlan {
    #[must_use]
    pub fn first(&self) -> Option<&Invocation> {
        self.invocations.first()
    }
}

/// One built file: its name, the path it was written from, its size and its byte hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub hash: u64,
}

/// How a finished step went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Failed,
    Skipped,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// One step of a build, as it was recorded: a name, a caller-supplied duration and how it
/// went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRun {
    pub name: String,
    pub duration_ms: u64,
    pub outcome: Outcome,
    pub message: String,
}

/// A record of a build's steps, with a real total and its slowest step.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildLog {
    pub steps: Vec<StepRun>,
}

impl BuildLog {
    #[must_use]
    pub fn total_ms(&self) -> u64 {
        self.steps.iter().map(|step| step.duration_ms).sum()
    }

    #[must_use]
    pub fn slowest(&self) -> Option<&StepRun> {
        self.steps.iter().max_by_key(|step| step.duration_ms)
    }

    #[must_use]
    pub fn failed(&self) -> Vec<&StepRun> {
        self.steps
            .iter()
            .filter(|step| step.outcome == Outcome::Failed)
            .collect()
    }
}

/// Which registered steps rerun for the input hashes seen, and which are reused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IncrementalStatus {
    pub rerun: Vec<String>,
    pub reused: Vec<String>,
}

/// How far a build got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Configured,
    Packaged,
    Failed,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Configured => "configured",
            Self::Packaged => "packaged",
            Self::Failed => "failed",
        }
    }
}

/// One out of however many steps has a recorded result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// Why a build could not run here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// No compiler is installed in this host.
    NoCompiler(String),
    /// The client's own code is not built here.
    Client(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCompiler(why) | Self::Client(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for BuildError {}

#[derive(Debug)]
struct Step {
    name: String,
    inputs: Vec<String>,
}

type Rule = Box<dyn Fn() -> Vec<String> + Send>;

struct Validator {
    name: String,
    run: Rule,
}

struct Generator {
    name: String,
    run: Rule,
}

struct Build {
    config: BuildConfig,
    target_version: String,
    profiles: Vec<Profile>,
    steps: Vec<Step>,
    packaging: Vec<String>,
    validators: Vec<Validator>,
    generators: Vec<Generator>,
    compilers: Vec<String>,
    artifacts: Vec<Artifact>,
    log: Option<BuildLog>,
    errors: Vec<String>,
    warnings: Vec<String>,
    inputs: BTreeMap<String, u64>,
    incremental: IncrementalStatus,
    status: Status,
}

impl Default for Build {
    fn default() -> Self {
        Self {
            config: BuildConfig::new(Profile::debug(), Target::new(host_triple())),
            target_version: env!("CARGO_PKG_VERSION").to_owned(),
            profiles: Vec::new(),
            steps: Vec::new(),
            packaging: Vec::new(),
            validators: Vec::new(),
            generators: Vec::new(),
            compilers: Vec::new(),
            artifacts: Vec::new(),
            log: None,
            errors: Vec::new(),
            warnings: Vec::new(),
            inputs: BTreeMap::new(),
            incremental: IncrementalStatus::default(),
            status: Status::Idle,
        }
    }
}

fn host_triple() -> String {
    format!(
        "{}-unknown-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    )
}

fn build() -> &'static Mutex<Build> {
    static BUILD: OnceLock<Mutex<Build>> = OnceLock::new();
    BUILD.get_or_init(|| Mutex::new(Build::default()))
}

fn with<T>(body: impl FnOnce(&mut Build) -> T) -> T {
    let mut guard = build().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a profile, refusing a name already taken so a duplicate is not a silent second
/// meaning for it.
pub fn register_profile(profile: Profile) -> bool {
    with(|build| {
        if build
            .profiles
            .iter()
            .any(|held| held.name == profile.name)
        {
            crate::warn!(
                "a build profile named {} is already registered",
                profile.name
            );
            return false;
        }
        build.profiles.push(profile);
        true
    })
}

/// The profiles registered, in registration order.
#[must_use]
pub fn get_profiles() -> Vec<Profile> {
    with(|build| build.profiles.clone())
}

/// Sets up a default build: the debug profile against this host, where output goes.
pub fn configure() -> bool {
    with(|build| {
        build.config = BuildConfig::new(Profile::debug(), Target::new(host_triple()));
        build.status = Status::Configured;
        build.errors.clear();
        build.warnings.clear();
        true
    })
}

/// The command lines a build would run, from the config's profile and target.
#[must_use]
pub fn plan(config: &BuildConfig) -> BuildPlan {
    let cargo_name = config.profile.mode.cargo_name().to_owned();
    let env = vec![
        ("CARGO_TARGET_DIR".to_owned(), config.cache.clone()),
        (
            "RUSTFLAGS".to_owned(),
            config.profile.flags().join(" "),
        ),
        ("CINNALOADER_ABI".to_owned(), config.target.abi.to_string()),
    ];
    let program = with(|build| build.compilers.first().cloned()).unwrap_or_else(|| "cargo".to_owned());

    let mut args = vec![
        "build".to_owned(),
        "--profile".to_owned(),
        cargo_name.clone(),
        "--target".to_owned(),
        config.target.triple.clone(),
    ];
    if !config.target.features.is_empty() {
        args.push("--features".to_owned());
        args.push(config.target.features.join(","));
    }
    let mut invocations = vec![Invocation {
        program: program.clone(),
        args,
        cwd: config.output.clone(),
        env: env.clone(),
    }];

    let packaging = with(|build| build.packaging.clone());
    for step in packaging {
        invocations.push(Invocation {
            program: program.clone(),
            args: vec![
                "package".to_owned(),
                "--profile".to_owned(),
                cargo_name.clone(),
                "--target".to_owned(),
                config.target.triple.clone(),
                "--out".to_owned(),
                config.output.clone(),
                "--name".to_owned(),
                step,
            ],
            cwd: config.output.clone(),
            env: env.clone(),
        });
    }
    BuildPlan { invocations }
}

/// Turns caller-supplied files into artifacts with real sizes and hashes.
#[must_use]
pub fn artifacts(files: &[(String, Vec<u8>)]) -> Vec<Artifact> {
    files
        .iter()
        .map(|(path, bytes)| Artifact {
            name: basename(path),
            path: path.clone(),
            size: bytes.len() as u64,
            hash: serialization::hash(bytes),
        })
        .collect()
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

/// The sum of every artifact's size.
#[must_use]
pub fn total_size(artifacts: &[Artifact]) -> u64 {
    artifacts.iter().map(|artifact| artifact.size).sum()
}

/// Records a build log, which is what [`get_progress`] counts against.
pub fn record_log(log: BuildLog) {
    with(|build| build.log = Some(log));
}

/// Builds a log from the step runs a caller recorded.
#[must_use]
pub fn build_log(runs: Vec<StepRun>) -> BuildLog {
    BuildLog { steps: runs }
}

/// Turns caller-supplied files into the held artifact list and marks the build packaged.
pub fn package(files: &[(String, Vec<u8>)]) -> Vec<Artifact> {
    let held = artifacts(files);
    with(|build| {
        build.artifacts = held.clone();
        build.status = Status::Packaged;
    });
    held
}

/// Disposes of the artifacts and any recorded run, and answers how many artifacts went.
pub fn clean() -> usize {
    with(|build| {
        let removed = build.artifacts.len();
        build.artifacts.clear();
        build.log = None;
        build.errors.clear();
        build.warnings.clear();
        build.status = Status::Configured;
        removed
    })
}

/// Clears the remembered input hashes, so the next comparison sees everything as new, and
/// answers how many inputs went.
pub fn clean_cache() -> usize {
    with(|build| {
        let removed = build.inputs.len();
        build.inputs.clear();
        build.incremental = IncrementalStatus::default();
        removed
    })
}

/// Compiling. This host has no compiler, so it refuses and says to run [`plan`] yourself.
pub fn compile() -> Result<(), BuildError> {
    let why = why_unknown("compiler");
    crate::warn!("a mod asked this host to compile: {why}");
    with(|build| {
        build.errors.push(why.clone());
        build.status = Status::Failed;
    });
    Err(BuildError::NoCompiler(why))
}

/// A clean then a compile, which is still a refusal: the compile is the part that cannot run.
pub fn rebuild() -> Result<(), BuildError> {
    clean();
    compile()
}

/// Building the client's own code, which no mod and not this host does.
pub fn build_client() -> Result<(), BuildError> {
    let why = why_unknown("client");
    crate::warn!("a mod asked this host to build the client: {why}");
    Err(BuildError::Client(why))
}

/// Runs the registered validators and answers what they found.
pub fn validate() -> Vec<String> {
    let problems: Vec<String> = with(|build| {
        build
            .validators
            .iter()
            .flat_map(|validator| (validator.run)())
            .collect()
    });
    with(|build| {
        build.errors = problems.clone();
        if !problems.is_empty() {
            build.status = Status::Failed;
        }
    });
    problems
}

/// Runs the registered generators and answers everything they would write.
#[must_use]
pub fn run_generators() -> Vec<String> {
    with(|build| {
        build
            .generators
            .iter()
            .flat_map(|generator| (generator.run)())
            .collect()
    })
}

/// What the build is doing.
#[must_use]
pub fn get_status() -> Status {
    with(|build| build.status)
}

/// How many registered steps have a recorded result, against how many there are.
#[must_use]
pub fn get_progress() -> Progress {
    with(|build| Progress {
        done: build.log.as_ref().map_or(0, |log| log.steps.len()),
        total: build.steps.len() + build.packaging.len(),
    })
}

#[must_use]
pub fn get_errors() -> Vec<String> {
    with(|build| build.errors.clone())
}

#[must_use]
pub fn get_warnings() -> Vec<String> {
    with(|build| build.warnings.clone())
}

/// The artifacts a [`package`] left, or nothing before one ran.
#[must_use]
pub fn get_artifacts() -> Vec<Artifact> {
    with(|build| build.artifacts.clone())
}

/// The ids this mod says it needs, as declared.
#[must_use]
pub fn get_dependencies() -> Vec<String> {
    crate::metadata::manifest()
        .dependencies
        .iter()
        .map(|dependency| dependency.id.clone())
        .collect()
}

/// The dependencies, sorted and with a repeat named once.
#[must_use]
pub fn resolve_dependencies() -> Vec<String> {
    let mut resolved = get_dependencies();
    resolved.sort();
    resolved.dedup();
    resolved
}

#[must_use]
pub fn get_build_config() -> BuildConfig {
    with(|build| build.config.clone())
}

/// Replaces the build config.
pub fn set_build_config(config: BuildConfig) -> bool {
    with(|build| {
        build.config = config;
        build.status = Status::Configured;
        true
    })
}

#[must_use]
pub fn get_target_platform() -> String {
    with(|build| build.config.target.triple.clone())
}

/// Sets the target triple, refusing an empty one.
pub fn set_target_platform(triple: &str) -> bool {
    if triple.trim().is_empty() {
        return false;
    }
    with(|build| {
        build.config.target.triple = triple.to_owned();
        true
    })
}

/// The client version the build aims at.
#[must_use]
pub fn get_target_version() -> String {
    with(|build| build.target_version.clone())
}

/// Sets the target version, refusing an empty one.
pub fn set_target_version(version: &str) -> bool {
    if version.trim().is_empty() {
        return false;
    }
    with(|build| {
        build.target_version = version.to_owned();
        true
    })
}

#[must_use]
pub fn get_output_directory() -> String {
    with(|build| build.config.output.clone())
}

/// Sets where output goes, refusing an empty path.
pub fn set_output_directory(dir: &str) -> bool {
    if dir.trim().is_empty() {
        return false;
    }
    with(|build| {
        build.config.output = dir.to_owned();
        true
    })
}

#[must_use]
pub fn get_cache_directory() -> String {
    with(|build| build.config.cache.clone())
}

/// Registers a named build step and the inputs whose change forces it to rerun. Refuses an
/// empty or already-taken name.
pub fn register_build_step(name: &str, inputs: &[&str]) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    with(|build| {
        if build.steps.iter().any(|step| step.name == name) {
            crate::warn!("a build step named {name} is already registered");
            return false;
        }
        build.steps.push(Step {
            name: name.to_owned(),
            inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
        });
        true
    })
}

/// Registers a packaging step by name, which [`plan`] then gives a command line.
pub fn register_packaging_step(name: &str) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    with(|build| {
        if build.packaging.iter().any(|held| held == name) {
            crate::warn!("a packaging step named {name} is already registered");
            return false;
        }
        build.packaging.push(name.to_owned());
        true
    })
}

/// Registers a validator that answers the problems it can see.
pub fn register_validator(name: &str, run: impl Fn() -> Vec<String> + Send + 'static) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    with(|build| {
        if build.validators.iter().any(|held| held.name == name) {
            crate::warn!("a validator named {name} is already registered");
            return false;
        }
        build.validators.push(Validator {
            name: name.to_owned(),
            run: Box::new(run),
        });
        true
    })
}

/// Registers a generator that answers the files it would write.
pub fn register_generator(name: &str, run: impl Fn() -> Vec<String> + Send + 'static) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    with(|build| {
        if build.generators.iter().any(|held| held.name == name) {
            crate::warn!("a generator named {name} is already registered");
            return false;
        }
        build.generators.push(Generator {
            name: name.to_owned(),
            run: Box::new(run),
        });
        true
    })
}

/// Registers a compiler program [`plan`] would call, refusing an empty or repeated one.
pub fn register_compiler(program: &str) -> bool {
    if program.trim().is_empty() {
        return false;
    }
    with(|build| {
        if build.compilers.iter().any(|held| held == program) {
            crate::warn!("a compiler named {program} is already registered");
            return false;
        }
        build.compilers.push(program.to_owned());
        true
    })
}

fn changed_paths(
    previous: &BTreeMap<String, u64>,
    current: &BTreeMap<String, u64>,
) -> Vec<String> {
    let mut changed = Vec::new();
    for (path, hash) in current {
        if previous.get(path) != Some(hash) {
            changed.push(path.clone());
        }
    }
    for path in previous.keys() {
        if !current.contains_key(path) {
            changed.push(path.clone());
        }
    }
    changed
}

/// Records the input hashes now seen, decides which registered steps rerun against the ones
/// held from last time, and stores the answer.
pub fn update_inputs(inputs: &[(String, u64)]) -> IncrementalStatus {
    with(|build| {
        let current: BTreeMap<String, u64> = inputs.iter().cloned().collect();
        let changed = changed_paths(&build.inputs, &current);
        let mut status = IncrementalStatus::default();
        for step in &build.steps {
            if step
                .inputs
                .iter()
                .any(|input| changed.contains(input))
            {
                status.rerun.push(step.name.clone());
            } else {
                status.reused.push(step.name.clone());
            }
        }
        build.inputs = current;
        build.incremental = status.clone();
        status
    })
}

/// The decision the last [`update_inputs`] made.
#[must_use]
pub fn get_incremental_status() -> IncrementalStatus {
    with(|build| build.incremental.clone())
}

/// The facts a reproduce-the-build check compares: the flags, the target and the hash.
#[must_use]
pub fn get_reproducibility_info() -> Vec<(String, String)> {
    let config = get_build_config();
    let target_version = with(|build| build.target_version.clone());
    let mut rows = vec![
        ("profile".to_owned(), config.profile.name.clone()),
        ("mode".to_owned(), config.profile.mode.as_str().to_owned()),
        (
            "opt_level".to_owned(),
            config.profile.opt_level.to_string(),
        ),
        (
            "debug_info".to_owned(),
            config.profile.debug_info.to_string(),
        ),
        ("lto".to_owned(), config.profile.lto.to_string()),
        ("panic".to_owned(), config.profile.panic.to_string()),
        ("target".to_owned(), config.target.triple.clone()),
        ("features".to_owned(), config.target.features.join(",")),
        ("abi".to_owned(), config.target.abi.to_string()),
        ("target_version".to_owned(), target_version),
        (
            "compatibility".to_owned(),
            config.target.compatibility().sentence(),
        ),
    ];
    rows.push(("build_hash".to_owned(), get_build_hash().to_string()));
    rows
}

fn config_value(config: &BuildConfig, target_version: &str) -> Value {
    let mut profile = Map::new();
    profile.insert("name", Value::str(&config.profile.name));
    profile.insert("mode", Value::str(config.profile.mode.as_str()));
    profile.insert("opt_level", Value::Int(i64::from(config.profile.opt_level)));
    profile.insert("debug_info", Value::Int(i64::from(config.profile.debug_info)));
    profile.insert("lto", Value::Bool(config.profile.lto));
    profile.insert("panic", Value::str(config.profile.panic.as_str()));

    let mut target = Map::new();
    target.insert("triple", Value::str(&config.target.triple));
    target.insert(
        "features",
        Value::List(
            config
                .target
                .features
                .iter()
                .map(Value::str)
                .collect(),
        ),
    );
    target.insert("abi", Value::Int(i64::from(config.target.abi)));
    target.insert(
        "compatibility",
        Value::str(config.target.compatibility().sentence()),
    );

    let mut map = Map::new();
    map.insert("profile", Value::Map(profile));
    map.insert("target", Value::Map(target));
    map.insert("target_version", Value::str(target_version));
    map.insert("output", Value::str(&config.output));
    map.insert("cache", Value::str(&config.cache));
    Value::Map(map)
}

/// The whole build description as a value tree, with the real totals in it.
#[must_use]
pub fn report_value() -> Value {
    let config = get_build_config();
    let target_version = with(|build| build.target_version.clone());
    let artifacts = get_artifacts();
    let log = with(|build| build.log.clone());
    let incremental = get_incremental_status();
    let status = get_status();
    let steps = with(|build| {
        build
            .steps
            .iter()
            .map(|step| step.name.clone())
            .chain(build.packaging.iter().cloned())
            .collect::<Vec<_>>()
    });

    let mut map = Map::new();
    map.insert("status", Value::str(status.as_str()));
    map.insert("config", config_value(&config, &target_version));
    map.insert(
        "steps",
        Value::List(steps.into_iter().map(Value::str).collect()),
    );
    map.insert(
        "artifacts",
        Value::List(
            artifacts
                .iter()
                .map(|artifact| {
                    let mut entry = Map::new();
                    entry.insert("name", Value::str(&artifact.name));
                    entry.insert("path", Value::str(&artifact.path));
                    entry.insert("size", Value::Int(artifact.size as i64));
                    entry.insert("hash", Value::Int(artifact.hash as i64));
                    Value::Map(entry)
                })
                .collect(),
        ),
    );
    map.insert(
        "artifact_count",
        Value::Int(artifacts.len() as i64),
    );
    map.insert(
        "artifact_bytes",
        Value::Int(total_size(&artifacts) as i64),
    );

    let mut log_map = Map::new();
    match &log {
        Some(log) => {
            log_map.insert("total_ms", Value::Int(log.total_ms() as i64));
            log_map.insert(
                "slowest",
                Value::str(log.slowest().map(|step| step.name.as_str()).unwrap_or("")),
            );
            log_map.insert(
                "steps",
                Value::List(
                    log.steps
                        .iter()
                        .map(|step| {
                            let mut entry = Map::new();
                            entry.insert("name", Value::str(&step.name));
                            entry.insert("duration_ms", Value::Int(step.duration_ms as i64));
                            entry.insert("outcome", Value::str(step.outcome.as_str()));
                            entry.insert("message", Value::str(&step.message));
                            Value::Map(entry)
                        })
                        .collect(),
                ),
            );
        }
        None => {
            log_map.insert("total_ms", Value::Int(0));
            log_map.insert("slowest", Value::str(""));
            log_map.insert("steps", Value::List(Vec::new()));
        }
    }
    map.insert("log", Value::Map(log_map));

    let mut incremental_map = Map::new();
    incremental_map.insert(
        "rerun",
        Value::List(incremental.rerun.into_iter().map(Value::str).collect()),
    );
    incremental_map.insert(
        "reused",
        Value::List(incremental.reused.into_iter().map(Value::str).collect()),
    );
    map.insert("incremental", Value::Map(incremental_map));

    let hash = serialization::hash(serialization::to_text(&Value::Map(map.clone())).as_bytes());
    map.insert("build_hash", Value::Int(hash as i64));
    Value::Map(map)
}

/// A hash of the whole description without the hash itself, so it is stable to compare.
#[must_use]
pub fn get_build_hash() -> u64 {
    report_value()
        .as_map()
        .and_then(|map| map.get("build_hash"))
        .and_then(Value::as_int)
        .unwrap_or(0) as u64
}

/// The whole build description in this crate's text form.
#[must_use]
pub fn report() -> String {
    serialization::to_text(&report_value())
}

/// The whole build description as JSON.
#[must_use]
pub fn report_json() -> String {
    serialization::to_json(&report_value()).unwrap_or_default()
}

/// Why a build question has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "compiler" => {
            "this host ships no compiler: a mod's own source is compiled on the developer's \
             machine, so build::compile() answers the plan and the caller runs it"
                .to_owned()
        }
        "client" => {
            "building the client means rebuilding the game's own code, which only the client's \
             own toolchain does"
                .to_owned()
        }
        _ => format!("{what} is not something this host can build"),
    }
}

/// What this host cannot build.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["compiler", "client"]
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
        with(|build| *build = Build::default());
    }

    #[test]
    fn a_profile_carries_real_flags_and_a_duplicate_is_refused() {
        let _serial = serial();
        fresh();

        let debug = Profile::debug();
        assert_eq!(debug.mode, Mode::Debug);
        assert_eq!(debug.opt_level, 0);
        assert_eq!(debug.debug_info, 2);
        assert!(!debug.lto);
        assert_eq!(debug.panic, PanicStrategy::Unwind);
        assert_eq!(
            debug.flags(),
            vec![
                "-C",
                "opt-level=0",
                "-C",
                "debuginfo=2",
                "-C",
                "panic=unwind"
            ]
        );

        let release = Profile::release();
        assert_eq!(release.opt_level, 3);
        assert_eq!(
            release.flags(),
            vec![
                "-C",
                "opt-level=3",
                "-C",
                "debuginfo=0",
                "-C",
                "panic=abort",
                "-C",
                "lto"
            ]
        );

        assert!(register_profile(debug.clone()));
        assert!(!register_profile(debug), "the same name is refused");
        assert_eq!(get_profiles().len(), 1);
    }

    #[test]
    fn a_target_answers_compatibility_naming_both_versions() {
        let _serial = serial();
        fresh();

        let host = Target::new("x86_64-unknown-linux-gnu");
        assert_eq!(
            host.compatibility(),
            Compatibility::Compatible {
                abi: crate::ABI_VERSION
            }
        );
        assert!(host.is_compatible());
        assert_eq!(
            host.compatibility().sentence(),
            format!("built for loader ABI {}, which is this host's ABI", crate::ABI_VERSION)
        );

        let other = Target::new("aarch64-apple-darwin").abi(crate::ABI_VERSION + 1);
        assert_eq!(
            other.compatibility(),
            Compatibility::Mismatch {
                built_for: crate::ABI_VERSION + 1,
                host: crate::ABI_VERSION
            }
        );
        assert!(!other.is_compatible());
        let sentence = other.compatibility().sentence();
        assert!(sentence.contains(&(crate::ABI_VERSION + 1).to_string()));
        assert!(sentence.contains(&crate::ABI_VERSION.to_string()));
    }

    #[test]
    fn the_plan_lists_the_exact_arguments_for_a_known_profile() {
        let _serial = serial();
        fresh();

        let target = Target::new("x86_64-unknown-linux-gnu")
            .feature("fast")
            .feature("simd");
        let config = BuildConfig::new(Profile::release(), target).cache("out/cache");
        let built = plan(&config);

        let first = built.first().expect("a build invocation");
        assert_eq!(first.program, "cargo");
        assert_eq!(
            first.args,
            vec![
                "build",
                "--profile",
                "release",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--features",
                "fast,simd",
            ]
        );
        assert_eq!(first.cwd, "target/mod");
        assert_eq!(
            first.env,
            vec![
                ("CARGO_TARGET_DIR".to_owned(), "out/cache".to_owned()),
                (
                    "RUSTFLAGS".to_owned(),
                    "-C opt-level=3 -C debuginfo=0 -C panic=abort -C lto".to_owned()
                ),
                (
                    "CINNALOADER_ABI".to_owned(),
                    crate::ABI_VERSION.to_string()
                ),
            ]
        );
        assert_eq!(
            first.line(),
            "cargo build --profile release --target x86_64-unknown-linux-gnu --features fast,simd"
        );

        let debug = BuildConfig::new(Profile::debug(), Target::new("x86_64-unknown-linux-gnu"));
        assert_eq!(plan(&debug).first().expect("a plan").args[2], "dev");

        assert!(register_packaging_step("zip"));
        let with_packaging = plan(&config);
        assert_eq!(with_packaging.invocations.len(), 2);
        assert_eq!(with_packaging.invocations[1].args[0], "package");
    }

    #[test]
    fn artifacts_carry_sizes_hashes_and_a_total() {
        let _serial = serial();
        fresh();

        let files = vec![
            ("build/mod.so".to_owned(), vec![1, 2, 3]),
            ("build/manifest.json".to_owned(), b"{}".to_vec()),
        ];
        let held = artifacts(&files);
        assert_eq!(held.len(), 2);
        assert_eq!(held[0].name, "mod.so");
        assert_eq!(held[0].size, 3);
        assert_eq!(held[0].hash, serialization::hash(&[1, 2, 3]));
        assert_eq!(held[1].name, "manifest.json");
        assert_eq!(held[1].size, 2);
        assert_eq!(total_size(&held), 5);

        let packed = package(&files);
        assert_eq!(packed, held);
        assert_eq!(get_artifacts(), held);
        assert_eq!(get_status(), Status::Packaged);
        assert_eq!(clean(), 2, "the artifacts that went");
        assert!(get_artifacts().is_empty());
    }

    #[test]
    fn a_log_totals_durations_and_names_the_slowest_step() {
        let _serial = serial();
        fresh();

        let log = build_log(vec![
            StepRun {
                name: "compile".to_owned(),
                duration_ms: 120,
                outcome: Outcome::Ok,
                message: "built".to_owned(),
            },
            StepRun {
                name: "link".to_owned(),
                duration_ms: 400,
                outcome: Outcome::Failed,
                message: "no linker".to_owned(),
            },
            StepRun {
                name: "sign".to_owned(),
                duration_ms: 30,
                outcome: Outcome::Skipped,
                message: "not reached".to_owned(),
            },
        ]);
        assert_eq!(log.total_ms(), 550);
        assert_eq!(log.slowest().map(|step| step.name.as_str()), Some("link"));
        assert_eq!(log.failed().len(), 1);
        assert_eq!(log.failed()[0].name, "link");

        record_log(log);
        assert_eq!(get_progress().done, 3);
    }

    #[test]
    fn incremental_decisions_rerun_exactly_the_changed_steps() {
        let _serial = serial();
        fresh();

        assert!(register_build_step("compile", &["src/lib.rs"]));
        assert!(register_build_step("assets", &["assets/icon.png"]));
        assert!(register_build_step("package", &[]));

        let first = update_inputs(&[
            ("src/lib.rs".to_owned(), 11),
            ("assets/icon.png".to_owned(), 22),
        ]);
        assert_eq!(first.rerun, vec!["compile", "assets"]);
        assert_eq!(first.reused, vec!["package"], "a step with no inputs");

        let again = update_inputs(&[
            ("src/lib.rs".to_owned(), 11),
            ("assets/icon.png".to_owned(), 22),
        ]);
        assert!(again.rerun.is_empty(), "nothing changed");
        assert_eq!(again.reused, vec!["compile", "assets", "package"]);

        let touched = update_inputs(&[
            ("src/lib.rs".to_owned(), 11),
            ("assets/icon.png".to_owned(), 99),
        ]);
        assert_eq!(touched.rerun, vec!["assets"]);
        assert_eq!(touched.reused, vec!["compile", "package"]);
        assert_eq!(get_incremental_status(), touched);

        assert_eq!(clean_cache(), 2, "the remembered inputs");
        let back = update_inputs(&[
            ("src/lib.rs".to_owned(), 11),
            ("assets/icon.png".to_owned(), 22),
        ]);
        assert_eq!(back.rerun, vec!["compile", "assets"], "nothing is remembered");
        assert_eq!(back.reused, vec!["package"]);
    }

    #[test]
    fn a_report_carries_the_description_and_its_totals() {
        let _serial = serial();
        fresh();
        configure();
        package(&[
            ("build/mod.so".to_owned(), vec![1, 2, 3, 4]),
            ("build/assets.bin".to_owned(), vec![5, 6]),
        ]);
        record_log(build_log(vec![
            StepRun {
                name: "compile".to_owned(),
                duration_ms: 90,
                outcome: Outcome::Ok,
                message: "built".to_owned(),
            },
            StepRun {
                name: "package".to_owned(),
                duration_ms: 15,
                outcome: Outcome::Ok,
                message: "packed".to_owned(),
            },
        ]));

        let value = report_value();
        let map = value.as_map().expect("a map");
        assert_eq!(
            map.get("artifact_bytes").and_then(Value::as_int),
            Some(6)
        );
        assert_eq!(map.get("artifact_count").and_then(Value::as_int), Some(2));
        let log = map.get("log").and_then(Value::as_map).expect("a log");
        assert_eq!(log.get("total_ms").and_then(Value::as_int), Some(105));
        assert_eq!(
            log.get("slowest").and_then(Value::as_str),
            Some("compile")
        );

        let text = report();
        assert!(text.contains("\"build_hash\""));
        assert!(text.contains("artifact_bytes"));
        let json = report_json();
        assert!(json.starts_with('{'));
        assert!(json.contains("\"total_ms\":105"));

        let hash = get_build_hash();
        assert_ne!(hash, 0);
        assert_eq!(get_build_hash(), hash, "stable for the same build");
        let info = get_reproducibility_info();
        assert!(info
            .iter()
            .any(|(key, value)| key == "build_hash" && *value == hash.to_string()));
        assert!(info.iter().any(|(key, _)| key == "target"));
    }

    #[test]
    fn the_refused_entry_points_say_why() {
        let _serial = serial();
        fresh();

        let compiled = compile();
        assert!(matches!(compiled, Err(BuildError::NoCompiler(_))));
        assert_eq!(get_status(), Status::Failed);
        assert_eq!(get_errors().len(), 1);
        assert!(get_errors()[0].contains("no compiler"));

        assert!(matches!(rebuild(), Err(BuildError::NoCompiler(_))));
        assert!(matches!(build_client(), Err(BuildError::Client(_))));

        let refusals = refusals();
        assert_eq!(refusals.len(), 2);
        for (what, why) in refusals {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can build"));
        }
        assert!(why_unknown("nonsense").contains("nonsense"));

        assert!(register_compiler("rustc"));
        assert!(!register_compiler("rustc"));
        assert!(register_packaging_step("zip"));
        assert!(register_build_step("compile", &["src/lib.rs"]));
        assert!(!register_build_step("compile", &[]));
        assert!(register_validator("lint", || vec!["a warning".to_owned()]));
        assert_eq!(validate(), vec!["a warning".to_owned()]);
        assert!(register_generator("manifest", || vec!["manifest.json".to_owned()]));
        assert_eq!(run_generators(), vec!["manifest.json".to_owned()]);
    }
}
