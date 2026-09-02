//! Semantic connector anchors shared by Diagram interaction and rendering.
//!
//! A connector's connection point is the transformed Modelica coordinate
//! origin of the connector instance. It is deliberately not inferred from a
//! graphic's visual bounds: the graphic is only used for optional hit-test
//! bounds and highlighting.

use std::collections::HashMap;

use modelica_core::ClassKind;
use modelica_core::scene::{
    ComponentInstance, ConnectorRef, DiagramConnection, DiagramScene, Extent, Graphic,
    GraphicOwnerKind, IconScene, Point, ResolvedGraphic, Transform2D,
};

use crate::{Bounds, line_local_to_world, world_to_line_local};

const DEFAULT_COMPONENT_EXTENT: Extent = Extent {
    p1: Point { x: -10.0, y: -10.0 },
    p2: Point { x: 10.0, y: 10.0 },
};

/// Stable identity for a connector in a Diagram.
///
/// `connector_path` is the complete public connector path, including nested
/// names and array subscripts (for example `bus.signal` or `ports[1]`).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PortKey {
    pub owner_component_id: String,
    pub connector_path: String,
}

impl PortKey {
    pub fn new(owner_component_id: impl Into<String>, connector_path: impl Into<String>) -> Self {
        Self {
            owner_component_id: owner_component_id.into(),
            connector_path: connector_path.into(),
        }
    }
}

/// A resolved connector endpoint and its semantic world position.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorAnchor {
    pub key: PortKey,
    pub connector_ref: ConnectorRef,
    pub world_position: Point,
    pub visual_bounds: Option<Bounds>,
    pub qualified_type: Option<String>,
    pub owner_component_id: String,
    pub editable: bool,
}

/// Both endpoints resolved through the same anchor table used for hit testing.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedConnectionEndpoints {
    pub lhs: ConnectorAnchor,
    pub rhs: ConnectorAnchor,
    pub lhs_line_position: Point,
    pub rhs_line_position: Point,
    pub lhs_distance: f32,
    pub rhs_distance: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionEndpointSide {
    Lhs,
    Rhs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectorResolutionError {
    MissingLine,
    MissingEndpoint { side: ConnectionEndpointSide },
}

/// Enumerate semantic connector anchors in a Diagram.
pub fn connector_anchors(scene: &DiagramScene) -> Vec<ConnectorAnchor> {
    let mut anchors = Vec::new();
    for component in &scene.components {
        if !component.visible {
            continue;
        }
        let Some(layer) = component.diagram_layer() else {
            continue;
        };
        let placement = component_placement_transform(layer, component);

        if is_connector_component(component) {
            anchors.push(ConnectorAnchor {
                key: PortKey::new(&component.id, ""),
                connector_ref: ConnectorRef {
                    component_name: component.name.clone(),
                    connector_path: String::new(),
                },
                world_position: transform_point(Point { x: 0.0, y: 0.0 }, placement),
                visual_bounds: scene_visual_bounds(layer, placement),
                qualified_type: component.resolved_type_qualified_name.clone(),
                owner_component_id: component.id.clone(),
                editable: component.editable,
            });
            continue;
        }

        let mut public = HashMap::<String, ConnectorAnchor>::new();
        for graphic in &layer.graphics {
            if graphic.owner.kind != GraphicOwnerKind::Connector {
                continue;
            }
            let Some(path) = graphic.owner.instance_name.as_deref() else {
                continue;
            };
            let path = path.to_owned();
            let connector_transform = compose_transform(placement, graphic.transform);
            let world_position = transform_point(Point { x: 0.0, y: 0.0 }, connector_transform);
            let visual_bounds = resolved_graphic_bounds(graphic, connector_transform);
            public
                .entry(path.clone())
                .and_modify(|anchor| {
                    anchor.visual_bounds = union_bounds(anchor.visual_bounds, visual_bounds);
                })
                .or_insert_with(|| ConnectorAnchor {
                    key: PortKey::new(&component.id, path.clone()),
                    connector_ref: ConnectorRef {
                        component_name: component.name.clone(),
                        connector_path: path,
                    },
                    world_position,
                    visual_bounds,
                    qualified_type: Some(graphic.owner.qualified_name.clone()),
                    owner_component_id: component.id.clone(),
                    editable: component.editable,
                });
        }
        anchors.extend(public.into_values());
    }
    anchors
}

/// Find a connector using the topology reference from a `connect` equation.
pub fn find_connector_anchor<'a>(
    anchors: &'a [ConnectorAnchor],
    connector: &ConnectorRef,
) -> Option<&'a ConnectorAnchor> {
    anchors.iter().find(|anchor| {
        anchor.connector_ref.component_name == connector.component_name
            && anchor.connector_ref.connector_path == connector.connector_path
    })
}

/// Find the nearest connector within a model-space tolerance.
pub fn nearest_connector_anchor<'a>(
    anchors: &'a [ConnectorAnchor],
    point: Point,
    tolerance: f32,
) -> Option<&'a ConnectorAnchor> {
    let tolerance = tolerance.max(0.0);
    anchors
        .iter()
        .filter_map(|anchor| {
            let distance = distance(anchor.world_position, point);
            (distance <= tolerance).then_some((distance, anchor))
        })
        .min_by(|(left, _), (right, _)| left.total_cmp(right))
        .map(|(_, anchor)| anchor)
}

/// Resolve both line endpoints to semantic connector positions.
pub fn resolve_connection_endpoints(
    scene: &DiagramScene,
    connection: &DiagramConnection,
) -> Result<ResolvedConnectionEndpoints, ConnectorResolutionError> {
    let Some(line) = connection.line.as_ref() else {
        return Err(ConnectorResolutionError::MissingLine);
    };
    let Some(rhs_index) = line.points.len().checked_sub(1) else {
        return Err(ConnectorResolutionError::MissingLine);
    };
    let anchors = connector_anchors(scene);
    let lhs = find_connector_anchor(&anchors, &connection.lhs)
        .cloned()
        .ok_or(ConnectorResolutionError::MissingEndpoint {
            side: ConnectionEndpointSide::Lhs,
        })?;
    let rhs = find_connector_anchor(&anchors, &connection.rhs)
        .cloned()
        .ok_or(ConnectorResolutionError::MissingEndpoint {
            side: ConnectionEndpointSide::Rhs,
        })?;
    let lhs_line_position = line_local_to_world(line, line.points[0]);
    let rhs_line_position = line_local_to_world(line, line.points[rhs_index]);
    Ok(ResolvedConnectionEndpoints {
        lhs_distance: distance(lhs.world_position, lhs_line_position),
        rhs_distance: distance(rhs.world_position, rhs_line_position),
        lhs,
        rhs,
        lhs_line_position,
        rhs_line_position,
    })
}

/// Return line-local positions that are strictly anchored to the connectors.
pub fn strict_connection_points(
    scene: &DiagramScene,
    connection: &DiagramConnection,
) -> Result<(Point, Point), ConnectorResolutionError> {
    let endpoints = resolve_connection_endpoints(scene, connection)?;
    let Some(line) = connection.line.as_ref() else {
        return Err(ConnectorResolutionError::MissingLine);
    };
    Ok((
        world_to_line_local(line, endpoints.lhs.world_position),
        world_to_line_local(line, endpoints.rhs.world_position),
    ))
}

fn is_connector_component(component: &ComponentInstance) -> bool {
    matches!(
        component.class_kind,
        Some(ClassKind::Connector | ClassKind::ExpandableConnector)
    )
}

fn component_placement_transform(layer: &IconScene, component: &ComponentInstance) -> Transform2D {
    let target = component
        .placement_extent
        .unwrap_or(DEFAULT_COMPONENT_EXTENT);
    let source = layer.coordinate_system.extent;
    let source_width = non_zero_dimension(source.p2.x - source.p1.x);
    let source_height = non_zero_dimension(source.p2.y - source.p1.y);
    let scale_x = (target.p2.x - target.p1.x) / source_width;
    let scale_y = (target.p2.y - target.p1.y) / source_height;
    Transform2D {
        translation: Point {
            x: component.origin.x + target.p1.x - source.p1.x * scale_x,
            y: component.origin.y + target.p1.y - source.p1.y * scale_y,
        },
        rotation: component.rotation,
        scale_x,
        scale_y,
    }
}

fn non_zero_dimension(value: f32) -> f32 {
    if value.abs() <= f32::EPSILON {
        1.0
    } else {
        value
    }
}

fn compose_transform(parent: Transform2D, child: Transform2D) -> Transform2D {
    let angle = parent.rotation.to_radians();
    let child_translation = Point {
        x: child.translation.x * parent.scale_x,
        y: child.translation.y * parent.scale_y,
    };
    Transform2D {
        translation: Point {
            x: parent.translation.x + child_translation.x * angle.cos()
                - child_translation.y * angle.sin(),
            y: parent.translation.y
                + child_translation.x * angle.sin()
                + child_translation.y * angle.cos(),
        },
        rotation: parent.rotation + child.rotation,
        scale_x: parent.scale_x * child.scale_x,
        scale_y: parent.scale_y * child.scale_y,
    }
}

fn transform_point(point: Point, transform: Transform2D) -> Point {
    let angle = transform.rotation.to_radians();
    let scaled_x = point.x * transform.scale_x;
    let scaled_y = point.y * transform.scale_y;
    Point {
        x: transform.translation.x + scaled_x * angle.cos() - scaled_y * angle.sin(),
        y: transform.translation.y + scaled_x * angle.sin() + scaled_y * angle.cos(),
    }
}

fn resolved_graphic_bounds(graphic: &ResolvedGraphic, transform: Transform2D) -> Option<Bounds> {
    let (points, origin, rotation) = match &graphic.graphic {
        Graphic::Line(line) => (line.points.clone(), line.origin, line.rotation),
        Graphic::Polygon(polygon) => (polygon.points.clone(), polygon.origin, polygon.rotation),
        Graphic::Rectangle(rectangle) => (
            extent_corners(rectangle.extent),
            rectangle.origin,
            rectangle.rotation,
        ),
        Graphic::Ellipse(ellipse) => (
            extent_corners(ellipse.extent),
            ellipse.origin,
            ellipse.rotation,
        ),
        Graphic::Text(text) => (extent_corners(text.extent), text.origin, text.rotation),
        Graphic::Bitmap(bitmap) => (
            extent_corners(bitmap.extent),
            bitmap.origin,
            bitmap.rotation,
        ),
    };
    let points = points
        .into_iter()
        .map(|point| transform_point(rotate_about_origin(point, origin, rotation), transform));
    bounds_from_points(points)
}

fn scene_visual_bounds(layer: &IconScene, placement: Transform2D) -> Option<Bounds> {
    layer
        .graphics
        .iter()
        .filter_map(|graphic| {
            resolved_graphic_bounds(graphic, compose_transform(placement, graphic.transform))
        })
        .fold(None, |bounds, next| union_bounds(bounds, Some(next)))
}

fn rotate_about_origin(point: Point, origin: Point, rotation: f32) -> Point {
    let angle = rotation.to_radians();
    let x = point.x - origin.x;
    let y = point.y - origin.y;
    Point {
        x: origin.x + x * angle.cos() - y * angle.sin(),
        y: origin.y + x * angle.sin() + y * angle.cos(),
    }
}

fn extent_corners(extent: Extent) -> Vec<Point> {
    vec![
        extent.p1,
        Point {
            x: extent.p2.x,
            y: extent.p1.y,
        },
        extent.p2,
        Point {
            x: extent.p1.x,
            y: extent.p2.y,
        },
    ]
}

fn bounds_from_points(points: impl Iterator<Item = Point>) -> Option<Bounds> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut any = false;
    for point in points {
        any = true;
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    any.then_some(Bounds {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

fn union_bounds(left: Option<Bounds>, right: Option<Bounds>) -> Option<Bounds> {
    match (left, right) {
        (None, bounds) | (bounds, None) => bounds,
        (Some(left), Some(right)) => {
            let min_x = left.x.min(right.x);
            let min_y = left.y.min(right.y);
            let max_x = (left.x + left.width).max(right.x + right.width);
            let max_y = (left.y + left.height).max(right.y + right.height);
            Some(Bounds {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            })
        }
    }
}

fn distance(left: Point, right: Point) -> f32 {
    (left.x - right.x).hypot(left.y - right.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelica_core::scene::{
        CoordinateSystem, DiagramConnection, GraphicId, GraphicOwner, LineGraphic, ResolvedGraphic,
    };

    fn point(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    fn layer(
        owner: &str,
        graphic_owner: GraphicOwner,
        graphic_transform: Transform2D,
    ) -> IconScene {
        IconScene {
            owner_qualified_name: Some(owner.into()),
            coordinate_system: CoordinateSystem::default(),
            graphics: vec![ResolvedGraphic {
                id: GraphicId(format!("{owner}:graphic")),
                graphic: Graphic::Ellipse(modelica_core::scene::EllipseGraphic {
                    origin: point(0.0, 0.0),
                    rotation: 0.0,
                    extent: Extent {
                        p1: point(-40.0, 40.0),
                        p2: point(40.0, -40.0),
                    },
                    line_color: [0, 0, 0],
                    fill_color: [0, 127, 255],
                    line_pattern: None,
                    line_thickness: None,
                    fill_pattern: None,
                    start_angle: None,
                    end_angle: None,
                }),
                owner: graphic_owner,
                transform: graphic_transform,
                editable: false,
            }],
            diagnostics: Vec::new(),
        }
    }

    fn component(
        id: &str,
        name: &str,
        kind: Option<ClassKind>,
        origin: Point,
        extent: Extent,
        layer: IconScene,
    ) -> ComponentInstance {
        ComponentInstance {
            id: id.into(),
            name: name.into(),
            source_owner: "Example".into(),
            type_name: "Port".into(),
            resolved_type_qualified_name: Some("Example.Port".into()),
            class_kind: kind,
            origin,
            rotation: 0.0,
            placement_extent: Some(extent),
            visible: true,
            editable: true,
            resolved_icon: Some(Box::new(layer)),
            resolved_diagram: None,
        }
    }

    fn scene(
        components: Vec<ComponentInstance>,
        connection: Option<DiagramConnection>,
    ) -> DiagramScene {
        DiagramScene {
            class_qualified_name: Some("Example.Top".into()),
            class_kind: Some(ClassKind::Model),
            coordinate_system: CoordinateSystem::default(),
            background_graphics: Vec::new(),
            components,
            connections: connection.into_iter().collect(),
            diagnostics: Vec::new(),
            content_bounds: None,
        }
    }

    #[test]
    fn top_level_connector_anchor_uses_transformed_origin() {
        let scene = scene(
            vec![component(
                "port-id",
                "port_a",
                Some(ClassKind::Connector),
                point(10.0, 20.0),
                Extent {
                    p1: point(-110.0, -10.0),
                    p2: point(-90.0, 10.0),
                },
                layer(
                    "Example.Port",
                    GraphicOwner {
                        qualified_name: "Example.Port".into(),
                        kind: GraphicOwnerKind::Own,
                        instance_name: None,
                    },
                    Transform2D::identity(),
                ),
            )],
            None,
        );
        let anchors = connector_anchors(&scene);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].key, PortKey::new("port-id", ""));
        assert_eq!(anchors[0].connector_ref.component_name, "port_a");
        assert_eq!(anchors[0].world_position, point(-90.0, 20.0));
    }

    #[test]
    fn public_connector_keeps_nested_path_and_mirror() {
        let scene = scene(
            vec![component(
                "body-id",
                "body",
                Some(ClassKind::Model),
                point(0.0, 0.0),
                Extent {
                    p1: point(-100.0, -100.0),
                    p2: point(100.0, 100.0),
                },
                layer(
                    "Example.Body",
                    GraphicOwner {
                        qualified_name: "Example.Port".into(),
                        kind: GraphicOwnerKind::Connector,
                        instance_name: Some("bus.signal[1]".into()),
                    },
                    Transform2D {
                        translation: point(50.0, 0.0),
                        rotation: 0.0,
                        scale_x: -1.0,
                        scale_y: 1.0,
                    },
                ),
            )],
            None,
        );
        let anchors = connector_anchors(&scene);
        assert_eq!(anchors[0].key.connector_path, "bus.signal[1]");
        assert_eq!(anchors[0].connector_ref.connector_path, "bus.signal[1]");
        assert_eq!(anchors[0].world_position, point(50.0, 0.0));
    }

    #[test]
    fn strict_connection_points_use_world_anchor_and_preserve_line_rotation() {
        let components = vec![
            component(
                "a-id",
                "a",
                Some(ClassKind::Connector),
                point(-50.0, 0.0),
                Extent {
                    p1: point(-10.0, -10.0),
                    p2: point(10.0, 10.0),
                },
                layer(
                    "Example.Port",
                    GraphicOwner {
                        qualified_name: "Example.Port".into(),
                        kind: GraphicOwnerKind::Own,
                        instance_name: None,
                    },
                    Transform2D::identity(),
                ),
            ),
            component(
                "b-id",
                "b",
                Some(ClassKind::Connector),
                point(50.0, 0.0),
                Extent {
                    p1: point(-10.0, -10.0),
                    p2: point(10.0, 10.0),
                },
                layer(
                    "Example.Port",
                    GraphicOwner {
                        qualified_name: "Example.Port".into(),
                        kind: GraphicOwnerKind::Own,
                        instance_name: None,
                    },
                    Transform2D::identity(),
                ),
            ),
        ];
        let line = LineGraphic {
            origin: point(0.0, 0.0),
            rotation: 90.0,
            points: vec![point(0.0, 50.0), point(0.0, -50.0)],
            color: [0, 0, 0],
            pattern: None,
            thickness: 1.0,
            arrow: Vec::new(),
            arrow_size: None,
            smooth: None,
        };
        let connection = DiagramConnection {
            key: modelica_core::scene::ConnectionKey::new(
                "Example.Top",
                ConnectorRef {
                    component_name: "a".into(),
                    connector_path: String::new(),
                },
                ConnectorRef {
                    component_name: "b".into(),
                    connector_path: String::new(),
                },
                0,
            ),
            id: "connection:0".into(),
            lhs: ConnectorRef {
                component_name: "a".into(),
                connector_path: String::new(),
            },
            rhs: ConnectorRef {
                component_name: "b".into(),
                connector_path: String::new(),
            },
            from: "a".into(),
            to: "b".into(),
            line: Some(line),
            source_range: None,
            line_source_range: None,
        };
        let scene = scene(components, Some(connection));
        let endpoints = resolve_connection_endpoints(&scene, &scene.connections[0]).unwrap();
        assert!(endpoints.lhs_distance < 0.001);
        assert!(endpoints.rhs_distance < 0.001);
        let points = strict_connection_points(&scene, &scene.connections[0]).unwrap();
        assert!((points.0.x - 0.0).abs() < 0.001);
        assert!((points.0.y - 50.0).abs() < 0.001);
        assert!((points.1.x - 0.0).abs() < 0.001);
        assert!((points.1.y + 50.0).abs() < 0.001);
    }
}
