//! Section 97 demo: an integrity manifest over two files with one mismatch named by both
//! hashes, the four trust decisions, a signed manifest whose fingerprint proves the payload
//! and whose unknown signer is reported distinctly, a capability denied with its missing
//! permission, an audit entry's fields, and the refusal sentence.

use cinnaloader::capability::Capability;
use cinnaloader::fs;
use cinnaloader::security::{self, Decision, MismatchKind, TrustState};

pub fn run() -> Vec<String> {
    let mut out = Vec::new();
    security::clear_audit_log();
    security::clear_security_report();

    let _ = fs::delete("s97verify/a.txt");
    let _ = fs::delete("s97verify/b.txt");
    let wrote = fs::write_text("s97verify/a.txt", "alive").is_ok()
        && fs::write_text("s97verify/b.txt", "one\n").is_ok();
    if wrote {
        let manifest = security::get_mod_integrity("s97verify");
        let clean = security::verify_package(&manifest, "s97verify");
        out.push(format!(
            "integrity(97): entries={} checked={} clean={}",
            manifest.len(),
            clean.checked(),
            clean.is_clean()
        ));
        assert_eq!(manifest.len(), 2, "both files are fingerprinted");

        fs::write_text("s97verify/a.txt", "dead").expect("writable");
        let dirty = security::detect_modified_files(&manifest, "s97verify");
        let changed = dirty.mismatch("a.txt").expect("a.txt changed");
        out.push(format!(
            "mismatch(97): kind={:?} path={} expected_hash={:016x} found_hash={:016x} checked={}",
            changed.kind,
            changed.path,
            changed.expected.expect("recorded").1,
            changed.found.expect("found").1,
            dirty.checked()
        ));
        assert_eq!(changed.kind, MismatchKind::Changed);
        assert_eq!(dirty.mismatches().len(), 1);
        let _ = fs::delete("s97verify/a.txt");
        let _ = fs::delete("s97verify/b.txt");
    } else {
        out.push("integrity(97): no writable mod directory, skipped".to_owned());
    }

    security::distrust("seen-mod");
    security::trust("vouched-mod");
    security::block("bad-mod");
    assert_eq!(security::get_trust("never-seen"), TrustState::Unknown);
    assert_eq!(security::get_trust("seen-mod"), TrustState::Known);
    assert_eq!(security::get_trust("vouched-mod"), TrustState::Trusted);
    assert_eq!(security::get_trust("bad-mod"), TrustState::Blocked);
    for (name, subject) in [
        ("unknown", "never-seen"),
        ("known", "seen-mod"),
        ("trusted", "vouched-mod"),
        ("blocked", "bad-mod"),
    ] {
        out.push(format!(
            "trust(97): {name}={} decide={}",
            security::get_trust(subject),
            security::decide(subject)
        ));
    }

    let payload = b"payload";
    let signed = security::sign("hello-team", payload);
    security::register_trusted_signer("hello-team");
    let matched = security::verify_signature(&signed, payload);
    let mismatched = security::verify_signature(&signed, b"other");
    let unknown = security::verify_signature(&security::sign("nobody", payload), payload);
    out.push(format!(
        "fingerprint(97): recorded={:016x} payload_hash={:016x}",
        signed.fingerprint,
        cinnaloader::serialization::hash(payload)
    ));
    out.push(format!(
        "signature(97): matched={} mismatched={} unknown_signer={}",
        matched.matched(),
        mismatched.as_str(),
        unknown.as_str()
    ));
    assert!(matched.matched() && !mismatched.matched());
    assert!(matches!(unknown, security::SignatureOutcome::UnknownSigner { .. }));

    let denial = security::check_capability(Capability::WorldRead).expect_err("read_world is never granted");
    out.push(format!("capability(97): {denial}"));

    security::evaluate_policy("vouched-mod", 4242);
    let entry = security::get_audit_log()
        .into_iter()
        .find(|entry| entry.at_millis == 4242)
        .expect("just recorded");
    out.push(format!(
        "audit(97): what={} subject={} decision={} why={} at={}",
        entry.action,
        entry.subject,
        entry.decision.as_str(),
        entry.reason,
        entry.at_millis
    ));
    assert_eq!(entry.decision, Decision::Allow);

    let tamper = security::get_security_report();
    out.push(format!("tamper(97): total={} clean={}", tamper.total(), tamper.is_clean()));

    let refused = security::get_client_trust_store();
    out.push(format!(
        "refuse(97): none={} why={}",
        refused.is_none(),
        security::why_unknown("client_trust_store")
    ));
    assert!(refused.is_none());

    out
}
