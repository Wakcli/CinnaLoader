//! Section 54 demo.

use cinnaloader::block_entities::BlockEntityType;
use cinnaloader::config::ConfigValue;
use cinnaloader::items::ItemStack;
use cinnaloader::structures::{Bounds, Direction, Jigsaw, Mirror, Rotation, Structure, Template};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let template = Template::new("hello:hut", (3, 1, 2))
        .named("Hut")
        .block_state((0, 0, 0), "minecraft:oak_planks", &[("facing", "south")])
        .block((2, 0, 1), "minecraft:glass")
        .jigsaw(
            Jigsaw::new((0, 0, 0), "hello:door", Direction::South)
                .target("hello:hall")
                .pool("hello:pools"),
        );

    lines.push(format!(
        "cinnaloader::structures::register_template={} cinnaloader::structures::register={}",
        cinnaloader::structures::register_template(template.clone()),
        cinnaloader::structures::register(Structure::new("hello:hut", "Hut").template("hello:hut")),
    ));

    lines.push(format!(
        "cinnaloader::structures::rotate((2,0,1),(3,1,2)) Cw90/Cw180/Cw270={:?}/{:?}/{:?} Cw90_size={:?}",
        cinnaloader::structures::rotate((2, 0, 1), (3, 1, 2), Rotation::Cw90),
        cinnaloader::structures::rotate((2, 0, 1), (3, 1, 2), Rotation::Cw180),
        cinnaloader::structures::rotate((2, 0, 1), (3, 1, 2), Rotation::Cw270),
        Rotation::Cw90.rotate_size((3, 1, 2)),
    ));

    lines.push(format!(
        "cinnaloader::structures::mirror((0,0,0),(3,1,2)) LR/FB={:?}/{:?} cinnaloader::structures::place_template(Cw90,LR) positions={:?}",
        cinnaloader::structures::mirror((0, 0, 0), (3, 1, 2), Mirror::LeftRight),
        cinnaloader::structures::mirror((0, 0, 0), (3, 1, 2), Mirror::FrontBack),
        cinnaloader::structures::place_template(&template, (0, 0, 0), Rotation::Cw90, Mirror::LeftRight)
            .blocks
            .iter()
            .map(|(pos, _)| *pos)
            .collect::<Vec<_>>(),
    ));

    if let Some(placed) = cinnaloader::structures::place(
        "hello:hut",
        (10, 64, 10),
        Rotation::None,
        Mirror::None,
    ) {
        let target = Bounds::new((0, 64, 0), (31, 80, 31));
        lines.push(format!(
            "cinnaloader::structures::place bounds.size={:?} volume={} corners={} fits_target={} outside={}",
            placed.bounds.size(),
            placed.bounds.volume(),
            placed.bounds.corners().len(),
            placed.fits(&target),
            cinnaloader::structures::place_template(&template, (100, 64, 100), Rotation::None, Mirror::None)
                .fits(&target),
        ));
    }

    let seed_a = cinnaloader::structures::generate("hello:hut", (0, 64, 0), 42)
        .map(|placed| (placed.rotation, placed.mirror));
    let seed_b = cinnaloader::structures::generate("hello:hut", (0, 64, 0), 42)
        .map(|placed| (placed.rotation, placed.mirror));
    lines.push(format!(
        "cinnaloader::structures::generate(seed42)={:?} same={} get_pieces={} piece_dir={:?} get_bounds={:?} refusals={:?}",
        seed_a,
        seed_a == seed_b,
        cinnaloader::structures::get_pieces("hello:hut").len(),
        cinnaloader::structures::get_pieces("hello:hut").first().map(|piece| piece.direction),
        cinnaloader::structures::get_bounds("hello:hut").map(|bounds| bounds.size()),
        cinnaloader::structures::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
    ));

    lines.push(format!(
        "cinnaloader::block_entities::register_type={} cinnaloader::block_entities::create={} cinnaloader::block_entities::get_at={}",
        cinnaloader::block_entities::register_type(
            BlockEntityType::new("hello:furnace", "Furnace")
                .block("minecraft:furnace")
                .inventory(3)
        ),
        cinnaloader::block_entities::create("hello:furnace", (0, 64, 0)).is_some(),
        cinnaloader::block_entities::get_at((0, 64, 0)).is_some(),
    ));

    let id = "hello:furnace@0,64,0";
    cinnaloader::block_entities::set_data(id, "fuel", ConfigValue::Integer(3));
    cinnaloader::block_entities::add_item(id, ItemStack::new("minecraft:coal", 5));
    let ticks = [
        cinnaloader::block_entities::tick(id),
        cinnaloader::block_entities::tick(id),
        cinnaloader::block_entities::tick(id),
        cinnaloader::block_entities::tick(id),
    ];
    lines.push(format!(
        "cinnaloader::block_entities::tick ticks={:?} fuel={:?} lit={:?} get_data={:?}",
        ticks,
        cinnaloader::block_entities::get_data(id, "fuel").and_then(|value| value.as_integer()),
        cinnaloader::block_entities::get_data(id, "lit").and_then(|value| value.as_bool()),
        cinnaloader::block_entities::get_data(id, "fuel"),
    ));

    let saved = cinnaloader::block_entities::save(id);
    let round = saved
        .as_deref()
        .and_then(cinnaloader::block_entities::deserialize);
    lines.push(format!(
        "cinnaloader::block_entities::serialize/deserialize equal={} bytes={:?} load={} inventory0={:?}",
        round == cinnaloader::block_entities::get(id),
        saved.as_deref().map(str::len),
        saved
            .as_deref()
            .and_then(cinnaloader::block_entities::load)
            .is_some(),
        cinnaloader::block_entities::get_inventory(id)
            .and_then(|slots| slots[0].as_ref().map(|stack| (stack.item().to_owned(), stack.count()))),
    ));

    lines.push(format!(
        "cinnaloader::block_entities::refusals={:?} get_viewers={:?} send_update={} receive_update={:?}",
        cinnaloader::block_entities::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
        cinnaloader::block_entities::get_viewers(id),
        cinnaloader::block_entities::send_update(id),
        cinnaloader::block_entities::receive_update("x").is_some(),
    ));

    lines
}
