//! Commands: a real parser, completer and dispatcher for the commands a mod ships, and the
//! client's dispatcher refused.
//!
//! A command a mod ships is a name, its aliases, an argument spec, a permission level and an
//! executor. Everything except the client's own dispatcher is real here:
//!
//! * **Parsing is real.** [`parse`] tokenizes a command line the way a player types it —
//!   whitespace separated, with double- and single-quoted tokens that may contain spaces and
//!   escapes, and an unterminated quote refused — then binds each token against the command's
//!   [`Argument`] spec. String, integer, float, boolean, enum and greedy-string arguments are
//!   typed; an optional or defaulted argument may be left off, and a required one that is
//!   missing is refused with its name. A wrong type is a real error, never a silent zero.
//! * **Completion is real.** [`complete`] takes a partial line and answers the command names
//!   and aliases that begin with what is typed, or the value of the argument being typed
//!   (from its declared [`choices`](Argument::choices) or a registered suggestion provider),
//!   and nothing once the arguments are filled. Names match case-insensitively and answers
//!   come back in registration order.
//! * **Dispatch is real.** [`execute`] parses, checks the caller's permission level against
//!   the command's and calls the executor with the parsed arguments; [`execute_as`] and
//!   [`execute_for`] run the same command as a different caller, so a caller who is not
//!   allowed gets [`CommandError::PermissionDenied`] and the executor is never called.
//!
//! What is *not* real is the client's command system. This host cannot put a command into the
//! client's dispatcher, run one as the client or the server, read the client's own command
//! list, or alias a client command; each is refused with its own sentence in [`why_unknown`]
//! and listed by [`refusals`], never a fake success.
//!
//! ```
//! use cinnaloader::commands::{self, Argument, ArgumentType, Command};
//!
//! commands::register(
//!     Command::new("greet", "greet someone").argument(Argument::new("who", ArgumentType::Word)),
//!     |call| Ok(format!("hello, {}", call.text("who").unwrap_or("world"))),
//! );
//! assert!(commands::get("greet").is_some());
//! assert_eq!(commands::execute("/greet Steve").expect("a command").output, "hello, Steve");
//! assert_eq!(commands::complete("/gre"), vec!["greet".to_owned()]);
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The permission level a command is given when it says nothing else.
pub const DEFAULT_PERMISSION: u32 = 0;
/// The permission level a mod's own dispatcher runs a command at.
pub const MOD_PERMISSION: u32 = 4;

/// What kind of value an argument holds. The wishlist's list of types, spelled as variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ArgumentType {
    #[default]
    String,
    Word,
    GreedyString,
    Integer,
    Float,
    Boolean,
    Enum,
    Player,
    Entity,
    Block,
    Item,
    Position,
    Rotation,
    Dimension,
    ResourceId,
    Duration,
    Color,
    Vector,
    Range,
}

impl ArgumentType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Word => "word",
            Self::GreedyString => "greedy_string",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Boolean => "boolean",
            Self::Enum => "enum",
            Self::Player => "player",
            Self::Entity => "entity",
            Self::Block => "block",
            Self::Item => "item",
            Self::Position => "position",
            Self::Rotation => "rotation",
            Self::Dimension => "dimension",
            Self::ResourceId => "resource_id",
            Self::Duration => "duration",
            Self::Color => "color",
            Self::Vector => "vector",
            Self::Range => "range",
        }
    }

    /// Whether it swallows the rest of the line, which only a greedy string does.
    #[must_use]
    pub fn is_greedy(self) -> bool {
        self == Self::GreedyString
    }

    /// Reads one token as this type. The types this host cannot check against the client —
    /// players, entities, blocks, positions and the rest — are kept as text; the client would
    /// resolve them.
    fn parse_token(self, text: &str, choices: &[String]) -> Result<ArgumentValue, String> {
        match self {
            Self::Integer => text
                .parse::<i64>()
                .map(ArgumentValue::Integer)
                .map_err(|_| text.to_owned()),
            Self::Float => text
                .parse::<f64>()
                .map(ArgumentValue::Float)
                .map_err(|_| text.to_owned()),
            Self::Boolean => match text.to_ascii_lowercase().as_str() {
                "true" => Ok(ArgumentValue::Boolean(true)),
                "false" => Ok(ArgumentValue::Boolean(false)),
                _ => Err(text.to_owned()),
            },
            Self::Enum => match choices.iter().any(|choice| choice.eq_ignore_ascii_case(text)) {
                true => Ok(ArgumentValue::Text(text.to_owned())),
                false => Err(text.to_owned()),
            },
            _ => Ok(ArgumentValue::Text(text.to_owned())),
        }
    }
}

impl fmt::Display for ArgumentType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A token an argument bound to, after it was read as its type.
#[derive(Debug, Clone, PartialEq)]
pub enum ArgumentValue {
    Text(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

impl ArgumentValue {
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(number) => Some(*number),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(number) => Some(*number),
            Self::Integer(number) => Some(*number as f64),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(flag) => Some(*flag),
            _ => None,
        }
    }

    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Integer(number) => number.to_string(),
            Self::Float(number) => number.to_string(),
            Self::Boolean(flag) => flag.to_string(),
        }
    }
}

impl fmt::Display for ArgumentValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

/// One argument of a command: its name, its type, whether it has to be there, and where its
/// value comes from when it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub name: String,
    pub kind: ArgumentType,
    pub choices: Vec<String>,
    /// Whether the line is refused when it leaves this one off.
    pub required: bool,
    /// What to bind when it is left off, which implies it is optional.
    pub default: Option<String>,
    /// The name of a registered suggestion provider, for completion.
    pub source: Option<String>,
}

impl Argument {
    #[must_use]
    pub fn new(name: &str, kind: ArgumentType) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            choices: Vec::new(),
            required: true,
            default: None,
            source: None,
        }
    }

    #[must_use]
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    #[must_use]
    pub fn default(mut self, value: &str) -> Self {
        self.default = Some(value.to_owned());
        self.required = false;
        self
    }

    #[must_use]
    pub fn choices(mut self, choices: &[&str]) -> Self {
        self.choices = choices.iter().map(|choice| (*choice).to_owned()).collect();
        self
    }

    #[must_use]
    pub fn suggestion(mut self, source: &str) -> Self {
        self.source = Some(source.to_owned());
        self
    }
}

/// Where a command came from, which only changes how it is described.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CommandOrigin {
    #[default]
    Mod,
    Builtin,
}

/// A command as a mod declares it: its text, its level and its arguments, but not its
/// executor, which lives beside it in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub permission: u32,
    pub arguments: Vec<Argument>,
    pub origin: CommandOrigin,
    pub redirect: Option<String>,
    pub parent: Option<String>,
    pub subcommands: Vec<String>,
}

impl Command {
    #[must_use]
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_owned(),
            description: description.to_owned(),
            permission: DEFAULT_PERMISSION,
            arguments: Vec::new(),
            origin: CommandOrigin::Mod,
            redirect: None,
            parent: None,
            subcommands: Vec::new(),
        }
    }

    #[must_use]
    pub fn argument(mut self, argument: Argument) -> Self {
        self.arguments.push(argument);
        self
    }

    #[must_use]
    pub fn permission(mut self, level: u32) -> Self {
        self.permission = level;
        self
    }
}

/// Who ran a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSource {
    pub name: String,
    pub permission: u32,
}

impl CommandSource {
    #[must_use]
    pub fn new(name: &str, permission: u32) -> Self {
        Self {
            name: name.to_owned(),
            permission,
        }
    }
}

/// What an executor is handed: the command, the caller and the parsed arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandCall {
    pub command: String,
    pub source: CommandSource,
    pub permission: u32,
    pub arguments: Vec<(String, ArgumentValue)>,
}

impl CommandCall {
    #[must_use]
    pub fn argument(&self, name: &str) -> Option<&ArgumentValue> {
        self.arguments
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, value)| value)
    }

    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        self.argument(name).and_then(ArgumentValue::as_text)
    }

    #[must_use]
    pub fn integer(&self, name: &str) -> Option<i64> {
        self.argument(name).and_then(ArgumentValue::as_integer)
    }

    #[must_use]
    pub fn float(&self, name: &str) -> Option<f64> {
        self.argument(name).and_then(ArgumentValue::as_float)
    }

    #[must_use]
    pub fn boolean(&self, name: &str) -> Option<bool> {
        self.argument(name).and_then(ArgumentValue::as_bool)
    }
}

/// A line that parsed against a registered command.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCommand {
    pub command: String,
    pub arguments: Vec<(String, ArgumentValue)>,
}

/// What running a command produced.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandResult {
    pub command: String,
    pub source: CommandSource,
    pub arguments: Vec<(String, ArgumentValue)>,
    pub output: String,
    pub success: bool,
}

/// Anything that can go wrong parsing or running a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    UnterminatedQuote,
    EmptyCommand,
    UnknownCommand(String),
    MissingArgument(String),
    WrongType {
        argument: String,
        wanted: String,
        found: String,
    },
    TooManyArguments {
        command: String,
        extra: usize,
    },
    PermissionDenied {
        command: String,
        needed: u32,
        had: u32,
    },
    NoExecutor(String),
    Executor(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedQuote => formatter.write_str("the line ends inside a quote"),
            Self::EmptyCommand => formatter.write_str("there is no command to run"),
            Self::UnknownCommand(name) => write!(formatter, "command {name} is not registered"),
            Self::MissingArgument(name) => write!(formatter, "argument {name} is required"),
            Self::WrongType {
                argument,
                wanted,
                found,
            } => write!(formatter, "argument {argument} wants {wanted}, not {found}"),
            Self::TooManyArguments { command, extra } => {
                write!(formatter, "command {command} got {extra} argument(s) too many")
            }
            Self::PermissionDenied {
                command,
                needed,
                had,
            } => write!(
                formatter,
                "command {command} needs permission {needed}, the caller has {had}"
            ),
            Self::NoExecutor(command) => write!(formatter, "command {command} has no executor"),
            Self::Executor(problem) => write!(formatter, "the executor failed: {problem}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// One node of the declaration tree a mod built, for a caller that wants to walk it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub name: String,
    pub description: String,
    pub arguments: usize,
    pub redirect: Option<String>,
    pub children: Vec<TreeNode>,
}

type Executor = Box<dyn FnMut(&CommandCall) -> Result<String, String> + Send + 'static>;
type Provider = Box<dyn FnMut(&str) -> Vec<String> + Send + 'static>;

#[derive(Default)]
struct Held {
    commands: Vec<Command>,
    aliases: Vec<(String, String)>,
    executors: BTreeMap<String, Executor>,
    arg_types: Vec<String>,
    providers: BTreeMap<String, Provider>,
    last_call: Option<CommandCall>,
    last_result: Option<CommandResult>,
    last_source: Option<CommandSource>,
    errors: Vec<String>,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Splits a line into tokens: whitespace separated, with quoted runs kept whole and a
/// backslash escaping the next character. An unterminated quote is refused.
fn tokenize(line: &str) -> Result<Vec<String>, CommandError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        match quote {
            Some(open) => {
                if ch == '\\' {
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    } else {
                        current.push('\\');
                    }
                } else if ch == open {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => {
                    quote = Some(ch);
                    started = true;
                }
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    }
                    started = true;
                }
                _ if ch.is_whitespace() => {
                    if started {
                        tokens.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                _ => {
                    current.push(ch);
                    started = true;
                }
            },
        }
    }
    if quote.is_some() {
        return Err(CommandError::UnterminatedQuote);
    }
    if started {
        tokens.push(current);
    }
    Ok(tokens)
}

fn resolve_index(held: &Held, name: &str) -> Option<usize> {
    let name = name.strip_prefix('/').unwrap_or(name);
    if let Some(pos) = held
        .commands
        .iter()
        .position(|command| command.name.eq_ignore_ascii_case(name))
    {
        return Some(follow_redirect(held, pos));
    }
    let target = held
        .aliases
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, target)| target)?;
    let pos = held
        .commands
        .iter()
        .position(|command| command.name.eq_ignore_ascii_case(target))?;
    Some(follow_redirect(held, pos))
}

fn follow_redirect(held: &Held, start: usize) -> usize {
    // two commands may point at each other, so the walk is bounded by the registry size
    let mut pos = start;
    for _ in 0..=held.commands.len() {
        let Some(target) = held.commands[pos].redirect.as_ref() else {
            break;
        };
        let Some(next) = held
            .commands
            .iter()
            .position(|command| command.name.eq_ignore_ascii_case(target))
        else {
            break;
        };
        if next == pos {
            break;
        }
        pos = next;
    }
    pos
}

fn insert_command(held: &mut Held, command: Command) -> bool {
    if command.name.is_empty()
        || held
            .commands
            .iter()
            .any(|held| held.name.eq_ignore_ascii_case(&command.name))
        || resolve_index(held, &command.name).is_some()
    {
        return false;
    }
    held.commands.push(command);
    true
}

/// Declares a command and the executor that runs it. A duplicate name is refused, not
/// overwritten.
pub fn register<F>(command: Command, executor: F) -> bool
where
    F: FnMut(&CommandCall) -> Result<String, String> + Send + 'static,
{
    with(|held| {
        let name = command.name.clone();
        if !insert_command(held, command) {
            return false;
        }
        held.executors.insert(name, Box::new(executor));
        true
    })
}

/// Declares a command with no executor, for one that only groups subcommands.
pub fn register_root_command(command: Command) -> bool {
    with(|held| insert_command(held, command))
}

/// Declares a command as one this host provides rather than the mod.
pub fn register_builtin_command<F>(mut command: Command, executor: F) -> bool
where
    F: FnMut(&CommandCall) -> Result<String, String> + Send + 'static,
{
    command.origin = CommandOrigin::Builtin;
    register(command, executor)
}

/// Declares a command under a parent, and answers whether both are there.
pub fn register_subcommand<F>(parent: &str, mut command: Command, executor: F) -> bool
where
    F: FnMut(&CommandCall) -> Result<String, String> + Send + 'static,
{
    with(|held| {
        let Some(parent_pos) = resolve_index(held, parent) else {
            return false;
        };
        let parent_name = held.commands[parent_pos].name.clone();
        command.parent = Some(parent_name);
        let child = command.name.clone();
        if !insert_command(held, command) {
            return false;
        }
        held.executors.insert(child.clone(), Box::new(executor));
        held.commands[parent_pos].subcommands.push(child);
        true
    })
}

/// Makes a name redirect to another command, so running it runs the target. Both have to be
/// registered.
pub fn register_redirect(from: &str, to: &str) -> bool {
    with(|held| {
        let Some(from_pos) = resolve_index(held, from) else {
            return false;
        };
        let Some(to_pos) = resolve_index(held, to) else {
            return false;
        };
        let target = held.commands[to_pos].name.clone();
        held.commands[from_pos].redirect = Some(target);
        true
    })
}

/// Forgets a command, its executor, its aliases and its place in its parent.
pub fn unregister(name: &str) -> bool {
    with(|held| {
        let Some(pos) = held
            .commands
            .iter()
            .position(|command| command.name.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        let command = held.commands.remove(pos);
        held.executors.remove(&command.name);
        held.aliases
            .retain(|(_, target)| !target.eq_ignore_ascii_case(&command.name));
        for other in &mut held.commands {
            other
                .subcommands
                .retain(|child| !child.eq_ignore_ascii_case(&command.name));
        }
        true
    })
}

/// Points an alias at a registered command. A name already taken, by a command or another
/// alias, is refused.
pub fn register_alias(alias: &str, command: &str) -> bool {
    if alias.is_empty() {
        return false;
    }
    with(|held| {
        let Some(target) = resolve_index(held, command) else {
            return false;
        };
        let canonical = held.commands[target].name.clone();
        if resolve_index(held, alias).is_some()
            || held
                .aliases
                .iter()
                .any(|(held, _)| held.eq_ignore_ascii_case(alias))
        {
            return false;
        }
        held.aliases.push((alias.to_owned(), canonical));
        true
    })
}

/// Forgets an alias, and answers whether it was there.
pub fn unregister_alias(alias: &str) -> bool {
    with(|held| {
        let before = held.aliases.len();
        held.aliases
            .retain(|(held, _)| !held.eq_ignore_ascii_case(alias));
        held.aliases.len() != before
    })
}

/// Registers a custom argument type by name. It is a declaration this host parses as text.
pub fn register_argument_type(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    with(|held| {
        if held.arg_types.iter().any(|held| held.eq_ignore_ascii_case(name)) {
            return false;
        }
        held.arg_types.push(name.to_owned());
        true
    })
}

/// Registers a source of completions under a name an argument may point at with
/// [`Argument::suggestion`].
pub fn register_suggestion_provider<F>(name: &str, provider: F) -> bool
where
    F: FnMut(&str) -> Vec<String> + Send + 'static,
{
    if name.is_empty() {
        return false;
    }
    with(|held| held.providers.insert(name.to_owned(), Box::new(provider)).is_none())
}

/// A command by name or alias, with any redirect followed.
#[must_use]
pub fn get(name: &str) -> Option<Command> {
    with(|held| resolve_index(held, name).map(|pos| held.commands[pos].clone()))
}

/// Every command, in registration order.
#[must_use]
pub fn get_all() -> Vec<Command> {
    with(|held| held.commands.clone())
}

/// The same registry keyed by name, for a caller that wants to look one up.
#[must_use]
pub fn get_registry() -> BTreeMap<String, Command> {
    with(|held| {
        held.commands
            .iter()
            .map(|command| (command.name.clone(), command.clone()))
            .collect()
    })
}

/// The aliases pointing at a command, in registration order.
#[must_use]
pub fn get_aliases(command: &str) -> Vec<String> {
    with(|held| {
        let Some(pos) = resolve_index(held, command) else {
            return Vec::new();
        };
        let canonical = held.commands[pos].name.clone();
        held.aliases
            .iter()
            .filter(|(_, target)| target == &canonical)
            .map(|(alias, _)| alias.clone())
            .collect()
    })
}

/// A command's arguments, as declared.
#[must_use]
pub fn get_arguments(command: &str) -> Option<Vec<Argument>> {
    with(|held| resolve_index(held, command).map(|pos| held.commands[pos].arguments.clone()))
}

/// The permission level a command requires.
#[must_use]
pub fn get_permissions(command: &str) -> Option<u32> {
    with(|held| resolve_index(held, command).map(|pos| held.commands[pos].permission))
}

/// Changes the level a command requires, and answers whether it was there.
pub fn set_permissions(command: &str, level: u32) -> bool {
    with(|held| match resolve_index(held, command) {
        Some(pos) => {
            held.commands[pos].permission = level;
            true
        }
        None => false,
    })
}

fn bind_arguments(
    command: &Command,
    tokens: &[String],
) -> Result<Vec<(String, ArgumentValue)>, CommandError> {
    let mut bound = Vec::new();
    let mut index = 0;
    for argument in &command.arguments {
        if argument.kind.is_greedy() {
            if index < tokens.len() {
                let rest = tokens[index..].join(" ");
                let value = argument
                    .kind
                    .parse_token(&rest, &argument.choices)
                    .map_err(|found| wrong_type(argument, found))?;
                bound.push((argument.name.clone(), value));
            } else if argument.required {
                return Err(CommandError::MissingArgument(argument.name.clone()));
            } else if let Some(default) = &argument.default {
                let value = argument
                    .kind
                    .parse_token(default, &argument.choices)
                    .map_err(|found| wrong_type(argument, found))?;
                bound.push((argument.name.clone(), value));
            }
            index = tokens.len();
            continue;
        }
        match tokens.get(index) {
            Some(token) => {
                let value = argument
                    .kind
                    .parse_token(token, &argument.choices)
                    .map_err(|found| wrong_type(argument, found))?;
                bound.push((argument.name.clone(), value));
                index += 1;
            }
            None => {
                if argument.required {
                    return Err(CommandError::MissingArgument(argument.name.clone()));
                }
                if let Some(default) = &argument.default {
                    let value = argument
                        .kind
                        .parse_token(default, &argument.choices)
                        .map_err(|found| wrong_type(argument, found))?;
                    bound.push((argument.name.clone(), value));
                }
            }
        }
    }
    if index < tokens.len() {
        return Err(CommandError::TooManyArguments {
            command: command.name.clone(),
            extra: tokens.len() - index,
        });
    }
    Ok(bound)
}

fn wrong_type(argument: &Argument, found: String) -> CommandError {
    CommandError::WrongType {
        argument: argument.name.clone(),
        wanted: argument.kind.as_str().to_owned(),
        found,
    }
}

fn parse_inner(line: &str) -> Result<ParsedCommand, CommandError> {
    let mut tokens = tokenize(line)?;
    if tokens.is_empty() {
        return Err(CommandError::EmptyCommand);
    }
    let first = tokens.remove(0);
    with(|held| {
        let pos = resolve_index(held, &first)
            .ok_or_else(|| CommandError::UnknownCommand(first.clone()))?;
        let command = &held.commands[pos];
        let arguments = bind_arguments(command, &tokens)?;
        Ok(ParsedCommand {
            command: command.name.clone(),
            arguments,
        })
    })
}

/// Reads a command line against the registry, answering the command and its bound arguments.
/// A line that does not parse is refused, and the refusal is kept for [`get_errors`].
pub fn parse(line: &str) -> Result<ParsedCommand, CommandError> {
    match parse_inner(line) {
        Ok(parsed) => Ok(parsed),
        Err(error) => {
            with(|held| held.errors.push(error.to_string()));
            Err(error)
        }
    }
}

fn default_source() -> CommandSource {
    CommandSource::new(&crate::metadata::id(), MOD_PERMISSION)
}

/// Runs a command line as this mod: it parses, checks the permission level and calls the
/// executor.
pub fn execute(line: &str) -> Result<CommandResult, CommandError> {
    execute_for(&default_source(), line)
}

/// Runs a command line as a caller with a given name and permission level.
pub fn execute_as(name: &str, permission: u32, line: &str) -> Result<CommandResult, CommandError> {
    execute_for(&CommandSource::new(name, permission), line)
}

/// Runs a command line as a given caller. A caller whose level is below the command's gets
/// [`CommandError::PermissionDenied`] and the executor is never called.
pub fn execute_for(source: &CommandSource, line: &str) -> Result<CommandResult, CommandError> {
    let parsed = match parse_inner(line) {
        Ok(parsed) => parsed,
        Err(error) => {
            with(|held| held.errors.push(error.to_string()));
            return Err(error);
        }
    };
    let command = with(|held| {
        resolve_index(held, &parsed.command).map(|pos| held.commands[pos].clone())
    })
    .ok_or_else(|| CommandError::UnknownCommand(parsed.command.clone()))?;

    if source.permission < command.permission {
        let error = CommandError::PermissionDenied {
            command: command.name.clone(),
            needed: command.permission,
            had: source.permission,
        };
        with(|held| {
            held.errors.push(error.to_string());
            held.last_source = Some(source.clone());
        });
        return Err(error);
    }

    let call = CommandCall {
        command: command.name.clone(),
        source: source.clone(),
        permission: source.permission,
        arguments: parsed.arguments.clone(),
    };
    let ran = with(|held| held.executors.get_mut(&command.name).map(|executor| executor(&call)));
    let output = ran.ok_or_else(|| CommandError::NoExecutor(command.name.clone()))?;

    let result = match output {
        Ok(output) => CommandResult {
            command: command.name.clone(),
            source: source.clone(),
            arguments: parsed.arguments,
            output,
            success: true,
        },
        Err(problem) => {
            let error = CommandError::Executor(problem);
            with(|held| {
                held.errors.push(error.to_string());
                held.last_call = Some(call.clone());
                held.last_source = Some(source.clone());
            });
            return Err(error);
        }
    };
    with(|held| {
        held.last_call = Some(call);
        held.last_result = Some(result.clone());
        held.last_source = Some(source.clone());
    });
    Ok(result)
}

fn matches_prefix(candidate: &str, partial: &str) -> bool {
    candidate
        .to_ascii_lowercase()
        .starts_with(&partial.to_ascii_lowercase())
}

fn completions_for_command(held: &Held, partial: &str) -> Vec<String> {
    let mut matches: Vec<String> = held
        .commands
        .iter()
        .filter(|command| command.parent.is_none())
        .filter(|command| matches_prefix(&command.name, partial))
        .map(|command| command.name.clone())
        .collect();
    matches.extend(
        held.aliases
            .iter()
            .filter(|(alias, _)| matches_prefix(alias, partial))
            .map(|(alias, _)| alias.clone()),
    );
    matches
}

/// Completes the token being typed: the command names and aliases that begin with it, or the
/// value of the argument it belongs to. Nothing past a complete argument.
#[must_use]
pub fn complete(line: &str) -> Vec<String> {
    let line = line.strip_prefix('/').unwrap_or(line);
    let split = line
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(at, ch)| at + ch.len_utf8());
    let (head, partial) = match split {
        Some(at) => (&line[..at], &line[at..]),
        None => ("", line),
    };
    if head.trim().is_empty() {
        return with(|held| completions_for_command(held, partial));
    }
    let Ok(tokens) = tokenize(head) else {
        return Vec::new();
    };
    let Some(name) = tokens.first() else {
        return with(|held| completions_for_command(held, partial));
    };
    with(|held| {
        let Some(pos) = resolve_index(held, name) else {
            return Vec::new();
        };
        let arg_index = tokens.len() - 1;
        let Some(argument) = held.commands[pos].arguments.get(arg_index).cloned() else {
            return Vec::new();
        };
        completions_for_argument(held, &argument, partial)
    })
}

fn completions_for_argument(held: &mut Held, argument: &Argument, partial: &str) -> Vec<String> {
    if !argument.choices.is_empty() {
        return argument
            .choices
            .iter()
            .filter(|choice| matches_prefix(choice, partial))
            .cloned()
            .collect();
    }
    match argument.source.as_ref().and_then(|source| held.providers.get_mut(source)) {
        Some(provider) => provider(partial),
        None => Vec::new(),
    }
}

/// The values one argument of a command can complete to, for a caller that knows which one it
/// is asking about.
#[must_use]
pub fn suggest(command: &str, argument: &str, partial: &str) -> Vec<String> {
    with(|held| {
        let Some(pos) = resolve_index(held, command) else {
            return Vec::new();
        };
        let Some(argument) = held.commands[pos]
            .arguments
            .iter()
            .find(|held| held.name.eq_ignore_ascii_case(argument))
            .cloned()
        else {
            return Vec::new();
        };
        completions_for_argument(held, &argument, partial)
    })
}

/// The source of the last command that ran or was refused.
#[must_use]
pub fn get_source() -> Option<CommandSource> {
    with(|held| held.last_source.clone())
}

/// The last command that ran and what it was handed.
#[must_use]
pub fn get_context() -> Option<CommandCall> {
    with(|held| held.last_call.clone())
}

/// What the last command that ran produced.
#[must_use]
pub fn get_result() -> Option<CommandResult> {
    with(|held| held.last_result.clone())
}

/// Every parse, permission or executor failure seen since the last clear.
#[must_use]
pub fn get_errors() -> Vec<String> {
    with(|held| held.errors.clone())
}

/// Drops the errors, and answers how many went.
pub fn clear_errors() -> usize {
    with(|held| std::mem::take(&mut held.errors).len())
}

fn node_for(held: &Held, command: &Command) -> TreeNode {
    let children = command
        .subcommands
        .iter()
        .filter_map(|child| {
            held.commands
                .iter()
                .find(|held| held.name.eq_ignore_ascii_case(child))
        })
        .map(|child| node_for(held, child))
        .collect();
    TreeNode {
        name: command.name.clone(),
        description: command.description.clone(),
        arguments: command.arguments.len(),
        redirect: command.redirect.clone(),
        children,
    }
}

/// The declaration tree a mod built: its root commands with their subcommands under them.
#[must_use]
pub fn get_command_tree() -> Vec<TreeNode> {
    with(|held| {
        held.commands
            .iter()
            .filter(|command| command.parent.is_none())
            .map(|command| node_for(held, command))
            .collect()
    })
}

/// What is wrong with the declarations: duplicates, dangling parents, aliases and redirects
/// that point nowhere, a greedy argument that is not last, a required one after an optional.
#[must_use]
pub fn validate_tree() -> Vec<String> {
    with(|held| {
        let mut problems = Vec::new();
        for (index, command) in held.commands.iter().enumerate() {
            if command.name.is_empty() {
                problems.push("a command has no name".to_owned());
            }
            if held.commands.iter().enumerate().any(|(other, held)| {
                other != index && held.name.eq_ignore_ascii_case(&command.name)
            }) {
                problems.push(format!("{} is registered more than once", command.name));
            }
            if let Some(target) = &command.redirect {
                if held
                    .commands
                    .iter()
                    .all(|held| !held.name.eq_ignore_ascii_case(target))
                {
                    problems.push(format!(
                        "{} redirects to {target}, which is not registered",
                        command.name
                    ));
                }
            }
            if let Some(parent) = &command.parent {
                if held
                    .commands
                    .iter()
                    .all(|held| !held.name.eq_ignore_ascii_case(parent))
                {
                    problems.push(format!(
                        "{} has parent {parent}, which is not registered",
                        command.name
                    ));
                }
            }
            let last = command.arguments.len().saturating_sub(1);
            let mut seen_optional = false;
            for (position, argument) in command.arguments.iter().enumerate() {
                if argument.kind.is_greedy() && position != last {
                    problems.push(format!(
                        "{}'s {} argument is greedy but not last",
                        command.name, argument.name
                    ));
                }
                if argument.required && seen_optional {
                    problems.push(format!(
                        "{}'s required argument {} follows an optional one",
                        command.name, argument.name
                    ));
                }
                if !argument.required {
                    seen_optional = true;
                }
            }
        }
        for (alias, target) in &held.aliases {
            if held
                .commands
                .iter()
                .all(|held| !held.name.eq_ignore_ascii_case(target))
            {
                problems.push(format!("alias {alias} points at {target}, which is not registered"));
            }
            if held
                .commands
                .iter()
                .any(|held| held.name.eq_ignore_ascii_case(alias))
            {
                problems.push(format!("alias {alias} is also a command name"));
            }
        }
        problems
    })
}

fn refused(what: &str) -> bool {
    crate::warn!(
        "a mod asked this host to touch the client's commands: {}",
        why_unknown(what)
    );
    false
}

/// Puts a command into the client's dispatcher. `false`.
pub fn register_client_command(_command: Command) -> bool {
    refused("dispatch")
}

/// Registers a command to run on the server. `false`.
pub fn register_server_command(_command: Command) -> bool {
    refused("run")
}

/// The client's own command list. Empty.
#[must_use]
pub fn get_client_commands() -> Vec<Command> {
    crate::warn!(
        "a mod asked this host to touch the client's commands: {}",
        why_unknown("list")
    );
    Vec::new()
}

/// Makes an alias of a client command. `false`.
pub fn alias_client_command(_alias: &str, _command: &str) -> bool {
    refused("alias")
}

/// Why a command question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "dispatch" => {
            "the client's command dispatcher is built by the client from its own registries, \
             and this host has no handle on it, so a mod's command cannot be put there"
                .to_owned()
        }
        "run" => {
            "running a command as the client or the server means driving a dispatcher this \
             host does not own; only the mod's own dispatcher here can run one"
                .to_owned()
        }
        "list" => {
            "the client builds its command list as it registers its own commands, and this \
             host cannot read it; a mod sees only what it registered itself"
                .to_owned()
        }
        "alias" => {
            "an alias of a client command would point at something this host cannot register, \
             so it is refused"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about commands.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["dispatch", "run", "list", "alias"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            held.commands.clear();
            held.aliases.clear();
            held.executors.clear();
            held.arg_types.clear();
            held.providers.clear();
            held.last_call = None;
            held.last_result = None;
            held.last_source = None;
            held.errors.clear();
        });
    }

    fn greet() -> Command {
        Command::new("greet", "greet someone").argument(Argument::new("who", ArgumentType::Word))
    }

    #[test]
    fn a_quoted_argument_arrives_intact_and_an_unterminated_quote_is_refused() {
        let _serial = serial();
        fresh();
        assert!(register(greet(), |call| Ok(call.text("who").unwrap_or("world").to_owned())));
        assert!(register(
            Command::new("echo", "echo text")
                .argument(Argument::new("text", ArgumentType::GreedyString)),
            |call| Ok(call.text("text").unwrap_or("").to_owned()),
        ));

        let result = execute("/echo \"hello world\"").expect("a quoted argument");
        assert_eq!(result.output, "hello world");
        assert_eq!(
            result.arguments,
            vec![("text".to_owned(), ArgumentValue::Text("hello world".to_owned()))]
        );
        assert_eq!(
            execute("/echo 'single quoted'").expect("single quotes").output,
            "single quoted"
        );
        assert_eq!(
            execute("/greet \"with \\\"escape\\\"\"").expect("escapes").output,
            "with \"escape\""
        );
        assert_eq!(execute("/echo a b c").expect("greedy").output, "a b c");
        assert_eq!(execute("/greet \"oops"), Err(CommandError::UnterminatedQuote));
    }

    #[test]
    fn a_missing_required_argument_is_refused_by_name() {
        let _serial = serial();
        fresh();
        assert!(register(
            Command::new("give", "give an item")
                .argument(Argument::new("item", ArgumentType::Word))
                .argument(Argument::new("count", ArgumentType::Integer)),
            |_| Ok("given".to_owned()),
        ));
        assert_eq!(
            execute("/give diamond"),
            Err(CommandError::MissingArgument("count".to_owned()))
        );
        assert_eq!(execute("/give"), Err(CommandError::MissingArgument("item".to_owned())));
        assert!(get_errors().iter().any(|error| error.contains("count")));
    }

    #[test]
    fn an_integer_argument_accepts_12_and_refuses_twelve() {
        let _serial = serial();
        fresh();
        assert!(register(
            Command::new("count", "count to a number")
                .argument(Argument::new("n", ArgumentType::Integer)),
            |call| Ok(format!("{}", call.integer("n").unwrap_or(0))),
        ));
        let result = execute("/count 12").expect("12 is an integer");
        assert_eq!(result.arguments, vec![("n".to_owned(), ArgumentValue::Integer(12))]);
        assert_eq!(result.output, "12");
        assert_eq!(
            execute("/count twelve"),
            Err(CommandError::WrongType {
                argument: "n".to_owned(),
                wanted: "integer".to_owned(),
                found: "twelve".to_owned(),
            })
        );
    }

    #[test]
    fn an_alias_resolves_to_the_same_command() {
        let _serial = serial();
        fresh();
        assert!(register(greet(), |call| Ok(call.text("who").unwrap_or("world").to_owned())));
        assert!(register_alias("g", "greet"));
        assert_eq!(get("g"), get("greet"));
        assert_eq!(get_aliases("greet"), vec!["g".to_owned()]);
        assert_eq!(execute("/g Steve").expect("the alias runs").output, "Steve");
        assert!(!register_alias("g", "greet"), "an alias is not reused");
        assert!(!register_alias("h", "nothing"), "the target has to exist");
        assert!(unregister_alias("g"));
        assert_eq!(get("g"), None);
    }

    #[test]
    fn completion_matches_names_and_argument_values() {
        let _serial = serial();
        fresh();
        assert!(register(Command::new("give", "give"), |_| Ok(String::new())));
        assert!(register(Command::new("git", "git"), |_| Ok(String::new())));
        assert!(register(greet(), |_| Ok(String::new())));
        assert_eq!(complete("/gi"), vec!["give".to_owned(), "git".to_owned()]);
        assert_eq!(complete("/GI"), vec!["give".to_owned(), "git".to_owned()]);
        assert_eq!(complete("/"), vec!["give".to_owned(), "git".to_owned(), "greet".to_owned()]);
        assert!(register_alias("gr", "greet"));
        assert_eq!(complete("/gr"), vec!["greet".to_owned(), "gr".to_owned()]);

        assert!(register(
            Command::new("mode", "set mode").argument(
                Argument::new("mode", ArgumentType::Enum)
                    .choices(&["survival", "creative"])
                    .optional(),
            ),
            |_| Ok(String::new()),
        ));
        assert_eq!(complete("/mode cr"), vec!["creative".to_owned()]);
        assert!(complete("/mode survival ").is_empty(), "nothing past a complete argument");

        assert!(register_suggestion_provider("players", |partial| {
            ["alex", "steve", "alexa"]
                .iter()
                .filter(|name| name.starts_with(partial))
                .map(|name| (*name).to_owned())
                .collect()
        }));
        assert!(register(
            Command::new("tpa", "teleport to a player").argument(
                Argument::new("who", ArgumentType::Player).suggestion("players"),
            ),
            |_| Ok(String::new()),
        ));
        assert_eq!(complete("/tpa al"), vec!["alex".to_owned(), "alexa".to_owned()]);
        assert_eq!(suggest("tpa", "who", "st"), vec!["steve".to_owned()]);
    }

    #[test]
    fn a_permission_denied_call_does_not_run_the_executor() {
        let _serial = serial();
        fresh();
        let ran = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&ran);
        assert!(register(
            Command::new("op", "op someone")
                .permission(2)
                .argument(Argument::new("who", ArgumentType::Word)),
            move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok("opped".to_owned())
            },
        ));
        assert_eq!(execute("/op Steve").expect("the mod may").output, "opped");
        assert_eq!(ran.load(Ordering::SeqCst), 1);

        assert_eq!(
            execute_as("guest", 0, "/op Steve"),
            Err(CommandError::PermissionDenied {
                command: "op".to_owned(),
                needed: 2,
                had: 0,
            })
        );
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the executor never ran");
        assert_eq!(
            execute_for(&CommandSource::new("guest", 1), "/op Steve"),
            Err(CommandError::PermissionDenied {
                command: "op".to_owned(),
                needed: 2,
                had: 1,
            })
        );
        assert!(
            execute_for(&CommandSource::new("admin", 2), "/op Steve").is_ok(),
            "a caller at the level runs it"
        );
        assert_eq!(ran.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_registry_accessors_read_back_what_was_registered() {
        let _serial = serial();
        fresh();
        assert!(register(greet(), |_| Ok("hello".to_owned())));
        assert!(register_builtin_command(
            Command::new("help", "list commands"),
            |_| Ok("helped".to_owned()),
        ));
        assert_eq!(get_all().len(), 2);
        assert_eq!(get_registry().len(), 2);
        assert_eq!(get_registry().get("help").map(|command| command.origin), Some(CommandOrigin::Builtin));
        assert_eq!(get_arguments("greet").map(|arguments| arguments.len()), Some(1));
        assert_eq!(get_permissions("greet"), Some(DEFAULT_PERMISSION));
        assert!(set_permissions("greet", 3));
        assert_eq!(get_permissions("greet"), Some(3));

        assert_eq!(execute("/help").expect("runs").output, "helped");
        assert_eq!(get_source().map(|source| source.permission), Some(MOD_PERMISSION));
        assert_eq!(get_result().map(|result| result.output), Some("helped".to_owned()));
        assert_eq!(get_context().map(|call| call.command), Some("help".to_owned()));

        assert!(register_argument_type("hello:colour"));
        assert!(!register_argument_type("hello:colour"));
        assert!(unregister("help"));
        assert!(!unregister("help"));
        assert_eq!(get("help"), None);
    }

    #[test]
    fn an_optional_argument_falls_back_to_its_default() {
        let _serial = serial();
        fresh();
        assert!(register(
            Command::new("say", "say something").argument(
                Argument::new("text", ArgumentType::Word)
                    .optional()
                    .default("hi"),
            ),
            |call| Ok(call.text("text").unwrap_or("").to_owned()),
        ));
        assert_eq!(
            execute("/say").expect("runs").arguments,
            vec![("text".to_owned(), ArgumentValue::Text("hi".to_owned()))]
        );
        assert_eq!(execute("/say bye").expect("runs").output, "bye");
        assert_eq!(
            execute("/say a b"),
            Err(CommandError::TooManyArguments {
                command: "say".to_owned(),
                extra: 1,
            })
        );
    }

    #[test]
    fn the_command_tree_is_built_and_validated() {
        let _serial = serial();
        fresh();
        assert!(register(greet(), |_| Ok(String::new())));
        assert!(register_subcommand(
            "greet",
            Command::new("hello", "say hello"),
            |_| Ok(String::new()),
        ));
        let tree = get_command_tree();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, "greet");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].name, "hello");
        assert!(validate_tree().is_empty(), "{:?}", validate_tree());

        assert!(register_root_command(
            Command::new("bad", "bad declaration")
                .argument(Argument::new("rest", ArgumentType::GreedyString))
                .argument(Argument::new("more", ArgumentType::Word)),
        ));
        assert!(validate_tree()
            .iter()
            .any(|problem| problem.contains("greedy")));
    }

    #[test]
    fn the_clients_command_system_is_refused() {
        let _serial = serial();
        fresh();
        assert!(!register_client_command(Command::new("client:cmd", "no")));
        assert!(!register_server_command(Command::new("server:cmd", "no")));
        assert!(get_client_commands().is_empty());
        assert!(!alias_client_command("c", "client:cmd"));
        assert_eq!(get_all().len(), 0, "nothing was registered by a refusal");

        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("dispatch").contains("dispatcher"));
        assert_eq!(why_unknown("other"), "other is not something this host can see");
    }
}
