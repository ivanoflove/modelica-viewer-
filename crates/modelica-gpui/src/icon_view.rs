use gpui::{
    Bounds, MouseButton, PathBuilder, Pixels, ScrollDelta, TransformationMatrix, Window, canvas,
    div, fill, point, prelude::*, px, radians, rgb, size,
};
use modelica_core::{Graphic, IconScene, ResolvedGraphic, Transform2D};
use modelica_render::{
    ArrowKind, Bounds as RenderBounds, LineEnd, ModelBounds, ModelPoint, PathSegment, ScreenPoint,
    Vec2, Viewport, build_modelica_line_path, hit_test_resolved_graphics, line_arrow_geometry,
    transform_point,
};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

const HIT_TOLERANCE_PX: f32 = 6.0;
const HANDLE_SIZE_PX: f32 = 7.0;
const HATCH_STEP_PX: f32 = 8.0;

#[derive(Clone, Debug)]
pub struct IconViewState {
    viewport: Viewport,
    fit_requested: bool,
    canvas_bounds: Option<RenderBounds>,
    selected_graphic: Option<usize>,
    pan_active: bool,
    last_mouse: Option<Vec2>,
}

impl Default for IconViewState {
    fn default() -> Self {
        Self {
            viewport: Viewport::default(),
            fit_requested: true,
            canvas_bounds: None,
            selected_graphic: None,
            pan_active: false,
            last_mouse: None,
        }
    }
}

impl IconViewState {
    pub fn reset_fit(&mut self) {
        self.fit_requested = true;
        self.selected_graphic = None;
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

    fn set_bounds_and_fit_if_needed(&mut self, scene: &IconScene, bounds: RenderBounds) {
        self.canvas_bounds = Some(bounds);
        if self.fit_requested {
            self.viewport = Viewport::fit_with_aspect(
                scene_bounds(scene),
                bounds,
                0.12,
                scene.coordinate_system.preserve_aspect_ratio,
            );
            self.fit_requested = false;
        }
    }

    fn zoom_at_screen(&mut self, factor: f32, x: f32, y: f32) {
        let Some(bounds) = self.canvas_bounds else {
            return;
        };
        self.fit_requested = false;
        self.viewport
            .zoom_at(factor, ScreenPoint(Vec2 { x, y }), bounds);
    }

    fn begin_pan(&mut self, x: f32, y: f32) {
        self.fit_requested = false;
        self.pan_active = true;
        self.last_mouse = Some(Vec2 { x, y });
    }

    fn update_pan(&mut self, x: f32, y: f32) {
        if !self.pan_active {
            return;
        }
        let next = Vec2 { x, y };
        if let Some(previous) = self.last_mouse {
            self.viewport
                .pan_screen_delta(next.x - previous.x, next.y - previous.y);
        }
        self.last_mouse = Some(next);
    }

    fn end_pan(&mut self) {
        self.pan_active = false;
        self.last_mouse = None;
    }

    fn select_at(&mut self, scene: &IconScene, x: f32, y: f32) {
        let Some(bounds) = self.canvas_bounds else {
            self.selected_graphic = None;
            return;
        };
        let model = self
            .viewport
            .screen_to_model(ScreenPoint(Vec2 { x, y }), bounds);
        let tolerance = self
            .viewport
            .model_tolerance_for_screen_pixels(HIT_TOLERANCE_PX);
        self.selected_graphic = hit_test_resolved_graphics(
            &scene.graphics,
            modelica_core::scene::Point {
                x: model.0.x,
                y: model.0.y,
            },
            tolerance,
        );
    }
}

pub type SharedIconViewState = Arc<Mutex<IconViewState>>;

pub fn new_icon_view_state() -> SharedIconViewState {
    Arc::new(Mutex::new(IconViewState::default()))
}

pub fn icon_canvas(scene: IconScene, state: SharedIconViewState) -> impl IntoElement {
    let paint_scene = scene.clone();
    let paint_state = state.clone();
    let wheel_state = state.clone();
    let middle_down_state = state.clone();
    let middle_up_state = state.clone();
    let move_state = state.clone();
    let left_state = state.clone();
    let left_scene = scene.clone();

    div()
        .id("icon-interaction-surface")
        .size_full()
        .relative()
        .overflow_hidden()
        .on_scroll_wheel(move |event, window, cx| {
            if event.modifiers.control || event.modifiers.platform {
                let delta = match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    ScrollDelta::Lines(delta) => delta.y * 20.0,
                };
                let factor = if delta > 0.0 {
                    1.0 / (1.0 + delta.abs() * 0.01)
                } else {
                    1.0 + delta.abs() * 0.01
                };
                if let Ok(mut state) = wheel_state.lock() {
                    state.zoom_at_screen(
                        factor,
                        f32::from(event.position.x),
                        f32::from(event.position.y),
                    );
                }
                cx.stop_propagation();
                window.refresh();
            }
        })
        .on_mouse_down(MouseButton::Middle, move |event, window, cx| {
            if let Ok(mut state) = middle_down_state.lock() {
                state.begin_pan(f32::from(event.position.x), f32::from(event.position.y));
            }
            cx.stop_propagation();
            window.refresh();
        })
        .on_mouse_up(MouseButton::Middle, move |_, window, cx| {
            if let Ok(mut state) = middle_up_state.lock() {
                state.end_pan();
            }
            cx.stop_propagation();
            window.refresh();
        })
        .on_mouse_move(move |event, window, _cx| {
            if let Ok(mut state) = move_state.lock()
                && state.pan_active
            {
                state.update_pan(f32::from(event.position.x), f32::from(event.position.y));
                window.refresh();
            }
        })
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if let Ok(mut state) = left_state.lock() {
                state.select_at(
                    &left_scene,
                    f32::from(event.position.x),
                    f32::from(event.position.y),
                );
            }
            cx.stop_propagation();
            window.refresh();
        })
        .child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, cx| {
                    paint_icon_scene(&paint_scene, &paint_state, bounds, window, cx);
                },
            )
            .size_full(),
        )
}

fn paint_icon_scene(
    scene: &IconScene,
    state: &SharedIconViewState,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let render_bounds = to_render_bounds(bounds);
    let (viewport, selected) = {
        let mut state = state.lock().expect("icon viewport mutex poisoned");
        state.set_bounds_and_fit_if_needed(scene, render_bounds);
        (state.viewport, state.selected_graphic)
    };

    let map = SceneMap {
        bounds: render_bounds,
        viewport,
    };

    for graphic in &scene.graphics {
        paint_graphic(graphic, &map, window, cx);
    }

    if let Some(index) = selected
        && let Some(graphic) = scene.graphics.get(index)
    {
        paint_selection_overlay(graphic, &map, window);
    }
}

fn paint_graphic(
    resolved: &ResolvedGraphic,
    map: &SceneMap,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let graphic = &resolved.graphic;
    let transform = resolved.transform;
    let stroke_scale = transform
        .scale_x
        .abs()
        .min(transform.scale_y.abs())
        .max(f32::EPSILON);
    match graphic {
        Graphic::Rectangle(graphic) => {
            let points = extent_points(graphic.extent).map(|point| {
                map.graphic_point_with_transform(point, graphic.origin, graphic.rotation, transform)
            });
            paint_closed_polygon(
                window,
                &points,
                graphic.fill_color,
                graphic.line_color,
                graphic.fill_pattern.as_deref(),
                graphic.line_pattern.as_deref(),
                graphic.line_thickness.unwrap_or(0.25) * map.viewport.zoom * stroke_scale,
            );
        }
        Graphic::Ellipse(graphic) => {
            let center_x = (graphic.extent.p1.x + graphic.extent.p2.x) * 0.5;
            let center_y = (graphic.extent.p1.y + graphic.extent.p2.y) * 0.5;
            let radius_x = (graphic.extent.p2.x - graphic.extent.p1.x).abs() * 0.5;
            let radius_y = (graphic.extent.p2.y - graphic.extent.p1.y).abs() * 0.5;
            let mut points = Vec::with_capacity(72);
            for index in 0..72 {
                let angle = std::f32::consts::TAU * index as f32 / 72.0;
                points.push(map.graphic_point_with_transform(
                    modelica_core::scene::Point {
                        x: center_x + radius_x * angle.cos(),
                        y: center_y + radius_y * angle.sin(),
                    },
                    graphic.origin,
                    graphic.rotation,
                    transform,
                ));
            }
            paint_closed_polygon(
                window,
                &points,
                graphic.fill_color,
                graphic.line_color,
                graphic.fill_pattern.as_deref(),
                graphic.line_pattern.as_deref(),
                graphic.line_thickness.unwrap_or(0.25) * map.viewport.zoom * stroke_scale,
            );
        }
        Graphic::Line(graphic) => {
            if is_none_pattern(graphic.pattern.as_deref()) {
                return;
            }
            let points = graphic
                .points
                .iter()
                .copied()
                .map(|point| {
                    map.graphic_point_with_transform(
                        point,
                        graphic.origin,
                        graphic.rotation,
                        transform,
                    )
                })
                .collect::<Vec<_>>();
            let stroke_width = graphic.thickness.max(0.25) * map.viewport.zoom * stroke_scale;
            if graphic.smooth.as_deref() == Some("Smooth.Bezier") {
                let path = build_modelica_line_path(&graphic.points, graphic.smooth.as_deref());
                paint_path_segments(window, &path, map, resolved, graphic.color, stroke_width);
            } else {
                paint_polyline_patterned(
                    window,
                    &points,
                    graphic.color,
                    stroke_width,
                    graphic.pattern.as_deref(),
                    false,
                );
            }
            paint_line_arrows(window, graphic, map, transform, stroke_width);
        }
        Graphic::Polygon(graphic) => {
            let points = graphic
                .points
                .iter()
                .copied()
                .map(|point| {
                    map.graphic_point_with_transform(
                        point,
                        graphic.origin,
                        graphic.rotation,
                        transform,
                    )
                })
                .collect::<Vec<_>>();
            paint_closed_polygon(
                window,
                &points,
                graphic.fill_color,
                graphic.line_color,
                graphic.fill_pattern.as_deref(),
                graphic.line_pattern.as_deref(),
                graphic.line_thickness.unwrap_or(0.25) * map.viewport.zoom * stroke_scale,
            );
        }
        Graphic::Bitmap(graphic) => {
            let points = extent_points(graphic.extent).map(|point| {
                map.graphic_point_with_transform(point, graphic.origin, graphic.rotation, transform)
            });
            paint_bitmap_placeholder(window, &points);
        }
        Graphic::Text(graphic) => paint_text(graphic, transform, map, window, cx),
    }
}

struct SceneMap {
    bounds: RenderBounds,
    viewport: Viewport,
}

impl SceneMap {
    fn graphic_point_with_transform(
        &self,
        local: modelica_core::scene::Point,
        origin: modelica_core::scene::Point,
        rotation: f32,
        transform: Transform2D,
    ) -> gpui::Point<Pixels> {
        let angle = rotation.to_radians();
        let rotated_x = local.x * angle.cos() - local.y * angle.sin();
        let rotated_y = local.x * angle.sin() + local.y * angle.cos();
        let model = transform_point(
            modelica_core::scene::Point {
                x: origin.x + rotated_x,
                y: origin.y + rotated_y,
            },
            transform,
        );
        let screen = self.viewport.model_to_screen(
            ModelPoint(Vec2 {
                x: model.x,
                y: model.y,
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
    fill_color: [u8; 3],
    stroke_color: [u8; 3],
    fill_pattern: Option<&str>,
    line_pattern: Option<&str>,
    stroke_width: f32,
) {
    if points.len() < 3 {
        return;
    }

    if !is_none_pattern(fill_pattern) {
        paint_solid_polygon(window, points, fill_color);
        match fill_pattern {
            Some("FillPattern.Horizontal") => paint_horizontal_hatch(window, points, stroke_color),
            Some("FillPattern.Vertical") => paint_vertical_hatch(window, points, stroke_color),
            Some("FillPattern.Cross") => {
                paint_horizontal_hatch(window, points, stroke_color);
                paint_vertical_hatch(window, points, stroke_color);
            }
            Some("FillPattern.Forward") => paint_diagonal_hatch(window, points, stroke_color, true),
            Some("FillPattern.Backward") => {
                paint_diagonal_hatch(window, points, stroke_color, false)
            }
            Some("FillPattern.CrossDiag") => {
                paint_diagonal_hatch(window, points, stroke_color, true);
                paint_diagonal_hatch(window, points, stroke_color, false);
            }
            Some("FillPattern.HorizontalCylinder") => {
                paint_horizontal_hatch(window, points, blend_color(stroke_color, fill_color, 0.55));
            }
            Some("FillPattern.VerticalCylinder") => {
                paint_vertical_hatch(window, points, blend_color(stroke_color, fill_color, 0.55));
            }
            Some("FillPattern.Sphere") => {
                paint_horizontal_hatch(window, points, blend_color(stroke_color, fill_color, 0.35));
                paint_vertical_hatch(window, points, blend_color(stroke_color, fill_color, 0.35));
            }
            Some("FillPattern.Solid") | None | Some(_) => {}
        }
    }

    if !is_none_pattern(line_pattern) {
        paint_polyline_patterned(
            window,
            points,
            stroke_color,
            stroke_width,
            line_pattern,
            true,
        );
    }
}

fn paint_solid_polygon(window: &mut Window, points: &[gpui::Point<Pixels>], fill_color: [u8; 3]) {
    let mut builder = PathBuilder::fill();
    builder.move_to(points[0]);
    for point in &points[1..] {
        builder.line_to(*point);
    }
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(rgb24(fill_color)));
    }
}

fn paint_polyline_patterned(
    window: &mut Window,
    points: &[gpui::Point<Pixels>],
    color: [u8; 3],
    stroke_width: f32,
    pattern: Option<&str>,
    closed: bool,
) {
    if points.len() < 2 || is_none_pattern(pattern) {
        return;
    }
    let dash = dash_pattern(pattern);
    if dash.is_none() {
        let mut builder = PathBuilder::stroke(px(stroke_width.max(0.5)));
        builder.move_to(points[0]);
        for point in &points[1..] {
            builder.line_to(*point);
        }
        if closed {
            builder.line_to(points[0]);
        }
        if let Ok(path) = builder.build() {
            window.paint_path(path, rgb(rgb24(color)));
        }
        return;
    }

    let mut all = points.to_vec();
    if closed {
        all.push(points[0]);
    }
    for segment in all.windows(2) {
        paint_dashed_segment(
            window,
            segment[0],
            segment[1],
            color,
            stroke_width,
            dash.expect("checked above"),
        );
    }
}

fn paint_path_segments(
    window: &mut Window,
    segments: &[PathSegment],
    map: &SceneMap,
    resolved: &ResolvedGraphic,
    color: [u8; 3],
    stroke_width: f32,
) {
    let (origin, rotation) = graphic_pose(&resolved.graphic);
    let transform = resolved.transform;
    let mut builder = PathBuilder::stroke(px(stroke_width.max(0.5)));
    for segment in segments {
        match *segment {
            PathSegment::MoveTo(point) => builder
                .move_to(map.graphic_point_with_transform(point, origin, rotation, transform)),
            PathSegment::LineTo(point) => builder
                .line_to(map.graphic_point_with_transform(point, origin, rotation, transform)),
            PathSegment::CubicTo {
                control1,
                control2,
                end,
            } => builder.cubic_bezier_to(
                map.graphic_point_with_transform(control1, origin, rotation, transform),
                map.graphic_point_with_transform(control2, origin, rotation, transform),
                map.graphic_point_with_transform(end, origin, rotation, transform),
            ),
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(rgb24(color)));
    }
}

fn graphic_pose(graphic: &Graphic) -> (modelica_core::scene::Point, f32) {
    match graphic {
        Graphic::Line(item) => (item.origin, item.rotation),
        Graphic::Polygon(item) => (item.origin, item.rotation),
        Graphic::Rectangle(item) => (item.origin, item.rotation),
        Graphic::Ellipse(item) => (item.origin, item.rotation),
        Graphic::Text(item) => (item.origin, item.rotation),
        Graphic::Bitmap(item) => (item.origin, item.rotation),
    }
}

fn paint_line_arrows(
    window: &mut Window,
    graphic: &modelica_core::scene::LineGraphic,
    map: &SceneMap,
    transform: Transform2D,
    stroke_width: f32,
) {
    let arrow_size = graphic.arrow_size.unwrap_or(10.0);
    for (index, end) in [(0, LineEnd::Start), (1, LineEnd::End)] {
        let kind = ArrowKind::parse(graphic.arrow.get(index).map(String::as_str));
        let Some(arrow) = line_arrow_geometry(&graphic.points, kind, arrow_size, end) else {
            continue;
        };
        let tip = map.graphic_point_with_transform(
            arrow.tip,
            graphic.origin,
            graphic.rotation,
            transform,
        );
        let left = map.graphic_point_with_transform(
            arrow.left,
            graphic.origin,
            graphic.rotation,
            transform,
        );
        let right = map.graphic_point_with_transform(
            arrow.right,
            graphic.origin,
            graphic.rotation,
            transform,
        );
        match kind {
            ArrowKind::Open => {
                paint_simple_segment(window, tip, left, graphic.color, stroke_width);
                paint_simple_segment(window, tip, right, graphic.color, stroke_width);
            }
            ArrowKind::Half => {
                paint_simple_segment(window, tip, left, graphic.color, stroke_width);
            }
            ArrowKind::Filled => {
                let mut builder = PathBuilder::fill();
                builder.move_to(tip);
                builder.line_to(left);
                builder.line_to(right);
                builder.close();
                if let Ok(path) = builder.build() {
                    window.paint_path(path, rgb(rgb24(graphic.color)));
                }
            }
            ArrowKind::None => {}
        }
    }
}

fn dash_pattern(pattern: Option<&str>) -> Option<&'static [f32]> {
    match pattern {
        Some("LinePattern.Dash") => Some(&[8.0, 4.0]),
        Some("LinePattern.Dot") => Some(&[2.0, 3.0]),
        Some("LinePattern.DashDot") => Some(&[8.0, 4.0, 2.0, 4.0]),
        Some("LinePattern.DashDotDot") => Some(&[8.0, 4.0, 2.0, 4.0, 2.0, 4.0]),
        _ => None,
    }
}

fn paint_dashed_segment(
    window: &mut Window,
    a: gpui::Point<Pixels>,
    b: gpui::Point<Pixels>,
    color: [u8; 3],
    stroke_width: f32,
    dash: &[f32],
) {
    let ax = f32::from(a.x);
    let ay = f32::from(a.y);
    let bx = f32::from(b.x);
    let by = f32::from(b.y);
    let dx = bx - ax;
    let dy = by - ay;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f32::EPSILON {
        return;
    }
    let ux = dx / length;
    let uy = dy / length;
    let mut cursor = 0.0;
    let mut index = 0usize;
    let mut draw = true;
    while cursor < length {
        let step = dash[index % dash.len()].max(0.5);
        let end = (cursor + step).min(length);
        if draw {
            let mut builder = PathBuilder::stroke(px(stroke_width.max(0.5)));
            builder.move_to(point(px(ax + ux * cursor), px(ay + uy * cursor)));
            builder.line_to(point(px(ax + ux * end), px(ay + uy * end)));
            if let Ok(path) = builder.build() {
                window.paint_path(path, rgb(rgb24(color)));
            }
        }
        draw = !draw;
        cursor = end;
        index += 1;
    }
}

fn paint_horizontal_hatch(window: &mut Window, polygon: &[gpui::Point<Pixels>], color: [u8; 3]) {
    let (min_y, max_y) = polygon.iter().fold((f32::MAX, f32::MIN), |acc, point| {
        (acc.0.min(f32::from(point.y)), acc.1.max(f32::from(point.y)))
    });
    let mut y = min_y;
    while y <= max_y {
        let mut intersections = polygon_scan_intersections(polygon, y, false);
        intersections.sort_by(|a, b| a.total_cmp(b));
        for pair in intersections.chunks_exact(2) {
            paint_simple_segment(
                window,
                point(px(pair[0]), px(y)),
                point(px(pair[1]), px(y)),
                color,
                1.0,
            );
        }
        y += HATCH_STEP_PX;
    }
}

fn paint_vertical_hatch(window: &mut Window, polygon: &[gpui::Point<Pixels>], color: [u8; 3]) {
    let swapped = polygon.iter().map(|p| point(p.y, p.x)).collect::<Vec<_>>();
    let (min_x, max_x) = polygon.iter().fold((f32::MAX, f32::MIN), |acc, point| {
        (acc.0.min(f32::from(point.x)), acc.1.max(f32::from(point.x)))
    });
    let mut x = min_x;
    while x <= max_x {
        let mut intersections = polygon_scan_intersections(&swapped, x, false);
        intersections.sort_by(|a, b| a.total_cmp(b));
        for pair in intersections.chunks_exact(2) {
            paint_simple_segment(
                window,
                point(px(x), px(pair[0])),
                point(px(x), px(pair[1])),
                color,
                1.0,
            );
        }
        x += HATCH_STEP_PX;
    }
}

fn paint_diagonal_hatch(
    window: &mut Window,
    polygon: &[gpui::Point<Pixels>],
    color: [u8; 3],
    forward: bool,
) {
    if polygon.is_empty() {
        return;
    }
    let values = polygon.iter().map(|point| {
        let x = f32::from(point.x);
        let y = f32::from(point.y);
        if forward { x + y } else { x - y }
    });
    let (min_value, max_value) = values.fold((f32::MAX, f32::MIN), |(min, max), value| {
        (min.min(value), max.max(value))
    });
    let mut scan = min_value;
    while scan <= max_value {
        let mut intersections = Vec::new();
        for index in 0..polygon.len() {
            let a = polygon[index];
            let b = polygon[(index + 1) % polygon.len()];
            let ax = f32::from(a.x);
            let ay = f32::from(a.y);
            let bx = f32::from(b.x);
            let by = f32::from(b.y);
            let a_value = if forward { ax + ay } else { ax - ay };
            let b_value = if forward { bx + by } else { bx - by };
            if (a_value > scan) == (b_value > scan) || (b_value - a_value).abs() <= f32::EPSILON {
                continue;
            }
            let t = (scan - a_value) / (b_value - a_value);
            intersections.push((ax + t * (bx - ax), ay + t * (by - ay)));
        }
        intersections.sort_by(|left, right| left.0.total_cmp(&right.0));
        for pair in intersections.chunks_exact(2) {
            paint_simple_segment(
                window,
                point(px(pair[0].0), px(pair[0].1)),
                point(px(pair[1].0), px(pair[1].1)),
                color,
                1.0,
            );
        }
        scan += HATCH_STEP_PX * std::f32::consts::SQRT_2;
    }
}

fn blend_color(first: [u8; 3], second: [u8; 3], first_weight: f32) -> [u8; 3] {
    let weight = first_weight.clamp(0.0, 1.0);
    std::array::from_fn(|index| {
        (f32::from(first[index]) * weight + f32::from(second[index]) * (1.0 - weight)) as u8
    })
}

fn polygon_scan_intersections(
    polygon: &[gpui::Point<Pixels>],
    scan: f32,
    _unused: bool,
) -> Vec<f32> {
    let mut intersections = Vec::new();
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let ay = f32::from(a.y);
        let by = f32::from(b.y);
        if (ay > scan) == (by > scan) || (by - ay).abs() <= f32::EPSILON {
            continue;
        }
        let t = (scan - ay) / (by - ay);
        intersections.push(f32::from(a.x) + t * (f32::from(b.x) - f32::from(a.x)));
    }
    intersections
}

fn paint_simple_segment(
    window: &mut Window,
    a: gpui::Point<Pixels>,
    b: gpui::Point<Pixels>,
    color: [u8; 3],
    stroke_width: f32,
) {
    let mut builder = PathBuilder::stroke(px(stroke_width.max(0.5)));
    builder.move_to(a);
    builder.line_to(b);
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(rgb24(color)));
    }
}

fn paint_text(
    graphic: &modelica_core::scene::TextGraphic,
    transform: Transform2D,
    map: &SceneMap,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    if graphic.text.is_empty() {
        return;
    }
    let width_model = (graphic.extent.p2.x - graphic.extent.p1.x).abs().max(0.1);
    let height_model = (graphic.extent.p2.y - graphic.extent.p1.y).abs().max(0.1);
    let chars = graphic.text.chars().count().max(1) as f32;
    let auto_font = (height_model * 0.82).min(width_model / (chars * 0.62).max(1.0));
    let font_model = graphic
        .font_size
        .filter(|size| *size > 0.0)
        .unwrap_or(auto_font);
    let center_local = modelica_core::scene::Point {
        x: (graphic.extent.p1.x + graphic.extent.p2.x) * 0.5,
        y: (graphic.extent.p1.y + graphic.extent.p2.y) * 0.5,
    };
    let center =
        map.graphic_point_with_transform(center_local, graphic.origin, graphic.rotation, transform);
    let screen_width = width_model * map.viewport.scale_x * transform.scale_x.abs();
    let screen_height = height_model * map.viewport.scale_y * transform.scale_y.abs();
    let bounds = Bounds::new(
        point(
            px(f32::from(center.x) - screen_width * 0.5),
            px(f32::from(center.y) - screen_height * 0.5),
        ),
        size(px(screen_width.max(1.0)), px(screen_height.max(1.0))),
    );
    let text_anchor = match graphic.horizontal_alignment.as_deref() {
        Some("TextAlignment.Left") => "start",
        Some("TextAlignment.Right") => "end",
        _ => "middle",
    };
    let anchor_x = match text_anchor {
        "start" => 0.0,
        "end" => width_model,
        _ => width_model * 0.5,
    };
    let font_weight = if graphic
        .text_style
        .iter()
        .any(|style| style.ends_with("Bold"))
    {
        "700"
    } else {
        "400"
    };
    let font_style = if graphic
        .text_style
        .iter()
        .any(|style| style.ends_with("Italic"))
    {
        "italic"
    } else {
        "normal"
    };
    let decoration = if graphic
        .text_style
        .iter()
        .any(|style| style.ends_with("UnderLine"))
    {
        "underline"
    } else {
        "none"
    };
    let family = graphic.font_name.as_deref().unwrap_or("sans-serif");
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width_model}" height="{height_model}" viewBox="0 0 {width_model} {height_model}"><text x="{anchor_x}" y="{y}" text-anchor="{text_anchor}" dominant-baseline="middle" font-family="{}" font-size="{font_model}" font-weight="{font_weight}" font-style="{font_style}" text-decoration="{decoration}">{}</text></svg>"#,
        escape_xml(family),
        escape_xml(&graphic.text),
        y = height_model * 0.5,
    );
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut hasher);
    let path = format!("__modelica_text_{:x}", hasher.finish());
    let rotation = (graphic.rotation + transform.rotation).to_radians();
    let _ = window.paint_svg(
        bounds,
        path.into(),
        Some(svg.as_bytes()),
        TransformationMatrix::unit().rotate(radians(rotation)),
        rgb(rgb24(graphic.color)).into(),
        cx,
    );
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn paint_selection_overlay(graphic: &ResolvedGraphic, map: &SceneMap, window: &mut Window) {
    let points = graphic_screen_points(graphic, map);
    if points.is_empty() {
        return;
    }
    let min_x = points
        .iter()
        .map(|p| f32::from(p.x))
        .fold(f32::MAX, f32::min);
    let max_x = points
        .iter()
        .map(|p| f32::from(p.x))
        .fold(f32::MIN, f32::max);
    let min_y = points
        .iter()
        .map(|p| f32::from(p.y))
        .fold(f32::MAX, f32::min);
    let max_y = points
        .iter()
        .map(|p| f32::from(p.y))
        .fold(f32::MIN, f32::max);

    let rect = [
        point(px(min_x), px(min_y)),
        point(px(max_x), px(min_y)),
        point(px(max_x), px(max_y)),
        point(px(min_x), px(max_y)),
    ];
    paint_polyline_patterned(
        window,
        &rect,
        [108, 92, 231],
        1.0,
        Some("LinePattern.Dash"),
        true,
    );

    let handles = [
        (min_x, min_y),
        ((min_x + max_x) * 0.5, min_y),
        (max_x, min_y),
        (max_x, (min_y + max_y) * 0.5),
        (max_x, max_y),
        ((min_x + max_x) * 0.5, max_y),
        (min_x, max_y),
        (min_x, (min_y + max_y) * 0.5),
    ];
    for (x, y) in handles {
        let half = HANDLE_SIZE_PX * 0.5;
        window.paint_quad(fill(
            Bounds::new(
                point(px(x - half), px(y - half)),
                size(px(HANDLE_SIZE_PX), px(HANDLE_SIZE_PX)),
            ),
            rgb(0x6c5ce7),
        ));
    }
}

fn graphic_screen_points(resolved: &ResolvedGraphic, map: &SceneMap) -> Vec<gpui::Point<Pixels>> {
    let graphic = &resolved.graphic;
    let transform = resolved.transform;
    match graphic {
        Graphic::Line(item) => item
            .points
            .iter()
            .copied()
            .map(|p| map.graphic_point_with_transform(p, item.origin, item.rotation, transform))
            .collect(),
        Graphic::Polygon(item) => item
            .points
            .iter()
            .copied()
            .map(|p| map.graphic_point_with_transform(p, item.origin, item.rotation, transform))
            .collect(),
        Graphic::Rectangle(item) => extent_points(item.extent)
            .map(|p| map.graphic_point_with_transform(p, item.origin, item.rotation, transform))
            .to_vec(),
        Graphic::Ellipse(item) => extent_points(item.extent)
            .map(|p| map.graphic_point_with_transform(p, item.origin, item.rotation, transform))
            .to_vec(),
        Graphic::Text(item) => extent_points(item.extent)
            .map(|p| map.graphic_point_with_transform(p, item.origin, item.rotation, transform))
            .to_vec(),
        Graphic::Bitmap(item) => extent_points(item.extent)
            .map(|p| map.graphic_point_with_transform(p, item.origin, item.rotation, transform))
            .to_vec(),
    }
}

fn paint_bitmap_placeholder(window: &mut Window, points: &[gpui::Point<Pixels>; 4]) {
    paint_polyline_patterned(
        window,
        points,
        [123, 135, 153],
        1.0,
        Some("LinePattern.Dash"),
        true,
    );
    paint_simple_segment(window, points[0], points[2], [123, 135, 153], 1.0);
    paint_simple_segment(window, points[1], points[3], [123, 135, 153], 1.0);
}

fn rgb24(color: [u8; 3]) -> u32 {
    (u32::from(color[0]) << 16) | (u32::from(color[1]) << 8) | u32::from(color[2])
}
