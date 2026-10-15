//! Security, integrity, and trust: the loader's own checks, and the client's it cannot make.
//!
//! Three separate ideas live here, kept separate on purpose. **Integrity** is a manifest of
//! `(path, size, hash)` that says what a tree or a set of bytes looked like when it was
//! fingerprinted; verifying it names every file that changed, went missing or appeared.
//! **Trust** is a state per subject — a mod id or a file — that the user sets, and a decision
//! (allow, warn, deny) derived from it. **Signing** is a fingerprint comparison and nothing
//! more: a manifest carries a payload's fingerprint and a signer id, and verification checks
//! the fingerprint against the payload and the signer against the trust list. It notices a
//! changed payload; it does not prove who built one and is **not cryptography**.
//!
//! The hashes come from [`crate::serialization`], FNV-1a 64, quick and not cryptographic.
//! Capabilities are resolved to the permissions they require through [`crate::permissions`],
//! so a mod that declares a capability without its permission is told exactly which
//! permission is missing.
//!
//! What this host cannot do is the client's own side of security: the checks the client runs
//! on itself and the trust store it keeps. Those are refused with their reasons by
//! [`why_unknown`] and [`refusals`].
//!
//! ```
//! use cinnaloader::security::{self, Decision, IntegrityManifest};
//!
//! let payload = b"hello";
//! let manifest = IntegrityManifest::from_bytes("greeting.txt", payload);
//! assert!(manifest.verify_bytes("greeting.txt", payload).is_clean());
//! assert_eq!(manifest.verify_bytes("greeting.txt", b"other").mismatches().len(), 1);
//!
//! let signed = security::sign("hello-team", payload);
//! security::register_trusted_signer("hello-team");
//! assert!(security::verify_signature(&signed, payload).matched());
//!
//! security::trust("hello-mod");
//! assert_eq!(security::decide("hello-mod"), Decision::Allow);
//! security::block("rival-mod");
//! assert_eq!(security::decide("rival-mod"), Decision::Deny);
//! assert!(security::audit_count() >= 2);
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::capability::Capability;
use crate::modinfo::{ModManifest, Permission, ALL_PERMISSIONS};
use crate::serialization::{self, Map, Value};

/// How much the user trusts one subject. A mod id and a file are both subjects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TrustState {
    /// Never seen before.
    #[default]
    Unknown,
    /// Seen, and not vouched for.
    Known,
    /// Vouched for by the user or a trusted signer.
    Trusted,
    /// The user blocked it.
    Blocked,
}

impl TrustState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Known => "known",
            Self::Trusted => "trusted",
            Self::Blocked => "blocked",
        }
    }

    /// What this state does to a subject, stated for the user reading it back.
    #[must_use]
    pub fn rule(self) -> &'static str {
        match self {
            Self::Unknown => {
                "never seen before: warn and keep going, because refusing every first run \
                 would refuse every mod"
            }
            Self::Known => "seen and not vouched for: warn and keep going, with the user watching",
            Self::Trusted => "vouched for by the user or a trusted signer: allow",
            Self::Blocked => "the user blocked it: deny",
        }
    }
}

impl fmt::Display for TrustState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What the loader does about a subject, derived from its trust state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Decision {
    Allow,
    Warn,
    Deny,
}

impl Decision {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Warn => "warn",
            Self::Deny => "deny",
        }
    }
}

impl fmt::Display for Decision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The trust rule for a state: allow, warn or deny, one per state, no exceptions.
#[must_use]
pub fn rule(state: TrustState) -> &'static str {
    state.rule()
}

fn decision_for(state: TrustState) -> Decision {
    match state {
        TrustState::Trusted => Decision::Allow,
        TrustState::Blocked => Decision::Deny,
        TrustState::Unknown | TrustState::Known => Decision::Warn,
    }
}

/// One file's size and content hash, taken as the tree was fingerprinted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityEntry {
    pub path: String,
    pub size: u64,
    pub hash: u64,
}

/// A manifest of what a tree or a set of bytes looked like: `(path, size, hash)` per entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrityManifest {
    pub entries: Vec<IntegrityEntry>,
}

impl IntegrityManifest {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one entry, replacing an earlier one for the same path.
    pub fn push(&mut self, path: impl Into<String>, size: u64, hash: u64) {
        let path = path.into();
        self.entries.retain(|entry| entry.path != path);
        self.entries.push(IntegrityEntry { path, size, hash });
        self.entries.sort_by(|left, right| left.path.cmp(&right.path));
    }

    /// The manifest for a single set of bytes under one path.
    #[must_use]
    pub fn from_bytes(path: impl Into<String>, bytes: &[u8]) -> Self {
        let mut manifest = Self::new();
        manifest.push(path, bytes.len() as u64, serialization::hash(bytes));
        manifest
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A fingerprint over every entry, for a signed manifest to carry.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        let mut all = Vec::new();
        for entry in &self.entries {
            all.extend_from_slice(entry.path.as_bytes());
            all.extend_from_slice(&entry.size.to_le_bytes());
            all.extend_from_slice(&entry.hash.to_le_bytes());
        }
        serialization::hash(&all)
    }

    /// Verifies one path's bytes against the entry recorded for it.
    #[must_use]
    pub fn verify_bytes(&self, path: &str, bytes: &[u8]) -> IntegrityReport {
        let mut report = IntegrityReport::default();
        let size = bytes.len() as u64;
        let hash = serialization::hash(bytes);
        match self.entries.iter().find(|entry| entry.path == path) {
            Some(entry) => {
                report.checked = 1;
                if entry.size != size || entry.hash != hash {
                    report
                        .mismatches
                        .push(Mismatch::changed(path, entry.size, entry.hash, size, hash));
                }
            }
            None => {
                report.mismatches.push(Mismatch::extra(path, size, hash));
            }
        }
        flag_mismatches(&report);
        report
    }

    /// Verifies a whole directory tree against the manifest, in path order.
    #[must_use]
    pub fn verify_tree(&self, source: &str) -> IntegrityReport {
        let found = fingerprint_tree(source);
        let mut on_disk: BTreeMap<&str, (u64, u64)> = found
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), (entry.size, entry.hash)))
            .collect();
        let mut report = IntegrityReport::default();
        for entry in &self.entries {
            report.checked += 1;
            match on_disk.remove(entry.path.as_str()) {
                Some((size, hash)) if size == entry.size && hash == entry.hash => {}
                Some((size, hash)) => report.mismatches.push(Mismatch::changed(
                    &entry.path,
                    entry.size,
                    entry.hash,
                    size,
                    hash,
                )),
                None => report.mismatches.push(Mismatch::missing(
                    &entry.path,
                    entry.size,
                    entry.hash,
                )),
            }
        }
        for (path, (size, hash)) in on_disk {
            report.checked += 1;
            report.mismatches.push(Mismatch::extra(path, size, hash));
        }
        flag_mismatches(&report);
        report
    }
}

/// Why one path disagreed with the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MismatchKind {
    /// The manifest lists it and the tree does not have it.
    Missing,
    /// The tree has it and the manifest does not list it.
    Extra,
    /// Both have it and the size or hash differs.
    Changed,
}

/// One path that failed verification, with the hashes on each side where they exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub path: String,
    pub kind: MismatchKind,
    /// `(size, hash)` the manifest recorded, when it has an entry for the path.
    pub expected: Option<(u64, u64)>,
    /// `(size, hash)` found, when the tree or bytes had the path.
    pub found: Option<(u64, u64)>,
}

impl Mismatch {
    fn changed(path: &str, expected_size: u64, expected_hash: u64, size: u64, hash: u64) -> Self {
        Self {
            path: path.to_owned(),
            kind: MismatchKind::Changed,
            expected: Some((expected_size, expected_hash)),
            found: Some((size, hash)),
        }
    }

    fn missing(path: &str, expected_size: u64, expected_hash: u64) -> Self {
        Self {
            path: path.to_owned(),
            kind: MismatchKind::Missing,
            expected: Some((expected_size, expected_hash)),
            found: None,
        }
    }

    fn extra(path: &str, size: u64, hash: u64) -> Self {
        Self {
            path: path.to_owned(),
            kind: MismatchKind::Extra,
            expected: None,
            found: Some((size, hash)),
        }
    }
}

impl fmt::Display for Mismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.kind, self.expected, self.found) {
            (MismatchKind::Changed, Some((es, eh)), Some((fs, fh))) => write!(
                formatter,
                "changed {}: expected {es}/{eh:016x}, found {fs}/{fh:016x}",
                self.path
            ),
            (MismatchKind::Missing, Some((es, eh)), _) => {
                write!(formatter, "missing {}: expected {es}/{eh:016x}", self.path)
            }
            (MismatchKind::Extra, _, Some((fs, fh))) => {
                write!(formatter, "extra {}: found {fs}/{fh:016x}", self.path)
            }
            _ => write!(formatter, "{} disagrees with the manifest", self.path),
        }
    }
}

/// What one verification checked and what it found wrong.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    pub checked: usize,
    pub mismatches: Vec<Mismatch>,
}

impl IntegrityReport {
    /// How many paths were looked at, manifest entries and extra files together.
    #[must_use]
    pub fn checked(&self) -> usize {
        self.checked
    }

    #[must_use]
    pub fn mismatches(&self) -> &[Mismatch] {
        &self.mismatches
    }

    #[must_use]
    pub fn mismatch(&self, path: &str) -> Option<&Mismatch> {
        self.mismatches.iter().find(|entry| entry.path == path)
    }

    /// Whether nothing disagreed with the manifest.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// A manifest that carries a payload's fingerprint and the id of the signer, compared and not
/// cryptographically checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedManifest {
    pub signer: String,
    pub fingerprint: u64,
}

impl SignedManifest {
    #[must_use]
    pub fn new(signer: impl Into<String>, fingerprint: u64) -> Self {
        Self {
            signer: signer.into(),
            fingerprint,
        }
    }
}

/// The three things a signature check can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureOutcome {
    /// The fingerprint matches the payload and the signer is trusted.
    Matched,
    /// The payload's fingerprint is not the recorded one.
    Mismatched { recorded: u64, computed: u64 },
    /// The fingerprint matches and the signer is not in the trust list.
    UnknownSigner { signer: String },
}

impl SignatureOutcome {
    #[must_use]
    pub fn matched(&self) -> bool {
        matches!(self, Self::Matched)
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::Mismatched { .. } => "mismatched",
            Self::UnknownSigner { .. } => "unknown_signer",
        }
    }
}

impl fmt::Display for SignatureOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Matched => formatter.write_str("matched"),
            Self::Mismatched { recorded, computed } => write!(
                formatter,
                "mismatched: recorded {recorded:016x}, computed {computed:016x}"
            ),
            Self::UnknownSigner { signer } => write!(formatter, "unknown signer {signer}"),
        }
    }
}

/// One thing that failed verification or was blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub subject: String,
    pub reason: String,
}

/// Everything that failed verification or was blocked, as text or JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TamperReport {
    pub failures: Vec<Failure>,
}

impl TamperReport {
    #[must_use]
    pub fn total(&self) -> usize {
        self.failures.len()
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }

    fn value(&self) -> Value {
        let mut map = Map::new();
        map.insert("total", Value::Int(self.failures.len() as i64));
        let list = self
            .failures
            .iter()
            .map(|failure| {
                let mut entry = Map::new();
                entry.insert("subject", Value::Str(failure.subject.clone()));
                entry.insert("reason", Value::Str(failure.reason.clone()));
                Value::Map(entry)
            })
            .collect();
        map.insert("failures", Value::List(list));
        Value::Map(map)
    }

    #[must_use]
    pub fn text(&self) -> String {
        serialization::to_text(&self.value())
    }

    pub fn to_json(&self) -> Result<String, serialization::Error> {
        serialization::to_json(&self.value())
    }
}

/// One line of the security audit log: what was decided, for whom, why and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub action: String,
    pub subject: String,
    pub decision: Decision,
    pub reason: String,
    pub at_millis: u64,
}

impl fmt::Display for AuditEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:>8}ms {:<5} {} {} ({})",
            self.at_millis,
            self.decision.as_str(),
            self.subject,
            self.action,
            self.reason
        )
    }
}

/// How many audit lines are kept.
pub const AUDIT_LIMIT: usize = 256;

/// How many tamper lines are kept.
pub const TAMPER_LIMIT: usize = 512;

#[derive(Default)]
struct Registry {
    trust: BTreeMap<String, TrustState>,
    signers: Vec<String>,
    audit: Vec<AuditEntry>,
    tamper: Vec<Failure>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn note(
    action: &str,
    subject: &str,
    decision: Decision,
    reason: impl Into<String>,
    at_millis: u64,
) {
    with(|registry| {
        if registry.audit.len() == AUDIT_LIMIT {
            registry.audit.remove(0);
        }
        registry.audit.push(AuditEntry {
            action: action.to_owned(),
            subject: subject.to_owned(),
            decision,
            reason: reason.into(),
            at_millis,
        });
    });
}

fn flag(subject: impl Into<String>, reason: impl Into<String>) {
    with(|registry| {
        if registry.tamper.len() == TAMPER_LIMIT {
            registry.tamper.remove(0);
        }
        registry.tamper.push(Failure {
            subject: subject.into(),
            reason: reason.into(),
        });
    });
}

fn flag_mismatches(report: &IntegrityReport) {
    for mismatch in &report.mismatches {
        flag(mismatch.path.clone(), mismatch.to_string());
    }
}

fn fingerprint_tree(source: &str) -> IntegrityManifest {
    let mut manifest = IntegrityManifest::new();
    let Ok(base) = crate::fs::resolve(source) else {
        return manifest;
    };
    if !base.is_dir() {
        return manifest;
    }
    for path in crate::fs::walk(source).unwrap_or_default() {
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(&base) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        manifest.push(
            relative.to_string_lossy().replace('\\', "/"),
            bytes.len() as u64,
            serialization::hash(&bytes),
        );
    }
    manifest
}

/// Walks a directory tree and fingerprints every file under it, in path order.
#[must_use]
pub fn get_mod_integrity(source: &str) -> IntegrityManifest {
    fingerprint_tree(source)
}

/// The fingerprint a manifest carries, over its entries.
#[must_use]
pub fn get_manifest_hash(manifest: &IntegrityManifest) -> u64 {
    manifest.fingerprint()
}

/// The hash of one file's contents, or `None` when it cannot be read.
#[must_use]
pub fn get_file_hash(path: &str) -> Option<u64> {
    crate::fs::read(path)
        .ok()
        .map(|bytes| serialization::hash(&bytes))
}

/// Whether a set of bytes hashes to the value a fingerprint recorded.
#[must_use]
pub fn verify_checksum(bytes: &[u8], expected: u64) -> bool {
    serialization::hash(bytes) == expected
}

/// Verifies a package's directory tree against its manifest.
#[must_use]
pub fn verify_package(manifest: &IntegrityManifest, source: &str) -> IntegrityReport {
    manifest.verify_tree(source)
}

/// The paths that no longer match an integrity manifest.
#[must_use]
pub fn detect_modified_files(manifest: &IntegrityManifest, source: &str) -> IntegrityReport {
    manifest.verify_tree(source)
}

/// Builds the manifest a signer would carry for a payload: its fingerprint and the signer.
#[must_use]
pub fn sign(signer: impl Into<String>, payload: &[u8]) -> SignedManifest {
    SignedManifest::new(signer, serialization::hash(payload))
}

/// Compares a manifest's fingerprint against the payload and its signer against the trust
/// list. A mismatch is reported before the signer, because a wrong payload is wrong whatever
/// signed it; matching bytes from an untrusted signer are an unknown signer, not a match.
/// This is a fingerprint comparison, **not cryptography**.
#[must_use]
pub fn verify_signature(manifest: &SignedManifest, payload: &[u8]) -> SignatureOutcome {
    let computed = serialization::hash(payload);
    if computed != manifest.fingerprint {
        return SignatureOutcome::Mismatched {
            recorded: manifest.fingerprint,
            computed,
        };
    }
    if !with(|registry| registry.signers.iter().any(|signer| signer == &manifest.signer)) {
        return SignatureOutcome::UnknownSigner {
            signer: manifest.signer.clone(),
        };
    }
    SignatureOutcome::Matched
}

/// Whether a payload both matches a manifest and was signed by someone trusted.
#[must_use]
pub fn is_package_trusted(manifest: &SignedManifest, payload: &[u8]) -> bool {
    verify_signature(manifest, payload).matched()
}

/// Adds a signer to the trust list. `false` when it was already there.
pub fn register_trusted_signer(signer: &str) -> bool {
    with(|registry| {
        if registry.signers.iter().any(|held| held == signer) {
            return false;
        }
        registry.signers.push(signer.to_owned());
        true
    })
}

/// Removes a signer from the trust list. `false` when it was not there.
pub fn unregister_trusted_signer(signer: &str) -> bool {
    with(|registry| {
        let before = registry.signers.len();
        registry.signers.retain(|held| held != signer);
        registry.signers.len() != before
    })
}

/// The signers this host trusts, in the order they were registered.
#[must_use]
pub fn trusted_signers() -> Vec<String> {
    with(|registry| registry.signers.clone())
}

/// The trust state in force for a subject.
#[must_use]
pub fn get_trust(subject: &str) -> TrustState {
    with(|registry| registry.trust.get(subject).copied().unwrap_or_default())
}

/// The same as [`get_trust`], under the wishlist's name.
#[must_use]
pub fn get_trust_level(subject: &str) -> TrustState {
    get_trust(subject)
}

/// Sets a subject's trust state and records the decision.
pub fn set_trust_level(subject: &str, state: TrustState) -> bool {
    with(|registry| {
        registry.trust.insert(subject.to_owned(), state);
    });
    note("policy", subject, decision_for(state), state.rule(), 0);
    true
}

/// The same as [`set_trust_level`], for a caller registering a policy by name.
pub fn register_policy(subject: &str, state: TrustState) -> bool {
    set_trust_level(subject, state)
}

/// Vouches for a subject.
pub fn trust(subject: &str) -> bool {
    set_trust_level(subject, TrustState::Trusted)
}

/// Marks a subject as seen but not vouched for.
pub fn distrust(subject: &str) -> bool {
    set_trust_level(subject, TrustState::Known)
}

/// Blocks a subject and records it as tamper.
pub fn block(subject: &str) -> bool {
    set_trust_level(subject, TrustState::Blocked);
    flag(subject, "blocked");
    true
}

/// Takes a subject back to seen but not vouched for.
pub fn unblock(subject: &str) -> bool {
    set_trust_level(subject, TrustState::Known)
}

/// The decision a subject's state earns, without recording it.
#[must_use]
pub fn decide(subject: &str) -> Decision {
    decision_for(get_trust(subject))
}

/// The same as [`decide`], recording the decision in the audit log with the caller's time.
pub fn evaluate_policy(subject: &str, at_millis: u64) -> Decision {
    let state = get_trust(subject);
    let decision = decision_for(state);
    note("evaluate", subject, decision, state.rule(), at_millis);
    decision
}

/// Every subject that has a trust state, with the decision it earns.
#[must_use]
pub fn get_policy_result() -> Vec<(String, Decision)> {
    with(|registry| {
        registry
            .trust
            .iter()
            .map(|(subject, state)| (subject.clone(), decision_for(*state)))
            .collect()
    })
}

/// Appends one audit line. The time is the caller's to supply.
pub fn record(action: &str, subject: &str, decision: Decision, reason: &str, at_millis: u64) {
    note(action, subject, decision, reason, at_millis);
}

/// The audit log, oldest first.
#[must_use]
pub fn get_audit_log() -> Vec<AuditEntry> {
    with(|registry| registry.audit.clone())
}

/// The audit lines about one subject.
#[must_use]
pub fn filter_by_subject(subject: &str) -> Vec<AuditEntry> {
    get_audit_log()
        .into_iter()
        .filter(|entry| entry.subject == subject)
        .collect()
}

/// How many audit lines are held.
#[must_use]
pub fn audit_count() -> usize {
    with(|registry| registry.audit.len())
}

/// Drops the audit log and answers how many lines went.
pub fn clear_audit_log() -> usize {
    with(|registry| {
        let count = registry.audit.len();
        registry.audit.clear();
        count
    })
}

/// The capabilities this mod declared without the permissions they require, each pair named.
#[must_use]
pub fn check_permissions() -> Vec<(Capability, Permission)> {
    missing_for(&crate::metadata::manifest())
}

fn missing_for(manifest: &ModManifest) -> Vec<(Capability, Permission)> {
    let mut missing = Vec::new();
    for capability in &manifest.capabilities {
        for permission in capability.requirements() {
            if !crate::permissions::has(permission.canonical()) {
                missing.push((*capability, *permission));
            }
        }
    }
    missing
}

/// Whether one capability's permissions are all held, or the first one that is not, named with
/// the capability that needs it.
pub fn check_capability(capability: Capability) -> Result<(), String> {
    for permission in capability.requirements() {
        if !crate::permissions::has(permission.canonical()) {
            return Err(format!(
                "capability {capability} needs the {permission} permission, which this mod \
                 does not hold"
            ));
        }
    }
    Ok(())
}

/// The permissions this mod asked for at runtime.
#[must_use]
pub fn get_requested_permissions() -> Vec<Permission> {
    crate::permissions::list_requested()
}

/// The permissions this mod now holds.
#[must_use]
pub fn get_granted_permissions() -> Vec<Permission> {
    ALL_PERMISSIONS
        .iter()
        .copied()
        .filter(|permission| crate::permissions::has(permission.canonical()))
        .collect()
}

/// Everything that failed verification or was blocked.
#[must_use]
pub fn get_security_report() -> TamperReport {
    with(|registry| TamperReport {
        failures: registry.tamper.clone(),
    })
}

/// Drops the tamper log and answers how many lines went.
pub fn clear_security_report() -> usize {
    with(|registry| {
        let count = registry.tamper.len();
        registry.tamper.clear();
        count
    })
}

/// Whether a mod runs inside a sandbox. It does not: a mod is native code loaded into this
/// process, so there is nothing between it and the client.
#[must_use]
pub fn is_mod_sandboxed() -> bool {
    crate::warn!(
        "a mod asked whether it is sandboxed: {}",
        why_unknown("sandbox")
    );
    false
}

/// Why a question about the client's own security has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client_integrity" => {
            "the client checks its own files and this host cannot read that state; it can only \
             verify the files a mod ships"
                .to_owned()
        }
        "client_trust_store" => {
            "the client keeps its own trust store and this host cannot read or write it"
                .to_owned()
        }
        "client_security" => {
            "the client's own security checks run inside the client, out of this host's reach"
                .to_owned()
        }
        "sandbox" => {
            "a mod is native code in the client's own process, so there is no sandbox to be \
             inside"
                .to_owned()
        }
        _ => format!("{what} is not something this host can read about the client"),
    }
}

/// What this host cannot reach about the client's own security.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client_integrity", "client_trust_store", "client_security"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own integrity state. `None`, with [`why_unknown`].
#[must_use]
pub fn get_client_integrity() -> Option<IntegrityManifest> {
    crate::warn!(
        "a mod asked for the client's own integrity state: {}",
        why_unknown("client_integrity")
    );
    None
}

/// The client's own trust store. `None`, with [`why_unknown`].
#[must_use]
pub fn get_client_trust_store() -> Option<Vec<String>> {
    crate::warn!(
        "a mod asked for the client's own trust store: {}",
        why_unknown("client_trust_store")
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|registry| *registry = Registry::default());
    }

    fn root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cinnaloader-security-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());
        root
    }

    #[test]
    fn a_manifest_over_a_tree_names_a_changed_missing_and_extra_file() {
        let _serial = serial();
        fresh();
        let root = root("tree");
        crate::fs::write_text("tree/a.txt", "alive").unwrap();
        crate::fs::write_text("tree/b.txt", "one\n").unwrap();
        let manifest = get_mod_integrity("tree");
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest.entries[0].path, "a.txt");
        assert_eq!(manifest.entries[0].size, 5);
        assert_eq!(manifest.entries[0].hash, 0x50d1_febb_65db_f0e6);
        assert!(verify_package(&manifest, "tree").is_clean());

        crate::fs::write_text("tree/a.txt", "dead").unwrap();
        crate::fs::delete("tree/b.txt").unwrap();
        crate::fs::write_text("tree/c.txt", "three").unwrap();
        let report = detect_modified_files(&manifest, "tree");
        assert_eq!(report.checked(), 3, "two entries and one extra file");
        assert_eq!(report.mismatches().len(), 3);
        let changed = report.mismatch("a.txt").expect("a changed file");
        assert_eq!(changed.kind, MismatchKind::Changed);
        assert_eq!(changed.expected.unwrap(), (5, 0x50d1_febb_65db_f0e6));
        assert_eq!(changed.found.unwrap(), (4, 0xa5bc_2267_4255_8d67));
        assert_eq!(report.mismatch("b.txt").unwrap().kind, MismatchKind::Missing);
        assert_eq!(report.mismatch("c.txt").unwrap().kind, MismatchKind::Extra);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bytes_verify_against_one_entry_and_a_change_names_both_hashes() {
        let _serial = serial();
        fresh();
        let manifest = IntegrityManifest::from_bytes("greeting.txt", b"hello");
        let mut other = manifest.clone();
        other.entries[0].hash ^= 1;
        assert_ne!(manifest.fingerprint(), other.fingerprint());
        assert_eq!(manifest.fingerprint(), get_manifest_hash(&manifest));

        let ok = manifest.verify_bytes("greeting.txt", b"hello");
        assert_eq!(ok.checked(), 1);
        assert!(ok.is_clean());

        let bad = manifest.verify_bytes("greeting.txt", b"other");
        assert_eq!(bad.mismatches().len(), 1);
        let changed = bad.mismatch("greeting.txt").expect("a changed file");
        assert_eq!(changed.kind, MismatchKind::Changed);
        assert_eq!(changed.expected.unwrap().1, 0xa430_d846_80aa_bd0b);
        assert_eq!(changed.found.unwrap().1, 0x0a24_ad61_c256_2a55);
        assert!(!verify_checksum(b"other", 0xa430_d846_80aa_bd0b));
        assert!(verify_checksum(b"hello", 0xa430_d846_80aa_bd0b));
    }

    #[test]
    fn each_trust_state_decides_its_own_way() {
        let _serial = serial();
        fresh();
        assert_eq!(get_trust("fresh"), TrustState::Unknown);
        assert_eq!(decide("fresh"), Decision::Warn);

        assert!(distrust("seen"));
        assert_eq!(get_trust("seen"), TrustState::Known);
        assert_eq!(decide("seen"), Decision::Warn);

        assert!(trust("vouched"));
        assert_eq!(get_trust("vouched"), TrustState::Trusted);
        assert_eq!(decide("vouched"), Decision::Allow);

        assert!(block("bad"));
        assert_eq!(get_trust("bad"), TrustState::Blocked);
        assert_eq!(decide("bad"), Decision::Deny);
        assert!(unblock("bad"));
        assert_eq!(get_trust("bad"), TrustState::Known);

        assert!(register_policy("forced", TrustState::Trusted));
        assert_eq!(get_trust_level("forced"), TrustState::Trusted);
        assert!(get_policy_result()
            .iter()
            .any(|(subject, decision)| subject == "forced" && *decision == Decision::Allow));
    }

    #[test]
    fn a_signed_manifest_matches_mismatches_or_names_an_unknown_signer() {
        let _serial = serial();
        fresh();
        assert!(register_trusted_signer("team"));
        assert!(!register_trusted_signer("team"), "once");
        assert_eq!(trusted_signers(), vec!["team".to_owned()]);

        let signed = sign("team", b"payload");
        assert!(verify_signature(&signed, b"payload").matched());
        assert!(is_package_trusted(&signed, b"payload"));
        assert_eq!(
            verify_signature(&signed, b"other"),
            SignatureOutcome::Mismatched {
                recorded: serialization::hash(b"payload"),
                computed: serialization::hash(b"other"),
            }
        );

        let stranger = sign("nobody", b"payload");
        assert_eq!(
            verify_signature(&stranger, b"payload"),
            SignatureOutcome::UnknownSigner {
                signer: "nobody".to_owned()
            }
        );

        assert!(unregister_trusted_signer("team"));
        assert!(!unregister_trusted_signer("team"));
        assert_eq!(
            verify_signature(&signed, b"payload"),
            SignatureOutcome::UnknownSigner {
                signer: "team".to_owned()
            }
        );
    }

    #[test]
    fn a_capability_without_its_permission_is_denied_with_both_named() {
        let _serial = serial();
        fresh();
        let manifest = ModManifest::new("probe", "probe", "1").capability(Capability::WorldRead);
        assert_eq!(
            missing_for(&manifest),
            vec![(Capability::WorldRead, Permission::ReadWorld)]
        );
        let denial = check_capability(Capability::WorldRead).unwrap_err();
        assert!(denial.contains("world_read"), "{denial}");
        assert!(denial.contains("read_world"), "{denial}");
        let _ = get_requested_permissions();
        let _ = get_granted_permissions();
    }

    #[test]
    fn the_audit_log_records_the_decision_and_filters_by_subject() {
        let _serial = serial();
        fresh();
        assert!(trust("alpha"));
        assert!(block("beta"));
        assert_eq!(evaluate_policy("alpha", 4242), Decision::Allow);
        assert_eq!(audit_count(), 3);

        let alpha = filter_by_subject("alpha");
        assert_eq!(alpha.len(), 2);
        let entry = alpha.iter().find(|entry| entry.at_millis == 4242).expect("recorded");
        assert_eq!(entry.action, "evaluate");
        assert_eq!(entry.subject, "alpha");
        assert_eq!(entry.decision, Decision::Allow);
        assert_eq!(entry.reason, TrustState::Trusted.rule());

        for at in 0..(AUDIT_LIMIT + 10) {
            record("probe", "ring", Decision::Warn, "why", at as u64);
        }
        assert_eq!(audit_count(), AUDIT_LIMIT);
        let log = get_audit_log();
        assert_eq!(log.first().unwrap().at_millis, 10);
        assert_eq!(log.last().unwrap().at_millis, (AUDIT_LIMIT + 10 - 1) as u64);
    }

    #[test]
    fn the_tamper_report_carries_what_failed_and_what_was_blocked() {
        let _serial = serial();
        fresh();
        let manifest = IntegrityManifest::from_bytes("f.txt", b"hello");
        let report = manifest.verify_bytes("f.txt", b"other");
        assert_eq!(report.mismatches().len(), 1);
        assert!(block("evil"));

        let tamper = get_security_report();
        assert_eq!(tamper.total(), 2, "one mismatch and one blocked subject");
        assert!(!tamper.is_clean());
        assert!(tamper.text().contains("f.txt"), "{}", tamper.text());
        let json = tamper.to_json().expect("a finite value");
        assert!(json.contains("\"total\":2"), "{json}");
        assert_eq!(clear_security_report(), 2);
        assert!(get_security_report().is_clean());
    }

    #[test]
    fn a_file_hash_reads_the_file_and_refusals_name_what_is_unreadable() {
        let _serial = serial();
        fresh();
        let root = root("hash");
        crate::fs::write_text("f.txt", "one").unwrap();
        assert_eq!(get_file_hash("f.txt"), Some(0x1a08_aa19_21ca_5caf));
        assert_eq!(get_file_hash("missing.txt"), None);

        assert!(why_unknown("client_trust_store").contains("trust store"));
        assert_eq!(refusals().len(), 3);
        assert!(get_client_integrity().is_none());
        assert!(get_client_trust_store().is_none());
        assert!(!is_mod_sandboxed());
        let _ = std::fs::remove_dir_all(&root);
    }
}
