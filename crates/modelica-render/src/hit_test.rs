use modelica_core::scene::{Extent, Graphic, Point};

/// Hit test in Modelica model coordinates. `tolerance` is also expressed in
/// Modelica units; callers should convert a fixed screen-space tolerance using
/// `Viewport::model_tolerance_for_screen_pixels` so selection does not become
/// harder when zoomed out.
pub fn graphic_contains_point(graphic: &Graphic, point: Point, tolerance: f32) -> bool {
    let local = inverse_graphic_transform(point, graphic_origin(graphic), graphic_rotation(graphic));
    let tolerance = tolerance.max(0.0);
    match graphic {
        Graphic::Line(item) => polyline_hit(&item.points, local, tolerance.max(item.thickness * 0.5)),
        Graphic::Polygon(item) => {
            point_in_polygon(&item.points, local)
                || polyline_hit_closed(&item.points, local, tolerance.max(item.line_thickness.unwrap_or(0.25) * 0.5))
        }
        Graphic::Rectangle(item) => extent_contains(item.extent, local, tolerance),
        Graphic::Ellipse(item) => ellipse_contains(item.extent, local, tolerance),
        Graphic::Text(item) => extent_contains(item.extent, local, tolerance),
        Graphic::Bitmap(item) => extent_contains(item.extent, local, tolerance),
    }
}

/// Return the top-most graphic index at the supplied Modelica point. Graphics
/// are painted in source order, therefore hit testing walks them in reverse.
pub fn hit_test_graphics(graphics: &[Graphic], point: Point, tolerance: f32) -> Option<usize> {
    graphics
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, graphic)| graphic_contains_point(graphic, point, tolerance).then_some(index))
}

fn graphic_origin(graphic: &Graphic) -> Point {
    match graphic {
        Graphic::Line(item) => item.origin,
        Graphic::Polygon(item) => item.origin,
        Graphic::Rectangle(item) => item.origin,
        Graphic::Ellipse(item) => item.origin,
        Graphic::Text(item) => item.origin,
        Graphic::Bitmap(item) => item.origin,
    }
}

fn graphic_rotation(graphic: &Graphic) -> f32 {
    match graphic {
        Graphic::Line(item) => item.rotation,
        Graphic::Polygon(item) => item.rotation,
        Graphic::Rectangle(item) => item.rotation,
        Graphic::Ellipse(item) => item.rotation,
        Graphic::Text(item) => item.rotation,
        Graphic::Bitmap(item) => item.rotation,
    }
}

fn inverse_graphic_transform(point: Point, origin: Point, rotation: f32) -> Point {
    let x = point.x - origin.x;
    let y = point.y - origin.y;
    let angle = (-rotation).to_radians();
    Point {
        x: x * angle.cos() - y * angle.sin(),
        y: x * angle.sin() + y * angle.cos(),
    }
}

fn extent_contains(extent: Extent, point: Point, tolerance: f32) -> bool {
    let (min_x, max_x, min_y, max_y) = extent_bounds(extent);
    point.x >= min_x - tolerance
        && point.x <= max_x + tolerance
        && point.y >= min_y - tolerance
        && point.y <= max_y + tolerance
}

fn ellipse_contains(extent: Extent, point: Point, tolerance: f32) -> bool {
    let (min_x, max_x, min_y, max_y) = extent_bounds(extent);
    let cx = (min_x + max_x) * 0.5;
    let cy = (min_y + max_y) * 0.5;
    let rx = ((max_x - min_x) * 0.5 + tolerance).max(1.0e-6);
    let ry = ((max_y - min_y) * 0.5 + tolerance).max(1.0e-6);
    let dx = (point.x - cx) / rx;
    let dy = (point.y - cy) / ry;
    dx * dx + dy * dy <= 1.0
}

fn point_in_polygon(points: &[Point], point: Point) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = points[points.len() - 1];
    for &current in points {
        let crosses = (current.y > point.y) != (previous.y > point.y);
        if crosses {
            let x = (previous.x - current.x) * (point.y - current.y)
                / (previous.y - current.y)
                + current.x;
            if point.x < x {
                inside = !inside;
            }
        }
        previous = current;
    }
    inside
}

fn polyline_hit(points: &[Point], point: Point, tolerance: f32) -> bool {
    points
        .windows(2)
        .any(|segment| distance_to_segment(point, segment[0], segment[1]) <= tolerance)
}

fn polyline_hit_closed(points: &[Point], point: Point, tolerance: f32) -> bool {
    polyline_hit(points, point, tolerance)
        || points
            .first()
            .zip(points.last())
            .is_some_and(|(&first, &last)| distance_to_segment(point, last, first) <= tolerance)
}

fn distance_to_segment(point: Point, a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_sq = dx * dx + dy * dy;
    if length_sq <= f32::EPSILON {
        return ((point.x - a.x).powi(2) + (point.y - a.y).powi(2)).sqrt();
    }
    let t = (((point.x - a.x) * dx + (point.y - a.y) * dy) / length_sq).clamp(0.0, 1.0);
    let closest_x = a.x + t * dx;
    let closest_y = a.y + t * dy;
    ((point.x - closest_x).powi(2) + (point.y - closest_y).powi(2)).sqrt()
}

fn extent_bounds(extent: Extent) -> (f32, f32, f32, f32) {
    (
        extent.p1.x.min(extent.p2.x),
        extent.p1.x.max(extent.p2.x),
        extent.p1.y.min(extent.p2.y),
        extent.p1.y.max(extent.p2.y),
    )
}

#[cfg(test)]
mod tests {
    use modelica_core::scene::{Extent, Graphic, LineGraphic, Point, RectangleGraphic};

    use super::{graphic_contains_point, hit_test_graphics};

    fn point(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    #[test]
    fn line_hit_uses_distance_not_only_bounding_box() {
        let line = Graphic::Line(LineGraphic {
            origin: point(0.0, 0.0),
            rotation: 0.0,
            points: vec![point(0.0, 0.0), point(100.0, 100.0)],
            color: [0, 0, 0],
            pattern: None,
            thickness: 1.0,
            arrow: Vec::new(),
            arrow_size: None,
            smooth: None,
        });
        assert!(graphic_contains_point(&line, point(50.0, 50.5), 1.0));
        assert!(!graphic_contains_point(&line, point(10.0, 90.0), 1.0));
    }

    #[test]
    fn origin_and_rotation_are_applied_before_hit_testing() {
        let rectangle = Graphic::Rectangle(RectangleGraphic {
            origin: point(10.0, 20.0),
            rotation: 90.0,
            extent: Extent {
                p1: point(-5.0, -2.0),
                p2: point(5.0, 2.0),
            },
            line_color: [0, 0, 0],
            fill_color: [255, 255, 255],
            line_pattern: None,
            line_thickness: None,
            fill_pattern: None,
            radius: None,
        });
        assert!(graphic_contains_point(&rectangle, point(10.0, 24.0), 0.1));
        assert!(!graphic_contains_point(&rectangle, point(15.0, 20.0), 0.1));
    }

    #[test]
    fn reverse_order_returns_topmost_graphic() {
        let make_rect = || {
            Graphic::Rectangle(RectangleGraphic {
                origin: point(0.0, 0.0),
                rotation: 0.0,
                extent: Extent {
                    p1: point(-10.0, -10.0),
                    p2: point(10.0, 10.0),
                },
                line_color: [0, 0, 0],
                fill_color: [255, 255, 255],
                line_pattern: None,
                line_thickness: None,
                fill_pattern: None,
                radius: None,
            })
        };
        assert_eq!(hit_test_graphics(&[make_rect(), make_rect()], point(0.0, 0.0), 1.0), Some(1));
    }
}
