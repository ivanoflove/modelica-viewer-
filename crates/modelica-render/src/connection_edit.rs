use modelica_core::scene::{LineGraphic, Point};

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
    use super::{line_local_to_world, world_to_line_local};
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
