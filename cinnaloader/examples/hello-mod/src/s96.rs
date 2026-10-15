//! Section 96 demo: an update answer with its kind, a pin that will not move, each policy's
//! decision, the throttle at its boundary, a plan, a verification and the changelog.

use cinnaloader::updates::{self, Channel, Manifest, Policy, Release, Verification};
use cinnaloader::version::Version;

pub fn run() -> Vec<String> {
    let mut out = Vec::new();

    let current = Version::parse("1.2.0");
    let latest = Version::parse("1.3.0");
    let kind = updates::compare(&current, &latest);
    out.push(format!(
        "compare(96): {current} -> {latest} is {} newer={}",
        kind.as_str(),
        updates::is_newer(&current, &latest)
    ));
    assert_eq!(kind.as_str(), "minor");

    let update = Release::new(Manifest::new(
        Version::parse("1.3.0"),
        "https://mods.example/sprint-1.3.0.jar",
        4096,
        updates::hash_of(b"body"),
    ))
    .note("sprint no longer cancels on lava");
    let higher = Release::new(Manifest::new(
        Version::parse("1.4.0"),
        "https://mods.example/sprint-1.4.0.jar",
        8192,
        "deadbeefdeadbeef",
    ));

    let pin = Channel::Pinned(Version::parse("1.2.0"));
    let candidates = [update.clone(), higher.clone()];
    let selected = pin.select(&candidates);
    out.push(format!(
        "pin(96): rule={} selected={:?}",
        pin.rule(),
        selected.map(|rel| rel.manifest.version.to_string())
    ));
    assert_eq!(
        selected.map(|rel| rel.manifest.version.to_string()),
        Some("1.2.0".to_owned()),
        "the pin is the latest candidate, so the caller is never told to move"
    );

    for policy in [Policy::Auto, Policy::Notify, Policy::Manual, Policy::Pinned] {
        let decision = updates::decide(policy, &current, &latest);
        out.push(format!(
            "policy(96): {} -> kind={} prompt={} fetch={}",
            policy.as_str(),
            decision.kind.as_str(),
            decision.prompt,
            decision.fetch
        ));
    }
    assert!(updates::decide(Policy::Auto, &current, &latest).fetch);
    assert!(updates::decide(Policy::Notify, &current, &latest).prompt);
    assert!(!updates::decide(Policy::Manual, &current, &latest).fetch);
    assert_eq!(updates::decide(Policy::Pinned, &current, &latest).kind.as_str(), "up_to_date");

    let mut throttle = updates::Throttle::new(30);
    let first = throttle.allows(0);
    let near = throttle.allows(29);
    let at = throttle.allows(30);
    out.push(format!("throttle(96): tick0={first} tick29={near} tick30={at}"));
    assert!(first && !near && at, "one tick before the interval is refused, at it, allowed");

    let plan = updates::plan_update(&current, &update, "/tmp/cl-mods");
    out.push(format!(
        "plan(96): url={} size={} hash={} dest={} kind={}",
        plan.url,
        plan.size,
        plan.hash,
        plan.destination,
        plan.kind.as_str()
    ));
    assert_eq!(plan.destination, "/tmp/cl-mods/sprint-1.3.0.jar");
    assert_eq!(plan.size, 4096);

    let body = b"a fetched jar".to_vec();
    let good = updates::hash_of(&body);
    let verified = updates::verify_update(&body, &good).is_verified();
    let mismatch = match updates::verify_update(&body, "0000000000000000") {
        Verification::Mismatch { expected, found } => format!("{expected} != {found}"),
        Verification::Verified => "verified".to_owned(),
    };
    out.push(format!("verify(96): good={verified} mismatch={mismatch}"));
    assert!(verified, "the fetched bytes match their hash");
    assert!(mismatch.contains("0000000000000000") && mismatch.contains(&good));

    let mut log = updates::Changelog::new();
    log.add(update.clone());
    log.add(
        Release::new(Manifest::new(Version::parse("1.2.1"), "u", 1, "h"))
            .note("fix crash")
            .note("faster ticks"),
    );
    log.add(higher.clone());
    let entries = log.between(&Version::parse("1.2.0"), &Version::parse("1.4.0"));
    out.push(format!(
        "changelog(96): {} entries [{}]",
        entries.len(),
        entries
            .iter()
            .map(|entry| format!("{}={}", entry.version, entry.text))
            .collect::<Vec<_>>()
            .join("; ")
    ));
    assert_eq!(entries.len(), 3, "1.2.1's two notes and 1.3.0's one");

    out
}
