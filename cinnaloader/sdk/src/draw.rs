//! Drawing: a list a mod builds, and the little of it this host can put on screen.
//!
//! Every shape here is a value in a [`DrawList`]: lines, rectangles, circles, polygons, boxes,
//! spheres, text, textures and icons, with the transform stack and the state that goes with
//! them (colour, alpha, blend, depth, cull, line width). Building the list is entirely real,
//! and the transforms compose the way a 2D renderer's do.
//!
//! [`DrawList::flush`] is where honesty is required. This host has exactly one way to put
//! something in front of the player: the loader's overlay line. So a flush draws the *text*
//! commands as overlay lines and counts everything else as a draw call it cannot issue, with
//! the reason in [`why_unknown`] — a mod that builds a HUD gets its text on screen and a
//! truthful report of what was not drawn. `get_draw_calls` answers both counts.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let mut list = draw::DrawList::new();
//! list.translate(10.0, 20.0);
//! list.set_color([1.0, 0.0, 0.0, 1.0]);
//! list.filled_rect(0.0, 0.0, 100.0, 20.0);
//! list.text("hello", 4.0, 4.0);
//! list.push_transform();
//! list.scale(2.0, 2.0);
//! list.line(0.0, 0.0, 10.0, 10.0);
//! list.pop_transform();
//! assert_eq!(list.len(), 3);
//! let (drawn, undrawn) = list.flush();
//! assert_eq!(drawn, 1, "the text went to the overlay");
//! assert_eq!(undrawn, 2, "and the shapes could not be drawn here");
//! ```

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// A colour with an alpha, each between zero and one.
pub type Color = [f32; 4];

/// A point, in two dimensions.
pub type Point = (f32, f32);

/// An affine transform, as a 3×3 matrix in row-major order, which is what a 2D renderer
/// composes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub matrix: [f32; 9],
}

impl Default for Transform {
    fn default() -> Self {
        Self::identity()
    }
}

impl Transform {
    #[must_use]
    pub fn identity() -> Self {
        Self {
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    #[must_use]
    pub fn translation(x: f32, y: f32) -> Self {
        Self {
            matrix: [1.0, 0.0, x, 0.0, 1.0, y, 0.0, 0.0, 1.0],
        }
    }

    #[must_use]
    pub fn rotation(degrees: f32) -> Self {
        let radians = degrees.to_radians();
        let (sin, cos) = radians.sin_cos();
        Self {
            matrix: [cos, -sin, 0.0, sin, cos, 0.0, 0.0, 0.0, 1.0],
        }
    }

    #[must_use]
    pub fn scaling(x: f32, y: f32) -> Self {
        Self {
            matrix: [x, 0.0, 0.0, 0.0, y, 0.0, 0.0, 0.0, 1.0],
        }
    }

    /// The composition `self × other`, which applies `other` first.
    #[must_use]
    pub fn then(&self, other: &Self) -> Self {
        let mut matrix = [0.0f32; 9];
        for row in 0..3 {
            for column in 0..3 {
                let mut sum = 0.0;
                for k in 0..3 {
                    sum += self.matrix[row * 3 + k] * other.matrix[k * 3 + column];
                }
                matrix[row * 3 + column] = sum;
            }
        }
        Self { matrix }
    }

    /// Where a point ends up.
    #[must_use]
    pub fn apply(&self, point: Point) -> Point {
        let (x, y) = point;
        (
            self.matrix[0] * x + self.matrix[1] * y + self.matrix[2],
            self.matrix[3] * x + self.matrix[4] * y + self.matrix[5],
        )
    }

    /// The inverse, or `None` when it cannot be inverted.
    #[must_use]
    pub fn inverted(&self) -> Option<Self> {
        let m = self.matrix;
        let determinant = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6]);
        if determinant.abs() < f32::EPSILON {
            return None;
        }
        let inverse = 1.0 / determinant;
        Some(Self {
            matrix: [
                (m[4] * m[8] - m[5] * m[7]) * inverse,
                (m[2] * m[7] - m[1] * m[8]) * inverse,
                (m[1] * m[5] - m[2] * m[4]) * inverse,
                (m[5] * m[6] - m[3] * m[8]) * inverse,
                (m[0] * m[8] - m[2] * m[6]) * inverse,
                (m[2] * m[3] - m[0] * m[5]) * inverse,
                (m[3] * m[7] - m[4] * m[6]) * inverse,
                (m[1] * m[6] - m[0] * m[7]) * inverse,
                (m[0] * m[4] - m[1] * m[3]) * inverse,
            ],
        })
    }
}

/// How a shape is blended with what is behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlendMode {
    #[default]
    Alpha,
    Additive,
    Multiply,
    Replace,
}

impl BlendMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alpha => "alpha",
            Self::Additive => "additive",
            Self::Multiply => "multiply",
            Self::Replace => "replace",
        }
    }
}

/// Which faces are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CullMode {
    #[default]
    Back,
    Front,
    None,
}

/// One thing in a draw list.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawCommand {
    Line {
        from: Point,
        to: Point,
        width: f32,
    },
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        filled: bool,
        corner: f32,
    },
    GradientRect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        from: Color,
        to: Color,
    },
    Circle {
        x: f32,
        y: f32,
        radius: f32,
        filled: bool,
    },
    Ellipse {
        x: f32,
        y: f32,
        radius_x: f32,
        radius_y: f32,
    },
    Triangle {
        a: Point,
        b: Point,
        c: Point,
    },
    Polygon {
        points: Vec<Point>,
        filled: bool,
    },
    Polyline {
        points: Vec<Point>,
        width: f32,
    },
    Quad {
        corners: [Point; 4],
    },
    /// A three-dimensional shape, in the mod's own coordinates.
    Box3 {
        origin: (f32, f32, f32),
        size: (f32, f32, f32),
        wireframe: bool,
    },
    Sphere3 {
        centre: (f32, f32, f32),
        radius: f32,
        segments: u32,
    },
    Cylinder3 {
        base: (f32, f32, f32),
        radius: f32,
        height: f32,
    },
    Cone3 {
        base: (f32, f32, f32),
        radius: f32,
        height: f32,
    },
    Capsule3 {
        from: (f32, f32, f32),
        to: (f32, f32, f32),
        radius: f32,
    },
    Mesh {
        name: String,
        vertices: usize,
    },
    Text {
        text: String,
        x: f32,
        y: f32,
        shadow: bool,
        outline: bool,
    },
    TexturedRect {
        texture: String,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    Sprite {
        name: String,
        x: f32,
        y: f32,
    },
    Icon {
        kind: IconKind,
        id: String,
        x: f32,
        y: f32,
    },
    /// A scissor rectangle, which clips what follows it.
    Scissor {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    /// The end of a scissor rectangle.
    ClipEnd,
}

/// What kind of icon a command draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconKind {
    Item,
    Block,
    Entity,
}

impl IconKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Item => "item",
            Self::Block => "block",
            Self::Entity => "entity",
        }
    }
}

impl DrawCommand {
    /// Whether this host could draw it, which is only the text, through the overlay.
    #[must_use]
    pub fn is_drawable_here(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// What it is, for a report.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Line { .. } => "line",
            Self::Rect { filled: true, .. } => "filled_rect",
            Self::Rect { .. } => "rect",
            Self::GradientRect { .. } => "gradient_rect",
            Self::Circle { filled: true, .. } => "filled_circle",
            Self::Circle { .. } => "circle",
            Self::Ellipse { .. } => "ellipse",
            Self::Triangle { .. } => "triangle",
            Self::Polygon { .. } => "polygon",
            Self::Polyline { .. } => "polyline",
            Self::Quad { .. } => "quad",
            Self::Box3 { wireframe: true, .. } => "wireframe_box",
            Self::Box3 { .. } => "box",
            Self::Sphere3 { .. } => "sphere",
            Self::Cylinder3 { .. } => "cylinder",
            Self::Cone3 { .. } => "cone",
            Self::Capsule3 { .. } => "capsule",
            Self::Mesh { .. } => "mesh",
            Self::Text { .. } => "text",
            Self::TexturedRect { .. } => "textured_rect",
            Self::Sprite { .. } => "sprite",
            Self::Icon { .. } => "icon",
            Self::Scissor { .. } => "scissor",
            Self::ClipEnd => "clip_end",
        }
    }
}

/// A list of things to draw, with the state and the transform stack that go with them.
#[derive(Debug, Clone)]
pub struct DrawList {
    commands: Vec<(DrawCommand, State)>,
    state: State,
    transforms: Vec<Transform>,
    /// How many flushes have happened.
    flushes: u64,
    /// How many commands were drawn and how many could not be.
    drawn: u64,
    undrawn: u64,
}

/// The state a command is drawn with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub color: Color,
    pub blend: BlendMode,
    pub depth_test: bool,
    pub depth_write: bool,
    pub cull: CullMode,
    pub line_width: f32,
    pub transform: Transform,
}

impl Default for State {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            blend: BlendMode::Alpha,
            depth_test: true,
            depth_write: true,
            cull: CullMode::Back,
            line_width: 1.0,
            transform: Transform::identity(),
        }
    }
}

impl DrawList {
    /// An empty list, with no transform and white.
    #[must_use]
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            state: State::default(),
            transforms: vec![Transform::identity()],
            flushes: 0,
            drawn: 0,
            undrawn: 0,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// The current transform.
    #[must_use]
    pub fn transform(&self) -> Transform {
        self.state.transform
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// Every command, with the state it was added under.
    #[must_use]
    pub fn commands(&self) -> Vec<(&DrawCommand, &State)> {
        self.commands
            .iter()
            .map(|(command, state)| (command, state))
            .collect()
    }

    /// Adds a command under the current state.
    pub fn push_command(&mut self, command: DrawCommand) -> &mut Self {
        self.commands.push((command, self.state));
        self
    }

    /// Throws the list away without drawing it.
    pub fn clear(&mut self) -> usize {
        let count = self.commands.len();
        self.commands.clear();
        count
    }

    /// Sets the colour.
    pub fn set_color(&mut self, color: Color) -> &mut Self {
        self.state.color = color;
        self
    }

    /// Sets the alpha, keeping the colour.
    pub fn set_alpha(&mut self, alpha: f32) -> &mut Self {
        self.state.color[3] = alpha.clamp(0.0, 1.0);
        self
    }

    pub fn set_blend_mode(&mut self, blend: BlendMode) -> &mut Self {
        self.state.blend = blend;
        self
    }

    pub fn set_depth_test(&mut self, enabled: bool) -> &mut Self {
        self.state.depth_test = enabled;
        self
    }

    pub fn set_depth_write(&mut self, enabled: bool) -> &mut Self {
        self.state.depth_write = enabled;
        self
    }

    pub fn set_cull_mode(&mut self, cull: CullMode) -> &mut Self {
        self.state.cull = cull;
        self
    }

    pub fn set_line_width(&mut self, width: f32) -> &mut Self {
        self.state.line_width = width.max(0.0);
        self
    }

    /// Keeps the current transform for a `pop_transform` to come back to.
    pub fn push_transform(&mut self) -> &mut Self {
        self.transforms.push(self.state.transform);
        self
    }

    /// Goes back to the transform a `push_transform` saved.
    pub fn pop_transform(&mut self) -> &mut Self {
        if self.transforms.len() > 1 {
            let previous = self.transforms.pop().unwrap_or_default();
            self.state.transform = previous;
        }
        self
    }

    /// How deep the transform stack is.
    #[must_use]
    pub fn transform_depth(&self) -> usize {
        self.transforms.len()
    }

    pub fn translate(&mut self, x: f32, y: f32) -> &mut Self {
        self.state.transform = self.state.transform.then(&Transform::translation(x, y));
        self
    }

    pub fn rotate(&mut self, degrees: f32) -> &mut Self {
        self.state.transform = self.state.transform.then(&Transform::rotation(degrees));
        self
    }

    pub fn scale(&mut self, x: f32, y: f32) -> &mut Self {
        self.state.transform = self.state.transform.then(&Transform::scaling(x, y));
        self
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) -> &mut Self {
        self.push_command(DrawCommand::Line {
            from: (x1, y1),
            to: (x2, y2),
            width: self.state.line_width,
        })
    }

    pub fn rect(&mut self, x: f32, y: f32, width: f32, height: f32) -> &mut Self {
        self.push_command(DrawCommand::Rect {
            x,
            y,
            width,
            height,
            filled: false,
            corner: 0.0,
        })
    }

    pub fn filled_rect(&mut self, x: f32, y: f32, width: f32, height: f32) -> &mut Self {
        self.push_command(DrawCommand::Rect {
            x,
            y,
            width,
            height,
            filled: true,
            corner: 0.0,
        })
    }

    pub fn rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        corner: f32,
    ) -> &mut Self {
        self.push_command(DrawCommand::Rect {
            x,
            y,
            width,
            height,
            filled: true,
            corner,
        })
    }

    pub fn gradient_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        from: Color,
        to: Color,
    ) -> &mut Self {
        self.push_command(DrawCommand::GradientRect {
            x,
            y,
            width,
            height,
            from,
            to,
        })
    }

    pub fn circle(&mut self, x: f32, y: f32, radius: f32) -> &mut Self {
        self.push_command(DrawCommand::Circle {
            x,
            y,
            radius,
            filled: false,
        })
    }

    pub fn filled_circle(&mut self, x: f32, y: f32, radius: f32) -> &mut Self {
        self.push_command(DrawCommand::Circle {
            x,
            y,
            radius,
            filled: true,
        })
    }

    pub fn ellipse(&mut self, x: f32, y: f32, radius_x: f32, radius_y: f32) -> &mut Self {
        self.push_command(DrawCommand::Ellipse {
            x,
            y,
            radius_x,
            radius_y,
        })
    }

    pub fn triangle(&mut self, a: Point, b: Point, c: Point) -> &mut Self {
        self.push_command(DrawCommand::Triangle { a, b, c })
    }

    pub fn polygon(&mut self, points: &[Point], filled: bool) -> &mut Self {
        self.push_command(DrawCommand::Polygon {
            points: points.to_vec(),
            filled,
        })
    }

    pub fn polyline(&mut self, points: &[Point]) -> &mut Self {
        let width = self.state.line_width;
        self.push_command(DrawCommand::Polyline {
            points: points.to_vec(),
            width,
        })
    }

    pub fn quad(&mut self, corners: [Point; 4]) -> &mut Self {
        self.push_command(DrawCommand::Quad { corners })
    }

    pub fn box_shape(
        &mut self,
        origin: (f32, f32, f32),
        size: (f32, f32, f32),
        wireframe: bool,
    ) -> &mut Self {
        self.push_command(DrawCommand::Box3 {
            origin,
            size,
            wireframe,
        })
    }

    /// The same, under the name the wishlist uses for a solid box.
    pub fn box_(&mut self, origin: (f32, f32, f32), size: (f32, f32, f32)) -> &mut Self {
        self.box_shape(origin, size, false)
    }

    /// The same, outlined.
    pub fn wireframe_box(&mut self, origin: (f32, f32, f32), size: (f32, f32, f32)) -> &mut Self {
        self.box_shape(origin, size, true)
    }

    pub fn sphere(&mut self, centre: (f32, f32, f32), radius: f32, segments: u32) -> &mut Self {
        self.push_command(DrawCommand::Sphere3 {
            centre,
            radius,
            segments,
        })
    }

    pub fn cylinder(&mut self, base: (f32, f32, f32), radius: f32, height: f32) -> &mut Self {
        self.push_command(DrawCommand::Cylinder3 {
            base,
            radius,
            height,
        })
    }

    pub fn cone(&mut self, base: (f32, f32, f32), radius: f32, height: f32) -> &mut Self {
        self.push_command(DrawCommand::Cone3 {
            base,
            radius,
            height,
        })
    }

    pub fn capsule(
        &mut self,
        from: (f32, f32, f32),
        to: (f32, f32, f32),
        radius: f32,
    ) -> &mut Self {
        self.push_command(DrawCommand::Capsule3 { from, to, radius })
    }

    pub fn mesh(&mut self, name: &str, vertices: usize) -> &mut Self {
        self.push_command(DrawCommand::Mesh {
            name: name.to_owned(),
            vertices,
        })
    }

    pub fn text(&mut self, text: &str, x: f32, y: f32) -> &mut Self {
        self.push_command(DrawCommand::Text {
            text: text.to_owned(),
            x,
            y,
            shadow: false,
            outline: false,
        })
    }

    pub fn text_shadow(&mut self, text: &str, x: f32, y: f32) -> &mut Self {
        self.push_command(DrawCommand::Text {
            text: text.to_owned(),
            x,
            y,
            shadow: true,
            outline: false,
        })
    }

    pub fn outlined_text(&mut self, text: &str, x: f32, y: f32) -> &mut Self {
        self.push_command(DrawCommand::Text {
            text: text.to_owned(),
            x,
            y,
            shadow: false,
            outline: true,
        })
    }

    pub fn textured_rect(
        &mut self,
        texture: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> &mut Self {
        self.push_command(DrawCommand::TexturedRect {
            texture: texture.to_owned(),
            x,
            y,
            width,
            height,
        })
    }

    /// The same, filling a whole rectangle with a texture.
    pub fn texture(&mut self, texture: &str, x: f32, y: f32) -> &mut Self {
        self.textured_rect(texture, x, y, 1.0, 1.0)
    }

    pub fn sprite(&mut self, name: &str, x: f32, y: f32) -> &mut Self {
        self.push_command(DrawCommand::Sprite {
            name: name.to_owned(),
            x,
            y,
        })
    }

    pub fn icon(&mut self, kind: IconKind, id: &str, x: f32, y: f32) -> &mut Self {
        self.push_command(DrawCommand::Icon {
            kind,
            id: id.to_owned(),
            x,
            y,
        })
    }

    pub fn scissor(&mut self, x: f32, y: f32, width: f32, height: f32) -> &mut Self {
        self.push_command(DrawCommand::Scissor {
            x,
            y,
            width,
            height,
        })
    }

    pub fn clip_rect(&mut self, x: f32, y: f32, width: f32, height: f32) -> &mut Self {
        self.scissor(x, y, width, height)
    }

    /// Draws what this host can: the text, as overlay lines. Everything else is counted as a
    /// draw call it could not issue, and the list is emptied either way.
    pub fn flush(&mut self) -> (u64, u64) {
        let mut drawn = 0;
        let mut undrawn = 0;
        for (command, state) in std::mem::take(&mut self.commands) {
            match command {
                DrawCommand::Text { text, x, y, .. } => {
                    crate::overlay_line(&format!(
                        "{text} at {x:.0},{y:.0} alpha {:.2}",
                        state.color[3]
                    ));
                    drawn += 1;
                }
                other => {
                    let _ = other.kind();
                    undrawn += 1;
                }
            }
        }
        self.flushes += 1;
        self.drawn += drawn;
        self.undrawn += undrawn;
        note_draw(drawn, undrawn);
        (drawn, undrawn)
    }
}

impl Default for DrawList {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DrawList {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} command(s), {} transform(s) deep, {} flush(es)",
            self.commands.len(),
            self.transforms.len(),
            self.flushes
        )
    }
}

/// How many draw calls the loader has issued, and how many it could not.
static DRAWN: AtomicU64 = AtomicU64::new(0);
static UNDRAWN: AtomicU64 = AtomicU64::new(0);

/// How many commands this host has drawn and how many it could not, across every list.
#[must_use]
pub fn get_draw_calls() -> (u64, u64) {
    (
        DRAWN.load(Ordering::Relaxed),
        UNDRAWN.load(Ordering::Relaxed),
    )
}

/// Adds to the count, which [`DrawList::flush`] does through the loader's overlay.
pub fn note_draw(drawn: u64, undrawn: u64) {
    DRAWN.fetch_add(drawn, Ordering::Relaxed);
    UNDRAWN.fetch_add(undrawn, Ordering::Relaxed);
}

/// Why a shape cannot be drawn here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "shape" => {
            "this host has no renderer of its own: the client's passes are out of reach \
             (section 29), so only text can go on screen, through the loader's overlay"
                .to_owned()
        }
        "texture" => "a texture would be uploaded to the client's GPU device, which this host has no handle on".to_owned(),
        "icon" => "an item, block or entity icon is drawn by the client's own item and model renderers".to_owned(),
        "mesh" => "a mesh would be uploaded and drawn by the client's pipeline".to_owned(),
        "state" => "blend, depth and cull state belong to the client's render pipeline".to_owned(),
        "scissor" => "a scissor rectangle belongs to the client's pass, not to a mod host".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What cannot be drawn here, for a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["shape", "texture", "icon", "mesh", "state", "scissor"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Draws one line on the loader's overlay, which is the only drawing this host can do.
pub fn overlay(text: &str) -> bool {
    crate::overlay_line(text);
    note_draw(1, 0);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn transforms_compose_the_way_a_2d_renderer_does() {
        let _serial = serial();
        let translate = Transform::translation(10.0, 20.0);
        assert_eq!(translate.apply((1.0, 2.0)), (11.0, 22.0));
        let scale = Transform::scaling(2.0, 3.0);
        assert_eq!(scale.apply((1.0, 1.0)), (2.0, 3.0));
        // `a.then(b)` applies `b` to the point first, which is what an immediate-mode API
        // wants: the transform a caller adds last acts on local coordinates, and the one it
        // added first positions the result.
        let moved_then_scaled = translate.then(&scale);
        assert_eq!(moved_then_scaled.apply((1.0, 1.0)), (12.0, 23.0));
        let scaled_then_moved = scale.then(&translate);
        assert_eq!(scaled_then_moved.apply((1.0, 1.0)), (22.0, 63.0));
        // A rotation of a quarter turn sends x to y.
        let quarter = Transform::rotation(90.0);
        let turned = quarter.apply((1.0, 0.0));
        assert!(turned.0.abs() < 1e-6 && (turned.1 - 1.0).abs() < 1e-6, "{turned:?}");
        // Inverting gives back the point.
        let inverse = moved_then_scaled.inverted().expect("invertible");
        let back = inverse.apply(moved_then_scaled.apply((3.0, 4.0)));
        assert!((back.0 - 3.0).abs() < 1e-4 && (back.1 - 4.0).abs() < 1e-4, "{back:?}");
        assert!(Transform::scaling(0.0, 0.0).inverted().is_none());
        assert_eq!(Transform::identity().apply((7.0, 8.0)), (7.0, 8.0));
    }

    #[test]
    fn a_list_builds_every_shape_and_keeps_its_state() {
        let _serial = serial();
        let mut list = DrawList::new();
        assert!(list.is_empty());
        list.set_color([1.0, 0.0, 0.0, 1.0]).set_alpha(0.5).set_blend_mode(BlendMode::Additive);
        list.set_depth_test(false).set_depth_write(false).set_cull_mode(CullMode::None);
        list.set_line_width(2.0);
        list.line(0.0, 0.0, 1.0, 1.0);
        list.rect(0.0, 0.0, 10.0, 10.0);
        list.filled_rect(0.0, 0.0, 10.0, 10.0);
        list.rounded_rect(0.0, 0.0, 10.0, 10.0, 2.0);
        list.gradient_rect(0.0, 0.0, 10.0, 10.0, [1.0; 4], [0.0; 4]);
        list.circle(0.0, 0.0, 5.0);
        list.filled_circle(0.0, 0.0, 5.0);
        list.ellipse(0.0, 0.0, 5.0, 3.0);
        list.triangle((0.0, 0.0), (1.0, 0.0), (0.0, 1.0));
        list.polygon(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], true);
        list.polyline(&[(0.0, 0.0), (1.0, 1.0)]);
        list.quad([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
        list.box_shape((0.0, 0.0, 0.0), (1.0, 1.0, 1.0), false);
        list.wireframe_box((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
        list.sphere((0.0, 0.0, 0.0), 1.0, 16);
        list.cylinder((0.0, 0.0, 0.0), 1.0, 2.0);
        list.cone((0.0, 0.0, 0.0), 1.0, 2.0);
        list.capsule((0.0, 0.0, 0.0), (0.0, 1.0, 0.0), 0.5);
        list.mesh("hello:mesh", 24);
        list.text("hello", 1.0, 2.0);
        list.text_shadow("shadow", 1.0, 2.0);
        list.outlined_text("outlined", 1.0, 2.0);
        list.textured_rect("hello:tex", 0.0, 0.0, 8.0, 8.0);
        list.texture("hello:tex", 0.0, 0.0);
        list.sprite("hello:sprite", 0.0, 0.0);
        list.icon(IconKind::Item, "minecraft:stone", 0.0, 0.0);
        list.icon(IconKind::Block, "minecraft:stone", 0.0, 0.0);
        list.icon(IconKind::Entity, "minecraft:pig", 0.0, 0.0);
        list.scissor(0.0, 0.0, 10.0, 10.0);
        list.clip_rect(0.0, 0.0, 10.0, 10.0);
        assert_eq!(list.len(), 30);
        let state = list.state();
        assert_eq!(state.color, [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(state.blend, BlendMode::Additive);
        assert!(!state.depth_test);
        assert!(!state.depth_write);
        assert_eq!(state.cull, CullMode::None);
        assert_eq!(state.line_width, 2.0);
        let kinds: Vec<&str> = list.commands().iter().map(|(command, _)| command.kind()).collect();
        assert!(kinds.contains(&"filled_rect"));
        assert!(kinds.contains(&"wireframe_box"));
        assert!(kinds.contains(&"text"));
        assert!(kinds.contains(&"icon"));
        assert_eq!(BlendMode::Multiply.as_str(), "multiply");
        assert_eq!(IconKind::Entity.as_str(), "entity");
        assert!(list.to_string().contains("30 command(s)"));
        assert_eq!(list.clear(), 30);
        assert!(list.is_empty());
    }

    #[test]
    fn the_transform_stack_goes_down_and_comes_back_up() {
        let _serial = serial();
        let mut list = DrawList::new();
        list.translate(5.0, 5.0);
        assert_eq!(list.transform().apply((0.0, 0.0)), (5.0, 5.0));
        assert_eq!(list.transform_depth(), 1);
        list.push_transform();
        list.scale(2.0, 2.0);
        assert_eq!(list.transform_depth(), 2);
        // The scale acts on local coordinates and the translate positions the result.
        assert_eq!(list.transform().apply((1.0, 1.0)), (7.0, 7.0));
        list.pop_transform();
        assert_eq!(list.transform_depth(), 1);
        assert_eq!(list.transform().apply((1.0, 1.0)), (6.0, 6.0));
        list.pop_transform();
        assert_eq!(list.transform_depth(), 1, "the bottom of the stack stays");
        list.rotate(90.0);
        assert_eq!(list.transform().apply((1.0, 0.0)), (5.0, 6.0));
    }

    #[test]
    fn a_flush_draws_the_text_and_says_what_it_could_not_draw() {
        let _serial = serial();
        let before = get_draw_calls();
        let mut list = DrawList::new();
        list.filled_rect(0.0, 0.0, 10.0, 10.0);
        list.text("hello", 0.0, 0.0);
        list.line(0.0, 0.0, 1.0, 1.0);
        assert_eq!(list.flush(), (1, 2));
        assert!(list.is_empty(), "a flush empties the list");
        assert_eq!(list.flush(), (0, 0), "and a second one has nothing to do");
        assert!(get_draw_calls().0 > before.0);
        assert!(get_draw_calls().1 > before.1 + 1);
        let mut list = DrawList::new();
        list.outlined_text("outlined", 1.0, 2.0);
        assert_eq!(list.flush().0, 1, "outlined text is still text");
        assert!(overlay("a direct overlay line"));
        assert!(get_draw_calls().0 > before.0 + 1);
    }

    #[test]
    fn there_are_six_things_this_host_cannot_draw_and_each_says_why() {
        let _serial = serial();
        assert_eq!(refusals().len(), 6);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("shape").contains("overlay"));
        assert!(DrawCommand::Text {
            text: "x".to_owned(),
            x: 0.0,
            y: 0.0,
            shadow: false,
            outline: false,
        }
        .is_drawable_here());
        assert!(!DrawCommand::ClipEnd.is_drawable_here());
    }
}
