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

use crate::connection_edit::ORTHOGONAL_EPSILON;
use crate::{Bounds, line_local_to_world, world_to_line_local};

const DEFAULT_COMPONENT_EXTENT: Extent = Extent {
    p1: Point { x: -10.0, y: -10.0 },
    p2: Point { x: 10.0, y: 10.0 },
};

/// Stable identity for a connector in a Diagram.
///
/// `connector_path` contains nested names without the terminal array
/// subscripts. Array element identity lives in `ConnectorRef::subscripts`.
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
    /// Geometric order of the parsed Line points. Modelica's `connect(lhs,
    /// rhs)` argument order does not require `Line.points` to use the same
    /// direction.
    pub point_order: ConnectionPointOrder,
    pub lhs_line_position: Point,
    pub rhs_line_position: Point,
    pub lhs_distance: f32,
    pub rhs_distance: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionPointOrder {
    LhsToRhs,
    RhsToLhs,
}

impl ConnectionPointOrder {
    pub fn is_lhs_first(self) -> bool {
        matches!(self, Self::LhsToRhs)
    }
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
            let base = ConnectorRef::with_component_path(&component.name, "");
            let world_position = transform_point(Point { x: 0.0, y: 0.0 }, placement);
            let visual_bounds = scene_visual_bounds(layer, placement);
            let template = ConnectorAnchor {
                key: PortKey::new(&component.id, ""),
                connector_ref: base.clone(),
                world_position,
                visual_bounds,
                qualified_type: component.resolved_type_qualified_name.clone(),
                owner_component_id: component.id.clone(),
                editable: component.editable,
            };
            anchors.extend(expand_array_anchors(
                scene,
                component,
                &base,
                template,
                &component.dimensions,
            ));
            continue;
        }

        let mut public = HashMap::<ConnectorRef, (ConnectorAnchor, Vec<String>)>::new();
        for graphic in &layer.graphics {
            if graphic.owner.kind != GraphicOwnerKind::Connector {
                continue;
            }
            let Some(path) = graphic.owner.instance_name.as_deref() else {
                continue;
            };
            let connector_ref = ConnectorRef::with_component_path(&component.name, path);
            let graphic_dimensions = graphic.owner.dimensions.clone();
            let connector_transform = compose_transform(placement, graphic.transform);
            let world_position = transform_point(Point { x: 0.0, y: 0.0 }, connector_transform);
            let visual_bounds = resolved_graphic_bounds(graphic, connector_transform);
            public
                .entry(connector_ref.clone())
                .and_modify(|(anchor, dimensions)| {
                    anchor.visual_bounds = union_bounds(anchor.visual_bounds, visual_bounds);
                    if dimensions.is_empty() {
                        *dimensions = graphic_dimensions.clone();
                    }
                })
                .or_insert_with(|| {
                    (
                        ConnectorAnchor {
                            key: PortKey::new(&component.id, connector_ref.connector_path_text()),
                            connector_ref,
                            world_position,
                            visual_bounds,
                            qualified_type: Some(graphic.owner.qualified_name.clone()),
                            owner_component_id: component.id.clone(),
                            editable: component.editable,
                        },
                        graphic_dimensions,
                    )
                });
        }
        for (base, (template, dimensions)) in public {
            anchors.extend(expand_array_anchors(
                scene,
                component,
                &base,
                template,
                &dimensions,
            ));
        }
    }
    anchors.sort_by(|left, right| {
        left.owner_component_id
            .cmp(&right.owner_component_id)
            .then_with(|| {
                left.connector_ref
                    .connector_path
                    .cmp(&right.connector_ref.connector_path)
            })
            .then_with(|| {
                left.connector_ref
                    .subscripts
                    .cmp(&right.connector_ref.subscripts)
            })
            .then_with(|| {
                left.key
                    .owner_component_id
                    .cmp(&right.key.owner_component_id)
            })
            .then_with(|| left.key.connector_path.cmp(&right.key.connector_path))
    });
    anchors
}

fn expand_array_anchors(
    scene: &DiagramScene,
    component: &ComponentInstance,
    base: &ConnectorRef,
    template: ConnectorAnchor,
    dimensions: &[String],
) -> Vec<ConnectorAnchor> {
    let mut references = scene
        .connections
        .iter()
        .flat_map(|connection| [&connection.lhs, &connection.rhs])
        .filter(|reference| {
            reference.component_name == component.name
                && reference.connector_path == base.connector_path
        })
        .filter(|reference| base.subscripts.is_empty() || reference == &base)
        .cloned()
        .collect::<Vec<_>>();

    if !base.subscripts.is_empty() {
        references.push(base.clone());
    } else {
        references.extend(
            constant_array_subscripts(dimensions)
                .into_iter()
                .map(|subscripts| ConnectorRef {
                    component_name: component.name.clone(),
                    connector_path: base.connector_path.clone(),
                    subscripts,
                }),
        );
    }
    if references.is_empty() {
        references.push(base.clone());
    }
    references.sort_by(|left, right| left.subscripts.cmp(&right.subscripts));
    references.dedup();

    references
        .into_iter()
        .map(|connector_ref| {
            let mut anchor = template.clone();
            anchor.key = PortKey::new(&component.id, connector_ref.connector_path_text());
            anchor.connector_ref = connector_ref;
            anchor
        })
        .collect()
}

fn constant_array_subscripts(dimensions: &[String]) -> Vec<Vec<String>> {
    if dimensions.is_empty() {
        return Vec::new();
    }
    let mut combinations = vec![Vec::new()];
    for dimension in dimensions {
        let sizes = split_dimension_values(dimension)
            .into_iter()
            .map(|value| value.parse::<usize>().ok())
            .collect::<Option<Vec<_>>>();
        let Some(sizes) = sizes else {
            return Vec::new();
        };
        if sizes.contains(&0) {
            return Vec::new();
        }
        for size in sizes {
            combinations = combinations
                .into_iter()
                .flat_map(|prefix| {
                    (1..=size).map(move |index| {
                        let mut subscripts = prefix.clone();
                        subscripts.push(index.to_string());
                        subscripts
                    })
                })
                .collect();
        }
    }
    combinations
}

fn split_dimension_values(value: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut paren_depth = 0;
    let mut brace_depth = 0;
    for (index, character) in value.char_indices() {
        match character {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            ',' if paren_depth == 0 && brace_depth == 0 => {
                result.push(value[start..index].trim().to_owned());
                start = index + 1;
            }
            _ => {}
        }
    }
    let last = value[start..].trim();
    if !last.is_empty() {
        result.push(last.to_owned());
    }
    result
}

/// Find a connector using the topology reference from a `connect` equation.
pub fn find_connector_anchor<'a>(
    anchors: &'a [ConnectorAnchor],
    connector: &ConnectorRef,
) -> Option<&'a ConnectorAnchor> {
    anchors
        .iter()
        .find(|anchor| anchor.connector_ref == *connector)
}

/// Find the nearest connector within a model-space tolerance.
pub fn nearest_connector_anchor(
    anchors: &[ConnectorAnchor],
    point: Point,
    tolerance: f32,
) -> Option<&ConnectorAnchor> {
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

/// Return the hit distance for a connector's semantic point or visual bounds.
///
/// The semantic point is preferred. The visual bounds make larger connector
/// graphics easy to select without changing the point used for topology.
pub fn connector_anchor_hit_distance(
    anchor: &ConnectorAnchor,
    point: Point,
    tolerance: f32,
) -> Option<f32> {
    let semantic_distance = distance(anchor.world_position, point);
    if semantic_distance <= tolerance.max(0.0) {
        return Some(semantic_distance);
    }
    let bounds = anchor.visual_bounds?;
    let min_x = bounds.x - tolerance.max(0.0);
    let min_y = bounds.y - tolerance.max(0.0);
    let max_x = bounds.x + bounds.width + tolerance.max(0.0);
    let max_y = bounds.y + bounds.height + tolerance.max(0.0);
    if point.x < min_x || point.x > max_x || point.y < min_y || point.y > max_y {
        return None;
    }
    let nearest_x = point.x.clamp(bounds.x, bounds.x + bounds.width);
    let nearest_y = point.y.clamp(bounds.y, bounds.y + bounds.height);
    Some(distance(
        point,
        Point {
            x: nearest_x,
            y: nearest_y,
        },
    ))
}

/// Find the nearest connector using both semantic and visual hit regions.
pub fn hit_test_connector_anchor(
    anchors: &[ConnectorAnchor],
    point: Point,
    tolerance: f32,
) -> Option<&ConnectorAnchor> {
    anchors
        .iter()
        .filter_map(|anchor| {
            connector_anchor_hit_distance(anchor, point, tolerance)
                .map(|distance| (distance, anchor))
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
    let lhs = match find_connector_anchor(&anchors, &connection.lhs).cloned() {
        Some(anchor) => anchor,
        None => {
            report_missing_connector(
                scene,
                connection,
                ConnectionEndpointSide::Lhs,
                &connection.lhs,
                &anchors,
            );
            return Err(ConnectorResolutionError::MissingEndpoint {
                side: ConnectionEndpointSide::Lhs,
            });
        }
    };
    let rhs = match find_connector_anchor(&anchors, &connection.rhs).cloned() {
        Some(anchor) => anchor,
        None => {
            report_missing_connector(
                scene,
                connection,
                ConnectionEndpointSide::Rhs,
                &connection.rhs,
                &anchors,
            );
            return Err(ConnectorResolutionError::MissingEndpoint {
                side: ConnectionEndpointSide::Rhs,
            });
        }
    };
    let lhs_line_position = line_local_to_world(line, line.points[0]);
    let rhs_line_position = line_local_to_world(line, line.points[rhs_index]);
    let forward_error = distance(lhs.world_position, lhs_line_position)
        + distance(rhs.world_position, rhs_line_position);
    let reverse_error = distance(rhs.world_position, lhs_line_position)
        + distance(lhs.world_position, rhs_line_position);
    Ok(ResolvedConnectionEndpoints {
        lhs_distance: distance(lhs.world_position, lhs_line_position),
        rhs_distance: distance(rhs.world_position, rhs_line_position),
        point_order: if forward_error <= reverse_error {
            ConnectionPointOrder::LhsToRhs
        } else {
            ConnectionPointOrder::RhsToLhs
        },
        lhs,
        rhs,
        lhs_line_position,
        rhs_line_position,
    })
}

fn report_missing_connector(
    scene: &DiagramScene,
    connection: &DiagramConnection,
    side: ConnectionEndpointSide,
    reference: &ConnectorRef,
    anchors: &[ConnectorAnchor],
) {
    let available = anchors
        .iter()
        .filter(|anchor| anchor.connector_ref.component_name == reference.component_name)
        .map(|anchor| anchor.connector_ref.text())
        .collect::<Vec<_>>();
    eprintln!(
        "[CONNECTOR RESOLVE] class={} connection={} side={side:?} ref={} component={} subscripts={:?} available={available:?}",
        scene.class_qualified_name.as_deref().unwrap_or("<unknown>"),
        connection.id,
        reference.text(),
        reference.component_name,
        reference.subscripts,
    );
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
    let (first, last) = match endpoints.point_order {
        ConnectionPointOrder::LhsToRhs => {
            (endpoints.lhs.world_position, endpoints.rhs.world_position)
        }
        ConnectionPointOrder::RhsToLhs => {
            (endpoints.rhs.world_position, endpoints.lhs.world_position)
        }
    };
    Ok((
        world_to_line_local(line, first),
        world_to_line_local(line, last),
    ))
}

/// Re-anchor a routed polyline while preserving its interior route.
///
/// The first and last points are always derived from semantic connector
/// positions. Their adjacent points are adjusted only on the endpoint
/// segment's existing axis, so a component move, resize, or middle-segment
/// edit cannot turn a valid orthogonal route into a free-floating endpoint.
pub fn reanchor_connection_points(
    scene: &DiagramScene,
    connection: &DiagramConnection,
    points: &[Point],
) -> Result<Vec<Point>, ConnectorResolutionError> {
    if points.is_empty() {
        return Err(ConnectorResolutionError::MissingLine);
    }
    let endpoints = resolve_connection_endpoints(scene, connection)?;
    let line = connection
        .line
        .as_ref()
        .ok_or(ConnectorResolutionError::MissingLine)?;
    let (first_world, last_world) = match endpoints.point_order {
        ConnectionPointOrder::LhsToRhs => {
            (endpoints.lhs.world_position, endpoints.rhs.world_position)
        }
        ConnectionPointOrder::RhsToLhs => {
            (endpoints.rhs.world_position, endpoints.lhs.world_position)
        }
    };
    let first = world_to_line_local(line, first_world);
    let last = world_to_line_local(line, last_world);
    if points.len() == 2 {
        return Ok(reanchor_two_point_connection(points, first, last));
    }

    let mut result = points.to_vec();
    let lhs_before = result[0];
    result[0] = first;
    let rhs_index = result.len() - 1;
    let lhs_neighbor = points
        .get(1)
        .copied()
        .ok_or(ConnectorResolutionError::MissingLine)?;
    preserve_endpoint_axis(&mut result[1], lhs_before, lhs_neighbor, first);
    let rhs_before = result[rhs_index];
    result[rhs_index] = last;
    let rhs_neighbor = points
        .get(rhs_index - 1)
        .copied()
        .ok_or(ConnectorResolutionError::MissingLine)?;
    preserve_endpoint_axis(&mut result[rhs_index - 1], rhs_before, rhs_neighbor, last);
    Ok(result)
}

fn reanchor_two_point_connection(points: &[Point], lhs: Point, rhs: Point) -> Vec<Point> {
    debug_assert_eq!(points.len(), 2);
    if (lhs.x - rhs.x).abs() <= ORTHOGONAL_EPSILON || (lhs.y - rhs.y).abs() <= ORTHOGONAL_EPSILON {
        return vec![lhs, rhs];
    }

    let lhs_before = points[0];
    let rhs_before = points[1];
    let lhs_moved = distance(lhs_before, lhs) > ORTHOGONAL_EPSILON;
    let rhs_moved = distance(rhs_before, rhs) > ORTHOGONAL_EPSILON;
    let was_horizontal = (lhs_before.y - rhs_before.y).abs() <= ORTHOGONAL_EPSILON;

    let elbow = if was_horizontal {
        if rhs_moved && !lhs_moved {
            Point { x: lhs.x, y: rhs.y }
        } else {
            Point { x: rhs.x, y: lhs.y }
        }
    } else if rhs_moved && !lhs_moved {
        Point { x: rhs.x, y: lhs.y }
    } else {
        Point { x: lhs.x, y: rhs.y }
    };

    vec![lhs, elbow, rhs]
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

fn preserve_endpoint_axis(
    neighbor: &mut Point,
    before_endpoint: Point,
    before_neighbor: Point,
    after_endpoint: Point,
) {
    if (before_endpoint.y - before_neighbor.y).abs() <= 0.001 {
        neighbor.y = after_endpoint.y;
    } else if (before_endpoint.x - before_neighbor.x).abs() <= 0.001 {
        neighbor.x = after_endpoint.x;
    }
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
            dimensions: Vec::new(),
            resolved_type_qualified_name: Some("Example.Port".into()),
            model_text_context: modelica_core::ModelTextContext::default(),
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
                        dimensions: Vec::new(),
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
    fn vector_connector_anchors_are_created_from_dimensions_and_connections() {
        let mut ports = component(
            "ports-id",
            "ports",
            Some(ClassKind::Connector),
            point(0.0, 0.0),
            Extent {
                p1: point(-10.0, -10.0),
                p2: point(10.0, 10.0),
            },
            layer(
                "Example.FluidPorts_a",
                GraphicOwner {
                    qualified_name: "Example.FluidPorts_a".into(),
                    kind: GraphicOwnerKind::Own,
                    instance_name: None,
                    dimensions: Vec::new(),
                },
                Transform2D::identity(),
            ),
        );
        ports.dimensions = vec!["3".into()];
        let lhs = ConnectorRef::parse("ports[1]");
        let rhs = ConnectorRef::parse("ports[2]");
        let connection = DiagramConnection {
            key: modelica_core::scene::ConnectionKey::new(
                "Example.Top",
                lhs.clone(),
                rhs.clone(),
                0,
            ),
            id: "connection:ports[1]->ports[2]".into(),
            lhs,
            rhs,
            from: "ports[1]".into(),
            to: "ports[2]".into(),
            line: Some(LineGraphic {
                origin: point(0.0, 0.0),
                rotation: 0.0,
                points: vec![point(0.0, 0.0), point(0.0, 0.0)],
                color: [0, 0, 0],
                pattern: None,
                thickness: 1.0,
                arrow: Vec::new(),
                arrow_size: None,
                smooth: None,
            }),
            source_range: None,
            line_source_range: None,
        };
        let scene = scene(vec![ports], Some(connection));
        let anchors = connector_anchors(&scene);
        assert_eq!(anchors.len(), 3);
        assert!(find_connector_anchor(&anchors, &ConnectorRef::parse("ports[1]")).is_some());
        assert!(find_connector_anchor(&anchors, &ConnectorRef::parse("ports[2]")).is_some());
        assert!(find_connector_anchor(&anchors, &ConnectorRef::parse("ports[3]")).is_some());
        assert_eq!(anchors[0].key.owner_component_id, "ports-id");
        let endpoints = resolve_connection_endpoints(&scene, &scene.connections[0]).unwrap();
        assert_eq!(endpoints.lhs.connector_ref, ConnectorRef::parse("ports[1]"));
        assert_eq!(endpoints.rhs.connector_ref, ConnectorRef::parse("ports[2]"));
    }

    #[test]
    fn nested_vector_connector_anchors_keep_each_connect_reference() {
        let mixer = component(
            "mixer-id",
            "mixer",
            Some(ClassKind::Model),
            point(0.0, 0.0),
            Extent {
                p1: point(-100.0, -100.0),
                p2: point(100.0, 100.0),
            },
            layer(
                "Example.Mixer",
                GraphicOwner {
                    qualified_name: "Example.FluidPorts_a".into(),
                    kind: GraphicOwnerKind::Connector,
                    instance_name: Some("ports".into()),
                    dimensions: vec!["3".into()],
                },
                Transform2D::identity(),
            ),
        );
        let lhs = ConnectorRef::parse("mixer.ports[1]");
        let rhs = ConnectorRef::parse("mixer.ports[2]");
        let connection = DiagramConnection {
            key: modelica_core::scene::ConnectionKey::new(
                "Example.Top",
                lhs.clone(),
                rhs.clone(),
                0,
            ),
            id: "connection:mixer.ports[1]->mixer.ports[2]".into(),
            lhs,
            rhs,
            from: "mixer.ports[1]".into(),
            to: "mixer.ports[2]".into(),
            line: None,
            source_range: None,
            line_source_range: None,
        };
        let scene = scene(vec![mixer], Some(connection));
        let anchors = connector_anchors(&scene);
        assert_eq!(anchors.len(), 3);
        assert!(find_connector_anchor(&anchors, &ConnectorRef::parse("mixer.ports[1]")).is_some());
        assert!(find_connector_anchor(&anchors, &ConnectorRef::parse("mixer.ports[2]")).is_some());
        assert!(find_connector_anchor(&anchors, &ConnectorRef::parse("mixer.ports[3]")).is_some());
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
                        dimensions: Vec::new(),
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
        assert_eq!(anchors[0].connector_ref.connector_path, "bus.signal");
        assert_eq!(anchors[0].connector_ref.subscripts, vec!["1"]);
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
                        dimensions: Vec::new(),
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
                        dimensions: Vec::new(),
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
                    subscripts: Vec::new(),
                },
                ConnectorRef {
                    component_name: "b".into(),
                    connector_path: String::new(),
                    subscripts: Vec::new(),
                },
                0,
            ),
            id: "connection:0".into(),
            lhs: ConnectorRef {
                component_name: "a".into(),
                connector_path: String::new(),
                subscripts: Vec::new(),
            },
            rhs: ConnectorRef {
                component_name: "b".into(),
                connector_path: String::new(),
                subscripts: Vec::new(),
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
        let raw_reanchored = vec![point(4.0, 50.0), point(4.0, -50.0)];
        let reanchored =
            reanchor_connection_points(&scene, &scene.connections[0], &raw_reanchored).unwrap();
        assert!((reanchored[0].x - points.0.x).abs() < 0.001);
        assert!((reanchored[1].x - points.1.x).abs() < 0.001);

        // A two-point route has no interior neighbor. In particular, a raw
        // horizontal pair must not let the rhs-axis adjustment overwrite the
        // already fixed lhs semantic endpoint.
        let raw_two_point = vec![point(4.0, 50.0), point(100.0, 50.0)];
        let two_point =
            reanchor_connection_points(&scene, &scene.connections[0], &raw_two_point).unwrap();
        assert_eq!(two_point[0], points.0);
        assert_eq!(two_point[1], points.1);
    }

    #[test]
    fn reanchor_preserves_reversed_line_point_order_for_complex_routes() {
        let components = vec![
            component(
                "a-id",
                "a",
                Some(ClassKind::Connector),
                point(-40.0, 10.0),
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
                        dimensions: Vec::new(),
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
                        dimensions: Vec::new(),
                    },
                    Transform2D::identity(),
                ),
            ),
        ];
        let connection = DiagramConnection {
            key: modelica_core::scene::ConnectionKey::new(
                "Example.Top",
                ConnectorRef {
                    component_name: "a".into(),
                    connector_path: String::new(),
                    subscripts: Vec::new(),
                },
                ConnectorRef {
                    component_name: "b".into(),
                    connector_path: String::new(),
                    subscripts: Vec::new(),
                },
                0,
            ),
            id: "connection:reversed".into(),
            lhs: ConnectorRef {
                component_name: "a".into(),
                connector_path: String::new(),
                subscripts: Vec::new(),
            },
            rhs: ConnectorRef {
                component_name: "b".into(),
                connector_path: String::new(),
                subscripts: Vec::new(),
            },
            from: "a".into(),
            to: "b".into(),
            line: Some(LineGraphic {
                origin: point(0.0, 0.0),
                rotation: 0.0,
                points: vec![
                    point(50.0, 0.0),
                    point(50.0, 30.0),
                    point(-20.0, 30.0),
                    point(-20.0, 0.0),
                    point(-50.0, 0.0),
                ],
                color: [0, 0, 0],
                pattern: None,
                thickness: 1.0,
                arrow: Vec::new(),
                arrow_size: None,
                smooth: None,
            }),
            source_range: None,
            line_source_range: None,
        };
        let scene = scene(components, Some(connection));
        let endpoints = resolve_connection_endpoints(&scene, &scene.connections[0]).unwrap();
        assert_eq!(endpoints.point_order, ConnectionPointOrder::RhsToLhs);
        let strict = strict_connection_points(&scene, &scene.connections[0]).unwrap();
        assert_eq!(strict, (point(50.0, 0.0), point(-40.0, 10.0)));

        let reanchored = reanchor_connection_points(
            &scene,
            &scene.connections[0],
            &scene.connections[0].line.as_ref().unwrap().points,
        )
        .unwrap();
        assert_eq!(
            reanchored,
            vec![
                point(50.0, 0.0),
                point(50.0, 30.0),
                point(-20.0, 30.0),
                point(-20.0, 10.0),
                point(-40.0, 10.0),
            ]
        );
    }

    #[test]
    fn two_point_connection_adds_elbow_when_endpoint_moves_off_axis() {
        let points = vec![point(0.0, 0.0), point(100.0, 0.0)];
        let reanchored =
            reanchor_two_point_connection(&points, point(20.0, 30.0), point(100.0, 0.0));

        assert_eq!(
            reanchored,
            vec![point(20.0, 30.0), point(100.0, 30.0), point(100.0, 0.0),]
        );
    }

    #[test]
    fn two_point_connection_keeps_original_axis_near_stationary_endpoint() {
        let points = vec![point(0.0, 0.0), point(0.0, 100.0)];
        let reanchored =
            reanchor_two_point_connection(&points, point(20.0, 0.0), point(0.0, 120.0));

        assert_eq!(
            reanchored,
            vec![point(20.0, 0.0), point(20.0, 120.0), point(0.0, 120.0),]
        );
    }
}
