//! Permissions: what a mod may do, what it asked for, and what the user allowed.
//!
//! The rule this module is built on, because getting it wrong is how a mod loader becomes a
//! security problem: **a mod cannot raise its own permissions.** It declares what it wants
//! and the host decides. [`request`] records the wish and logs it; [`grant`] only ever
//! narrows what the mod already declared; [`set_trust_level`] only ever lowers. A permission
//! that this host cannot enforce — raw memory, launching processes, unrestricted filesystem
//! access — is answered with an honest `Never` and a reason, never a silent yes.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! if !permissions::has(Permission::WriteFiles) {
//!     permissions::request(Permission::WriteFiles);
//! }
//! permissions::record_access(Permission::ReadFiles, "read own config");
//! for entry in permissions::get_audit_log() {
//!     info!("{}", entry);
//! }
//! ```

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::modinfo::{Permission, ALL_PERMISSIONS};

/// How much the host trusts a mod. A mod asks; the host decides, and the loader clamps what
/// a manifest claims for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TrustLevel {
    /// Nothing beyond being loaded: every request is refused.
    Untrusted,
    /// Only what it declared and nothing else.
    Restricted,
    /// What it declared, within what the host can enforce.
    #[default]
    Normal,
    /// Everything the host can give; only the user may set this, and the loader refuses a
    /// manifest that claims it.
    Trusted,
}

impl TrustLevel {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::Restricted => "restricted",
            Self::Normal => "normal",
            Self::Trusted => "trusted",
        }
    }

    /// The same, one step narrower. A mod may only ever move this way.
    #[must_use]
    pub fn narrowed(self) -> Self {
        match self {
            Self::Trusted => Self::Normal,
            Self::Normal => Self::Restricted,
            Self::Restricted | Self::Untrusted => Self::Untrusted,
        }
    }
}

impl fmt::Display for TrustLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TrustLevel {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        match text.trim().to_ascii_lowercase().as_str() {
            "untrusted" => Ok(Self::Untrusted),
            "restricted" => Ok(Self::Restricted),
            "normal" => Ok(Self::Normal),
            "trusted" => Ok(Self::Trusted),
            _ => Err(()),
        }
    }
}

/// What the host's policy says about one permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Given to a mod that declares it.
    Always,
    /// Given when the mod declares it and the user has not narrowed the trust level.
    OnRequest,
    /// Never given here, and this is why.
    Never(&'static str),
}

impl Grant {
    #[must_use]
    pub fn is_available(self) -> bool {
        !matches!(self, Self::Never(_))
    }

    #[must_use]
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Never(reason) => Some(reason),
            _ => None,
        }
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Always => formatter.write_str("granted"),
            Self::OnRequest => formatter.write_str("on request"),
            Self::Never(reason) => write!(formatter, "never ({reason})"),
        }
    }
}

/// One line of the audit log: what was asked, what was done, and about what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub permission: Permission,
    pub kind: AuditKind,
    pub detail: String,
    pub at_millis: u64,
}

impl fmt::Display for AuditEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:>8}ms {:?} {} {}",
            self.at_millis,
            self.kind,
            self.permission,
            self.detail
        )
    }
}

/// What the audit line is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditKind {
    Requested,
    Granted,
    Revoked,
    Denied,
    Restricted,
    Accessed,
    Prompted,
}

#[derive(Default)]
struct Held {
    /// Permissions this mod asked for at runtime.
    requested: Vec<Permission>,
    /// Permissions it narrowed away.
    revoked: Vec<Permission>,
    /// Permissions the host refused, with the reason.
    denied: Vec<(Permission, String)>,
    /// Permissions the mod itself restricted, which nothing can widen again.
    restricted: Vec<Permission>,
    /// Permissions a prompt was put to the user for, and what came back.
    prompted: Vec<(Permission, bool)>,
    /// Names a mod added beyond the built-in set.
    registered: Vec<(String, String)>,
    audit: Vec<AuditEntry>,
    trust: Option<TrustLevel>,
}

/// How many audit lines are kept.
pub const AUDIT_LIMIT: usize = 256;

static NEXT_RECORD: AtomicU64 = AtomicU64::new(1);

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn note(held: &mut Held, permission: Permission, kind: AuditKind, detail: impl Into<String>) {
    if held.audit.len() == AUDIT_LIMIT {
        held.audit.remove(0);
    }
    let entry = AuditEntry {
        permission,
        kind,
        detail: detail.into(),
        at_millis: crate::diagnostics::get_timeline()
            .last()
            .map(|(seconds, _)| seconds * 1000)
            .unwrap_or_default(),
    };
    held.audit.push(entry);
}

/// What this host does about one permission. The list is deliberately short: everything the
/// client gives no hook for is a `Never` with the reason, so a mod is told the truth instead
/// of being given something that does nothing.
#[must_use]
pub fn get_policy() -> Vec<(Permission, Grant)> {
    ALL_PERMISSIONS
        .iter()
        .copied()
        .map(|permission| (permission, policy_for(permission)))
        .collect()
}

fn policy_for(permission: Permission) -> Grant {
    match permission.canonical() {
        Permission::ReadFiles | Permission::WriteFiles => Grant::Always,
        // Drawing into the loader's own HUD is what the overlay is, and reading input is
        // what a keybind already is; both are enforced by what the host offers.
        Permission::ModifyRendering
        | Permission::ReadGameState
        | Permission::Input => Grant::OnRequest,
        Permission::ReadWorld
        | Permission::ModifyWorld
        | Permission::ReadPlayerData
        | Permission::ModifyPlayerData
        | Permission::ReadInventory
        | Permission::ModifyInventory => {
            Grant::Never("needs hooks into the client's world and entity state")
        }
        Permission::SendNetworkMessages
        | Permission::InterceptPackets
        | Permission::ModifyPackets => Grant::Never("needs a hook on the client's packet path"),
        Permission::UseSockets => Grant::Never("the loader does not proxy sockets for a mod"),
        Permission::AccessExternalFiles => {
            Grant::Never("a mod reaches its own directory, not the user's files")
        }
        Permission::LaunchProcesses
        | Permission::LoadNativeLibraries
        | Permission::AccessMemory
        | Permission::AccessSystemInformation => {
            Grant::Never("a mod is already native code, so this cannot be granted or withheld")
        }
        Permission::RegisterCommands => Grant::Never("needs the client's command registry"),
        Permission::ExecuteCommands => {
            Grant::Never("needs a hook into the client's chat and command path")
        }
        Permission::AccessClipboard | Permission::OpenUrls => {
            Grant::Never("needs the client's platform layer")
        }
        _ => Grant::Never("this host does not provide it"),
    }
}

/// Every permission, as the policy sees it.
#[must_use]
pub fn list() -> Vec<Permission> {
    ALL_PERMISSIONS.to_vec()
}

/// The trust level in force for this mod.
#[must_use]
pub fn get_trust_level() -> TrustLevel {
    // The loader's answer wins: it is the one enforcing, and it clamps what a manifest
    // claims for itself.
    if let Some(info) = crate::metadata::reported() {
        return info.trust;
    }
    with(|held| held.trust.unwrap_or_else(|| crate::metadata::manifest().trust))
}

/// Narrows the trust level, never widens it. A mod that tries to promote itself stays where
/// it is.
pub fn set_trust_level(level: TrustLevel) -> bool {
    let current = get_trust_level();
    if level >= current {
        with(|held| {
            held.trust = Some(current);
        });
        crate::warn!(
            "a mod cannot raise its own trust level ({} -> {level} was refused)",
            current.as_str()
        );
        return false;
    }
    with(|held| held.trust = Some(level));
    true
}

/// Whether this mod is trusted enough to be given what it declared.
#[must_use]
pub fn is_trusted() -> bool {
    get_trust_level() >= TrustLevel::Normal
}

/// Every permission this mod declared in its manifest, as the loader sees it.
#[must_use]
pub fn list_declared() -> Vec<Permission> {
    crate::metadata::manifest().permissions
}

/// The permissions this mod asked for at runtime and has.
#[must_use]
pub fn list_requested() -> Vec<Permission> {
    with(|held| held.requested.clone())
}

/// Every permission another mod declared, by mod id or name, from the loader's registry.
#[must_use]
pub fn list_for_mod(id: &str) -> Vec<Permission> {
    let Some(text) = crate::read_graph() else {
        return Vec::new();
    };
    let name = crate::mods::registry()
        .mods
        .iter()
        .find(|entry| entry.name == id || entry.id == id)
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| id.to_owned());
    let mut permissions = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("modperm=") {
            let mut fields = rest.split('|');
            if fields.next() == Some(name.as_str()) {
                if let Some(permission) = fields.next().and_then(|text| text.parse().ok()) {
                    permissions.push(permission);
                }
            }
        }
    }
    permissions
}

/// Whether this mod holds `permission`: it declared or asked for it, the host's policy allows
/// it, it was not revoked, and the trust level reaches it.
#[must_use]
pub fn has(permission: Permission) -> bool {
    let canonical = permission.canonical();
    let held = with(|held| {
        (
            held.revoked.clone(),
            held.restricted.clone(),
            held.requested.clone(),
        )
    });
    let (revoked, restricted, requested) = held;
    if revoked.contains(&canonical) || restricted.contains(&canonical) {
        return false;
    }
    let declared = list_declared()
        .iter()
        .any(|held| held.canonical() == canonical)
        || requested.iter().any(|held| held.canonical() == canonical);
    if !declared {
        return false;
    }
    match policy_for(canonical) {
        Grant::Always => true,
        Grant::OnRequest => is_trusted(),
        Grant::Never(_) => false,
    }
}

/// The same as [`has`], for a manifest that spelled the permission an older way.
#[must_use]
pub fn check(permission: Permission) -> bool {
    has(permission)
}

/// Asks for a permission: it joins this mod's manifest and the loader is told. Whether it is
/// then held is [`has`]'s answer, and a permission this host cannot enforce is refused here
/// with the reason in the audit log.
pub fn request(permission: Permission) -> bool {
    let canonical = permission.canonical();
    if has(canonical) {
        return true;
    }
    let refusal = policy_for(canonical).reason().map(str::to_owned);
    with(|held| {
        if !held.requested.contains(&canonical) {
            held.requested.push(canonical);
        }
        match &refusal {
            Some(reason) => {
                if !held.denied.iter().any(|(held, _)| *held == canonical) {
                    held.denied.push((canonical, reason.clone()));
                }
                note(held, canonical, AuditKind::Denied, reason.clone());
            }
            None => note(held, canonical, AuditKind::Requested, "declared at runtime"),
        }
    });
    let mut manifest = crate::metadata::manifest();
    if !manifest.permissions.iter().any(|held| held.canonical() == canonical) {
        manifest.permissions.push(canonical);
        crate::metadata::amend(|current| {
            if !current.permissions.iter().any(|held| held.canonical() == canonical) {
                current.permissions.push(canonical);
            }
        });
    }
    // The request is recorded either way; this says whether the host will grant it.
    refusal.is_none()
}

/// Gives a permission that this host can enforce. It can only narrow: a permission the policy
/// refuses stays refused, and the reason is kept.
pub fn grant(permission: Permission) -> bool {
    let canonical = permission.canonical();
    if with(|held| held.restricted.contains(&canonical)) {
        with(|held| {
            held.denied
                .push((canonical, "this mod restricted it, and nothing widens that back".to_owned()));
            note(
                held,
                canonical,
                AuditKind::Denied,
                "this mod restricted it, and nothing widens that back",
            );
        });
        return false;
    }
    match policy_for(canonical) {
        Grant::Never(reason) => {
            with(|held| {
                held.denied.push((canonical, reason.to_owned()));
                note(held, canonical, AuditKind::Denied, reason);
            });
            false
        }
        _ => {
            with(|held| {
                held.revoked.retain(|held| *held != canonical);
                if !held.requested.contains(&canonical) {
                    held.requested.push(canonical);
                }
                note(held, canonical, AuditKind::Granted, "granted");
            });
            true
        }
    }
}

/// Takes a permission back. A mod can always do this to itself.
pub fn revoke(permission: Permission) -> bool {
    let canonical = permission.canonical();
    with(|held| {
        if !held.revoked.contains(&canonical) {
            held.revoked.push(canonical);
        }
        note(held, canonical, AuditKind::Revoked, "revoked");
    });
    true
}

/// Marks a permission as one this mod will never use again, which is stronger than revoking:
/// nothing widens it back.
pub fn restrict(permission: Permission) -> bool {
    let canonical = permission.canonical();
    with(|held| {
        if !held.restricted.contains(&canonical) {
            held.restricted.push(canonical);
        }
        note(held, canonical, AuditKind::Restricted, "restricted for this mod");
    });
    true
}

/// Asks the user, which in this host means recording the request where the user will see it:
/// the panel lists a mod's permissions and the loader's log carries the line. It does not
/// grant anything by itself, and it is honest about that.
pub fn prompt_user(permission: Permission) -> bool {
    let canonical = permission.canonical();
    with(|held| {
        held.prompted.push((canonical, false));
        note(
            held,
            canonical,
            AuditKind::Prompted,
            "no dialog in this host; the panel and the log carry it",
        );
    });
    crate::warn!(
        "{} wants the {} permission ({}); it is listed in the loader's panel, and the user decides",
        crate::metadata::id(),
        canonical,
        canonical.describe()
    );
    false
}

/// Teaches the policy about a permission of the mod's own, without giving it more than the
/// host can enforce: a registered name is only ever `OnRequest`.
pub fn register(permission: Permission) -> bool {
    let canonical = permission.canonical();
    with(|held| {
        let name = canonical.as_str().to_owned();
        if held.registered.iter().any(|(held, _)| held == &name) {
            return false;
        }
        held.registered
            .push((name, canonical.describe().to_owned()));
        note(held, canonical, AuditKind::Requested, "registered");
        true
    })
}

/// Forgets a permission the mod registered.
pub fn unregister(permission: Permission) -> bool {
    let canonical = permission.canonical();
    with(|held| {
        let name = canonical.as_str().to_owned();
        let before = held.registered.len();
        held.registered.retain(|(held, _)| held != &name);
        held.registered.len() != before
    })
}

/// Permissions this mod registered beyond the built-in set.
#[must_use]
pub fn registered() -> Vec<(String, String)> {
    with(|held| held.registered.clone())
}

/// Permissions the host refused, with the reason.
#[must_use]
pub fn denied() -> Vec<(Permission, String)> {
    with(|held| held.denied.clone())
}

/// Notes that a mod used a permission, for a user reading the log afterwards.
pub fn record_access(permission: Permission, detail: &str) {
    let canonical = permission.canonical();
    with(|held| note(held, canonical, AuditKind::Accessed, format!("used: {detail}")));
    crate::log::log_fields(
        crate::log::Level::Info,
        "permission use",
        &[
            ("permission", canonical.as_str()),
            ("held", if has(canonical) { "yes" } else { "no" }),
            ("detail", detail),
        ],
    );
}

/// The audit log, oldest first.
#[must_use]
pub fn get_audit_log() -> Vec<AuditEntry> {
    with(|held| held.audit.clone())
}

/// Drops the audit log and answers how many lines went.
pub fn clear_audit_log() -> usize {
    with(|held| {
        let count = held.audit.len();
        held.audit.clear();
        count
    })
}

/// Everything a user needs to decide about this mod: what it declared, what it holds, what
/// was refused and why, and the trust level.
#[must_use]
pub fn report() -> Vec<String> {
    let mut lines = vec![format!(
        "{} at trust {} (trusted={})",
        crate::metadata::id(),
        get_trust_level(),
        is_trusted()
    )];
    for permission in list_declared() {
        lines.push(format!(
            "  {permission:<26} {:<12} {}",
            if has(permission) { "held" } else { "not held" },
            policy_for(permission)
        ));
    }
    for (permission, reason) in denied() {
        lines.push(format!("  refused {permission}: {reason}"));
    }
    lines
}

/// A fresh counter for a mod that wants to notice how often it uses something.
#[must_use]
pub fn next_record_id() -> u64 {
    NEXT_RECORD.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            held.requested.clear();
            held.revoked.clear();
            held.denied.clear();
            held.restricted.clear();
            held.prompted.clear();
            held.registered.clear();
            held.audit.clear();
            held.trust = Some(TrustLevel::Normal);
        });
    }

    #[test]
    fn a_permission_the_host_cannot_enforce_is_never_granted() {
        let _serial = serial();
        fresh();
        assert_eq!(
            policy_for(Permission::AccessMemory),
            Grant::Never("a mod is already native code, so this cannot be granted or withheld")
        );
        assert!(!request(Permission::AccessMemory));
        assert!(!has(Permission::AccessMemory));
        assert!(denied().iter().any(|(permission, _)| *permission == Permission::AccessMemory));
        assert!(get_audit_log()
            .iter()
            .any(|entry| entry.kind == AuditKind::Denied));
    }

    #[test]
    fn a_mod_cannot_raise_its_own_trust() {
        let _serial = serial();
        fresh();
        assert_eq!(get_trust_level(), TrustLevel::Normal);
        assert!(!set_trust_level(TrustLevel::Trusted), "that is the user's to give");
        assert_eq!(get_trust_level(), TrustLevel::Normal);
        assert!(set_trust_level(TrustLevel::Untrusted), "narrowing is allowed");
        assert_eq!(get_trust_level(), TrustLevel::Untrusted);
        assert!(!set_trust_level(TrustLevel::Trusted));
        assert_eq!(get_trust_level(), TrustLevel::Untrusted, "still narrowed");
    }

    #[test]
    fn an_undeclared_permission_is_not_held() {
        let _serial = serial();
        fresh();
        assert!(!has(Permission::WriteFiles));
        assert!(request(Permission::WriteFiles));
        assert!(has(Permission::WriteFiles));
        assert!(crate::metadata::manifest()
            .permissions
            .iter()
            .any(|held| held.canonical() == Permission::WriteFiles));
    }

    #[test]
    fn revoking_and_restricting_narrow_what_is_held() {
        let _serial = serial();
        fresh();
        request(Permission::ReadFiles);
        assert!(has(Permission::ReadFiles));
        assert!(revoke(Permission::ReadFiles));
        assert!(!has(Permission::ReadFiles));
        assert!(grant(Permission::ReadFiles));
        assert!(has(Permission::ReadFiles));
        assert!(restrict(Permission::ReadFiles));
        assert!(!has(Permission::ReadFiles));
        assert!(!grant(Permission::ReadFiles), "a restricted permission stays restricted");
    }

    #[test]
    fn an_older_spelling_is_the_same_permission() {
        let _serial = serial();
        fresh();
        assert_eq!(Permission::Render.canonical(), Permission::ModifyRendering);
        request(Permission::Render);
        assert!(has(Permission::ModifyRendering));
        assert!(has(Permission::Render));
    }

    #[test]
    fn a_prompt_is_recorded_rather_than_answered() {
        let _serial = serial();
        fresh();
        assert!(!prompt_user(Permission::ReadGameState));
        assert!(get_audit_log()
            .iter()
            .any(|entry| entry.kind == AuditKind::Prompted));
    }

    #[test]
    fn recording_an_access_leaves_a_line() {
        let _serial = serial();
        fresh();
        request(Permission::ReadFiles);
        record_access(Permission::ReadFiles, "read own config");
        assert!(get_audit_log().iter().any(|entry| entry.kind == AuditKind::Accessed
            && entry.detail.contains("read own config")));
    }

    #[test]
    fn the_policy_names_every_permission() {
        let policy = get_policy();
        assert_eq!(policy.len(), ALL_PERMISSIONS.len());
        assert!(policy
            .iter()
            .any(|(permission, grant)| *permission == Permission::ReadFiles
                && *grant == Grant::Always));
        assert!(policy
            .iter()
            .any(|(permission, grant)| *permission == Permission::LaunchProcesses
                && !grant.is_available()));
        for (permission, grant) in policy {
            if let Grant::Never(reason) = grant {
                assert!(!reason.is_empty(), "{permission} has no reason");
            }
        }
    }

    #[test]
    fn a_registered_permission_can_be_forgotten_again() {
        let _serial = serial();
        fresh();
        assert!(register(Permission::OpenUrls));
        assert!(!register(Permission::OpenUrls), "once");
        assert!(registered()
            .iter()
            .any(|(name, _)| name == Permission::OpenUrls.as_str()));
        assert!(unregister(Permission::OpenUrls));
        assert!(!unregister(Permission::OpenUrls));
    }

    #[test]
    fn the_report_reads_like_something_a_user_can_decide_on() {
        let _serial = serial();
        fresh();
        request(Permission::AccessClipboard);
        let lines = report();
        assert!(lines[0].contains("trust"), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("refused")), "{lines:?}");
    }
}
