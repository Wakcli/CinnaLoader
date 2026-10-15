//! Section 72 demo: a small scripting language, lexed, parsed and interpreted for real.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cinnaloader::scripting::{self, ScriptError, Value};

fn show(result: Result<Value, ScriptError>) -> String {
    match result {
        Ok(value) => value.to_string(),
        Err(error) => format!("error: {error}"),
    }
}

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    lines.push(format!(
        "precedence: 1 + 2 * 3 = {} but (1 + 2) * 3 = {}, so * binds tighter than +",
        show(scripting::eval("1 + 2 * 3")),
        show(scripting::eval("(1 + 2) * 3")),
    ));

    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    scripting::register_function("boom", 0, move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Bool(true))
    });
    lines.push(format!(
        "short-circuit: false and boom() = {} while boom ran {} time(s), so its right side was never evaluated",
        show(scripting::eval("false and boom()")),
        calls.load(Ordering::SeqCst),
    ));

    scripting::register_function("double", 1, |args| {
        Ok(Value::Int(args[0].as_int().unwrap_or(0) * 2))
    });
    lines.push(format!(
        "host call: double(21) = {} through a Rust closure the script named",
        show(scripting::eval("double(21)")),
    ));

    lines.push(format!(
        "unknown function: {}",
        show(scripting::eval("missing()")),
    ));

    scripting::register_function("add", 2, |args| {
        Ok(Value::Int(
            args[0].as_int().unwrap_or(0) + args[1].as_int().unwrap_or(0),
        ))
    });
    lines.push(format!(
        "wrong arity: {} and {} for the same function declared with two arguments",
        show(scripting::eval("add(1)")),
        show(scripting::eval("add(1, 2, 3)")),
    ));

    lines.push(format!(
        "divide by zero: {} rather than an infinity",
        show(scripting::eval("1 / 0")),
    ));

    scripting::set_instruction_limit(1_000);
    lines.push(format!(
        "budget: {} so a runaway while true stops instead of hanging",
        show(scripting::eval("while true {}")),
    ));
    scripting::set_instruction_limit(1_000_000);

    lines.push(format!("syntax error: {}", show(scripting::eval("1 +"))));

    lines.push(format!(
        "bindings: eval_with x * 2 = {}",
        show(scripting::eval_with("x * 2", &[("x", Value::Int(4))])),
    ));

    let refused = scripting::refusals()
        .into_iter()
        .map(|(what, _)| what)
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!("refusals: [{refused}] never faked"));

    lines
}
