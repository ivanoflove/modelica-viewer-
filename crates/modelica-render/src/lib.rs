//! Backend-independent scene geometry, viewport transforms and hit testing.

pub mod connection_edit;
pub mod connector;
pub mod fill;
pub mod geometry;
pub mod hit_test;
pub mod line;
pub mod viewport;

pub use connection_edit::{line_local_to_world, world_to_line_local};
pub use connector::{
    ConnectionEndpointSide, ConnectorAnchor, ConnectorResolutionError, PortKey,
    ResolvedConnectionEndpoints, connector_anchor_hit_distance, connector_anchors,
    find_connector_anchor, hit_test_connector_anchor, nearest_connector_anchor,
    reanchor_connection_points, resolve_connection_endpoints, strict_connection_points,
};
pub use fill::{FillStyle, HatchPattern};
pub use geometry::{Bounds, ModelPoint, ScreenPoint, Vec2};
pub use hit_test::{
    graphic_contains_point, hit_test_graphics, hit_test_resolved_graphics,
    resolved_graphic_contains_point, transform_point,
};
pub use line::{
    ArrowGeometry, ArrowKind, LineEnd, PathSegment, build_modelica_line_path, line_arrow_geometry,
};
pub use viewport::{ModelBounds, Viewport};
