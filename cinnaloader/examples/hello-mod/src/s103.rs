//! Section 103 demo: a foreign API bridged to a local function.

use cinnaloader::bridges::{
    self, Adapter, AdapterKind, ArgKind, Bridge, BridgeError, Foreign, MethodMap,
};
use cinnaloader::config::ConfigValue;
use cinnaloader::interop::Version;

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    bridges::register(
        Bridge::new("legacy:math", Version::new(1, 2, 0))
            .kind(AdapterKind::Shim)
            .method(
                MethodMap::new("add", "math_add", ArgKind::Integer)
                    .arg("a", ArgKind::Integer)
                    .arg("b", ArgKind::Integer),
            ),
    );
    bridges::register_adapter(Adapter::new("legacy:math", "math_add", |args| match args {
        [ConfigValue::Integer(a), ConfigValue::Integer(b)] => Ok(ConfigValue::Integer(a + b)),
        _ => Err("math_add wants two integers".to_owned()),
    }));
    lines.push(format!("available: {}", bridges::is_available("legacy:math")));

    match bridges::call(
        "legacy:math",
        "add",
        &[Foreign::Integer(20), Foreign::Integer(22)],
    ) {
        Ok(Foreign::Integer(sum)) => lines.push(format!("legacy:math::add(20, 22) -> {sum}")),
        other => lines.push(format!("call unexpectedly {other:?}")),
    }

    match bridges::call(
        "legacy:math",
        "add",
        &[Foreign::Text("twenty".to_owned()), Foreign::Integer(22)],
    ) {
        Err(BridgeError::TypeMismatch {
            argument,
            wanted,
            found,
            ..
        }) => lines.push(format!("{argument} wants {wanted}, the value is {found}")),
        other => lines.push(format!("mismatch unexpectedly {other:?}")),
    }

    match bridges::call("legacy:math", "multiply", &[]) {
        Err(BridgeError::UnknownMethod { method, .. }) => {
            lines.push(format!("no method is mapped as {method}"))
        }
        other => lines.push(format!("unknown method unexpectedly {other:?}")),
    }

    match bridges::call("legacy:math", "add", &[Foreign::Integer(1)]) {
        Err(BridgeError::WrongArity { expected, got, .. }) => {
            lines.push(format!("add wants {expected} arguments, the call passed {got}"))
        }
        other => lines.push(format!("arity unexpectedly {other:?}")),
    }

    match bridges::require("legacy:math", Version::new(2, 0, 0)) {
        Err(BridgeError::VersionMismatch {
            requested,
            offered,
            ..
        }) => lines.push(format!("asked for {requested}, the bridge offers {offered}")),
        other => lines.push(format!("version unexpectedly {other:?}")),
    }

    lines.push(format!("client refused: {}", bridges::why_unknown("classes")));

    bridges::unregister("legacy:math");
    lines
}
