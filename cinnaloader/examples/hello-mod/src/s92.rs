//! Section 92 demo: the server model a mod owns, and the client's server refused.

use cinnaloader::commands::{self, Argument, ArgumentType, Command};
use cinnaloader::server::{self, ServerState};

pub fn run() -> Vec<String> {
    server::reset();
    let mut out = Vec::new();

    server::configure("Cinnaloader demo", 2);
    let _ = server::start(1_000);
    let _ = server::transition(ServerState::Running);
    let refused = server::transition(ServerState::Starting).unwrap_err();
    out.push(format!(
        "state: is_running={} refused={} ({} -> {})",
        server::is_running(),
        refused,
        refused.from,
        refused.to
    ));

    let first = server::join("Alex", 1_100).is_ok();
    let dup = server::join("Alex", 1_200).unwrap_err();
    let _ = server::join("Steve", 1_300);
    let full = server::join("Herobrine", 1_400).unwrap_err();
    out.push(format!(
        "players: first_join={first} duplicate={dup} full={full} count={}",
        server::get_online_player_count()
    ));
    let players = server::get_players();
    let order: Vec<&str> = players.iter().map(|player| player.name.as_str()).collect();
    out.push(format!("players: join order = {order:?}"));

    let _ = server::add_world("overworld", 42, "minecraft:noise");
    let _ = server::add_world("nether", -7, "minecraft:hell");
    let selected = server::select_world("nether");
    let duplicate = server::add_world("nether", 0, "flat");
    out.push(format!(
        "world: selected={selected} current={:?} duplicate_refused={}",
        server::current_world().map(|world| world.name),
        !duplicate
    ));

    let fast = server::advance_ticks(20, 1_000);
    out.push(format!("tick: tps={fast} lagging={}", server::is_lagging()));
    let slow = server::advance_ticks(20, 2_000);
    out.push(format!(
        "tick: tps={slow} lagging={} total_ticks={}",
        server::is_lagging(),
        server::get_tick_count()
    ));

    commands::register(
        Command::new("s92:greet", "greet").argument(Argument::new("who", ArgumentType::Word)),
        |call| Ok(format!("hello, {}", call.text("who").unwrap_or("world"))),
    );
    let known = server::execute_command("/s92:greet Steve")
        .map(|result| result.output)
        .unwrap_or_default();
    let unknown = server::execute_command("/nope").unwrap_err().to_string();
    out.push(format!("console: greet={known:?} unknown={unknown:?}"));

    out.push(format!(
        "refused: connected={} address={:?} players={:?}",
        server::is_connected(),
        server::get_address(),
        server::get_client_players()
    ));
    out.push(format!("refused: {}", server::why_unknown("connection")));

    out
}
