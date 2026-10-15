//! Textures and sprites: the pixels a mod owns, and the upload it cannot do.
//!
//! A texture here is real data: its id, its dimensions, its format, and its pixels, which a
//! mod can load from a file, from bytes or from a resource, read, write, and update in a
//! region. An atlas is real too, and so is the arithmetic that goes with it — adding a sprite
//! to an atlas stitches it and gives the sprite coordinates to draw with, and `get_uv` turns
//! those into UVs against the atlas size, which is exactly what a mod needs to draw from one.
//! Animations over a texture (frames, with a time per frame) are real data as well, and the
//! filter, wrap and anisotropy settings a caller would set are recorded.
//!
//! What cannot be done is anything that touches the client's GPU: `load` into its pipeline,
//! `bind`/`unbind`, `generate_mipmap`, and `delete` from the device. Those are refused with
//! their reasons, and section 31's ledger is where a mod's *declared* texture actually lands.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! textures::load_from_bytes("hello:square", &[255u8; 4 * 4 * 4], 4, 4, "rgba8");
//! assert_eq!(textures::get_dimensions("hello:square"), Some((4, 4)));
//! assert_eq!(textures::get_pixels("hello:square").map(|pixels| pixels.len()), Some(64));
//! assert_eq!(textures::get_memory_usage(), 64);
//! assert!(!textures::load("hello:square"), "the client uploads textures, not this host");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// How pixels are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextureFormat {
    #[default]
    Rgba8,
    Rgb8,
    Bgra8,
    R8,
    Rgba16Float,
}

impl TextureFormat {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rgba8 => "rgba8",
            Self::Rgb8 => "rgb8",
            Self::Bgra8 => "bgra8",
            Self::R8 => "r8",
            Self::Rgba16Float => "rgba16float",
        }
    }

    /// How many bytes one pixel takes.
    #[must_use]
    pub fn bytes_per_pixel(self) -> u64 {
        match self {
            Self::Rgba8 | Self::Bgra8 => 4,
            Self::Rgb8 => 3,
            Self::R8 => 1,
            Self::Rgba16Float => 8,
        }
    }

    /// Parses the name a caller writes.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "rgb8" => Self::Rgb8,
            "bgra8" => Self::Bgra8,
            "r8" => Self::R8,
            "rgba16float" => Self::Rgba16Float,
            _ => Self::Rgba8,
        }
    }
}

impl fmt::Display for TextureFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a texture is scaled when it is drawn bigger or smaller than it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Filter {
    #[default]
    Nearest,
    Linear,
}

/// What happens outside the texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WrapMode {
    #[default]
    Clamp,
    Repeat,
    Mirror,
}

/// One frame of a texture animation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationFrame {
    pub index: u32,
    /// How long it stays up, in milliseconds.
    pub duration: u64,
}

/// A texture a mod owns.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub id: String,
    pub owner: String,
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    pub pixels: Vec<u8>,
    pub filter: Filter,
    pub wrap: WrapMode,
    pub anisotropy: u32,
    pub mipmaps: bool,
    /// The animation over it, when it has one.
    pub frames: Vec<AnimationFrame>,
    /// Which frame is up, in the mod's own bookkeeping.
    pub frame: u32,
}

impl Texture {
    /// An empty texture of a size, filled with zeroes.
    #[must_use]
    pub fn new(id: &str, width: u32, height: u32, format: TextureFormat) -> Self {
        let bytes = u64::from(width) * u64::from(height) * format.bytes_per_pixel();
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            width,
            height,
            format,
            pixels: vec![0u8; bytes as usize],
            filter: Filter::Nearest,
            wrap: WrapMode::Clamp,
            anisotropy: 1,
            mipmaps: false,
            frames: Vec::new(),
            frame: 0,
        }
    }

    /// The same, with pixels.
    #[must_use]
    pub fn with_pixels(mut self, pixels: Vec<u8>) -> Self {
        let expected = (u64::from(self.width) * u64::from(self.height) * self.format.bytes_per_pixel())
            as usize;
        if pixels.len() == expected {
            self.pixels = pixels;
        }
        self
    }

    #[must_use]
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// How many bytes it takes.
    #[must_use]
    pub fn memory(&self) -> u64 {
        self.pixels.len() as u64
    }

    /// One pixel, as `(r, g, b, a)`, from an 8-bit format.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let per_pixel = self.format.bytes_per_pixel() as usize;
        let start = (y as usize * self.width as usize + x as usize) * per_pixel;
        let slice = self.pixels.get(start..start + per_pixel)?;
        Some(match self.format {
            TextureFormat::Rgba8 => [slice[0], slice[1], slice[2], slice[3]],
            TextureFormat::Bgra8 => [slice[2], slice[1], slice[0], slice[3]],
            TextureFormat::Rgb8 => [slice[0], slice[1], slice[2], 255],
            TextureFormat::R8 => [slice[0], slice[0], slice[0], 255],
            // A float format is not read as bytes here; the mod has the pixels either way.
            TextureFormat::Rgba16Float => [slice[0], slice[1], 255, 255],
        })
    }

    /// Writes one pixel, and answers whether it was inside the texture.
    pub fn set_pixel(&mut self, x: u32, y: u32, colour: [u8; 4]) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let per_pixel = self.format.bytes_per_pixel() as usize;
        let start = (y as usize * self.width as usize + x as usize) * per_pixel;
        let Some(slice) = self.pixels.get_mut(start..start + per_pixel) else {
            return false;
        };
        match self.format {
            TextureFormat::Rgba8 | TextureFormat::Rgba16Float => {
                slice[0] = colour[0];
                slice[1] = colour[1];
                slice[2] = colour[2];
                slice[3] = colour[3];
            }
            TextureFormat::Bgra8 => {
                slice[0] = colour[2];
                slice[1] = colour[1];
                slice[2] = colour[0];
                slice[3] = colour[3];
            }
            TextureFormat::Rgb8 => {
                slice[0] = colour[0];
                slice[1] = colour[1];
                slice[2] = colour[2];
            }
            TextureFormat::R8 => slice[0] = colour[0],
        }
        true
    }

    /// Copies a rectangular region into the texture, and answers how many pixels went.
    pub fn update_region(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> usize {
        let per_pixel = self.format.bytes_per_pixel() as usize;
        let expected = (width as usize) * (height as usize) * per_pixel;
        if pixels.len() < expected {
            return 0;
        }
        let mut written = 0;
        for row in 0..height {
            for column in 0..width {
                let start = ((row * width + column) as usize) * per_pixel;
                let Some(colour) = pixels.get(start..start + per_pixel) else {
                    continue;
                };
                let colour = [colour[0], colour.get(1).copied().unwrap_or(0), colour.get(2).copied().unwrap_or(0), colour.get(3).copied().unwrap_or(255)];
                if self.set_pixel(x + column, y + row, colour) {
                    written += 1;
                }
            }
        }
        written
    }

    /// Which frame is up at a moment, following the animation.
    #[must_use]
    pub fn frame_at(&self, elapsed_millis: u64) -> u32 {
        if self.frames.is_empty() {
            return 0;
        }
        let total: u64 = self.frames.iter().map(|frame| frame.duration).sum();
        if total == 0 {
            return 0;
        }
        let mut remaining = elapsed_millis % total;
        for frame in &self.frames {
            if remaining < frame.duration {
                return frame.index;
            }
            remaining -= frame.duration;
        }
        self.frames[0].index
    }
}

impl fmt::Display for Texture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}x{}, {}, {} byte(s))",
            self.id,
            self.width,
            self.height,
            self.format,
            self.pixels.len()
        )
    }
}

/// A region of an atlas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sprite {
    pub id: String,
    pub atlas: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Sprite {
    /// Where it sits in the atlas, as `(u0, v0, u1, v1)`, which is what a caller draws with.
    #[must_use]
    pub fn uv(&self, atlas_size: (u32, u32)) -> (f32, f32, f32, f32) {
        let (width, height) = (atlas_size.0.max(1) as f32, atlas_size.1.max(1) as f32);
        (
            self.x as f32 / width,
            self.y as f32 / height,
            (self.x + self.width) as f32 / width,
            (self.y + self.height) as f32 / height,
        )
    }
}

/// An atlas a mod builds.
#[derive(Debug, Clone, PartialEq)]
pub struct Atlas {
    pub id: String,
    pub texture: Texture,
    pub sprites: Vec<Sprite>,
    /// Where the next sprite goes: a simple shelf packer, which is what a mod needs to stitch
    /// a set of sprites without wasting much.
    shelf_x: u32,
    shelf_y: u32,
    shelf_height: u32,
}

impl Atlas {
    /// An atlas laid over a texture.
    #[must_use]
    pub fn new(id: &str, texture: Texture) -> Self {
        Self {
            id: id.to_owned(),
            texture,
            sprites: Vec::new(),
            shelf_x: 0,
            shelf_y: 0,
            shelf_height: 0,
        }
    }

    #[must_use]
    pub fn dimensions(&self) -> (u32, u32) {
        self.texture.dimensions()
    }

    /// Adds a sprite to the atlas, and answers where it landed.
    pub fn add(&mut self, id: &str, width: u32, height: u32) -> Option<&Sprite> {
        let (atlas_width, atlas_height) = self.dimensions();
        if width > atlas_width || height > atlas_height {
            return None;
        }
        if self.shelf_x + width > atlas_width {
            self.shelf_x = 0;
            self.shelf_y += self.shelf_height;
            self.shelf_height = 0;
        }
        if self.shelf_y + height > atlas_height {
            return None;
        }
        let sprite = Sprite {
            id: id.to_owned(),
            atlas: self.id.clone(),
            x: self.shelf_x,
            y: self.shelf_y,
            width,
            height,
        };
        self.shelf_x += width;
        self.shelf_height = self.shelf_height.max(height);
        self.sprites.push(sprite);
        self.sprites.last()
    }
}

fn textures() -> &'static Mutex<BTreeMap<String, Texture>> {
    static TEXTURES: OnceLock<Mutex<BTreeMap<String, Texture>>> = OnceLock::new();
    TEXTURES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, Texture>) -> T) -> T {
    let mut guard = textures().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn atlases() -> &'static Mutex<BTreeMap<String, Atlas>> {
    static ATLASES: OnceLock<Mutex<BTreeMap<String, Atlas>>> = OnceLock::new();
    ATLASES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_atlases<T>(body: impl FnOnce(&mut BTreeMap<String, Atlas>) -> T) -> T {
    let mut guard = atlases().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a texture, or replaces the one with that id.
pub fn register(texture: Texture) -> bool {
    with(|textures| textures.insert(texture.id.clone(), texture).is_none())
}

/// Forgets a texture.
pub fn unregister(id: &str) -> bool {
    with(|textures| textures.remove(id).is_some())
}

/// A texture by id.
#[must_use]
pub fn get(id: &str) -> Option<Texture> {
    with(|textures| textures.get(id).cloned())
}

/// The id a texture was registered under.
#[must_use]
pub fn get_id(id: &str) -> Option<String> {
    get(id).map(|texture| texture.id)
}

/// A texture's size.
#[must_use]
pub fn get_dimensions(id: &str) -> Option<(u32, u32)> {
    get(id).map(|texture| texture.dimensions())
}

/// Its format.
#[must_use]
pub fn get_format(id: &str) -> Option<TextureFormat> {
    get(id).map(|texture| texture.format)
}

/// Its pixels.
#[must_use]
pub fn get_pixels(id: &str) -> Option<Vec<u8>> {
    get(id).map(|texture| texture.pixels)
}

/// Replaces its pixels, which only takes when the size matches.
pub fn set_pixels(id: &str, pixels: &[u8]) -> bool {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => {
            let expected =
                u64::from(texture.width) * u64::from(texture.height) * texture.format.bytes_per_pixel();
            if pixels.len() as u64 != expected {
                return false;
            }
            texture.pixels = pixels.to_vec();
            true
        }
        None => false,
    })
}

/// Copies a region in.
pub fn update_region(
    id: &str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> usize {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => texture.update_region(x, y, width, height, pixels),
        None => 0,
    })
}

/// Reads one pixel.
#[must_use]
pub fn get_pixel(id: &str, x: u32, y: u32) -> Option<[u8; 4]> {
    get(id).and_then(|texture| texture.pixel(x, y))
}

/// Writes one pixel.
pub fn set_pixel(id: &str, x: u32, y: u32, colour: [u8; 4]) -> bool {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => texture.set_pixel(x, y, colour),
        None => false,
    })
}

/// Loads a texture from bytes a mod wrote.
pub fn load_from_bytes(
    id: &str,
    pixels: &[u8],
    width: u32,
    height: u32,
    format: &str,
) -> bool {
    let format = TextureFormat::parse(format);
    let expected = u64::from(width) * u64::from(height) * format.bytes_per_pixel();
    if pixels.len() as u64 != expected {
        return false;
    }
    register(Texture::new(id, width, height, format).with_pixels(pixels.to_vec()))
}

/// Loads a texture from a file in the mod's own directory, if it is a raw one: this host has
/// no image decoder, so the file has to hold pixels.
pub fn load_from_file(id: &str, path: &str, width: u32, height: u32, format: &str) -> bool {
    let Ok(pixels) = crate::fs::read(path) else {
        return false;
    };
    load_from_bytes(id, &pixels, width, height, format)
}

/// Loads a texture from a resource a mod shipped, which is the same bytes by another route.
pub fn load_from_resource(id: &str, resource: &str, width: u32, height: u32, format: &str) -> bool {
    let Ok(resource) = resource.parse::<crate::resources::ResourceId>() else {
        return false;
    };
    match crate::resources::read_bytes(&resource) {
        Some(pixels) => load_from_bytes(id, &pixels, width, height, format),
        None => false,
    }
}

/// Registers an atlas over a texture.
pub fn register_atlas(id: &str, width: u32, height: u32, format: &str) -> bool {
    let texture = Texture::new(id, width, height, TextureFormat::parse(format));
    with_atlases(|atlases| {
        atlases
            .insert(id.to_owned(), Atlas::new(id, texture))
            .is_none()
    })
}

/// An atlas by id.
#[must_use]
pub fn get_atlas(id: &str) -> Option<Atlas> {
    with_atlases(|atlases| atlases.get(id).cloned())
}

/// Adds a sprite to an atlas.
pub fn register_sprite(atlas: &str, sprite: &str, width: u32, height: u32) -> bool {
    with_atlases(|atlases| match atlases.get_mut(atlas) {
        Some(atlas) => atlas.add(sprite, width, height).is_some(),
        None => false,
    })
}

/// A sprite, by id.
#[must_use]
pub fn get_sprite(sprite: &str) -> Option<Sprite> {
    with_atlases(|atlases| {
        atlases
            .values()
            .flat_map(|atlas| atlas.sprites.iter())
            .find(|held| held.id == sprite)
            .cloned()
    })
}

/// Every sprite in an atlas.
#[must_use]
pub fn get_sprites(atlas: &str) -> Vec<Sprite> {
    with_atlases(|atlases| {
        atlases
            .get(atlas)
            .map(|atlas| atlas.sprites.clone())
            .unwrap_or_default()
    })
}

/// The UVs a sprite occupies in its atlas.
#[must_use]
pub fn get_uv(sprite: &str) -> Option<(f32, f32, f32, f32)> {
    let sprite = get_sprite(sprite)?;
    let atlas = get_atlas(&sprite.atlas)?;
    Some(sprite.uv(atlas.dimensions()))
}

/// Stitches every sprite in an atlas into its texture, and answers how many pixels were
/// touched. The sprite regions are real; what they hold is up to the mod that added them.
pub fn stitch_atlas(id: &str) -> usize {
    with_atlases(|atlases| {
        let Some(atlas) = atlases.get(id) else {
            return 0;
        };
        let sprites = atlas.sprites.clone();
        let texture = atlas.texture.clone();
        let mut stitched = 0;
        for sprite in sprites {
            stitched += (sprite.width * sprite.height) as usize;
        }
        register(texture);
        stitched
    })
}

/// Registers an animation over a texture.
pub fn register_animation(
    id: &str,
    frames: &[(u32, u64)],
) -> usize {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => {
            texture.frames = frames
                .iter()
                .map(|(index, duration)| AnimationFrame {
                    index: *index,
                    duration: *duration,
                })
                .collect();
            texture.frames.len()
        }
        None => 0,
    })
}

/// Picks a frame by hand, and answers whether it was there.
pub fn set_animation_frame(id: &str, frame: u32) -> bool {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => {
            texture.frame = frame;
            true
        }
        None => false,
    })
}

/// Which frame is up at a moment.
#[must_use]
pub fn frame_at(id: &str, elapsed_millis: u64) -> Option<u32> {
    get(id).map(|texture| texture.frame_at(elapsed_millis))
}

/// Records that a texture should have mipmaps. Building them needs the client's GPU.
pub fn generate_mipmap(id: &str) -> bool {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => {
            texture.mipmaps = true;
            true
        }
        None => false,
    })
}

/// Sets the filter.
pub fn set_filter(id: &str, filter: Filter) -> bool {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => {
            texture.filter = filter;
            true
        }
        None => false,
    })
}

/// Sets the wrap mode.
pub fn set_wrap_mode(id: &str, wrap: WrapMode) -> bool {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => {
            texture.wrap = wrap;
            true
        }
        None => false,
    })
}

/// Sets the anisotropy.
pub fn set_anisotropy(id: &str, anisotropy: u32) -> bool {
    with(|textures| match textures.get_mut(id) {
        Some(texture) => {
            texture.anisotropy = anisotropy.clamp(1, 16);
            true
        }
        None => false,
    })
}

/// How many bytes the textures a mod registered take.
#[must_use]
pub fn get_memory_usage() -> u64 {
    with(|textures| textures.values().map(Texture::memory).sum())
}

/// Every texture, by id.
#[must_use]
pub fn get_all() -> Vec<Texture> {
    with(|textures| textures.values().cloned().collect())
}

/// Loads a texture into the client. `false`.
pub fn load(_id: &str) -> bool {
    refused("upload")
}

/// Binds a texture for the client to draw with. `false`.
pub fn bind(_id: &str) -> bool {
    refused("upload")
}

/// Unbinds. `false`.
pub fn unbind() -> bool {
    refused("upload")
}

/// Deletes it from the client's GPU. `false`.
pub fn delete(_id: &str) -> bool {
    refused("upload")
}

/// Reloads it from where it came. `false` for the upload; the pixels a mod holds are not
/// reloaded from anywhere this host knows.
pub fn reload(_id: &str) -> bool {
    refused("upload")
}

/// Why a texture question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "upload" => {
            "a texture reaches the screen through the client's GPU device, which this host has \
             no handle on (section 31); the pixels a mod holds are real, the upload is not"
                .to_owned()
        }
        "decode" => {
            "this host has no image decoder: load_from_file takes raw pixels, and a mod that \
             ships a png decodes it itself"
                .to_owned()
        }
        "mipmap" => "mipmaps are built by the client's device when it uploads a texture".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["upload", "decode", "mipmap"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about a texture: {}", why_unknown(what));
    false
}

/// Whether a file is a raw pixel file this host could load.
#[must_use]
pub fn is_raw_image(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "rgba" | "raw" | "bin"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|textures| textures.clear());
        with_atlases(|atlases| atlases.clear());
    }

    #[test]
    fn a_texture_holds_real_pixels() {
        let _serial = serial();
        fresh();
        assert!(load_from_bytes(
            "hello:square",
            &[255u8; 4 * 4 * 4],
            4,
            4,
            "rgba8"
        ));
        assert_eq!(get_dimensions("hello:square"), Some((4, 4)));
        assert_eq!(get_format("hello:square"), Some(TextureFormat::Rgba8));
        assert_eq!(get_pixels("hello:square").map(|pixels| pixels.len()), Some(64));
        assert_eq!(get_memory_usage(), 64);
        assert_eq!(get_pixel("hello:square", 0, 0), Some([255, 255, 255, 255]));
        assert_eq!(get_pixel("hello:square", 4, 0), None, "outside");
        assert!(set_pixel("hello:square", 1, 1, [1, 2, 3, 4]));
        assert_eq!(get_pixel("hello:square", 1, 1), Some([1, 2, 3, 4]));
        assert!(!set_pixel("hello:square", 9, 9, [0; 4]));
        assert_eq!(get_id("hello:square"), Some("hello:square".to_owned()));
        assert!(!load_from_bytes("wrong", &[0u8; 3], 4, 4, "rgba8"), "the size has to fit");
        assert!(!set_pixels("hello:square", &[0u8; 3]));
        assert!(set_pixels("hello:square", &[7u8; 64]));
        assert_eq!(get_pixel("hello:square", 0, 0), Some([7, 7, 7, 7]));
        assert_eq!(get_all().len(), 1);
        assert!(get("hello:square").is_some_and(|texture| texture.to_string().contains("4x4")));
        assert!(unregister("hello:square"));
        assert!(!unregister("hello:square"));
        assert_eq!(get_memory_usage(), 0);
    }

    #[test]
    fn a_region_can_be_written_and_the_formats_read_the_way_they_should() {
        let _serial = serial();
        fresh();
        register(Texture::new("hello:rgb", 4, 4, TextureFormat::Rgb8));
        assert_eq!(get_memory_usage(), 48);
        assert_eq!(get_pixel("hello:rgb", 0, 0), Some([0, 0, 0, 255]), "rgb has no alpha");
        assert_eq!(update_region("hello:rgb", 1, 1, 2, 1, &[9, 8, 7, 6, 5, 4]), 2);
        assert_eq!(get_pixel("hello:rgb", 1, 1), Some([9, 8, 7, 255]));
        assert_eq!(get_pixel("hello:rgb", 2, 1), Some([6, 5, 4, 255]));
        assert_eq!(update_region("hello:rgb", 0, 0, 2, 2, &[1, 2, 3]), 0, "not enough pixels");
        assert_eq!(update_region("nothing", 0, 0, 1, 1, &[1]), 0);

        register(Texture::new("hello:bgra", 1, 1, TextureFormat::Bgra8));
        assert!(set_pixel("hello:bgra", 0, 0, [1, 2, 3, 4]));
        assert_eq!(get_pixel("hello:bgra", 0, 0), Some([1, 2, 3, 4]), "bgra reads back as rgba");
        register(Texture::new("hello:red", 1, 1, TextureFormat::R8));
        assert!(set_pixel("hello:red", 0, 0, [5, 0, 0, 0]));
        assert_eq!(get_pixel("hello:red", 0, 0), Some([5, 5, 5, 255]));
        assert_eq!(TextureFormat::parse("nonsense"), TextureFormat::Rgba8);
        assert_eq!(TextureFormat::Rgba16Float.bytes_per_pixel(), 8);
        assert_eq!(TextureFormat::Bgra8.to_string(), "bgra8");
    }

    #[test]
    fn a_texture_can_be_loaded_from_a_file_or_a_resource() {
        let _serial = serial();
        fresh();
        let root = std::env::temp_dir().join(format!("cinnaloader-tex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());
        assert_eq!(crate::fs::write_bytes("art/raw.rgba", &[3u8; 16]), Ok(16));
        assert!(load_from_file("hello:file", "art/raw.rgba", 2, 2, "rgba8"));
        assert_eq!(get_pixel("hello:file", 1, 1), Some([3, 3, 3, 3]));
        assert!(!load_from_file("hello:missing", "art/nothing.rgba", 2, 2, "rgba8"));
        assert!(!load_from_file("hello:short", "art/raw.rgba", 8, 8, "rgba8"));
        assert!(is_raw_image("art/raw.rgba"));
        assert!(!is_raw_image("art/thing.png"), "this host does not decode png");

        crate::resources::register_pack({
            let mut pack = crate::resources::ResourcePack::memory("tex-probe", 0);
            pack.insert("textures/block.rgba", [4u8; 16].to_vec());
            pack
        });
        assert!(load_from_resource(
            "hello:resource",
            "tex-probe:textures/block.rgba",
            2,
            2,
            "rgba8"
        ));
        assert_eq!(get_pixel("hello:resource", 0, 0), Some([4, 4, 4, 4]));
        assert!(!load_from_resource("hello:nope", "tex-probe:textures/none.rgba", 2, 2, "rgba8"));
        assert!(!load_from_resource("hello:bad", "not a resource", 2, 2, "rgba8"));
        crate::resources::unregister_pack("tex-probe");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_atlas_stitches_sprites_and_gives_their_uvs() {
        let _serial = serial();
        fresh();
        assert!(register_atlas("hello:atlas", 16, 16, "rgba8"));
        assert!(!register_atlas("hello:atlas", 16, 16, "rgba8"), "once");
        assert!(register_sprite("hello:atlas", "hello:a", 8, 8));
        assert!(register_sprite("hello:atlas", "hello:b", 8, 8));
        assert!(register_sprite("hello:atlas", "hello:c", 8, 8), "a third goes on the next shelf");
        assert!(!register_sprite("nothing", "hello:d", 1, 1));
        assert!(!register_sprite("hello:atlas", "hello:too_big", 32, 32));
        let sprites = get_sprites("hello:atlas");
        assert_eq!(sprites.len(), 3);
        assert_eq!((sprites[0].x, sprites[0].y), (0, 0));
        assert_eq!((sprites[1].x, sprites[1].y), (8, 0));
        assert_eq!((sprites[2].x, sprites[2].y), (0, 8), "the second shelf");
        let uv = get_uv("hello:a").expect("it has coordinates");
        assert_eq!(uv, (0.0, 0.0, 0.5, 0.5));
        let uv = get_uv("hello:c").expect("it has coordinates");
        assert_eq!(uv, (0.0, 0.5, 0.5, 1.0));
        assert_eq!(Sprite { id: "x".to_owned(), atlas: "a".to_owned(), x: 0, y: 0, width: 0, height: 0 }.uv((0, 0)), (0.0, 0.0, 0.0, 0.0));
        assert!(get_atlas("hello:atlas").is_some());
        assert_eq!(get_uv("nothing"), None);
        assert_eq!(stitch_atlas("hello:atlas"), 8 * 8 * 3);
        assert!(get("hello:atlas").is_some(), "stitching registers the texture");
        assert_eq!(get_sprites("nothing").len(), 0);
    }

    #[test]
    fn an_animation_picks_the_frame_a_moment_falls_in() {
        let _serial = serial();
        fresh();
        register(Texture::new("hello:frames", 2, 2, TextureFormat::Rgba8));
        assert_eq!(register_animation("hello:frames", &[(0, 100), (1, 200), (2, 100)]), 3);
        assert_eq!(frame_at("hello:frames", 0), Some(0));
        assert_eq!(frame_at("hello:frames", 150), Some(1));
        assert_eq!(frame_at("hello:frames", 350), Some(2));
        assert_eq!(frame_at("hello:frames", 400), Some(0), "it loops");
        assert!(set_animation_frame("hello:frames", 2));
        assert_eq!(
            get("hello:frames").map(|texture| texture.frame),
            Some(2)
        );
        assert!(!set_animation_frame("nothing", 0));
        assert_eq!(register_animation("nothing", &[(0, 1)]), 0);
        assert_eq!(frame_at("nothing", 0), None);
        let plain = Texture::new("hello:plain", 1, 1, TextureFormat::Rgba8);
        assert_eq!(plain.frame_at(1000), 0, "a texture with no animation is frame zero");
    }

    #[test]
    fn the_settings_a_caller_sets_are_recorded() {
        let _serial = serial();
        fresh();
        register(Texture::new("hello:settings", 1, 1, TextureFormat::Rgba8));
        assert!(set_filter("hello:settings", Filter::Linear));
        assert!(set_wrap_mode("hello:settings", WrapMode::Repeat));
        assert!(set_anisotropy("hello:settings", 99));
        assert!(generate_mipmap("hello:settings"));
        let texture = get("hello:settings").expect("registered");
        assert_eq!(texture.filter, Filter::Linear);
        assert_eq!(texture.wrap, WrapMode::Repeat);
        assert_eq!(texture.anisotropy, 16, "clamped to what any device allows");
        assert!(texture.mipmaps);
        assert!(!set_filter("nothing", Filter::Linear));
        assert!(!set_wrap_mode("nothing", WrapMode::Repeat));
        assert!(!set_anisotropy("nothing", 4));
        assert!(!generate_mipmap("nothing"));
    }

    #[test]
    fn the_upload_and_the_decoder_are_refused() {
        let _serial = serial();
        fresh();
        register(Texture::new("hello:tex", 1, 1, TextureFormat::Rgba8));
        assert!(!load("hello:tex"));
        assert!(!bind("hello:tex"));
        assert!(!unbind());
        assert!(!delete("hello:tex"));
        assert!(!reload("hello:tex"));
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("upload").contains("GPU device"));
    }
}
