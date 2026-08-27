//! Backend-independent scene geometry, viewport transforms and hit testing.

pub mod geometry;
pub mod hit_test;
pub mod viewport;

pub use geometry::{Bounds, ModelPoint, ScreenPoint, Vec2};
pub use viewport::Viewport;
