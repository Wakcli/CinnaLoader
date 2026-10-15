//! Statistics: the counters a mod owns, and the client's own statistics it does not.
//!
//! The client keeps its own statistics inside itself, so this host keeps the model instead:
//! named [`StatisticCategory`] groups and named [`Statistic`]s, each with a real `i64`
//! counter that a mod can set, read, increment, decrement and reset. Counters are clamped at
//! zero — a decrement past zero stops there rather than wrapping, which [`decrement`]
//! documents and its test pins — and [`total`] aggregates a whole category while [`top`]
//! answers the highest values in a real order, highest first and ties by name. [`export`] and
//! [`import`] are a real text round trip.
//!
//! What is refused is the client: [`get`] binding its own statistics, with its sentence in
//! [`why_unknown`] and its line in [`refusals`]. Nothing here returns a fake success for it.
//!
//! ```
//! use cinnaloader::statistics::{self, Statistic, StatisticCategory};
//!
//! assert!(statistics::register_category(StatisticCategory::new("hello:mining")));
//! assert!(statistics::register(Statistic::new("hello:stone_mined", "hello:mining")));
//! statistics::set_value("hello:stone_mined", 4);
//! statistics::increment("hello:stone_mined", 1);
//! assert_eq!(statistics::get_value("hello:stone_mined"), Some(5));
//! assert_eq!(statistics::total("hello:mining"), Some(5));
//! assert!(statistics::get().is_none(), "the client's statistics are the client's");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// A named group a statistic belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatisticCategory {
    pub id: String,
    pub owner: String,
    pub display_name: String,
}

impl StatisticCategory {
    /// A category with its id as its display name.
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            display_name: id.to_owned(),
        }
    }

    #[must_use]
    pub fn display_name(mut self, display_name: &str) -> Self {
        self.display_name = display_name.to_owned();
        self
    }
}

impl fmt::Display for StatisticCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} '{}'", self.id, self.display_name)
    }
}

/// One named counter, in a category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statistic {
    pub name: String,
    pub owner: String,
    pub category: String,
}

impl Statistic {
    #[must_use]
    pub fn new(name: &str, category: &str) -> Self {
        Self {
            name: name.to_owned(),
            owner: crate::metadata::id(),
            category: category.to_owned(),
        }
    }
}

impl fmt::Display for Statistic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} in {}", self.name, self.category)
    }
}

/// The whole store a mod owns: its categories, its statistics and their values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Statistics {
    pub categories: BTreeMap<String, StatisticCategory>,
    pub statistics: BTreeMap<String, Statistic>,
    pub values: BTreeMap<String, i64>,
}

impl Statistics {
    /// A registered statistic's value, or zero when it has none set yet.
    fn current(&self, name: &str) -> i64 {
        self.values.get(name).copied().unwrap_or(0)
    }
}

impl fmt::Display for Statistics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} categor(ies), {} statistic(s)",
            self.categories.len(),
            self.statistics.len()
        )
    }
}

fn store() -> &'static Mutex<Statistics> {
    static STORE: OnceLock<Mutex<Statistics>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Statistics::default()))
}

fn with<T>(body: impl FnOnce(&mut Statistics) -> T) -> T {
    let mut guard = store().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Binds the client's own statistics. This host cannot: it keeps them inside itself.
#[must_use]
pub fn get() -> Option<Statistics> {
    crate::warn!(
        "a mod asked for the client's statistics: {}",
        why_unknown("statistics")
    );
    None
}

/// Every registered statistic and its value, in name order.
#[must_use]
pub fn get_all() -> Vec<(String, i64)> {
    with(|store| {
        store
            .statistics
            .keys()
            .map(|name| (name.clone(), store.current(name)))
            .collect()
    })
}

/// One statistic's value, or `None` when no such statistic is registered.
#[must_use]
pub fn get_value(name: &str) -> Option<i64> {
    with(|store| {
        store
            .statistics
            .contains_key(name)
            .then(|| store.current(name))
    })
}

/// Sets a statistic's value, clamped at zero. Answers the value, or `None` when no such
/// statistic is registered.
pub fn set_value(name: &str, value: i64) -> Option<i64> {
    let value = value.max(0);
    with(|store| {
        store.statistics.contains_key(name).then(|| {
            store.values.insert(name.to_owned(), value);
            value
        })
    })
}

/// Adds to a statistic, creating it at zero first. Answers the new value, or `None` when no
/// such statistic is registered.
pub fn increment(name: &str, by: i64) -> Option<i64> {
    with(|store| {
        if !store.statistics.contains_key(name) {
            return None;
        }
        let value = store.current(name).saturating_add(by).max(0);
        store.values.insert(name.to_owned(), value);
        Some(value)
    })
}

/// Takes from a statistic without letting it fall below zero. Answers the new value, or
/// `None` when no such statistic is registered.
pub fn decrement(name: &str, by: i64) -> Option<i64> {
    with(|store| {
        if !store.statistics.contains_key(name) {
            return None;
        }
        let value = store.current(name).saturating_sub(by).max(0);
        store.values.insert(name.to_owned(), value);
        Some(value)
    })
}

/// Puts one statistic back to zero. Answers zero, or `None` when no such statistic is
/// registered.
pub fn reset(name: &str) -> Option<i64> {
    with(|store| {
        store.statistics.contains_key(name).then(|| {
            store.values.insert(name.to_owned(), 0);
            0
        })
    })
}

/// Puts every statistic back to zero. Answers how many values were cleared.
pub fn reset_all() -> usize {
    with(|store| {
        let cleared = store.values.len();
        store.values.clear();
        cleared
    })
}

/// Registers a statistic, or replaces the one with that name.
pub fn register(statistic: Statistic) -> bool {
    with(|store| {
        store
            .statistics
            .insert(statistic.name.clone(), statistic)
            .is_none()
    })
}

/// Forgets a statistic, and its value with it.
pub fn unregister(name: &str) -> bool {
    with(|store| {
        store.values.remove(name);
        store.statistics.remove(name).is_some()
    })
}

/// Registers a category, or replaces the one with that id.
pub fn register_category(category: StatisticCategory) -> bool {
    with(|store| {
        store
            .categories
            .insert(category.id.clone(), category)
            .is_none()
    })
}

/// The category a statistic belongs to, or `None` when it is not registered.
#[must_use]
pub fn get_category(name: &str) -> Option<String> {
    with(|store| {
        store
            .statistics
            .get(name)
            .map(|statistic| statistic.category.clone())
    })
}

/// Every registered category, in id order.
#[must_use]
pub fn get_categories() -> Vec<StatisticCategory> {
    with(|store| store.categories.values().cloned().collect())
}

/// Every registered statistic, in name order.
#[must_use]
pub fn get_statistics() -> Vec<Statistic> {
    with(|store| store.statistics.values().cloned().collect())
}

/// The sum of every statistic in a category. Answers `None` when no such category is
/// registered.
#[must_use]
pub fn total(category: &str) -> Option<i64> {
    with(|store| {
        store.categories.contains_key(category).then(|| {
            store
                .statistics
                .values()
                .filter(|statistic| statistic.category == category)
                .map(|statistic| store.current(&statistic.name))
                .sum()
        })
    })
}

/// The highest statistics by value, highest first with ties by name lowest first.
#[must_use]
pub fn top(count: usize) -> Vec<(String, i64)> {
    let mut values: Vec<(String, i64)> = with(|store| {
        store
            .statistics
            .keys()
            .map(|name| (name.clone(), store.current(name)))
            .collect()
    });
    values.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    values.truncate(count);
    values
}

/// The whole store as text, one line per category and per statistic, which [`import`] reads.
#[must_use]
pub fn export() -> String {
    with(|store| {
        let mut text = String::new();
        for category in store.categories.values() {
            text.push_str(&format!(
                "category\t{}\t{}\n",
                category.id, category.display_name
            ));
        }
        for statistic in store.statistics.values() {
            text.push_str(&format!(
                "stat\t{}\t{}\t{}\n",
                statistic.name,
                statistic.category,
                store.current(&statistic.name)
            ));
        }
        text
    })
}

/// Replaces the store with what [`export`] wrote. Answers whether every line parsed.
pub fn import(text: &str) -> bool {
    let mut categories = BTreeMap::new();
    let mut statistics = BTreeMap::new();
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("category") => {
                let (Some(id), Some(display_name)) = (fields.next(), fields.next()) else {
                    return false;
                };
                categories.insert(
                    id.to_owned(),
                    StatisticCategory {
                        id: id.to_owned(),
                        owner: crate::metadata::id(),
                        display_name: display_name.to_owned(),
                    },
                );
            }
            Some("stat") => {
                let (Some(name), Some(category), Some(value)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    return false;
                };
                let Ok(value) = value.parse::<i64>() else {
                    return false;
                };
                statistics.insert(name.to_owned(), Statistic::new(name, category));
                values.insert(name.to_owned(), value);
            }
            _ => return false,
        }
    }
    with(|store| {
        store.categories = categories;
        store.statistics = statistics;
        store.values = values;
    });
    true
}

/// Why a statistics question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "statistics" => {
            "the client's own statistics are counters it keeps inside itself; this host keeps \
             a store of its own, and the client never reads it"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["statistics"]
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
        with(|store| {
            store.categories.clear();
            store.statistics.clear();
            store.values.clear();
        });
    }

    fn seeded() {
        fresh();
        register_category(StatisticCategory::new("mining").display_name("Mining"));
        register_category(StatisticCategory::new("combat"));
        register(Statistic::new("stone", "mining"));
        register(Statistic::new("diamond", "mining"));
        register(Statistic::new("kills", "combat"));
    }

    #[test]
    fn a_counter_starts_at_zero_and_is_set_incremented_and_decremented() {
        let _serial = serial();
        seeded();
        assert_eq!(get_value("stone"), Some(0), "a fresh counter is zero");
        assert_eq!(set_value("stone", 4), Some(4));
        assert_eq!(increment("stone", 3), Some(7));
        assert_eq!(get_value("stone"), Some(7));
        assert_eq!(decrement("stone", 2), Some(5));
        assert_eq!(reset("stone"), Some(0));
        assert_eq!(get_value("stone"), Some(0));
        assert_eq!(get_value("nothing"), None);
        assert_eq!(set_value("nothing", 1), None);
        assert_eq!(increment("nothing", 1), None);
        assert_eq!(decrement("nothing", 1), None);
        assert_eq!(reset("nothing"), None);
    }

    #[test]
    fn a_decrement_below_zero_stops_at_zero() {
        let _serial = serial();
        seeded();
        set_value("stone", 2);
        assert_eq!(decrement("stone", 5), Some(0), "it stops at zero rather than wrapping");
        assert_eq!(decrement("stone", 1), Some(0));
        assert_eq!(get_value("stone"), Some(0));
        set_value("stone", 10);
        assert_eq!(decrement("stone", 4), Some(6));
        assert_eq!(set_value("stone", -3), Some(0), "a counter is never negative");
        assert_eq!(increment("stone", -2), Some(0));
    }

    #[test]
    fn categories_group_statistics_and_total_them() {
        let _serial = serial();
        seeded();
        set_value("stone", 10);
        set_value("diamond", 5);
        set_value("kills", 3);
        assert_eq!(get_category("stone"), Some("mining".to_owned()));
        assert_eq!(get_category("nothing"), None);
        assert_eq!(get_statistics().len(), 3);
        assert_eq!(get_categories().len(), 2);
        assert!(get_categories()
            .iter()
            .any(|category| category.id == "mining" && category.display_name == "Mining"));
        assert_eq!(total("mining"), Some(15));
        assert_eq!(total("combat"), Some(3));
        assert_eq!(total("nothing"), None);
    }

    #[test]
    fn the_top_values_come_back_in_order_with_a_tie() {
        let _serial = serial();
        seeded();
        set_value("stone", 10);
        set_value("diamond", 10);
        set_value("kills", 3);
        assert_eq!(
            top(2),
            vec![("diamond".to_owned(), 10), ("stone".to_owned(), 10)],
            "highest first, ties by name"
        );
        assert_eq!(top(10).len(), 3);
        assert!(top(0).is_empty());
    }

    #[test]
    fn a_statistic_can_be_registered_and_unregistered() {
        let _serial = serial();
        fresh();
        assert!(register(Statistic::new("stone", "mining")));
        assert!(!register(Statistic::new("stone", "mining")), "it replaces");
        assert!(register_category(StatisticCategory::new("mining")));
        assert!(!register_category(StatisticCategory::new("mining")));
        assert_eq!(new_names(), vec!["stone".to_owned()]);
        assert!(unregister("stone"));
        assert!(!unregister("stone"));
        assert!(get_all().is_empty());
    }

    fn new_names() -> Vec<String> {
        get_all().into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn resetting_one_or_all_statistics_returns_them_to_zero() {
        let _serial = serial();
        seeded();
        set_value("stone", 1);
        set_value("diamond", 2);
        assert_eq!(
            get_all(),
            vec![
                ("diamond".to_owned(), 2),
                ("kills".to_owned(), 0),
                ("stone".to_owned(), 1),
            ]
        );
        assert_eq!(reset_all(), 2, "only the counters that had a value were set");
        assert_eq!(
            get_all(),
            vec![
                ("diamond".to_owned(), 0),
                ("kills".to_owned(), 0),
                ("stone".to_owned(), 0),
            ]
        );
    }

    #[test]
    fn statistics_export_and_import_round_trip() {
        let _serial = serial();
        seeded();
        set_value("stone", 42);
        let text = export();
        assert!(text.contains("category\tmining\tMining\n"), "{text}");
        assert!(text.contains("stat\tstone\tmining\t42\n"), "{text}");
        fresh();
        assert_eq!(get_value("stone"), None);
        assert!(import(&text));
        assert_eq!(get_value("stone"), Some(42));
        assert_eq!(get_category("stone"), Some("mining".to_owned()));
        assert_eq!(total("mining"), Some(42));
        assert!(!import("garbage"));
    }

    #[test]
    fn the_clients_statistics_are_refused() {
        let _serial = serial();
        assert!(get().is_none());
        assert_eq!(refusals().len(), 1);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_eq!(StatisticCategory::new("x").to_string(), "x 'x'");
        assert_eq!(Statistic::new("a", "b").to_string(), "a in b");
        assert!(Statistics::default().to_string().contains("0 statistic(s)"));
    }
}
