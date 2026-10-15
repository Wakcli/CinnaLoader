//! Section 77 demo: an entity declaration refused and read back, an instance hurt to death, a
//! ballistic projectile stepped twice, a hit and a miss, a lifetime expiring, and the client
//! refusals.

pub fn run() -> Vec<String> {
    let mut out = Vec::new();

    let nameless = cinnaloader::custom_entities::EntityDefinition::new("", "Nameless");
    let problems = cinnaloader::custom_entities::validate_definition(&nameless);
    let refused = cinnaloader::custom_entities::register_type(nameless);
    out.push(format!(
        "custom_entities(77): empty id refused={refused} problems={} first={:?}",
        problems.len(),
        problems.first()
    ));

    let wisp = cinnaloader::custom_entities::EntityDefinition::new("s77:wisp", "Wisp")
        .hitbox(0.5, 0.5)
        .health(8.0)
        .speed(0.3)
        .model("s77:wisp", "s77:textures/entity/wisp")
        .attribute("minecraft:armor", 2.0);
    let declared = cinnaloader::custom_entities::register_type(wisp);
    let by_mod = cinnaloader::custom_entities::registry_by_mod();
    out.push(format!(
        "custom_entities(77): wisp declared={declared} health={:?} hitbox={:?} per_mod={by_mod:?}",
        cinnaloader::custom_entities::get_definition("s77:wisp").map(|d| d.health),
        cinnaloader::custom_entities::get_definition("s77:wisp").map(|d| d.hitbox)
    ));

    let Some(mut mob) = cinnaloader::custom_entities::spawn("s77:wisp", (0.0, 10.0, 0.0)) else {
        out.push("custom_entities(77): wisp was not declared".to_owned());
        return out;
    };
    let first = mob.hurt(&cinnaloader::effects::damage::DamageInstance::new(
        3.0,
        "minecraft:generic",
    ));
    out.push(format!(
        "custom_entities(77): hurt 3.0 landed={first} health={} alive={}",
        mob.health(),
        mob.is_alive()
    ));
    let rest = mob.take_damage(100.0, "minecraft:generic");
    out.push(format!(
        "custom_entities(77): hurt 100 with armor 2 landed={rest:.2} health={} dead={}",
        mob.health(),
        mob.is_dead()
    ));

    let mut faller = cinnaloader::custom_entities::spawn("s77:wisp", (0.0, 10.0, 0.0))
        .expect("just declared");
    faller.step(1.0);
    out.push(format!(
        "custom_entities(77): gravity step y 10.0 -> {:.4} velocity_y {:.4}",
        faller.position().y,
        faller.velocity().y
    ));

    let arrow = cinnaloader::projectiles::ProjectileType::new("s77:arrow")
        .damage(2.0)
        .speed(1.5)
        .gravity(0.08)
        .drag(0.5)
        .lifetime(3)
        .collision(cinnaloader::projectiles::CollisionBehavior::Bounce);
    let shot_declared = cinnaloader::projectiles::register_type(arrow);
    let Some(mut shot) = cinnaloader::projectiles::create(
        "s77:arrow",
        cinnaloader::math::Vec3::new(0.0, 10.0, 0.0),
    ) else {
        out.push("projectiles(77): arrow was not declared".to_owned());
        return out;
    };
    cinnaloader::projectiles::set_velocity(&mut shot, cinnaloader::math::Vec3::new(1.0, 0.0, 0.0));
    cinnaloader::projectiles::set_drag(&mut shot, 0.5);
    shot.step(1.0);
    shot.step(1.0);
    let p = shot.position();
    let v = shot.velocity();
    out.push(format!(
        "projectiles(77): declared={shot_declared} two steps position ({:.2}, {:.2}, {:.2}) velocity ({}, {}, {}) trail={}",
        p.x, p.y, p.z, v.x, v.y, v.z, shot.trail().len()
    ));

    let mut aimed = cinnaloader::projectiles::create("s77:arrow", cinnaloader::math::Vec3::ZERO)
        .expect("just declared");
    cinnaloader::projectiles::set_velocity(&mut aimed, cinnaloader::math::Vec3::new(2.0, 0.0, 0.0));
    let boxed = cinnaloader::math::Aabb::new(
        cinnaloader::math::Vec3::new(1.0, -0.5, -0.5),
        cinnaloader::math::Vec3::new(2.0, 0.5, 0.5),
    );
    let high = cinnaloader::math::Aabb::new(
        cinnaloader::math::Vec3::new(1.0, 5.0, -0.5),
        cinnaloader::math::Vec3::new(2.0, 6.0, 0.5),
    );
    out.push(format!(
        "projectiles(77): would_hit box in the way={} box above={}",
        aimed.would_hit(&cinnaloader::projectiles::HitTarget::Box(boxed)),
        aimed.would_hit(&cinnaloader::projectiles::HitTarget::Box(high))
    ));

    let mut short = cinnaloader::projectiles::create("s77:arrow", cinnaloader::math::Vec3::ZERO)
        .expect("just declared");
    cinnaloader::projectiles::set_velocity(&mut short, cinnaloader::math::Vec3::X);
    cinnaloader::projectiles::set_gravity(&mut short, 0.0);
    cinnaloader::projectiles::set_drag(&mut short, 1.0);
    cinnaloader::projectiles::set_lifetime(&mut short, 2);
    short.step(1.0);
    let after_one = short.is_alive();
    short.step(1.0);
    out.push(format!(
        "projectiles(77): lifetime 2 alive after step1={after_one} after step2={} trail={}",
        short.is_alive(),
        short.trail().len()
    ));

    out.push(format!(
        "custom_entities(77/refused): {}",
        cinnaloader::custom_entities::why_unknown("store")
    ));
    out.push(format!(
        "projectiles(77/refused): {}",
        cinnaloader::projectiles::why_unknown("world")
    ));
    out.push(format!(
        "refusals(77): custom_entities={} projectiles={}",
        cinnaloader::custom_entities::refusals().len(),
        cinnaloader::projectiles::refusals().len()
    ));

    out
}
