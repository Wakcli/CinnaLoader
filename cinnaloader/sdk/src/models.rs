//! Models and animations: the geometry a mod ships, and the playback it can drive itself.
//!
//! A model here is real data: bones with a parent and a transform, parts with vertices,
//! indices, a material and a texture, a visibility flag each, and bounds worked out from the
//! vertices. A mod registers one, reads it back, builds one with `create`/`create_mesh`/
//! `create_bone`, sets materials and transforms, and exports and imports it as text, which is
//! a real round trip.
//!
//! Animations are real too, and this is the section where a mod can do genuine work: keyframes
//! per bone, a time, a speed, looping, blending between animations by weight, a crossfade from
//! one to another, parameters a mod sets and reads, controllers that group animations, and a
//! skeleton whose bone transforms are *sampled from the keyframes* at the current time. What
//! the client does with all of that — drawing it, playing it in its own renderer — is
//! refused, and so are its model loaders and renderer registrations, which are recorded as
//! declarations and never called here.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! models::register(Model::new("hello:block").bone(Bone::new("body")).part(
//!     ModelPart::new("body", "body")
//!         .vertices(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
//!         .indices(&[0, 1, 2]),
//! ));
//! assert_eq!(models::get_bounds("hello:block").map(|b| b.1), Some((1.0, 1.0, 1.0)));
//! assert!(models::load("hello:block") == false, "the client loads models, not this host");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// A point in three dimensions.
pub type Vec3 = (f32, f32, f32);

/// The box a model or a part occupies, as its lowest and highest corner.
pub type Bounds = (Vec3, Vec3);

/// Where a bone sits, and what it is scaled and turned by, in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoneTransform {
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
}

impl BoneTransform {
    /// The transform nothing has moved yet.
    #[must_use]
    pub fn identity() -> Self {
        Self {
            position: (0.0, 0.0, 0.0),
            rotation: (0.0, 0.0, 0.0),
            scale: (1.0, 1.0, 1.0),
        }
    }

    #[must_use]
    pub fn at(x: f32, y: f32, z: f32) -> Self {
        Self {
            position: (x, y, z),
            ..Self::identity()
        }
    }

    #[must_use]
    pub fn rotated(yaw: f32, pitch: f32, roll: f32) -> Self {
        Self {
            rotation: (yaw, pitch, roll),
            ..Self::identity()
        }
    }

    /// Moves a point by this transform, in the order a bone applies them: scale, then
    /// rotate, then translate.
    #[must_use]
    pub fn apply(&self, point: Vec3) -> Vec3 {
        let scaled = (
            point.0 * self.scale.0,
            point.1 * self.scale.1,
            point.2 * self.scale.2,
        );
        let (yaw, pitch, roll) = (
            self.rotation.0.to_radians(),
            self.rotation.1.to_radians(),
            self.rotation.2.to_radians(),
        );
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let (sin_roll, cos_roll) = roll.sin_cos();
        let (x, y, z) = scaled;
        // Yaw about y, pitch about x, roll about z.
        let (x, z) = (x * cos_yaw + z * sin_yaw, -x * sin_yaw + z * cos_yaw);
        let (y, z) = (y * cos_pitch - z * sin_pitch, y * sin_pitch + z * cos_pitch);
        let (x, y) = (x * cos_roll - y * sin_roll, x * sin_roll + y * cos_roll);
        (
            x + self.position.0,
            y + self.position.1,
            z + self.position.2,
        )
    }

    /// The halfway transform between this and another, which is what a blend does.
    #[must_use]
    pub fn blend(&self, other: &Self, weight: f32) -> Self {
        let weight = weight.clamp(0.0, 1.0);
        let mix = |left: f32, right: f32| left + (right - left) * weight;
        let mix3 = |left: Vec3, right: Vec3| {
            (mix(left.0, right.0), mix(left.1, right.1), mix(left.2, right.2))
        };
        Self {
            position: mix3(self.position, other.position),
            rotation: mix3(self.rotation, other.rotation),
            scale: mix3(self.scale, other.scale),
        }
    }
}

impl fmt::Display for BoneTransform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "at ({:.2}, {:.2}, {:.2}) turned ({:.1}, {:.1}, {:.1})",
            self.position.0, self.position.1, self.position.2,
            self.rotation.0, self.rotation.1, self.rotation.2
        )
    }
}

/// A bone: a name, what it hangs off, and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<String>,
    pub transform: BoneTransform,
    pub visible: bool,
}

impl Bone {
    /// A bone at the origin, attached to nothing.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            parent: None,
            transform: BoneTransform::identity(),
            visible: true,
        }
    }

    #[must_use]
    pub fn parent(mut self, parent: &str) -> Self {
        self.parent = Some(parent.to_owned());
        self
    }

    #[must_use]
    pub fn transform(mut self, transform: BoneTransform) -> Self {
        self.transform = transform;
        self
    }

    #[must_use]
    pub fn at(mut self, x: f32, y: f32, z: f32) -> Self {
        self.transform = BoneTransform::at(x, y, z);
        self
    }
}

/// A part of a model: the geometry a bone carries.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPart {
    pub name: String,
    pub bone: String,
    /// Vertices, three floats each.
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub material: String,
    pub texture: String,
    pub visible: bool,
}

impl ModelPart {
    /// An empty part on a bone.
    #[must_use]
    pub fn new(name: &str, bone: &str) -> Self {
        Self {
            name: name.to_owned(),
            bone: bone.to_owned(),
            vertices: Vec::new(),
            indices: Vec::new(),
            material: "minecraft:entity".to_owned(),
            texture: String::new(),
            visible: true,
        }
    }

    /// Vertices, three floats each, with the count checked.
    pub fn vertices(mut self, vertices: &[f32]) -> Self {
        if vertices.len().is_multiple_of(3) {
            self.vertices = vertices.to_vec();
        }
        self
    }

    #[must_use]
    pub fn indices(mut self, indices: &[u32]) -> Self {
        self.indices = indices.to_vec();
        self
    }

    #[must_use]
    pub fn material(mut self, material: &str) -> Self {
        self.material = material.to_owned();
        self
    }

    #[must_use]
    pub fn texture(mut self, texture: &str) -> Self {
        self.texture = texture.to_owned();
        self
    }

    /// How many vertices it has.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / 3
    }

    /// How many triangles its indices make.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The box that holds every vertex.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        let mut low = (f32::MAX, f32::MAX, f32::MAX);
        let mut high = (f32::MIN, f32::MIN, f32::MIN);
        for vertex in self.vertices.as_chunks::<3>().0 {
            low = (low.0.min(vertex[0]), low.1.min(vertex[1]), low.2.min(vertex[2]));
            high = (high.0.max(vertex[0]), high.1.max(vertex[1]), high.2.max(vertex[2]));
        }
        if self.vertices.is_empty() {
            return ((0.0, 0.0, 0.0), (0.0, 0.0, 0.0));
        }
        (low, high)
    }
}

/// A model a mod ships.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub owner: String,
    pub bones: Vec<Bone>,
    pub parts: Vec<ModelPart>,
    /// A loader name, for a mod that has its own format.
    pub loader: String,
}

impl Model {
    /// A model with no bones or parts yet.
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            bones: Vec::new(),
            parts: Vec::new(),
            loader: String::new(),
        }
    }

    /// The same, under the name the wishlist uses for building one.
    #[must_use]
    pub fn create(id: &str) -> Self {
        Self::new(id)
    }

    #[must_use]
    pub fn loader(mut self, loader: &str) -> Self {
        self.loader = loader.to_owned();
        self
    }

    #[must_use]
    pub fn bone(mut self, bone: Bone) -> Self {
        self.bones.retain(|held| held.name != bone.name);
        self.bones.push(bone);
        self
    }

    #[must_use]
    pub fn part(mut self, part: ModelPart) -> Self {
        self.parts.retain(|held| held.name != part.name);
        self.parts.push(part);
        self
    }

    /// A bone by name.
    #[must_use]
    pub fn bone_of(&self, name: &str) -> Option<&Bone> {
        self.bones.iter().find(|bone| bone.name == name)
    }

    /// A part by name.
    #[must_use]
    pub fn part_of(&self, name: &str) -> Option<&ModelPart> {
        self.parts.iter().find(|part| part.name == name)
    }

    /// Everything the model's parts hold, as one box.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        let mut low = (f32::MAX, f32::MAX, f32::MAX);
        let mut high = (f32::MIN, f32::MIN, f32::MIN);
        for part in &self.parts {
            let (part_low, part_high) = part.bounds();
            low = (
                low.0.min(part_low.0),
                low.1.min(part_low.1),
                low.2.min(part_low.2),
            );
            high = (
                high.0.max(part_high.0),
                high.1.max(part_high.1),
                high.2.max(part_high.2),
            );
        }
        if self.parts.is_empty() {
            return ((0.0, 0.0, 0.0), (0.0, 0.0, 0.0));
        }
        (low, high)
    }

    /// Every bone a bone hangs off, walked from a part up to the root.
    #[must_use]
    pub fn chain_of(&self, bone: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = Some(bone.to_owned());
        while let Some(name) = current {
            if chain.contains(&name) {
                break;
            }
            chain.push(name.clone());
            current = self.bone_of(&name).and_then(|bone| bone.parent.clone());
        }
        chain
    }

    /// The model as text, one line per bone and part, which is the round trip `import` reads.
    #[must_use]
    pub fn export(&self) -> String {
        let mut text = format!("model {}\n", self.id);
        for bone in &self.bones {
            text.push_str(&format!(
                "bone {} parent={} at={},{},{} rot={},{},{}\n",
                bone.name,
                bone.parent.clone().unwrap_or_default(),
                bone.transform.position.0,
                bone.transform.position.1,
                bone.transform.position.2,
                bone.transform.rotation.0,
                bone.transform.rotation.1,
                bone.transform.rotation.2
            ));
        }
        for part in &self.parts {
            text.push_str(&format!(
                "part {} bone={} vertices={} indices={} material={} texture={}\n",
                part.name,
                part.bone,
                part.vertices.len(),
                part.indices.len(),
                part.material,
                part.texture
            ));
        }
        text
    }

    /// Reads what [`Model::export`] wrote, which carries the counts and the transforms but not
    /// the vertex values themselves — a caller that wants those keeps its own source.
    #[must_use]
    pub fn import(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let header = lines.next()?;
        let id = header.strip_prefix("model ")?.trim();
        let mut model = Self::new(id);
        for line in lines {
            let mut fields = line.split_whitespace();
            match fields.next()? {
                "bone" => {
                    let name = fields.next()?.to_owned();
                    let mut bone = Bone::new(&name);
                    for field in fields {
                        let (key, value) = field.split_once('=')?;
                        match key {
                            "parent" if !value.is_empty() => bone = bone.parent(value),
                            "at" => {
                                let parts: Vec<f32> =
                                    value.split(',').filter_map(|p| p.parse().ok()).collect();
                                if parts.len() == 3 {
                                    bone.transform.position = (parts[0], parts[1], parts[2]);
                                }
                            }
                            "rot" => {
                                let parts: Vec<f32> =
                                    value.split(',').filter_map(|p| p.parse().ok()).collect();
                                if parts.len() == 3 {
                                    bone.transform.rotation = (parts[0], parts[1], parts[2]);
                                }
                            }
                            _ => {}
                        }
                    }
                    model = model.bone(bone);
                }
                "part" => {
                    let name = fields.next()?.to_owned();
                    let mut part = ModelPart::new(&name, "");
                    for field in fields {
                        let (key, value) = field.split_once('=')?;
                        match key {
                            "bone" => part.bone = value.to_owned(),
                            "vertices" => {
                                let count: usize = value.parse().ok()?;
                                part.vertices = vec![0.0; count];
                            }
                            "indices" => {
                                let count: usize = value.parse().ok()?;
                                part.indices = vec![0; count];
                            }
                            "material" => part.material = value.to_owned(),
                            "texture" => part.texture = value.to_owned(),
                            _ => {}
                        }
                    }
                    model = model.part(part);
                }
                _ => {}
            }
        }
        Some(model)
    }
}

impl fmt::Display for Model {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({} bone(s), {} part(s), {} triangle(s))",
            self.id,
            self.bones.len(),
            self.parts.len(),
            self.parts.iter().map(ModelPart::triangle_count).sum::<usize>()
        )
    }
}

/// One change to a bone at a moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe {
    pub bone: String,
    /// When it happens, in milliseconds from the start.
    pub time: u64,
    pub transform: BoneTransform,
}

impl Keyframe {
    #[must_use]
    pub fn new(bone: &str, time: u64, transform: BoneTransform) -> Self {
        Self {
            bone: bone.to_owned(),
            time,
            transform,
        }
    }
}

/// How an animation repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LoopMode {
    /// Plays once and holds the last frame.
    Once,
    #[default]
    /// Starts again at the end.
    Loop,
    /// Plays forwards, then backwards.
    PingPong,
    /// Holds the last frame, whatever the length.
    Hold,
}

/// An animation a mod ships, as keyframes per bone.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub id: String,
    pub owner: String,
    pub keyframes: Vec<Keyframe>,
    /// How long it is, when the keyframes do not say.
    pub length: u64,
    pub loop_mode: LoopMode,
}

impl Animation {
    /// An animation with no keyframes yet.
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            keyframes: Vec::new(),
            length: 0,
            loop_mode: LoopMode::Loop,
        }
    }

    #[must_use]
    pub fn frame(mut self, keyframe: Keyframe) -> Self {
        self.length = self.length.max(keyframe.time);
        self.keyframes.push(keyframe);
        self
    }

    #[must_use]
    pub fn looping(mut self, mode: LoopMode) -> Self {
        self.loop_mode = mode;
        self
    }

    /// How long it runs, from its keyframes or from what a caller set.
    #[must_use]
    pub fn length(&self) -> u64 {
        self.length.max(
            self.keyframes
                .iter()
                .map(|keyframe| keyframe.time)
                .max()
                .unwrap_or(0),
        )
    }

    /// Where a moment lands in the animation, following the loop mode.
    #[must_use]
    pub fn time_at(&self, elapsed_millis: u64) -> u64 {
        let length = self.length();
        if length == 0 {
            return 0;
        }
        match self.loop_mode {
            LoopMode::Once | LoopMode::Hold => elapsed_millis.min(length),
            LoopMode::Loop => elapsed_millis % length,
            LoopMode::PingPong => {
                let cycle = length * 2;
                let position = elapsed_millis % cycle;
                if position <= length {
                    position
                } else {
                    cycle - position
                }
            }
        }
    }

    /// The transform a bone has at a moment, interpolated between the keyframes around it.
    #[must_use]
    pub fn sample(&self, bone: &str, elapsed_millis: u64) -> Option<BoneTransform> {
        let time = self.time_at(elapsed_millis);
        let mut frames: Vec<&Keyframe> = self
            .keyframes
            .iter()
            .filter(|keyframe| keyframe.bone == bone)
            .collect();
        if frames.is_empty() {
            return None;
        }
        frames.sort_by_key(|keyframe| keyframe.time);
        if time <= frames[0].time {
            return Some(frames[0].transform);
        }
        let last = frames.last()?;
        if time >= last.time {
            return Some(last.transform);
        }
        for pair in frames.windows(2) {
            let (before, after) = (pair[0], pair[1]);
            if time >= before.time && time <= after.time {
                let span = (after.time - before.time) as f32;
                let weight = if span == 0.0 {
                    0.0
                } else {
                    (time - before.time) as f32 / span
                };
                return Some(before.transform.blend(&after.transform, weight));
            }
        }
        Some(last.transform)
    }

    /// Every bone it drives.
    #[must_use]
    pub fn bones(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .keyframes
            .iter()
            .map(|keyframe| keyframe.bone.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

impl fmt::Display for Animation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({} keyframe(s), {} ms, {:?})",
            self.id,
            self.keyframes.len(),
            self.length(),
            self.loop_mode
        )
    }
}

/// A playing animation, with the state that goes with playing one.
#[derive(Debug, Clone, PartialEq)]
pub struct Playback {
    pub animation: String,
    pub time: u64,
    pub speed: f32,
    pub looping: bool,
    pub paused: bool,
    /// How much it counts when it is blended with others.
    pub weight: f32,
    /// Whether it has reached the end and is holding.
    pub finished: bool,
}

impl Playback {
    /// A playback that has just started.
    #[must_use]
    pub fn new(animation: &str) -> Self {
        Self {
            animation: animation.to_owned(),
            time: 0,
            speed: 1.0,
            looping: true,
            paused: false,
            weight: 1.0,
            finished: false,
        }
    }

    /// Moves it on by a slice of time, following its speed and looping.
    pub fn advance(&mut self, delta_millis: u64, length: u64) {
        if self.paused || self.finished {
            return;
        }
        let step = (delta_millis as f32 * self.speed.max(0.0)) as u64;
        self.time += step;
        if !self.looping && length > 0 && self.time >= length {
            self.time = length;
            self.finished = true;
        } else if self.looping && length > 0 {
            self.time %= length;
        }
    }
}

/// Every animation and model a mod registered, and what is playing, all in one place.
#[derive(Debug, Default, Clone)]
pub struct AnimationController {
    playbacks: Vec<Playback>,
    parameters: BTreeMap<String, f32>,
    /// How long a crossfade takes, and what it is fading from.
    crossfade: Option<(String, u64)>,
}

impl AnimationController {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts an animation, or replaces the playback of the same name.
    pub fn play(&mut self, animation: &str) -> bool {
        if animation.is_empty() {
            return false;
        }
        self.playbacks.retain(|playback| playback.animation != animation);
        self.playbacks.push(Playback::new(animation));
        true
    }

    /// Pauses one.
    pub fn pause(&mut self, animation: &str) -> bool {
        match self
            .playbacks
            .iter_mut()
            .find(|playback| playback.animation == animation)
        {
            Some(playback) => {
                playback.paused = true;
                true
            }
            None => false,
        }
    }

    /// Starts it again from where it was.
    pub fn resume(&mut self, animation: &str) -> bool {
        match self
            .playbacks
            .iter_mut()
            .find(|playback| playback.animation == animation)
        {
            Some(playback) => {
                playback.paused = false;
                playback.finished = false;
                true
            }
            None => false,
        }
    }

    /// Stops one.
    pub fn stop(&mut self, animation: &str) -> bool {
        let before = self.playbacks.len();
        self.playbacks.retain(|playback| playback.animation != animation);
        self.playbacks.len() != before
    }

    /// Stops every one, and answers how many there were.
    pub fn stop_all(&mut self) -> usize {
        let count = self.playbacks.len();
        self.playbacks.clear();
        count
    }

    /// One playback.
    #[must_use]
    pub fn get(&self, animation: &str) -> Option<Playback> {
        self.playbacks
            .iter()
            .find(|playback| playback.animation == animation)
            .cloned()
    }

    /// Every playback, in the order they started.
    #[must_use]
    pub fn all(&self) -> Vec<Playback> {
        self.playbacks.clone()
    }

    /// Moves every playback on by a slice of time, given each animation's length.
    pub fn advance(&mut self, delta_millis: u64, length_of: impl Fn(&str) -> u64) {
        if let Some((from, remaining)) = self.crossfade.clone() {
            let step = delta_millis.min(remaining);
            let done = remaining - step;
            let progress = if remaining == 0 {
                1.0
            } else {
                1.0 - (done as f32 / remaining as f32)
            };
            let (fading_out, fading_in): (Vec<&mut Playback>, Vec<&mut Playback>) = {
                let (a, b): (Vec<&mut Playback>, Vec<&mut Playback>) = self
                    .playbacks
                    .iter_mut()
                    .partition(|playback| playback.animation == from);
                (a, b)
            };
            for playback in fading_out {
                playback.weight = 1.0 - progress;
            }
            for playback in fading_in {
                playback.weight = progress.min(1.0);
            }
            self.crossfade = if done == 0 { None } else { Some((from, done)) };
        }
        for playback in &mut self.playbacks {
            let length = length_of(&playback.animation);
            playback.advance(delta_millis, length);
        }
    }

    /// Sets how fast one runs.
    pub fn set_speed(&mut self, animation: &str, speed: f32) -> bool {
        match self
            .playbacks
            .iter_mut()
            .find(|playback| playback.animation == animation)
        {
            Some(playback) => {
                playback.speed = speed.max(0.0);
                true
            }
            None => false,
        }
    }

    /// Sets whether it loops.
    pub fn set_looping(&mut self, animation: &str, looping: bool) -> bool {
        match self
            .playbacks
            .iter_mut()
            .find(|playback| playback.animation == animation)
        {
            Some(playback) => {
                playback.looping = looping;
                true
            }
            None => false,
        }
    }

    /// Sets how much it counts when blended.
    pub fn set_blend_weight(&mut self, animation: &str, weight: f32) -> bool {
        match self
            .playbacks
            .iter_mut()
            .find(|playback| playback.animation == animation)
        {
            Some(playback) => {
                playback.weight = weight.clamp(0.0, 1.0);
                true
            }
            None => false,
        }
    }

    /// The weights of everything playing, which is what a blend is drawn with.
    #[must_use]
    pub fn blend(&self) -> Vec<(String, f32)> {
        let total: f32 = self.playbacks.iter().map(|playback| playback.weight).sum();
        if total <= 0.0 {
            return Vec::new();
        }
        self.playbacks
            .iter()
            .map(|playback| (playback.animation.clone(), playback.weight / total))
            .collect()
    }

    /// Fades from one animation to another over a time, which starts the new one.
    pub fn crossfade(&mut self, from: &str, to: &str, duration_millis: u64) -> bool {
        // The animation being faded to is the one that starts playing; the one being faded
        // from keeps running while its weight falls.
        if !self.play(to) {
            return false;
        }
        self.crossfade = Some((from.to_owned(), duration_millis));
        true
    }

    /// Sets how far into a playback is, and answers whether it is playing at all.
    pub fn set_time(&mut self, animation: &str, time: u64) -> bool {
        match self
            .playbacks
            .iter_mut()
            .find(|playback| playback.animation == animation)
        {
            Some(playback) => {
                playback.time = time;
                playback.finished = false;
                true
            }
            None => false,
        }
    }

    /// Sets a parameter an animation can be driven by.
    pub fn set_parameter(&mut self, name: &str, value: f32) {
        self.parameters.insert(name.to_owned(), value);
    }

    /// A parameter.
    #[must_use]
    pub fn get_parameter(&self, name: &str) -> Option<f32> {
        self.parameters.get(name).copied()
    }

    /// Every parameter.
    #[must_use]
    pub fn parameters(&self) -> Vec<(String, f32)> {
        self.parameters
            .iter()
            .map(|(name, value)| (name.clone(), *value))
            .collect()
    }

    /// Whether a crossfade is running.
    #[must_use]
    pub fn is_crossfading(&self) -> bool {
        self.crossfade.is_some()
    }
}

fn models() -> &'static Mutex<BTreeMap<String, Model>> {
    static MODELS: OnceLock<Mutex<BTreeMap<String, Model>>> = OnceLock::new();
    MODELS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, Model>) -> T) -> T {
    let mut guard = models().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn animations() -> &'static Mutex<BTreeMap<String, Animation>> {
    static ANIMATIONS: OnceLock<Mutex<BTreeMap<String, Animation>>> = OnceLock::new();
    ANIMATIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_animations<T>(body: impl FnOnce(&mut BTreeMap<String, Animation>) -> T) -> T {
    let mut guard = animations().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn controllers() -> &'static Mutex<AnimationController> {
    static CONTROLLER: OnceLock<Mutex<AnimationController>> = OnceLock::new();
    CONTROLLER.get_or_init(|| Mutex::new(AnimationController::new()))
}

fn with_controller<T>(body: impl FnOnce(&mut AnimationController) -> T) -> T {
    let mut guard = controllers().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn renderers() -> &'static Mutex<BTreeMap<String, String>> {
    static RENDERERS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    RENDERERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Registers a model.
pub fn register(model: Model) -> bool {
    with(|models| models.insert(model.id.clone(), model).is_none())
}

/// Forgets a model.
pub fn unregister(id: &str) -> bool {
    with(|models| models.remove(id).is_some())
}

/// A model by id.
#[must_use]
pub fn get(id: &str) -> Option<Model> {
    with(|models| models.get(id).cloned())
}

/// The same, under the name the wishlist also uses.
#[must_use]
pub fn get_by_id(id: &str) -> Option<Model> {
    get(id)
}

/// Every model a mod registered.
#[must_use]
pub fn get_all() -> Vec<Model> {
    with(|models| models.values().cloned().collect())
}

/// Starts a model, and answers it, for a mod that wants to build one up.
#[must_use]
pub fn create(id: &str) -> Model {
    Model::create(id)
}

/// Builds a part from vertices and indices.
#[must_use]
pub fn create_mesh(
    name: &str,
    bone: &str,
    vertices: &[f32],
    indices: &[u32],
) -> ModelPart {
    ModelPart::new(name, bone).vertices(vertices).indices(indices)
}

/// Builds a bone.
#[must_use]
pub fn create_bone(name: &str) -> Bone {
    Bone::new(name)
}

/// A model's bones.
#[must_use]
pub fn get_bones(id: &str) -> Vec<Bone> {
    get(id).map(|model| model.bones).unwrap_or_default()
}

/// A model's parts.
#[must_use]
pub fn get_parts(id: &str) -> Vec<ModelPart> {
    get(id).map(|model| model.parts).unwrap_or_default()
}

/// A model's vertices, as `(part, vertices)`.
#[must_use]
pub fn get_vertices(id: &str) -> Vec<(String, Vec<f32>)> {
    get(id)
        .map(|model| {
            model
                .parts
                .into_iter()
                .map(|part| (part.name, part.vertices))
                .collect()
        })
        .unwrap_or_default()
}

/// A model's indices, as `(part, indices)`.
#[must_use]
pub fn get_indices(id: &str) -> Vec<(String, Vec<u32>)> {
    get(id)
        .map(|model| {
            model
                .parts
                .into_iter()
                .map(|part| (part.name, part.indices))
                .collect()
        })
        .unwrap_or_default()
}

/// The materials a model's parts use.
#[must_use]
pub fn get_materials(id: &str) -> Vec<String> {
    let mut materials: Vec<String> = get(id)
        .map(|model| model.parts.into_iter().map(|part| part.material).collect())
        .unwrap_or_default();
    materials.sort();
    materials.dedup();
    materials
}

/// Sets a part's material.
pub fn set_material(id: &str, part: &str, material: &str) -> bool {
    with(|models| match models.get_mut(id) {
        Some(model) => match model.parts.iter_mut().find(|held| held.name == part) {
            Some(held) => {
                held.material = material.to_owned();
                true
            }
            None => false,
        },
        None => false,
    })
}

/// Sets a bone's transform.
pub fn set_transform(id: &str, bone: &str, transform: BoneTransform) -> bool {
    with(|models| match models.get_mut(id) {
        Some(model) => match model.bones.iter_mut().find(|held| held.name == bone) {
            Some(held) => {
                held.transform = transform;
                true
            }
            None => false,
        },
        None => false,
    })
}

/// Shows or hides a bone or a part, by name.
pub fn set_visibility(id: &str, name: &str, visible: bool) -> bool {
    with(|models| match models.get_mut(id) {
        Some(model) => {
            let mut found = false;
            if let Some(bone) = model.bones.iter_mut().find(|held| held.name == name) {
                bone.visible = visible;
                found = true;
            }
            if let Some(part) = model.parts.iter_mut().find(|held| held.name == name) {
                part.visible = visible;
                found = true;
            }
            found
        }
        None => false,
    })
}

/// Sets a part's texture.
pub fn set_texture(id: &str, part: &str, texture: &str) -> bool {
    with(|models| match models.get_mut(id) {
        Some(model) => match model.parts.iter_mut().find(|held| held.name == part) {
            Some(held) => {
                held.texture = texture.to_owned();
                true
            }
            None => false,
        },
        None => false,
    })
}

/// A model's bounds, worked out from its vertices.
#[must_use]
pub fn get_bounds(id: &str) -> Option<Bounds> {
    get(id).map(|model| model.bounds())
}

/// The animation controller, for a mod that wants to hold it.
#[must_use]
pub fn get_animation_controller() -> AnimationController {
    with_controller(|controller| controller.clone())
}

/// Puts a controller back, which a mod that drove its own would do.
pub fn set_animation_controller(controller: AnimationController) {
    with_controller(|held| *held = controller);
}

/// Exports a model as text.
#[must_use]
pub fn export(id: &str) -> Option<String> {
    get(id).map(|model| model.export())
}

/// Reads a model back from what [`export`] wrote.
pub fn import(text: &str) -> Option<Model> {
    let model = Model::import(text)?;
    register(model.clone());
    Some(model)
}

/// Records that a mod has a loader for its own model format. This host calls its own formats;
/// nothing here calls a mod's loader for the client's models.
pub fn register_loader(name: &str) -> bool {
    with_loaders(|loaders| {
        if loaders.iter().any(|held| held == name) {
            false
        } else {
            loaders.push(name.to_owned());
            true
        }
    })
}

/// The loaders a mod registered.
#[must_use]
pub fn get_loaders() -> Vec<String> {
    with_loaders(|loaders| loaders.clone())
}

fn loaders() -> &'static Mutex<Vec<String>> {
    static LOADERS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    LOADERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_loaders<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = loaders().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Records that a mod has a renderer for an entity, which the client would call. Nothing here
/// calls it.
pub fn register_entity_renderer(kind: &str) -> bool {
    with(|_models| {
        let mut renderers = renderers().lock().unwrap_or_else(|error| error.into_inner());
        if renderers.contains_key(kind) {
            false
        } else {
            renderers.insert(kind.to_owned(), "entity".to_owned());
            true
        }
    })
}

/// The same for a block.
pub fn register_block_renderer(kind: &str) -> bool {
    with(|_models| {
        let mut renderers = renderers().lock().unwrap_or_else(|error| error.into_inner());
        if renderers.contains_key(kind) {
            false
        } else {
            renderers.insert(kind.to_owned(), "block".to_owned());
            true
        }
    })
}

/// The same for an item.
pub fn register_item_renderer(kind: &str) -> bool {
    with(|_models| {
        let mut renderers = renderers().lock().unwrap_or_else(|error| error.into_inner());
        if renderers.contains_key(kind) {
            false
        } else {
            renderers.insert(kind.to_owned(), "item".to_owned());
            true
        }
    })
}

/// The renderers a mod registered, as `(kind, what it draws)`.
#[must_use]
pub fn get_renderers() -> Vec<(String, String)> {
    renderers()
        .lock()
        .map(|renderers| {
            renderers
                .iter()
                .map(|(kind, what)| (kind.clone(), what.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Loads a model into the client. `false`.
pub fn load(_id: &str) -> bool {
    refused("load")
}

/// Reloads them. `false`.
pub fn reload() -> bool {
    refused("load")
}

/// Registers an animation.
pub fn register_animation(animation: Animation) -> bool {
    with_animations(|animations| animations.insert(animation.id.clone(), animation).is_none())
}

/// Forgets an animation.
pub fn unregister_animation(id: &str) -> bool {
    with_animations(|animations| animations.remove(id).is_some())
}

/// An animation by id.
#[must_use]
pub fn get_animation(id: &str) -> Option<Animation> {
    with_animations(|animations| animations.get(id).cloned())
}

/// Every animation.
#[must_use]
pub fn get_animations() -> Vec<Animation> {
    with_animations(|animations| animations.values().cloned().collect())
}

/// Starts an animation on the controller.
pub fn play(animation: &str) -> bool {
    with_controller(|controller| controller.play(animation))
}

/// Pauses it.
pub fn pause(animation: &str) -> bool {
    with_controller(|controller| controller.pause(animation))
}

/// Starts it again.
pub fn resume(animation: &str) -> bool {
    with_controller(|controller| controller.resume(animation))
}

/// Stops it.
pub fn stop(animation: &str) -> bool {
    with_controller(|controller| controller.stop(animation))
}

/// Stops every one.
pub fn stop_all() -> usize {
    with_controller(|controller| controller.stop_all())
}

/// A playback.
#[must_use]
pub fn get_playback(animation: &str) -> Option<Playback> {
    with_controller(|controller| controller.get(animation))
}

/// The state of every playback.
#[must_use]
pub fn get_state() -> Vec<Playback> {
    with_controller(|controller| controller.all())
}

/// How far into an animation a playback is.
#[must_use]
pub fn get_time(animation: &str) -> Option<u64> {
    get_playback(animation).map(|playback| playback.time)
}

/// Sets how far into it a playback is.
pub fn set_time(animation: &str, time: u64) -> bool {
    with_controller(|controller| controller.set_time(animation, time))
}

/// Sets how fast it runs.
pub fn set_speed(animation: &str, speed: f32) -> bool {
    with_controller(|controller| controller.set_speed(animation, speed))
}

/// Sets whether it loops.
pub fn set_looping(animation: &str, looping: bool) -> bool {
    with_controller(|controller| controller.set_looping(animation, looping))
}

/// Sets its blend weight.
pub fn set_blend_weight(animation: &str, weight: f32) -> bool {
    with_controller(|controller| controller.set_blend_weight(animation, weight))
}

/// Every playing animation, normalised to weights that add up to one.
#[must_use]
pub fn blend() -> Vec<(String, f32)> {
    with_controller(|controller| controller.blend())
}

/// Fades from one animation to another over a time.
pub fn crossfade(from: &str, to: &str, duration_millis: u64) -> bool {
    with_controller(|controller| controller.crossfade(from, to, duration_millis))
}

/// Sets a parameter an animation is driven by.
pub fn set_parameter(name: &str, value: f32) {
    with_controller(|controller| controller.set_parameter(name, value));
}

/// A parameter.
#[must_use]
pub fn get_parameter(name: &str) -> Option<f32> {
    with_controller(|controller| controller.get_parameter(name))
}

/// Adds an animation to the controller, which for this host is the same as playing it.
pub fn add_controller(animation: &str) -> bool {
    play(animation)
}

/// Takes it out, which is stopping it.
pub fn remove_controller(animation: &str) -> bool {
    stop(animation)
}

/// Moves every playback on by a slice of time, using each animation's own length.
pub fn advance(delta_millis: u64) {
    with_controller(|controller| {
        controller.advance(delta_millis, |animation| {
            get_animation(animation)
                .map(|animation| animation.length())
                .unwrap_or(0)
        });
    });
}

/// Records that a mod has a keyframe handler, which the client would call. Nothing here calls
/// it.
pub fn register_keyframe_handler(name: &str) -> bool {
    with_handlers(|handlers| {
        if handlers.contains(&name.to_owned()) {
            false
        } else {
            handlers.push(name.to_owned());
            true
        }
    })
}

/// Records an animation event a mod wants. Nothing here fires it.
pub fn register_animation_event(name: &str) -> bool {
    register_keyframe_handler(name)
}

fn handlers() -> &'static Mutex<Vec<String>> {
    static HANDLERS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    HANDLERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_handlers<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = handlers().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The handlers a mod registered.
#[must_use]
pub fn get_handlers() -> Vec<String> {
    with_handlers(|handlers| handlers.clone())
}

/// A model's skeleton: its bones, with each one's transform at a moment, sampled from an
/// animation.
#[must_use]
pub fn get_skeleton(model: &str, animation: &str, time: u64) -> Vec<(String, BoneTransform)> {
    let Some(model) = get(model) else {
        return Vec::new();
    };
    let Some(animation) = get_animation(animation) else {
        return Vec::new();
    };
    model
        .bones
        .into_iter()
        .map(|bone| {
            let transform = animation
                .sample(&bone.name, time)
                .unwrap_or(bone.transform);
            (bone.name, transform)
        })
        .collect()
}

/// One bone's transform at a moment, sampled from an animation.
#[must_use]
pub fn get_bone_transform(model: &str, animation: &str, bone: &str, time: u64) -> Option<BoneTransform> {
    get_skeleton(model, animation, time)
        .into_iter()
        .find(|(name, _)| name == bone)
        .map(|(_, transform)| transform)
}

/// Sets a bone's transform in a model, which is `set_transform` under the name the animation
/// side uses.
pub fn set_bone_transform(model: &str, bone: &str, transform: BoneTransform) -> bool {
    set_transform(model, bone, transform)
}

/// Why a model question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "load" => {
            "a model reaches the screen through the client's own model and renderer; this host \
             owns the geometry and the animation it samples, not the drawing"
                .to_owned()
        }
        "renderer" => {
            "an entity, block or item renderer is called by the client as it draws; a \
             registration here is recorded and never called"
                .to_owned()
        }
        "handler" => {
            "a keyframe handler or animation event would be called by the client's animation \
             player; nothing here calls one"
                .to_owned()
        }
        "loader" => "the client's own model loaders are inside it, in its own binary".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["load", "renderer", "handler", "loader"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about a model: {}", why_unknown(what));
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|models| models.clear());
        with_animations(|animations| animations.clear());
        set_animation_controller(AnimationController::new());
        with_loaders(|loaders| loaders.clear());
        with_handlers(|handlers| handlers.clear());
        renderers().lock().unwrap().clear();
    }

    fn cube() -> Model {
        Model::new("hello:cube")
            .bone(Bone::new("body").at(0.0, 1.0, 0.0))
            .part(
                ModelPart::new("shell", "body")
                    .vertices(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0])
                    .indices(&[0, 1, 2])
                    .material("minecraft:entity")
                    .texture("hello:skin"),
            )
    }

    #[test]
    fn a_model_is_bones_parts_and_geometry() {
        let _serial = serial();
        fresh();
        assert!(register(cube()));
        let model = get("hello:cube").expect("registered");
        assert_eq!(model.bones.len(), 1);
        assert_eq!(model.parts.len(), 1);
        assert_eq!(get_bones("hello:cube").len(), 1);
        assert_eq!(get_parts("hello:cube").len(), 1);
        assert_eq!(get_vertices("hello:cube").len(), 1);
        assert_eq!(get_indices("hello:cube").len(), 1);
        assert_eq!(get_materials("hello:cube"), vec!["minecraft:entity".to_owned()]);
        assert_eq!(get_bounds("hello:cube").map(|bounds| bounds.1), Some((1.0, 1.0, 1.0)));
        assert_eq!(get_bounds("hello:cube").map(|bounds| bounds.0), Some((0.0, 0.0, 0.0)));
        let part = model.part_of("shell").expect("it is there");
        assert_eq!(part.vertex_count(), 3);
        assert_eq!(part.triangle_count(), 1);
        assert!(model.to_string().contains("1 triangle"), "{model}");
        assert_eq!(get("nothing"), None);
        assert!(!register(cube()), "it replaces");
        assert!(unregister("hello:cube"));
        assert!(!unregister("hello:cube"));

        // A part only takes vertices in threes.
        let part = ModelPart::new("p", "b").vertices(&[1.0, 2.0]);
        assert_eq!(part.vertex_count(), 0);
        let built = create_mesh("mesh", "body", &[0.0, 0.0, 0.0], &[0]);
        assert_eq!(built.bone, "body");
        assert_eq!(create("hello:new").id, "hello:new");
        assert_eq!(create_bone("head").name, "head");
        assert_eq!(get_all().len(), 0);
    }

    #[test]
    fn material_transform_visibility_and_texture_are_the_mods_to_set() {
        let _serial = serial();
        fresh();
        register(cube());
        assert!(set_material("hello:cube", "shell", "minecraft:glow"));
        assert!(set_texture("hello:cube", "shell", "hello:other_skin"));
        assert!(set_transform("hello:cube", "body", BoneTransform::at(1.0, 2.0, 3.0)));
        assert!(set_visibility("hello:cube", "shell", false));
        assert!(set_visibility("hello:cube", "body", false));
        let model = get("hello:cube").expect("registered");
        assert_eq!(model.part_of("shell").map(|p| p.material.clone()), Some("minecraft:glow".to_owned()));
        assert_eq!(model.part_of("shell").map(|p| p.texture.clone()), Some("hello:other_skin".to_owned()));
        assert_eq!(model.bone_of("body").map(|b| b.transform.position), Some((1.0, 2.0, 3.0)));
        assert!(!model.part_of("shell").expect("there").visible);
        assert!(!model.bone_of("body").expect("there").visible);
        assert!(!set_material("hello:cube", "nothing", "m"));
        assert!(!set_texture("hello:cube", "nothing", "t"));
        assert!(!set_transform("hello:cube", "nothing", BoneTransform::identity()));
        assert!(!set_visibility("hello:cube", "nothing", true));
        assert!(!set_material("nothing", "shell", "m"));
    }

    #[test]
    fn a_bone_chain_walks_up_to_the_root() {
        let _serial = serial();
        fresh();
        register(
            Model::new("hello:rig")
                .bone(Bone::new("root"))
                .bone(Bone::new("spine").parent("root"))
                .bone(Bone::new("head").parent("spine")),
        );
        let model = get("hello:rig").expect("registered");
        assert_eq!(
            model.chain_of("head"),
            vec!["head".to_owned(), "spine".to_owned(), "root".to_owned()]
        );
        assert_eq!(model.chain_of("root"), vec!["root".to_owned()]);
        // A cycle stops rather than looping forever.
        register(
            Model::new("hello:cycle")
                .bone(Bone::new("a").parent("b"))
                .bone(Bone::new("b").parent("a")),
        );
        assert_eq!(
            get("hello:cycle").expect("registered").chain_of("a").len(),
            2
        );
    }

    #[test]
    fn a_model_exports_and_imports_as_text() {
        let _serial = serial();
        fresh();
        register(cube());
        let text = export("hello:cube").expect("it exports");
        assert!(text.contains("model hello:cube"), "{text}");
        assert!(text.contains("bone body"), "{text}");
        assert!(text.contains("part shell bone=body"), "{text}");
        let imported = import(&text).expect("it imports");
        assert_eq!(imported.id, "hello:cube");
        assert_eq!(imported.bones.len(), 1);
        assert_eq!(imported.parts.len(), 1);
        assert_eq!(
            imported.part_of("shell").map(|part| part.vertex_count()),
            Some(3),
            "the counts round trip; the vertex values do not"
        );
        assert!(get("hello:cube").is_some(), "importing registers it");
        assert!(import("not a model").is_none());
        assert!(import("").is_none());
        assert!(export("nothing").is_none());
    }

    #[test]
    fn a_transform_applies_scale_then_rotation_then_translation() {
        let _serial = serial();
        assert_eq!(BoneTransform::identity().apply((1.0, 2.0, 3.0)), (1.0, 2.0, 3.0));
        let at = BoneTransform::at(10.0, 0.0, 0.0);
        assert_eq!(at.apply((1.0, 0.0, 0.0)), (11.0, 0.0, 0.0));
        // A quarter turn takes x to z, which way round depends on the handedness.
        let turned = BoneTransform::rotated(90.0, 0.0, 0.0);
        let point = turned.apply((1.0, 0.0, 0.0));
        assert!(point.1.abs() < 1e-6, "{point:?}");
        assert!(point.2.abs() > 0.9, "a quarter turn moves it onto z: {point:?}");
        let scaled = BoneTransform { scale: (2.0, 2.0, 2.0), ..BoneTransform::identity() };
        assert_eq!(scaled.apply((1.0, 1.0, 1.0)), (2.0, 2.0, 2.0));
        // A blend halfway between two transforms is halfway between their results.
        let half = BoneTransform::identity().blend(&at, 0.5);
        assert_eq!(half.apply((0.0, 0.0, 0.0)), (5.0, 0.0, 0.0));
        assert!(half.to_string().contains("at (5.00"));
    }

    #[test]
    fn an_animation_samples_between_its_keyframes() {
        let _serial = serial();
        fresh();
        let animation = Animation::new("hello:walk")
            .frame(Keyframe::new("leg", 0, BoneTransform::identity()))
            .frame(Keyframe::new("leg", 1000, BoneTransform::at(0.0, 0.0, 10.0)))
            .frame(Keyframe::new("arm", 0, BoneTransform::at(1.0, 0.0, 0.0)));
        assert_eq!(animation.length(), 1000);
        assert_eq!(animation.bones(), vec!["arm".to_owned(), "leg".to_owned()]);
        assert_eq!(
            animation.sample("leg", 0).map(|t| t.position),
            Some((0.0, 0.0, 0.0))
        );
        assert_eq!(
            animation.sample("leg", 999).map(|t| t.position),
            Some((0.0, 0.0, 9.99))
        );
        assert_eq!(
            animation.sample("leg", 1000).map(|t| t.position),
            Some((0.0, 0.0, 0.0)),
            "at exactly its length a looping animation is back at the start"
        );
        assert_eq!(
            animation.sample("leg", 500).map(|t| t.position),
            Some((0.0, 0.0, 5.0)),
            "halfway between the keyframes"
        );
        assert_eq!(
            animation.sample("leg", 5000).map(|t| t.position),
            Some((0.0, 0.0, 0.0)),
            "it loops from the start"
        );
        assert_eq!(animation.sample("nothing", 0), None);
        assert!(animation.to_string().contains("1000 ms"));

        // The loop modes land where they should.
        let mut ping_pong = animation.clone();
        ping_pong.loop_mode = LoopMode::PingPong;
        assert_eq!(ping_pong.time_at(1500), 500, "it comes back");
        let mut once = animation.clone();
        once.loop_mode = LoopMode::Once;
        assert_eq!(once.time_at(5000), 1000, "it holds the end");
        let mut hold = Animation::new("hello:empty");
        hold.loop_mode = LoopMode::Loop;
        assert_eq!(hold.time_at(500), 0, "no keyframes means no time");
        assert!(register_animation(animation.clone()));
        assert!(!register_animation(animation.clone()));
        assert_eq!(get_animations().len(), 1);
        assert_eq!(
            get_animation("hello:walk").map(|animation| animation.length()),
            Some(1000)
        );
        assert!(unregister_animation("hello:walk"));
    }

    #[test]
    fn a_playback_runs_at_its_speed_and_a_crossfade_blends_two() {
        let _serial = serial();
        fresh();
        let walk = Animation::new("hello:walk")
            .frame(Keyframe::new("leg", 0, BoneTransform::identity()))
            .frame(Keyframe::new("leg", 1000, BoneTransform::at(0.0, 0.0, 1.0)));
        let run = Animation::new("hello:run")
            .frame(Keyframe::new("leg", 0, BoneTransform::identity()))
            .frame(Keyframe::new("leg", 500, BoneTransform::at(0.0, 0.0, 1.0)));
        register_animation(walk);
        register_animation(run);

        assert!(play("hello:walk"));
        assert_eq!(get_time("hello:walk"), Some(0));
        advance(250);
        assert_eq!(get_time("hello:walk"), Some(250));
        assert!(set_speed("hello:walk", 2.0));
        advance(250);
        assert_eq!(get_time("hello:walk"), Some(750), "at double speed");
        assert!(pause("hello:walk"));
        advance(250);
        assert_eq!(get_time("hello:walk"), Some(750), "a paused one does not move");
        assert!(resume("hello:walk"));
        assert!(set_speed("hello:walk", 1.0));
        advance(500);
        assert_eq!(get_time("hello:walk"), Some(250), "750 + 500 wraps at the end");
        assert!(set_looping("hello:walk", false));
        advance(5000);
        assert_eq!(get_time("hello:walk"), Some(1000), "with looping off it holds");
        assert!(get_playback("hello:walk").is_some_and(|playback| playback.finished));
        assert!(!set_speed("nothing", 1.0));
        assert!(!pause("nothing"));
        assert!(!resume("nothing"));
        assert!(!set_looping("nothing", true));

        // A crossfade moves the weights from one to the other.
        assert!(crossfade("hello:walk", "hello:run", 1000));
        assert!(get_playback("hello:run").is_some());
        advance(500);
        let weights = blend();
        assert_eq!(weights.len(), 2);
        let walk_weight = weights
            .iter()
            .find(|(name, _)| name == "hello:walk")
            .map(|(_, weight)| *weight)
            .unwrap_or_default();
        assert!(walk_weight > 0.3 && walk_weight < 0.7, "{weights:?}");
        advance(600);
        let weights = blend();
        let run_weight = weights
            .iter()
            .find(|(name, _)| name == "hello:run")
            .map(|(_, weight)| *weight)
            .unwrap_or_default();
        assert!((run_weight - 1.0).abs() < 1e-6, "the fade finished: {weights:?}");

        assert!(set_blend_weight("hello:run", 0.25));
        set_parameter("speed", 0.5);
        assert_eq!(get_parameter("speed"), Some(0.5));
        assert_eq!(get_state().len(), 2);
        assert!(stop("hello:run"));
        assert!(!stop("hello:run"));
        assert_eq!(stop_all(), 1);
        assert!(get_state().is_empty());
        assert!(!add_controller(""));
        assert!(add_controller("hello:walk"));
        assert!(remove_controller("hello:walk"));
    }

    #[test]
    fn a_skeleton_samples_every_bone_from_an_animation() {
        let _serial = serial();
        fresh();
        register(
            Model::new("hello:rig")
                .bone(Bone::new("root"))
                .bone(Bone::new("leg")),
        );
        register_animation(
            Animation::new("hello:walk")
                .frame(Keyframe::new("leg", 0, BoneTransform::at(0.0, 0.0, 0.0)))
                .frame(Keyframe::new("leg", 100, BoneTransform::at(0.0, 0.0, 2.0))),
        );
        let skeleton = get_skeleton("hello:rig", "hello:walk", 50);
        assert_eq!(skeleton.len(), 2);
        let leg = skeleton
            .iter()
            .find(|(name, _)| name == "leg")
            .map(|(_, transform)| *transform)
            .expect("it is there");
        assert_eq!(leg.position, (0.0, 0.0, 1.0), "sampled halfway");
        assert_eq!(
            get_bone_transform("hello:rig", "hello:walk", "root", 50).map(|t| t.position),
            Some((0.0, 0.0, 0.0)),
            "a bone the animation does not drive keeps its own transform"
        );
        assert!(get_bone_transform("hello:rig", "hello:walk", "nothing", 0).is_none());
        assert!(get_skeleton("nothing", "hello:walk", 0).is_empty());
        assert!(get_skeleton("hello:rig", "nothing", 0).is_empty());
        assert!(set_bone_transform("hello:rig", "leg", BoneTransform::at(1.0, 1.0, 1.0)));
    }

    #[test]
    fn the_clients_renderers_and_loaders_are_recorded_but_never_called() {
        let _serial = serial();
        fresh();
        assert!(!load("hello:cube"));
        assert!(!reload());
        assert!(register_loader("hello:own_format"));
        assert!(!register_loader("hello:own_format"));
        assert_eq!(get_loaders().len(), 1);
        assert!(register_entity_renderer("minecraft:pig"));
        assert!(!register_entity_renderer("minecraft:pig"));
        assert!(register_block_renderer("minecraft:stone"));
        assert!(register_item_renderer("minecraft:stick"));
        assert_eq!(get_renderers().len(), 3);
        assert!(register_keyframe_handler("hello:on_step"));
        assert!(!register_animation_event("hello:on_step"));
        assert_eq!(get_handlers().len(), 1);
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("renderer").contains("never called"));
    }
}
