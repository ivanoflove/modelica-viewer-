use modelica_core::scene::{LineGraphic, Point};

/// Tolerance used by orthogonal connection editing and canonicalization.
pub const ORTHOGONAL_EPSILON: f32 = 0.001;

/// Remove redundant vertices from an orthogonal connection route.
///
/// Only contiguous vertices are considered: a pair of parallel segments that
/// is separated by a corner is never merged. The first and last input points
/// are restored if reduction would leave fewer than two points so callers can
/// still validate the semantic connection endpoints.
pub fn canonicalize_orthogonal_points(points: &[Point]) -> Vec<Point> {
    if points.len() < 2 {
        return points.to_vec();
    }

    let first = *points.first().expect("points has at least two entries");
    let last = *points.last().expect("points has at least two entries");
    let mut canonical = points.to_vec();

    loop {
        let mut changed = false;
        let mut without_duplicates = Vec::with_capacity(canonical.len());
        for point in canonical {
            if without_duplicates
                .last()
                .is_some_and(|previous| point_distance(*previous, point) <= ORTHOGONAL_EPSILON)
            {
                changed = true;
                continue;
            }
            without_duplicates.push(point);
        }

        let mut without_collinear = Vec::with_capacity(without_duplicates.len());
        for point in without_duplicates {
            without_collinear.push(point);
            while without_collinear.len() >= 3 {
                let len = without_collinear.len();
                let previous = without_collinear[len - 3];
                let current = without_collinear[len - 2];
                let next = without_collinear[len - 1];
                let horizontal = (previous.y - current.y).abs() <= ORTHOGONAL_EPSILON
                    && (current.y - next.y).abs() <= ORTHOGONAL_EPSILON;
                let vertical = (previous.x - current.x).abs() <= ORTHOGONAL_EPSILON
                    && (current.x - next.x).abs() <= ORTHOGONAL_EPSILON;
                if !horizontal && !vertical {
                    break;
                }
                without_collinear.remove(len - 2);
                changed = true;
            }
        }

        canonical = without_collinear;
        if !changed {
            break;
        }
    }

    if canonical.len() < 2 {
        vec![first, last]
    } else {
        canonical
    }
}

fn point_distance(first: Point, second: Point) -> f32 {
    ((first.x - second.x).powi(2) + (first.y - second.y).powi(2)).sqrt()
}

/// Convert a point stored in `Line.points` local coordinates into Diagram
/// coordinates using the Line annotation's own origin and rotation.
pub fn line_local_to_world(line: &LineGraphic, point: Point) -> Point {
    let angle = line.rotation.to_radians();
    let (sin, cos) = angle.sin_cos();
    Point {
        x: line.origin.x + point.x * cos - point.y * sin,
        y: line.origin.y + point.x * sin + point.y * cos,
    }
}

/// Convert a Diagram/world point back into the local coordinate system used
/// by `Line.points`. This is the exact inverse of [`line_local_to_world`].
pub fn world_to_line_local(line: &LineGraphic, point: Point) -> Point {
    let translated = Point {
        x: point.x - line.origin.x,
        y: point.y - line.origin.y,
    };
    let angle = (-line.rotation).to_radians();
    let (sin, cos) = angle.sin_cos();
    Point {
        x: translated.x * cos - translated.y * sin,
        y: translated.x * sin + translated.y * cos,
    }
}

#[cfg(test)]
mod tests {
    use super::{canonicalize_orthogonal_points, line_local_to_world, world_to_line_local};
    use modelica_core::scene::{LineGraphic, Point};

    fn line(rotation: f32) -> LineGraphic {
        LineGraphic {
            origin: Point { x: 20.0, y: 30.0 },
            rotation,
            points: Vec::new(),
            color: [0, 127, 255],
            pattern: None,
            thickness: 0.5,
            arrow: Vec::new(),
            arrow_size: None,
            smooth: None,
        }
    }

    fn point(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    #[test]
    fn canonicalization_removes_contiguous_duplicates() {
        assert_eq!(
            canonicalize_orthogonal_points(&[
                point(0.0, 0.0),
                point(20.0, 0.0),
                point(20.0, 0.0),
                point(40.0, 0.0)
            ]),
            vec![point(0.0, 0.0), point(40.0, 0.0)]
        );
    }

    #[test]
    fn canonicalization_merges_horizontal_collinear_segments() {
        assert_eq!(
            canonicalize_orthogonal_points(&[point(0.0, 0.0), point(20.0, 0.0), point(40.0, 0.0)]),
            vec![point(0.0, 0.0), point(40.0, 0.0)]
        );
    }

    #[test]
    fn canonicalization_merges_vertical_collinear_segments() {
        assert_eq!(
            canonicalize_orthogonal_points(&[point(0.0, 0.0), point(0.0, 20.0), point(0.0, 40.0)]),
            vec![point(0.0, 0.0), point(0.0, 40.0)]
        );
    }

    #[test]
    fn canonicalization_keeps_corners_and_parallel_segments_separate() {
        let points = [point(0.0, 0.0), point(20.0, 0.0), point(20.0, 40.0)];
        assert_eq!(canonicalize_orthogonal_points(&points), points);

        let parallel = [
            point(0.0, 0.0),
            point(40.0, 0.0),
            point(40.0, 20.0),
            point(80.0, 20.0),
        ];
        assert_eq!(canonicalize_orthogonal_points(&parallel), parallel);
    }

    #[test]
    fn canonicalization_collapses_u_shape_after_middle_horizontal_move() {
        let points = [
            point(0.0, 0.0),
            point(30.0, 0.0),
            point(30.0, 0.0),
            point(70.0, 0.0),
            point(70.0, 0.0),
            point(100.0, 0.0),
        ];
        assert_eq!(
            canonicalize_orthogonal_points(&points),
            vec![point(0.0, 0.0), point(100.0, 0.0)]
        );
    }

    #[test]
    fn canonicalization_keeps_two_endpoint_minimum() {
        let points = [point(10.0, 10.0), point(10.0, 10.0)];
        assert_eq!(canonicalize_orthogonal_points(&points), points);
    }

    #[test]
    fn rotated_line_round_trips_local_and_world_points() {
        let line = line(90.0);
        let local = Point { x: 20.0, y: 40.0 };
        let world = line_local_to_world(&line, local);
        assert!((world.x - -20.0).abs() < 1.0e-4);
        assert!((world.y - 50.0).abs() < 1.0e-4);
        let round_trip = world_to_line_local(&line, world);
        assert!((round_trip.x - local.x).abs() < 1.0e-4);
        assert!((round_trip.y - local.y).abs() < 1.0e-4);
    }
}
