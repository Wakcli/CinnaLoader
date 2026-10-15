//! A server a mod runs: its life, its players, its worlds, its ticks and its console, and the
//! client's connection to *its* server, which is not here.
//!
//! The client holds one connection to its own server inside the client, and this host has no
//! handle on it — its address, its player list and whether it is connected are refused with a
//! sentence each. What is real is the **model** of a server a mod owns: a [`ServerState`]
//! machine that refuses an illegal transition by naming both states, a player registry that
//! refuses a duplicate name and a full server, a world registry with seeds and generators, a
//! tick model whose TPS is computed from the duration it is handed, a console that runs
//! commands through [`crate::commands`], and a ban and kick list that keep reasons.
//!
//! ```
//! use cinnaloader::commands::{self, Argument, ArgumentType, Command};
//! use cinnaloader::server::{self, ServerState};
//!
//! server::reset();
//! server::configure("A mod server", 2);
//! assert!(server::start(1_000).is_ok());
//! assert_eq!(server::transition(ServerState::Running), Ok(ServerState::Running));
//! assert!(server::is_running());
//! assert!(server::join("Steve", 2_000).is_ok());
//! assert_eq!(server::get_online_player_count(), 1);
//! assert!(server::add_world("overworld", 42, "minecraft:noise"));
//! assert!(server::select_world("overworld"));
//! assert_eq!(server::current_world().map(|world| world.name), Some("overworld".to_owned()));
//! assert_eq!(server::advance_ticks(20, 1_000), 20.0);
//! assert!(!server::is_lagging());
//! assert_eq!(server::advance_ticks(20, 2_000), 10.0);
//! assert!(server::is_lagging());
//! commands::register(
//!     Command::new("s92:greet", "greet").argument(Argument::new("who", ArgumentType::Word)),
//!     |call| Ok(format!("hello, {}", call.text("who").unwrap_or("world"))),
//! );
//! assert_eq!(server::execute_command("/s92:greet Steve").expect("a command").output, "hello, Steve");
//! assert!(!server::is_connected(), "the client's connection is not this host's");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::commands::{self, Command, CommandCall, CommandError, CommandResult};

/// The capacity a server has before anyone says otherwise.
pub const DEFAULT_MAX_PLAYERS: usize = 20;
/// The message of the day a server has before anyone says otherwise.
pub const DEFAULT_MOTD: &str = "A Cinnaloader server";
/// Below this TPS the server is called lagging.
pub const LAG_THRESHOLD: f64 = 20.0;

/// Where a server is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ServerState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
}

impl ServerState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
        }
    }

    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// The states this one may move to. Nothing else is allowed.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Stopped, Self::Starting)
                | (Self::Starting, Self::Running)
                | (Self::Starting, Self::Stopped)
                | (Self::Running, Self::Stopping)
                | (Self::Stopping, Self::Stopped)
        )
    }
}

impl fmt::Display for ServerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A transition the machine will not make, naming both states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionRefused {
    pub from: ServerState,
    pub to: ServerState,
}

impl fmt::Display for TransitionRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "a server cannot go from {} to {}", self.from, self.to)
    }
}

impl std::error::Error for TransitionRefused {}

/// One transition the machine made, for the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub from: ServerState,
    pub to: ServerState,
}

/// Why a join did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinRefused {
    Duplicate(String),
    Full { max: usize },
}

impl fmt::Display for JoinRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(name) => write!(formatter, "{name} is already on the server"),
            Self::Full { max } => write!(formatter, "the server is full at {max} players"),
        }
    }
}

impl std::error::Error for JoinRefused {}

/// A player who is on the server right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub name: String,
    pub joined_millis: u64,
}

/// One stay on the server, open until the player leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub name: String,
    pub joined_millis: u64,
    pub left_millis: Option<u64>,
}

/// What is known about a player, online or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerData {
    pub name: String,
    pub online: bool,
    pub joined_millis: Option<u64>,
    pub sessions: usize,
}

/// A world a mod declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct World {
    pub name: String,
    pub seed: i64,
    pub generator: String,
}

/// A world with the one question the registry can answer about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldData {
    pub name: String,
    pub seed: i64,
    pub generator: String,
    pub current: bool,
}

/// A player who is not welcome back, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ban {
    pub name: String,
    pub reason: String,
}

/// A player who was removed while online, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kick {
    pub name: String,
    pub reason: String,
}

/// What a server is set to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerProperties {
    pub motd: String,
    pub max_players: usize,
}

impl Default for ServerProperties {
    fn default() -> Self {
        Self {
            motd: DEFAULT_MOTD.to_owned(),
            max_players: DEFAULT_MAX_PLAYERS,
        }
    }
}

/// A snapshot of the server a mod runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub state: ServerState,
    pub motd: String,
    pub max_players: usize,
    pub online_players: usize,
    pub current_world: Option<String>,
    pub tick_count: u64,
    pub tps: f64,
    pub start_millis: Option<u64>,
}

#[derive(Default)]
struct Held {
    state: ServerState,
    exists: bool,
    configured: ServerProperties,
    properties: ServerProperties,
    start_millis: Option<u64>,
    transitions: Vec<Transition>,
    players: Vec<Player>,
    sessions: Vec<Session>,
    worlds: Vec<World>,
    current: Option<String>,
    bans: Vec<Ban>,
    kicks: Vec<Kick>,
    whitelist: Vec<String>,
    listeners: Vec<String>,
    plugin_messages: Vec<String>,
    permissions: BTreeMap<String, u32>,
    tick_count: u64,
    tps: f64,
    saves: u64,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn lock() -> MutexGuard<'static, Held> {
    held().lock().unwrap_or_else(|error| error.into_inner())
}

/// Empties the model, so a test or a demo starts from nothing.
pub fn reset() {
    *lock() = Held::default();
}

fn transition_to(next: ServerState) -> Result<ServerState, TransitionRefused> {
    let mut held = lock();
    if !held.state.can_transition_to(next) {
        return Err(TransitionRefused {
            from: held.state,
            to: next,
        });
    }
    let from = held.state;
    held.state = next;
    held.transitions.push(Transition { from, to: next });
    Ok(next)
}

/// Moves the machine to `next`, or refuses naming both states.
pub fn transition(next: ServerState) -> Result<ServerState, TransitionRefused> {
    transition_to(next)
}

/// Sets the server up: its message of the day and its capacity.
pub fn configure(motd: &str, max_players: usize) {
    let mut held = lock();
    let properties = ServerProperties {
        motd: motd.to_owned(),
        max_players,
    };
    held.configured = properties.clone();
    held.properties = properties;
    held.exists = true;
}

/// Begins a start, from `stopped`, recording `at_millis` as the start time.
pub fn start(at_millis: u64) -> Result<ServerState, TransitionRefused> {
    let state = transition_to(ServerState::Starting)?;
    let mut held = lock();
    held.start_millis = Some(at_millis);
    held.exists = true;
    Ok(state)
}

/// Finishes a start, from `starting` to `running`.
pub fn finish_start() -> Result<ServerState, TransitionRefused> {
    transition_to(ServerState::Running)
}

/// Asks the server to stop, from `running` to `stopping`.
pub fn stop() -> Result<ServerState, TransitionRefused> {
    transition_to(ServerState::Stopping)
}

/// Finishes a stop, from `stopping` to `stopped`.
pub fn finish_stop() -> Result<ServerState, TransitionRefused> {
    transition_to(ServerState::Stopped)
}

/// Where the server is now.
#[must_use]
pub fn get_status() -> ServerState {
    lock().state
}

/// The state, same as [`get_status`].
#[must_use]
pub fn state() -> ServerState {
    lock().state
}

/// Whether the server is running.
#[must_use]
pub fn is_running() -> bool {
    lock().state.is_running()
}

/// Every transition the machine made, oldest first.
#[must_use]
pub fn transition_history() -> Vec<Transition> {
    lock().transitions.clone()
}

/// The server, once one exists.
#[must_use]
pub fn get() -> Option<Server> {
    let held = lock();
    if !held.exists {
        return None;
    }
    Some(Server {
        state: held.state,
        motd: held.properties.motd.clone(),
        max_players: held.properties.max_players,
        online_players: held.players.len(),
        current_world: held.current.clone(),
        tick_count: held.tick_count,
        tps: held.tps,
        start_millis: held.start_millis,
    })
}

/// The server's settings.
#[must_use]
pub fn get_properties() -> ServerProperties {
    lock().properties.clone()
}

/// The message of the day.
#[must_use]
pub fn get_motd() -> String {
    lock().properties.motd.clone()
}

/// How many players fit.
#[must_use]
pub fn get_max_players() -> usize {
    lock().properties.max_players
}

/// When the server started, from the value it was handed.
#[must_use]
pub fn get_start_time() -> Option<u64> {
    lock().start_millis
}

/// How long it has been up at `now_millis`, zero before it started.
#[must_use]
pub fn get_uptime(now_millis: u64) -> u64 {
    match lock().start_millis {
        Some(start) => now_millis.saturating_sub(start),
        None => 0,
    }
}

/// Puts a player on the server. A name already there, or a full server, is refused.
pub fn join(name: &str, at_millis: u64) -> Result<(), JoinRefused> {
    let mut held = lock();
    if held.players.iter().any(|player| player.name == name) {
        return Err(JoinRefused::Duplicate(name.to_owned()));
    }
    if held.players.len() >= held.properties.max_players {
        return Err(JoinRefused::Full {
            max: held.properties.max_players,
        });
    }
    held.players.push(Player {
        name: name.to_owned(),
        joined_millis: at_millis,
    });
    held.sessions.push(Session {
        name: name.to_owned(),
        joined_millis: at_millis,
        left_millis: None,
    });
    Ok(())
}

/// Takes a player off the server at `at_millis`, closing their stay.
pub fn leave(name: &str, at_millis: u64) -> bool {
    let mut held = lock();
    let Some(pos) = held.players.iter().position(|player| player.name == name) else {
        return false;
    };
    held.players.remove(pos);
    if let Some(session) = held
        .sessions
        .iter_mut()
        .rev()
        .find(|session| session.name == name && session.left_millis.is_none())
    {
        session.left_millis = Some(at_millis);
    }
    true
}

/// Who is on the server, in join order.
#[must_use]
pub fn get_players() -> Vec<Player> {
    lock().players.clone()
}

/// One player by name.
#[must_use]
pub fn player(name: &str) -> Option<Player> {
    lock().players.iter().find(|player| player.name == name).cloned()
}

/// How many are on right now.
#[must_use]
pub fn get_online_player_count() -> usize {
    lock().players.len()
}

/// Whether a name has ever been on the server.
#[must_use]
pub fn has_played(name: &str) -> bool {
    lock().sessions.iter().any(|session| session.name == name)
}

/// Every stay on the server, in the order they began.
#[must_use]
pub fn get_sessions() -> Vec<Session> {
    lock().sessions.clone()
}

/// What is known about a player, online or not.
#[must_use]
pub fn get_player_data(name: &str) -> Option<PlayerData> {
    let held = lock();
    let sessions: Vec<&Session> = held
        .sessions
        .iter()
        .filter(|session| session.name == name)
        .collect();
    let online = held.players.iter().find(|player| player.name == name);
    if sessions.is_empty() && online.is_none() {
        return None;
    }
    Some(PlayerData {
        name: name.to_owned(),
        online: online.is_some(),
        joined_millis: online.map(|player| player.joined_millis),
        sessions: sessions.len(),
    })
}

/// Declares a world. A name already in the registry is refused.
pub fn add_world(name: &str, seed: i64, generator: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut held = lock();
    if held.worlds.iter().any(|world| world.name == name) {
        return false;
    }
    held.worlds.push(World {
        name: name.to_owned(),
        seed,
        generator: generator.to_owned(),
    });
    true
}

/// Forgets a world, and clears it as current if it was.
pub fn remove_world(name: &str) -> bool {
    let mut held = lock();
    let before = held.worlds.len();
    held.worlds.retain(|world| world.name != name);
    if held.worlds.len() == before {
        return false;
    }
    if held.current.as_deref() == Some(name) {
        held.current = None;
    }
    true
}

/// Makes a declared world the current one.
pub fn select_world(name: &str) -> bool {
    let mut held = lock();
    if !held.worlds.iter().any(|world| world.name == name) {
        return false;
    }
    held.current = Some(name.to_owned());
    true
}

/// The world that is selected right now.
#[must_use]
pub fn current_world() -> Option<World> {
    let held = lock();
    let name = held.current.as_ref()?;
    held.worlds.iter().find(|world| &world.name == name).cloned()
}

/// Every declared world, in the order they were added.
#[must_use]
pub fn get_worlds() -> Vec<World> {
    lock().worlds.clone()
}

/// A world by name, with whether it is the current one.
#[must_use]
pub fn get_world_data(name: &str) -> Option<WorldData> {
    let held = lock();
    held.worlds
        .iter()
        .find(|world| world.name == name)
        .map(|world| WorldData {
            name: world.name.clone(),
            seed: world.seed,
            generator: world.generator.clone(),
            current: held.current.as_deref() == Some(name),
        })
}

/// Advances the tick counter by `ticks` over `elapsed_millis`, and answers the TPS.
pub fn advance_ticks(ticks: u64, elapsed_millis: u64) -> f64 {
    let mut held = lock();
    held.tick_count += ticks;
    held.tps = if elapsed_millis == 0 {
        0.0
    } else {
        ticks as f64 * 1000.0 / elapsed_millis as f64
    };
    held.tps
}

/// The TPS from the last advance.
#[must_use]
pub fn get_tick_rate() -> f64 {
    lock().tps
}

/// How many ticks have advanced.
#[must_use]
pub fn get_tick_count() -> u64 {
    lock().tick_count
}

/// Whether the last tick rate is below [`LAG_THRESHOLD`].
#[must_use]
pub fn is_lagging() -> bool {
    let held = lock();
    held.tick_count > 0 && held.tps < LAG_THRESHOLD
}

/// Declares a command on the mod's own dispatcher.
pub fn register_command<F>(command: Command, executor: F) -> bool
where
    F: FnMut(&CommandCall) -> Result<String, String> + Send + 'static,
{
    commands::register(command, executor)
}

/// Runs a command line on the mod's own dispatcher.
pub fn execute_command(line: &str) -> Result<CommandResult, CommandError> {
    commands::execute(line)
}

/// Sends a message to every online player, and answers how many took it.
#[must_use]
pub fn broadcast_message(_message: &str) -> usize {
    lock().players.len()
}

/// Removes an online player at `at_millis` and records the reason.
pub fn kick_player(name: &str, reason: &str, at_millis: u64) -> bool {
    if !leave(name, at_millis) {
        return false;
    }
    lock().kicks.push(Kick {
        name: name.to_owned(),
        reason: reason.to_owned(),
    });
    true
}

/// Every kick, in the order they happened.
#[must_use]
pub fn get_kicks() -> Vec<Kick> {
    lock().kicks.clone()
}

/// How many have been kicked.
#[must_use]
pub fn kick_count() -> usize {
    lock().kicks.len()
}

/// Bans a name with a reason. A name already banned is refused.
pub fn ban_player(name: &str, reason: &str) -> bool {
    let mut held = lock();
    if held.bans.iter().any(|ban| ban.name == name) {
        return false;
    }
    held.bans.push(Ban {
        name: name.to_owned(),
        reason: reason.to_owned(),
    });
    true
}

/// Lifts a ban.
pub fn unban_player(name: &str) -> bool {
    let mut held = lock();
    let before = held.bans.len();
    held.bans.retain(|ban| ban.name != name);
    held.bans.len() != before
}

/// Whether a name is banned.
#[must_use]
pub fn is_banned(name: &str) -> bool {
    lock().bans.iter().any(|ban| ban.name == name)
}

/// Every ban, with its reason.
#[must_use]
pub fn get_bans() -> Vec<Ban> {
    lock().bans.clone()
}

/// How many are banned.
#[must_use]
pub fn ban_count() -> usize {
    lock().bans.len()
}

/// Adds a name to the whitelist.
pub fn whitelist_player(name: &str) -> bool {
    let mut held = lock();
    if held.whitelist.iter().any(|held| held == name) {
        return false;
    }
    held.whitelist.push(name.to_owned());
    true
}

/// Removes a name from it.
pub fn remove_from_whitelist(name: &str) -> bool {
    let mut held = lock();
    let before = held.whitelist.len();
    held.whitelist.retain(|held| held != name);
    held.whitelist.len() != before
}

/// Whether a name is on it.
#[must_use]
pub fn is_whitelisted(name: &str) -> bool {
    lock().whitelist.iter().any(|held| held == name)
}

/// Everyone on it, in the order they were added.
#[must_use]
pub fn get_whitelist() -> Vec<String> {
    lock().whitelist.clone()
}

/// Registers interest in an event name, answering whether it was new.
pub fn register_listener(event: &str) -> bool {
    if event.is_empty() {
        return false;
    }
    let mut held = lock();
    if held.listeners.iter().any(|held| held == event) {
        return false;
    }
    held.listeners.push(event.to_owned());
    true
}

/// Every event name a listener is on.
#[must_use]
pub fn get_listeners() -> Vec<String> {
    lock().listeners.clone()
}

/// Declares a plugin-message channel the server would serve.
pub fn register_plugin_message(channel: &str) -> bool {
    if channel.is_empty() {
        return false;
    }
    let mut held = lock();
    if held.plugin_messages.iter().any(|held| held == channel) {
        return false;
    }
    held.plugin_messages.push(channel.to_owned());
    true
}

/// Every plugin-message channel declared.
#[must_use]
pub fn get_plugin_messages() -> Vec<String> {
    lock().plugin_messages.clone()
}

/// Flushes the model's worlds to their store, and answers how many there were.
#[must_use]
pub fn save_all() -> usize {
    let mut held = lock();
    held.saves += 1;
    held.worlds.len()
}

/// How many times the worlds have been saved.
#[must_use]
pub fn get_save_count() -> u64 {
    lock().saves
}

/// Re-applies the configured settings and clears the tick window. `false` when no server.
pub fn reload() -> bool {
    let mut held = lock();
    if !held.exists {
        return false;
    }
    held.properties = held.configured.clone();
    held.tick_count = 0;
    held.tps = 0.0;
    true
}

/// The permission level a name holds here, zero when it holds none.
#[must_use]
pub fn get_permissions(name: &str) -> u32 {
    lock().permissions.get(name).copied().unwrap_or(0)
}

/// Grants a name a permission level.
pub fn set_permissions(name: &str, level: u32) {
    lock().permissions.insert(name.to_owned(), level);
}

/// Why a question about the client's own server has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "connection" => {
            "the client's connection to its server lives inside the client's network manager, \
             and this host holds no handle on it"
                .to_owned()
        }
        "address" => {
            "the address of the server the client is connected to is chosen and stored by the \
             client, and this host cannot read it"
                .to_owned()
        }
        "players" => {
            "the client keeps its own view of the players on its server, and this host cannot \
             read that list"
                .to_owned()
        }
        "server" => {
            "the game's server is not started or owned by a mod host; only its own model of one \
             is real here"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's server.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["connection", "address", "players", "server"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused<T: Default>(what: &str) -> T {
    crate::warn!(
        "a mod asked this host about the client's server: {}",
        why_unknown(what)
    );
    T::default()
}

/// Whether the client is connected to its server. `false`.
#[must_use]
pub fn is_connected() -> bool {
    refused("connection")
}

/// The address of the client's server. `None`.
#[must_use]
pub fn get_address() -> Option<String> {
    refused("address")
}

/// The players the client sees on its server. Empty.
#[must_use]
pub fn get_client_players() -> Vec<String> {
    refused("players")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{self, Argument, ArgumentType, Command};

    fn serial() -> MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn the_state_machine_refuses_an_illegal_move_and_keeps_history() {
        let _serial = serial();
        reset();
        configure("test", 8);
        assert_eq!(get_status(), ServerState::Stopped);
        assert!(start(1_000).is_ok());
        assert_eq!(finish_start(), Ok(ServerState::Running));
        assert!(is_running());

        let refused = transition(ServerState::Starting).unwrap_err();
        assert_eq!(refused.from, ServerState::Running);
        assert_eq!(refused.to, ServerState::Starting);
        assert_eq!(refused.to_string(), "a server cannot go from running to starting");
        assert_eq!(get_status(), ServerState::Running, "a refusal changes nothing");

        assert_eq!(stop(), Ok(ServerState::Stopping));
        assert_eq!(finish_stop(), Ok(ServerState::Stopped));
        assert_eq!(transition(ServerState::Running).unwrap_err().from, ServerState::Stopped);
        assert_eq!(
            transition_history(),
            vec![
                Transition { from: ServerState::Stopped, to: ServerState::Starting },
                Transition { from: ServerState::Starting, to: ServerState::Running },
                Transition { from: ServerState::Running, to: ServerState::Stopping },
                Transition { from: ServerState::Stopping, to: ServerState::Stopped },
            ]
        );
        assert_eq!(get_start_time(), Some(1_000));
        assert_eq!(get_uptime(1_500), 500);
    }

    #[test]
    fn a_duplicate_join_and_a_full_server_are_refused() {
        let _serial = serial();
        reset();
        configure("test", 2);
        assert!(join("Alex", 100).is_ok());
        assert_eq!(
            join("Alex", 200).unwrap_err(),
            JoinRefused::Duplicate("Alex".to_owned())
        );
        assert!(join("Steve", 300).is_ok());
        assert_eq!(get_online_player_count(), 2);
        assert_eq!(join("Herobrine", 400).unwrap_err(), JoinRefused::Full { max: 2 });
        assert_eq!(
            get_players().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["Alex", "Steve"]
        );
        assert_eq!(player("Steve").map(|p| p.joined_millis), Some(300));
        assert!(player("Nobody").is_none());
    }

    #[test]
    fn the_player_history_keeps_join_and_leave_times() {
        let _serial = serial();
        reset();
        configure("test", 8);
        assert!(join("Alex", 100).is_ok());
        assert!(leave("Alex", 250));
        assert!(!leave("Alex", 300), "already gone");
        assert!(join("Alex", 400).is_ok());
        assert!(has_played("Alex"));
        assert!(!has_played("Steve"));

        assert_eq!(
            get_sessions(),
            vec![
                Session { name: "Alex".to_owned(), joined_millis: 100, left_millis: Some(250) },
                Session { name: "Alex".to_owned(), joined_millis: 400, left_millis: None },
            ]
        );
        let data = get_player_data("Alex").expect("known");
        assert!(data.online);
        assert_eq!(data.joined_millis, Some(400));
        assert_eq!(data.sessions, 2);
        assert!(get_player_data("Nobody").is_none());
    }

    #[test]
    fn the_world_registry_selects_and_refuses_a_duplicate() {
        let _serial = serial();
        reset();
        assert!(add_world("overworld", 42, "minecraft:noise"));
        assert!(!add_world("overworld", 7, "minecraft:flat"), "a duplicate is refused");
        assert!(add_world("nether", -9, "minecraft:hell"));
        assert_eq!(get_worlds().len(), 2);
        assert!(current_world().is_none());

        assert!(select_world("nether"));
        assert_eq!(current_world().map(|world| world.name), Some("nether".to_owned()));
        assert_eq!(current_world().map(|world| world.seed), Some(-9));
        assert_eq!(
            get_world_data("overworld"),
            Some(WorldData {
                name: "overworld".to_owned(),
                seed: 42,
                generator: "minecraft:noise".to_owned(),
                current: false,
            })
        );
        assert!(!select_world("nothing"), "only a declared world can be selected");
        assert!(remove_world("nether"));
        assert!(current_world().is_none(), "the removed world is no longer current");
        assert!(!remove_world("nether"));
    }

    #[test]
    fn tps_is_computed_from_the_duration_it_is_handed() {
        let _serial = serial();
        reset();
        assert_eq!(advance_ticks(20, 1_000), 20.0);
        assert_eq!(get_tick_rate(), 20.0);
        assert!(!is_lagging(), "20.0 is not below the threshold");
        assert_eq!(advance_ticks(20, 2_000), 10.0);
        assert_eq!(get_tick_rate(), 10.0);
        assert!(is_lagging(), "10.0 is below the threshold");
        assert_eq!(get_tick_count(), 40);
        assert_eq!(advance_ticks(5, 0), 0.0, "no time means no rate");
        assert_eq!(get_tick_count(), 45);
    }

    #[test]
    fn the_console_runs_a_known_command_and_refuses_an_unknown_one() {
        let _serial = serial();
        reset();
        assert!(register_command(
            Command::new("s92:greet", "greet").argument(Argument::new("who", ArgumentType::Word)),
            |call| Ok(format!("hello, {}", call.text("who").unwrap_or("world"))),
        ));
        assert!(register_command(
            Command::new("s92:pardon", "pardon"),
            |_| Ok("pardoned".to_owned()),
        ));
        let result = execute_command("/s92:greet Steve").expect("a known command");
        assert_eq!(result.command, "s92:greet");
        assert_eq!(result.output, "hello, Steve");
        assert!(result.success);
        assert_eq!(
            execute_command("/nope"),
            Err(CommandError::UnknownCommand("/nope".to_owned()))
        );
        assert!(commands::unregister("s92:greet"));
        assert!(commands::unregister("s92:pardon"));
    }

    #[test]
    fn the_ban_and_kick_lists_hold_reasons_and_counts() {
        let _serial = serial();
        reset();
        configure("test", 4);
        assert!(ban_player("Herobrine", "griefing"));
        assert!(!ban_player("Herobrine", "again"), "already banned");
        assert!(ban_player("Griefer", "spam"));
        assert!(is_banned("Herobrine"));
        assert!(!is_banned("Alex"));
        assert_eq!(ban_count(), 2);
        assert_eq!(get_bans()[0].reason, "griefing");
        assert!(unban_player("Herobrine"));
        assert!(!unban_player("Herobrine"));
        assert_eq!(ban_count(), 1);

        assert!(join("Alex", 10).is_ok());
        assert!(kick_player("Alex", "afk", 90));
        assert!(!kick_player("Steve", "afk", 91), "not online");
        assert_eq!(kick_count(), 1);
        assert_eq!(get_kicks()[0].reason, "afk");
        assert_eq!(get_online_player_count(), 0);
        assert_eq!(get_sessions()[0].left_millis, Some(90));
    }

    #[test]
    fn the_client_server_connection_is_refused_with_its_own_reason() {
        let _serial = serial();
        reset();
        assert!(!is_connected());
        assert!(get_address().is_none());
        assert!(get_client_players().is_empty());
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
        }
        assert!(why_unknown("connection").contains("client"));
        assert!(!why_unknown("address").is_empty());
    }

    #[test]
    fn a_snapshot_and_the_auxiliary_registries_read_back() {
        let _serial = serial();
        reset();
        assert!(get().is_none(), "no server yet");
        configure("hello", 3);
        start(10).unwrap();
        finish_start().unwrap();
        join("Alex", 20).unwrap();
        add_world("overworld", 1, "flat");
        select_world("overworld");
        advance_ticks(20, 1_000);

        let server = get().expect("a server");
        assert_eq!(server.state, ServerState::Running);
        assert_eq!(server.motd, "hello");
        assert_eq!(server.max_players, 3);
        assert_eq!(server.online_players, 1);
        assert_eq!(server.current_world, Some("overworld".to_owned()));
        assert_eq!(server.tick_count, 20);
        assert_eq!(server.tps, 20.0);
        assert_eq!(get_motd(), "hello");
        assert_eq!(get_max_players(), 3);
        assert_eq!(get_properties().max_players, 3);

        assert_eq!(broadcast_message("hi"), 1);
        assert!(register_listener("player.join"));
        assert!(!register_listener("player.join"));
        assert_eq!(get_listeners(), vec!["player.join".to_owned()]);
        assert!(register_plugin_message("hello:state"));
        assert_eq!(get_plugin_messages(), vec!["hello:state".to_owned()]);
        assert_eq!(save_all(), 1);
        assert_eq!(get_save_count(), 1);
        assert!(reload());
        assert_eq!(get_tick_count(), 0);
        set_permissions("Alex", 2);
        assert_eq!(get_permissions("Alex"), 2);
        assert_eq!(get_permissions("Nobody"), 0);

        assert!(whitelist_player("Alex"));
        assert!(!whitelist_player("Alex"));
        assert!(is_whitelisted("Alex"));
        assert_eq!(get_whitelist(), vec!["Alex".to_owned()]);
        assert!(remove_from_whitelist("Alex"));
        assert!(!remove_from_whitelist("Alex"));
    }
}
