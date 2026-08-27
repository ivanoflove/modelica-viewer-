use modelica_core::scene::Point;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrowKind {
    None,
    Open,
    Filled,
    Half,
}

impl ArrowKind {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("Arrow.Open") => Self::Open,
            Some("Arrow.Filled") => Self::Filled,
            Some("Arrow.Half") => Self::Half,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnd {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrowGeometry {
    pub tip: Point,
    pub left: Point,
    pub right: Point,
    pub end: LineEnd,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathSegment {
    MoveTo(Point),
    LineTo(Point),
    CubicTo {
        control1: Point,
        control2: Point,
        end: Point,
    },
}

pub fn line_arrow_geometry(
    points: &[Point],
    arrow: ArrowKind,
    arrow_size: f32,
    end: LineEnd,
) -> Option<ArrowGeometry> {
    if points.len() < 2 || arrow == ArrowKind::None {
        return None;
    }
    let (tip, previous) = match end {
        LineEnd::Start => (points[0], points[1]),
        LineEnd::End => (points[points.len() - 1], points[points.len() - 2]),
    };
    let dx = tip.x - previous.x;
    let dy = tip.y - previous.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f32::EPSILON {
        return None;
    }
    let size = arrow_size.abs().max(f32::EPSILON);
    let ux = dx / length;
    let uy = dy / length;
    let base = Point {
        x: tip.x - ux * size,
        y: tip.y - uy * size,
    };
    let half_width = size * 0.55;
    Some(ArrowGeometry {
        tip,
        left: Point {
            x: base.x - uy * half_width,
            y: base.y + ux * half_width,
        },
        right: Point {
            x: base.x + uy * half_width,
            y: base.y - ux * half_width,
        },
        end,
    })
}

pub fn build_modelica_line_path(points: &[Point], smooth: Option<&str>) -> Vec<PathSegment> {
    if points.is_empty() {
        return Vec::new();
    }
    if smooth != Some("Smooth.Bezier") || points.len() < 3 {
        let mut path = Vec::with_capacity(points.len());
        path.push(PathSegment::MoveTo(points[0]));
        path.extend(points[1..].iter().copied().map(PathSegment::LineTo));
        return path;
    }

    let mut path = vec![PathSegment::MoveTo(points[0])];
    for index in 0..points.len() - 1 {
        let previous = if index == 0 {
            points[index]
        } else {
            points[index - 1]
        };
        let current = points[index];
        let next = points[index + 1];
        let following = if index + 2 < points.len() {
            points[index + 2]
        } else {
            next
        };
        path.push(PathSegment::CubicTo {
            control1: Point {
                x: current.x + (next.x - previous.x) / 6.0,
                y: current.y + (next.y - previous.y) / 6.0,
            },
            control2: Point {
                x: next.x - (following.x - current.x) / 6.0,
                y: next.y - (following.y - current.y) / 6.0,
            },
            end: next,
        });
    }
    path
}

#[cfg(test)]
mod tests {
    use modelica_core::scene::Point;

    use super::{ArrowKind, LineEnd, PathSegment, build_modelica_line_path, line_arrow_geometry};

    fn point(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    #[test]
    fn builds_arrows_at_both_line_ends() {
        let points = [point(0.0, 0.0), point(10.0, 0.0)];
        let start = line_arrow_geometry(&points, ArrowKind::Open, 2.0, LineEnd::Start);
        let end = line_arrow_geometry(&points, ArrowKind::Filled, 2.0, LineEnd::End);
        assert_eq!(start.expect("start arrow").tip, points[0]);
        assert_eq!(end.expect("end arrow").tip, points[1]);
    }

    #[test]
    fn bezier_path_preserves_endpoints() {
        let path = build_modelica_line_path(
            &[point(0.0, 0.0), point(5.0, 10.0), point(10.0, 0.0)],
            Some("Smooth.Bezier"),
        );
        assert!(matches!(path[0], PathSegment::MoveTo(value) if value == point(0.0, 0.0)));
        assert!(matches!(path[1], PathSegment::CubicTo { end, .. } if end == point(5.0, 10.0)));
        assert!(matches!(path[2], PathSegment::CubicTo { end, .. } if end == point(10.0, 0.0)));
    }
}
