//! Backend-independent scene geometry, viewport transforms and hit testing.

pub mod geometry;
pub mod hit_test;
pub mod viewport;

pub use geometry::{Bounds, ModelPoint, ScreenPoint, Vec2};
pub use hit_test::{
    graphic_contains_point, hit_test_graphics, hit_test_resolved_graphics,
    resolved_graphic_contains_point, transform_point,
};
pub use viewport::{ModelBounds, Viewport};
