//! Section 87 demo: feature flags with stages, an experimental gate and a deprecation path.

pub fn run() -> Vec<String> {
    use cinnaloader::features::{self, Feature, FeatureStage};

    let mut out = Vec::new();

    for id in ["s87:collision", "s87:netcode", "s87:legacy-hud", "s87:shaders"] {
        features::unregister(id);
    }

    let refused = features::enable("s87:never-declared");
    out.push(format!(
        "features(87): an undeclared feature refused by id = {refused:?}"
    ));

    features::register(Feature::new("s87:collision", "hello-mod", "0.7")).expect("fresh id");
    features::register(Feature::new("s87:netcode", "hello-mod", "0.7").stage(FeatureStage::Experimental))
        .expect("fresh id");
    features::register(Feature::new("s87:legacy-hud", "hello-mod", "0.5").default_enabled(true))
        .expect("fresh id");
    features::register(Feature::new("s87:shaders", "hello-mod", "0.6")).expect("fresh id");
    features::mark_deprecated("s87:shaders", "the shader path was replaced", "0.8", Some("s87:collision"));

    let gated = features::enable("s87:netcode");
    out.push(format!(
        "features(87): experimental enable refused without acknowledgement = {gated:?}"
    ));

    features::acknowledge("s87:netcode", "ada", "2026-10-15T09:00:00Z").expect("declared");
    let enabled = features::enable("s87:netcode");
    let ack = features::get_acknowledgement("s87:netcode");
    out.push(format!(
        "features(87): the same enable accepted after acknowledging = {enabled:?} recorded {ack:?}"
    ));

    let stable = (features::is_enabled("s87:legacy-hud"), features::enable("s87:legacy-hud"));
    out.push(format!(
        "features(87): a stable feature needs no gate, its default and enable = {stable:?}"
    ));

    let warning = features::enable("s87:shaders");
    out.push(format!(
        "features(87): a deprecation warning names the replacement = {warning:?}"
    ));

    out.push(format!(
        "features(87): experimental = {:?} deprecated = {:?}",
        features::get_experimental(),
        features::get_deprecated()
    ));

    let report = features::report();
    out.push(format!(
        "features(87): {} enabled, {} experimental | counts = {:?}",
        report.enabled.len(),
        report.experimental.len(),
        report.counts
    ));
    out.push(format!("features(87): report text = {}", features::report_text()));
    out.push(format!(
        "features(87): report json = {}",
        features::report_json().unwrap_or_default()
    ));

    out.push(format!(
        "features(87): client flags = {:?} | {}",
        features::client_flags(),
        features::why_unknown("flags")
    ));
    out.push(format!(
        "features(87): {} refusals, first = {:?}",
        features::refusals().len(),
        features::refusals().into_iter().next()
    ));

    out
}
