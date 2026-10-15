//! Section 79 demo.

fn describe(result: Result<cinnaloader::ui::Screen, cinnaloader::builders_ui::BuildError>) -> String {
    match result {
        Ok(screen) => format!("built {} (!)", screen.id),
        Err(error) => error.to_string(),
    }
}

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let builder = cinnaloader::builders_ui::ScreenBuilder::new()
        .id("s79:menu")
        .title("Demo menu")
        .size(320, 240)
        .theme("hello:dark")
        .layout(cinnaloader::ui::Layout::Vertical)
        .add_text("s79:title", "Welcome")
        .add_button("s79:play", "Play");

    let screen = match builder.build() {
        Ok(screen) => screen,
        Err(error) => {
            lines.push(format!("build refused: {error}"));
            return lines;
        }
    };
    lines.push(format!(
        "built title={:?} size={:?} theme={:?} layout={:?} widgets={:?}",
        screen.title,
        builder.pixel_size(),
        screen.theme,
        screen.layout,
        screen.widgets.iter().map(|widget| widget.id.as_str()).collect::<Vec<_>>(),
    ));
    let placed = builder.place();
    lines.push(format!("placed title={:?} play={:?}", placed[0].1, placed[1].1));

    let dup_widget = cinnaloader::builders_ui::ScreenBuilder::new()
        .id("s79:dup")
        .title("Dup")
        .add_button("s79:a", "A")
        .add_text("s79:a", "again")
        .build();
    lines.push(format!("duplicate widget id refused: {}", describe(dup_widget)));

    let mut child = cinnaloader::widget::WidgetNode::new(
        "s79:child",
        cinnaloader::widget::WidgetType::TextLabel,
        "inside",
    );
    child.parent = Some("s79:panel".to_owned());
    let missing_parent = cinnaloader::builders_ui::ScreenBuilder::new()
        .id("s79:orphan")
        .title("Orphan")
        .add_widget(child)
        .build();
    lines.push(format!("missing parent refused: {}", describe(missing_parent)));

    lines.push(format!("duplicate screen id refused: {}", describe(builder.build())));

    let shown = builder.show();
    lines.push(format!(
        "show on the client={shown}: {}",
        cinnaloader::builders_ui::why_unknown("show"),
    ));
    lines.push(format!(
        "refusals={} handlers={}",
        cinnaloader::builders_ui::refusals().len(),
        cinnaloader::builders_ui::get_handlers("s79:menu").len(),
    ));

    lines
}
