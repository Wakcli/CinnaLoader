//! Section 78 demo: fluent builders that register real blocks and items.

pub fn run() -> Vec<String> {
    use cinnaloader::builders::{BlockBuilder, ItemBuilder};
    use cinnaloader::{blocks, items};

    let mut out = Vec::new();

    blocks::unregister("s78:lamp");
    items::unregister("s78:lens");

    let block_id = BlockBuilder::new()
        .id("s78:lamp")
        .name("Lamp")
        .hardness(1.5)
        .resistance(3.0)
        .sound("metal")
        .light_level(12)
        .drops(&["s78:lamp"])
        .register()
        .expect("the lamp is in range and free");
    out.push(format!("builders(78): chained block registered as {block_id}"));

    out.push(format!(
        "builders(78): read back through blocks::get_hardness(s78:lamp)={:?} light={:?} name={:?}",
        blocks::get_hardness("s78:lamp"),
        blocks::get_light_level("s78:lamp"),
        blocks::get_type("s78:lamp").map(|block| block.name)
    ));

    let item_id = ItemBuilder::new()
        .id("s78:lens")
        .name("Lens")
        .max_stack_size(16)
        .durability(64)
        .texture("s78:lens_icon")
        .register()
        .expect("the lens is in range and free");
    out.push(format!(
        "builders(78): chained item registered as {item_id} max_stack={:?} durability={:?}",
        items::get_max_stack_size("s78:lens"),
        items::get_durability("s78:lens")
    ));

    let no_id = BlockBuilder::new().hardness(2.0).register().expect_err("an empty id");
    out.push(format!("builders(78): a build with no id refused = {no_id}"));

    let duplicate = BlockBuilder::new().id("s78:lamp").register().expect_err("a duplicate");
    out.push(format!("builders(78): a duplicate id refused = {duplicate}"));

    let range = BlockBuilder::new().id("s78:bad").light_level(16).register().expect_err("out of range");
    out.push(format!("builders(78): an out-of-range value refused naming the field = {range}"));

    let mut built = 0;
    for i in 0..5 {
        let id = format!("s78:lamp{i}");
        blocks::unregister(&id);
        if BlockBuilder::new()
            .id(&id)
            .hardness(i as f32)
            .light_level(i as u8)
            .register()
            .is_ok()
        {
            built += 1;
        }
    }
    out.push(format!("builders(78): a loop of five distinct builds registered {built}"));

    out
}
