//! Section 73: mods declaring APIs to each other, resolving a request, calling it and
//! refusing what the client's own API would be needed for.

use cinnaloader::capability::Capability;
use cinnaloader::config::ConfigValue;
use cinnaloader::interop::{
    self, ApiError, MethodSpec, ModApi, ReturnKind, Service, Version,
};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let api = ModApi::new("hello:interop:dice", Version::new(1, 2, 0))
        .capability(Capability::FilesystemRead)
        .method(MethodSpec::new("add", 2, ReturnKind::Int))
        .method(MethodSpec::new("roll", 1, ReturnKind::Int));
    let published = interop::register_service(
        Service::new(api, "hello:dice-mod", Version::new(1, 0, 0)),
        |args| match args {
            [ConfigValue::Integer(a), ConfigValue::Integer(b)] => Ok(ConfigValue::Integer(a + b)),
            [ConfigValue::Integer(sides)] => Ok(ConfigValue::Integer(sides * 2)),
            _ => Err("dice: expected integers".to_owned()),
        },
    );
    lines.push(format!(
        "interop::register_service(hello:interop:dice 1.2.0)={} has_mod={} get_api_version={:?} get_mod_version={:?}",
        published,
        interop::has_mod("hello:interop:dice"),
        interop::get_api_version("hello:interop:dice"),
        interop::get_mod_version("hello:dice-mod"),
    ));

    match interop::require("hello:interop:dice", Version::new(1, 0, 0)) {
        Ok(service) => lines.push(format!(
            "interop::require(hello:interop:dice, 1.0.0) resolved: provider={} offered={} (same major, minor 2>=0)",
            service.provider, service.api.version,
        )),
        Err(error) => lines.push(format!("interop::require(1.0.0) unexpectedly refused: {error}")),
    }

    match interop::require("hello:interop:dice", Version::new(2, 0, 0)) {
        Ok(_) => lines.push("interop::require(2.0.0) unexpectedly succeeded".to_owned()),
        Err(error) => lines.push(format!(
            "interop::require(hello:interop:dice, 2.0.0) refused, both versions named: {error}"
        )),
    }

    match interop::require_capability(
        "hello:interop:dice",
        Version::new(1, 0, 0),
        Capability::PacketSend,
    ) {
        Ok(_) => lines.push("missing-capability lookup unexpectedly succeeded".to_owned()),
        Err(error) => lines.push(format!(
            "interop::require_capability(..., packet_send) refused naming the capability: {error}"
        )),
    }

    match interop::call_service(
        "hello:interop:dice",
        "add",
        &[ConfigValue::Integer(20), ConfigValue::Integer(22)],
    ) {
        Ok(value) => lines.push(format!(
            "interop::call_service(hello:interop:dice, add, [20,22]) answered a value: {value:?}"
        )),
        Err(error) => lines.push(format!("interop::call_service(add) unexpectedly refused: {error}")),
    }

    match interop::call_service("hello:interop:dice", "add", &[ConfigValue::Integer(1)]) {
        Ok(_) => lines.push("wrong-arity call unexpectedly succeeded".to_owned()),
        Err(error) => lines.push(format!(
            "interop::call_service(hello:interop:dice, add, [1]) refused with both counts: {error}"
        )),
    }

    match interop::call_service("hello:interop:dice", "multiply", &[]) {
        Ok(_) => lines.push("unknown-method call unexpectedly succeeded".to_owned()),
        Err(ApiError::UnknownMethod { method, .. }) => {
            lines.push(format!("interop::call_service(hello:interop:dice, multiply) refused by name: no method {method}"))
        }
        Err(error) => lines.push(format!("unknown-method call refused differently: {error}")),
    }

    let first = interop::subscribe_for("hello:listener-a", "hello:interop:tick", |_value| {});
    let second = interop::subscribe_for("hello:listener-b", "hello:interop:tick", |_value| {});
    let delivered = interop::send_message("hello:interop:tick", ConfigValue::Integer(1));
    interop::unsubscribe(first);
    interop::unsubscribe(second);
    lines.push(format!(
        "interop::subscribe_for x2 then send_message(hello:interop:tick): delivered={delivered} distinct subscribers"
    ));

    lines.push(format!(
        "interop::get_public_api()={:?} client_capabilities()={:?} call_client refused={:?}",
        interop::get_public_api(),
        interop::client_capabilities(),
        interop::call_client("tick", &[]),
    ));

    lines
}
