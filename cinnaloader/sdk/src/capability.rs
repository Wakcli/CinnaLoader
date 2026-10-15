//! What a mod is allowed to do, what it asked for, and what this loader can actually do.
//!
//! A capability is a permission with teeth: the loader refuses to load a mod that requires
//! one the host cannot provide, and [`has`] answers for this mod only after checking what
//! it declared. Nothing here is a stub — [`Capability::is_supported`] is the honest list of
//! what a mod running inside this client can really reach today, and it grows as the
//! hooks behind each capability land.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! if capabilities::has(Capability::Render) {
//!     // registered an overlay line earlier; drawing into the HUD is granted
//! }
//! for missing in capabilities::check_permissions() {
//!     warn!("granted without the permission it implies: {}", missing.0);
//! }
//! ```

use std::fmt;
use std::str::FromStr;

use crate::modinfo::{ModManifest, Permission};

/// The version of the capability definitions this SDK speaks.
pub const VERSION: &str = "1";

/// One thing a mod may be granted. The set follows the wishlist's capability list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    // world and entities
    WorldRead,
    WorldWrite,
    EntityRead,
    EntityWrite,
    BlockRead,
    BlockWrite,
    InventoryRead,
    InventoryWrite,
    // networking
    PacketObserve,
    PacketModify,
    PacketSend,
    RawNetworkAccess,
    // rendering
    Render,
    CustomRendering,
    ShaderAccess,
    TextureRegistration,
    ModelRegistration,
    CustomScreens,
    HudModification,
    // input and camera
    Input,
    InputInterception,
    CameraControl,
    // content
    SoundRegistration,
    CommandRegistration,
    ResourceReload,
    // files and code
    FilesystemRead,
    FilesystemWrite,
    Reflection,
    NativeCode,
    ScriptExecution,
    // generation and new content
    WorldGeneration,
    CustomDimensions,
    CustomEntities,
    CustomBlocks,
    CustomItems,
    // the local player
    PlayerRead,
    // whose this loader is
    ServerIntegration,
    ClientIntegration,
}

/// Every capability this loader knows about.
pub const ALL: [Capability; 38] = [
    Capability::WorldRead,
    Capability::WorldWrite,
    Capability::EntityRead,
    Capability::EntityWrite,
    Capability::BlockRead,
    Capability::BlockWrite,
    Capability::InventoryRead,
    Capability::InventoryWrite,
    Capability::PacketObserve,
    Capability::PacketModify,
    Capability::PacketSend,
    Capability::RawNetworkAccess,
    Capability::Render,
    Capability::CustomRendering,
    Capability::ShaderAccess,
    Capability::TextureRegistration,
    Capability::ModelRegistration,
    Capability::CustomScreens,
    Capability::HudModification,
    Capability::Input,
    Capability::InputInterception,
    Capability::CameraControl,
    Capability::SoundRegistration,
    Capability::CommandRegistration,
    Capability::ResourceReload,
    Capability::FilesystemRead,
    Capability::FilesystemWrite,
    Capability::Reflection,
    Capability::NativeCode,
    Capability::ScriptExecution,
    Capability::WorldGeneration,
    Capability::CustomDimensions,
    Capability::CustomEntities,
    Capability::CustomBlocks,
    Capability::CustomItems,
    Capability::PlayerRead,
    Capability::ServerIntegration,
    Capability::ClientIntegration,
];

impl Capability {
    /// The name the wire format and the loader's feature list use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorldRead => "world_read",
            Self::WorldWrite => "world_write",
            Self::EntityRead => "entity_read",
            Self::EntityWrite => "entity_write",
            Self::BlockRead => "block_read",
            Self::BlockWrite => "block_write",
            Self::InventoryRead => "inventory_read",
            Self::InventoryWrite => "inventory_write",
            Self::PacketObserve => "packet_observe",
            Self::PacketModify => "packet_modify",
            Self::PacketSend => "packet_send",
            Self::RawNetworkAccess => "raw_network_access",
            Self::Render => "render",
            Self::CustomRendering => "custom_rendering",
            Self::ShaderAccess => "shader_access",
            Self::TextureRegistration => "texture_registration",
            Self::ModelRegistration => "model_registration",
            Self::CustomScreens => "custom_screens",
            Self::HudModification => "hud_modification",
            Self::Input => "input",
            Self::InputInterception => "input_interception",
            Self::CameraControl => "camera_control",
            Self::SoundRegistration => "sound_registration",
            Self::CommandRegistration => "command_registration",
            Self::ResourceReload => "resource_reload",
            Self::FilesystemRead => "filesystem_read",
            Self::FilesystemWrite => "filesystem_write",
            Self::Reflection => "reflection",
            Self::NativeCode => "native_code",
            Self::ScriptExecution => "script_execution",
            Self::WorldGeneration => "world_generation",
            Self::CustomDimensions => "custom_dimensions",
            Self::CustomEntities => "custom_entities",
            Self::CustomBlocks => "custom_blocks",
            Self::CustomItems => "custom_items",
            Self::PlayerRead => "player_read",
            Self::ServerIntegration => "server_integration",
            Self::ClientIntegration => "client_integration",
        }
    }

    /// What it lets a mod do, in one line, for a panel or a log.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::WorldRead => "read the world: blocks, chunks, time, weather",
            Self::WorldWrite => "change the world: place and break blocks",
            Self::EntityRead => "read entities: position, type, health",
            Self::EntityWrite => "change entities: teleport, damage, spawn, remove",
            Self::BlockRead => "read block state at a position",
            Self::BlockWrite => "write block state at a position",
            Self::InventoryRead => "read containers and inventories",
            Self::InventoryWrite => "move into and out of containers and inventories",
            Self::PacketObserve => "watch packets as they pass",
            Self::PacketModify => "rewrite packets as they pass",
            Self::PacketSend => "send packets of its own",
            Self::RawNetworkAccess => "open sockets of its own",
            Self::Render => "draw into the HUD",
            Self::CustomRendering => "draw its own geometry in the world",
            Self::ShaderAccess => "supply or replace shaders",
            Self::TextureRegistration => "register textures",
            Self::ModelRegistration => "register models",
            Self::CustomScreens => "open screens of its own",
            Self::HudModification => "add, move and hide HUD elements",
            Self::Input => "read keyboard and mouse beyond its declared keybinds",
            Self::InputInterception => "consume input before the game sees it",
            Self::CameraControl => "move and aim the camera",
            Self::SoundRegistration => "register and play sounds",
            Self::CommandRegistration => "register chat commands",
            Self::ResourceReload => "ask for a resource reload",
            Self::FilesystemRead => "read files",
            Self::FilesystemWrite => "write files",
            Self::Reflection => "inspect and reach into the running process",
            Self::NativeCode => "load and run native code",
            Self::ScriptExecution => "run scripts",
            Self::WorldGeneration => "take part in generating the world",
            Self::CustomDimensions => "add dimensions",
            Self::CustomEntities => "add entity types",
            Self::CustomBlocks => "add block types",
            Self::CustomItems => "add item types",
            Self::PlayerRead => "read the local player: position, health, held item",
            Self::ServerIntegration => "run as part of a server",
            Self::ClientIntegration => "run as part of the client: its menus, its frames",
        }
    }

    /// The version of this capability's definition, for a mod that gated on it.
    #[must_use]
    pub fn version(self) -> &'static str {
        VERSION
    }

    /// Whether the capability is still settling, so a mod should treat it as optional.
    #[must_use]
    pub fn is_experimental(self) -> bool {
        matches!(
            self,
            Self::ShaderAccess
                | Self::CustomRendering
                | Self::CameraControl
                | Self::WorldGeneration
                | Self::CustomDimensions
                | Self::CustomEntities
                | Self::CustomBlocks
                | Self::CustomItems
                | Self::ResourceReload
                | Self::ServerIntegration
        )
    }

    /// Whether granting it hands a mod something that can break the process or the user's
    /// data, so it is worth asking about out loud.
    #[must_use]
    pub fn is_unsafe(self) -> bool {
        matches!(
            self,
            Self::RawNetworkAccess
                | Self::Reflection
                | Self::NativeCode
                | Self::ScriptExecution
                | Self::PacketModify
                | Self::WorldWrite
                | Self::EntityWrite
                | Self::BlockWrite
                | Self::InventoryWrite
                | Self::FilesystemWrite
        )
    }

    /// The permissions a mod is expected to declare alongside this capability.
    #[must_use]
    pub fn requirements(self) -> &'static [Permission] {
        match self {
            Self::FilesystemRead => &[Permission::ReadFiles],
            Self::FilesystemWrite => &[Permission::WriteFiles],
            Self::RawNetworkAccess | Self::PacketSend => &[Permission::UseSockets],
            Self::PlayerRead => &[Permission::ReadPlayerData],
            Self::WorldRead
            | Self::BlockRead
            | Self::EntityRead
            | Self::InventoryRead => &[Permission::ReadWorld],
            Self::WorldWrite
            | Self::BlockWrite
            | Self::EntityWrite
            | Self::InventoryWrite => &[Permission::ModifyWorld],
            Self::Render
            | Self::CustomRendering
            | Self::ShaderAccess
            | Self::TextureRegistration
            | Self::ModelRegistration
            | Self::CustomScreens
            | Self::HudModification => &[Permission::ModifyRendering],
            Self::Input | Self::InputInterception | Self::CameraControl => &[Permission::Input],
            _ => &[],
        }
    }

    /// Whether a mod running here can actually be given this today. The list is short on
    /// purpose: everything missing needs a hook into the client that does not exist yet.
    #[must_use]
    pub fn is_supported(self) -> bool {
        matches!(
            self,
            Self::FilesystemRead
                | Self::FilesystemWrite
                | Self::Render
                | Self::HudModification
                | Self::Input
                | Self::ClientIntegration
        )
    }
}

/// Why a capability a mod needs cannot be given here.
#[must_use]
pub fn why_unsupported(capability: Capability) -> &'static str {
    match capability {
        Capability::FilesystemRead
        | Capability::FilesystemWrite
        | Capability::Render
        | Capability::HudModification
        | Capability::Input
        | Capability::ClientIntegration => "supported",
        Capability::WorldRead
        | Capability::WorldWrite
        | Capability::EntityRead
        | Capability::EntityWrite
        | Capability::BlockRead
        | Capability::BlockWrite
        | Capability::InventoryRead
        | Capability::InventoryWrite
        | Capability::PlayerRead => "needs hooks into the client's world and entity state",
        Capability::PacketObserve | Capability::PacketModify | Capability::PacketSend => {
            "needs a hook on the client's packet path"
        }
        Capability::CustomRendering | Capability::ShaderAccess | Capability::TextureRegistration
        | Capability::ModelRegistration | Capability::CustomScreens => {
            "needs the client's render and UI internals"
        }
        Capability::InputInterception | Capability::CameraControl => {
            "needs input and camera hooks, not the loader's own key poll"
        }
        Capability::SoundRegistration | Capability::CommandRegistration | Capability::ResourceReload => {
            "needs the client's resource and command registries"
        }
        Capability::Reflection
        | Capability::NativeCode
        | Capability::ScriptExecution
        | Capability::RawNetworkAccess => {
            "a mod is already native code, so this cannot be granted or withheld"
        }
        Capability::WorldGeneration
        | Capability::CustomDimensions
        | Capability::CustomEntities
        | Capability::CustomBlocks
        | Capability::CustomItems => "needs world generation and content registration hooks",
        Capability::ServerIntegration => "this loader is a client mod host, not a server",
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Capability {
    type Err = ();

    /// Parses a capability name. The spellings this loader used before the capability
    /// system existed are still accepted, so an older manifest still reads.
    fn from_str(text: &str) -> Result<Self, ()> {
        let text = text.trim().to_ascii_lowercase();
        let found = ALL.iter().copied().find(|cap| cap.as_str() == text);
        if let Some(found) = found {
            return Ok(found);
        }
        match text.as_str() {
            "files" => Ok(Self::FilesystemRead),
            "network" => Ok(Self::RawNetworkAccess),
            "network_intercept" => Ok(Self::PacketModify),
            _ => Err(()),
        }
    }
}

/// Every capability this loader knows about.
#[must_use]
pub fn list() -> Vec<Capability> {
    ALL.to_vec()
}

/// One capability by name, in any spelling [`Capability::from_str`] accepts.
#[must_use]
pub fn get(name: &str) -> Option<Capability> {
    name.parse().ok()
}

/// One capability by its own name.
#[must_use]
pub fn describe(capability: Capability) -> &'static str {
    capability.describe()
}

/// The version of a capability's definition.
#[must_use]
pub fn get_version(capability: Capability) -> &'static str {
    capability.version()
}

#[must_use]
pub fn is_experimental(capability: Capability) -> bool {
    capability.is_experimental()
}

#[must_use]
pub fn is_unsafe(capability: Capability) -> bool {
    capability.is_unsafe()
}

/// The permissions a capability implies.
#[must_use]
pub fn get_requirements(capability: Capability) -> &'static [Permission] {
    capability.requirements()
}

/// What the host can provide, whatever this mod declared.
#[must_use]
pub fn supported(capability: Capability) -> bool {
    if let Some(text) = crate::read_graph() {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("capability=") {
                let mut fields = rest.split('|');
                if fields.next() == Some(capability.as_str()) {
                    return fields.next() == Some("1");
                }
            }
        }
    }
    capability.is_supported()
}

/// Whether this mod holds `capability`: the host provides it and the mod declared it.
#[must_use]
pub fn has(capability: Capability) -> bool {
    has_declared(capability) && supported(capability)
}

/// Whether a mod other than this one provides `capability`, for a mod that offers an
/// optional integration with it.
#[must_use]
pub fn provided_by_id(capability: Capability) -> Vec<String> {
    let mut providers = Vec::new();
    if let Some(text) = crate::read_graph() {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("modcap=") {
                let mut fields = rest.split('|');
                if fields.next().is_some() && fields.next() == Some(capability.as_str()) {
                    if let Some(name) = rest.split('|').next() {
                        providers.push(name.to_owned());
                    }
                }
            }
        }
    }
    providers
}

/// Whether `capability` can be used right now, and a warning when it cannot.
///
/// This is the check to put in front of optional work: a capability that the host does not
/// provide is not an error, it is a feature that stays off.
#[must_use]
pub fn require(capability: Capability) -> bool {
    if has(capability) {
        return true;
    }
    if !capability.is_supported() {
        crate::warn!(
            "capability {} is not available here: {}",
            capability,
            why_unsupported(capability)
        );
    } else {
        crate::warn!(
            "capability {} is available but this mod did not declare it",
            capability
        );
    }
    false
}

/// Adds a capability to this mod's manifest and tells the loader, for a capability that
/// only becomes necessary at runtime.
#[must_use]
pub fn register(capability: Capability) -> bool {
    if has_declared(capability) {
        return true;
    }
    crate::metadata::amend(|manifest| manifest.capabilities.push(capability))
}

/// Takes a capability back out of this mod's manifest.
#[must_use]
pub fn unregister(capability: Capability) -> bool {
    if !has_declared(capability) {
        return true;
    }
    crate::metadata::amend(|manifest| {
        manifest.capabilities.retain(|held| *held != capability);
    })
}

/// Whether this mod's own manifest lists `capability`, whatever the host can provide.
fn has_declared(capability: Capability) -> bool {
    crate::metadata::manifest()
        .capabilities
        .contains(&capability)
}

/// Capabilities this mod declared without the permissions they imply, which is what a
/// reviewer needs to see. Empty is the answer that everything lines up.
#[must_use]
pub fn check_permissions() -> Vec<(Capability, Permission)> {
    let manifest: ModManifest = crate::metadata::manifest();
    let mut missing = Vec::new();
    for capability in &manifest.capabilities {
        for permission in capability.requirements() {
            // Compared the way the manifest may spell it, so `render` and
            // `modify_rendering` are the same permission.
            if !manifest
                .permissions
                .iter()
                .any(|held| held.canonical() == permission.canonical())
            {
                missing.push((*capability, *permission));
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_has_a_name_that_parses_back() {
        for capability in ALL {
            assert_eq!(
                capability.as_str().parse::<Capability>(),
                Ok(capability),
                "{} did not parse back",
                capability.as_str()
            );
            assert!(!capability.describe().is_empty());
        }
    }

    #[test]
    fn an_older_spelling_still_reads() {
        assert_eq!("files".parse::<Capability>(), Ok(Capability::FilesystemRead));
        assert_eq!(
            "network".parse::<Capability>(),
            Ok(Capability::RawNetworkAccess)
        );
        assert_eq!(
            "network_intercept".parse::<Capability>(),
            Ok(Capability::PacketModify)
        );
        assert!("nonsense".parse::<Capability>().is_err());
    }

    #[test]
    fn what_is_not_supported_says_why() {
        for capability in ALL {
            if capability.is_supported() {
                assert_eq!(why_unsupported(capability), "supported");
            } else {
                assert_ne!(why_unsupported(capability), "supported");
                assert_ne!(why_unsupported(capability), "needs a hook that does not exist yet");
            }
        }
    }

    #[test]
    fn writing_and_removing_a_capability_touches_the_manifest() {
        let capability = Capability::CommandRegistration;
        assert!(register(capability));
        assert!(crate::metadata::manifest().capabilities.contains(&capability));
        assert!(unregister(capability));
        assert!(!crate::metadata::manifest().capabilities.contains(&capability));
    }

    #[test]
    fn a_permission_that_was_not_declared_is_reported() {
        crate::metadata::declare(
            ModManifest::new("perm-probe", "Perm probe", "0.1")
                .capability(Capability::FilesystemWrite)
                .permission(Permission::ReadFiles),
        );
        let missing = check_permissions();
        assert_eq!(missing, vec![(Capability::FilesystemWrite, Permission::WriteFiles)]);
    }
}
