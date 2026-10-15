//! Section 93 demo: a permission set resolved node by node, with the refusals and the round
//! trip.

pub fn run() -> Vec<String> {
    use cinnaloader::server_permissions::{self as perms, Role};

    let mut out = Vec::new();

    perms::register("hello.commands.give");
    perms::register("hello.commands.list");
    perms::register("hello.admin.ban");

    let _ = perms::create_group(Role::new("member", 0).grant("hello.*"));
    let duplicate = perms::create_group(Role::new("member", 1)).unwrap_err().to_string();
    let _ = perms::create_group(Role::new("guest", 0).grant("hello.commands.give"));
    let _ = perms::create_group(Role::new("banned", 0).deny("hello.commands.give"));
    let _ = perms::create_group(Role::new("admin", 10).grant("hello.commands.give"));
    let _ = perms::create_group(Role::new("lead", 0).grant("hello.*").grant("hello.commands.give"));
    let _ = perms::add_to_group("alice", "member");
    let _ = perms::add_to_group("bob", "lead");
    let _ = perms::add_to_group("carol", "guest");
    let _ = perms::add_to_group("carol", "banned");
    let _ = perms::add_to_group("dave", "banned");
    let _ = perms::add_to_group("dave", "admin");

    let deep = perms::resolve("alice", "hello.commands.give");
    let sibling = perms::resolve("alice", "other.thing");
    out.push(format!(
        "wildcard(93): deep={} deep_by={:?} sibling={} sibling_role={:?}",
        deep.allowed, deep.pattern, sibling.allowed, sibling.role
    ));
    assert!(deep.allowed && !sibling.allowed, "hello.* covers a deep node and not a sibling");
    assert_eq!(deep.pattern.as_deref(), Some("hello.*"));

    let specific = perms::resolve("bob", "hello.commands.give");
    let broad = perms::resolve("bob", "hello.commands.list");
    out.push(format!(
        "specific(93): give_by={:?} list_by={:?}",
        specific.pattern, broad.pattern
    ));
    assert_eq!(specific.pattern.as_deref(), Some("hello.commands.give"), "the specific grant decided");
    assert_eq!(broad.pattern.as_deref(), Some("hello.*"), "the wildcard decided the rest");

    let deny = perms::resolve("carol", "hello.commands.give");
    out.push(format!(
        "deny(93): allowed={} decided_by={:?} note={deny}",
        deny.allowed, deny.role
    ));
    assert!(!deny.allowed && deny.role.as_deref() == Some("banned"), "the deny beat the allow");

    let win = perms::resolve("dave", "hello.commands.give");
    out.push(format!(
        "priority(93): allowed={} decided_by={:?} note={win}",
        win.allowed, win.role
    ));
    assert!(win.allowed && win.role.as_deref() == Some("admin"), "the higher-priority role won");

    let unknown = perms::resolve("alice", "other.thing");
    out.push(format!(
        "default(93): allowed={} role={:?} note={unknown} duplicate={duplicate}",
        unknown.allowed, unknown.role
    ));
    assert!(!unknown.allowed && unknown.role.is_none(), "no match is a deny");

    let _ = perms::create_group(Role::new("parent", 0));
    let _ = perms::create_group(Role::new("kid", 0));
    let _ = perms::set_inheritance("parent", Some("kid"));
    let cycle = perms::set_inheritance("kid", Some("parent")).unwrap_err().to_string();
    out.push(format!("cycle(93): refused={cycle}"));
    assert!(cycle.contains("kid") && cycle.contains("parent"), "the cycle is named");

    let _ = perms::create_group(Role::new("vip", 5).grant("hello.*").deny("hello.commands.give").prefix("[VIP]").suffix("!"));
    let before = perms::export_json();
    let _ = perms::import_json(&before);
    let after = perms::export_json();
    out.push(format!(
        "roundtrip(93): equal={} bytes={} roles={}",
        before == after,
        after.len(),
        perms::get_groups().len()
    ));
    assert_eq!(before, after, "the set round trips through serialization");

    let refused = perms::sync_from_server();
    out.push(format!(
        "refuse(93): entrypoint={refused} listed={:?} why={}",
        perms::refusals().iter().map(|(what, _)| what.clone()).collect::<Vec<_>>(),
        perms::why_unknown("server")
    ));
    assert!(!refused, "the real server's state is not reachable");

    out
}
