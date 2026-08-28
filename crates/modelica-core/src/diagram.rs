use crate::annotation::{AnnotationCall, AnnotationValue, parse_call};
use crate::ast::{Class, SourceRange};
use crate::graphics::{
    resolve_coordinate_system, resolve_graphic_call, resolve_graphics_from_call,
};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::library::LibraryRegistry;
use crate::scene::{
    ComponentInstance, ConnectorRef, DiagramBounds, DiagramConnection, DiagramScene, Extent,
    Graphic, IconScene, LineGraphic, Point,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AnnotationOwner {
    Class,
    Component,
    Connection,
    Extends,
    Other,
}

struct ScopedAnnotation {
    annotation: AnnotationCall,
    token_index: usize,
    owner: AnnotationOwner,
}

#[derive(Clone, Copy)]
struct PlacementTransform {
    origin: Point,
    extent: Extent,
    rotation: f32,
}

struct Placement {
    visible: bool,
    transformation: Option<PlacementTransform>,
}

const CLASS_KEYWORDS: &[&str] = &[
    "package",
    "model",
    "block",
    "connector",
    "record",
    "function",
    "class",
    "type",
];
const DECLARATION_PREFIXES: &[&str] = &[
    "input",
    "output",
    "parameter",
    "constant",
    "discrete",
    "flow",
    "stream",
    "inner",
    "outer",
    "replaceable",
    "final",
    "each",
    "constrainedby",
];

/// Resolve the Diagram layer owned by one class. Nested class ranges are
/// masked before scanning annotations, preserving the selected-class scope.
pub fn resolve_diagram(
    class: &Class,
    source: &str,
    registry: &mut LibraryRegistry,
) -> DiagramScene {
    let class_slice = class_owned_slice(class, source);
    let tokens = tokenize(&class_slice);
    let annotations = collect_annotations(&class_slice, &tokens);
    let mut components = Vec::new();
    let mut diagnostics = Vec::new();

    for (component_index, record) in annotations
        .iter()
        .filter(|record| record.owner == AnnotationOwner::Component)
        .enumerate()
    {
        let Some((type_name, name)) =
            parse_declaration(&statement_tokens(&tokens, record.token_index))
        else {
            continue;
        };
        let Some(placement) = find_call(&record.annotation, "Placement").and_then(parse_placement)
        else {
            continue;
        };
        if !placement.visible {
            continue;
        }
        let transformation = placement.transformation.unwrap_or(PlacementTransform {
            origin: Point { x: 0.0, y: 0.0 },
            extent: Extent {
                p1: Point { x: -10.0, y: -10.0 },
                p2: Point { x: 10.0, y: 10.0 },
            },
            rotation: 0.0,
        });
        let mut component = ComponentInstance {
            id: format!(
                "{}:component:{}:{component_index}",
                class.qualified_name, name
            ),
            name,
            type_name: type_name.clone(),
            resolved_type_qualified_name: None,
            class_kind: None,
            origin: transformation.origin,
            rotation: transformation.rotation,
            placement_extent: Some(transformation.extent),
            visible: true,
            resolved_icon: None,
        };

        if let Some((component_class, component_source)) =
            resolve_component(registry, class, &type_name)
        {
            component.resolved_type_qualified_name = Some(component_class.qualified_name.clone());
            component.class_kind = Some(component_class.kind);
            let icon = IconResolverAdapter::resolve(registry, &component_class, &component_source);
            component.resolved_icon = Some(Box::new(icon));
        } else {
            diagnostics.push(format!(
                "COMPONENT_TYPE_UNRESOLVED: {}: declaredTypeName={type_name}",
                component.name
            ));
        }
        components.push(component);
    }

    let (coordinate_system, background_graphics) = diagram_layer(&annotations, &mut diagnostics);
    let connections = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.text == "connect")
        .filter_map(|(index, _)| parse_connection(&class_slice, &tokens, index, &mut diagnostics))
        .collect::<Vec<_>>();

    let content_bounds = calculate_bounds(&background_graphics, &components, &connections);
    DiagramScene {
        class_qualified_name: Some(class.qualified_name.clone()),
        class_kind: Some(class.kind),
        coordinate_system,
        background_graphics,
        components,
        connections,
        diagnostics: diagnostics
            .into_iter()
            .map(|message| crate::Diagnostic::warning("DIAGRAM_RESOLVE", message))
            .collect(),
        content_bounds,
    }
}

struct IconResolverAdapter;

impl IconResolverAdapter {
    fn resolve(registry: &mut LibraryRegistry, class: &Class, source: &str) -> IconScene {
        crate::IconResolver::new(registry).resolve(class, source)
    }
}

fn diagram_layer(
    annotations: &[ScopedAnnotation],
    diagnostics: &mut Vec<String>,
) -> (crate::scene::CoordinateSystem, Vec<Graphic>) {
    for record in annotations {
        if record.owner != AnnotationOwner::Class {
            continue;
        }
        let Some(diagram) = find_call(&record.annotation, "Diagram") else {
            continue;
        };
        let coordinate_system = find_call(diagram, "coordinateSystem")
            .map(resolve_coordinate_system)
            .unwrap_or_default();
        let (graphics, graphic_diagnostics) = resolve_graphics_from_call(diagram);
        diagnostics.extend(
            graphic_diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message),
        );
        return (coordinate_system, graphics);
    }
    (crate::scene::CoordinateSystem::default(), Vec::new())
}

fn collect_annotations(class_slice: &str, tokens: &[Token]) -> Vec<ScopedAnnotation> {
    let mut result = Vec::new();
    for index in 0..tokens.len() {
        if tokens[index].text != "annotation" {
            continue;
        }
        let Some(open) = next_significant(tokens, index + 1) else {
            continue;
        };
        if tokens[open].text != "(" {
            continue;
        }
        let Some(close) = matching_paren(tokens, open) else {
            continue;
        };
        let Some(annotation_source) = class_slice.get(tokens[index].start..tokens[close].end)
        else {
            continue;
        };
        let Ok(annotation) = parse_call(annotation_source) else {
            continue;
        };
        result.push(ScopedAnnotation {
            annotation,
            token_index: index,
            owner: classify_annotation(tokens, index),
        });
    }
    result
}

fn classify_annotation(tokens: &[Token], annotation_index: usize) -> AnnotationOwner {
    let statement = statement_tokens(tokens, annotation_index);
    if statement.iter().any(|token| token.text == "connect") {
        return AnnotationOwner::Connection;
    }
    if statement.iter().any(|token| token.text == "extends") {
        return AnnotationOwner::Extends;
    }
    if parse_declaration(&statement).is_some() {
        return AnnotationOwner::Component;
    }
    if statement
        .iter()
        .any(|token| CLASS_KEYWORDS.contains(&token.text.as_str()))
    {
        return AnnotationOwner::Class;
    }
    if statement.is_empty() || statement.iter().any(|token| token.text == "annotation") {
        return AnnotationOwner::Class;
    }
    AnnotationOwner::Other
}

fn statement_tokens(tokens: &[Token], annotation_index: usize) -> Vec<Token> {
    let start = tokens[..annotation_index]
        .iter()
        .rposition(|token| token.text == ";")
        .map_or(0, |index| index + 1);
    tokens[start..annotation_index]
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .cloned()
        .collect()
}

fn parse_declaration(tokens: &[Token]) -> Option<(String, String)> {
    if tokens
        .iter()
        .any(|token| DECLARATION_PREFIXES.contains(&token.text.as_str()))
    {
        return None;
    }
    for start in 0..tokens.len() {
        let first = tokens.get(start)?;
        if !matches!(first.kind, TokenKind::Identifier | TokenKind::Keyword)
            || CLASS_KEYWORDS.contains(&first.text.as_str())
        {
            continue;
        }
        let mut index = start + 1;
        let mut type_name = first.text.clone();
        while tokens.get(index).is_some_and(|token| token.text == ".") {
            let part = tokens.get(index + 1)?;
            if !matches!(part.kind, TokenKind::Identifier | TokenKind::Keyword) {
                break;
            }
            type_name.push('.');
            type_name.push_str(&part.text);
            index += 2;
        }
        let name = tokens.get(index)?;
        if !matches!(name.kind, TokenKind::Identifier | TokenKind::Keyword) {
            continue;
        }
        if tokens
            .get(index + 1)
            .is_some_and(|token| matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword))
        {
            continue;
        }
        return Some((type_name, name.text.clone()));
    }
    None
}

fn parse_placement(call: &AnnotationCall) -> Option<Placement> {
    let transformation = find_call(call, "transformation")
        .or_else(|| find_call(call, "iconTransformation"))
        .map(|transform| PlacementTransform {
            origin: parse_point(transform.named("origin")).unwrap_or(Point { x: 0.0, y: 0.0 }),
            extent: parse_extent(transform.named("extent")).unwrap_or(Extent {
                p1: Point { x: -10.0, y: -10.0 },
                p2: Point { x: 10.0, y: 10.0 },
            }),
            rotation: parse_number(transform.named("rotation")).unwrap_or(0.0),
        });
    Some(Placement {
        visible: parse_bool(call.named("visible")).unwrap_or(true),
        transformation,
    })
}

fn resolve_component(
    registry: &mut LibraryRegistry,
    class: &Class,
    type_name: &str,
) -> Option<(Class, String)> {
    let mut candidates = vec![type_name.to_owned()];
    let parts = class.qualified_name.split('.').collect::<Vec<_>>();
    for length in (1..parts.len()).rev() {
        candidates.push(format!("{}.{}", parts[..length].join("."), type_name));
    }
    candidates.dedup();
    candidates
        .into_iter()
        .find_map(|candidate| registry.resolve_class(&candidate))
}

fn parse_connection(
    source: &str,
    tokens: &[Token],
    connect_index: usize,
    diagnostics: &mut Vec<String>,
) -> Option<DiagramConnection> {
    let open = next_significant(tokens, connect_index + 1)?;
    if tokens[open].text != "(" {
        return None;
    }
    let close = matching_paren(tokens, open)?;
    let args = split_top_level(source.get(tokens[open].end..tokens[close].start)?);
    if args.len() != 2 {
        diagnostics.push("invalid connect() argument count".into());
        return None;
    }
    let mut line = None;
    let mut line_source_range = None;
    let mut end = tokens[close].end;
    let mut index = close + 1;
    while let Some(token) = tokens.get(index) {
        if token.text == ";" {
            end = token.end;
            break;
        }
        if token.text == "annotation" {
            let Some(annotation) = source
                .get(token.start..)
                .and_then(|value| parse_call(value).ok())
            else {
                break;
            };
            if let Some(line_call) = find_call(&annotation, "Line") {
                let (graphic, line_diagnostics) = resolve_graphic_call(line_call);
                diagnostics.extend(
                    line_diagnostics
                        .into_iter()
                        .map(|diagnostic| diagnostic.message),
                );
                if let Some(Graphic::Line(value)) = graphic {
                    line = Some(value);
                    line_source_range = Some(SourceRange::new(
                        token.start + line_call.source_range.start,
                        token.start + line_call.source_range.end,
                    ));
                }
            }
            end = token.start + annotation.source_range.end;
            break;
        }
        index += 1;
    }
    Some(DiagramConnection {
        id: format!("connection:{connect_index}"),
        lhs: connector_ref(args[0].trim()),
        rhs: connector_ref(args[1].trim()),
        from: args[0].trim().to_owned(),
        to: args[1].trim().to_owned(),
        line,
        source_range: Some(SourceRange::new(tokens[connect_index].start, end)),
        line_source_range,
    })
}

fn connector_ref(value: &str) -> ConnectorRef {
    let (component_name, connector_path) = value
        .split_once('.')
        .map_or((value, ""), |(component, connector)| (component, connector));
    ConnectorRef {
        component_name: component_name.trim().to_owned(),
        connector_path: connector_path.trim().to_owned(),
    }
}

fn split_top_level(value: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut parens = 0;
    let mut braces = 0;
    let mut brackets = 0;
    let mut quoted = false;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] as char {
            '"' => {
                if quoted && bytes.get(index + 1) == Some(&b'"') {
                    index += 1;
                } else {
                    quoted = !quoted;
                }
            }
            '(' if !quoted => parens += 1,
            ')' if !quoted => parens -= 1,
            '{' if !quoted => braces += 1,
            '}' if !quoted => braces -= 1,
            '[' if !quoted => brackets += 1,
            ']' if !quoted => brackets -= 1,
            ',' if !quoted && parens == 0 && braces == 0 && brackets == 0 => {
                result.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    result.push(&value[start..]);
    result
}

fn find_call<'a>(call: &'a AnnotationCall, name: &str) -> Option<&'a AnnotationCall> {
    call.args.iter().find_map(|entry| {
        entry
            .value
            .as_call()
            .filter(|candidate| candidate.name == name)
    })
}

fn parse_number(value: Option<&AnnotationValue>) -> Option<f32> {
    match value? {
        AnnotationValue::Number(value) => Some(*value as f32),
        _ => None,
    }
}

fn parse_bool(value: Option<&AnnotationValue>) -> Option<bool> {
    match value? {
        AnnotationValue::Bool(value) => Some(*value),
        AnnotationValue::Name(value) if value == "true" => Some(true),
        AnnotationValue::Name(value) if value == "false" => Some(false),
        _ => None,
    }
}

fn parse_point(value: Option<&AnnotationValue>) -> Option<Point> {
    let values = value?.as_array()?;
    Some(Point {
        x: parse_number(values.first())?,
        y: parse_number(values.get(1))?,
    })
}

fn parse_extent(value: Option<&AnnotationValue>) -> Option<Extent> {
    let values = value?.as_array()?;
    Some(Extent {
        p1: parse_point(values.first())?,
        p2: parse_point(values.get(1))?,
    })
}

fn next_significant(tokens: &[Token], mut index: usize) -> Option<usize> {
    while index < tokens.len()
        && matches!(
            tokens[index].kind,
            TokenKind::Whitespace | TokenKind::Comment
        )
    {
        index += 1;
    }
    (index < tokens.len()).then_some(index)
}

fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if token.text == "(" {
            depth += 1;
        } else if token.text == ")" {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn class_owned_slice(class: &Class, source: &str) -> String {
    let Some(class_source) = source.get(class.source_range.start..class.source_range.end) else {
        return String::new();
    };
    let mut bytes = class_source.as_bytes().to_vec();
    for child in &class.children {
        let start = child
            .source_range
            .start
            .saturating_sub(class.source_range.start)
            .min(bytes.len());
        let end = child
            .source_range
            .end
            .saturating_sub(class.source_range.start)
            .min(bytes.len());
        for byte in &mut bytes[start..end] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).expect("source masking preserves UTF-8")
}

fn calculate_bounds(
    graphics: &[Graphic],
    components: &[ComponentInstance],
    connections: &[DiagramConnection],
) -> Option<DiagramBounds> {
    let mut bounds = MutableBounds::default();
    for graphic in graphics {
        include_graphic(&mut bounds, graphic);
    }
    for component in components {
        if let Some(extent) = component.placement_extent {
            include_extent(&mut bounds, extent, component.origin);
        } else if let Some(icon) = component.resolved_icon.as_deref() {
            let extent = icon.coordinate_system.extent;
            include_point(
                &mut bounds,
                Point {
                    x: component.origin.x + extent.p1.x,
                    y: component.origin.y + extent.p1.y,
                },
            );
            include_point(
                &mut bounds,
                Point {
                    x: component.origin.x + extent.p2.x,
                    y: component.origin.y + extent.p2.y,
                },
            );
        }
    }
    for connection in connections {
        if let Some(LineGraphic { points, origin, .. }) = &connection.line {
            for point in points {
                include_point(
                    &mut bounds,
                    Point {
                        x: point.x + origin.x,
                        y: point.y + origin.y,
                    },
                );
            }
        }
    }
    bounds.finish()
}

#[derive(Default)]
struct MutableBounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    initialized: bool,
}

impl MutableBounds {
    fn finish(self) -> Option<DiagramBounds> {
        self.initialized.then_some(DiagramBounds {
            x: self.min_x,
            y: self.min_y,
            width: (self.max_x - self.min_x).max(1.0),
            height: (self.max_y - self.min_y).max(1.0),
        })
    }
}

fn include_point(bounds: &mut MutableBounds, point: Point) {
    if !bounds.initialized {
        bounds.min_x = point.x;
        bounds.min_y = point.y;
        bounds.max_x = point.x;
        bounds.max_y = point.y;
        bounds.initialized = true;
    } else {
        bounds.min_x = bounds.min_x.min(point.x);
        bounds.min_y = bounds.min_y.min(point.y);
        bounds.max_x = bounds.max_x.max(point.x);
        bounds.max_y = bounds.max_y.max(point.y);
    }
}

fn include_graphic(bounds: &mut MutableBounds, graphic: &Graphic) {
    match graphic {
        Graphic::Line(value) => value.points.iter().for_each(|point| {
            include_point(
                bounds,
                Point {
                    x: point.x + value.origin.x,
                    y: point.y + value.origin.y,
                },
            )
        }),
        Graphic::Polygon(value) => value.points.iter().for_each(|point| {
            include_point(
                bounds,
                Point {
                    x: point.x + value.origin.x,
                    y: point.y + value.origin.y,
                },
            )
        }),
        Graphic::Rectangle(value) => include_extent(bounds, value.extent, value.origin),
        Graphic::Ellipse(value) => include_extent(bounds, value.extent, value.origin),
        Graphic::Text(value) => include_extent(bounds, value.extent, value.origin),
        Graphic::Bitmap(value) => include_extent(bounds, value.extent, value.origin),
    }
}

fn include_extent(bounds: &mut MutableBounds, extent: Extent, origin: Point) {
    include_point(
        bounds,
        Point {
            x: extent.p1.x + origin.x,
            y: extent.p1.y + origin.y,
        },
    );
    include_point(
        bounds,
        Point {
            x: extent.p2.x + origin.x,
            y: extent.p2.y + origin.y,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::resolve_diagram;
    use crate::library::LibraryRegistry;
    use crate::parser::parse;

    #[test]
    fn resolves_diagram_background_component_and_connection() {
        let source = r#"
model Unit
  annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})}));
end Unit;

model Top
  Unit left annotation(Placement(transformation(extent={{-80,-10},{-60,10}})));
  Unit right annotation(Placement(transformation(extent={{60,-10},{80,10}})));
  equation
    connect(left, right) annotation(Line(points={{-60,0},{60,0}}));
  annotation(Diagram(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
    Rectangle(extent={{-95,-95},{95,95}})
  }));
end Top;
"#;
        let file = parse(source, "Diagram.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("Diagram.mo", source)
            .expect("index");
        let scene = resolve_diagram(&file.classes[1], source, &mut registry);
        assert_eq!(scene.background_graphics.len(), 1);
        assert_eq!(scene.components.len(), 2);
        assert_eq!(scene.connections.len(), 1);
        assert_eq!(scene.connections[0].from, "left");
        assert_eq!(scene.connections[0].lhs.component_name, "left");
        assert_eq!(scene.connections[0].lhs.connector_path, "");
        assert_eq!(scene.connections[0].rhs.component_name, "right");
        assert_eq!(scene.connections[0].rhs.connector_path, "");
        assert!(
            scene
                .components
                .iter()
                .all(|component| component.resolved_icon.is_some())
        );
        assert!(scene.content_bounds.is_some());
    }

    #[test]
    fn masks_nested_class_diagram_annotations() {
        let source = r#"package P
  model Child annotation(Diagram(graphics={Rectangle(extent={{-1,-1},{1,1}})})); end Child;
  model Parent annotation(Diagram(graphics={Ellipse(extent={{-10,-10},{10,10}})})); end Parent;
end P;"#;
        let file = parse(source, "Nested.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("Nested.mo", source)
            .expect("index");
        let parent = &file.classes[0].children[1];
        let scene = resolve_diagram(parent, source, &mut registry);
        assert_eq!(scene.background_graphics.len(), 1);
    }

    #[test]
    fn keeps_connection_endpoint_ownership_for_array_connectors() {
        let source = r#"
model Top
  Real mixer[2];
  Real h2grid;
equation
  connect(mixer.ports_a[2], h2grid.port)
    annotation(Line(points={{-40,0},{0,0},{40,20}}));
end Top;
"#;
        let file = parse(source, "ConnectorRefs.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("ConnectorRefs.mo", source)
            .expect("index");
        let scene = resolve_diagram(&file.classes[0], source, &mut registry);
        let connection = &scene.connections[0];
        assert_eq!(connection.lhs.component_name, "mixer");
        assert_eq!(connection.lhs.connector_path, "ports_a[2]");
        assert_eq!(connection.rhs.component_name, "h2grid");
        assert_eq!(connection.rhs.connector_path, "port");
        assert_eq!(connection.line.as_ref().unwrap().points.len(), 3);
        assert!(connection.line_source_range.is_some());
    }
}
