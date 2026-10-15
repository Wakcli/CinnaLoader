//! Section 94 demo: the accessibility option set, palettes, descriptions, captions and contrast.

use cinnaloader::accessibility as a11y;
use cinnaloader::chat::{self, TextColor};

pub fn run() -> Vec<String> {
    let mut out = Vec::new();

    let corrected = a11y::AccessibilitySettings {
        reduced_motion: true,
        text_scale: 1.5,
        ..a11y::AccessibilitySettings::default()
    };
    let changes = a11y::set_settings(&corrected).expect("the demo's values fit");
    out.push(format!(
        "option change: {} {} -> {} ({} moved)",
        changes[0].option,
        changes[0].old.render(),
        changes[0].new.render(),
        changes.len()
    ));
    let again = a11y::set_settings(&corrected).expect("the demo's values fit");
    out.push(format!("same-value set fired {} changes", again.len()));
    out.push(format!(
        "reduced_motion={} text_scale={}",
        a11y::get_reduced_motion(),
        a11y::get_text_scale()
    ));

    let palette = a11y::ColorPalette::new("hello:protanopia")
        .replace(TextColor::named("red"), TextColor::named("blue"));
    let listed = palette.lookup(&TextColor::named("red"));
    let unlisted = palette.lookup(&TextColor::named("green"));
    out.push(format!(
        "palette: red -> {} (listed), green -> {} (unchanged={})",
        listed.as_str(),
        unlisted.as_str(),
        unlisted == TextColor::named("green")
    ));
    let message = chat::create_text("danger").color(TextColor::named("red"));
    let mapped = palette.apply(&message);
    out.push(format!(
        "message colours: {:?} -> {:?}",
        message.annotations(),
        mapped.annotations()
    ));

    out.push(format!("button: {:?}", a11y::get_widget_description("button", "Play")));
    out.push(format!("slider: {:?}", a11y::get_widget_description("slider", "Volume")));
    out.push(format!("nothing: {:?}", a11y::get_widget_description("", "")));

    a11y::show_caption("Anvil falls", 2.0, "block.anvil.fall");
    out.push(format!("caption queued: count={}", a11y::caption_count()));
    out.push(format!(
        "advance 1.5 expired {} (count={})",
        a11y::advance_captions(1.5),
        a11y::caption_count()
    ));
    out.push(format!(
        "advance 0.5 expired {} (count={})",
        a11y::advance_captions(0.5),
        a11y::caption_count()
    ));

    out.push(format!(
        "contrast black/white={} readable={}",
        a11y::contrast_ratio((0, 0, 0), (255, 255, 255)),
        a11y::is_readable((0, 0, 0), (255, 255, 255))
    ));

    out
}
