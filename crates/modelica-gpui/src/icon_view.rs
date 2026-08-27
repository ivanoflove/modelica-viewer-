use gpui::{Bounds, PathBuilder, Pixels, Window, canvas, point, prelude::*, px, rgb};
use modelica_core::{Graphic, IconScene};
use modelica_render::{Bounds as RenderBounds, ModelBounds, ModelPoint, Vec2, Viewport};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct IconViewState {
    viewport: Viewport,
    fit_requested: bool,
}

impl Default for IconViewState {
    fn default() -> Self {
        Self {
            viewport: Viewport::default(),
            fit_requested: true,
        }
    }
}

impl IconViewState {
    pub fn reset_fit(&mut self) {
        self.fit_requested = true;
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.fit_requested = false;
        self.viewport.zoom_by(factor);
    }

    pub fn reset_100(&mut self) {
        self.fit_requested = false;
        self.viewport = Viewport::default();
    }

    pub fn zoom_percent(&self) -> i32 {
        (self.viewport.zoom * 100.0).round() as i32
    }
}

pub type SharedIconViewState = Arc<Mutex<IconViewState>>;

pub fn new_icon_view_state() -> SharedIconViewState {
    Arc::new(Mutex::new(IconViewState::default()))
}

pub fn icon_canvas(scene: IconScene, state: SharedIconViewState) -> impl IntoElement {
    canvas(
        move |_, _, _| {},
        move |bounds, _, window, _| {
            paint_icon_scene(&scene, &state, bounds, window);
        },
    )
    .size_full()
}

fn paint_icon_scene(
    scene: &IconScene,
    state: &SharedIconViewState,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let render_bounds = to_render_bounds(bounds);
    let viewport = {
        let mut state = state.lock().expect("icon viewport mutex poisoned");
        if state.fit_requested {
            state.viewport = Viewport::fit(scene_bounds(scene), render_bounds, 0.12);
            state.fit_requested = false;
        }
        state.viewport
    };

    let map = SceneMap {
        bounds: render_bounds,
        viewport,
    };

    for graphic in &scene.graphics {
        match graphic {
            Graphic::Rectangle(graphic) => {
                let points = extent_points(graphic.extent)
                    .map(|point| map.graphic_point(point, graphic.origin, graphic.rotation));
                paint_closed_polygon(
                    window,
                    &points,
                    graphic.fill_color,
                    graphic.line_color,
                    graphic.fill_pattern.as_deref(),
                    graphic.line_pattern.as_deref(),
                    graphic.line_thickness.unwrap_or(0.25) * viewport.zoom,
                );
            }
            Graphic::Ellipse(graphic) => {
                let center_x = (graphic.extent.p1.x + graphic.extent.p2.x) * 0.5;
                let center_y = (graphic.extent.p1.y + graphic.extent.p2.y) * 0.5;
                let radius_x = (graphic.extent.p2.x - graphic.extent.p1.x).abs() * 0.5;
                let radius_y = (graphic.extent.p2.y - graphic.extent.p1.y).abs() * 0.5;
                let mut points = Vec::with_capacity(64);
                for index in 0..64 {
                    let angle = std::f32::consts::TAU * index as f32 / 64.0;
                    points.push(map.graphic_point(
                        modelica_core::scene::Point {
                            x: center_x + radius_x * angle.cos(),
                            y: center_y + radius_y * angle.sin(),
                        },
                        graphic.origin,
                        graphic.rotation,
                    ));
                }
                paint_closed_polygon(
                    window,
                    &points,
                    graphic.fill_color,
                    graphic.line_color,
                    graphic.fill_pattern.as_deref(),
                    graphic.line_pattern.as_deref(),
                    graphic.line_thickness.unwrap_or(0.25) * viewport.zoom,
                );
            }
            Graphic::Line(graphic) => {
                if is_none_pattern(graphic.pattern.as_deref()) {
                    continue;
                }
                let points = graphic
                    .points
                    .iter()
                    .copied()
                    .map(|point| map.graphic_point(point, graphic.origin, graphic.rotation))
                    .collect::<Vec<_>>();
                paint_polyline(
                    window,
                    &points,
                    graphic.color,
                    graphic.thickness.max(0.25) * viewport.zoom,
                );
            }
            Graphic::Polygon(graphic) => {
                let points = graphic
                    .points
                    .iter()
                    .copied()
                    .map(|point| map.graphic_point(point, graphic.origin, graphic.rotation))
                    .collect::<Vec<_>>();
                paint_closed_polygon(
                    window,
                    &points,
                    graphic.fill_color,
                    graphic.line_color,
                    graphic.fill_pattern.as_deref(),
                    graphic.line_pattern.as_deref(),
                    graphic.line_thickness.unwrap_or(0.25) * viewport.zoom,
                );
            }
            Graphic::Bitmap(graphic) => {
                let points = extent_points(graphic.extent)
                    .map(|point| map.graphic_point(point, graphic.origin, graphic.rotation));
                paint_bitmap_placeholder(window, &points);
            }
            Graphic::Text(_) => {
                // Text uses GPUI shaped-text painting in the next parity step.
                // Keeping it in the scene prevents semantic data loss.
            }
        }
    }
}

struct SceneMap {
    bounds: RenderBounds,
    viewport: Viewport,
}

impl SceneMap {
    fn graphic_point(
        &self,
        local: modelica_core::scene::Point,
        origin: modelica_core::scene::Point,
        rotation: f32,
    ) -> gpui::Point<Pixels> {
        let angle = rotation.to_radians();
        let rotated_x = local.x * angle.cos() - local.y * angle.sin();
        let rotated_y = local.x * angle.sin() + local.y * angle.cos();
        let screen = self.viewport.model_to_screen(
            ModelPoint(Vec2 {
                x: origin.x + rotated_x,
                y: origin.y + rotated_y,
            }),
            self.bounds,
        );
        point(px(screen.0.x), px(screen.0.y))
    }
}

fn scene_bounds(scene: &IconScene) -> ModelBounds {
    let extent = scene.coordinate_system.extent;
    ModelBounds {
        min_x: extent.p1.x.min(extent.p2.x),
        min_y: extent.p1.y.min(extent.p2.y),
        max_x: extent.p1.x.max(extent.p2.x),
        max_y: extent.p1.y.max(extent.p2.y),
    }
}

fn to_render_bounds(bounds: Bounds<Pixels>) -> RenderBounds {
    RenderBounds {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
    }
}

fn extent_points(extent: modelica_core::scene::Extent) -> [modelica_core::scene::Point; 4] {
    [
        extent.p1,
        modelica_core::scene::Point {
            x: extent.p2.x,
            y: extent.p1.y,
        },
        extent.p2,
        modelica_core::scene::Point {
            x: extent.p1.x,
            y: extent.p2.y,
        },
    ]
}

fn is_none_pattern(pattern: Option<&str>) -> bool {
    matches!(pattern, Some("LinePattern.None") | Some("FillPattern.None"))
}

fn paint_closed_polygon(
    window: &mut Window,
    points: &[gpui::Point<Pixels>],
    fill: [u8; 3],
    stroke: [u8; 3],
    fill_pattern: Option<&str>,
    line_pattern: Option<&str>,
    stroke_width: f32,
) {
    if points.len() < 3 {
        return;
    }

    if !is_none_pattern(fill_pattern) {
        let mut fill_builder = PathBuilder::fill();
        fill_builder.move_to(points[0]);
        for point in &points[1..] {
            fill_builder.line_to(*point);
        }
        fill_builder.close();
        if let Ok(path) = fill_builder.build() {
            window.paint_path(path, rgb(rgb24(fill)));
        }
    }

    if !is_none_pattern(line_pattern) {
        let mut stroke_builder = PathBuilder::stroke(px(stroke_width.max(0.5)));
        stroke_builder.move_to(points[0]);
        for point in &points[1..] {
            stroke_builder.line_to(*point);
        }
        stroke_builder.line_to(points[0]);
        if let Ok(path) = stroke_builder.build() {
            window.paint_path(path, rgb(rgb24(stroke)));
        }
    }
}

fn paint_polyline(
    window: &mut Window,
    points: &[gpui::Point<Pixels>],
    color: [u8; 3],
    stroke_width: f32,
) {
    if points.len() < 2 {
        return;
    }
    let mut builder = PathBuilder::stroke(px(stroke_width.max(0.5)));
    builder.move_to(points[0]);
    for point in &points[1..] {
        builder.line_to(*point);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(rgb24(color)));
    }
}

fn paint_bitmap_placeholder(window: &mut Window, points: &[gpui::Point<Pixels>; 4]) {
    let mut border = PathBuilder::stroke(px(1.0));
    border.move_to(points[0]);
    border.line_to(points[1]);
    border.line_to(points[2]);
    border.line_to(points[3]);
    border.line_to(points[0]);
    if let Ok(path) = border.build() {
        window.paint_path(path, rgb(0x7b8799));
    }

    let mut cross = PathBuilder::stroke(px(1.0));
    cross.move_to(points[0]);
    cross.line_to(points[2]);
    cross.move_to(points[1]);
    cross.line_to(points[3]);
    if let Ok(path) = cross.build() {
        window.paint_path(path, rgb(0x7b8799));
    }
}

fn rgb24(color: [u8; 3]) -> u32 {
    (u32::from(color[0]) << 16) | (u32::from(color[1]) << 8) | u32::from(color[2])
}
