//! Section 64 demo.

use std::collections::BTreeSet;

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let mut weather = cinnaloader::environment::WeatherMachine::with_weather(
        7,
        cinnaloader::environment::WeatherState::Rain,
        400,
    );
    let start = weather.rain_level();
    weather.advance(50);
    let half = weather.rain_level();
    weather.advance(50);
    let full = weather.rain_level();
    let thunder = weather.thunder_level();
    weather.set_weather(cinnaloader::environment::WeatherState::Clear);
    weather.advance(50);
    lines.push(format!(
        "cinnaloader::environment::WeatherMachine rain ramp after 50/100 ticks = {start}/{half}/{full}, after Clear 50 = {} (thunder {thunder})",
        weather.rain_level(),
    ));

    let mut left = cinnaloader::environment::WeatherMachine::new(42);
    let mut right = cinnaloader::environment::WeatherMachine::new(42);
    let mut states = Vec::new();
    let mut same = true;
    for _ in 0..64 {
        same &= left.weather() == right.weather();
        states.push(left.weather().as_str().to_owned());
        left.advance(500);
        right.advance(500);
    }
    let distinct: BTreeSet<&str> = states.iter().map(String::as_str).collect();
    lines.push(format!(
        "cinnaloader::environment::WeatherMachine::new(42) two runs same sequence = {same}, distinct states = {} ({distinct:?})",
        distinct.len(),
    ));

    let light = cinnaloader::environment::LightModel::new()
        .with_source(cinnaloader::environment::LightSource::block((0, 0, 0), 15));
    lines.push(format!(
        "cinnaloader::environment::LightModel block source 15 at 0/1/5/15 blocks = {}/{}/{}/{}",
        light.block_light_at(0, 0, 0),
        light.block_light_at(1, 0, 0),
        light.block_light_at(5, 0, 0),
        light.block_light_at(15, 0, 0),
    ));

    let sky = cinnaloader::environment::LightModel::new()
        .with_source(cinnaloader::environment::LightSource::sky((0, 0, 0), 15));
    let day = sky.clone().with_day_time(6000).light_at(0, 0, 0);
    let night = sky.with_day_time(18_000).light_at(0, 0, 0);
    lines.push(format!(
        "cinnaloader::environment::LightModel sky 15 at noon(6000)/midnight(18000) = {day}/{night} (darken {}/{})",
        cinnaloader::environment::sky_darken(6000),
        cinnaloader::environment::sky_darken(18_000),
    ));

    let _ = cinnaloader::environment::register_climate(
        cinnaloader::environment::BiomeClimate::new("hello:tiaga", -0.5)
            .humidity(0.8)
            .downfall(0.4),
    );
    lines.push(format!(
        "cinnaloader::environment temperature_at(hello:tiaga) = {:?}, category = {:?}, precipitation = {:?}, snows = {}",
        cinnaloader::environment::temperature_at("hello:tiaga"),
        cinnaloader::environment::get_climate("hello:tiaga").map(|climate| climate.category),
        cinnaloader::environment::precipitation_at("hello:tiaga"),
        cinnaloader::environment::snows_at("hello:tiaga"),
    ));

    let clock = cinnaloader::environment::EnvironmentClock::new();
    let fog = cinnaloader::environment::EnvironmentEffect::new(
        "hello:fog",
        cinnaloader::environment::EffectKind::Fog,
    )
    .duration(100)
    .amplifier(2);
    let mut running = cinnaloader::environment::EffectSet::new();
    running.apply(&fog, &clock);
    let at99 = running.active_at(99).len();
    let at100 = running.active_at(100).len();
    let mut later = cinnaloader::environment::EnvironmentClock::new();
    later.advance(100);
    let expired = running.expire(&later);
    lines.push(format!(
        "cinnaloader::environment::EffectSet effect 100 ticks active at 99/100 = {at99}/{at100}, expire(clock=100) removed = {expired}, left = {}",
        running.active_at(100).len(),
    ));

    let registered = format!(
        "{}/{}/{}",
        cinnaloader::environment::register_weather(cinnaloader::environment::WeatherType::new(
            "hello:storm",
            "Storm",
            cinnaloader::environment::WeatherState::Thunder,
        )),
        cinnaloader::environment::register_weather_effect(
            cinnaloader::environment::EnvironmentEffect::new(
                "hello:mist",
                cinnaloader::environment::EffectKind::Fog,
            )
            .duration(600),
        ),
        cinnaloader::environment::register_sky_renderer(
            cinnaloader::environment::EnvironmentPiece::new(
                cinnaloader::environment::PieceKind::SkyRenderer,
                "hello:sky",
            )
            .named("Sky")
            .setting("shader", "hello:sky"),
        ),
    );
    lines.push(format!(
        "cinnaloader::environment register weather/effect/sky = {registered}, refusals = {:?}",
        cinnaloader::environment::refusals()
            .into_iter()
            .map(|(what, _)| what)
            .collect::<Vec<_>>(),
    ));

    lines
}
