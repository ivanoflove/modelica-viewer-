use crate::annotation::{AnnotationCall, AnnotationValue};
use crate::diagnostics::Diagnostic;
use crate::scene::{
    BitmapGraphic, CoordinateSystem, EllipseGraphic, Extent, Graphic, GraphicId, GraphicOwner,
    GraphicOwnerKind, IconScene, LineGraphic, Point, PolygonGraphic, RectangleGraphic,
    ResolvedGraphic, TextGraphic, Transform2D,
};

const DEFAULT_EXTENT: Extent = Extent {
    p1: Point {
        x: -100.0,
        y: -100.0,
    },
    p2: Point { x: 100.0, y: 100.0 },
};
const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

pub fn resolve_icon_call(icon: &AnnotationCall) -> IconScene {
    let mut diagnostics = Vec::new();
    let coordinate_system = resolve_coordinate_system(icon);
    let graphics = match graphics_value(icon) {
        None => Vec::new(),
        Some(value) => match value.as_array() {
            None => {
                diagnostics.push(Diagnostic::warning(
                    "ICON_GRAPHICS_NOT_ARRAY",
                    "Icon.graphics must be an array",
                ));
                Vec::new()
            }
            Some(items) => items
                .iter()
                .filter_map(|item| resolve_graphic(item, &mut diagnostics))
                .collect(),
        },
    };
    let graphics = graphics
        .into_iter()
        .enumerate()
        .map(|(index, graphic)| ResolvedGraphic {
            id: GraphicId(format!("<unresolved>:Icon.graphics:{index}")),
            graphic,
            owner: GraphicOwner {
                qualified_name: "<unresolved>".into(),
                kind: GraphicOwnerKind::Own,
                instance_name: None,
            },
            transform: Transform2D::identity(),
            editable: true,
        })
        .collect();
    IconScene {
        owner_qualified_name: None,
        coordinate_system,
        graphics,
        diagnostics,
    }
}

fn resolve_coordinate_system(icon: &AnnotationCall) -> CoordinateSystem {
    let coordinate_call = icon
        .named("coordinateSystem")
        .and_then(AnnotationValue::as_call)
        .or_else(|| {
            icon.args
                .iter()
                .filter(|entry| entry.name.is_none())
                .find_map(|entry| {
                    entry
                        .value
                        .as_call()
                        .filter(|call| call.name == "coordinateSystem")
                })
        });
    let extent = coordinate_call
        .and_then(|call| call.named("extent"))
        .and_then(parse_extent)
        .or_else(|| icon.named("extent").and_then(parse_extent))
        .unwrap_or(DEFAULT_EXTENT);
    let preserve_aspect_ratio = coordinate_call
        .and_then(|call| call.named("preserveAspectRatio"))
        .and_then(parse_bool)
        .unwrap_or(true);
    let grid = coordinate_call
        .and_then(|call| call.named("grid"))
        .and_then(parse_point);
    let initial_scale = coordinate_call
        .and_then(|call| call.named("initialScale"))
        .and_then(parse_number);
    CoordinateSystem {
        extent,
        preserve_aspect_ratio,
        grid,
        initial_scale,
    }
}

fn graphics_value(icon: &AnnotationCall) -> Option<&AnnotationValue> {
    icon.named("graphics").or_else(|| {
        icon.args
            .iter()
            .find(|entry| entry.name.is_none())
            .and_then(|entry| entry.value.as_array().map(|_| &entry.value))
    })
}

fn resolve_graphic(value: &AnnotationValue, diagnostics: &mut Vec<Diagnostic>) -> Option<Graphic> {
    let call = value.as_call()?;
    match call.name.as_str() {
        "Rectangle" => parse_rectangle(call, diagnostics).map(Graphic::Rectangle),
        "Ellipse" => parse_ellipse(call, diagnostics).map(Graphic::Ellipse),
        "Line" => parse_line(call, diagnostics).map(Graphic::Line),
        "Polygon" => parse_polygon(call, diagnostics).map(Graphic::Polygon),
        "Text" => parse_text(call, diagnostics).map(Graphic::Text),
        "Bitmap" => parse_bitmap(call, diagnostics).map(Graphic::Bitmap),
        other => {
            diagnostics.push(Diagnostic::warning(
                "ICON_UNSUPPORTED_GRAPHIC",
                format!("unsupported Icon graphic `{other}`"),
            ));
            None
        }
    }
}

fn parse_rectangle(
    call: &AnnotationCall,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<RectangleGraphic> {
    unknown_properties(
        call,
        &[
            "origin",
            "extent",
            "rotation",
            "lineColor",
            "fillColor",
            "pattern",
            "linePattern",
            "fillPattern",
            "lineThickness",
            "radius",
        ],
        diagnostics,
    );
    Some(RectangleGraphic {
        origin: parse_origin(call),
        rotation: parse_number_arg(call, "rotation").unwrap_or(0.0),
        extent: required_extent(call, diagnostics)?,
        line_color: parse_color_arg(call, "lineColor").unwrap_or(BLACK),
        fill_color: parse_color_arg(call, "fillColor").unwrap_or(WHITE),
        line_pattern: parse_name_arg(call, "pattern")
            .or_else(|| parse_name_arg(call, "linePattern")),
        line_thickness: parse_number_arg(call, "lineThickness"),
        fill_pattern: parse_name_arg(call, "fillPattern"),
        radius: parse_number_arg(call, "radius"),
    })
}

fn parse_ellipse(
    call: &AnnotationCall,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EllipseGraphic> {
    unknown_properties(
        call,
        &[
            "origin",
            "extent",
            "rotation",
            "lineColor",
            "fillColor",
            "pattern",
            "linePattern",
            "fillPattern",
            "lineThickness",
            "startAngle",
            "endAngle",
        ],
        diagnostics,
    );
    Some(EllipseGraphic {
        origin: parse_origin(call),
        rotation: parse_number_arg(call, "rotation").unwrap_or(0.0),
        extent: required_extent(call, diagnostics)?,
        line_color: parse_color_arg(call, "lineColor").unwrap_or(BLACK),
        fill_color: parse_color_arg(call, "fillColor").unwrap_or(WHITE),
        line_pattern: parse_name_arg(call, "pattern")
            .or_else(|| parse_name_arg(call, "linePattern")),
        line_thickness: parse_number_arg(call, "lineThickness"),
        fill_pattern: parse_name_arg(call, "fillPattern"),
        start_angle: parse_number_arg(call, "startAngle"),
        end_angle: parse_number_arg(call, "endAngle"),
    })
}

fn parse_line(call: &AnnotationCall, diagnostics: &mut Vec<Diagnostic>) -> Option<LineGraphic> {
    unknown_properties(
        call,
        &[
            "origin",
            "points",
            "rotation",
            "color",
            "lineColor",
            "pattern",
            "thickness",
            "arrow",
            "arrowSize",
            "smooth",
        ],
        diagnostics,
    );
    let points = call.named("points").and_then(parse_points);
    if points.as_ref().is_none_or(|points| points.len() < 2) {
        diagnostics.push(Diagnostic::warning(
            "ICON_LINE_POINTS",
            "Line.points must contain at least two points",
        ));
        return None;
    }
    Some(LineGraphic {
        origin: parse_origin(call),
        rotation: parse_number_arg(call, "rotation").unwrap_or(0.0),
        points: points?,
        color: parse_color_arg(call, "color")
            .or_else(|| parse_color_arg(call, "lineColor"))
            .unwrap_or(BLACK),
        pattern: parse_name_arg(call, "pattern"),
        thickness: parse_number_arg(call, "thickness")
            .or_else(|| parse_number_arg(call, "lineThickness"))
            .unwrap_or(0.25),
        arrow: call
            .named("arrow")
            .and_then(parse_names)
            .unwrap_or_default(),
        arrow_size: parse_number_arg(call, "arrowSize"),
        smooth: parse_name_arg(call, "smooth"),
    })
}

fn parse_polygon(
    call: &AnnotationCall,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<PolygonGraphic> {
    unknown_properties(
        call,
        &[
            "origin",
            "points",
            "rotation",
            "lineColor",
            "fillColor",
            "pattern",
            "linePattern",
            "fillPattern",
            "lineThickness",
            "smooth",
        ],
        diagnostics,
    );
    let points = call.named("points").and_then(parse_points);
    if points.as_ref().is_none_or(|points| points.len() < 2) {
        diagnostics.push(Diagnostic::warning(
            "ICON_POLYGON_POINTS",
            "Polygon.points must contain at least two points",
        ));
        return None;
    }
    Some(PolygonGraphic {
        origin: parse_origin(call),
        rotation: parse_number_arg(call, "rotation").unwrap_or(0.0),
        points: points?,
        line_color: parse_color_arg(call, "lineColor").unwrap_or(BLACK),
        fill_color: parse_color_arg(call, "fillColor").unwrap_or(WHITE),
        line_pattern: parse_name_arg(call, "pattern")
            .or_else(|| parse_name_arg(call, "linePattern")),
        line_thickness: parse_number_arg(call, "lineThickness"),
        fill_pattern: parse_name_arg(call, "fillPattern"),
        smooth: parse_name_arg(call, "smooth"),
    })
}

fn parse_text(call: &AnnotationCall, diagnostics: &mut Vec<Diagnostic>) -> Option<TextGraphic> {
    unknown_properties(
        call,
        &[
            "origin",
            "extent",
            "rotation",
            "textString",
            "fontSize",
            "fontName",
            "textColor",
            "lineColor",
            "horizontalAlignment",
            "textStyle",
        ],
        diagnostics,
    );
    let extent = required_extent(call, diagnostics)?;
    let text = call
        .named("textString")
        .and_then(parse_string)
        .unwrap_or_default();
    Some(TextGraphic {
        origin: parse_origin(call),
        rotation: parse_number_arg(call, "rotation").unwrap_or(0.0),
        extent,
        text,
        color: parse_color_arg(call, "textColor")
            .or_else(|| parse_color_arg(call, "lineColor"))
            .unwrap_or(BLACK),
        font_size: parse_number_arg(call, "fontSize"),
        font_name: call.named("fontName").and_then(parse_string),
        horizontal_alignment: parse_name_arg(call, "horizontalAlignment")
            .or_else(|| parse_name_arg(call, "textAlignment")),
        text_style: call
            .named("textStyle")
            .and_then(parse_names)
            .unwrap_or_default(),
    })
}

fn parse_bitmap(call: &AnnotationCall, diagnostics: &mut Vec<Diagnostic>) -> Option<BitmapGraphic> {
    unknown_properties(
        call,
        &["origin", "extent", "rotation", "fileName", "imageSource"],
        diagnostics,
    );
    Some(BitmapGraphic {
        origin: parse_origin(call),
        rotation: parse_number_arg(call, "rotation").unwrap_or(0.0),
        extent: required_extent(call, diagnostics)?,
        file_name: call.named("fileName").and_then(parse_string),
        image_source: call.named("imageSource").and_then(parse_string),
    })
}

fn required_extent(call: &AnnotationCall, diagnostics: &mut Vec<Diagnostic>) -> Option<Extent> {
    let extent = call.named("extent").and_then(parse_extent);
    if extent.is_none() {
        diagnostics.push(Diagnostic::warning(
            "ICON_EXTENT_MISSING",
            format!("{} requires extent", call.name),
        ));
    }
    extent
}

fn parse_origin(call: &AnnotationCall) -> Point {
    call.named("origin")
        .and_then(parse_point)
        .unwrap_or(Point { x: 0.0, y: 0.0 })
}
fn parse_number_arg(call: &AnnotationCall, name: &str) -> Option<f32> {
    call.named(name).and_then(parse_number)
}
fn parse_name_arg(call: &AnnotationCall, name: &str) -> Option<String> {
    call.named(name).and_then(parse_name)
}

fn parse_number(value: &AnnotationValue) -> Option<f32> {
    if let AnnotationValue::Number(value) = value {
        Some(*value as f32)
    } else {
        None
    }
}
fn parse_string(value: &AnnotationValue) -> Option<String> {
    if let AnnotationValue::String(value) = value {
        Some(value.clone())
    } else {
        None
    }
}
fn parse_bool(value: &AnnotationValue) -> Option<bool> {
    if let AnnotationValue::Bool(value) = value {
        Some(*value)
    } else if let AnnotationValue::Name(value) = value {
        match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    } else {
        None
    }
}
fn parse_name(value: &AnnotationValue) -> Option<String> {
    if let AnnotationValue::Name(value) = value {
        Some(value.clone())
    } else {
        None
    }
}

fn parse_point(value: &AnnotationValue) -> Option<Point> {
    let values = value.as_array()?;
    if values.len() != 2 {
        return None;
    }
    Some(Point {
        x: parse_number(&values[0])?,
        y: parse_number(&values[1])?,
    })
}

fn parse_extent(value: &AnnotationValue) -> Option<Extent> {
    let values = value.as_array()?;
    if values.len() != 2 {
        return None;
    }
    Some(Extent {
        p1: parse_point(&values[0])?,
        p2: parse_point(&values[1])?,
    })
}

fn parse_points(value: &AnnotationValue) -> Option<Vec<Point>> {
    Some(value.as_array()?.iter().filter_map(parse_point).collect())
}

fn parse_color_arg(call: &AnnotationCall, name: &str) -> Option<[u8; 3]> {
    let values = call.named(name)?.as_array()?;
    if values.len() != 3 {
        return None;
    }
    Some([
        parse_number(&values[0])?.round().clamp(0.0, 255.0) as u8,
        parse_number(&values[1])?.round().clamp(0.0, 255.0) as u8,
        parse_number(&values[2])?.round().clamp(0.0, 255.0) as u8,
    ])
}

fn parse_names(value: &AnnotationValue) -> Option<Vec<String>> {
    Some(value.as_array()?.iter().filter_map(parse_name).collect())
}

fn unknown_properties(call: &AnnotationCall, known: &[&str], diagnostics: &mut Vec<Diagnostic>) {
    for entry in &call.args {
        if let Some(name) = entry.name.as_deref()
            && !known.contains(&name)
        {
            diagnostics.push(Diagnostic::warning(
                "ICON_UNKNOWN_PROPERTY",
                format!("ignored {}.{}", call.name, name),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_icon_call;
    use crate::annotation::parse_annotation;
    use crate::scene::Graphic;

    #[test]
    fn resolves_all_phase_one_primitives_and_coordinate_system() {
        let source = r#"annotation(Icon(coordinateSystem(extent={{-200,-100},{200,100}}, preserveAspectRatio=false, grid={10,10}, initialScale=0.5), graphics={Rectangle(extent={{-10,-20},{10,20}}, fillPattern=FillPattern.Solid, radius=2), Ellipse(extent={{-30,-20},{30,20}}, startAngle=10, endAngle=350), Line(points={{-10,0},{0,10},{10,0}}, color={1,2,3}), Polygon(points={{-10,-10},{10,-10},{0,10}}, fillPattern=FillPattern.HorizontalCylinder), Text(extent={{-50,-5},{50,5}}, textString="%name", textStyle={TextStyle.Bold}), Bitmap(extent={{-5,-5},{5,5}}, fileName="icon.png") }))"#;
        let annotation = parse_annotation(source).expect("annotation");
        let icon = annotation.entries[0].value.as_call().expect("Icon");
        let scene = resolve_icon_call(icon);
        assert_eq!(scene.graphics.len(), 6);
        assert!(matches!(scene.graphics[0].graphic, Graphic::Rectangle(_)));
        assert_eq!(scene.coordinate_system.extent.p1.x, -200.0);
        assert!(!scene.coordinate_system.preserve_aspect_ratio);
        assert!(scene.diagnostics.is_empty());
    }

    #[test]
    fn reports_unknown_properties_without_failing_the_graphic() {
        let annotation = parse_annotation(
            "annotation(Icon(graphics={Rectangle(extent={{0,0},{1,1}}, futureStyle=Solid)}))",
        )
        .expect("annotation");
        let icon = annotation.entries[0].value.as_call().expect("Icon");
        let scene = resolve_icon_call(icon);
        assert_eq!(scene.graphics.len(), 1);
        assert!(
            scene
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "ICON_UNKNOWN_PROPERTY")
        );
    }

    #[test]
    fn accepts_modelica_pattern_for_closed_graphics() {
        let annotation = parse_annotation(
            "annotation(Icon(graphics={Rectangle(extent={{0,0},{1,1}}, pattern=LinePattern.None), Ellipse(extent={{0,0},{1,1}}, pattern=LinePattern.Dash), Polygon(points={{0,0},{1,0},{0,1}}, pattern=LinePattern.Dot)}))",
        )
        .expect("annotation");
        let icon = annotation.entries[0].value.as_call().expect("Icon call");
        let scene = resolve_icon_call(icon);
        assert!(
            !scene
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code == "ICON_UNKNOWN_PROPERTY" }),
            "{:?}",
            scene.diagnostics
        );
        assert!(matches!(
            &scene.graphics[0].graphic,
            Graphic::Rectangle(item) if item.line_pattern.as_deref() == Some("LinePattern.None")
        ));
        assert!(matches!(
            &scene.graphics[1].graphic,
            Graphic::Ellipse(item) if item.line_pattern.as_deref() == Some("LinePattern.Dash")
        ));
        assert!(matches!(
            &scene.graphics[2].graphic,
            Graphic::Polygon(item) if item.line_pattern.as_deref() == Some("LinePattern.Dot")
        ));
    }
}
