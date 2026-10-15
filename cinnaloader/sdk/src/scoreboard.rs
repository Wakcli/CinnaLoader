//! Scoreboards and teams: the whole board a mod owns, and the client's board it does not.
//!
//! The client's scoreboard lives inside the client, so this host builds the model instead:
//! named [`Objective`]s, each with a criterion, a display name and a render type; scores that
//! are set, added to, reset and removed, keyed by an entry name; a display slot per
//! objective; and [`Team`]s with a display name, a colour, a prefix and suffix, friendly-fire
//! and collision rules, name-tag and death-message visibility, and a member list. The
//! arithmetic is exact, `i64` and signed, so a score may go negative and pass back through
//! zero; [`get_scores`] sorts by score, highest first, and breaks ties by entry name, lowest
//! first. Adding a player who is already on another team moves it rather than duplicating it,
//! and [`get_team_members`] and [`team_of`] answer the same membership from either side.
//!
//! What is refused is the client: [`get`] binding its scoreboard, [`client_sidebar`] reading
//! the board it draws on screen, and [`client_team`] reading the team it has assigned, each
//! with its sentence in [`why_unknown`] and its line in [`refusals`]. Nothing here returns a
//! fake success for any of them.
//!
//! ```
//! use cinnaloader::scoreboard::{self, Objective, Team};
//!
//! scoreboard::create();
//! scoreboard::register_objective(Objective::new("hello:kills", "dummy"));
//! scoreboard::set_score("hello:kills", "steve", 3);
//! scoreboard::add_score("hello:kills", "steve", -5);
//! assert_eq!(scoreboard::get_score("hello:kills", "steve"), Some(-2));
//! scoreboard::register_team(Team::new("hello:red"));
//! assert!(scoreboard::add_player("hello:red", "steve"));
//! assert_eq!(scoreboard::get_team_members("hello:red"), vec!["steve".to_owned()]);
//! assert!(scoreboard::get().is_none(), "the client's board is the client's");
//! ```

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::chat::TextColor;

/// How an objective's scores are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RenderType {
    #[default]
    Integer,
    Hearts,
}

impl RenderType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Hearts => "hearts",
        }
    }
}

/// Where an objective's scores are shown on the client's board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplaySlot {
    List,
    Sidebar,
    BelowName,
}

impl DisplaySlot {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Sidebar => "sidebar",
            Self::BelowName => "below_name",
        }
    }
}

/// What happens when two members of a team push into each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CollisionRule {
    #[default]
    Always,
    Never,
    PushOtherTeams,
    PushOwnTeam,
}

impl CollisionRule {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::PushOtherTeams => "push_other_teams",
            Self::PushOwnTeam => "push_own_team",
        }
    }
}

/// Who can see a member's name tag or death message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Visibility {
    #[default]
    Always,
    HideForOtherTeams,
    HideForOwnTeam,
    Never,
}

impl Visibility {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::HideForOtherTeams => "hide_for_other_teams",
            Self::HideForOwnTeam => "hide_for_own_team",
            Self::Never => "never",
        }
    }
}

/// A named row of a scoreboard: what it counts, what it is called and how it is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Objective {
    pub name: String,
    pub owner: String,
    /// What has to happen for the objective to count, such as `dummy` or `deathCount`.
    pub criterion: String,
    pub display_name: String,
    pub render_type: RenderType,
    pub display_slot: Option<DisplaySlot>,
}

impl Objective {
    /// An objective with the defaults a caller would expect: an integer render, no display
    /// slot, and its name as its display name.
    #[must_use]
    pub fn new(name: &str, criterion: &str) -> Self {
        Self {
            name: name.to_owned(),
            owner: crate::metadata::id(),
            criterion: criterion.to_owned(),
            display_name: name.to_owned(),
            render_type: RenderType::Integer,
            display_slot: None,
        }
    }

    #[must_use]
    pub fn display_name(mut self, display_name: &str) -> Self {
        self.display_name = display_name.to_owned();
        self
    }

    #[must_use]
    pub fn render_type(mut self, render_type: RenderType) -> Self {
        self.render_type = render_type;
        self
    }

    #[must_use]
    pub fn display_slot(mut self, slot: DisplaySlot) -> Self {
        self.display_slot = Some(slot);
        self
    }
}

impl fmt::Display for Objective {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}) as {}",
            self.display_name,
            self.criterion,
            self.render_type.as_str()
        )
    }
}

/// A team: a display name, a colour, a prefix and suffix, its rules, and who is on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    pub name: String,
    pub owner: String,
    pub display_name: String,
    pub color: TextColor,
    pub prefix: String,
    pub suffix: String,
    pub friendly_fire: bool,
    pub collision_rule: CollisionRule,
    pub name_tag_visibility: Visibility,
    pub death_message_visibility: Visibility,
    pub members: Vec<String>,
}

impl Team {
    /// A team with the defaults a caller would expect: a white name, no decoration, no
    /// friendly fire, always colliding, and nothing hidden.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            owner: crate::metadata::id(),
            display_name: name.to_owned(),
            color: TextColor::named("white"),
            prefix: String::new(),
            suffix: String::new(),
            friendly_fire: false,
            collision_rule: CollisionRule::Always,
            name_tag_visibility: Visibility::Always,
            death_message_visibility: Visibility::Always,
            members: Vec::new(),
        }
    }

    #[must_use]
    pub fn display_name(mut self, display_name: &str) -> Self {
        self.display_name = display_name.to_owned();
        self
    }

    #[must_use]
    pub fn color(mut self, color: TextColor) -> Self {
        self.color = color;
        self
    }

    #[must_use]
    pub fn prefix(mut self, prefix: &str) -> Self {
        self.prefix = prefix.to_owned();
        self
    }

    #[must_use]
    pub fn suffix(mut self, suffix: &str) -> Self {
        self.suffix = suffix.to_owned();
        self
    }

    #[must_use]
    pub fn friendly_fire(mut self, friendly_fire: bool) -> Self {
        self.friendly_fire = friendly_fire;
        self
    }

    #[must_use]
    pub fn collision_rule(mut self, collision_rule: CollisionRule) -> Self {
        self.collision_rule = collision_rule;
        self
    }

    #[must_use]
    pub fn name_tag_visibility(mut self, visibility: Visibility) -> Self {
        self.name_tag_visibility = visibility;
        self
    }

    #[must_use]
    pub fn death_message_visibility(mut self, visibility: Visibility) -> Self {
        self.death_message_visibility = visibility;
        self
    }
}

impl fmt::Display for Team {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} '{}' {} {} member(s)",
            self.name,
            self.display_name,
            self.color.as_str(),
            self.members.len()
        )
    }
}

/// The whole board a mod owns: its objectives, their scores and its teams.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scoreboard {
    pub objectives: BTreeMap<String, Objective>,
    /// Every score, keyed by objective and entry.
    pub scores: BTreeMap<(String, String), i64>,
    pub teams: BTreeMap<String, Team>,
}

impl fmt::Display for Scoreboard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} objective(s), {} score(s), {} team(s)",
            self.objectives.len(),
            self.scores.len(),
            self.teams.len()
        )
    }
}

fn board() -> &'static Mutex<Scoreboard> {
    static BOARD: OnceLock<Mutex<Scoreboard>> = OnceLock::new();
    BOARD.get_or_init(|| Mutex::new(Scoreboard::default()))
}

fn with<T>(body: impl FnOnce(&mut Scoreboard) -> T) -> T {
    let mut guard = board().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Starts a fresh, mod-owned board, throwing away anything on the old one. Answers true, so a
/// caller can fold it into its own setup check.
pub fn create() -> bool {
    with(|board| {
        board.objectives.clear();
        board.scores.clear();
        board.teams.clear();
        true
    })
}

/// Throws away the mod-owned board's contents. Answers whether there was anything to throw.
pub fn remove() -> bool {
    with(|board| {
        let had = !board.objectives.is_empty()
            || !board.scores.is_empty()
            || !board.teams.is_empty();
        board.objectives.clear();
        board.scores.clear();
        board.teams.clear();
        had
    })
}

/// Binds the client's scoreboard. This host cannot: it is inside the client.
#[must_use]
pub fn get() -> Option<Scoreboard> {
    crate::warn!(
        "a mod asked for the client's scoreboard: {}",
        why_unknown("scoreboard")
    );
    None
}

/// Every objective the mod owns, in name order.
#[must_use]
pub fn get_objectives() -> Vec<Objective> {
    with(|board| board.objectives.values().cloned().collect())
}

/// Declares an objective, or replaces the one with that name.
pub fn register_objective(objective: Objective) -> bool {
    with(|board| board.objectives.insert(objective.name.clone(), objective).is_none())
}

/// Forgets an objective, and every score kept under it.
pub fn unregister_objective(name: &str) -> bool {
    with(|board| {
        let gone = board.objectives.remove(name).is_some();
        board.scores.retain(|(objective, _), _| objective != name);
        gone
    })
}

/// An objective by name.
#[must_use]
pub fn get_objective(name: &str) -> Option<Objective> {
    with(|board| board.objectives.get(name).cloned())
}

/// Every score on an objective, highest first, ties broken by entry name lowest first.
#[must_use]
pub fn get_scores(objective: &str) -> Vec<(String, i64)> {
    let mut scores: Vec<(String, i64)> = with(|board| {
        board
            .scores
            .iter()
            .filter(|((name, _), _)| name == objective)
            .map(|((_, entry), score)| (entry.clone(), *score))
            .collect()
    });
    scores.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    scores
}

/// One entry's score on an objective.
#[must_use]
pub fn get_score(objective: &str, entry: &str) -> Option<i64> {
    with(|board| board.scores.get(&(objective.to_owned(), entry.to_owned())).copied())
}

/// Sets an entry's score. Answers the score, or `None` when no such objective is declared.
pub fn set_score(objective: &str, entry: &str, value: i64) -> Option<i64> {
    with(|board| {
        board.objectives.contains_key(objective).then(|| {
            board
                .scores
                .insert((objective.to_owned(), entry.to_owned()), value);
            value
        })
    })
}

/// Adds to an entry's score, creating it at zero first. Answers the new score, or `None` when
/// no such objective is declared.
pub fn add_score(objective: &str, entry: &str, by: i64) -> Option<i64> {
    with(|board| {
        if !board.objectives.contains_key(objective) {
            return None;
        }
        let score = board
            .scores
            .entry((objective.to_owned(), entry.to_owned()))
            .or_insert(0);
        *score = score.saturating_add(by);
        Some(*score)
    })
}

/// Removes an entry's score. Answers whether there was one.
pub fn remove_score(objective: &str, entry: &str) -> bool {
    with(|board| {
        board
            .scores
            .remove(&(objective.to_owned(), entry.to_owned()))
            .is_some()
    })
}

/// Puts an entry's score back to zero, creating it if it was not there. Answers zero, or `None`
/// when no such objective is declared.
pub fn reset_score(objective: &str, entry: &str) -> Option<i64> {
    with(|board| {
        board.objectives.contains_key(objective).then(|| {
            board
                .scores
                .insert((objective.to_owned(), entry.to_owned()), 0);
            0
        })
    })
}

/// Which display slot an objective is assigned to.
#[must_use]
pub fn get_display_slot(objective: &str) -> Option<DisplaySlot> {
    with(|board| {
        board
            .objectives
            .get(objective)
            .and_then(|objective| objective.display_slot)
    })
}

/// Assigns an objective to a display slot, or clears it with `None`.
pub fn set_display_slot(objective: &str, slot: Option<DisplaySlot>) -> bool {
    with(|board| match board.objectives.get_mut(objective) {
        Some(objective) => {
            objective.display_slot = slot;
            true
        }
        None => false,
    })
}

/// Every entry with a score on an objective, in name order.
#[must_use]
pub fn get_entries(objective: &str) -> Vec<String> {
    with(|board| {
        board
            .scores
            .keys()
            .filter(|(name, _)| name == objective)
            .map(|(_, entry)| entry.clone())
            .collect()
    })
}

/// Adds an entry with a zero score, and answers whether it was not already there.
pub fn add_entry(objective: &str, entry: &str) -> bool {
    with(|board| {
        if !board.objectives.contains_key(objective) {
            return false;
        }
        match board
            .scores
            .entry((objective.to_owned(), entry.to_owned()))
        {
            Entry::Vacant(slot) => {
                slot.insert(0);
                true
            }
            Entry::Occupied(_) => false,
        }
    })
}

/// Removes an entry and its score. Answers whether there was one.
pub fn remove_entry(objective: &str, entry: &str) -> bool {
    remove_score(objective, entry)
}

/// Declares a team, or replaces the one with that name.
pub fn register_team(team: Team) -> bool {
    with(|board| board.teams.insert(team.name.clone(), team).is_none())
}

/// Forgets a team.
pub fn unregister_team(name: &str) -> bool {
    with(|board| board.teams.remove(name).is_some())
}

/// A team by name.
#[must_use]
pub fn get_team(name: &str) -> Option<Team> {
    with(|board| board.teams.get(name).cloned())
}

/// Puts a player on a team, taking it off any other team first so it is on exactly one.
/// Answers whether the team exists.
pub fn add_player(team: &str, player: &str) -> bool {
    with(|board| {
        if !board.teams.contains_key(team) {
            return false;
        }
        for held in board.teams.values_mut() {
            held.members.retain(|member| member != player);
        }
        let members = &mut board.teams.get_mut(team).expect("the team was just checked").members;
        if !members.iter().any(|member| member == player) {
            members.push(player.to_owned());
        }
        true
    })
}

/// Takes a player off a team. Answers whether it was on it.
pub fn remove_player(team: &str, player: &str) -> bool {
    with(|board| match board.teams.get_mut(team) {
        Some(held) => {
            let before = held.members.len();
            held.members.retain(|member| member != player);
            held.members.len() != before
        }
        None => false,
    })
}

/// A team's members, in name order.
#[must_use]
pub fn get_team_members(team: &str) -> Vec<String> {
    let mut members = with(|board| {
        board
            .teams
            .get(team)
            .map(|held| held.members.clone())
            .unwrap_or_default()
    });
    members.sort();
    members
}

/// The team a player is on, asked from the member's side. Answers the team name.
#[must_use]
pub fn team_of(player: &str) -> Option<String> {
    with(|board| {
        board
            .teams
            .iter()
            .find(|(_, team)| team.members.iter().any(|member| member == player))
            .map(|(name, _)| name.clone())
    })
}

/// Sets a team's colour.
pub fn set_team_color(team: &str, color: TextColor) -> bool {
    with(|board| match board.teams.get_mut(team) {
        Some(held) => {
            held.color = color;
            true
        }
        None => false,
    })
}

/// Sets a team's prefix.
pub fn set_prefix(team: &str, prefix: &str) -> bool {
    with(|board| match board.teams.get_mut(team) {
        Some(held) => {
            held.prefix = prefix.to_owned();
            true
        }
        None => false,
    })
}

/// Sets a team's suffix.
pub fn set_suffix(team: &str, suffix: &str) -> bool {
    with(|board| match board.teams.get_mut(team) {
        Some(held) => {
            held.suffix = suffix.to_owned();
            true
        }
        None => false,
    })
}

/// Sets when two members of a team push into each other.
pub fn set_collision_rule(team: &str, rule: CollisionRule) -> bool {
    with(|board| match board.teams.get_mut(team) {
        Some(held) => {
            held.collision_rule = rule;
            true
        }
        None => false,
    })
}

/// Turns friendly fire on or off for a team.
pub fn set_friendly_fire(team: &str, friendly_fire: bool) -> bool {
    with(|board| match board.teams.get_mut(team) {
        Some(held) => {
            held.friendly_fire = friendly_fire;
            true
        }
        None => false,
    })
}

/// The board contents the client draws on its sidebar. This host cannot read them.
#[must_use]
pub fn client_sidebar() -> Vec<(String, i64)> {
    crate::warn!(
        "a mod asked for the client's sidebar: {}",
        why_unknown("sidebar")
    );
    Vec::new()
}

/// The team the client has assigned to a player. This host cannot read it.
#[must_use]
pub fn client_team(player: &str) -> Option<Team> {
    crate::warn!(
        "a mod asked for the client's team of {player}: {}",
        why_unknown("team")
    );
    None
}

/// Why a scoreboard question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "scoreboard" => {
            "the client's scoreboard lives inside the client; this host owns a board of its \
             own, which the client never draws"
                .to_owned()
        }
        "sidebar" => {
            "the sidebar a player sees is the client's rendering of its own board, and this \
             host has no way to put a line there"
                .to_owned()
        }
        "team" => {
            "the client's teams colour name tags and score its own board; a team declared here \
             is the mod's model, not the client's"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["scoreboard", "sidebar", "team"]
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
        create();
    }

    #[test]
    fn a_score_is_exact_through_negative_values() {
        let _serial = serial();
        fresh();
        register_objective(Objective::new("points", "dummy"));
        assert_eq!(set_score("points", "steve", 5), Some(5));
        assert_eq!(get_score("points", "steve"), Some(5));
        assert_eq!(add_score("points", "steve", -3), Some(2));
        assert_eq!(add_score("points", "steve", -5), Some(-3), "it passes through zero");
        assert_eq!(add_score("points", "steve", 10), Some(7));
        assert_eq!(get_score("points", "steve"), Some(7));
        assert_eq!(reset_score("points", "steve"), Some(0));
        assert_eq!(get_score("points", "steve"), Some(0));
        assert!(remove_score("points", "steve"));
        assert_eq!(get_score("points", "steve"), None);
        assert_eq!(set_score("points", "alex", -1), Some(-1));
        assert_eq!(add_score("points", "alex", 1), Some(0), "it climbs back to zero");
        assert_eq!(set_score("nothing", "steve", 1), None);
        assert_eq!(add_score("nothing", "steve", 1), None);
        assert_eq!(reset_score("nothing", "steve"), None);
    }

    #[test]
    fn scores_sort_by_value_with_a_name_tie_break() {
        let _serial = serial();
        fresh();
        register_objective(Objective::new("points", "dummy"));
        set_score("points", "b", 5);
        set_score("points", "a", 5);
        set_score("points", "c", 9);
        set_score("points", "d", -2);
        assert_eq!(
            get_scores("points"),
            vec![
                ("c".to_owned(), 9),
                ("a".to_owned(), 5),
                ("b".to_owned(), 5),
                ("d".to_owned(), -2),
            ],
            "highest first, ties by name"
        );
        assert!(get_scores("nothing").is_empty());
    }

    #[test]
    fn an_objective_keeps_its_criterion_name_and_slot() {
        let _serial = serial();
        fresh();
        assert!(register_objective(
            Objective::new("hello:kills", "dummy")
                .display_name("Kills")
                .render_type(RenderType::Hearts)
        ));
        assert!(!register_objective(Objective::new("hello:kills", "dummy")), "it replaces");
        let objective = get_objective("hello:kills").expect("declared");
        assert_eq!(objective.criterion, "dummy");
        assert_eq!(objective.display_name, "hello:kills", "the replacement took the defaults");
        assert_eq!(objective.render_type, RenderType::Integer);
        assert_eq!(get_display_slot("hello:kills"), None);
        assert!(set_display_slot("hello:kills", Some(DisplaySlot::Sidebar)));
        assert_eq!(get_display_slot("hello:kills"), Some(DisplaySlot::Sidebar));
        assert!(set_display_slot("hello:kills", None));
        assert_eq!(get_display_slot("hello:kills"), None);
        assert!(!set_display_slot("nothing", Some(DisplaySlot::List)));
        assert_eq!(get_objectives().len(), 1);
        assert_eq!(RenderType::Hearts.as_str(), "hearts");
        assert_eq!(DisplaySlot::BelowName.as_str(), "below_name");
        assert!(unregister_objective("hello:kills"));
        assert!(!unregister_objective("hello:kills"));
    }

    #[test]
    fn entries_are_created_and_removed_on_an_objective() {
        let _serial = serial();
        fresh();
        register_objective(Objective::new("points", "dummy"));
        assert!(add_entry("points", "steve"));
        assert!(!add_entry("points", "steve"), "it is already there");
        assert_eq!(get_entries("points"), vec!["steve".to_owned()]);
        set_score("points", "alex", 4);
        assert_eq!(get_entries("points"), vec!["alex".to_owned(), "steve".to_owned()]);
        assert!(remove_entry("points", "alex"));
        assert!(!remove_entry("points", "alex"));
        assert!(!add_entry("nothing", "steve"));
    }

    #[test]
    fn unregistering_an_objective_takes_its_scores_with_it() {
        let _serial = serial();
        fresh();
        register_objective(Objective::new("points", "dummy"));
        set_score("points", "steve", 3);
        assert_eq!(get_scores("points").len(), 1);
        assert!(unregister_objective("points"));
        assert_eq!(get_score("points", "steve"), None);
        assert!(get_scores("points").is_empty());
    }

    #[test]
    fn a_team_move_keeps_a_player_on_exactly_one_team() {
        let _serial = serial();
        fresh();
        register_team(Team::new("red"));
        register_team(Team::new("blue"));
        assert!(add_player("red", "steve"));
        assert_eq!(get_team_members("red"), vec!["steve".to_owned()]);
        assert_eq!(team_of("steve"), Some("red".to_owned()));
        assert!(add_player("blue", "steve"), "it moves rather than duplicating");
        assert!(get_team_members("red").is_empty());
        assert_eq!(get_team_members("blue"), vec!["steve".to_owned()]);
        assert_eq!(team_of("steve"), Some("blue".to_owned()));
        assert!(remove_player("blue", "steve"));
        assert!(get_team_members("blue").is_empty());
        assert_eq!(team_of("steve"), None);
        assert!(!add_player("nothing", "steve"));
        assert!(!remove_player("nothing", "steve"));
        assert!(unregister_team("red"));
        assert!(!unregister_team("red"));
    }

    #[test]
    fn team_attributes_are_set_and_read_back() {
        let _serial = serial();
        fresh();
        assert!(register_team(Team::new("red")));
        assert!(set_team_color("red", TextColor::named("dark_red")));
        assert!(set_prefix("red", "["));
        assert!(set_suffix("red", "]"));
        assert!(set_collision_rule("red", CollisionRule::Never));
        assert!(set_friendly_fire("red", true));
        let team = get_team("red").expect("declared");
        assert_eq!(team.prefix, "[");
        assert_eq!(team.suffix, "]");
        assert_eq!(team.color, TextColor::named("dark_red"));
        assert_eq!(team.collision_rule, CollisionRule::Never);
        assert!(team.friendly_fire);
        assert_eq!(team.name_tag_visibility, Visibility::Always);
        assert!(!register_team(
            Team::new("red")
                .name_tag_visibility(Visibility::HideForOtherTeams)
                .death_message_visibility(Visibility::Never)
        ), "it replaces the team already there");
        let team = get_team("red").expect("redeclared");
        assert_eq!(team.name_tag_visibility, Visibility::HideForOtherTeams);
        assert_eq!(team.death_message_visibility, Visibility::Never);
        assert!(!team.friendly_fire, "the replacement took the defaults");
        for setter in [
            set_team_color("nothing", TextColor::named("red")),
            set_prefix("nothing", "x"),
            set_suffix("nothing", "x"),
            set_collision_rule("nothing", CollisionRule::Never),
            set_friendly_fire("nothing", true),
        ] {
            assert!(!setter, "setting something that is not there says so");
        }
        assert_eq!(CollisionRule::PushOwnTeam.as_str(), "push_own_team");
        assert_eq!(Visibility::Never.as_str(), "never");
    }

    #[test]
    fn the_clients_board_sidebar_and_teams_are_refused() {
        let _serial = serial();
        assert!(get().is_none());
        assert!(client_sidebar().is_empty());
        assert!(client_team("steve").is_none());
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
