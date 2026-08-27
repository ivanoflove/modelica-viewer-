use crate::geometry::{Bounds, ModelPoint, ScreenPoint, Vec2};

const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl ModelBounds {
    pub fn width(self) -> f32 {
        (self.max_x - self.min_x).abs().max(1.0)
    }

    pub fn height(self) -> f32 {
        (self.max_y - self.min_y).abs().max(1.0)
    }

    pub fn center(self) -> Vec2 {
        Vec2 {
            x: (self.min_x + self.max_x) * 0.5,
            y: (self.min_y + self.max_y) * 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Screen pixels per Modelica unit.
    pub zoom: f32,
    /// Model-space translation applied before scaling. Keeping pan in model
    /// space makes viewport state independent from window size and DPI.
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

    /// Fit a Modelica rectangle into the viewport while preserving aspect
    /// ratio. `padding_fraction` is the fraction of each viewport dimension
    /// reserved around the fitted content (0.12 means 12% total padding).
    pub fn fit(model: ModelBounds, bounds: Bounds, padding_fraction: f32) -> Self {
        let padding = padding_fraction.clamp(0.0, 0.9);
        let available_width = (bounds.width * (1.0 - padding)).max(1.0);
        let available_height = (bounds.height * (1.0 - padding)).max(1.0);
        let zoom = (available_width / model.width())
            .min(available_height / model.height())
            .clamp(MIN_ZOOM, MAX_ZOOM);
        let center = model.center();
        Self {
            zoom,
            pan: Vec2 {
                x: -center.x,
                y: -center.y,
            },
        }
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// Zoom around a screen-space anchor. The Modelica point underneath the
    /// cursor remains stationary, matching the legacy Electron Icon viewer.
    pub fn zoom_at(&mut self, factor: f32, anchor: ScreenPoint, bounds: Bounds) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let before = self.screen_to_model(anchor, bounds);
        self.zoom_by(factor);
        let after = self.screen_to_model(anchor, bounds);
        self.pan.x += after.0.x - before.0.x;
        self.pan.y += after.0.y - before.0.y;
    }

    /// Pan by a mouse drag measured in screen pixels. Positive dx moves the
    /// drawing right; positive dy moves it down, while Modelica remains Y-up.
    pub fn pan_screen_delta(&mut self, dx: f32, dy: f32) {
        if self.zoom <= 0.0 {
            return;
        }
        self.pan.x += dx / self.zoom;
        self.pan.y -= dy / self.zoom;
    }

    pub fn model_tolerance_for_screen_pixels(self, pixels: f32) -> f32 {
        pixels.max(0.0) / self.zoom.max(MIN_ZOOM)
    }
}

#[cfg(test)]
mod tests {
    use super::{Bounds, ModelBounds, ModelPoint, ScreenPoint, Vec2, Viewport};

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1.0e-4, "{a} != {b}");
    }

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

    #[test]
    fn fit_centers_coordinate_system_and_preserves_aspect_ratio() {
        let viewport = Viewport::fit(
            ModelBounds {
                min_x: -100.0,
                min_y: -50.0,
                max_x: 100.0,
                max_y: 50.0,
            },
            Bounds {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 600.0,
            },
            0.12,
        );
        close(viewport.pan.x, 0.0);
        close(viewport.pan.y, 0.0);
        close(viewport.zoom, 4.4);
    }

    #[test]
    fn zoom_at_keeps_anchor_model_point_stationary() {
        let bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        let anchor = ScreenPoint(Vec2 { x: 612.0, y: 183.0 });
        let mut viewport = Viewport {
            zoom: 2.0,
            pan: Vec2 { x: 5.0, y: -7.0 },
        };
        let before = viewport.screen_to_model(anchor, bounds);
        viewport.zoom_at(1.5, anchor, bounds);
        let after = viewport.screen_to_model(anchor, bounds);
        close(before.0.x, after.0.x);
        close(before.0.y, after.0.y);
    }

    #[test]
    fn pixel_pan_is_zoom_independent_in_screen_space() {
        let bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        let model = ModelPoint(Vec2 { x: 0.0, y: 0.0 });
        let mut viewport = Viewport {
            zoom: 4.0,
            pan: Vec2::default(),
        };
        let before = viewport.model_to_screen(model, bounds);
        viewport.pan_screen_delta(20.0, 15.0);
        let after = viewport.model_to_screen(model, bounds);
        close(after.0.x - before.0.x, 20.0);
        close(after.0.y - before.0.y, 15.0);
    }
}
