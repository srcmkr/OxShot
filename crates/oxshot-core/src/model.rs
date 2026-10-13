//! The editable OxShot document: a base screenshot plus a stack of vector annotations.
//!
//! All coordinates are in *base image pixel space*. The canvas rect decides which part
//! (possibly extending beyond the base image) is exported.

use std::sync::Arc;

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::geom::{IRect, Point, Rect};

pub const DOCUMENT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------------------
// Color

/// Straight (non-premultiplied) sRGB color. Serialized as `"#rrggbbaa"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    /// Screenpresso-like signal red.
    pub const RED: Color = Color::rgb(0xE5, 0x1C, 0x23);
    pub const YELLOW: Color = Color::rgb(0xFF, 0xE1, 0x00);

    pub fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }
    pub fn from_hex(s: &str) -> Option<Color> {
        let s = s.strip_prefix('#')?;
        let p = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            6 => Some(Color::rgb(p(0)?, p(2)?, p(4)?)),
            8 => Some(Color::rgba(p(0)?, p(2)?, p(4)?, p(6)?)),
            _ => None,
        }
    }
    /// Relative luminance in 0..1, used to pick readable text colors.
    pub fn luminance(&self) -> f32 {
        (0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32) / 255.0
    }
}

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}
impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::from_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color {s}")))
    }
}

// ---------------------------------------------------------------------------------------------
// Shared styles

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub color: Color,
    pub width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    /// Font family name; "sans-serif" resolves to the system UI sans font.
    pub family: String,
    /// Font size in image pixels.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub align: Align,
    pub color: Color,
    /// Halo around the glyphs ("glow" in Screenpresso) for legibility on busy backgrounds.
    pub outline: Option<Stroke>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family: "sans-serif".into(),
            size: 20.0,
            bold: true,
            italic: false,
            underline: false,
            align: Align::Left,
            color: Color::RED,
            outline: Some(Stroke { color: Color::WHITE, width: 3.0 }),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Shapes

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ArrowStyle {
    /// Constant-width line with a triangular head.
    #[default]
    Line,
    /// Shaft that widens from the tail to the head (classic "callout arrow").
    Tapered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ArrowHeads {
    #[default]
    End,
    Both,
    /// Plain line.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrowShape {
    pub start: Point,
    /// The tip of the arrow head.
    pub end: Point,
    /// Optional quadratic-Bézier control point for curved arrows.
    pub curve: Option<Point>,
    pub color: Color,
    pub width: f32,
    pub style: ArrowStyle,
    pub heads: ArrowHeads,
    pub dashed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RectShape {
    pub rect: Rect,
    /// `None` = no outline.
    pub stroke: Option<Stroke>,
    pub fill: Option<Color>,
    pub corner_radius: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EllipseShape {
    /// Bounding box of the ellipse.
    pub rect: Rect,
    pub stroke: Option<Stroke>,
    pub fill: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextShape {
    /// Top-left corner of the text block.
    pub origin: Point,
    pub text: String,
    pub style: TextStyle,
    /// Wrap width; `None` = only explicit line breaks.
    pub max_width: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalloutShape {
    /// The text box (body of the bubble).
    pub rect: Rect,
    /// Where the bubble's pointer ends. `None` renders a plain text box.
    pub tail: Option<Point>,
    pub text: String,
    pub style: TextStyle,
    pub fill: Color,
    pub border: Option<Stroke>,
    pub corner_radius: f32,
    /// Inner padding between box edge and text.
    pub padding: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConnectorStyle {
    #[default]
    Line,
    Arrow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    pub target: Point,
    pub style: ConnectorStyle,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepShape {
    pub center: Point,
    pub radius: f32,
    pub number: u32,
    pub fill: Color,
    pub text_color: Color,
    /// Optional ring around the circle (white by default for contrast).
    pub border: Option<Stroke>,
    /// Optional line/arrow from the circle to a target point (drawn below the circle).
    pub connector: Option<Connector>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HighlightShape {
    pub rect: Rect,
    /// Marker color; blended with *multiply* so text underneath stays readable.
    pub color: Color,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolygonShape {
    pub points: Vec<Point>,
    pub closed: bool,
    pub stroke: Option<Stroke>,
    pub fill: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreehandShape {
    pub points: Vec<Point>,
    pub color: Color,
    pub width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlurMode {
    #[default]
    Gaussian,
    Pixelate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlurShape {
    pub rect: Rect,
    pub mode: BlurMode,
    /// 1..=20 like Screenpresso's "blur level".
    pub strength: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StampShape {
    /// Destination rect; the image is stretched into it (editor keeps aspect ratio).
    pub rect: Rect,
    /// Key into [`Document::resources`].
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MagnifierShape {
    /// Center of the magnified source area.
    pub source: Point,
    /// Center of the glass (where the magnified image is shown).
    pub glass: Point,
    /// Glass radius in image pixels. The source radius is `radius / zoom`.
    pub radius: f32,
    pub zoom: f32,
    pub border: Stroke,
    /// Draw the source circle and the cone connecting it with the glass.
    pub show_source: bool,
}

impl MagnifierShape {
    pub fn source_radius(&self) -> f32 {
        self.radius / self.zoom.max(0.01)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BraceShape {
    /// The two ends of the brace (the brace runs along this segment).
    pub start: Point,
    pub end: Point,
    /// How far the middle tip sticks out, measured along `(end - start).perp()`.
    /// Negative values flip the brace to the other side.
    pub depth: f32,
    pub color: Color,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shape {
    Arrow(ArrowShape),
    Rect(RectShape),
    Ellipse(EllipseShape),
    Text(TextShape),
    Callout(CalloutShape),
    Step(StepShape),
    Highlight(HighlightShape),
    Polygon(PolygonShape),
    Freehand(FreehandShape),
    Blur(BlurShape),
    Stamp(StampShape),
    Magnifier(MagnifierShape),
    Brace(BraceShape),
}

impl Shape {
    /// Shapes whose pixels depend on what is below them (they must be re-rendered when
    /// anything underneath changes).
    pub fn reads_backdrop(&self) -> bool {
        matches!(self, Shape::Highlight(_) | Shape::Blur(_) | Shape::Magnifier(_))
    }
}

pub type ObjectId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    /// 0..=1, multiplies the whole object.
    #[serde(default = "one")]
    pub opacity: f32,
    /// Drop shadow strength 0..=1 (0 = none).
    #[serde(default)]
    pub shadow: f32,
    pub shape: Shape,
}

fn one() -> f32 {
    1.0
}

// ---------------------------------------------------------------------------------------------
// Document

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    /// Exported area in base image coordinates. May extend beyond the base image
    /// (padding filled with `background`) or be smaller (crop).
    pub rect: IRect,
    pub background: Color,
}

/// A decoded image shared between document clones (cheap to clone, immutable).
pub type SharedImage = Arc<RgbaImage>;

#[derive(Debug, Clone)]
pub struct Document {
    pub base: SharedImage,
    pub canvas: Canvas,
    pub objects: Vec<Object>,
    /// Images referenced by [`StampShape::resource`].
    pub resources: std::collections::BTreeMap<String, SharedImage>,
    next_id: ObjectId,
}

impl Document {
    pub fn new(base: RgbaImage) -> Self {
        let (w, h) = base.dimensions();
        Self {
            base: Arc::new(base),
            canvas: Canvas { rect: IRect::new(0, 0, w, h), background: Color::TRANSPARENT },
            objects: Vec::new(),
            resources: Default::default(),
            next_id: 1,
        }
    }

    pub fn from_parts(
        base: SharedImage,
        canvas: Canvas,
        objects: Vec<Object>,
        resources: std::collections::BTreeMap<String, SharedImage>,
    ) -> Self {
        let next_id = objects.iter().map(|o| o.id).max().unwrap_or(0) + 1;
        Self { base, canvas, objects, resources, next_id }
    }

    pub fn alloc_id(&mut self) -> ObjectId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Appends a new object on top and returns its id.
    pub fn push(&mut self, shape: Shape) -> ObjectId {
        let id = self.alloc_id();
        self.objects.push(Object { id, opacity: 1.0, shadow: 0.0, shape });
        id
    }

    pub fn index_of(&self, id: ObjectId) -> Option<usize> {
        self.objects.iter().position(|o| o.id == id)
    }

    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    /// Next number for the step tool: one more than the highest existing step.
    pub fn next_step_number(&self) -> u32 {
        self.objects
            .iter()
            .filter_map(|o| match &o.shape {
                Shape::Step(s) => Some(s.number),
                _ => None,
            })
            .max()
            .map_or(1, |n| n + 1)
    }

    pub fn canvas_rect(&self) -> Rect {
        self.canvas.rect.to_rect()
    }
}
