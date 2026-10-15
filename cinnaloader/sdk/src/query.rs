//! World and entity queries: a query engine over an index a mod owns, and the client's.
//!
//! The client's world, its entities and its blocks are out of reach — section 10 lists the
//! hook that would read them as `never` — so [`live_world`], [`live_entities`] and
//! [`live_blocks`] refuse with a reason rather than inventing a world. What a mod *can* have
//! is a query engine over records it feeds into an [`Index`]: each record has an id, a kind,
//! a position, tags and numeric fields. [`Query`] narrows those records with real filters
//! that compose with and/or/not, orders them, pages them, and renders to a text form that
//! [`Query::parse`] reads back.
//!
//! ```
//! use cinnaloader::query;
//!
//! let records = vec![
//!     query::Record::new("zombie-1", "entity", 1.0, 64.0, 0.0).hostile(true),
//!     query::Record::new("cow-1", "entity", 8.0, 64.0, 0.0),
//! ];
//! let near = query::Query::new(records)
//!     .by_type("entity")
//!     .within_radius((0.0, 64.0, 0.0), 2.0)
//!     .collect();
//! assert_eq!(near.len(), 1);
//! assert_eq!(near[0].id, "zombie-1");
//! assert!(query::live_entities().is_empty(), "the client's entities are out of reach");
//! ```

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::config::ConfigValue;

/// Why a question about the client's world, entities or blocks has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "world" => {
            "the client's world is the only place it exists, and reading it needs the hooks \
             section 10 lists as never"
                .to_owned()
        }
        "entities" => {
            "live entities exist in the client's entity store, which this host cannot read"
                .to_owned()
        }
        "blocks" => "the client's loaded blocks are its own chunk data, which nothing here can reach".to_owned(),
        "chunks" => "chunks are streamed to the client as it moves, and nothing here sees that stream".to_owned(),
        "players" => "the other players are entities in the client's store, not data this host holds".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's world, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["world", "entities", "blocks", "chunks", "players"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn live(what: &str) -> bool {
    crate::warn!("a mod asked this host about the client's world: {}", why_unknown(what));
    false
}

/// The client's world. `None`, always: [`why_unknown`] says why.
#[must_use]
pub fn live_world() -> Option<()> {
    live("world");
    None
}

/// The client's live entities. Empty.
#[must_use]
pub fn live_entities() -> Vec<String> {
    live("entities");
    Vec::new()
}

/// The client's loaded blocks. Empty.
#[must_use]
pub fn live_blocks() -> Vec<String> {
    live("blocks");
    Vec::new()
}

/// One record a mod feeds into an [`Index`]: an id, a kind, where it is, and what it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: String,
    pub kind: String,
    pub position: (f64, f64, f64),
    pub dimension: String,
    pub chunk: (i32, i32),
    pub tags: BTreeMap<String, ConfigValue>,
    pub numbers: BTreeMap<String, f64>,
    pub components: Vec<String>,
    pub visible_to: Vec<String>,
    pub health: f64,
    pub age: u64,
    pub hostile: bool,
    pub dead: bool,
}

impl Record {
    #[must_use]
    pub fn new(id: &str, kind: &str, x: f64, y: f64, z: f64) -> Self {
        Self {
            id: id.to_owned(),
            kind: kind.to_owned(),
            position: (x, y, z),
            dimension: "overworld".to_owned(),
            chunk: ((x / 16.0).floor() as i32, (z / 16.0).floor() as i32),
            tags: BTreeMap::new(),
            numbers: BTreeMap::new(),
            components: Vec::new(),
            visible_to: Vec::new(),
            health: 20.0,
            age: 0,
            hostile: false,
            dead: false,
        }
    }

    #[must_use]
    pub fn dimension(mut self, dimension: &str) -> Self {
        self.dimension = dimension.to_owned();
        self
    }

    #[must_use]
    pub fn chunk(mut self, x: i32, z: i32) -> Self {
        self.chunk = (x, z);
        self
    }

    #[must_use]
    pub fn tag(mut self, key: &str, value: ConfigValue) -> Self {
        self.tags.insert(key.to_owned(), value);
        self
    }

    #[must_use]
    pub fn number(mut self, key: &str, value: f64) -> Self {
        self.numbers.insert(key.to_owned(), value);
        self
    }

    #[must_use]
    pub fn component(mut self, name: &str) -> Self {
        self.components.push(name.to_owned());
        self
    }

    #[must_use]
    pub fn visible_to(mut self, who: &str) -> Self {
        self.visible_to.push(who.to_owned());
        self
    }

    #[must_use]
    pub fn health(mut self, health: f64) -> Self {
        self.health = health;
        self
    }

    #[must_use]
    pub fn age(mut self, age: u64) -> Self {
        self.age = age;
        self
    }

    #[must_use]
    pub fn hostile(mut self, hostile: bool) -> Self {
        self.hostile = hostile;
        self
    }

    #[must_use]
    pub fn dead(mut self, dead: bool) -> Self {
        self.dead = dead;
        self
    }

    /// How far this record is from a point.
    #[must_use]
    pub fn distance_to(&self, origin: (f64, f64, f64)) -> f64 {
        let (dx, dy, dz) = (
            self.position.0 - origin.0,
            self.position.1 - origin.1,
            self.position.2 - origin.2,
        );
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    fn alive(&self) -> bool {
        !self.dead && self.health > 0.0
    }
}

impl fmt::Display for Record {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {:.1},{:.1},{:.1})",
            self.id, self.kind, self.position.0, self.position.1, self.position.2
        )
    }
}

/// One condition a record has to meet. Combine them with [`Filter::and`], [`Filter::or`] and
/// [`Filter::not`].
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// A kind is allowed unless it is denied.
    Type { allow: Vec<String>, deny: Vec<String> },
    Id(String),
    Tag(String),
    TagValue { key: String, value: ConfigValue },
    Component(String),
    WithoutComponent(String),
    Radius { origin: (f64, f64, f64), radius: f64 },
    Box { min: (f64, f64, f64), max: (f64, f64, f64) },
    Cone {
        origin: (f64, f64, f64),
        direction: (f64, f64, f64),
        half_angle_deg: f64,
        range: f64,
    },
    Dimension(String),
    Chunk { x: i32, z: i32 },
    VisibleTo(String),
    Number { key: String, min: Option<f64>, max: Option<f64> },
    Text(String),
    Alive,
    Dead,
    Hostile,
    Passive,
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
}

impl Filter {
    #[must_use]
    pub fn and(self, other: Filter) -> Filter {
        Filter::And(vec![self, other])
    }

    #[must_use]
    pub fn or(self, other: Filter) -> Filter {
        Filter::Or(vec![self, other])
    }

    /// Whether a record meets this condition.
    #[must_use]
    pub fn matches(&self, record: &Record) -> bool {
        match self {
            Self::Type { allow, deny } => {
                (allow.is_empty() || allow.contains(&record.kind)) && !deny.contains(&record.kind)
            }
            Self::Id(id) => &record.id == id,
            Self::Tag(key) => record.tags.contains_key(key),
            // tags compare by their text form, which is how the wishlist's query strings spell them
            Self::TagValue { key, value } => {
                record.tags.get(key).map(ConfigValue::render) == Some(value.render())
            }
            Self::Component(name) => record.components.contains(name),
            Self::WithoutComponent(name) => !record.components.contains(name),
            Self::Radius { origin, radius } => record.distance_to(*origin) <= *radius,
            Self::Box { min, max } => {
                let p = record.position;
                p.0 >= min.0 && p.1 >= min.1 && p.2 >= min.2 && p.0 <= max.0 && p.1 <= max.1 && p.2 <= max.2
            }
            Self::Cone {
                origin,
                direction,
                half_angle_deg,
                range,
            } => in_cone(record.position, *origin, *direction, *half_angle_deg, *range),
            Self::Dimension(name) => &record.dimension == name,
            Self::Chunk { x, z } => record.chunk == (*x, *z),
            Self::VisibleTo(who) => record.visible_to.contains(who),
            Self::Number { key, min, max } => match record.numbers.get(key) {
                Some(value) => min.is_none_or(|low| *value >= low) && max.is_none_or(|high| *value <= high),
                None => false,
            },
            Self::Text(needle) => record_matches_text(record, needle),
            Self::Alive => record.alive(),
            Self::Dead => !record.alive(),
            Self::Hostile => record.hostile,
            Self::Passive => !record.hostile,
            Self::And(filters) => filters.iter().all(|filter| filter.matches(record)),
            Self::Or(filters) => filters.iter().any(|filter| filter.matches(record)),
            Self::Not(filter) => !filter.matches(record),
        }
    }
}

impl std::ops::Not for Filter {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self::Not(Box::new(self))
    }
}

fn record_matches_text(record: &Record, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    record.id.to_lowercase().contains(&needle)
        || record.kind.to_lowercase().contains(&needle)
        || record
            .tags
            .values()
            .any(|value| value.render().to_lowercase().contains(&needle))
}

fn in_cone(
    position: (f64, f64, f64),
    origin: (f64, f64, f64),
    direction: (f64, f64, f64),
    half_angle_deg: f64,
    range: f64,
) -> bool {
    let (vx, vy, vz) = (
        position.0 - origin.0,
        position.1 - origin.1,
        position.2 - origin.2,
    );
    let distance = (vx * vx + vy * vy + vz * vz).sqrt();
    if distance > range {
        return false;
    }
    let length = (direction.0 * direction.0 + direction.1 * direction.1 + direction.2 * direction.2).sqrt();
    if length == 0.0 || distance == 0.0 {
        return distance == 0.0;
    }
    let cos = (vx * direction.0 + vy * direction.1 + vz * direction.2) / (distance * length);
    cos.clamp(-1.0, 1.0).acos().to_degrees() <= half_angle_deg
}

/// The order a query puts its records in, always broken by id ascending for a tie.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Sort {
    #[default]
    Id,
    Nearest { origin: (f64, f64, f64) },
    Health { descending: bool },
    Age { descending: bool },
    Value { key: String, descending: bool },
}

impl Sort {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Nearest { .. } => "nearest",
            Self::Health { .. } => "health",
            Self::Age { .. } => "age",
            Self::Value { .. } => "value",
        }
    }

    /// Orders two records, falling back to their ids so the order is total and stable.
    #[must_use]
    pub fn cmp(&self, a: &Record, b: &Record) -> Ordering {
        let ord = match self {
            Self::Id => Ordering::Equal,
            Self::Nearest { origin } => a
                .distance_to(*origin)
                .partial_cmp(&b.distance_to(*origin))
                .unwrap_or(Ordering::Equal),
            Self::Health { descending } => flip(a.health.partial_cmp(&b.health).unwrap_or(Ordering::Equal), *descending),
            Self::Age { descending } => flip(a.age.cmp(&b.age), *descending),
            Self::Value { key, descending } => {
                let av = a.numbers.get(key).copied().unwrap_or(0.0);
                let bv = b.numbers.get(key).copied().unwrap_or(0.0);
                flip(av.partial_cmp(&bv).unwrap_or(Ordering::Equal), *descending)
            }
        };
        if ord == Ordering::Equal {
            a.id.cmp(&b.id)
        } else {
            ord
        }
    }
}

fn flip(ord: Ordering, descending: bool) -> Ordering {
    if descending {
        ord.reverse()
    } else {
        ord
    }
}

/// A query over records: a filter tree, an order and a page, rendered to text and back.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    records: Vec<Record>,
    filter: Option<Filter>,
    sort: Sort,
    limit: Option<usize>,
    offset: usize,
}

impl Query {
    /// A query over a set of records, unordered beyond id until a sort says otherwise.
    #[must_use]
    pub fn new(records: Vec<Record>) -> Self {
        Self {
            records,
            filter: None,
            sort: Sort::Id,
            limit: None,
            offset: 0,
        }
    }

    /// The records this query was built over.
    #[must_use]
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// The order the query applies.
    #[must_use]
    pub fn sort(&self) -> &Sort {
        &self.sort
    }

    fn push(mut self, filter: Filter) -> Self {
        self.filter = Some(match self.filter.take() {
            Some(held) => Filter::And(vec![held, filter]),
            None => filter,
        });
        self
    }

    /// Narrows by a raw filter, for the combinations [`Filter::and`], [`Filter::or`] and
    /// [`Filter::not`] build.
    #[must_use]
    pub fn filter(self, filter: Filter) -> Self {
        self.push(filter)
    }

    #[must_use]
    pub fn by_type(self, kind: &str) -> Self {
        self.push(Filter::Type {
            allow: vec![kind.to_owned()],
            deny: Vec::new(),
        })
    }

    #[must_use]
    pub fn except_type(self, kind: &str) -> Self {
        self.push(Filter::Type {
            allow: Vec::new(),
            deny: vec![kind.to_owned()],
        })
    }

    #[must_use]
    pub fn by_id(self, id: &str) -> Self {
        self.push(Filter::Id(id.to_owned()))
    }

    #[must_use]
    pub fn by_tag(self, key: &str) -> Self {
        self.push(Filter::Tag(key.to_owned()))
    }

    #[must_use]
    pub fn by_tag_value(self, key: &str, value: ConfigValue) -> Self {
        self.push(Filter::TagValue {
            key: key.to_owned(),
            value,
        })
    }

    #[must_use]
    pub fn by_component(self, name: &str) -> Self {
        self.push(Filter::Component(name.to_owned()))
    }

    #[must_use]
    pub fn with_component(self, name: &str) -> Self {
        self.by_component(name)
    }

    #[must_use]
    pub fn without_component(self, name: &str) -> Self {
        self.push(Filter::WithoutComponent(name.to_owned()))
    }

    #[must_use]
    pub fn within_radius(self, origin: (f64, f64, f64), radius: f64) -> Self {
        self.push(Filter::Radius { origin, radius })
    }

    #[must_use]
    pub fn near_position(self, origin: (f64, f64, f64), radius: f64) -> Self {
        self.within_radius(origin, radius)
    }

    #[must_use]
    pub fn within_box(self, min: (f64, f64, f64), max: (f64, f64, f64)) -> Self {
        self.push(Filter::Box { min, max })
    }

    #[must_use]
    pub fn within_cone(
        self,
        origin: (f64, f64, f64),
        direction: (f64, f64, f64),
        half_angle_deg: f64,
        range: f64,
    ) -> Self {
        self.push(Filter::Cone {
            origin,
            direction,
            half_angle_deg,
            range,
        })
    }

    #[must_use]
    pub fn in_dimension(self, dimension: &str) -> Self {
        self.push(Filter::Dimension(dimension.to_owned()))
    }

    #[must_use]
    pub fn in_chunk(self, x: i32, z: i32) -> Self {
        self.push(Filter::Chunk { x, z })
    }

    #[must_use]
    pub fn visible_to(self, who: &str) -> Self {
        self.push(Filter::VisibleTo(who.to_owned()))
    }

    /// Narrows by a named numeric field's range; either end may be open.
    #[must_use]
    pub fn number_between(self, key: &str, min: Option<f64>, max: Option<f64>) -> Self {
        self.push(Filter::Number {
            key: key.to_owned(),
            min,
            max,
        })
    }

    #[must_use]
    pub fn text(self, needle: &str) -> Self {
        self.push(Filter::Text(needle.to_owned()))
    }

    #[must_use]
    pub fn alive(self) -> Self {
        self.push(Filter::Alive)
    }

    #[must_use]
    pub fn dead(self) -> Self {
        self.push(Filter::Dead)
    }

    #[must_use]
    pub fn hostile(self) -> Self {
        self.push(Filter::Hostile)
    }

    #[must_use]
    pub fn passive(self) -> Self {
        self.push(Filter::Passive)
    }

    #[must_use]
    pub fn sort_by_id(mut self) -> Self {
        self.sort = Sort::Id;
        self
    }

    #[must_use]
    pub fn sort_by_distance(mut self, origin: (f64, f64, f64)) -> Self {
        self.sort = Sort::Nearest { origin };
        self
    }

    #[must_use]
    pub fn sort_by_health(mut self, descending: bool) -> Self {
        self.sort = Sort::Health { descending };
        self
    }

    #[must_use]
    pub fn sort_by_age(mut self, descending: bool) -> Self {
        self.sort = Sort::Age { descending };
        self
    }

    #[must_use]
    pub fn sort_by_value(mut self, key: &str, descending: bool) -> Self {
        self.sort = Sort::Value {
            key: key.to_owned(),
            descending,
        };
        self
    }

    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    #[must_use]
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    fn matches(&self, record: &Record) -> bool {
        self.filter.as_ref().is_none_or(|filter| filter.matches(record))
    }

    /// How many records match, before the page is taken.
    #[must_use]
    pub fn count(&self) -> usize {
        self.records.iter().filter(|record| self.matches(record)).count()
    }

    /// The matching records, ordered and then paged.
    #[must_use]
    pub fn collect(&self) -> Vec<Record> {
        let mut matched: Vec<Record> = self
            .records
            .iter()
            .filter(|record| self.matches(record))
            .cloned()
            .collect();
        matched.sort_by(|a, b| self.sort.cmp(a, b));
        let start = self.offset.min(matched.len());
        let mut page: Vec<Record> = matched.split_off(start);
        if let Some(limit) = self.limit {
            page.truncate(limit);
        }
        page
    }

    /// The first record the page holds. Panics when the query matches nothing.
    #[must_use]
    pub fn first(&self) -> Record {
        self.first_or_none()
            .expect("the query matched nothing, so it has no first record")
    }

    /// The first record the page holds, or `None`.
    #[must_use]
    pub fn first_or_none(&self) -> Option<Record> {
        self.collect().into_iter().next()
    }

    /// Calls `body` for each record the page holds.
    pub fn for_each(&self, mut body: impl FnMut(&Record)) {
        for record in self.collect() {
            body(&record);
        }
    }

    /// The page as an iterator.
    #[must_use]
    pub fn stream(&self) -> std::vec::IntoIter<Record> {
        self.collect().into_iter()
    }

    /// One line saying what the query asked and what it found.
    #[must_use]
    pub fn explain(&self) -> String {
        let filter = self
            .filter
            .as_ref()
            .map(render_filter)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "everything".to_owned());
        format!(
            "{} records, filter: {filter}, sort {}, {} matched, {} on the page",
            self.records.len(),
            self.sort.as_str(),
            self.count(),
            self.collect().len(),
        )
    }

    /// How long one [`Query::collect`] takes, in microseconds.
    #[must_use]
    pub fn benchmark(&self) -> u64 {
        let started = Instant::now();
        let _ = self.collect();
        started.elapsed().as_micros() as u64
    }

    /// The query's text form, which [`Query::parse`] reads back.
    #[must_use]
    pub fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(filter) = &self.filter {
            let text = render_filter(filter);
            if !text.is_empty() {
                parts.push(text);
            }
        }
        match &self.sort {
            Sort::Id => {}
            Sort::Nearest { origin } => {
                if !self.filter.as_ref().is_some_and(contains_radius) {
                    parts.push(format!("origin={},{},{}", facet(origin.0), facet(origin.1), facet(origin.2)));
                }
                parts.push("sort=nearest".to_owned());
            }
            Sort::Health { descending } => {
                parts.push("sort=health".to_owned());
                if *descending {
                    parts.push("order=desc".to_owned());
                }
            }
            Sort::Age { descending } => {
                parts.push("sort=age".to_owned());
                if *descending {
                    parts.push("order=desc".to_owned());
                }
            }
            Sort::Value { key, descending } => {
                parts.push("sort=value".to_owned());
                parts.push(format!("value={key}"));
                if *descending {
                    parts.push("order=desc".to_owned());
                }
            }
        }
        if let Some(limit) = self.limit {
            parts.push(format!("limit={limit}"));
        }
        if self.offset != 0 {
            parts.push(format!("offset={}", self.offset));
        }
        parts.join(" ")
    }

    /// Reads a query over the mod's own index back from its text form.
    pub fn parse(text: &str) -> Result<Self, QueryError> {
        parse_into(text, snapshot())
    }

    /// Reads a query over these records back from its text form.
    pub fn parse_records(text: &str, records: Vec<Record>) -> Result<Self, QueryError> {
        parse_into(text, records)
    }
}

impl fmt::Display for Query {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

fn facet(value: f64) -> String {
    format!("{value}")
}

fn contains_radius(filter: &Filter) -> bool {
    match filter {
        Filter::Radius { .. } => true,
        Filter::And(filters) | Filter::Or(filters) => filters.iter().any(contains_radius),
        Filter::Not(filter) => contains_radius(filter),
        _ => false,
    }
}

fn render_filter(filter: &Filter) -> String {
    match filter {
        Filter::Type { allow, deny } => allow
            .first()
            .map(|kind| format!("kind={kind}"))
            .or_else(|| deny.first().map(|kind| format!("except={kind}")))
            .unwrap_or_default(),
        Filter::Id(id) => format!("id={id}"),
        Filter::Tag(key) => format!("tag={key}"),
        Filter::TagValue { key, value } => format!("tag={key}~{}", value.render()),
        Filter::Component(name) => format!("component={name}"),
        Filter::WithoutComponent(name) => format!("without_component={name}"),
        Filter::Radius { origin, radius } => {
            format!(
                "origin={},{},{} radius={}",
                facet(origin.0),
                facet(origin.1),
                facet(origin.2),
                facet(*radius)
            )
        }
        Filter::Box { min, max } => format!(
            "box={},{},{};{},{},{}",
            facet(min.0),
            facet(min.1),
            facet(min.2),
            facet(max.0),
            facet(max.1),
            facet(max.2)
        ),
        Filter::Cone {
            origin,
            direction,
            half_angle_deg,
            range,
        } => format!(
            "cone={},{},{};{},{},{};{};{}",
            facet(origin.0),
            facet(origin.1),
            facet(origin.2),
            facet(direction.0),
            facet(direction.1),
            facet(direction.2),
            facet(*half_angle_deg),
            facet(*range)
        ),
        Filter::Dimension(name) => format!("dimension={name}"),
        Filter::Chunk { x, z } => format!("chunk={x},{z}"),
        Filter::VisibleTo(who) => format!("visible_to={who}"),
        Filter::Number { key, min, max } => {
            let mut text = format!("number={key}");
            if let Some(min) = min {
                text.push_str(&format!(":{}", facet(*min)));
            }
            if let Some(max) = max {
                if min.is_none() {
                    text.push(':');
                }
                text.push_str(&format!(":{}", facet(*max)));
            }
            text
        }
        Filter::Text(needle) => format!("text={needle}"),
        Filter::Alive => "alive".to_owned(),
        Filter::Dead => "dead".to_owned(),
        Filter::Hostile => "hostile".to_owned(),
        Filter::Passive => "passive".to_owned(),
        Filter::And(filters) => filters
            .iter()
            .map(render_filter)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        Filter::Or(filters) => format!(
            "or({})",
            filters
                .iter()
                .map(render_filter)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Filter::Not(filter) => format!("not({})", render_filter(filter)),
    }
}

/// A query string that cannot be read, with the byte position that refused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryError {
    pub position: usize,
    pub message: String,
}

impl QueryError {
    fn at(position: usize, message: impl Into<String>) -> Self {
        Self {
            position,
            message: message.into(),
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at position {}", self.message, self.position)
    }
}

impl std::error::Error for QueryError {}

enum Part {
    Filter(Filter),
    Origin((f64, f64, f64)),
    Radius(f64),
    Sort(String),
    Order(bool),
    Value(String),
    Limit(usize),
    Offset(usize),
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, at: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += self.peek().map_or(0, char::len_utf8);
        }
    }

    fn word(&mut self) -> String {
        self.skip_ws();
        let start = self.at;
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == '=' || c == '(' || c == ')' {
                break;
            }
            self.at += c.len_utf8();
        }
        self.text[start..self.at].to_owned()
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.at += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), QueryError> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(QueryError::at(self.at, format!("expected '{c}'")))
        }
    }

    fn parse_query(&mut self) -> Result<Vec<Part>, QueryError> {
        let mut parts = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None | Some(')') => break,
                _ => parts.push(self.parse_term()?),
            }
        }
        Ok(parts)
    }

    fn parse_term(&mut self) -> Result<Part, QueryError> {
        let start = self.at;
        let word = self.word();
        if word.is_empty() {
            return Err(QueryError::at(start, "expected a filter"));
        }
        if self.peek() == Some('(') {
            return self.parse_group(start, &word);
        }
        match word.as_str() {
            "alive" => return Ok(Part::Filter(Filter::Alive)),
            "dead" => return Ok(Part::Filter(Filter::Dead)),
            "hostile" => return Ok(Part::Filter(Filter::Hostile)),
            "passive" => return Ok(Part::Filter(Filter::Passive)),
            _ => {}
        }
        if !self.eat('=') {
            return Err(QueryError::at(self.at, format!("'{word}' is not a filter")));
        }
        let value = self.word();
        if value.is_empty() {
            return Err(QueryError::at(self.at, format!("'{word}' has no value")));
        }
        build_part(start, &word, &value)
    }

    fn parse_group(&mut self, start: usize, name: &str) -> Result<Part, QueryError> {
        if !matches!(name, "and" | "or" | "not") {
            return Err(QueryError::at(start, format!("'{name}' is not a group")));
        }
        self.expect('(')?;
        let parts = self.parse_query()?;
        self.expect(')')?;
        let mut filters = Vec::new();
        for part in parts {
            match part {
                Part::Filter(filter) => filters.push(filter),
                _ => return Err(QueryError::at(start, format!("{name}(...) takes filters only"))),
            }
        }
        Ok(Part::Filter(match name {
            "and" => Filter::And(filters),
            "or" => Filter::Or(filters),
            _ => Filter::Not(Box::new(Filter::And(filters))),
        }))
    }
}

fn build_part(start: usize, key: &str, value: &str) -> Result<Part, QueryError> {
    Ok(match key {
        "kind" | "type" => Part::Filter(Filter::Type {
            allow: vec![value.to_owned()],
            deny: Vec::new(),
        }),
        "except" => Part::Filter(Filter::Type {
            allow: Vec::new(),
            deny: vec![value.to_owned()],
        }),
        "id" => Part::Filter(Filter::Id(value.to_owned())),
        "tag" => match value.split_once('~') {
            Some((key, text)) => Part::Filter(Filter::TagValue {
                key: key.to_owned(),
                value: ConfigValue::Text(text.to_owned()),
            }),
            None => Part::Filter(Filter::Tag(value.to_owned())),
        },
        "component" => Part::Filter(Filter::Component(value.to_owned())),
        "without_component" => Part::Filter(Filter::WithoutComponent(value.to_owned())),
        "origin" => Part::Origin(parse_triple(start, value)?),
        "radius" => Part::Radius(parse_number(start, key, value)?),
        "box" => {
            let (min, max) = value
                .split_once(';')
                .ok_or_else(|| QueryError::at(start, "a box needs two corners separated by ';'"))?;
            Part::Filter(Filter::Box {
                min: parse_triple(start, min)?,
                max: parse_triple(start, max)?,
            })
        }
        "cone" => {
            let fields: Vec<&str> = value.split(';').collect();
            if fields.len() != 4 {
                return Err(QueryError::at(
                    start,
                    "a cone needs origin;direction;angle;range",
                ));
            }
            Part::Filter(Filter::Cone {
                origin: parse_triple(start, fields[0])?,
                direction: parse_triple(start, fields[1])?,
                half_angle_deg: parse_number(start, key, fields[2])?,
                range: parse_number(start, key, fields[3])?,
            })
        }
        "dimension" => Part::Filter(Filter::Dimension(value.to_owned())),
        "chunk" => {
            let (x, z) = value
                .split_once(',')
                .ok_or_else(|| QueryError::at(start, "a chunk needs x,z"))?;
            Part::Filter(Filter::Chunk {
                x: parse_int(start, key, x)?,
                z: parse_int(start, key, z)?,
            })
        }
        "visible_to" => Part::Filter(Filter::VisibleTo(value.to_owned())),
        "number" => {
            let mut fields = value.splitn(3, ':');
            let key = fields.next().filter(|key| !key.is_empty()).ok_or_else(|| {
                QueryError::at(start, "a number range needs a field name")
            })?;
            let min = fields
                .next()
                .filter(|field| !field.is_empty())
                .map(|field| parse_number(start, key, field))
                .transpose()?;
            let max = fields
                .next()
                .filter(|field| !field.is_empty())
                .map(|field| parse_number(start, key, field))
                .transpose()?;
            Part::Filter(Filter::Number {
                key: key.to_owned(),
                min,
                max,
            })
        }
        "text" => Part::Filter(Filter::Text(value.to_owned())),
        "sort" => Part::Sort(value.to_owned()),
        "order" => Part::Order(matches!(value, "desc" | "descending")),
        "value" => Part::Value(value.to_owned()),
        "limit" => Part::Limit(parse_usize(start, key, value)?),
        "offset" => Part::Offset(parse_usize(start, key, value)?),
        _ => return Err(QueryError::at(start, format!("unknown filter '{key}'"))),
    })
}

fn parse_triple(start: usize, text: &str) -> Result<(f64, f64, f64), QueryError> {
    let mut parts = text.split(',');
    let x = parse_number(start, "a coordinate", parts.next().unwrap_or(""))?;
    let y = parse_number(start, "a coordinate", parts.next().unwrap_or(""))?;
    let z = parse_number(start, "a coordinate", parts.next().unwrap_or(""))?;
    Ok((x, y, z))
}

fn parse_number(start: usize, key: &str, text: &str) -> Result<f64, QueryError> {
    text.trim()
        .parse()
        .map_err(|_| QueryError::at(start, format!("'{text}' is not a number for {key}")))
}

fn parse_int(start: usize, key: &str, text: &str) -> Result<i32, QueryError> {
    text.trim()
        .parse()
        .map_err(|_| QueryError::at(start, format!("'{text}' is not an integer for {key}")))
}

fn parse_usize(start: usize, key: &str, text: &str) -> Result<usize, QueryError> {
    text.trim()
        .parse()
        .map_err(|_| QueryError::at(start, format!("'{text}' is not a count for {key}")))
}

fn parse_into(text: &str, records: Vec<Record>) -> Result<Query, QueryError> {
    let mut parser = Parser::new(text);
    let parts = parser.parse_query()?;
    parser.skip_ws();
    if parser.at < text.len() {
        return Err(QueryError::at(parser.at, "unexpected text after the query"));
    }
    let mut filters = Vec::new();
    let mut origin = (0.0, 0.0, 0.0);
    let mut radius = None;
    let mut sort = None;
    let mut descending = false;
    let mut value_key = String::new();
    let mut limit = None;
    let mut offset = 0;
    for part in parts {
        match part {
            Part::Filter(filter) => filters.push(filter),
            Part::Origin(pos) => origin = pos,
            Part::Radius(r) => radius = Some(r),
            Part::Sort(name) => sort = Some(name),
            Part::Order(down) => descending = down,
            Part::Value(key) => value_key = key,
            Part::Limit(count) => limit = Some(count),
            Part::Offset(count) => offset = count,
        }
    }
    if let Some(radius) = radius {
        filters.push(Filter::Radius { origin, radius });
    }
    let sort = match sort.as_deref() {
        None | Some("id") => Sort::Id,
        Some("nearest" | "distance") => Sort::Nearest { origin },
        Some("health") => Sort::Health { descending },
        Some("age") => Sort::Age { descending },
        Some("value") => Sort::Value {
            key: value_key,
            descending,
        },
        Some(other) => return Err(QueryError::at(0, format!("unknown sort '{other}'"))),
    };
    let filter = match filters.len() {
        0 => None,
        1 => filters.into_iter().next(),
        _ => Some(Filter::And(filters)),
    };
    Ok(Query {
        records,
        filter,
        sort,
        limit,
        offset,
    })
}

/// An index of records a mod feeds in and queries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Index {
    records: Vec<Record>,
}

impl Index {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a record, replacing one with the same id, and answers whether the id was new.
    pub fn add(&mut self, record: Record) -> bool {
        match self.records.iter_mut().find(|held| held.id == record.id) {
            Some(held) => {
                *held = record;
                false
            }
            None => {
                self.records.push(record);
                true
            }
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.records.len();
        self.records.retain(|record| record.id != id);
        self.records.len() != before
    }

    /// Empties the index, and answers how many records it held.
    pub fn clear(&mut self) -> usize {
        let count = self.records.len();
        self.records.clear();
        count
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Record> {
        self.records.iter().find(|record| record.id == id)
    }

    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Every record, in the order it was fed in.
    #[must_use]
    pub fn records(&self) -> Vec<Record> {
        self.records.clone()
    }

    /// A query over every record in the index.
    #[must_use]
    pub fn query(&self) -> Query {
        Query::new(self.records())
    }

    /// A query over these records, read from its text form.
    pub fn parse(&self, text: &str) -> Result<Query, QueryError> {
        parse_into(text, self.records())
    }
}

fn global() -> &'static Mutex<Index> {
    static GLOBAL: OnceLock<Mutex<Index>> = OnceLock::new();
    GLOBAL.get_or_init(|| Mutex::new(Index::new()))
}

fn with<T>(body: impl FnOnce(&mut Index) -> T) -> T {
    let mut guard = global().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn snapshot() -> Vec<Record> {
    with(|index| index.records())
}

/// Adds a record to the mod's own index, replacing one with the same id, and answers whether
/// the id was new.
pub fn add(record: Record) -> bool {
    with(|index| index.add(record))
}

/// Forgets a record from the mod's own index.
pub fn remove(id: &str) -> bool {
    with(|index| index.remove(id))
}

/// Empties the mod's own index, and answers how many records it held.
pub fn clear() -> usize {
    with(Index::clear)
}

/// A record from the mod's own index, by id.
#[must_use]
pub fn get(id: &str) -> Option<Record> {
    with(|index| index.get(id).cloned())
}

/// Whether the mod's own index holds a record.
#[must_use]
pub fn contains(id: &str) -> bool {
    with(|index| index.contains(id))
}

/// How many records the mod's own index holds.
#[must_use]
pub fn len() -> usize {
    with(|index| index.len())
}

#[must_use]
pub fn is_empty() -> bool {
    with(|index| index.is_empty())
}

/// Every record in the mod's own index.
#[must_use]
pub fn all() -> Vec<Record> {
    snapshot()
}

/// A query over every record in the mod's own index.
#[must_use]
pub fn search() -> Query {
    Query::new(snapshot())
}

/// A query over the mod's own records of one kind.
#[must_use]
pub fn entities() -> Query {
    search().by_type("entity")
}

#[must_use]
pub fn players() -> Query {
    search().by_type("player")
}

#[must_use]
pub fn blocks() -> Query {
    search().by_type("block")
}

#[must_use]
pub fn chunks() -> Query {
    search().by_type("chunk")
}

#[must_use]
pub fn items() -> Query {
    search().by_type("item")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Not;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn world() -> Vec<Record> {
        vec![
            Record::new("zombie-1", "entity", 1.0, 64.0, 0.0)
                .hostile(true)
                .health(18.0)
                .age(40)
                .tag("biome", ConfigValue::Text("plains".to_owned()))
                .number("speed", 0.23)
                .component("hello:glow"),
            Record::new("zombie-2", "entity", 4.0, 64.0, 0.0)
                .hostile(true)
                .health(20.0)
                .age(10)
                .tag("biome", ConfigValue::Text("plains".to_owned()))
                .number("speed", 0.23),
            Record::new("cow-1", "entity", 8.0, 64.0, 0.0)
                .health(10.0)
                .age(5)
                .tag("biome", ConfigValue::Text("meadow".to_owned()))
                .number("speed", 0.3)
                .visible_to("player-1"),
            Record::new("chest-1", "block", 2.0, 64.0, 2.0)
                .tag("loot", ConfigValue::Boolean(true))
                .component("hello:container"),
            Record::new("nether-1", "entity", 0.0, 40.0, 0.0)
                .dimension("nether")
                .chunk(2, 0)
                .dead(true)
                .health(0.0),
        ]
    }

    fn ids(records: Vec<Record>) -> Vec<String> {
        records.into_iter().map(|record| record.id).collect()
    }

    #[test]
    fn each_filter_excludes_what_it_should() {
        let q = |filter: Filter| ids(Query::new(world()).filter(filter).collect());
        assert_eq!(
            q(Filter::Type {
                allow: vec!["entity".to_owned()],
                deny: vec!["block".to_owned()]
            }),
            vec!["cow-1", "nether-1", "zombie-1", "zombie-2"]
        );
        assert_eq!(q(Filter::Id("cow-1".to_owned())), vec!["cow-1"]);
        assert_eq!(
            q(Filter::Tag("loot".to_owned())),
            vec!["chest-1"]
        );
        assert_eq!(
            q(Filter::TagValue {
                key: "biome".to_owned(),
                value: ConfigValue::Text("plains".to_owned())
            }),
            vec!["zombie-1", "zombie-2"]
        );
        assert_eq!(
            q(Filter::Component("hello:glow".to_owned())),
            vec!["zombie-1"]
        );
        assert_eq!(
            q(Filter::WithoutComponent("hello:glow".to_owned())),
            vec!["chest-1", "cow-1", "nether-1", "zombie-2"]
        );
        assert_eq!(
            q(Filter::Radius {
                origin: (0.0, 64.0, 0.0),
                radius: 3.0
            }),
            vec!["chest-1", "zombie-1"]
        );
        assert_eq!(
            q(Filter::Box {
                min: (0.0, 60.0, -1.0),
                max: (5.0, 65.0, 1.0)
            }),
            vec!["zombie-1", "zombie-2"]
        );
        assert_eq!(
            q(Filter::Cone {
                origin: (0.0, 64.0, 0.0),
                direction: (1.0, 0.0, 0.0),
                half_angle_deg: 30.0,
                range: 5.0
            }),
            vec!["zombie-1", "zombie-2"]
        );
        assert_eq!(
            q(Filter::Dimension("nether".to_owned())),
            vec!["nether-1"]
        );
        assert_eq!(
            q(Filter::Chunk { x: 0, z: 0 }),
            vec!["chest-1", "cow-1", "zombie-1", "zombie-2"]
        );
        assert_eq!(
            q(Filter::VisibleTo("player-1".to_owned())),
            vec!["cow-1"]
        );
        assert_eq!(
            q(Filter::Number {
                key: "speed".to_owned(),
                min: Some(0.25),
                max: None
            }),
            vec!["cow-1"]
        );
        assert_eq!(
            q(Filter::Alive),
            vec!["chest-1", "cow-1", "zombie-1", "zombie-2"]
        );
        assert_eq!(q(Filter::Dead), vec!["nether-1"]);
        assert_eq!(q(Filter::Hostile), vec!["zombie-1", "zombie-2"]);
        assert_eq!(q(Filter::Passive), vec!["chest-1", "cow-1", "nether-1"]);
        assert_eq!(
            q(Filter::Text("zombie".to_owned())),
            vec!["zombie-1", "zombie-2"]
        );
    }

    #[test]
    fn filters_compose_with_and_or_and_not() {
        let hostile_glowing = Filter::Hostile.and(Filter::Component("hello:glow".to_owned()));
        assert_eq!(
            ids(Query::new(world()).filter(hostile_glowing.clone()).collect()),
            vec!["zombie-1"]
        );

        let near_or_visible = Filter::Radius {
            origin: (0.0, 64.0, 0.0),
            radius: 3.0,
        }
        .or(Filter::VisibleTo("player-1".to_owned()));
        assert_eq!(
            ids(Query::new(world()).filter(near_or_visible).collect()),
            vec!["chest-1", "cow-1", "zombie-1"]
        );

        let hostile_not_glowing = Filter::Hostile.and(Filter::Component("hello:glow".to_owned()).not());
        assert_eq!(
            ids(Query::new(world()).filter(hostile_not_glowing).collect()),
            vec!["zombie-2"]
        );

        let anded = hostile_glowing.and(Filter::Radius {
            origin: (0.0, 64.0, 0.0),
            radius: 2.0,
        });
        assert_eq!(ids(Query::new(world()).filter(anded).collect()), vec!["zombie-1"]);
    }

    #[test]
    fn sorting_orders_with_a_documented_tie_break() {
        let base = Query::new(world()).by_type("entity");
        assert_eq!(
            ids(base.clone().sort_by_distance((0.0, 64.0, 0.0)).collect()),
            vec!["zombie-1", "zombie-2", "cow-1", "nether-1"]
        );
        assert_eq!(
            ids(base.clone().sort_by_value("speed", true).collect()),
            vec!["cow-1", "zombie-1", "zombie-2", "nether-1"]
        );
        assert_eq!(
            ids(base.clone().sort_by_age(true).collect()),
            vec!["zombie-1", "zombie-2", "cow-1", "nether-1"]
        );
        assert_eq!(
            ids(base.clone().sort_by_health(true).collect()),
            vec!["zombie-2", "zombie-1", "cow-1", "nether-1"]
        );
        assert_eq!(
            ids(base.sort_by_id().collect()),
            vec!["cow-1", "nether-1", "zombie-1", "zombie-2"]
        );

        // zombie-1 and zombie-2 share a speed, so the id breaks the tie ascending either way.
        assert_eq!(
            ids(Query::new(world()).by_type("entity").sort_by_value("speed", false).collect()),
            vec!["nether-1", "zombie-1", "zombie-2", "cow-1"]
        );
    }

    #[test]
    fn paging_answers_the_slice_count_and_an_offset_past_the_end() {
        let base = Query::new(world()).by_type("entity").sort_by_id();
        assert_eq!(base.count(), 4, "the count is before paging");
        assert_eq!(
            ids(base.clone().limit(2).collect()),
            vec!["cow-1", "nether-1"]
        );
        assert_eq!(base.clone().limit(2).count(), 4);
        assert_eq!(
            ids(base.clone().offset(1).limit(2).collect()),
            vec!["nether-1", "zombie-1"]
        );
        assert!(base.clone().offset(99).collect().is_empty());
        assert!(base.clone().limit(0).collect().is_empty());
        assert_eq!(base.first().id, "cow-1");
        assert!(base.clone().offset(99).first_or_none().is_none());
        assert_eq!(base.stream().count(), 4);
    }

    #[test]
    fn a_query_round_trips_through_its_text_form() {
        let _serial = serial();
        clear();
        for record in world() {
            add(record);
        }
        let built = entities()
            .within_radius((0.0, 64.0, 0.0), 5.0)
            .sort_by_distance((0.0, 64.0, 0.0))
            .limit(2);
        let text = built.render();
        assert_eq!(text, "kind=entity origin=0,64,0 radius=5 sort=nearest limit=2");
        let parsed = Query::parse(&text).expect("the text form reads back");
        assert_eq!(ids(parsed.collect()), ids(built.collect()));
        assert_eq!(ids(parsed.collect()), vec!["zombie-1", "zombie-2"]);

        let composed = Query::parse("or(tag=biome~plains component=hello:container)").expect("parses");
        assert_eq!(ids(composed.collect()), vec!["chest-1", "zombie-1", "zombie-2"]);
    }

    #[test]
    fn a_malformed_query_is_refused_by_position() {
        let _serial = serial();
        let error = Query::parse("kind=zombie nonsense=1").expect_err("unknown filter");
        assert_eq!(error.position, 12);
        assert!(error.message.contains("nonsense"), "{}", error.message);

        let error = Query::parse("kind").expect_err("not a filter");
        assert_eq!(error.position, 4);

        let error = Query::parse("radius=big").expect_err("not a number");
        assert_eq!(error.position, 0);

        let error = Query::parse("kind=zombie)").expect_err("stray paren");
        assert_eq!(error.position, 11);
    }

    #[test]
    fn the_mods_index_holds_the_records_it_is_fed() {
        let _serial = serial();
        clear();
        assert!(is_empty());
        assert!(add(Record::new("zombie-1", "entity", 1.0, 64.0, 0.0)));
        add(Record::new("cow-1", "entity", 8.0, 64.0, 0.0));
        add(Record::new("chest-1", "block", 2.0, 64.0, 2.0));
        assert_eq!(len(), 3);
        assert!(contains("cow-1"));
        assert!(!add(Record::new("cow-1", "entity", 9.0, 64.0, 0.0)), "the id was not new");
        assert_eq!(get("cow-1").map(|record| record.position.0), Some(9.0));
        assert_eq!(ids(entities().collect()), vec!["cow-1", "zombie-1"]);
        assert_eq!(ids(players().collect()), Vec::<String>::new());
        assert_eq!(ids(blocks().collect()), vec!["chest-1"]);
        assert!(chunks().collect().is_empty());
        assert!(items().collect().is_empty());
        assert_eq!(all().len(), 3);
        assert!(remove("cow-1"));
        assert!(!remove("cow-1"));
        assert_eq!(clear(), 2);
        assert!(is_empty());
    }

    #[test]
    fn the_clients_world_entities_and_blocks_are_refused() {
        let _serial = serial();
        assert!(live_world().is_none());
        assert!(live_entities().is_empty());
        assert!(live_blocks().is_empty());
        assert_eq!(refusals().len(), 5);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn explain_and_benchmark_say_something() {
        let query = Query::new(world()).by_type("entity").sort_by_distance((0.0, 64.0, 0.0)).limit(1);
        let text = query.explain();
        assert!(text.contains("5 records"), "{text}");
        assert!(text.contains("4 matched"), "{text}");
        assert!(text.contains("1 on the page"), "{text}");
        let _micros = query.benchmark();
    }
}
