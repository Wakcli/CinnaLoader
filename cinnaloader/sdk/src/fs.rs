//! Files: everything a mod may do with its own directory, and nothing outside it.
//!
//! Every path here is relative to the mod's own directory, and a path that tries to leave it
//! — `..`, an absolute path, a symlink out — is refused with the reason. That is the
//! `filesystem_read` and `filesystem_write` capability made concrete: a mod gets its own
//! folder, not the user's disk.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! # let root = std::env::temp_dir().join("cinnaloader-doc-fs");
//! # let _ = std::fs::remove_dir_all(&root);
//! # std::fs::create_dir_all(&root).unwrap();
//! # fs::set_root(root);
//! fs::create_directories("cache").expect("the mod may make its own directories");
//! fs::write_text("cache/last-run.txt", "hello").expect("and write in them");
//! assert_eq!(fs::read_text("cache/last-run.txt").unwrap(), "hello");
//! for entry in fs::walk("cache").unwrap() {
//!     info!("{}", entry.display());
//! }
//! # fs::delete("cache").ok();
//! ```

use std::fmt;
use std::fs as std_fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// What went wrong, with the path that went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    /// The path tries to leave the mod's own directory.
    Outside(String),
    /// There is no such file or directory.
    Missing(String),
    /// The path is not what the call needs, such as a directory where a file was wanted.
    WrongKind(String),
    /// The operating system refused, with its message.
    Io(String, String),
    /// A watch was asked for something that is not a file or directory.
    Unwatchable(String),
}

impl FsError {
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Outside(path)
            | Self::Missing(path)
            | Self::WrongKind(path)
            | Self::Unwatchable(path)
            | Self::Io(path, _) => path,
        }
    }

    /// The same error as an [`std::io::Error`], for a caller that wants one.
    #[must_use]
    pub fn to_io(&self) -> std::io::Error {
        match self {
            Self::Missing(path) => std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no such file or directory: {path}"),
            ),
            Self::Outside(path) => std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{path} is outside this mod's directory"),
            ),
            Self::WrongKind(path) => std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("wrong kind of path: {path}"),
            ),
            Self::Unwatchable(path) => std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("cannot watch {path}"),
            ),
            Self::Io(_, message) => std::io::Error::other(message.clone()),
        }
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Outside(path) => write!(formatter, "{} is outside this mod's directory", path),
            Self::Missing(path) => write!(formatter, "no such file or directory: {path}"),
            Self::WrongKind(path) => write!(formatter, "wrong kind of path: {path}"),
            Self::Io(path, message) => write!(formatter, "{path}: {message}"),
            Self::Unwatchable(path) => write!(formatter, "cannot watch {path}"),
        }
    }
}

impl std::error::Error for FsError {}

impl From<std::io::Error> for FsError {
    fn from(error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => return Self::Missing(String::new()),
            std::io::ErrorKind::PermissionDenied => "the operating system refused".to_owned(),
            _ => error.to_string(),
        };
        Self::Io(String::new(), kind)
    }
}

/// A readable, writable handle to a file, for a caller that wants to stream rather than
/// read it all at once.
pub struct File {
    path: PathBuf,
    inner: std_fs::File,
}

impl File {
    /// Where it is, inside the mod's directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads everything left in it.
    pub fn read_to_end(&mut self) -> Result<Vec<u8>, FsError> {
        let mut bytes = Vec::new();
        self.inner
            .read_to_end(&mut bytes)
            .map_err(|error| FsError::Io(self.path.display().to_string(), error.to_string()))?;
        Ok(bytes)
    }

    /// Reads everything left in it, as text.
    pub fn read_to_string(&mut self) -> Result<String, FsError> {
        let bytes = self.read_to_end()?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Reads up to `count` bytes.
    pub fn read(&mut self, count: usize) -> Result<Vec<u8>, FsError> {
        let mut buffer = vec![0u8; count];
        let read = self
            .inner
            .read(&mut buffer)
            .map_err(|error| FsError::Io(self.path.display().to_string(), error.to_string()))?;
        buffer.truncate(read);
        Ok(buffer)
    }

    /// Writes bytes.
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize, FsError> {
        self.inner
            .write_all(bytes)
            .map_err(|error| FsError::Io(self.path.display().to_string(), error.to_string()))?;
        Ok(bytes.len())
    }

    /// Pushes what has been written to the disk.
    pub fn flush(&mut self) -> Result<(), FsError> {
        self.inner
            .flush()
            .map_err(|error| FsError::Io(self.path.display().to_string(), error.to_string()))
    }
}

/// Where a scratch file goes when it is not inside the mod's directory. Set by the loader;
/// without one, the mod's own `tmp` directory is used.
static SCRATCH: OnceLock<PathBuf> = OnceLock::new();

/// The loader calls this with a directory it has made for scratch files.
pub fn set_scratch_directory(path: PathBuf) {
    let _ = SCRATCH.set(path);
}

/// A root the caller set, which a test uses to work somewhere harmless.
static ROOT: OnceLock<std::sync::RwLock<Option<PathBuf>>> = OnceLock::new();

fn root_slot() -> &'static std::sync::RwLock<Option<PathBuf>> {
    ROOT.get_or_init(|| std::sync::RwLock::new(None))
}

/// Points this module at the directory a mod may work in. The loader calls it with the mod's
/// own directory as it loads it; nothing else should.
pub fn set_root(path: PathBuf) {
    if let Ok(mut root) = root_slot().write() {
        *root = Some(path);
    }
}

/// The directory this mod's paths are relative to: the one the loader set, or the mod's
/// config directory, or nothing at all — in which case every path is refused, because
/// writing somewhere unknown is worse than an error.
#[must_use]
pub fn root() -> Option<PathBuf> {
    root_slot()
        .read()
        .ok()
        .and_then(|root| root.clone())
        .or_else(crate::metadata::config_directory)
}

/// Resolves a path inside the mod's own directory, or refuses it.
///
/// This is the whole security story of this module: `..`, absolute paths and anything that
/// only leaves the root are errors, never writes somewhere unexpected.
pub fn resolve(path: &str) -> Result<PathBuf, FsError> {
    let root = root().ok_or_else(|| {
        FsError::Outside(format!(
            "{path}: this mod has no directory to work in, so nothing is read or written"
        ))
    })?;
    resolve_in(&root, path)
}

fn resolve_in(root: &Path, path: &str) -> Result<PathBuf, FsError> {
    if path.is_empty() {
        return Ok(root.to_owned());
    }
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        // An absolute path is allowed only when it is already inside the root, which is how
        // a caller passes back something `walk` handed it.
        if candidate.starts_with(root) && !escapes(root, candidate) {
            return Ok(candidate.to_owned());
        }
        return Err(FsError::Outside(path.to_owned()));
    }
    let mut joined = root.to_owned();
    for part in candidate.components() {
        match part {
            Component::Normal(name) => joined.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                if !joined.pop() || !joined.starts_with(root) {
                    return Err(FsError::Outside(path.to_owned()));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::Outside(path.to_owned()));
            }
        }
    }
    if escapes(root, &joined) {
        return Err(FsError::Outside(path.to_owned()));
    }
    Ok(joined)
}

/// Whether following the path as it stands now would end up outside the root, which is how a
/// symlink pointing out is caught.
fn escapes(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize().or_else(|_| Ok::<_, std::io::Error>(root.to_owned()))
    else {
        return true;
    };
    match path.canonicalize() {
        Ok(real) => !real.starts_with(&root),
        // Not there yet, so nothing to follow: the lexical check already passed.
        Err(_) => false,
    }
}

fn with_path<T>(path: &str, body: impl FnOnce(&Path) -> Result<T, FsError>) -> Result<T, FsError> {
    let resolved = resolve(path)?;
    body(&resolved).map_err(|error| match error {
        FsError::Missing(_) => FsError::Missing(path.to_owned()),
        FsError::WrongKind(_) => FsError::WrongKind(path.to_owned()),
        FsError::Io(_, message) => FsError::Io(path.to_owned(), message),
        other => other,
    })
}

fn io(path: &Path, error: std::io::Error) -> FsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::Missing(path.display().to_string()),
        _ => FsError::Io(path.display().to_string(), error.to_string()),
    }
}

#[must_use]
pub fn exists(path: &str) -> bool {
    with_path(path, |path| Ok(path.exists())).unwrap_or(false)
}

#[must_use]
pub fn is_file(path: &str) -> bool {
    with_path(path, |path| Ok(path.is_file())).unwrap_or(false)
}

#[must_use]
pub fn is_directory(path: &str) -> bool {
    with_path(path, |path| Ok(path.is_dir())).unwrap_or(false)
}

/// Bytes, or an error saying why not.
pub fn read(path: &str) -> Result<Vec<u8>, FsError> {
    with_path(path, |path| std_fs::read(path).map_err(|error| io(path, error)))
}

/// Bytes, under the name the wishlist uses for the same thing.
pub fn read_bytes(path: &str) -> Result<Vec<u8>, FsError> {
    read(path)
}

/// Text, which is the common case.
pub fn read_text(path: &str) -> Result<String, FsError> {
    with_path(path, |path| {
        std_fs::read_to_string(path).map_err(|error| io(path, error))
    })
}

/// Writes bytes, making the directory above it when it does not exist yet.
pub fn write(path: &str, bytes: &[u8]) -> Result<usize, FsError> {
    with_path(path, |path| {
        if let Some(parent) = path.parent() {
            std_fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        }
        std_fs::write(path, bytes).map_err(|error| io(path, error))?;
        Ok(bytes.len())
    })
}

pub fn write_bytes(path: &str, bytes: &[u8]) -> Result<usize, FsError> {
    write(path, bytes)
}

/// Writes text.
pub fn write_text(path: &str, text: &str) -> Result<usize, FsError> {
    write(path, text.as_bytes())
}

/// Adds bytes to the end of a file, making it when it is not there.
pub fn append(path: &str, bytes: &[u8]) -> Result<usize, FsError> {
    with_path(path, |path| {
        if let Some(parent) = path.parent() {
            std_fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        }
        let mut file = std_fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| io(path, error))?;
        file.write_all(bytes).map_err(|error| io(path, error))?;
        Ok(bytes.len())
    })
}

/// Copies a file, refusing to overwrite one that is already there unless `overwrite`.
pub fn copy(from: &str, to: &str, overwrite: bool) -> Result<u64, FsError> {
    let from = resolve(from)?;
    with_path(to, |to| {
        if to.exists() && !overwrite {
            return Err(FsError::WrongKind(format!(
                "{} is already there",
                to.display()
            )));
        }
        if let Some(parent) = to.parent() {
            std_fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        }
        std_fs::copy(&from, to).map_err(|error| io(to, error))
    })
}

/// Moves a file or a directory.
pub fn move_path(from: &str, to: &str) -> Result<(), FsError> {
    let from = resolve(from)?;
    with_path(to, |to| {
        if let Some(parent) = to.parent() {
            std_fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        }
        std_fs::rename(&from, to).map_err(|error| io(to, error))
    })
}

/// The same as [`move_path`], under the name the wishlist uses for the same thing.
pub fn rename(from: &str, to: &str) -> Result<(), FsError> {
    move_path(from, to)
}

/// Deletes a file, or a directory and everything under it.
pub fn delete(path: &str) -> Result<(), FsError> {
    with_path(path, |path| match path.is_dir() {
        true => std_fs::remove_dir_all(path).map_err(|error| io(path, error)),
        false => std_fs::remove_file(path).map_err(|error| io(path, error)),
    })
}

/// Makes an empty file, or truncates one that is already there.
pub fn create_file(path: &str) -> Result<(), FsError> {
    with_path(path, |path| {
        if let Some(parent) = path.parent() {
            std_fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        }
        std_fs::write(path, []).map_err(|error| io(path, error))
    })
}

/// Makes one directory. The directory above it has to be there already.
pub fn create_directory(path: &str) -> Result<(), FsError> {
    with_path(path, |path| {
        std_fs::create_dir(path).map_err(|error| io(path, error))
    })
}

/// Makes a directory and everything above it.
pub fn create_directories(path: &str) -> Result<(), FsError> {
    with_path(path, |path| {
        std_fs::create_dir_all(path).map_err(|error| io(path, error))
    })
}

/// What is directly inside a directory, in name order.
pub fn list(path: &str) -> Result<Vec<PathBuf>, FsError> {
    let mut entries: Vec<PathBuf> = with_path(path, |path| {
        if !path.is_dir() {
            return Err(FsError::WrongKind(path.display().to_string()));
        }
        let read = std_fs::read_dir(path).map_err(|error| io(path, error))?;
        let mut entries = Vec::new();
        for entry in read {
            let entry = entry.map_err(|error| io(path, error))?;
            entries.push(entry.path());
        }
        Ok(entries)
    })?;
    entries.sort();
    Ok(entries)
}

/// Everything under a directory, directories included.
pub fn list_recursive(path: &str) -> Result<Vec<PathBuf>, FsError> {
    walk(path)
}

/// Walks a directory, depth first, in name order.
pub fn walk(path: &str) -> Result<Vec<PathBuf>, FsError> {
    let start = resolve(path)?;
    let mut found = Vec::new();
    let mut queue = vec![start.clone()];
    while let Some(next) = queue.pop() {
        let entries = match std_fs::read_dir(&next) {
            Ok(entries) => entries,
            Err(error) if next == start => return Err(io(&next, error)),
            // A file where a directory was expected ends the walk there rather than failing
            // the lot, which is what a caller walking a tree wants.
            Err(_) => continue,
        };
        for entry in entries {
            let entry = entry.map_err(|error| io(&next, error))?;
            let path = entry.path();
            found.push(path.clone());
            if path.is_dir() {
                queue.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// How big a file is, how old it is, and whether it is a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub path: PathBuf,
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub read_only: bool,
}

impl Metadata {
    /// How long ago it was changed, for a caller that wants that rather than a timestamp.
    #[must_use]
    pub fn age(&self) -> Option<Duration> {
        self.modified
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
    }
}

/// What the operating system says about a path.
pub fn metadata(path: &str) -> Result<Metadata, FsError> {
    with_path(path, |path| {
        let link = std_fs::symlink_metadata(path).map_err(|error| io(path, error))?;
        let real = std_fs::metadata(path).map_err(|error| io(path, error))?;
        Ok(Metadata {
            path: path.to_owned(),
            is_file: real.is_file(),
            is_directory: real.is_dir(),
            is_symlink: link.file_type().is_symlink(),
            size: real.len(),
            modified: real.modified().ok(),
            created: real.created().ok(),
            read_only: real.permissions().readonly(),
        })
    })
}

/// How many bytes a file holds, or 0 when there is no such file.
#[must_use]
pub fn file_size(path: &str) -> u64 {
    metadata(path).map(|metadata| metadata.size).unwrap_or_default()
}

/// When it was last changed.
#[must_use]
pub fn modified_time(path: &str) -> Option<SystemTime> {
    metadata(path).ok().and_then(|metadata| metadata.modified)
}

/// The real path, with `.` and `..` resolved — always inside the mod's directory.
pub fn canonicalize(path: &str) -> Result<PathBuf, FsError> {
    with_path(path, |path| path.canonicalize().map_err(|error| io(path, error)))
}

/// A file nobody else has, under `directory` when one is given.
pub fn temporary_file(directory: Option<&str>) -> Result<PathBuf, FsError> {
    let directory = scratch(directory)?;
    let mut last = None;
    for _ in 0..32 {
        let candidate = directory.join(format!("tmp-{}-{}", std::process::id(), next_scratch()));
        match std_fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(error) => last = Some(error),
        }
    }
    Err(FsError::Io(
        directory.display().to_string(),
        last.map(|error| error.to_string())
            .unwrap_or_else(|| "no name was free".to_owned()),
    ))
}

/// A directory nobody else has, under `directory` when one is given.
pub fn temporary_directory(directory: Option<&str>) -> Result<PathBuf, FsError> {
    let directory = scratch(directory)?;
    for _ in 0..32 {
        let candidate = directory.join(format!("tmp-{}-{}", std::process::id(), next_scratch()));
        match std_fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io(&candidate, error)),
        }
    }
    Err(FsError::Io(
        directory.display().to_string(),
        "no name was free".to_owned(),
    ))
}

fn scratch(directory: Option<&str>) -> Result<PathBuf, FsError> {
    let directory = match directory {
        Some(path) => resolve(path)?,
        None => match SCRATCH.get() {
            Some(path) if path.is_dir() => path.clone(),
            _ => {
                let path = resolve("tmp")?;
                std_fs::create_dir_all(&path).map_err(|error| io(&path, error))?;
                path
            }
        },
    };
    std_fs::create_dir_all(&directory).map_err(|error| io(&directory, error))?;
    Ok(directory)
}

fn next_scratch() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Writes a file by writing a neighbour first and renaming it over the top, so a crash
/// halfway through leaves the old file rather than half a new one.
pub fn atomic_write(path: &str, bytes: &[u8]) -> Result<usize, FsError> {
    let target = resolve(path)?;
    let temporary = target.with_extension(format!("part{}", next_scratch()));
    let written = with_path(&temporary.to_string_lossy(), |temporary| {
        if let Some(parent) = temporary.parent() {
            std_fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        }
        std_fs::write(temporary, bytes).map_err(|error| io(temporary, error))?;
        Ok(bytes.len())
    })?;
    std_fs::rename(&temporary, &target).map_err(|error| {
        let _ = std_fs::remove_file(&temporary);
        io(&target, error)
    })?;
    Ok(written)
}

/// A hash of a file's contents: FNV-1a, which is quick and not cryptographic. Use it to
/// notice that a file changed, not to prove that it did not.
#[must_use]
pub fn hash(path: &str) -> Option<u64> {
    read(path).ok().map(|bytes| hash_bytes(&bytes))
}

/// The same hash over bytes already in hand.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Opens a file to read it.
pub fn open_read(path: &str) -> Result<File, FsError> {
    with_path(path, |path| {
        let inner = std_fs::File::open(path).map_err(|error| io(path, error))?;
        Ok(File {
            path: path.to_owned(),
            inner,
        })
    })
}

/// Opens a file to write it, making it and the directory above it when they are not there.
pub fn open_write(path: &str) -> Result<File, FsError> {
    with_path(path, |path| {
        if let Some(parent) = path.parent() {
            std_fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        }
        let inner = std_fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|error| io(path, error))?;
        Ok(File {
            path: path.to_owned(),
            inner,
        })
    })
}

/// Packs bytes into this loader's own container: a run-length pass, which is what a mod
/// keeping text or repeated structures gets for free without a compression dependency.
///
/// It is not a general-purpose compressor and does not claim to be; the round trip through
/// [`decompress`] is exact for any input.
#[must_use]
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut packed = Vec::with_capacity(bytes.len() / 2 + 8);
    packed.extend_from_slice(b"CINNA1");
    packed.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    let mut index = 0;
    while index < bytes.len() {
        let mut run = 1usize;
        while index + run < bytes.len() && bytes[index + run] == bytes[index] && run < 255 {
            run += 1;
        }
        if run > 2 || bytes[index] == 0 {
            // A zero byte is the marker for a run, so it is always written as one, even
            // when the run is a single byte.
            packed.push(0);
            packed.push(run as u8);
            packed.push(bytes[index]);
        } else {
            for _ in 0..run {
                packed.push(bytes[index]);
            }
        }
        index += run;
    }
    packed
}

/// Unpacks what [`compress`] wrote, and refuses anything else.
pub fn decompress(packed: &[u8]) -> Result<Vec<u8>, FsError> {
    if packed.len() < 14 || &packed[..6] != b"CINNA1" {
        return Err(FsError::WrongKind(
            "not a cinnaloader pack: no header".to_owned(),
        ));
    }
    let mut length = [0u8; 8];
    length.copy_from_slice(&packed[6..14]);
    let length = u64::from_le_bytes(length) as usize;
    let mut out = Vec::with_capacity(length);
    let mut index = 14;
    while index < packed.len() {
        let byte = packed[index];
        if byte == 0 {
            if index + 2 >= packed.len() {
                return Err(FsError::WrongKind("truncated pack".to_owned()));
            }
            let run = packed[index + 1] as usize;
            let value = packed[index + 2];
            out.extend(std::iter::repeat_n(value, run));
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    if out.len() != length {
        return Err(FsError::WrongKind(format!(
            "the pack says {length} bytes and held {}",
            out.len()
        )));
    }
    Ok(out)
}

/// A file or directory being watched for changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub id: u64,
    pub path: PathBuf,
}

type Watcher = Box<dyn FnMut(&Path, Option<SystemTime>) + Send>;

struct Held {
    watchers: Vec<(u64, PathBuf, Option<SystemTime>, Watcher)>,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held { watchers: Vec::new() }))
}

/// Calls `watcher` when a path inside the mod's directory is written. The loader polls on
/// every tick — there is no inotify in this host — so a change is seen within a tick.
pub fn watch(
    path: &str,
    watcher: impl FnMut(&Path, Option<SystemTime>) + Send + 'static,
) -> Result<u64, FsError> {
    let resolved = resolve(path)?;
    if !resolved.exists() {
        return Err(FsError::Missing(path.to_owned()));
    }
    let modified = std_fs::metadata(&resolved).ok().and_then(|meta| meta.modified().ok());
    static NEXT_WATCH: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_WATCH.fetch_add(1, Ordering::Relaxed);
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    guard.watchers.push((id, resolved, modified, Box::new(watcher)));
    ensure_polling();
    Ok(id)
}

/// Stops watching, and says whether that watch existed.
pub fn unwatch(id: u64) -> bool {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    let before = guard.watchers.len();
    guard.watchers.retain(|(held, _, _, _)| *held != id);
    guard.watchers.len() != before
}

/// How many paths are being watched.
#[must_use]
pub fn watch_count() -> usize {
    held()
        .lock()
        .map(|guard| guard.watchers.len())
        .unwrap_or_default()
}

/// Checks every watched path once. The loader calls it on a timer; a test can call it too.
pub fn poll_watches() -> usize {
    let mut changed = 0;
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    for (_, path, last, watcher) in &mut guard.watchers {
        let modified = std_fs::metadata(&*path).ok().and_then(|meta| meta.modified().ok());
        if modified != *last {
            *last = modified;
            watcher(path, modified);
            changed += 1;
        }
    }
    changed
}

fn ensure_polling() {
    static POLLING: OnceLock<()> = OnceLock::new();
    POLLING.get_or_init(|| {
        crate::tasks::schedule_repeating(Duration::from_millis(500), || {
            poll_watches();
        });
    });
}

/// Every path inside a directory whose contents hash the same, which is how two copies are
/// compared.
pub fn identical(left: &str, right: &str) -> bool {
    match (hash(left), hash(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    /// A root of its own, so the tests never write into the repository.
    fn rooted(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cinnaloader-fs-{}-{name}",
            std::process::id()
        ));
        let _ = std_fs::remove_dir_all(&root);
        std_fs::create_dir_all(&root).unwrap();
        set_root(root.clone());
        root
    }

    /// A path inside the current test root, as the module wants it: relative.
    fn inside(name: &str) -> String {
        name.to_owned()
    }

    #[test]
    fn a_path_that_leaves_the_directory_is_refused() {
        let _serial = serial();
        let root = rooted("escape");
        assert!(resolve_in(&root, "notes.txt").is_ok());
        assert!(resolve_in(&root, "a/b/../c.txt").is_ok());
        assert_eq!(
            resolve_in(&root, "../outside.txt"),
            Err(FsError::Outside("../outside.txt".to_owned()))
        );
        assert_eq!(
            resolve_in(&root, "/etc/passwd"),
            Err(FsError::Outside("/etc/passwd".to_owned()))
        );
        assert_eq!(
            resolve_in(&root, "a/../../outside.txt"),
            Err(FsError::Outside("a/../../outside.txt".to_owned()))
        );
        assert_eq!(
            resolve_in(&root, "a").unwrap(),
            root.join("a"),
            "a plain path resolves under the root"
        );
        let _ = std_fs::remove_dir_all(&root);
    }

    #[test]
    fn text_and_bytes_round_trip_and_the_directories_appear() {
        let _serial = serial();
        let root = rooted("round-trip");
        let name = inside("a/b/note.txt");
        assert_eq!(write_text(&name, "hello").unwrap(), 5);
        assert!(exists(&name));
        assert!(is_file(&name));
        assert!(is_directory("a"));
        assert_eq!(read_text(&name).unwrap(), "hello");
        assert_eq!(read_bytes(&name).unwrap(), b"hello".to_vec());
        assert_eq!(file_size(&name), 5);
        assert!(modified_time(&name).is_some());
        assert_eq!(append(&name, b" there").unwrap(), 6);
        assert_eq!(read_text(&name).unwrap(), "hello there");
        assert_eq!(metadata(&name).unwrap().size, 11);
        let _ = std_fs::remove_dir_all(&root);
    }

    #[test]
    fn an_atomic_write_leaves_no_half_file_behind() {
        let _serial = serial();
        let root = rooted("atomic");
        let name = inside("config.txt");
        atomic_write(&name, b"first").unwrap();
        atomic_write(&name, b"second").unwrap();
        assert_eq!(read_text(&name).unwrap(), "second");
        let leftovers: Vec<PathBuf> = list("")
            .unwrap()
            .into_iter()
            .filter(|entry| entry.extension().is_some_and(|ext| ext.to_string_lossy().starts_with("part")))
            .collect();
        assert!(leftovers.is_empty(), "the part file is gone: {leftovers:?}");
        let _ = std_fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tree_can_be_listed_and_walked() {
        let _serial = serial();
        let root = rooted("walk");
        write_text("one.txt", "1").unwrap();
        write_text("deep/two.txt", "2").unwrap();
        let top = list("").unwrap();
        assert_eq!(top.len(), 2, "one file and one directory: {top:?}");
        let all = walk("").unwrap();
        assert_eq!(all.len(), 3, "plus what is inside: {all:?}");
        assert!(all.iter().any(|path| path.ends_with("two.txt")));
        let _ = std_fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_can_be_copied_moved_and_deleted() {
        let _serial = serial();
        let root = rooted("move");
        let source = inside("source.txt");
        let copy_to = inside("copy.txt");
        let moved = inside("moved.txt");
        write_text(&source, "content").unwrap();
        assert_eq!(copy(&source, &copy_to, false).unwrap(), 7);
        assert!(copy(&source, &copy_to, false).is_err(), "no overwriting by default");
        assert_eq!(copy(&source, &copy_to, true).unwrap(), 7);
        move_path(&copy_to, &moved).unwrap();
        assert!(!exists(&copy_to));
        assert!(identical(&source, &moved));
        delete(&moved).unwrap();
        assert!(!exists(&moved));
        assert!(delete(&moved).is_err(), "deleting what is gone is an error");
        let _ = std_fs::remove_dir_all(&root);
    }

    #[test]
    fn a_temporary_file_is_under_the_directory_and_private() {
        let _serial = serial();
        let root = rooted("temp");
        set_scratch_directory(root.clone());
        let one = temporary_file(None).unwrap();
        let two = temporary_file(None).unwrap();
        assert_ne!(one, two);
        assert!(one.starts_with(&root));
        let directory = temporary_directory(None).unwrap();
        assert!(directory.is_dir());
        let _ = std_fs::remove_dir_all(&root);
    }

    #[test]
    fn what_was_packed_reads_back_exactly() {
        let text = b"aaaaaaaabbbbbbbbbbbbcdddddddddddddddddddddddddddddddddd".to_vec();
        let packed = compress(&text);
        assert!(packed.len() < text.len(), "a run compresses");
        assert_eq!(decompress(&packed).unwrap(), text);
        let noise: Vec<u8> = (0..1024u32).map(|index| (index * 7 % 251) as u8).collect();
        assert_eq!(decompress(&compress(&noise)).unwrap(), noise);
        assert!(decompress(b"not a pack at all").is_err());
    }

    #[test]
    fn a_hash_notices_a_change() {
        let _serial = serial();
        let root = rooted("hash");
        let path = inside("h.txt");
        write_text(&path, "one").unwrap();
        let first = hash(&path).unwrap();
        write_text(&path, "two").unwrap();
        assert_ne!(hash(&path).unwrap(), first);
        assert_eq!(hash_bytes(b""), 0xcbf2_9ce4_8422_2325);
        let _ = std_fs::remove_dir_all(&root);
    }

    #[test]
    fn a_handle_reads_what_was_written_through_another() {
        let _serial = serial();
        let root = rooted("handle");
        let path = inside("h.txt");
        let mut file = open_write(&path).unwrap();
        file.write(b"streamed").unwrap();
        file.flush().unwrap();
        assert_eq!(file.path(), resolve(&path).unwrap());
        let mut reader = open_read(&path).unwrap();
        assert_eq!(reader.read_to_string().unwrap(), "streamed");
        let _ = std_fs::remove_dir_all(&root);
    }

    #[test]
    fn a_watcher_hears_about_a_write() {
        let _serial = serial();
        let root = rooted("watch");
        let name = inside("watched.txt");
        write_text(&name, "first").unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&seen);
        let id = watch(&name, move |path, _| {
            into.lock().unwrap().push(path.display().to_string());
        })
        .unwrap();
        assert_eq!(watch_count(), 1);
        assert_eq!(poll_watches(), 0, "nothing changed yet");
        // The filesystem's timestamp resolution is coarse, so make the change obviously
        // later rather than hoping.
        std::thread::sleep(Duration::from_millis(20));
        write_text(&name, "second").unwrap();
        std::fs::File::options()
            .write(true)
            .open(resolve(&name).unwrap())
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(1))
            .unwrap();
        assert_eq!(poll_watches(), 1);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(unwatch(id));
        assert_eq!(watch_count(), 0);
        let _ = std_fs::remove_dir_all(&root);
    }
}
