use crate::diagnostics::Diagnostic;
use crate::{ClassKind, SourceRange};

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
    pub fill_color: Option<[u8; 3]>,
    pub fill_pattern: Option<String>,
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiagramDebugStats {
    pub own_components: usize,
    pub inherited_components: usize,
    pub connector_components: usize,
    pub unresolved_components: usize,
    pub unresolved_bases: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentInstance {
    pub id: String,
    pub name: String,
    /// Qualified class that owns the declaration and its Placement.
    pub source_owner: String,
    pub type_name: String,
    pub resolved_type_qualified_name: Option<String>,
    pub class_kind: Option<ClassKind>,
    pub origin: Point,
    pub rotation: f32,
    pub placement_extent: Option<Extent>,
    pub visible: bool,
    /// Inherited components are visible and can be hit-tested, but are not
    /// directly editable until Modelica modifiers are supported.
    pub editable: bool,
    pub resolved_icon: Option<Box<IconScene>>,
    /// Connector components use their Diagram layer in a parent Diagram,
    /// falling back to the connector Icon when no Diagram layer exists.
    pub resolved_diagram: Option<Box<IconScene>>,
}

impl ComponentInstance {
    /// Select the graphic layer used when this component is placed in a
    /// Diagram. This mirrors Modelica's connector rendering rule: connector
    /// classes prefer their own Diagram annotation, while ordinary classes
    /// always use Icon.
    pub fn diagram_layer(&self) -> Option<&IconScene> {
        match self.class_kind {
            Some(ClassKind::Connector | ClassKind::ExpandableConnector) => self
                .resolved_diagram
                .as_deref()
                .or(self.resolved_icon.as_deref()),
            _ => self.resolved_icon.as_deref(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConnectorRef {
    pub component_name: String,
    pub connector_path: String,
}

/// Stable semantic identity for a `connect(lhs, rhs)` equation within a
/// class. Source ranges are intentionally excluded: every source edit can
/// move byte offsets, while the connector references and occurrence remain
/// meaningful after reparsing.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConnectionKey {
    pub owner_class: String,
    pub lhs: ConnectorRef,
    pub rhs: ConnectorRef,
    pub occurrence: usize,
}

impl ConnectionKey {
    pub fn new(
        owner_class: impl Into<String>,
        lhs: ConnectorRef,
        rhs: ConnectorRef,
        occurrence: usize,
    ) -> Self {
        Self {
            owner_class: owner_class.into(),
            lhs,
            rhs,
            occurrence,
        }
    }

    pub fn stable_id(&self) -> String {
        format!(
            "connection:{}:{}->{}#{}",
            self.owner_class,
            connector_ref_id(&self.lhs),
            connector_ref_id(&self.rhs),
            self.occurrence
        )
    }
}

fn connector_ref_id(reference: &ConnectorRef) -> String {
    if reference.connector_path.is_empty() {
        reference.component_name.clone()
    } else {
        format!("{}.{}", reference.component_name, reference.connector_path)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiagramConnection {
    pub key: ConnectionKey,
    pub id: String,
    pub lhs: ConnectorRef,
    pub rhs: ConnectorRef,
    /// Kept for callers that still display the original connect arguments.
    pub from: String,
    pub to: String,
    pub line: Option<LineGraphic>,
    pub source_range: Option<SourceRange>,
    pub line_source_range: Option<SourceRange>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagramBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
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
    pub class_qualified_name: Option<String>,
    pub class_kind: Option<ClassKind>,
    pub coordinate_system: CoordinateSystem,
    pub background_graphics: Vec<Graphic>,
    pub components: Vec<ComponentInstance>,
    pub connections: Vec<DiagramConnection>,
    pub diagnostics: Vec<Diagnostic>,
    pub content_bounds: Option<DiagramBounds>,
}

impl DiagramScene {
    pub fn debug_stats(&self) -> DiagramDebugStats {
        let mut stats = DiagramDebugStats {
            unresolved_components: self
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "DIAGRAM_COMPONENT_TYPE_UNRESOLVED")
                .count(),
            unresolved_bases: self
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "DIAGRAM_BASE_NOT_FOUND")
                .count(),
            ..Default::default()
        };
        for component in &self.components {
            if component.editable {
                stats.own_components += 1;
            } else {
                stats.inherited_components += 1;
            }
            if matches!(
                component.class_kind,
                Some(ClassKind::Connector | ClassKind::ExpandableConnector)
            ) {
                stats.connector_components += 1;
            }
        }
        stats
    }
}
