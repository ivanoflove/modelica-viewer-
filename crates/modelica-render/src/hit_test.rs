use modelica_core::scene::{Extent, Graphic, Point};

/// Conservative first-pass hit testing boundary. Detailed tolerance and
/// spatial indexing belong here, not in the GPUI widget layer.
pub fn graphic_contains_point(graphic: &Graphic, point: Point, tolerance: f32) -> bool {
    let (min_x, max_x, min_y, max_y) = match graphic {
        Graphic::Line(item) => bounds(item.points.iter().copied()),
        Graphic::Polygon(item) => bounds(item.points.iter().copied()),
        Graphic::Rectangle(item) => extent_bounds(item.extent),
        Graphic::Ellipse(item) => extent_bounds(item.extent),
        Graphic::Text(item) => extent_bounds(item.extent),
        Graphic::Bitmap(item) => extent_bounds(item.extent),
    };
    point.x >= min_x - tolerance
        && point.x <= max_x + tolerance
        && point.y >= min_y - tolerance
        && point.y <= max_y + tolerance
}

fn extent_bounds(extent: Extent) -> (f32, f32, f32, f32) {
    (
        extent.p1.x.min(extent.p2.x),
        extent.p1.x.max(extent.p2.x),
        extent.p1.y.min(extent.p2.y),
        extent.p1.y.max(extent.p2.y),
    )
}

fn bounds(points: impl Iterator<Item = Point>) -> (f32, f32, f32, f32) {
    let mut points = points;
    let Some(first) = points.next() else {
        return (0.0, 0.0, 0.0, 0.0);
    };
    points.fold(
        (first.x, first.x, first.y, first.y),
        |(min_x, max_x, min_y, max_y), point| {
            (
                min_x.min(point.x),
                max_x.max(point.x),
                min_y.min(point.y),
                max_y.max(point.y),
            )
        },
    )
}
