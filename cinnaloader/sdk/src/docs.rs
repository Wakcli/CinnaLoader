//! Documentation: a registry a mod writes entries into, a search over them, and the Markdown
//! it renders.
//!
//! An [`Entry`] describes one declared thing — a name, a summary, its [`Param`]s, the kind it
//! returns, an example, the [`Version`] it appeared in and any tags. [`register`] refuses a
//! name a second time; [`register_page`], [`register_example`], [`register_tutorial`] and
//! [`register_api_reference`] are the four kinds the wishlist names.
//!
//! [`search`] ranks hits in a fixed order — an exact name match first, then a name prefix, a
//! name it contains, a summary hit, and a tag last — and stops at the limit.
//! [`generate_reference`] renders every entry as Markdown and [`generate_reference_for`]
//! renders one; [`generate_mod_report`] names a declared thing that has no entry.
//!
//! What is refused is the client's *own* internals: a class, method, field or package inside
//! the client is not something a mod can see, so it is not something a mod can document.
//! [`why_unknown`] and [`refusals`] say so per kind and [`document_with_the_client`] warns.
//!
//! ```
//! use cinnaloader::docs::{self, Rank};
//!
//! docs::clear();
//! assert!(docs::register_page("blocks::get", "look up a block"));
//! assert!(!docs::register_page("blocks::get", "again"), "one entry per name");
//! let hits = docs::search("blocks::get", 8);
//! assert_eq!(hits[0].rank, Rank::ExactName);
//! assert_eq!(hits[0].entry.name, "blocks::get");
//! assert!(docs::generate_reference().starts_with("# blocks::get\n"));
//! docs::clear();
//! ```

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::version::Version;

/// What an entry documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DocKind {
    Module,
    Type,
    Function,
    Page,
    Example,
    Tutorial,
    ApiReference,
}

impl DocKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Type => "type",
            Self::Function => "function",
            Self::Page => "page",
            Self::Example => "example",
            Self::Tutorial => "tutorial",
            Self::ApiReference => "api_reference",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|kind| kind.as_str() == name)
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::Module,
            Self::Type,
            Self::Function,
            Self::Page,
            Self::Example,
            Self::Tutorial,
            Self::ApiReference,
        ]
    }
}

/// One parameter of a documented thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub kind: String,
    pub required: bool,
}

impl Param {
    #[must_use]
    pub fn new(name: &str, kind: &str, required: bool) -> Self {
        Self {
            name: name.to_owned(),
            kind: kind.to_owned(),
            required,
        }
    }
}

/// One documentation entry for one declared thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: DocKind,
    pub summary: String,
    pub params: Vec<Param>,
    pub returns: String,
    pub example: String,
    pub since: Version,
    pub tags: Vec<String>,
    pub deprecated: Option<String>,
}

impl Entry {
    #[must_use]
    pub fn new(name: &str, summary: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: DocKind::Page,
            summary: summary.to_owned(),
            params: Vec::new(),
            returns: String::new(),
            example: String::new(),
            since: Version::parse("0.1.0"),
            tags: Vec::new(),
            deprecated: None,
        }
    }

    #[must_use]
    pub fn kind(mut self, kind: DocKind) -> Self {
        self.kind = kind;
        self
    }

    #[must_use]
    pub fn param(mut self, name: &str, kind: &str, required: bool) -> Self {
        self.params.push(Param::new(name, kind, required));
        self
    }

    #[must_use]
    pub fn returns(mut self, kind: &str) -> Self {
        self.returns = kind.to_owned();
        self
    }

    #[must_use]
    pub fn example(mut self, text: &str) -> Self {
        self.example = text.to_owned();
        self
    }

    #[must_use]
    pub fn since(mut self, version: &str) -> Self {
        self.since = Version::parse(version);
        self
    }

    #[must_use]
    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_owned());
        self
    }

    /// Marks the entry deprecated, with the message a reader is shown.
    #[must_use]
    pub fn deprecated(mut self, message: &str) -> Self {
        self.deprecated = Some(message.to_owned());
        self
    }

    #[must_use]
    pub fn is_deprecated(&self) -> bool {
        self.deprecated.is_some()
    }
}

fn index() -> &'static Mutex<BTreeMap<String, Entry>> {
    static INDEX: OnceLock<Mutex<BTreeMap<String, Entry>>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, Entry>) -> T) -> T {
    let mut guard = index().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a fully built entry. `false` when the name is already taken.
pub fn register(entry: Entry) -> bool {
    with(|index| {
        if index.contains_key(&entry.name) {
            return false;
        }
        let name = entry.name.clone();
        index.insert(name, entry);
        true
    })
}

/// Registers a prose page.
pub fn register_page(name: &str, summary: &str) -> bool {
    register(Entry::new(name, summary))
}

/// Registers a worked example.
pub fn register_example(name: &str, example: &str) -> bool {
    register(Entry::new(name, example).kind(DocKind::Example).example(example))
}

/// Registers a migration guide.
pub fn register_tutorial(name: &str, summary: &str) -> bool {
    register(Entry::new(name, summary).kind(DocKind::Tutorial))
}

/// Registers an API reference entry.
pub fn register_api_reference(name: &str, summary: &str) -> bool {
    register(Entry::new(name, summary).kind(DocKind::ApiReference))
}

/// Forgets an entry by name.
pub fn unregister(name: &str) -> bool {
    with(|index| index.remove(name).is_some())
}

/// Forgets every entry.
pub fn clear() {
    with(|index| index.clear());
}

/// One entry by name.
#[must_use]
pub fn get(name: &str) -> Option<Entry> {
    with(|index| index.get(name).cloned())
}

fn get_of(name: &str, kind: DocKind) -> Option<Entry> {
    get(name).filter(|entry| entry.kind == kind)
}

/// The entry documenting a module.
#[must_use]
pub fn get_module(name: &str) -> Option<Entry> {
    get_of(name, DocKind::Module)
}

/// The entry documenting a type.
#[must_use]
pub fn get_type(name: &str) -> Option<Entry> {
    get_of(name, DocKind::Type)
}

/// The entry documenting a function.
#[must_use]
pub fn get_function(name: &str) -> Option<Entry> {
    get_of(name, DocKind::Function)
}

/// Every entry, by name.
#[must_use]
pub fn get_api_index() -> Vec<Entry> {
    with(|index| index.values().cloned().collect())
}

/// Every entry of one kind, by name.
#[must_use]
pub fn of_kind(kind: DocKind) -> Vec<Entry> {
    with(|index| {
        index
            .values()
            .filter(|entry| entry.kind == kind)
            .cloned()
            .collect()
    })
}

/// Every worked example.
#[must_use]
pub fn get_examples() -> Vec<Entry> {
    of_kind(DocKind::Example)
}

/// Every migration guide.
#[must_use]
pub fn get_migration_guides() -> Vec<Entry> {
    of_kind(DocKind::Tutorial)
}

/// Every deprecated entry, as `(name, message)`.
#[must_use]
pub fn get_deprecations() -> Vec<(String, String)> {
    with(|index| {
        index
            .values()
            .filter_map(|entry| {
                entry
                    .deprecated
                    .as_ref()
                    .map(|message| (entry.name.clone(), message.clone()))
            })
            .collect()
    })
}

/// Every capability this host knows, with its description.
#[must_use]
pub fn get_capabilities() -> Vec<(String, String)> {
    crate::capability::ALL
        .iter()
        .map(|capability| (capability.as_str().to_owned(), capability.describe().to_owned()))
        .collect()
}

/// Every permission a mod may hold, with its description.
#[must_use]
pub fn get_permissions() -> Vec<(String, String)> {
    crate::modinfo::ALL_PERMISSIONS
        .iter()
        .map(|permission| (permission.as_str().to_owned(), permission.describe().to_owned()))
        .collect()
}

/// Every event kind the loader fires.
#[must_use]
pub fn get_event_catalog() -> Vec<String> {
    crate::events::BUILT_IN_KINDS
        .iter()
        .map(|kind| (*kind).to_owned())
        .collect()
}

/// Every registry name the client owns.
#[must_use]
pub fn get_registry_catalog() -> Vec<String> {
    crate::registry::CLIENT_REGISTRIES
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

/// Entries grouped by the version they appeared in, versions in order.
#[must_use]
pub fn get_version_matrix() -> Vec<(String, Vec<String>)> {
    let mut by_version: BTreeMap<Version, Vec<String>> = BTreeMap::new();
    for entry in get_api_index() {
        by_version.entry(entry.since).or_default().push(entry.name);
    }
    by_version
        .into_iter()
        .map(|(version, mut names)| {
            names.sort();
            (version.to_string(), names)
        })
        .collect()
}

/// Why a hit matched, in the order [`search`] ranks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rank {
    ExactName,
    NamePrefix,
    NameContains,
    Summary,
    Tag,
}

impl Rank {
    /// The position a hit sorts at: 0 is the best.
    #[must_use]
    pub fn order(self) -> u8 {
        match self {
            Self::ExactName => 0,
            Self::NamePrefix => 1,
            Self::NameContains => 2,
            Self::Summary => 3,
            Self::Tag => 4,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactName => "exact_name",
            Self::NamePrefix => "name_prefix",
            Self::NameContains => "name_contains",
            Self::Summary => "summary",
            Self::Tag => "tag",
        }
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::ExactName,
            Self::NamePrefix,
            Self::NameContains,
            Self::Summary,
            Self::Tag,
        ]
    }
}

/// One search result: the entry and why it matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub rank: Rank,
    pub entry: Entry,
}

fn rank_of(entry: &Entry, needle: &str) -> Option<Rank> {
    if needle.is_empty() {
        return None;
    }
    let name = entry.name.to_lowercase();
    if name == needle {
        return Some(Rank::ExactName);
    }
    if name.starts_with(needle) {
        return Some(Rank::NamePrefix);
    }
    if name.contains(needle) {
        return Some(Rank::NameContains);
    }
    if entry.summary.to_lowercase().contains(needle) {
        return Some(Rank::Summary);
    }
    if entry.tags.iter().any(|tag| tag.to_lowercase().contains(needle)) {
        return Some(Rank::Tag);
    }
    None
}

/// Ranks every match in the fixed order [`Rank`] names, then by name, and stops at `limit`.
#[must_use]
pub fn search(query: &str, limit: usize) -> Vec<Hit> {
    let needle = query.to_lowercase();
    let mut hits: Vec<Hit> = with(|index| {
        index
            .values()
            .filter_map(|entry| {
                rank_of(entry, &needle).map(|rank| Hit {
                    rank,
                    entry: entry.clone(),
                })
            })
            .collect()
    });
    hits.sort_by(|left, right| {
        left.rank
            .order()
            .cmp(&right.rank.order())
            .then_with(|| left.entry.name.cmp(&right.entry.name))
    });
    hits.truncate(limit);
    hits
}

fn render(entry: &Entry) -> String {
    let mut out = format!(
        "# {}\n\n{}\n\n**kind** {}\n**since** {}\n",
        entry.name,
        entry.summary,
        entry.kind.as_str(),
        entry.since
    );
    if !entry.tags.is_empty() {
        out.push_str(&format!("**tags** {}\n", entry.tags.join(", ")));
    }
    if let Some(message) = &entry.deprecated {
        out.push_str(&format!("**deprecated** {message}\n"));
    }
    out.push_str("\n## Parameters\n\n");
    if entry.params.is_empty() {
        out.push_str("none\n");
    } else {
        out.push_str("| name | kind | required |\n| --- | --- | --- |\n");
        for param in &entry.params {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                param.name,
                param.kind,
                if param.required { "yes" } else { "no" }
            ));
        }
    }
    out.push_str("\n## Returns\n\n");
    let returns = if entry.returns.is_empty() {
        "none".to_owned()
    } else {
        format!("`{}`", entry.returns)
    };
    out.push_str(&returns);
    out.push('\n');
    if !entry.example.is_empty() {
        out.push_str("\n## Example\n\n```rust\n");
        out.push_str(&entry.example);
        out.push_str("\n```\n");
    }
    out
}

/// Every entry as one Markdown document, entries separated by a blank line.
#[must_use]
pub fn generate_reference() -> String {
    get_api_index()
        .iter()
        .map(render)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One entry as Markdown, or `None` when nothing is documented under that name.
#[must_use]
pub fn generate_reference_for(name: &str) -> Option<String> {
    get(name).map(|entry| render(&entry))
}

/// A Markdown report whose `missing` list names every declared thing with no entry.
#[must_use]
pub fn generate_mod_report() -> String {
    let mut missing: Vec<String> = crate::tools::inventory()
        .into_iter()
        .filter(|item| get(&item.id).is_none())
        .map(|item| item.id)
        .collect();
    missing.sort();
    missing.dedup();
    let mut out = String::from("# Mod report\n\n");
    out.push_str(&format!("- documented: {}\n", get_api_index().len()));
    out.push_str(&format!("- deprecated: {}\n", get_deprecations().len()));
    out.push_str(&format!("- missing: {}\n", missing.len()));
    for id in &missing {
        out.push_str(&format!("- {id}\n"));
    }
    out
}

/// Why an internal of the client cannot be documented. One sentence per kind.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "class" => {
            "a class inside the client is private to the client, so a mod can neither read it \
             nor document it"
                .to_owned()
        }
        "method" => {
            "a method on a client class is invisible to a mod, which holds no reference to \
             document it"
                .to_owned()
        }
        "field" => {
            "a field of a client class is not exposed across the mod boundary, so it has \
             nothing to document"
                .to_owned()
        }
        "package" => {
            "a package of the client's own code is compiled into the client alone, with no \
             listing a mod may read"
                .to_owned()
        }
        _ => format!("{what} is not a client internal this host can see"),
    }
}

/// Every internal this host cannot document, as `(kind, sentence)`.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["class", "method", "field", "package"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Asks to document an internal of the client, which a mod cannot see: warns and answers `false`.
pub fn document_with_the_client(what: &str) -> bool {
    crate::warn!(
        "a mod asked to document the client's own {}: {}",
        what,
        why_unknown(what)
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        clear();
    }

    fn blocks_get() -> Entry {
        Entry::new("blocks::get", "look up a block")
            .kind(DocKind::ApiReference)
            .param("id", "string", true)
            .param("kind", "string", false)
            .returns("Block")
            .example("let block = get(\"minecraft:stone\");")
            .since("0.2.0")
            .tag("world")
    }

    #[test]
    fn a_duplicate_name_is_refused() {
        let _serial = serial();
        fresh();
        assert!(register_page("blocks::list", "list every block"));
        assert!(!register_page("blocks::list", "again"));
        assert!(register(blocks_get()));
        assert!(!register(blocks_get()), "a full entry refuses the same name too");
        assert!(register_page("blocks::other", "another"));
        assert_eq!(get_api_index().len(), 3);
    }

    #[test]
    fn lookups_filter_by_kind_and_the_api_index_holds_everything() {
        let _serial = serial();
        fresh();
        assert!(register(Entry::new("blocks", "the blocks module").kind(DocKind::Module)));
        assert!(register(Entry::new("Block", "a block").kind(DocKind::Type)));
        assert!(register(Entry::new("blocks::get", "look one up").kind(DocKind::Function)));
        assert!(register_example("examples::get", "let b = blocks::get(\"stone\");"));
        assert!(register_tutorial("migrate::flows", "from fluids"));
        assert!(register_api_reference("blocks::reference", "reference"));

        assert!(get_module("blocks").is_some());
        assert!(get_type("Block").is_some());
        assert!(get_function("blocks::get").is_some());
        assert!(get_function("Block").is_none(), "a type is not a function");
        assert_eq!(get_examples().len(), 1);
        assert_eq!(get_migration_guides().len(), 1);
        assert_eq!(get_api_index().len(), 6);
        assert!(get("nothing").is_none());
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_summary_and_limits() {
        let _serial = serial();
        fresh();
        assert!(register_page("blocks::getter", "the getter form"));
        assert!(register_page("items::lookup", "mirrors blocks::get without a block"));
        assert!(register_page("blocks::get", "look up a block"));

        let hits = search("blocks::get", 8);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].rank, Rank::ExactName);
        assert_eq!(hits[0].entry.name, "blocks::get");
        assert_eq!(hits[1].rank, Rank::NamePrefix);
        assert_eq!(hits[1].entry.name, "blocks::getter");
        assert_eq!(hits[2].rank, Rank::Summary);
        assert_eq!(hits[2].entry.name, "items::lookup");

        let limited = search("blocks::get", 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].entry.name, "blocks::get");
        assert!(search("", 8).is_empty());
        assert!(search("nothing here", 8).is_empty());
    }

    #[test]
    fn a_rendered_entry_has_a_fixed_markdown_shape() {
        let _serial = serial();
        fresh();
        assert!(register(blocks_get()));
        let markdown = generate_reference_for("blocks::get").expect("documented");
        let expected = r##"# blocks::get

look up a block

**kind** api_reference
**since** 0.2.0
**tags** world

## Parameters

| name | kind | required |
| --- | --- | --- |
| id | string | yes |
| kind | string | no |

## Returns

`Block`

## Example

```rust
let block = get("minecraft:stone");
```
"##;
        assert_eq!(markdown, expected);
        assert_eq!(generate_reference(), expected);
        assert!(generate_reference_for("nothing").is_none());
    }

    #[test]
    fn the_report_names_a_declared_thing_with_no_entry() {
        let _serial = serial();
        fresh();
        // the tools inventory is shared with every other test in this process, so this
        // asserts the delta this test caused rather than a global count
        fn counted(report: &str, label: &str) -> i64 {
            report
                .lines()
                .find(|line| line.starts_with(&format!("- {label}: ")))
                .and_then(|line| line.rsplit(' ').next())
                .and_then(|value| value.parse().ok())
                .unwrap_or(-1)
        }

        crate::tools::clear();
        let before = generate_mod_report();
        crate::tools::register_command("hello:s90_cmd");
        assert!(register_page("hello:s90_cmd", "documented"));

        let report = generate_mod_report();
        assert!(report.starts_with("# Mod report\n\n"));
        assert_eq!(
            counted(&report, "documented") - counted(&before, "documented"),
            1,
            "{report}"
        );
        assert_eq!(
            counted(&report, "deprecated") - counted(&before, "deprecated"),
            0,
            "{report}"
        );
        assert_eq!(
            counted(&report, "missing") - counted(&before, "missing"),
            0,
            "{report}"
        );

        crate::tools::register_command("hello:s90_undocumented");
        let report = generate_mod_report();
        assert_eq!(
            counted(&report, "missing") - counted(&before, "missing"),
            1,
            "{report}"
        );
        assert!(report.ends_with("- hello:s90_undocumented\n"));
    }

    #[test]
    fn a_deprecated_entry_reports_its_message() {
        let _serial = serial();
        fresh();
        assert!(register(Entry::new("blocks::old_get", "the old lookup").deprecated("use blocks::get")));
        assert!(register_page("blocks::get", "look up a block"));
        assert!(get("blocks::old_get").expect("held").is_deprecated());
        assert!(!get("blocks::get").expect("held").is_deprecated());
        assert_eq!(
            get_deprecations(),
            vec![("blocks::old_get".to_owned(), "use blocks::get".to_owned())]
        );
        assert!(generate_reference_for("blocks::old_get")
            .expect("documented")
            .contains("**deprecated** use blocks::get\n"));
    }

    #[test]
    fn the_catalogs_and_the_version_matrix_come_from_their_modules() {
        assert_eq!(get_capabilities().len(), crate::capability::ALL.len());
        assert_eq!(get_permissions().len(), crate::modinfo::ALL_PERMISSIONS.len());
        assert_eq!(get_event_catalog(), crate::events::BUILT_IN_KINDS.to_vec());
        assert_eq!(get_registry_catalog().len(), 31);
        assert!(get_registry_catalog().contains(&"Blocks".to_owned()));
        assert_eq!(get_capabilities().len(), 38);
        assert_eq!(get_permissions().len(), 29);
        assert_eq!(get_event_catalog().len(), 25);

        let _serial = serial();
        fresh();
        assert!(register(Entry::new("blocks::get", "a").since("0.2.0")));
        assert!(register(Entry::new("blocks::list", "b").since("0.2.0")));
        assert!(register(Entry::new("items::get", "c").since("0.1.0")));
        let matrix = get_version_matrix();
        assert_eq!(matrix.len(), 2);
        assert_eq!(matrix[0], ("0.1.0".to_owned(), vec!["items::get".to_owned()]));
        assert_eq!(
            matrix[1],
            ("0.2.0".to_owned(), vec!["blocks::get".to_owned(), "blocks::list".to_owned()])
        );
    }

    #[test]
    fn the_client_internals_are_refused_with_a_sentence_each() {
        assert!(!document_with_the_client("method"));
        assert_eq!(refusals().len(), 4);
        for (what, sentence) in refusals() {
            assert_ne!(
                sentence,
                format!("{what} is not a client internal this host can see")
            );
            assert!(sentence.starts_with("a "), "{what}: {sentence}");
        }
        assert_eq!(
            why_unknown("nothing"),
            "nothing is not a client internal this host can see"
        );
    }
}
