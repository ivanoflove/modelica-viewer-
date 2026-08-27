use crate::diagnostics::Diagnostic;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub p1: Point,
    pub p2: Point,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoordinateSystem {
    pub extent: Extent,
    pub preserve_aspect_ratio: bool,
    pub grid: Option<Point>,
    pub initial_scale: Option<f32>,
}

impl Default for CoordinateSystem {
    fn default() -> Self {
        Self {
            extent: Extent {
                p1: Point {
                    x: -100.0,
                    y: -100.0,
                },
                p2: Point { x: 100.0, y: 100.0 },
            },
            preserve_aspect_ratio: true,
            grid: None,
            initial_scale: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LineGraphic {
    pub origin: Point,
    pub rotation: f32,
    pub points: Vec<Point>,
    pub color: [u8; 3],
    pub pattern: Option<String>,
    pub thickness: f32,
    pub arrow: Vec<String>,
    pub arrow_size: Option<f32>,
    pub smooth: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PolygonGraphic {
    pub origin: Point,
    pub rotation: f32,
    pub points: Vec<Point>,
    pub line_color: [u8; 3],
    pub fill_color: [u8; 3],
    pub line_pattern: Option<String>,
    pub line_thickness: Option<f32>,
    pub fill_pattern: Option<String>,
    pub smooth: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RectangleGraphic {
    pub origin: Point,
    pub rotation: f32,
    pub extent: Extent,
    pub line_color: [u8; 3],
    pub fill_color: [u8; 3],
    pub line_pattern: Option<String>,
    pub line_thickness: Option<f32>,
    pub fill_pattern: Option<String>,
    pub radius: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EllipseGraphic {
    pub origin: Point,
    pub rotation: f32,
    pub extent: Extent,
    pub line_color: [u8; 3],
    pub fill_color: [u8; 3],
    pub line_pattern: Option<String>,
    pub line_thickness: Option<f32>,
    pub fill_pattern: Option<String>,
    pub start_angle: Option<f32>,
    pub end_angle: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextGraphic {
    pub origin: Point,
    pub rotation: f32,
    pub extent: Extent,
    pub text: String,
    pub color: [u8; 3],
    pub font_size: Option<f32>,
    pub font_name: Option<String>,
    pub horizontal_alignment: Option<String>,
    pub text_style: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BitmapGraphic {
    pub origin: Point,
    pub rotation: f32,
    pub extent: Extent,
    pub file_name: Option<String>,
    pub image_source: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Graphic {
    Line(LineGraphic),
    Polygon(PolygonGraphic),
    Rectangle(RectangleGraphic),
    Ellipse(EllipseGraphic),
    Text(TextGraphic),
    Bitmap(BitmapGraphic),
}

/// Stable identity for a resolved graphic in an Icon scene.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GraphicId(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicOwnerKind {
    Own,
    Inherited,
    Connector,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphicOwner {
    pub qualified_name: String,
    pub kind: GraphicOwnerKind,
    pub instance_name: Option<String>,
}

/// Transform applied after the graphic's own origin and rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform2D {
    pub translation: Point,
    pub rotation: f32,
    pub scale_x: f32,
    pub scale_y: f32,
}

impl Transform2D {
    pub const fn identity() -> Self {
        Self {
            translation: Point { x: 0.0, y: 0.0 },
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedGraphic {
    pub id: GraphicId,
    pub graphic: Graphic,
    pub owner: GraphicOwner,
    pub transform: Transform2D,
    pub editable: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IconDebugStats {
    pub own_graphics: usize,
    pub inherited_graphics: usize,
    pub connector_graphics: usize,
    pub editable_graphics: usize,
    pub unresolved_bases: usize,
    pub unresolved_connectors: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentInstance {
    pub id: String,
    pub name: String,
    pub type_name: String,
    pub origin: Point,
    pub rotation: f32,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiagramConnection {
    pub id: String,
    pub from: String,
    pub to: String,
    pub line: Option<LineGraphic>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IconScene {
    pub owner_qualified_name: Option<String>,
    pub coordinate_system: CoordinateSystem,
    pub graphics: Vec<ResolvedGraphic>,
    pub diagnostics: Vec<Diagnostic>,
}

impl IconScene {
    pub fn debug_stats(&self) -> IconDebugStats {
        let mut stats = IconDebugStats {
            unresolved_bases: self
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "ICON_BASE_NOT_FOUND")
                .count(),
            unresolved_connectors: self
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "ICON_CONNECTOR_NOT_FOUND")
                .count(),
            ..Default::default()
        };
        for graphic in &self.graphics {
            match graphic.owner.kind {
                GraphicOwnerKind::Own => stats.own_graphics += 1,
                GraphicOwnerKind::Inherited => stats.inherited_graphics += 1,
                GraphicOwnerKind::Connector => stats.connector_graphics += 1,
            }
            if graphic.editable {
                stats.editable_graphics += 1;
            }
        }
        stats
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiagramScene {
    pub coordinate_system: CoordinateSystem,
    pub background_graphics: Vec<Graphic>,
    pub components: Vec<ComponentInstance>,
    pub connections: Vec<DiagramConnection>,
    pub diagnostics: Vec<Diagnostic>,
}
