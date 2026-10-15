//! Localization: keys a mod names, languages it ships, and the few formats a player sees.
//!
//! A mod registers translations per language, and asks for a key. The lookup goes: the
//! current language, then the fallback language, then the key itself, so a missing
//! translation reads as something a person can still understand rather than as nothing.
//! A mod's own `lang/<code>.txt` files in its `assets` are read through the resource stack
//! on [`reload`], so what it ships and what it registers at runtime are the same table.
//!
//! The number, date and currency helpers are deliberately simple and documented as such:
//! this host has no ICU, so they know a handful of conventions rather than every one.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! i18n::register_language("en_us", "English");
//! i18n::register_translations("en_us", &[("hello", "Hello, {0}!"), ("apples", "one apple|{0} apples")]);
//! assert_eq!(i18n::translate_with("hello", &["world"]), "Hello, world!");
//! assert_eq!(i18n::translate_plural("apples", 3), "3 apples");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::resources::ResourceId;

/// A callback that wants to know when the language changes.
type Watcher = Box<dyn FnMut(&str) + Send>;

/// What this loader speaks when nothing else is set.
pub const DEFAULT_LANGUAGE: &str = "en_us";

/// One language a mod knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    pub code: String,
    pub name: String,
    /// How many keys it has, so a caller can see a half-finished translation.
    pub keys: usize,
}

impl fmt::Display for Language {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({}, {} keys)", self.code, self.name, self.keys)
    }
}

/// How a language writes a number, for the three conventions this host knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberStyle {
    /// `1,234.56`
    DotDecimal,
    /// `1.234,56`
    CommaDecimal,
    /// `1 234,56`
    SpaceDecimal,
}

impl NumberStyle {
    /// The convention a language code usually follows.
    #[must_use]
    pub fn for_language(code: &str) -> Self {
        let language = code.split(['_', '-']).next().unwrap_or_default();
        match language {
            "de" | "es" | "it" | "pt" | "nl" | "tr" | "id" | "vi" => Self::CommaDecimal,
            "fr" | "ru" | "pl" | "sv" | "cs" | "uk" => Self::SpaceDecimal,
            _ => Self::DotDecimal,
        }
    }

    fn separators(self) -> (char, char) {
        match self {
            Self::DotDecimal => (',', '.'),
            Self::CommaDecimal => ('.', ','),
            Self::SpaceDecimal => (' ', ','),
        }
    }
}

struct Held {
    languages: BTreeMap<String, String>,
    /// language code -> key -> text
    translations: BTreeMap<String, BTreeMap<String, String>>,
    current: String,
    fallback: String,
    watchers: Vec<Watcher>,
    formatters: BTreeMap<String, NumberStyle>,
}

impl Default for Held {
    fn default() -> Self {
        let mut languages = BTreeMap::new();
        languages.insert(DEFAULT_LANGUAGE.to_owned(), "English".to_owned());
        Self {
            languages,
            translations: BTreeMap::new(),
            current: DEFAULT_LANGUAGE.to_owned(),
            fallback: DEFAULT_LANGUAGE.to_owned(),
            watchers: Vec::new(),
            formatters: BTreeMap::new(),
        }
    }
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn normalise(code: &str) -> String {
    code.trim().to_ascii_lowercase().replace('-', "_")
}

/// Translates a key, or answers with the key itself when nothing has it.
#[must_use]
pub fn translate(key: &str) -> String {
    get_translation(key).unwrap_or_else(|| key.to_owned())
}

/// Translates a key, substituting `{0}`, `{1}` and so on.
#[must_use]
pub fn translate_with(key: &str, args: &[&str]) -> String {
    let mut text = translate(key);
    for (index, arg) in args.iter().enumerate() {
        text = text.replace(&format!("{{{index}}}"), arg);
    }
    text
}

/// Translates a key with named arguments, substituting `{name}`.
#[must_use]
pub fn translate_named(key: &str, args: &[(&str, &str)]) -> String {
    let mut text = translate(key);
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// Translates a key that has a singular and a plural, written `one|many` or `one|few|many`.
///
/// English rules only: one is `1`, everything else is the last form. A language with more
/// forms than that needs its own function, and this does not pretend to know them.
#[must_use]
pub fn translate_plural(key: &str, count: i64) -> String {
    let text = translate(key);
    let forms: Vec<&str> = text.split('|').collect();
    let form = match forms.len() {
        0 | 1 => text.as_str(),
        2 => {
            if count == 1 {
                forms[0]
            } else {
                forms[1]
            }
        }
        // Three or more forms: the last one is the general case.
        _ => {
            if count == 1 {
                forms[0]
            } else {
                forms[forms.len() - 1]
            }
        }
    };
    form.replace("{0}", &count.to_string())
        .replace("{count}", &count.to_string())
}

/// Whether a key has a translation in the current language or the fallback.
#[must_use]
pub fn has_translation(key: &str) -> bool {
    get_translation(key).is_some()
}

/// The text for a key: current language, then fallback, then nothing.
#[must_use]
pub fn get_translation(key: &str) -> Option<String> {
    with(|held| {
        for language in [&held.current, &held.fallback] {
            if let Some(text) = held
                .translations
                .get(language)
                .and_then(|table| table.get(key))
            {
                return Some(text.clone());
            }
        }
        None
    })
}

/// The same, in a named language, without changing which one is current.
#[must_use]
pub fn get_translation_in(language: &str, key: &str) -> Option<String> {
    let language = normalise(language);
    with(|held| {
        held.translations
            .get(&language)
            .and_then(|table| table.get(key))
            .cloned()
    })
}

/// Teaches the loader about a language, with the name to show a user.
pub fn register_language(code: &str, name: &str) -> bool {
    let code = normalise(code);
    if code.is_empty() {
        return false;
    }
    with(|held| {
        held.languages.insert(code, name.to_owned());
        true
    })
}

/// Forgets a language and its translations.
pub fn unregister_language(code: &str) -> bool {
    let code = normalise(code);
    with(|held| {
        let had = held.languages.remove(&code).is_some();
        let had_text = held.translations.remove(&code).is_some();
        had || had_text
    })
}

/// The languages in place, by code.
#[must_use]
pub fn get_languages() -> Vec<Language> {
    with(|held| {
        held.languages
            .iter()
            .map(|(code, name)| Language {
                code: code.clone(),
                name: name.clone(),
                keys: held
                    .translations
                    .get(code)
                    .map(|table| table.len())
                    .unwrap_or_default(),
            })
            .collect()
    })
}

/// The language being asked for first.
#[must_use]
pub fn get_current_language() -> String {
    with(|held| held.current.clone())
}

/// Chooses a language, and tells everyone who asked to be told. An unknown language is
/// accepted — a mod may register it later — and the answer says whether it is known now.
pub fn set_current_language(code: &str) -> bool {
    let code = normalise(code);
    let known = with(|held| {
        let known = held.languages.contains_key(&code);
        held.current = code.clone();
        known
    });
    let count = watcher_count();
    for index in 0..count {
        notify(index, &code);
    }
    crate::events::publish(
        crate::events::Event::new(crate::events::kind::LANGUAGE_CHANGED).with_text(&code),
    );
    known
}

/// The language asked for when the current one has nothing.
#[must_use]
pub fn get_fallback_language() -> String {
    with(|held| held.fallback.clone())
}

/// Chooses the fallback language.
pub fn set_fallback_language(code: &str) {
    let code = normalise(code);
    with(|held| held.fallback = code);
}

/// Registers a table of translations for a language, adding to what is there.
pub fn register_translations(language: &str, entries: &[(&str, &str)]) -> usize {
    let language = normalise(language);
    with(|held| {
        set_number_style_if_absent(held, &language);
        let table = held.translations.entry(language.clone()).or_default();
        let mut added = 0;
        for (key, text) in entries {
            if table.insert((*key).to_owned(), (*text).to_owned()).is_none() {
                added += 1;
            }
        }
        held.languages
            .entry(language.clone())
            .or_insert_with(|| language.clone());
        added
    })
}

fn set_number_style_if_absent(held: &mut Held, language: &str) {
    held.formatters
        .entry(language.to_owned())
        .or_insert_with(|| NumberStyle::for_language(language));
}

/// Removes everything registered for a language, keeping the language itself.
pub fn unregister_translations(language: &str) -> usize {
    let language = normalise(language);
    with(|held| {
        held.translations
            .remove(&language)
            .map(|table| table.len())
            .unwrap_or_default()
    })
}

/// The whole table for a language, for a caller that wants to walk it.
#[must_use]
pub fn translations(language: &str) -> BTreeMap<String, String> {
    let language = normalise(language);
    with(|held| {
        held.translations
            .get(&language)
            .cloned()
            .unwrap_or_default()
    })
}

/// Reads every `lang/<code>.txt` file a resource pack offers, over what is registered in
/// memory. This is what a mod's shipped `assets/lang/en_us.txt` becomes a table.
pub fn reload() -> usize {
    let namespaces: Vec<String> = crate::resources::get_active_packs();
    let mut read = 0;
    for namespace in namespaces {
        for id in crate::resources::find(&format!("{namespace}:lang/*")) {
            let Some(code) = id.path().rsplit('/').next().map(|name| {
                name.trim_end_matches(".txt").trim_end_matches(".lang").to_owned()
            }) else {
                continue;
            };
            let Some(text) = crate::resources::read_text(&id) else {
                continue;
            };
            let entries: Vec<(String, String)> = text
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        return None;
                    }
                    let (key, value) = line.split_once('=')?;
                    Some((key.trim().to_owned(), value.trim().to_owned()))
                })
                .collect();
            read += with(|held| {
                set_number_style_if_absent(held, &code);
                let table = held.translations.entry(code.clone()).or_default();
                for (key, value) in entries {
                    table.insert(key, value);
                }
                held.languages.entry(code.clone()).or_insert_with(|| code.clone());
                table.len()
            });
        }
    }
    crate::events::publish(
        crate::events::Event::new(crate::events::kind::LANGUAGE_CHANGED).with_text(&resource_default()),
    );
    read
}

/// Calls `watcher` when the language changes.
pub fn on_language_changed(watcher: impl FnMut(&str) + Send + 'static) -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    with(|held| held.watchers.push(Box::new(watcher)));
    id
}

fn notify(index: usize, code: &str) {
    with(|held| {
        if let Some(watcher) = held.watchers.get_mut(index) {
            watcher(code);
        }
    });
}

/// Forgets a watcher by the id [`on_language_changed`] handed out. Watchers are told in the
/// order they were added, so the id is their position.
pub fn unwatch_language(index: u64) -> bool {
    let index = index as usize;
    with(|held| {
        if index == 0 || index > held.watchers.len() {
            return false;
        }
        let removed: Watcher = held.watchers.remove(index - 1);
        drop(removed);
        true
    })
}

/// How many watchers there are.
#[must_use]
pub fn watcher_count() -> usize {
    with(|held| held.watchers.len())
}

/// A number the way the current language writes one.
#[must_use]
pub fn format_number(value: f64) -> String {
    format_number_in(&get_current_language(), value)
}

/// The same, in a named language.
#[must_use]
pub fn format_number_in(language: &str, value: f64) -> String {
    let style = with(|held| {
        held.formatters
            .get(&normalise(language))
            .copied()
            .unwrap_or_else(|| NumberStyle::for_language(language))
    });
    let (group, decimal) = style.separators();
    let negative = value < 0.0;
    let text = format!("{:.2}", value.abs());
    let (whole, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let mut grouped = String::new();
    for (index, character) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(group);
        }
        grouped.push(character);
    }
    let mut out = grouped;
    let fraction = fraction.trim_end_matches('0');
    if !fraction.is_empty() {
        out.push(decimal);
        out.push_str(fraction);
    }
    if negative {
        out.insert(0, '-');
    }
    out
}

/// Seconds since the Unix epoch as a date, `YYYY-MM-DD`, optionally with a time.
#[must_use]
pub fn format_date(timestamp: i64) -> String {
    let (year, month, day, _, _) = civil_from_unix(timestamp);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The time of day at a Unix timestamp, `HH:MM:SS`.
#[must_use]
pub fn format_time(timestamp: i64) -> String {
    let day = 86_400;
    let seconds = timestamp.rem_euclid(day);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// A length of time in milliseconds, as `2h 3m 4s`, dropping what is zero.
#[must_use]
pub fn format_duration(millis: u64) -> String {
    let seconds = millis / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(format!("{seconds}s"));
    }
    parts.join(" ")
}

/// An amount with a currency code, as the current language writes it.
#[must_use]
pub fn format_currency(amount: f64, code: &str) -> String {
    format!("{} {}", format_number(amount), code.to_ascii_uppercase())
}

/// Splits a Unix timestamp into a calendar date, without a date library.
fn civil_from_unix(timestamp: i64) -> (i64, i64, i64, i64, i64) {
    let days = timestamp.div_euclid(86_400);
    let seconds = timestamp.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days, which is exact for every date this will see.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day, seconds / 3600, (seconds % 3600) / 60)
}

/// The language a reload just filled in, for the event that says so.
fn resource_default() -> String {
    get_current_language()
}

/// The resource id this mod's shipped language file would have, so a mod can check whether
/// one exists before reloading.
#[must_use]
pub fn resource_id_for(language: &str) -> Option<ResourceId> {
    let code = language.trim().trim_end_matches(".txt");
    ResourceId::new(&crate::metadata::id(), &format!("lang/{code}.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            *held = Held::default();
        });
    }

    #[test]
    fn a_key_reads_in_the_current_language_then_the_fallback() {
        let _serial = serial();
        fresh();
        register_translations("en_us", &[("hello", "Hello"), ("only_english", "English only")]);
        register_translations("de_de", &[("hello", "Hallo")]);
        set_current_language("de_de");
        assert_eq!(get_current_language(), "de_de");
        assert_eq!(translate("hello"), "Hallo");
        assert_eq!(translate("only_english"), "English only", "the fallback answers");
        assert_eq!(translate("nowhere"), "nowhere", "and a missing key reads as itself");
        assert!(has_translation("only_english"));
        assert!(!has_translation("nowhere"));
        assert_eq!(get_translation_in("de_de", "hello"), Some("Hallo".to_owned()));
    }

    #[test]
    fn placeholders_are_substituted() {
        let _serial = serial();
        fresh();
        register_translations("en_us", &[("hello", "Hello, {0}!")]);
        assert_eq!(translate_with("hello", &["world"]), "Hello, world!");
        assert_eq!(
            translate_named("hello", &[("0", "named")]),
            "Hello, named!"
        );
    }

    #[test]
    fn a_plural_picks_the_form_that_fits() {
        let _serial = serial();
        fresh();
        register_translations(
            "en_us",
            &[("apples", "one apple|{0} apples"), ("few", "{0} leaf|some leaves|many leaves")],
        );
        assert_eq!(translate_plural("apples", 1), "one apple");
        assert_eq!(translate_plural("apples", 0), "0 apples");
        assert_eq!(translate_plural("apples", 3), "3 apples");
        assert_eq!(translate_plural("few", 1), "1 leaf");
        assert_eq!(translate_plural("few", 5), "many leaves");
    }

    #[test]
    fn a_language_can_be_registered_and_forgotten() {
        let _serial = serial();
        fresh();
        assert!(register_language("fr_fr", "Français"));
        register_translations("fr_fr", &[("hello", "Bonjour")]);
        let languages = get_languages();
        assert!(languages.iter().any(|language| language.code == "fr_fr"
            && language.name == "Français"
            && language.keys == 1));
        assert_eq!(unregister_translations("fr_fr"), 1);
        assert!(unregister_language("fr_fr"));
        assert!(!unregister_language("fr_fr"));
    }

    #[test]
    fn a_change_of_language_tells_the_watchers() {
        let _serial = serial();
        fresh();
        let heard = std::sync::Arc::new(Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&heard);
        on_language_changed(move |code| {
            into.lock().unwrap().push(code.to_owned());
        });
        set_current_language("de_de");
        assert_eq!(*heard.lock().unwrap(), vec!["de_de".to_owned()]);
        assert_eq!(watcher_count(), 1);
        assert!(unwatch_language(1));
        assert_eq!(watcher_count(), 0);
    }

    #[test]
    fn a_fallback_can_be_chosen() {
        let _serial = serial();
        fresh();
        register_translations("de_de", &[("hello", "Hallo")]);
        register_translations("en_us", &[("hello", "Hello")]);
        set_fallback_language("de_de");
        assert_eq!(get_fallback_language(), "de_de");
        set_current_language("fr_fr");
        assert_eq!(translate("hello"), "Hallo", "the fallback answers for a language with nothing");
    }

    #[test]
    fn numbers_dates_and_durations_read_the_way_a_person_expects() {
        let _serial = serial();
        fresh();
        assert_eq!(format_number_in("en_us", 1234567.5), "1,234,567.5");
        assert_eq!(format_number_in("de_de", 1234567.5), "1.234.567,5");
        assert_eq!(format_number_in("fr_fr", 1234.0), "1 234");
        assert_eq!(format_number_in("en_us", -12.25), "-12.25");
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_time(0), "00:00:00");
        assert_eq!(format_time(86_400 + 3661), "01:01:01");
        assert_eq!(format_date(1_700_000_000), "2023-11-14");
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(1000), "1s");
        assert_eq!(format_duration(3_723_000), "1h 2m 3s");
        assert_eq!(format_currency(12.5, "usd"), "12.5 USD");
    }

    #[test]
    fn a_language_file_in_a_pack_becomes_a_table() {
        let _serial = serial();
        fresh();
        let root =
            std::env::temp_dir().join(format!("cinnaloader-i18n-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());
        crate::fs::write_text("assets/lang/de_de.txt", "hello=Hallo\nbye=Tschüss\n").unwrap();

        let mut pack = crate::resources::ResourcePack::memory("probe-lang", 0);
        pack.insert("lang/de_de.txt", b"hello=Hallo\nbye=Tsch\xc3\xbcss\n".to_vec());
        crate::resources::register_pack(pack);
        assert!(reload() > 0);
        assert_eq!(get_translation_in("de_de", "hello"), Some("Hallo".to_owned()));
        assert_eq!(get_translation_in("de_de", "bye"), Some("Tschüss".to_owned()));
        crate::resources::unregister_pack("probe-lang");
        let _ = std::fs::remove_dir_all(&root);
    }
}
