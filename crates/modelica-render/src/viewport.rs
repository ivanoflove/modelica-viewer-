use crate::geometry::{Bounds, ModelPoint, ScreenPoint, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub zoom: f32,
    pub pan: Vec2,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::default(),
        }
    }
}

impl Viewport {
    pub fn model_to_screen(self, point: ModelPoint, bounds: Bounds) -> ScreenPoint {
        ScreenPoint(Vec2 {
            x: bounds.x + bounds.width / 2.0 + (point.0.x + self.pan.x) * self.zoom,
            y: bounds.y + bounds.height / 2.0 - (point.0.y + self.pan.y) * self.zoom,
        })
    }

    pub fn screen_to_model(self, point: ScreenPoint, bounds: Bounds) -> ModelPoint {
        ModelPoint(Vec2 {
            x: (point.0.x - bounds.x - bounds.width / 2.0) / self.zoom - self.pan.x,
            y: -((point.0.y - bounds.y - bounds.height / 2.0) / self.zoom) - self.pan.y,
        })
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.05, 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::{Bounds, ModelPoint, Vec2, Viewport};

    #[test]
    fn model_and_screen_coordinates_round_trip() {
        let viewport = Viewport {
            zoom: 2.0,
            pan: Vec2 { x: 4.0, y: -3.0 },
        };
        let bounds = Bounds {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
        };
        let model = ModelPoint(Vec2 { x: 12.0, y: 8.0 });
        let screen = viewport.model_to_screen(model, bounds);
        assert_eq!(viewport.screen_to_model(screen, bounds), model);
    }
}
