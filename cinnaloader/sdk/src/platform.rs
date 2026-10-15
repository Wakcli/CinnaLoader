//! The machine a mod is running on, from facts this host can read itself.
//!
//! Nothing here needs the client. The operating system, architecture, family, a readable
//! platform id, the line and path separators, the endianness, whether the process is 64-bit,
//! the CPU count, and the directories a mod may use all come from `std` and
//! [`crate::diagnostics`]; the memory and runtime facts reuse what that module already
//! computes. One thing is honestly not answerable: the page size, which `std` does not
//! expose and this host will not pull a dependency in to ask the kernel for, so
//! [`page_size`] answers `None` with [`page_size_reason`].
//!
//! [`is_supported`] is the capability matrix, and its rule is one line: a platform capability
//! is supported only when this host can answer it from facts it reads itself. Everything that
//! would need the client's desktop session — the GPU driver, the clipboard, a browser, a file
//! manager, display and input devices, accessibility — answers `false` here, and each has its
//! own sentence in [`why_unknown`] and a row in [`refusals`].
//!
//! ```
//! use cinnaloader::platform;
//!
//! assert_eq!(platform::get_os(), std::env::consts::OS);
//! assert!(platform::cpu_count() >= 1);
//! assert_eq!(platform::is_64_bit(), std::mem::size_of::<usize>() == 8);
//! assert!(!platform::is_supported(platform::PlatformCapability::Gpu));
//! assert!(platform::page_size().is_none(), "{}", platform::page_size_reason());
//! ```

use std::path::PathBuf;

/// What the CPU is, as far as `std` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuInfo {
    pub architecture: String,
    pub family: String,
    pub cores: usize,
    pub is_64_bit: bool,
    pub page_size: Option<usize>,
}

/// What the GPU is, where a host could report one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GpuInfo {
    pub vendor: String,
    pub renderer: String,
    pub version: String,
}

/// What the process is using, in bytes where the kernel reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemoryInfo {
    pub resident_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
}

/// The display, where a host could report one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
}

/// The accessibility settings, where a host could report them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccessibilitySettings {
    pub reduce_motion: bool,
    pub high_contrast: bool,
    pub text_scale_percent: u32,
}

/// The directories a mod may use: its own, the user's home and the system temporary one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformPaths {
    pub app: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub temp: PathBuf,
    /// Whether this mod has a directory of its own that the loader sandboxes it to.
    pub writable: bool,
}

/// The operating system, as `std::env::consts::OS` spells it.
#[must_use]
pub fn get_os() -> String {
    std::env::consts::OS.to_owned()
}

/// The CPU architecture, as `std::env::consts::ARCH` spells it.
#[must_use]
pub fn get_architecture() -> String {
    std::env::consts::ARCH.to_owned()
}

/// The OS family: `unix` or `windows`.
#[must_use]
pub fn get_family() -> String {
    std::env::consts::FAMILY.to_owned()
}

/// A readable platform id, `os-arch`.
#[must_use]
pub fn platform_id() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!("{os}-{arch}")
}

/// The platform's line separator.
#[must_use]
pub fn line_separator() -> &'static str {
    if cfg!(windows) {
        "\r\n"
    } else {
        "\n"
    }
}

/// The platform's path separator, which is `std`'s own.
#[must_use]
pub fn path_separator() -> char {
    std::path::MAIN_SEPARATOR
}

/// Whether the target is little-endian.
#[must_use]
pub fn is_little_endian() -> bool {
    cfg!(target_endian = "little")
}

/// Whether the process's pointers are 64 bits wide.
#[must_use]
pub fn is_64_bit() -> bool {
    std::mem::size_of::<usize>() == 8
}

/// How many threads the process may run at once, at least one.
#[must_use]
pub fn cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

/// The kernel's page size. `None`, with [`page_size_reason`].
#[must_use]
pub fn page_size() -> Option<usize> {
    None
}

/// Why the page size is unreported.
#[must_use]
pub fn page_size_reason() -> &'static str {
    "std exposes no page size and this host adds no dependency to ask the kernel for one"
}

/// Everything about the CPU this host can read.
#[must_use]
pub fn get_cpu_info() -> CpuInfo {
    CpuInfo {
        architecture: get_architecture(),
        family: get_family(),
        cores: cpu_count(),
        is_64_bit: is_64_bit(),
        page_size: page_size(),
    }
}

/// The GPU. `None`: see [`why_unknown`].
#[must_use]
pub fn get_gpu_info() -> Option<GpuInfo> {
    None
}

/// What the process is using, from the kernel where it reports it.
#[must_use]
pub fn get_memory_info() -> MemoryInfo {
    let usage = crate::diagnostics::get_memory_usage();
    MemoryInfo {
        resident_bytes: usage.resident_bytes,
        virtual_bytes: usage.virtual_bytes,
    }
}

/// The process this mod is in, as [`crate::diagnostics`] already knows it.
#[must_use]
pub fn get_runtime() -> crate::diagnostics::RuntimeInfo {
    crate::diagnostics::get_runtime_info()
}

/// Where this is running, as [`crate::diagnostics`] already knows it.
#[must_use]
pub fn get_environment() -> crate::diagnostics::Environment {
    crate::diagnostics::get_environment()
}

/// The directory a mod may work in, which is the sandboxed one [`crate::fs`] resolves
/// against.
#[must_use]
pub fn get_app_directory() -> Option<PathBuf> {
    crate::fs::root()
}

/// The user's home directory, from the environment.
#[must_use]
pub fn get_home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The system's temporary directory.
#[must_use]
pub fn get_temp_directory() -> PathBuf {
    std::env::temp_dir()
}

/// The three directories in one value, with the one a mod may write to marked.
#[must_use]
pub fn get_paths() -> PlatformPaths {
    let app = get_app_directory();
    PlatformPaths {
        writable: app.is_some(),
        app,
        home: get_home_directory(),
        temp: get_temp_directory(),
    }
}

/// Asks the desktop to open a url. Refused: this host has no desktop session.
pub fn open_url(_url: &str) -> bool {
    refuse("open a url", "url")
}

/// Asks the desktop to open a file. Refused, for the same reason.
pub fn open_file(_path: &str) -> bool {
    refuse("open a file", "file")
}

/// Asks the desktop to reveal a file. Refused, for the same reason.
pub fn reveal_file(_path: &str) -> bool {
    refuse("reveal a file", "reveal")
}

/// Asks to write the clipboard. Refused: the clipboard belongs to the window session.
pub fn copy_to_clipboard(_text: &str) -> bool {
    refuse("write the clipboard", "clipboard_write")
}

/// Reads the clipboard. `None`: see [`why_unknown`].
#[must_use]
pub fn read_clipboard() -> Option<String> {
    None
}

/// The display's mode. `None`: see [`why_unknown`].
#[must_use]
pub fn get_display_info() -> Option<DisplayInfo> {
    None
}

/// The input devices. Empty: see [`why_unknown`].
#[must_use]
pub fn get_input_devices() -> Vec<String> {
    Vec::new()
}

/// The accessibility settings. `None`: see [`why_unknown`].
#[must_use]
pub fn get_accessibility_settings() -> Option<AccessibilitySettings> {
    None
}

fn refuse(action: &str, what: &str) -> bool {
    crate::warn!("a mod asked the platform to {action}: {}", why_unknown(what));
    false
}

/// A platform capability, for [`is_supported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlatformCapability {
    Cpu,
    Memory,
    Runtime,
    Environment,
    Paths,
    PageSize,
    Gpu,
    Clipboard,
    UrlOpen,
    FileOpen,
    RevealFile,
    DisplayInfo,
    InputDevices,
    Accessibility,
    DesktopSession,
}

/// Whether this host can answer `capability` from facts it reads itself.
///
/// The rule: a capability is supported only when it rests on something in this process —
/// `std`, `/proc`, the mod's own directory. Everything that would need the client's window
/// session answers `false` on every platform here, because this host has no session of its
/// own. [`page_size`] is the honest middle case: `std` cannot answer it, so it is `false`.
#[must_use]
pub fn is_supported(capability: PlatformCapability) -> bool {
    match capability {
        PlatformCapability::Cpu => cpu_count() >= 1,
        PlatformCapability::Memory => get_memory_info().resident_bytes.is_some(),
        PlatformCapability::Runtime => !get_runtime().sdk_version.is_empty(),
        PlatformCapability::Environment => !get_environment().platform.is_empty(),
        PlatformCapability::Paths => get_app_directory().is_some() || get_home_directory().is_some(),
        PlatformCapability::PageSize => page_size().is_some(),
        PlatformCapability::Gpu
        | PlatformCapability::Clipboard
        | PlatformCapability::UrlOpen
        | PlatformCapability::FileOpen
        | PlatformCapability::RevealFile
        | PlatformCapability::DisplayInfo
        | PlatformCapability::InputDevices
        | PlatformCapability::Accessibility
        | PlatformCapability::DesktopSession => false,
    }
}

/// Why a platform question has no answer in this host.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "gpu" => {
            "the GPU is a device behind the client's graphics context; this host has no driver handle"
                .to_owned()
        }
        "url" => "opening a url needs a browser in the user's desktop session, which this host has not".to_owned(),
        "file" => "opening a file hands it to the desktop's default application, out of this host's reach".to_owned(),
        "reveal" => "revealing a file needs the desktop's file manager, out of this host's reach".to_owned(),
        "clipboard_write" => "the clipboard belongs to the window session, and this host has no handle on it".to_owned(),
        "clipboard_read" => "reading the clipboard belongs to the window session, and this host has no handle on it".to_owned(),
        "display" => "the display's mode and refresh rate come from the client's window session".to_owned(),
        "input_devices" => "input devices are read by the client through its own window session".to_owned(),
        "accessibility" => "the accessibility settings belong to the client's session, not this host's".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// The platform questions this host cannot answer, each with its reason.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "gpu",
        "url",
        "file",
        "reveal",
        "clipboard_write",
        "clipboard_read",
        "display",
        "input_devices",
        "accessibility",
    ]
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
    fn the_platform_identity_is_what_std_reports() {
        assert_eq!(get_os(), std::env::consts::OS);
        assert_eq!(get_architecture(), std::env::consts::ARCH);
        assert_eq!(get_family(), std::env::consts::FAMILY);
        assert_eq!(
            platform_id(),
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
        );
        assert_eq!(is_64_bit(), std::mem::size_of::<usize>() == 8);
        assert_eq!(is_little_endian(), cfg!(target_endian = "little"));
    }

    #[test]
    fn the_separators_are_the_platforms_own() {
        assert_eq!(path_separator(), std::path::MAIN_SEPARATOR);
        assert_eq!(
            line_separator(),
            if cfg!(windows) { "\r\n" } else { "\n" }
        );
        assert_eq!(line_separator().len(), usize::from(cfg!(windows)) + 1);
    }

    #[test]
    fn the_cpu_facts_are_real_and_the_page_size_is_honestly_unknown() {
        let cpu = get_cpu_info();
        assert!(cpu.cores >= 1);
        assert_eq!(cpu.cores, cpu_count());
        assert_eq!(cpu.architecture, get_architecture());
        assert_eq!(cpu.family, get_family());
        assert_eq!(cpu.is_64_bit, is_64_bit());
        assert!(cpu.page_size.is_none());
        assert!(page_size().is_none());
        assert!(!page_size_reason().is_empty());
    }

    #[test]
    fn a_capability_is_reported_only_when_this_host_can_answer_it() {
        assert!(is_supported(PlatformCapability::Cpu));
        assert!(is_supported(PlatformCapability::Runtime));
        assert!(is_supported(PlatformCapability::Environment));
        assert!(!is_supported(PlatformCapability::PageSize));
        assert!(!is_supported(PlatformCapability::Gpu));
        assert!(!is_supported(PlatformCapability::Clipboard));
        assert!(!is_supported(PlatformCapability::DesktopSession));
    }

    #[test]
    fn the_process_and_memory_facts_reuse_diagnostics() {
        let runtime = get_runtime();
        assert_eq!(runtime.pid, std::process::id());
        assert!(!runtime.sdk_version.is_empty());
        assert_eq!(get_environment().platform, get_os());
        assert_eq!(
            get_memory_info().resident_bytes.is_some(),
            crate::diagnostics::get_memory_usage().resident_bytes.is_some()
        );
        if cfg!(target_os = "linux") {
            assert!(get_memory_info().resident_bytes.unwrap_or_default() > 0);
        }
    }

    #[test]
    fn the_paths_a_mod_may_use_are_real() {
        let _serial = serial();
        let root = std::env::temp_dir().join(format!(
            "cinnaloader-platform-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());
        assert_eq!(get_app_directory(), Some(root.clone()));
        let paths = get_paths();
        assert_eq!(paths.app, Some(root.clone()));
        assert!(paths.writable);
        assert_eq!(paths.temp, std::env::temp_dir());
        assert!(paths.temp.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_platform_request_the_desktop_owns_is_refused_with_its_sentence() {
        assert!(!open_url("https://example.invalid"));
        assert!(!open_file("notes.txt"));
        assert!(!reveal_file("notes.txt"));
        assert!(!copy_to_clipboard("hello"));
        assert!(read_clipboard().is_none());
        assert!(get_display_info().is_none());
        assert!(get_input_devices().is_empty());
        assert!(get_accessibility_settings().is_none());
        assert!(get_gpu_info().is_none());
        assert_eq!(refusals().len(), 9);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
