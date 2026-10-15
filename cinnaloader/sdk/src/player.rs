//! Players: the state a mod owns, and the local player, which the client owns.
//!
//! A [`PlayerState`] is a value: name, uuid, game mode, where they are and how they are
//! moving, health and food, experience and level, score, locale, team, permissions,
//! statistics, advancements, effects, respawn and spawn points, and — through the sections
//! that came before it — an inventory of stacks and a held stack. Everything a mod needs to
//! reason about a player is here, and all of it is arithmetic and data the mod already has.
//!
//! The local player is the client's: where they are, how much health they have, what they
//! are looking at, how they are connected and what they are pressing are all the client's
//! state, and every one of those answers `None` or an empty value with the reason in
//! [`why_unknown`].
//!
//! Two of the "send" calls *are* real, because this host has somewhere to put them:
//! [`send_message`] writes to the loader's log, and [`send_action_bar`] writes the loader's
//! on-screen overlay line, which is the closest thing a mod host has to an action bar. The
//! rest — titles, sounds, particles, screens — need the client's UI, audio and renderer, and
//! are refused rather than pretended.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let mut player = PlayerState::new("Tester");
//! player.set_health(20.0);
//! player.set_game_mode(GameMode::Creative);
//! player.inventory_mut().set_slot(0, Some(ItemStack::new("minecraft:torch", 64)));
//! assert_eq!(player.held_item().map(|stack| stack.count()), Some(64));
//! assert!(
//!     player::known_only_to_the_client::get_local().is_none(),
//!     "the local player is the client's"
//! );
//! ```

use std::collections::BTreeMap;
use std::fmt;

use crate::inventory::{Inventory, OFFHAND, ARMOUR_FIRST, ARMOUR_LAST};
use crate::items::ItemStack;

/// What a player is allowed to do on a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameMode {
    #[default]
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
            Self::Spectator => "spectator",
        }
    }
}

impl fmt::Display for GameMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Something a player is allowed to do, which a server decides and this host only records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerPermission {
    Build,
    Mine,
    UseDoors,
    UseContainers,
    AttackPlayers,
    AttackMobs,
    Fly,
    Teleport,
    Chat,
    Operator,
}

impl PlayerPermission {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Mine => "mine",
            Self::UseDoors => "use_doors",
            Self::UseContainers => "use_containers",
            Self::AttackPlayers => "attack_players",
            Self::AttackMobs => "attack_mobs",
            Self::Fly => "fly",
            Self::Teleport => "teleport",
            Self::Chat => "chat",
            Self::Operator => "operator",
        }
    }
}

/// One status effect on a player, as the loader's own record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub name: String,
    pub amplifier: u8,
    /// Milliseconds it has left.
    pub duration: u64,
    pub ambient: bool,
    pub visible: bool,
}

impl Effect {
    #[must_use]
    pub fn new(name: &str, amplifier: u8, duration: u64) -> Self {
        Self {
            name: name.to_owned(),
            amplifier,
            duration,
            ambient: false,
            visible: true,
        }
    }

    /// How strong it is, in roman numerals for a report.
    #[must_use]
    pub fn level(&self) -> u32 {
        u32::from(self.amplifier) + 1
    }

    /// Whether it has run out at that point in time.
    #[must_use]
    pub fn expired_at(&self, elapsed_millis: u64) -> bool {
        elapsed_millis >= self.duration
    }
}

/// One line of a player's statistics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Statistics {
    pub values: BTreeMap<String, i64>,
}

impl Statistics {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<i64> {
        self.values.get(name).copied()
    }

    pub fn set(&mut self, name: &str, value: i64) {
        self.values.insert(name.to_owned(), value);
    }

    pub fn add(&mut self, name: &str, by: i64) -> i64 {
        let value = self.values.entry(name.to_owned()).or_insert(0);
        *value += by;
        *value
    }
}

/// A player, as far as a mod needs to know about one.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub name: String,
    pub uuid: String,
    pub display_name: String,
    pub game_mode: GameMode,
    pub position: (f64, f64, f64),
    pub rotation: (f32, f32),
    pub velocity: (f64, f64, f64),
    pub health: f32,
    pub max_health: f32,
    pub food_level: u32,
    pub saturation: f32,
    pub experience: f32,
    pub level: u32,
    pub score: i64,
    pub locale: String,
    pub team: Option<String>,
    pub permissions: Vec<PlayerPermission>,
    pub statistics: Statistics,
    pub advancements: Vec<String>,
    pub effects: Vec<Effect>,
    pub respawn_position: Option<(f64, f64, f64)>,
    pub spawn_position: Option<(f64, f64, f64)>,
    pub last_death_position: Option<(f64, f64, f64)>,
    pub skin: String,
    pub cape: String,
    pub profile: BTreeMap<String, String>,
    pub selected_slot: usize,
    pub inventory: Inventory,
    pub flying: bool,
    pub sprinting: bool,
    pub sneaking: bool,
    pub invisible: bool,
}

impl PlayerState {
    /// A player with a name and the defaults of a fresh spawn.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            uuid: String::new(),
            display_name: name.to_owned(),
            game_mode: GameMode::Survival,
            position: (0.0, 64.0, 0.0),
            rotation: (0.0, 0.0),
            velocity: (0.0, 0.0, 0.0),
            health: 20.0,
            max_health: 20.0,
            food_level: 20,
            saturation: 5.0,
            experience: 0.0,
            level: 0,
            score: 0,
            locale: String::new(),
            team: None,
            permissions: Vec::new(),
            statistics: Statistics::default(),
            advancements: Vec::new(),
            effects: Vec::new(),
            respawn_position: None,
            spawn_position: None,
            last_death_position: None,
            skin: String::new(),
            cape: String::new(),
            profile: BTreeMap::new(),
            selected_slot: 0,
            inventory: Inventory::player(),
            flying: false,
            sprinting: false,
            sneaking: false,
            invisible: false,
        }
    }

    #[must_use]
    pub fn with_uuid(mut self, uuid: &str) -> Self {
        self.uuid = uuid.to_owned();
        self
    }

    #[must_use]
    pub fn with_game_mode(mut self, game_mode: GameMode) -> Self {
        self.game_mode = game_mode;
        self
    }

    #[must_use]
    pub fn at(mut self, x: f64, y: f64, z: f64) -> Self {
        self.position = (x, y, z);
        self
    }

    pub fn set_position(&mut self, x: f64, y: f64, z: f64) {
        self.position = (x, y, z);
    }

    pub fn set_rotation(&mut self, yaw: f32, pitch: f32) {
        self.rotation = (yaw, pitch);
    }

    pub fn set_velocity(&mut self, x: f64, y: f64, z: f64) {
        self.velocity = (x, y, z);
    }

    /// Sets health, clamped to the maximum and never below zero.
    pub fn set_health(&mut self, health: f32) -> &mut Self {
        self.health = health.clamp(0.0, self.max_health);
        self
    }

    pub fn set_max_health(&mut self, max_health: f32) {
        self.max_health = max_health.max(0.0);
        self.health = self.health.min(self.max_health);
    }

    pub fn set_food_level(&mut self, food_level: u32) {
        self.food_level = food_level.min(20);
    }

    pub fn set_saturation(&mut self, saturation: f32) {
        self.saturation = saturation.max(0.0);
    }

    pub fn set_experience(&mut self, experience: f32) {
        self.experience = experience.clamp(0.0, 1.0);
    }

    pub fn set_level(&mut self, level: u32) {
        self.level = level;
    }

    pub fn set_score(&mut self, score: i64) {
        self.score = score;
    }

    pub fn set_game_mode(&mut self, game_mode: GameMode) {
        self.game_mode = game_mode;
    }

    pub fn set_locale(&mut self, locale: &str) {
        self.locale = locale.to_owned();
    }

    pub fn set_team(&mut self, team: Option<&str>) {
        self.team = team.map(str::to_owned);
    }

    pub fn set_display_name(&mut self, display_name: &str) {
        self.display_name = display_name.to_owned();
    }

    pub fn set_skin(&mut self, skin: &str) {
        self.skin = skin.to_owned();
    }

    pub fn set_cape(&mut self, cape: &str) {
        self.cape = cape.to_owned();
    }

    pub fn set_flying(&mut self, flying: bool) {
        self.flying = flying;
    }

    pub fn set_sprinting(&mut self, sprinting: bool) {
        self.sprinting = sprinting;
    }

    pub fn set_sneaking(&mut self, sneaking: bool) {
        self.sneaking = sneaking;
    }

    pub fn set_invisible(&mut self, invisible: bool) {
        self.invisible = invisible;
    }

    pub fn grant(&mut self, permission: PlayerPermission) {
        if !self.permissions.contains(&permission) {
            self.permissions.push(permission);
        }
    }

    pub fn revoke(&mut self, permission: PlayerPermission) -> bool {
        let before = self.permissions.len();
        self.permissions.retain(|held| *held != permission);
        self.permissions.len() != before
    }

    #[must_use]
    pub fn has_permission(&self, permission: PlayerPermission) -> bool {
        self.permissions.contains(&permission)
    }

    /// Adds an effect, replacing one of the same name.
    pub fn apply_effect(&mut self, effect: Effect) {
        self.effects.retain(|held| held.name != effect.name);
        self.effects.push(effect);
    }

    pub fn remove_effect(&mut self, name: &str) -> bool {
        let before = self.effects.len();
        self.effects.retain(|effect| effect.name != name);
        self.effects.len() != before
    }

    /// Effects that are still running at that point in time.
    #[must_use]
    pub fn active_effects(&self, elapsed_millis: u64) -> Vec<Effect> {
        self.effects
            .iter()
            .filter(|effect| !effect.expired_at(elapsed_millis))
            .cloned()
            .collect()
    }

    pub fn unlock_advancement(&mut self, name: &str) -> bool {
        if self.advancements.iter().any(|held| held == name) {
            return false;
        }
        self.advancements.push(name.to_owned());
        true
    }

    pub fn set_profile(&mut self, key: &str, value: &str) {
        self.profile.insert(key.to_owned(), value.to_owned());
    }

    /// The slot the player has selected.
    pub fn set_selected_slot(&mut self, slot: usize) -> bool {
        if slot > crate::inventory::HOTBAR_LAST {
            return false;
        }
        self.selected_slot = slot;
        true
    }

    #[must_use]
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    pub fn inventory_mut(&mut self) -> &mut Inventory {
        &mut self.inventory
    }

    /// What the player is holding: the selected hotbar slot.
    #[must_use]
    pub fn held_item(&self) -> Option<ItemStack> {
        self.inventory.get_slot(self.selected_slot).ok().flatten()
    }

    /// The same, under the name the wishlist uses for it.
    #[must_use]
    pub fn selected_item(&self) -> Option<ItemStack> {
        self.held_item()
    }

    /// The armour the player is wearing, in slot order.
    #[must_use]
    pub fn armor(&self) -> Vec<Option<ItemStack>> {
        (ARMOUR_FIRST..=ARMOUR_LAST)
            .map(|slot| self.inventory.get_slot(slot).ok().flatten())
            .collect()
    }

    /// What is in the offhand.
    #[must_use]
    pub fn offhand(&self) -> Option<ItemStack> {
        self.inventory.get_slot(OFFHAND).ok().flatten()
    }

    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.health > 0.0
    }

    #[must_use]
    pub fn is_full_health(&self) -> bool {
        self.health >= self.max_health
    }

    /// How far this player is from a point.
    #[must_use]
    pub fn distance_to(&self, to: (f64, f64, f64)) -> f64 {
        crate::entities::distance(self.position, to)
    }

    /// Whether the player is in the given game mode.
    #[must_use]
    pub fn is_in_game_mode(&self, game_mode: GameMode) -> bool {
        self.game_mode == game_mode
    }

    /// Puts everything transient back the way a fresh spawn would be.
    pub fn reset_state(&mut self) {
        self.velocity = (0.0, 0.0, 0.0);
        self.effects.clear();
        self.flying = false;
        self.sprinting = false;
        self.sneaking = false;
        self.invisible = false;
    }
}

impl fmt::Display for PlayerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}, {:.0}/{:.0} health)",
            self.display_name,
            self.game_mode,
            self.locale,
            self.health,
            self.max_health
        )
    }
}

/// Why a question about the local player has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "local" => {
            "the local player is the client's own state, which this host has no handle on"
                .to_owned()
        }
        "identity" => "a player's name and uuid come from the server, by way of the client".to_owned(),
        "position" => "where the player is comes from the client's own interpolation".to_owned(),
        "health" => "health and food are applied by the client from what the server sends".to_owned(),
        "inventory" => "the player's inventory is the client's; section 18 holds a mod's own copy".to_owned(),
        "effects" => "status effects are the client's, applied from the server's packets".to_owned(),
        "connection" => "latency, ping and the network state belong to the client's connection".to_owned(),
        "appearance" => "skins and capes are the client's, from the server's player list".to_owned(),
        "input" => "what the player is pressing is the client's input".to_owned(),
        "camera" => "the camera is the client's own".to_owned(),
        "chat" => "this host has no chat: send_message writes to the loader's log, which is all it can do".to_owned(),
        "title" => "a title is drawn by the client's screen layer, which a mod host cannot reach".to_owned(),
        "sound" => "a sound is played by the client's audio, which a mod host cannot reach".to_owned(),
        "particle" => "a particle is drawn by the client's renderer, which a mod host cannot reach".to_owned(),
        "screen" => "opening a screen needs the client's UI, which a mod host cannot reach".to_owned(),
        "authority" => {
            "game mode, effects, items and teleports are the server's; this host cannot even \
             ask the client to send them"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The live player half: every one of these is the client's, and answers honestly.
pub mod known_only_to_the_client {
    /// The local player. `None`.
    #[must_use]
    pub fn get_local() -> Option<super::PlayerState> {
        None
    }

    /// Every player. Empty.
    #[must_use]
    pub fn get_all() -> Vec<super::PlayerState> {
        Vec::new()
    }

    /// A player by id. `None`.
    #[must_use]
    pub fn get_by_id(_id: &str) -> Option<super::PlayerState> {
        None
    }

    /// A player by uuid. `None`.
    #[must_use]
    pub fn get_by_uuid(_uuid: &str) -> Option<super::PlayerState> {
        None
    }

    /// A player by name. `None`.
    #[must_use]
    pub fn get_by_name(_name: &str) -> Option<super::PlayerState> {
        None
    }

    /// The player's connection. `None`.
    #[must_use]
    pub fn get_connection() -> Option<String> {
        None
    }

    /// How long the connection takes, in milliseconds. `None`.
    #[must_use]
    pub fn get_latency() -> Option<u32> {
        None
    }

    /// The same, under the name the wishlist also uses. `None`.
    #[must_use]
    pub fn get_ping() -> Option<u32> {
        None
    }

    /// The state of the connection. `None`.
    #[must_use]
    pub fn get_network_state() -> Option<String> {
        None
    }

    /// What the player is pressing. `None`.
    #[must_use]
    pub fn get_input_state() -> Option<String> {
        None
    }

    /// The camera. `None`.
    #[must_use]
    pub fn get_camera() -> Option<String> {
        None
    }

    /// The player's skin. `None`.
    #[must_use]
    pub fn get_skin() -> Option<String> {
        None
    }

    /// The player's cape. `None`.
    #[must_use]
    pub fn get_cape() -> Option<String> {
        None
    }

    /// The player's profile. Empty.
    #[must_use]
    pub fn get_profile() -> Vec<(String, String)> {
        Vec::new()
    }

    /// Everything the client would have to answer, with the reason.
    #[must_use]
    pub fn refusals() -> Vec<(String, String)> {
        [
            "local",
            "identity",
            "position",
            "health",
            "inventory",
            "effects",
            "connection",
            "appearance",
            "input",
            "camera",
            "chat",
            "title",
            "sound",
            "particle",
            "screen",
            "authority",
        ]
        .iter()
        .map(|what| ((*what).to_owned(), super::why_unknown(what)))
        .collect()
    }
}

/// The local player, which the client owns. `None`, under the name the wishlist uses.
#[must_use]
pub fn get_local() -> Option<PlayerState> {
    None
}

/// What this host cannot reach about players.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    known_only_to_the_client::refusals()
}

fn nope(what: &str) -> bool {
    crate::warn!("a mod asked this host about the local player: {}", why_unknown(what));
    false
}

/// Writes a line to the loader's log. This is not the game's chat — see [`why_unknown`] for
/// `chat` — and it is the closest a mod host gets to saying something.
pub fn send_message(text: &str) -> bool {
    crate::info!("[chat] {text}");
    true
}

/// Shows a line on the loader's on-screen overlay, which is what this host has instead of an
/// action bar.
pub fn send_action_bar(text: &str) -> bool {
    crate::overlay_line(text);
    true
}

/// A title. `false`: the client's screen layer draws those.
pub fn send_title(_title: &str, _subtitle: &str) -> bool {
    nope("title")
}

/// Plays a sound. `false`: the client's audio does.
pub fn play_sound(_sound: &str) -> bool {
    nope("sound")
}

/// Shows a particle. `false`: the client's renderer does.
pub fn show_particle(_particle: &str, _x: f64, _y: f64, _z: f64) -> bool {
    nope("particle")
}

/// Opens a screen. `false`: `CustomScreens` is `never` in section 10.
pub fn open_screen(_screen: &str) -> bool {
    nope("screen")
}

/// Closes a screen. `false`.
pub fn close_screen() -> bool {
    nope("screen")
}

/// Gives the player an item. `false`: the server decides what a player holds.
pub fn give_item(_stack: &ItemStack) -> bool {
    nope("authority")
}

/// Takes an item away. `false`.
pub fn take_item(_item: &str, _count: u32) -> bool {
    nope("authority")
}

/// Applies an effect. `false`.
pub fn apply_effect(_effect: Effect) -> bool {
    nope("authority")
}

/// Removes an effect. `false`.
pub fn remove_effect(_name: &str) -> bool {
    nope("authority")
}

/// Moves the player. `false`.
pub fn teleport(_x: f64, _y: f64, _z: f64) -> bool {
    nope("authority")
}

/// Sets the game mode. `false`: a server decides that.
pub fn set_game_mode(_game_mode: GameMode) -> bool {
    nope("authority")
}

/// The language the loader is speaking, which is what a mod's own translations use.
#[must_use]
pub fn get_locale() -> String {
    crate::i18n::get_current_language()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn a_player_state_is_a_value_a_mod_can_reason_about() {
        let _serial = serial();
        let mut player = PlayerState::new("Tester")
            .with_uuid("0000-1111")
            .with_game_mode(GameMode::Creative)
            .at(10.0, 70.0, -5.0);
        assert_eq!(player.name, "Tester");
        assert!(player.is_alive());
        assert!(player.is_full_health());
        assert_eq!(player.distance_to((13.0, 74.0, -5.0)), 5.0);
        assert!(player.is_in_game_mode(GameMode::Creative));
        player.set_health(4.0);
        assert!(player.is_alive());
        assert!(!player.is_full_health());
        player.set_health(-5.0);
        assert_eq!(player.health, 0.0);
        assert!(!player.is_alive());
        player.set_max_health(10.0);
        assert_eq!(player.max_health, 10.0);
        player.set_health(20.0);
        assert_eq!(player.health, 10.0, "clamped to the maximum");
        player.set_food_level(30);
        assert_eq!(player.food_level, 20);
        player.set_saturation(-1.0);
        assert_eq!(player.saturation, 0.0);
        player.set_experience(2.0);
        assert_eq!(player.experience, 1.0);
        player.set_level(7);
        player.set_score(42);
        player.set_locale("de_de");
        player.set_team(Some("red"));
        let rendered = player.to_string();
        assert!(rendered.contains("de_de"), "{rendered}");
        assert!(rendered.contains("Tester"), "{rendered}");
        assert!(rendered.contains("creative"), "{rendered}");
    }

    #[test]
    fn the_inventory_is_the_one_section_eighteen_defines() {
        let mut player = PlayerState::new("Tester");
        player
            .inventory_mut()
            .set_slot(0, Some(ItemStack::new("minecraft:torch", 64)))
            .unwrap();
        assert_eq!(player.held_item().map(|stack| stack.count()), Some(64));
        assert_eq!(player.selected_item().map(|stack| stack.count()), Some(64));
        assert!(player.set_selected_slot(3));
        assert!(player.held_item().is_none(), "slot three is empty");
        assert!(!player.set_selected_slot(40), "that is not a hotbar slot");
        player
            .inventory_mut()
            .set_slot(ARMOUR_FIRST, Some(ItemStack::new("minecraft:diamond_boots", 1)))
            .unwrap();
        player
            .inventory_mut()
            .set_slot(OFFHAND, Some(ItemStack::new("minecraft:shield", 1)))
            .unwrap();
        let armor = player.armor();
        assert_eq!(armor.len(), 4);
        assert!(armor[0].is_some());
        assert_eq!(player.offhand().map(|stack| stack.item().to_owned()), Some("minecraft:shield".to_owned()));
    }

    #[test]
    fn effects_permissions_and_advancements_are_the_mods_own_data() {
        let mut player = PlayerState::new("Tester");
        player.apply_effect(Effect::new("minecraft:speed", 1, 1_000));
        assert_eq!(player.effects.len(), 1);
        assert_eq!(player.effects[0].level(), 2);
        assert_eq!(player.active_effects(500).len(), 1);
        assert!(player.active_effects(1_500).is_empty(), "it ran out");
        player.apply_effect(Effect::new("minecraft:speed", 0, 5_000));
        assert_eq!(player.effects.len(), 1, "a second one of the same name replaces it");
        assert!(player.remove_effect("minecraft:speed"));
        assert!(!player.remove_effect("minecraft:speed"));

        player.grant(PlayerPermission::Build);
        player.grant(PlayerPermission::Build);
        assert_eq!(player.permissions.len(), 1);
        assert!(player.has_permission(PlayerPermission::Build));
        assert!(player.revoke(PlayerPermission::Build));
        assert!(!player.has_permission(PlayerPermission::Build));

        assert!(player.unlock_advancement("minecraft:story/root"));
        assert!(!player.unlock_advancement("minecraft:story/root"));
        player.statistics.set("walk_one_cm", 10);
        assert_eq!(player.statistics.add("walk_one_cm", 5), 15);
        assert_eq!(player.statistics.get("walk_one_cm"), Some(15));
        assert_eq!(player.statistics.get("nothing"), None);
        player.set_profile("xbox_id", "abc");
        assert_eq!(player.profile.get("xbox_id").map(String::as_str), Some("abc"));
    }

    #[test]
    fn resetting_clears_what_a_respawn_would() {
        let mut player = PlayerState::new("Tester");
        player.set_velocity(1.0, 2.0, 3.0);
        player.set_flying(true);
        player.set_sprinting(true);
        player.set_sneaking(true);
        player.set_invisible(true);
        player.apply_effect(Effect::new("minecraft:speed", 0, 100));
        player.reset_state();
        assert_eq!(player.velocity, (0.0, 0.0, 0.0));
        assert!(!player.flying);
        assert!(!player.sprinting);
        assert!(!player.sneaking);
        assert!(!player.invisible);
        assert!(player.effects.is_empty());
    }

    #[test]
    fn the_local_player_is_refused_with_a_reason() {
        let _serial = serial();
        assert!(known_only_to_the_client::get_local().is_none());
        assert!(known_only_to_the_client::get_all().is_empty());
        assert!(known_only_to_the_client::get_by_name("Tester").is_none());
        assert!(known_only_to_the_client::get_connection().is_none());
        assert!(known_only_to_the_client::get_latency().is_none());
        assert!(known_only_to_the_client::get_ping().is_none());
        assert!(known_only_to_the_client::get_camera().is_none());
        assert!(known_only_to_the_client::get_skin().is_none());
        assert_eq!(refusals().len(), 16);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_two_things_this_host_can_say_are_the_log_and_the_overlay() {
        let _serial = serial();
        assert!(send_message("hello from a mod"));
        assert!(send_action_bar("hello on the overlay"));
        assert!(!send_title("big", "small"));
        assert!(!play_sound("minecraft:click"));
        assert!(!show_particle("minecraft:spark", 0.0, 64.0, 0.0));
        assert!(!open_screen("inventory"));
        assert!(!close_screen());
        assert!(!give_item(&ItemStack::new("minecraft:stone", 1)));
        assert!(!take_item("minecraft:stone", 1));
        assert!(!apply_effect(Effect::new("minecraft:speed", 0, 100)));
        assert!(!remove_effect("minecraft:speed"));
        assert!(!teleport(0.0, 64.0, 0.0));
        assert!(!set_game_mode(GameMode::Creative));
    }

    #[test]
    fn the_locale_is_the_one_the_loader_speaks() {
        let _serial = serial();
        assert_eq!(get_locale(), crate::i18n::get_current_language());
        assert_eq!(GameMode::Adventure.as_str(), "adventure");
        assert_eq!(PlayerPermission::Fly.as_str(), "fly");
    }
}
