//! A mod package format this host really writes and reads back.
//!
//! A package is a header and a payload. The header is a magic string, the format version, the
//! loader ABI the mod was built against, a signature fingerprint, the mod id and version, and
//! one entry per file naming its size and hash. The payload is those files, in entry order.
//! [`pack`] writes the bytes and [`open`] reads them back; a package that round trips is equal
//! to the one written, down to the byte.
//!
//! Every byte of the layout is little-endian and a string is a `u32` length then its UTF-8:
//!
//! ```text
//! magic      : 8 bytes, `CINNAPKG`
//! format     : u32
//! abi        : u32
//! signature  : u64
//! id         : str
//! version    : str
//! count      : u32
//! entry*     : name str, size u32, hash u64
//! payload    : the files' bytes, in entry order
//! ```
//!
//! [`open`] refuses rather than guesses: a bad magic is [`PackageError::BadMagic`], a package
//! cut short is [`PackageError::Truncated`] naming what was missing, a format newer than
//! [`FORMAT_VERSION`] is [`PackageError::UnsupportedFormat`], and a file whose bytes no longer
//! hash to what the header recorded is [`PackageError::Corrupt`] naming the first such file.
//! The ABI check compares the header's ABI against [`crate::ABI_VERSION`]: this loader bumps
//! that number on every breaking change, so the whole number is its major and any other is
//! refused.
//!
//! The signature is a **fingerprint comparison, not cryptography**. It is the FNV-1a hash of
//! the mod's declared key and the payload, and [`verify_signature`] answers whether a caller's
//! list of trusted fingerprints contains it. It proves nothing against an attacker who can
//! edit the key and recompute; it is a cheap "is this the build I trusted" check, nothing more.
//!
//! What this host cannot read is the client's own package format, refused by
//! [`read_client_package`] with [`why_unknown`] and [`refusals`].
//!
//! ```
//! use cinnaloader::packages::{self, Manifest, Package, SignatureCheck};
//!
//! let manifest = Manifest::new("hello", "Hello", "1.2.0").key("hello-key");
//! let package = Package::new(&manifest, vec![("greeting.txt".to_owned(), b"hi".to_vec())]);
//! let bytes = packages::pack(&package);
//! assert_eq!(bytes.len(), 167);
//!
//! let opened = packages::open(&bytes).expect("a whole package");
//! assert_eq!(opened, package);
//! assert_eq!(packages::list_contents(&opened), vec!["manifest", "greeting.txt"]);
//! assert_eq!(packages::extract(&opened, "greeting.txt").as_deref(), Some(&b"hi"[..]));
//!
//! let trusted = [packages::signature_of("hello-key", &opened.payload)];
//! assert!(matches!(
//!     packages::verify_signature(&opened, &trusted),
//!     Ok(SignatureCheck::Matched { .. })
//! ));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::permissions::TrustLevel;
use crate::version::Version;

/// The eight bytes every package starts with.
pub const MAGIC: &[u8; 8] = b"CINNAPKG";
/// The format this build writes. A package whose format is above it is refused.
pub const FORMAT_VERSION: u32 = 1;
/// The ABI a package must name. [`crate::ABI_VERSION`](crate::ABI_VERSION) is a major-only
/// number — this loader bumps it on every breaking change — so any other is a different major.
pub const ABI_MAJOR: u32 = crate::ABI_VERSION;
/// The name the manifest is stored under inside a package.
pub const MANIFEST_FILE: &str = "manifest";
/// The most files a package header may declare.
pub const MAX_FILES: usize = 4096;

/// Anything that can go wrong writing, reading or checking a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageError {
    /// The bytes do not start with the package magic.
    BadMagic { found: String },
    /// The package is a newer format than this build reads.
    UnsupportedFormat { found: u32, supported: u32 },
    /// The package ended before a field was complete.
    Truncated { missing: String },
    /// Bytes after the files the header declares.
    Trailing { extra: usize },
    /// The loader ABI the package was built for is not this build's major.
    AbiMismatch { found: u32, supported: u32 },
    /// A file's bytes no longer hash to what the header recorded.
    Corrupt { file: String, want: u64, got: u64 },
    /// An (id, version) pair already in the index.
    Duplicate { id: String, version: String },
    /// A header or manifest field this host cannot read.
    Malformed(String),
    /// The client's own package format, with [`why_unknown`] as the reason.
    Refused(String),
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic { found } => {
                write!(formatter, "the bytes start with {found:?}, not a package magic")
            }
            Self::UnsupportedFormat { found, supported } => {
                write!(formatter, "the package is format {found} and this build reads {supported}")
            }
            Self::Truncated { missing } => write!(formatter, "the package ended before {missing}"),
            Self::Trailing { extra } => write!(formatter, "{extra} bytes past the declared files"),
            Self::AbiMismatch { found, supported } => write!(
                formatter,
                "the package was built for loader ABI {found} and this build is {supported}"
            ),
            Self::Corrupt { file, want, got } => {
                write!(formatter, "'{file}' hashes to {got} and the header records {want}")
            }
            Self::Duplicate { id, version } => {
                write!(formatter, "{id} {version} is already in the package index")
            }
            Self::Malformed(what) => write!(formatter, "malformed package: {what}"),
            Self::Refused(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for PackageError {}

/// One file a package carries: its name, its size and the hash of its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub size: u32,
    pub hash: u64,
}

/// A package's header: the facts a reader needs before its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub format: u32,
    pub abi: u32,
    pub signature: u64,
    pub id: String,
    pub version: String,
    pub files: Vec<FileEntry>,
}

/// What a mod declares about itself beyond the id and version in the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub abi: u32,
    pub key: String,
    pub trust: TrustLevel,
    pub dependencies: Vec<String>,
    pub permissions: Vec<String>,
}

impl Manifest {
    #[must_use]
    pub fn new(id: &str, name: &str, version: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            abi: ABI_MAJOR,
            key: String::new(),
            trust: TrustLevel::default(),
            dependencies: Vec::new(),
            permissions: Vec::new(),
        }
    }

    #[must_use]
    pub fn abi(mut self, abi: u32) -> Self {
        self.abi = abi;
        self
    }

    #[must_use]
    pub fn key(mut self, key: &str) -> Self {
        self.key = key.to_owned();
        self
    }

    #[must_use]
    pub fn trust(mut self, trust: TrustLevel) -> Self {
        self.trust = trust;
        self
    }

    #[must_use]
    pub fn depends_on(mut self, id: &str) -> Self {
        self.dependencies.push(id.to_owned());
        self
    }

    #[must_use]
    pub fn permission(mut self, name: &str) -> Self {
        self.permissions.push(name.to_owned());
        self
    }
}

/// A parsed package: its header and the concatenated file bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub header: Header,
    pub payload: Vec<u8>,
}

impl Package {
    /// Builds a package from a manifest and named files. The manifest is stored as the first
    /// file, and the signature is taken over the declared key and the whole payload.
    #[must_use]
    pub fn new(manifest: &Manifest, files: Vec<(String, Vec<u8>)>) -> Self {
        let mut all = Vec::with_capacity(files.len() + 1);
        all.push((MANIFEST_FILE.to_owned(), manifest_text(manifest).into_bytes()));
        all.extend(files);

        let mut payload = Vec::new();
        let mut entries = Vec::with_capacity(all.len());
        for (name, bytes) in all {
            entries.push(FileEntry {
                name,
                size: bytes.len() as u32,
                hash: crate::serialization::hash(&bytes),
            });
            payload.extend_from_slice(&bytes);
        }

        Self {
            header: Header {
                format: FORMAT_VERSION,
                abi: manifest.abi,
                signature: signature_of(&manifest.key, &payload),
                id: manifest.id.clone(),
                version: manifest.version.clone(),
                files: entries,
            },
            payload,
        }
    }

    /// The bytes of one file, by the name the header recorded.
    #[must_use]
    pub fn file(&self, name: &str) -> Option<&[u8]> {
        let mut offset = 0usize;
        for entry in &self.header.files {
            let size = entry.size as usize;
            if entry.name == name {
                return self.payload.get(offset..offset + size);
            }
            offset += size;
        }
        None
    }

    /// The recorded fingerprint.
    #[must_use]
    pub fn signature(&self) -> u64 {
        self.header.signature
    }
}

/// A summary of a package, for a log or a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspect {
    pub id: String,
    pub name: String,
    pub version: String,
    pub format: u32,
    pub abi: u32,
    pub signature: u64,
    pub trust: TrustLevel,
    pub files: usize,
    pub payload: usize,
    pub size: usize,
}

/// What a trusted-fingerprint comparison answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureCheck {
    /// The recomputed fingerprint is one the caller trusts.
    Matched { fingerprint: u64 },
    /// The recorded fingerprint does not match the key and payload.
    Mismatched { recorded: u64, computed: u64 },
    /// The fingerprint is sound but no trusted entry names it.
    UnknownSigner { fingerprint: u64 },
}

/// A package's security posture, gathered in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityReport {
    pub id: String,
    pub trust: TrustLevel,
    pub abi_ok: bool,
    pub integrity_ok: bool,
    pub signature: SignatureCheck,
    pub permissions: Vec<String>,
    pub dependencies: Vec<String>,
}

/// The FNV-1a hash of the declared key then the payload: the fingerprint a package records.
#[must_use]
pub fn signature_of(key: &str, payload: &[u8]) -> u64 {
    let mut bytes = Vec::with_capacity(key.len() + payload.len());
    bytes.extend_from_slice(key.as_bytes());
    bytes.extend_from_slice(payload);
    crate::serialization::hash(&bytes)
}

/// The major part of an ABI number. This loader has no minor part, so it is the number.
#[must_use]
pub fn abi_major(abi: u32) -> u32 {
    abi
}

/// Whether an ABI number is this build's major. The rule is same major, and the whole number
/// is the major.
pub fn check_abi(abi: u32) -> Result<(), PackageError> {
    if abi_major(abi) == ABI_MAJOR {
        Ok(())
    } else {
        Err(PackageError::AbiMismatch {
            found: abi,
            supported: ABI_MAJOR,
        })
    }
}

/// Writes a package to its bytes.
#[must_use]
pub fn pack(package: &Package) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, package.header.format);
    put_u32(&mut out, package.header.abi);
    put_u64(&mut out, package.header.signature);
    put_str(&mut out, &package.header.id);
    put_str(&mut out, &package.header.version);
    put_u32(&mut out, package.header.files.len() as u32);
    for entry in &package.header.files {
        put_str(&mut out, &entry.name);
        put_u32(&mut out, entry.size);
        put_u64(&mut out, entry.hash);
    }
    out.extend_from_slice(&package.payload);
    out
}

/// Reads a package back. Refuses a bad magic, a truncated one, a future format, another ABI
/// and a file whose bytes no longer hash to what the header recorded.
pub fn open(bytes: &[u8]) -> Result<Package, PackageError> {
    if bytes.len() < MAGIC.len() {
        return Err(PackageError::Truncated {
            missing: "the magic".to_owned(),
        });
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(PackageError::BadMagic {
            found: String::from_utf8_lossy(&bytes[..MAGIC.len()]).into_owned(),
        });
    }

    let mut reader = Reader::new(&bytes[MAGIC.len()..]);
    let format = reader.u32("the format version")?;
    if format > FORMAT_VERSION {
        return Err(PackageError::UnsupportedFormat {
            found: format,
            supported: FORMAT_VERSION,
        });
    }
    let abi = reader.u32("the loader ABI")?;
    check_abi(abi)?;
    let signature = reader.u64("the signature")?;
    let id = reader.string("the mod id")?;
    let version = reader.string("the mod version")?;
    let count = reader.u32("the file count")? as usize;
    if count > MAX_FILES {
        return Err(PackageError::Malformed(format!(
            "{count} files is over the {MAX_FILES} limit"
        )));
    }

    let mut files = Vec::with_capacity(count);
    let mut total = 0usize;
    for index in 0..count {
        let name = reader.string(&format!("file {index}'s name"))?;
        let size = reader.u32(&format!("file {index}'s size"))? as usize;
        let hash = reader.u64(&format!("file {index}'s hash"))?;
        total += size;
        files.push(FileEntry {
            name,
            size: size as u32,
            hash,
        });
    }

    let payload = reader.rest();
    if payload.len() < total {
        return Err(PackageError::Truncated {
            missing: format!("{} payload bytes", total - payload.len()),
        });
    }
    if payload.len() > total {
        return Err(PackageError::Trailing {
            extra: payload.len() - total,
        });
    }

    let package = Package {
        header: Header {
            format,
            abi,
            signature,
            id,
            version,
            files,
        },
        payload: payload.to_vec(),
    };
    check_integrity(&package)?;
    Ok(package)
}

/// Recomputes every file's hash against the header, naming the first that does not match.
pub fn validate_archive(package: &Package) -> Result<(), PackageError> {
    check_integrity(package)
}

/// The file names a package carries, in order.
#[must_use]
pub fn list_contents(package: &Package) -> Vec<String> {
    package
        .header
        .files
        .iter()
        .map(|entry| entry.name.clone())
        .collect()
}

/// One file's bytes, by name.
#[must_use]
pub fn extract(package: &Package, name: &str) -> Option<Vec<u8>> {
    package.file(name).map(<[u8]>::to_vec)
}

/// Describes a package for a log or a listing.
pub fn inspect(package: &Package) -> Result<Inspect, PackageError> {
    let manifest = read_manifest(package)?;
    Ok(Inspect {
        id: package.header.id.clone(),
        name: manifest.name,
        version: package.header.version.clone(),
        format: package.header.format,
        abi: package.header.abi,
        signature: package.header.signature,
        trust: manifest.trust,
        files: package.header.files.len(),
        payload: package.payload.len(),
        size: get_size(package),
    })
}

/// The whole package's size in bytes, header and payload.
#[must_use]
pub fn get_size(package: &Package) -> usize {
    pack(package).len()
}

/// The FNV-1a hash of the whole serialized package.
#[must_use]
pub fn calculate_checksum(package: &Package) -> u64 {
    crate::serialization::hash(&pack(package))
}

/// Reads the manifest a package carries.
pub fn read_manifest(package: &Package) -> Result<Manifest, PackageError> {
    let bytes = package.file(MANIFEST_FILE).ok_or_else(|| {
        PackageError::Malformed(format!("the package has no {MANIFEST_FILE} file"))
    })?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PackageError::Malformed(format!("{MANIFEST_FILE} is not UTF-8")))?;
    manifest_from_text(text)
        .ok_or_else(|| PackageError::Malformed(format!("{MANIFEST_FILE} has no usable id and version")))
}

/// What is wrong with a manifest, as a list of sentences. Empty when it is usable.
#[must_use]
pub fn validate_manifest(manifest: &Manifest) -> Vec<String> {
    let mut problems = Vec::new();
    if !usable_id(&manifest.id) {
        problems.push(format!("'{}' is not a usable mod id", manifest.id));
    }
    if manifest.name.trim().is_empty() {
        problems.push(format!("'{}' has no name", manifest.id));
    }
    if !Version::parse(&manifest.version).is_numeric() {
        problems.push(format!("'{}' has no usable version", manifest.version));
    }
    if let Err(error) = check_abi(manifest.abi) {
        problems.push(error.to_string());
    }
    for (label, seen) in [
        ("depends on", &manifest.dependencies),
        ("asks for", &manifest.permissions),
    ] {
        for (index, value) in seen.iter().enumerate() {
            if seen[..index].contains(value) {
                problems.push(format!("'{}' {label} {value} twice", manifest.id));
            }
        }
    }
    problems
}

/// The ids a package declares it needs.
pub fn get_dependencies(package: &Package) -> Result<Vec<String>, PackageError> {
    Ok(read_manifest(package)?.dependencies)
}

/// The permission names a package declares.
pub fn get_permissions(package: &Package) -> Result<Vec<String>, PackageError> {
    Ok(read_manifest(package)?.permissions)
}

/// The trust level a package declares for itself.
pub fn get_trust_status(package: &Package) -> Result<TrustLevel, PackageError> {
    Ok(read_manifest(package)?.trust)
}

/// Compares a package's fingerprint against a caller's trusted list.
pub fn verify_signature(
    package: &Package,
    trusted: &[u64],
) -> Result<SignatureCheck, PackageError> {
    let manifest = read_manifest(package)?;
    let computed = signature_of(&manifest.key, &package.payload);
    if computed != package.header.signature {
        return Ok(SignatureCheck::Mismatched {
            recorded: package.header.signature,
            computed,
        });
    }
    if trusted.contains(&computed) {
        Ok(SignatureCheck::Matched {
            fingerprint: computed,
        })
    } else {
        Ok(SignatureCheck::UnknownSigner {
            fingerprint: computed,
        })
    }
}

/// Everything a caller can check about a package, in one report.
pub fn get_security_report(
    package: &Package,
    trusted: &[u64],
) -> Result<SecurityReport, PackageError> {
    let manifest = read_manifest(package)?;
    Ok(SecurityReport {
        id: package.header.id.clone(),
        trust: manifest.trust,
        abi_ok: check_abi(package.header.abi).is_ok(),
        integrity_ok: validate_archive(package).is_ok(),
        signature: verify_signature(package, trusted)?,
        permissions: manifest.permissions,
        dependencies: manifest.dependencies,
    })
}

/// Why a question about the client's own package format has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "read_client_package" => {
            "reading the client's own mod package would mean decoding a byte layout only the \
             client defines, and this host cannot see it"
                .to_owned()
        }
        "write_client_package" => {
            "writing the client's own mod package would hand a mod a byte layout only the \
             client defines, which this host cannot produce"
                .to_owned()
        }
        _ => format!("{what} is not a package format this host knows"),
    }
}

/// What this host cannot reach about the client's own package format.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["read_client_package", "write_client_package"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> PackageError {
    let why = why_unknown(what);
    crate::warn!("a mod asked this host for the client's package format: {why}");
    PackageError::Refused(why)
}

/// Reading the client's own package format. [`PackageError::Refused`], with [`why_unknown`].
pub fn read_client_package(_bytes: &[u8]) -> Result<Package, PackageError> {
    Err(refused("read_client_package"))
}

/// Writing the client's own package format. [`PackageError::Refused`], with [`why_unknown`].
pub fn write_client_package(_package: &Package) -> Result<Vec<u8>, PackageError> {
    Err(refused("write_client_package"))
}

/// The id-version index of every package this host has registered.
#[derive(Default)]
struct Index {
    by_id: BTreeMap<String, BTreeMap<String, Package>>,
}

fn index() -> &'static Mutex<Index> {
    static INDEX: OnceLock<Mutex<Index>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(Index::default()))
}

fn with<T>(body: impl FnOnce(&mut Index) -> T) -> T {
    let mut guard = index().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a package in the index, refusing an (id, version) already held.
pub fn register(package: Package) -> Result<(), PackageError> {
    with(|index| {
        let id = package.header.id.clone();
        let version = package.header.version.clone();
        let versions = index.by_id.entry(id.clone()).or_default();
        if versions.contains_key(&version) {
            crate::warn!("a mod tried to register {id} {version} twice");
            return Err(PackageError::Duplicate { id, version });
        }
        versions.insert(version, package);
        Ok(())
    })
}

/// Drops one (id, version) from the index.
pub fn unregister(id: &str, version: &str) -> bool {
    with(|index| {
        index
            .by_id
            .get_mut(id)
            .is_some_and(|versions| versions.remove(version).is_some())
    })
}

/// The versions held for an id, oldest first by [`crate::version`] ordering.
#[must_use]
pub fn versions(id: &str) -> Vec<String> {
    with(|index| {
        index
            .by_id
            .get(id)
            .map(|versions| {
                let mut keys: Vec<String> = versions.keys().cloned().collect();
                keys.sort_by_key(|version| Version::parse(version));
                keys
            })
            .unwrap_or_default()
    })
}

/// The newest package held for an id, by [`crate::version`] ordering.
#[must_use]
pub fn newest(id: &str) -> Option<Package> {
    with(|index| {
        index.by_id.get(id).and_then(|versions| {
            versions
                .values()
                .max_by(|left, right| {
                    Version::parse(&left.header.version).cmp(&Version::parse(&right.header.version))
                })
                .cloned()
        })
    })
}

fn check_integrity(package: &Package) -> Result<(), PackageError> {
    let mut offset = 0usize;
    for entry in &package.header.files {
        let size = entry.size as usize;
        let Some(bytes) = package.payload.get(offset..offset + size) else {
            return Err(PackageError::Truncated {
                missing: format!("file '{}'", entry.name),
            });
        };
        let got = crate::serialization::hash(bytes);
        if got != entry.hash {
            return Err(PackageError::Corrupt {
                file: entry.name.clone(),
                want: entry.hash,
                got,
            });
        }
        offset += size;
    }
    Ok(())
}

fn manifest_text(manifest: &Manifest) -> String {
    let mut text = format!(
        "id={}\nname={}\nversion={}\nabi={}\nkey={}\ntrust={}\n",
        manifest.id, manifest.name, manifest.version, manifest.abi, manifest.key, manifest.trust
    );
    for dependency in &manifest.dependencies {
        text.push_str(&format!("depends={dependency}\n"));
    }
    for permission in &manifest.permissions {
        text.push_str(&format!("permission={permission}\n"));
    }
    text
}

fn manifest_from_text(text: &str) -> Option<Manifest> {
    let mut id = None;
    let mut name = String::new();
    let mut version = None;
    let mut abi = ABI_MAJOR;
    let mut key = String::new();
    let mut trust = TrustLevel::default();
    let mut dependencies = Vec::new();
    let mut permissions = Vec::new();
    for line in text.lines() {
        let Some((field, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match field.trim() {
            "id" => id = Some(value.to_owned()),
            "name" => name = value.to_owned(),
            "version" => version = Some(value.to_owned()),
            "abi" => abi = value.parse().ok()?,
            "key" => key = value.to_owned(),
            "trust" => trust = value.parse().ok()?,
            "depends" => dependencies.push(value.to_owned()),
            "permission" => permissions.push(value.to_owned()),
            _ => {}
        }
    }
    Some(Manifest {
        id: id?,
        name,
        version: version?,
        abi,
        key,
        trust,
        dependencies,
        permissions,
    })
}

fn usable_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.'))
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    put_u32(out, text.len() as u32);
    out.extend_from_slice(text.as_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize, missing: &str) -> Result<&'a [u8], PackageError> {
        if self.bytes.len() - self.at < count {
            return Err(PackageError::Truncated {
                missing: missing.to_owned(),
            });
        }
        let slice = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(slice)
    }

    fn u32(&mut self, missing: &str) -> Result<u32, PackageError> {
        let bytes = self.take(4, missing)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self, missing: &str) -> Result<u64, PackageError> {
        let bytes = self.take(8, missing)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn string(&mut self, missing: &str) -> Result<String, PackageError> {
        let length = self.u32(missing)? as usize;
        let bytes = self.take(length, missing)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| PackageError::Malformed(format!("{missing} is not UTF-8")))
    }

    fn rest(&mut self) -> &'a [u8] {
        let rest = &self.bytes[self.at..];
        self.at = self.bytes.len();
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn sample() -> Package {
        let manifest = Manifest::new("hello", "Hello", "1.2.0")
            .key("hello-key")
            .trust(TrustLevel::Normal)
            .depends_on("core_lib")
            .permission("read_files");
        Package::new(
            &manifest,
            vec![("greeting.txt".to_owned(), b"hi".to_vec())],
        )
    }

    #[test]
    fn a_package_round_trips_exactly() {
        let package = sample();
        let bytes = pack(&package);
        assert_eq!(bytes.len(), 206);
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(get_size(&package), 206);

        let opened = open(&bytes).expect("a whole package");
        assert_eq!(opened, package);
        assert_eq!(pack(&opened), bytes, "the bytes are identical");
        assert_eq!(list_contents(&opened), vec!["manifest", "greeting.txt"]);
        assert_eq!(opened.file("greeting.txt"), Some(&b"hi"[..]));
        assert_eq!(extract(&opened, "greeting.txt").as_deref(), Some(&b"hi"[..]));
        assert_eq!(extract(&opened, "gone.txt"), None);
        assert_eq!(calculate_checksum(&opened), crate::serialization::hash(&bytes));
    }

    #[test]
    fn the_manifest_reads_back_with_its_declarations() {
        let opened = open(&pack(&sample())).expect("a whole package");
        let manifest = read_manifest(&opened).expect("a manifest");
        assert_eq!(manifest.id, "hello");
        assert_eq!(manifest.name, "Hello");
        assert_eq!(manifest.version, "1.2.0");
        assert_eq!(manifest.abi, ABI_MAJOR);
        assert_eq!(manifest.key, "hello-key");
        assert_eq!(manifest.trust, TrustLevel::Normal);
        assert_eq!(get_dependencies(&opened).unwrap(), vec!["core_lib"]);
        assert_eq!(get_permissions(&opened).unwrap(), vec!["read_files"]);
        assert_eq!(get_trust_status(&opened).unwrap(), TrustLevel::Normal);
        assert!(validate_manifest(&manifest).is_empty());

        let inspect = inspect(&opened).expect("a manifest");
        assert_eq!(inspect.files, 2);
        assert_eq!(inspect.size, 206);
        assert_eq!(inspect.id, "hello");

        let bad = Manifest::new("", "", "nightly");
        let problems = validate_manifest(&bad);
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn a_bad_truncated_or_future_package_is_refused() {
        let mut bytes = pack(&sample());
        bytes[0] = b'X';
        assert_eq!(
            open(&bytes),
            Err(PackageError::BadMagic {
                found: "XINNAPKG".to_owned()
            })
        );
        assert_eq!(
            open(&bytes[..5]),
            Err(PackageError::Truncated {
                missing: "the magic".to_owned()
            })
        );

        let bytes = pack(&sample());
        assert_eq!(
            open(&bytes[..12]),
            Err(PackageError::Truncated {
                missing: "the loader ABI".to_owned()
            })
        );
        assert_eq!(
            open(&bytes[..bytes.len() - 3]),
            Err(PackageError::Truncated {
                missing: "3 payload bytes".to_owned()
            })
        );

        let mut future = pack(&sample());
        future[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert_eq!(
            open(&future),
            Err(PackageError::UnsupportedFormat {
                found: 2,
                supported: 1
            })
        );
    }

    #[test]
    fn a_tampered_file_is_corrupt_and_named() {
        let package = sample();
        let mut bytes = pack(&package);
        let header_len = bytes.len() - package.payload.len();
        bytes[header_len + 1] ^= 0xff;
        match open(&bytes) {
            Err(PackageError::Corrupt { file, .. }) => assert_eq!(file, "manifest"),
            other => panic!("expected corrupt, got {other:?}"),
        }
    }

    #[test]
    fn the_abi_rule_takes_this_build_and_refuses_another() {
        assert!(check_abi(ABI_MAJOR).is_ok());
        assert_eq!(abi_major(ABI_MAJOR), ABI_MAJOR);
        assert_eq!(
            check_abi(ABI_MAJOR + 1),
            Err(PackageError::AbiMismatch {
                found: ABI_MAJOR + 1,
                supported: ABI_MAJOR
            })
        );

        let other = Manifest::new("hello", "Hello", "1.2.0").abi(ABI_MAJOR + 1);
        let package = Package::new(&other, Vec::new());
        assert_eq!(
            open(&pack(&package)),
            Err(PackageError::AbiMismatch {
                found: ABI_MAJOR + 1,
                supported: ABI_MAJOR
            })
        );
    }

    #[test]
    fn a_signature_matches_mismatches_and_is_unknown() {
        let package = sample();
        let fingerprint = signature_of("hello-key", &package.payload);
        assert_eq!(
            verify_signature(&package, &[fingerprint]).unwrap(),
            SignatureCheck::Matched { fingerprint }
        );
        assert_eq!(
            verify_signature(&package, &[]).unwrap(),
            SignatureCheck::UnknownSigner { fingerprint }
        );

        let mut tampered = package.clone();
        let last = tampered.payload.len() - 1;
        tampered.payload[last] ^= 0xff;
        assert_eq!(
            verify_signature(&tampered, &[fingerprint]).unwrap(),
            SignatureCheck::Mismatched {
                recorded: fingerprint,
                computed: signature_of("hello-key", &tampered.payload)
            }
        );

        let report = get_security_report(&package, &[fingerprint]).unwrap();
        assert!(report.abi_ok && report.integrity_ok);
        assert_eq!(report.signature, SignatureCheck::Matched { fingerprint });
        assert_eq!(report.permissions, vec!["read_files"]);
    }

    #[test]
    fn the_index_lists_versions_and_picks_the_newest() {
        let _serial = serial();
        with(|index| index.by_id.clear());

        for version in ["1.2.0", "1.10.0", "1.2.1"] {
            let manifest = Manifest::new("hello", "Hello", version);
            assert!(register(Package::new(&manifest, Vec::new())).is_ok());
        }
        assert_eq!(versions("hello"), vec!["1.2.0", "1.2.1", "1.10.0"]);
        assert_eq!(
            newest("hello").map(|package| package.header.version),
            Some("1.10.0".to_owned())
        );
        assert!(versions("absent").is_empty());
        assert_eq!(newest("absent"), None);

        let duplicate = Package::new(&Manifest::new("hello", "Hello", "1.2.0"), Vec::new());
        assert_eq!(
            register(duplicate),
            Err(PackageError::Duplicate {
                id: "hello".to_owned(),
                version: "1.2.0".to_owned()
            })
        );
        assert!(unregister("hello", "1.2.1"));
        assert_eq!(versions("hello"), vec!["1.2.0", "1.10.0"]);
    }

    #[test]
    fn the_client_package_format_is_refused() {
        assert!(why_unknown("read_client_package").contains("client's own mod package"));
        assert_eq!(refusals().len(), 2);
        let refused = read_client_package(b"anything");
        assert!(matches!(refused, Err(PackageError::Refused(_))));
        assert!(matches!(
            write_client_package(&sample()),
            Err(PackageError::Refused(_))
        ));
        assert!(why_unknown("nonsense").contains("nonsense"));
    }
}
