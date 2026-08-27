//! Backend-independent scene geometry, viewport transforms and hit testing.

pub mod geometry;
pub mod hit_test;
pub mod viewport;

pub use geometry::{Bounds, ModelPoint, ScreenPoint, Vec2};
pub use hit_test::{graphic_contains_point, hit_test_graphics};
pub use viewport::{ModelBounds, Viewport};
