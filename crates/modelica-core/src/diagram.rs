use std::collections::HashMap;

use crate::annotation::{AnnotationCall, AnnotationValue, parse_call};
use crate::ast::{Class, SourceRange};
use crate::diagnostics::Diagnostic;
use crate::graphics::{
    resolve_coordinate_system, resolve_graphic_call, resolve_graphics_from_call,
};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::library::LibraryRegistry;
use crate::scene::{
    ComponentInstance, ConnectionKey, ConnectorRef, DiagramBounds, DiagramConnection, DiagramScene,
    Extent, Graphic, IconScene, LineGraphic, Point,
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
    "redeclare",
    "protected",
    "public",
    "equation",
    "algorithm",
    "initial",
    "when",
];

/// Resolve a class Diagram, including the Diagram content inherited from all
/// base classes. The renderer receives only this resolved scene; it must not
/// infer missing connector graphics from connection endpoints.
pub fn resolve_diagram(
    class: &Class,
    source: &str,
    registry: &mut LibraryRegistry,
) -> DiagramScene {
    DiagramResolver::new(registry).resolve(class, source)
}

pub struct DiagramResolver<'a> {
    registry: &'a mut LibraryRegistry,
}

impl<'a> DiagramResolver<'a> {
    pub fn new(registry: &'a mut LibraryRegistry) -> Self {
        Self { registry }
    }

    pub fn resolve(&mut self, class: &Class, source: &str) -> DiagramScene {
        self.resolve_inner(class, source, &mut Vec::new())
    }

    fn resolve_inner(
        &mut self,
        class: &Class,
        source: &str,
        visiting: &mut Vec<String>,
    ) -> DiagramScene {
        if visiting
            .iter()
            .any(|qualified_name| qualified_name == &class.qualified_name)
        {
            return DiagramScene {
                class_qualified_name: Some(class.qualified_name.clone()),
                class_kind: Some(class.kind),
                coordinate_system: Default::default(),
                background_graphics: Vec::new(),
                components: Vec::new(),
                connections: Vec::new(),
                diagnostics: vec![Diagnostic::warning(
                    "DIAGRAM_INHERITANCE_CYCLE",
                    format!("Diagram inheritance cycle at {}", class.qualified_name),
                )],
                content_bounds: None,
            };
        }

        visiting.push(class.qualified_name.clone());

        let mut coordinate_system = None;
        let mut background_graphics = Vec::new();
        let mut components = Vec::new();
        let mut connections = Vec::new();
        let mut diagnostics = Vec::new();

        // Base order is source order and is intentionally preserved. This
        // makes multiple inheritance deterministic and keeps inherited
        // connection identities owned by the class that defined them.
        for base_name in class.extends.clone() {
            let Some((base_class, base_source)) = resolve_base(self.registry, class, &base_name)
            else {
                diagnostics.push(Diagnostic::warning(
                    "DIAGRAM_BASE_NOT_FOUND",
                    format!(
                        "unable to resolve Diagram base `{base_name}` of {}",
                        class.qualified_name
                    ),
                ));
                continue;
            };
            let base = self.resolve_inner(&base_class, &base_source, visiting);
            coordinate_system.get_or_insert(base.coordinate_system);
            background_graphics.extend(base.background_graphics);
            let mut base_components = base.components;
            mark_inherited_components(&mut base_components);
            components.extend(base_components);
            connections.extend(base.connections);
            diagnostics.extend(base.diagnostics);
        }

        let (
            own_has_coordinate_system,
            own_coordinate_system,
            own_graphics,
            own_components,
            own_connections,
            own_diagnostics,
        ) = self.resolve_owned(class, source);
        if own_has_coordinate_system {
            coordinate_system = Some(own_coordinate_system);
        }
        background_graphics.extend(own_graphics);
        components.extend(own_components);
        connections.extend(own_connections);
        diagnostics.extend(own_diagnostics);

        visiting.pop();
        let content_bounds = calculate_bounds(&background_graphics, &components, &connections);
        DiagramScene {
            class_qualified_name: Some(class.qualified_name.clone()),
            class_kind: Some(class.kind),
            coordinate_system: coordinate_system.unwrap_or_default(),
            background_graphics,
            components,
            connections,
            diagnostics,
            content_bounds,
        }
    }

    fn resolve_owned(
        &mut self,
        class: &Class,
        source: &str,
    ) -> (
        bool,
        crate::scene::CoordinateSystem,
        Vec<Graphic>,
        Vec<ComponentInstance>,
        Vec<DiagramConnection>,
        Vec<Diagnostic>,
    ) {
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
            let Some(placement) =
                find_call(&record.annotation, "Placement").and_then(parse_placement)
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
                source_owner: class.qualified_name.clone(),
                type_name: type_name.clone(),
                resolved_type_qualified_name: None,
                class_kind: None,
                origin: transformation.origin,
                rotation: transformation.rotation,
                placement_extent: Some(transformation.extent),
                visible: true,
                editable: true,
                resolved_icon: None,
            };

            if let Some((component_class, component_source)) =
                resolve_component(self.registry, class, &type_name)
            {
                component.resolved_type_qualified_name =
                    Some(component_class.qualified_name.clone());
                component.class_kind = Some(component_class.kind);
                let icon = IconResolverAdapter::resolve(
                    self.registry,
                    &component_class,
                    &component_source,
                );
                component.resolved_icon = Some(Box::new(icon));
            } else {
                diagnostics.push(Diagnostic::warning(
                    "DIAGRAM_COMPONENT_TYPE_UNRESOLVED",
                    format!("{}: declaredTypeName={type_name}", component.name),
                ));
            }
            components.push(component);
        }

        let (has_coordinate_system, coordinate_system, background_graphics) =
            diagram_layer(&annotations, &mut diagnostics);
        let mut occurrence_by_endpoints = HashMap::<(ConnectorRef, ConnectorRef), usize>::new();
        let connections = tokens
            .iter()
            .enumerate()
            .filter(|(_, token)| token.text == "connect")
            .filter_map(|(index, _)| {
                let mut connection =
                    parse_connection(&class_slice, &tokens, index, &mut diagnostics)?;
                let endpoints = (connection.lhs.clone(), connection.rhs.clone());
                let occurrence = occurrence_by_endpoints
                    .entry(endpoints.clone())
                    .or_default();
                let key = ConnectionKey::new(
                    class.qualified_name.clone(),
                    endpoints.0,
                    endpoints.1,
                    *occurrence,
                );
                *occurrence += 1;
                connection.id = key.stable_id();
                connection.key = key;
                Some(connection)
            })
            .collect::<Vec<_>>();

        (
            has_coordinate_system,
            coordinate_system,
            background_graphics,
            components,
            connections,
            diagnostics,
        )
    }
}

struct IconResolverAdapter;

impl IconResolverAdapter {
    fn resolve(registry: &mut LibraryRegistry, class: &Class, source: &str) -> IconScene {
        crate::IconResolver::new(registry).resolve(class, source)
    }
}

fn mark_inherited_components(components: &mut [ComponentInstance]) {
    for component in components {
        component.editable = false;
    }
}

fn diagram_layer(
    annotations: &[ScopedAnnotation],
    diagnostics: &mut Vec<Diagnostic>,
) -> (bool, crate::scene::CoordinateSystem, Vec<Graphic>) {
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
        diagnostics.extend(graphic_diagnostics);
        return (true, coordinate_system, graphics);
    }
    (false, crate::scene::CoordinateSystem::default(), Vec::new())
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
        let annotation = match parse_call(annotation_source) {
            Ok(annotation) => annotation,
            Err(_) => continue,
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
    let significant = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect::<Vec<_>>();
    for mut start in 0..significant.len() {
        while significant
            .get(start)
            .is_some_and(|token| DECLARATION_PREFIXES.contains(&token.text.as_str()))
        {
            start += 1;
        }
        let Some(first) = significant.get(start) else {
            continue;
        };
        if !matches!(first.kind, TokenKind::Identifier | TokenKind::Keyword)
            || CLASS_KEYWORDS.contains(&first.text.as_str())
        {
            continue;
        }
        let mut index = start + 1;
        let mut type_name = first.text.clone();
        while significant
            .get(index)
            .is_some_and(|token| token.text == ".")
        {
            let Some(part) = significant.get(index + 1) else {
                break;
            };
            if !matches!(part.kind, TokenKind::Identifier | TokenKind::Keyword) {
                break;
            }
            type_name.push('.');
            type_name.push_str(&part.text);
            index += 2;
        }
        let Some(name) = significant.get(index) else {
            continue;
        };
        if !matches!(name.kind, TokenKind::Identifier | TokenKind::Keyword) {
            continue;
        }
        if significant
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

fn resolve_base(
    registry: &mut LibraryRegistry,
    class: &Class,
    base_name: &str,
) -> Option<(Class, String)> {
    let mut candidates = vec![base_name.to_owned()];
    let parts = class.qualified_name.split('.').collect::<Vec<_>>();
    for length in (1..parts.len()).rev() {
        candidates.push(format!("{}.{}", parts[..length].join("."), base_name));
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
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<DiagramConnection> {
    let open = next_significant(tokens, connect_index + 1)?;
    if tokens[open].text != "(" {
        return None;
    }
    let close = matching_paren(tokens, open)?;
    let args = split_top_level(source.get(tokens[open].end..tokens[close].start)?);
    if args.len() != 2 {
        diagnostics.push(Diagnostic::warning(
            "DIAGRAM_CONNECTION_INVALID",
            "invalid connect() argument count",
        ));
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
                diagnostics.extend(line_diagnostics);
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
        key: ConnectionKey::new(
            "",
            ConnectorRef {
                component_name: String::new(),
                connector_path: String::new(),
            },
            ConnectorRef {
                component_name: String::new(),
                connector_path: String::new(),
            },
            0,
        ),
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
        if !looks_like_nested_class_range(child, source) {
            continue;
        }
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

fn looks_like_nested_class_range(child: &Class, source: &str) -> bool {
    let preceding = source
        .get(..child.source_range.start)
        .map(tokenize)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment));
    if preceding.is_some_and(|token| {
        matches!(
            token.text.as_str(),
            "replaceable" | "redeclare" | "constrainedby"
        )
    }) {
        return false;
    }
    let Some(prefix) = source.get(child.source_range.start..child.source_range.end) else {
        return false;
    };
    let first_tokens = tokenize(prefix)
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .take(2)
        .collect::<Vec<_>>();
    first_tokens.first().is_some_and(|token| {
        CLASS_KEYWORDS.contains(&token.text.as_str())
            || (token.text == "partial"
                && first_tokens
                    .get(1)
                    .is_some_and(|next| CLASS_KEYWORDS.contains(&next.text.as_str())))
    })
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
    use super::{DiagramResolver, resolve_diagram};
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
        assert_eq!(connection.key.owner_class, "Top");
        assert_eq!(connection.key.occurrence, 0);
        assert_eq!(connection.line.as_ref().unwrap().points.len(), 3);
        assert!(connection.line_source_range.is_some());
    }

    #[test]
    fn connection_keys_are_stable_when_source_offsets_change() {
        let source = r#"
model Top
  Real a;
  Real b;
equation
  connect(a, b) annotation(Line(points={{0,0},{10,0}}));
  connect(a, b) annotation(Line(points={{0,1},{10,1}}));
end Top;
"#;
        let shifted = source.replace("connect(a, b)", "\n\n  connect(a, b)");
        let original_file = parse(source, "Stable.mo").expect("parse original");
        let shifted_file = parse(&shifted, "Stable.mo").expect("parse shifted");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("Stable.mo", source)
            .expect("index original");
        let original = resolve_diagram(&original_file.classes[0], source, &mut registry);
        registry
            .register_source("Stable.mo", &shifted)
            .expect("index shifted");
        let shifted_scene = resolve_diagram(&shifted_file.classes[0], &shifted, &mut registry);

        let original_keys = original
            .connections
            .iter()
            .map(|connection| connection.key.clone())
            .collect::<Vec<_>>();
        let shifted_keys = shifted_scene
            .connections
            .iter()
            .map(|connection| connection.key.clone())
            .collect::<Vec<_>>();
        assert_eq!(original_keys, shifted_keys);
        assert_eq!(original.connections[0].id, "connection:Top:a->b#0");
        assert_eq!(original.connections[1].id, "connection:Top:a->b#1");
    }

    #[test]
    fn inherits_diagram_ports_graphics_and_connections() {
        let source = r#"
connector FluidPort_a
  annotation(Icon(graphics={Ellipse(extent={{-10,-10},{10,10}}, fillColor={0,128,255})}));
end FluidPort_a;

connector FluidPort_b
  annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}}, fillColor={0,128,255})}));
end FluidPort_b;

partial model PartialTwoPort
  FluidPort_a port_a annotation(Placement(transformation(extent={{-110,-10},{-90,10}})));
  FluidPort_b port_b annotation(Placement(transformation(extent={{90,-10},{110,10}})));
equation
  connect(port_a, port_b) annotation(Line(points={{-90,0},{90,0}}));
annotation(
  Diagram(coordinateSystem(extent={{-120,-100},{120,100}}), graphics={
    Rectangle(extent={{-115,-95},{115,95}})
  })
);
end PartialTwoPort;

model Child
  extends PartialTwoPort;
end Child;
"#;
        let file = parse(source, "InheritedDiagram.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("InheritedDiagram.mo", source)
            .expect("index");
        let scene = resolve_diagram(&file.classes[3], source, &mut registry);

        assert_eq!(scene.components.len(), 2);
        assert!(scene.components.iter().any(|component| {
            component.name == "port_a"
                && component.class_kind == Some(crate::ast::ClassKind::Connector)
                && component
                    .resolved_icon
                    .as_ref()
                    .is_some_and(|icon| !icon.graphics.is_empty())
        }));
        assert!(scene.components.iter().any(|component| {
            component.name == "port_b"
                && component.class_kind == Some(crate::ast::ClassKind::Connector)
                && component
                    .resolved_icon
                    .as_ref()
                    .is_some_and(|icon| !icon.graphics.is_empty())
        }));
        assert_eq!(scene.background_graphics.len(), 1);
        assert_eq!(scene.connections.len(), 1);
        assert_eq!(scene.connections[0].key.owner_class, "PartialTwoPort");
        assert_eq!(scene.coordinate_system.extent.p1.x, -120.0);
        assert!(scene.components.iter().all(|component| {
            component.source_owner == "PartialTwoPort" && !component.editable
        }));
    }

    #[test]
    fn resolves_prefixed_inherited_input_and_output_components() {
        let source = r#"
connector RealInput
  annotation(Icon(graphics={Ellipse(extent={{-5,-5},{5,5}})}));
end RealInput;
connector RealOutput
  annotation(Icon(graphics={Ellipse(extent={{-5,-5},{5,5}})}));
end RealOutput;
partial model Ports
  input RealInput u annotation(Placement(transformation(extent={{-110,-10},{-90,10}})));
  output RealOutput y annotation(Placement(transformation(extent={{90,-10},{110,10}})));
end Ports;
model Derived extends Ports;
end Derived;
"#;
        let file = parse(source, "PrefixedPorts.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("PrefixedPorts.mo", source)
            .expect("index");
        let scene = resolve_diagram(&file.classes[3], source, &mut registry);
        assert_eq!(
            scene
                .components
                .iter()
                .map(|component| component.name.as_str())
                .collect::<Vec<_>>(),
            vec!["u", "y"]
        );
    }

    #[test]
    fn resolves_real_msl_partial_two_port_and_derived_child() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/modelica/msl-4.1.0/Modelica");
        let mut registry = LibraryRegistry::default();
        registry.add(crate::library::Library {
            root,
            name: Some("Modelica Standard Library".into()),
            version: Some("4.1.0".into()),
            kind: crate::library::LibraryKind::Builtin,
            read_only: true,
        });

        let (partial, partial_source) = registry
            .resolve_class("Modelica.Fluid.Interfaces.PartialTwoPort")
            .expect("bundled MSL PartialTwoPort");
        let partial_scene = DiagramResolver::new(&mut registry).resolve(&partial, &partial_source);
        for (name, expected_type) in [
            ("port_a", "Modelica.Fluid.Interfaces.FluidPort_a"),
            ("port_b", "Modelica.Fluid.Interfaces.FluidPort_b"),
        ] {
            let component = partial_scene
                .components
                .iter()
                .find(|component| component.name == name)
                .unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(
                component.resolved_type_qualified_name.as_deref(),
                Some(expected_type)
            );
            assert!(matches!(
                component.class_kind,
                Some(crate::ast::ClassKind::Connector)
            ));
            assert!(
                component
                    .resolved_icon
                    .as_ref()
                    .is_some_and(|icon| !icon.graphics.is_empty())
            );
        }
        assert_eq!(partial_scene.debug_stats().connector_components, 2);

        let child_source =
            "model Child\n  extends Modelica.Fluid.Interfaces.PartialTwoPort;\nend Child;";
        registry
            .register_source("DerivedDiagram.mo", child_source)
            .expect("index derived child");
        let (child, source) = registry.resolve_class("Child").expect("derived child");
        let child_scene = DiagramResolver::new(&mut registry).resolve(&child, &source);
        assert!(child_scene.components.iter().any(|component| {
            component.name == "port_a"
                && component.source_owner == "Modelica.Fluid.Interfaces.PartialTwoPort"
                && !component.editable
        }));
        assert!(child_scene.components.iter().any(|component| {
            component.name == "port_b"
                && component.source_owner == "Modelica.Fluid.Interfaces.PartialTwoPort"
                && !component.editable
        }));
        assert_eq!(child_scene.debug_stats().inherited_components, 2);
        assert_eq!(child_scene.debug_stats().connector_components, 2);
    }

    #[test]
    fn reports_diagram_inheritance_cycles() {
        let source = "model A extends B; end A; model B extends A; end B;";
        let file = parse(source, "DiagramCycle.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("DiagramCycle.mo", source)
            .expect("index");
        let scene = resolve_diagram(&file.classes[0], source, &mut registry);
        assert!(
            scene
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "DIAGRAM_INHERITANCE_CYCLE")
        );
    }
}
