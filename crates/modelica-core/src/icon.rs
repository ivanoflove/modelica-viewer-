use crate::annotation::{AnnotationCall, AnnotationValue, parse_call};
use crate::ast::{Class, ClassKind};
use crate::diagnostics::Diagnostic;
use crate::graphics::resolve_icon_call;
use crate::lexer::{TokenKind, tokenize};
use crate::library::LibraryRegistry;
use crate::scene::{CoordinateSystem, IconScene};
use std::collections::HashMap;

pub struct IconResolver<'a> {
    registry: &'a mut LibraryRegistry,
}

impl<'a> IconResolver<'a> {
    pub fn new(registry: &'a mut LibraryRegistry) -> Self {
        Self { registry }
    }

    pub fn resolve(&mut self, class: &Class, source: &str) -> IconScene {
        let mut visiting = Vec::new();
        self.resolve_inner(class, source, &mut visiting, &class.name)
    }

    fn resolve_inner(
        &mut self,
        class: &Class,
        source: &str,
        visiting: &mut Vec<String>,
        instance_name: &str,
    ) -> IconScene {
        if visiting.iter().any(|name| name == &class.qualified_name) {
            return empty_scene(
                class,
                vec![Diagnostic::warning(
                    "ICON_INHERITANCE_CYCLE",
                    format!("Icon inheritance cycle at {}", class.qualified_name),
                )],
            );
        }
        visiting.push(class.qualified_name.clone());
        let own = find_own_icon(class, source);
        let mut diagnostics = Vec::new();
        let mut inherited: Option<IconScene> = None;
        for base_name in &class.extends {
            let Some((base_class, base_source)) = self.resolve_base(class, base_name) else {
                if let Some(base) = fallback_base_icon(base_name) {
                    inherited = Some(match inherited.take() {
                        None => base,
                        Some(mut current) => {
                            current.graphics.extend(base.graphics);
                            current.diagnostics.extend(base.diagnostics);
                            current
                        }
                    });
                    continue;
                }
                diagnostics.push(Diagnostic::warning(
                    "ICON_BASE_NOT_FOUND",
                    format!("unable to resolve Icon base `{base_name}`"),
                ));
                continue;
            };
            let base = self.resolve_inner(&base_class, &base_source, visiting, instance_name);
            inherited = Some(match inherited.take() {
                None => base,
                Some(mut current) => {
                    current.graphics.extend(base.graphics);
                    current.diagnostics.extend(base.diagnostics);
                    current
                }
            });
        }
        let mut result = match (inherited, own) {
            (None, None) => empty_scene(class, diagnostics),
            (Some(mut base), None) => {
                base.owner_qualified_name = Some(class.qualified_name.clone());
                base.diagnostics.extend(diagnostics);
                base
            }
            (None, Some((_, _, mut own))) => {
                own.owner_qualified_name = Some(class.qualified_name.clone());
                own.diagnostics.extend(diagnostics);
                own
            }
            (Some(base), Some((has_coordinate_system, _, own))) => {
                let coordinate_system = if has_coordinate_system {
                    own.coordinate_system
                } else {
                    base.coordinate_system
                };
                let mut merged = IconScene {
                    owner_qualified_name: Some(class.qualified_name.clone()),
                    coordinate_system,
                    graphics: base.graphics,
                    diagnostics: base.diagnostics,
                };
                merged.graphics.extend(own.graphics);
                merged.diagnostics.extend(own.diagnostics);
                merged.diagnostics.extend(diagnostics);
                merged
            }
        };
        let connector_graphics =
            self.resolve_public_connector_graphics(class, source, visiting, instance_name);
        result.graphics.extend(connector_graphics);
        let parameter_defaults = parameter_defaults(class, source);
        expand_text_macros(&mut result, class, instance_name, &parameter_defaults);
        visiting.pop();
        for diagnostic in &mut result.diagnostics {
            diagnostic.owner = Some(class.qualified_name.clone());
        }
        result
    }

    fn resolve_public_connector_graphics(
        &mut self,
        class: &Class,
        source: &str,
        visiting: &mut Vec<String>,
        _instance_name: &str,
    ) -> Vec<crate::scene::Graphic> {
        let mut graphics = Vec::new();
        for component in find_component_placements(class, source) {
            if !component.visible || component.icon_visible == Some(false) {
                continue;
            }
            let Some(transformation) = component.icon_transformation.or(component.transformation)
            else {
                continue;
            };
            let Some((component_class, component_source)) =
                self.resolve_component(class, &component.type_name)
            else {
                continue;
            };
            if !matches!(
                component_class.kind,
                ClassKind::Connector | ClassKind::ExpandableConnector
            ) {
                continue;
            }
            let child = self.resolve_inner(
                &component_class,
                &component_source,
                visiting,
                &component.name,
            );
            let coordinate_system = child.coordinate_system;
            graphics.extend(child.graphics.into_iter().map(|graphic| {
                transform_graphic_by_placement(graphic, coordinate_system, transformation)
            }));
        }
        graphics
    }

    fn resolve_component(&mut self, class: &Class, type_name: &str) -> Option<(Class, String)> {
        let mut candidates = vec![type_name.to_owned()];
        let parts = class.qualified_name.split('.').collect::<Vec<_>>();
        for length in (1..parts.len()).rev() {
            candidates.push(format!("{}.{}", parts[..length].join("."), type_name));
        }
        candidates.dedup();
        candidates
            .into_iter()
            .find_map(|candidate| self.registry.resolve_class(&candidate))
    }

    fn resolve_base(&mut self, class: &Class, base_name: &str) -> Option<(Class, String)> {
        let mut candidates = vec![base_name.to_owned()];
        let parts = class.qualified_name.split('.').collect::<Vec<_>>();
        for length in (1..parts.len()).rev() {
            candidates.push(format!("{}.{}", parts[..length].join("."), base_name));
        }
        candidates.dedup();
        candidates
            .into_iter()
            .find_map(|candidate| self.registry.resolve_class(&candidate))
    }
}

#[derive(Clone, Copy)]
struct PlacementTransform {
    origin: crate::scene::Point,
    extent: crate::scene::Extent,
    rotation: f32,
}

struct ComponentPlacement {
    type_name: String,
    name: String,
    visible: bool,
    icon_visible: Option<bool>,
    transformation: Option<PlacementTransform>,
    icon_transformation: Option<PlacementTransform>,
}

fn find_component_placements(class: &Class, source: &str) -> Vec<ComponentPlacement> {
    let range = class.source_range;
    let Some(class_source) = source.get(range.start..range.end) else {
        return Vec::new();
    };
    let owned = mask_child_ranges(class, source, class_source);
    let tokens = tokenize(&owned);
    let mut result = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.text != "annotation" || token.kind != TokenKind::Keyword {
            continue;
        }
        let statement_start = tokens[..index]
            .iter()
            .rposition(|item| item.text == ";")
            .map_or(0, |item| item + 1);
        let statement = &tokens[statement_start..index];
        if statement.iter().any(|item| item.text == "protected") {
            continue;
        }
        let Some((type_name, name)) = component_declaration(statement) else {
            continue;
        };
        let Some(open_index) = next_significant(&tokens, index + 1) else {
            continue;
        };
        if tokens[open_index].text != "(" {
            continue;
        }
        let Some(close_index) = matching_paren(&tokens, open_index) else {
            continue;
        };
        let Some(call_source) = owned.get(token.start..tokens[close_index].end) else {
            continue;
        };
        let Ok(annotation) = parse_call(call_source) else {
            continue;
        };
        let Some(placement) = find_call_argument(&annotation, "Placement") else {
            continue;
        };
        result.push(ComponentPlacement {
            type_name,
            name,
            visible: placement
                .named("visible")
                .and_then(parse_bool_value)
                .unwrap_or(true),
            icon_visible: placement.named("iconVisible").and_then(parse_bool_value),
            transformation: find_call_argument(placement, "transformation")
                .and_then(parse_placement_transform),
            icon_transformation: find_call_argument(placement, "iconTransformation")
                .and_then(parse_placement_transform),
        });
    }
    result
}

fn component_declaration(tokens: &[crate::lexer::Token]) -> Option<(String, String)> {
    let significant = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect::<Vec<_>>();
    let ignored = [
        "algorithm",
        "block",
        "class",
        "connector",
        "else",
        "equation",
        "extends",
        "function",
        "if",
        "input",
        "output",
        "model",
        "package",
        "record",
        "flow",
        "stream",
        "final",
        "parameter",
        "constant",
        "discrete",
        "replaceable",
        "inner",
        "outer",
        "each",
        "redeclare",
        "constrainedby",
        "type",
        "when",
    ];
    let mut declaration = None;
    for start in 0..significant.len() {
        let first = significant[start];
        if ignored.contains(&first.text.as_str())
            || !matches!(first.kind, TokenKind::Identifier | TokenKind::Keyword)
        {
            continue;
        }
        let mut index = start + 1;
        let mut type_name = first.text.clone();
        while significant
            .get(index)
            .is_some_and(|token| token.text == ".")
        {
            let part = significant.get(index + 1)?;
            if !matches!(part.kind, TokenKind::Identifier | TokenKind::Keyword) {
                break;
            }
            type_name.push('.');
            type_name.push_str(&part.text);
            index += 2;
        }
        while significant
            .get(index)
            .is_some_and(|token| token.text == "{")
        {
            let mut depth = 0;
            while let Some(token) = significant.get(index) {
                index += 1;
                if token.text == "{" {
                    depth += 1;
                } else if token.text == "}" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
        }
        let Some(name) = significant.get(index) else {
            continue;
        };
        if !matches!(name.kind, TokenKind::Identifier | TokenKind::Keyword)
            || ignored.contains(&name.text.as_str())
        {
            continue;
        }
        declaration = Some((type_name, name.text.clone()));
    }
    declaration
}

fn find_call_argument<'a>(call: &'a AnnotationCall, name: &str) -> Option<&'a AnnotationCall> {
    call.named(name)
        .and_then(AnnotationValue::as_call)
        .or_else(|| {
            call.args
                .iter()
                .filter(|entry| entry.name.is_none())
                .find_map(|entry| {
                    entry
                        .value
                        .as_call()
                        .filter(|candidate| candidate.name == name)
                })
        })
}

fn parse_placement_transform(call: &AnnotationCall) -> Option<PlacementTransform> {
    Some(PlacementTransform {
        origin: call
            .named("origin")
            .and_then(parse_point)
            .unwrap_or(crate::scene::Point { x: 0.0, y: 0.0 }),
        extent: call.named("extent").and_then(parse_extent)?,
        rotation: call.named("rotation").and_then(parse_number).unwrap_or(0.0),
    })
}

fn transform_graphic_by_placement(
    graphic: crate::scene::Graphic,
    coordinate_system: crate::scene::CoordinateSystem,
    transform: PlacementTransform,
) -> crate::scene::Graphic {
    use crate::scene::Graphic;
    let origin = graphic_origin(&graphic);
    let rotation = graphic_rotation(&graphic);
    let map = |point: crate::scene::Point| {
        let source = transform_graphic_point(point, origin, rotation);
        transform_placement_point(source, coordinate_system, transform)
    };
    match graphic {
        Graphic::Line(mut item) => {
            item.points = item.points.into_iter().map(map).collect();
            item.origin = crate::scene::Point { x: 0.0, y: 0.0 };
            item.rotation = 0.0;
            Graphic::Line(item)
        }
        Graphic::Polygon(mut item) => {
            item.points = item.points.into_iter().map(map).collect();
            item.origin = crate::scene::Point { x: 0.0, y: 0.0 };
            item.rotation = 0.0;
            Graphic::Polygon(item)
        }
        Graphic::Rectangle(mut item) => {
            item.extent = transformed_extent(item.extent, &map);
            item.origin = crate::scene::Point { x: 0.0, y: 0.0 };
            item.rotation = 0.0;
            Graphic::Rectangle(item)
        }
        Graphic::Ellipse(mut item) => {
            item.extent = transformed_extent(item.extent, &map);
            item.origin = crate::scene::Point { x: 0.0, y: 0.0 };
            item.rotation = 0.0;
            Graphic::Ellipse(item)
        }
        Graphic::Text(mut item) => {
            item.extent = transformed_extent(item.extent, &map);
            item.origin = crate::scene::Point { x: 0.0, y: 0.0 };
            item.rotation = transform.rotation;
            Graphic::Text(item)
        }
        Graphic::Bitmap(mut item) => {
            item.extent = transformed_extent(item.extent, &map);
            item.origin = crate::scene::Point { x: 0.0, y: 0.0 };
            item.rotation = 0.0;
            Graphic::Bitmap(item)
        }
    }
}

fn graphic_origin(graphic: &crate::scene::Graphic) -> crate::scene::Point {
    match graphic {
        crate::scene::Graphic::Line(item) => item.origin,
        crate::scene::Graphic::Polygon(item) => item.origin,
        crate::scene::Graphic::Rectangle(item) => item.origin,
        crate::scene::Graphic::Ellipse(item) => item.origin,
        crate::scene::Graphic::Text(item) => item.origin,
        crate::scene::Graphic::Bitmap(item) => item.origin,
    }
}

fn graphic_rotation(graphic: &crate::scene::Graphic) -> f32 {
    match graphic {
        crate::scene::Graphic::Line(item) => item.rotation,
        crate::scene::Graphic::Polygon(item) => item.rotation,
        crate::scene::Graphic::Rectangle(item) => item.rotation,
        crate::scene::Graphic::Ellipse(item) => item.rotation,
        crate::scene::Graphic::Text(item) => item.rotation,
        crate::scene::Graphic::Bitmap(item) => item.rotation,
    }
}

fn transform_graphic_point(
    point: crate::scene::Point,
    origin: crate::scene::Point,
    rotation: f32,
) -> crate::scene::Point {
    let angle = rotation.to_radians();
    crate::scene::Point {
        x: origin.x + point.x * angle.cos() - point.y * angle.sin(),
        y: origin.y + point.x * angle.sin() + point.y * angle.cos(),
    }
}

fn transform_placement_point(
    point: crate::scene::Point,
    coordinate_system: crate::scene::CoordinateSystem,
    transform: PlacementTransform,
) -> crate::scene::Point {
    let source = coordinate_system.extent;
    let target = transform.extent;
    let source_width = target_dimension(source.p2.x - source.p1.x);
    let source_height = target_dimension(source.p2.y - source.p1.y);
    let scale_x = (target.p2.x - target.p1.x) / source_width;
    let scale_y = (target.p2.y - target.p1.y) / source_height;
    let translated = crate::scene::Point {
        x: point.x * scale_x + target.p1.x - source.p1.x * scale_x,
        y: point.y * scale_y + target.p1.y - source.p1.y * scale_y,
    };
    let angle = transform.rotation.to_radians();
    crate::scene::Point {
        x: transform.origin.x + translated.x * angle.cos() - translated.y * angle.sin(),
        y: transform.origin.y + translated.x * angle.sin() + translated.y * angle.cos(),
    }
}

fn target_dimension(value: f32) -> f32 {
    if value.abs() <= f32::EPSILON {
        1.0
    } else {
        value
    }
}

fn parse_bool_value(value: &AnnotationValue) -> Option<bool> {
    match value {
        AnnotationValue::Bool(value) => Some(*value),
        AnnotationValue::Name(value) => match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn parse_number_value(value: &AnnotationValue) -> Option<f32> {
    match value {
        AnnotationValue::Number(value) => Some(*value as f32),
        _ => None,
    }
}

fn parse_point(value: &AnnotationValue) -> Option<crate::scene::Point> {
    let values = value.as_array()?;
    Some(crate::scene::Point {
        x: values.first().and_then(parse_number_value)?,
        y: values.get(1).and_then(parse_number_value)?,
    })
}

fn parse_extent(value: &AnnotationValue) -> Option<crate::scene::Extent> {
    let values = value.as_array()?;
    Some(crate::scene::Extent {
        p1: values.first().and_then(parse_point)?,
        p2: values.get(1).and_then(parse_point)?,
    })
}

fn parse_number(value: &AnnotationValue) -> Option<f32> {
    parse_number_value(value)
}

fn transformed_extent(
    extent: crate::scene::Extent,
    map: &impl Fn(crate::scene::Point) -> crate::scene::Point,
) -> crate::scene::Extent {
    let points = [
        extent.p1,
        crate::scene::Point {
            x: extent.p1.x,
            y: extent.p2.y,
        },
        crate::scene::Point {
            x: extent.p2.x,
            y: extent.p1.y,
        },
        extent.p2,
    ]
    .map(map);
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for point in points {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }
    crate::scene::Extent {
        p1: crate::scene::Point { x: min_x, y: min_y },
        p2: crate::scene::Point { x: max_x, y: max_y },
    }
}

fn expand_text_macros(
    scene: &mut IconScene,
    class: &Class,
    instance_name: &str,
    defaults: &HashMap<String, String>,
) {
    for graphic in &mut scene.graphics {
        let crate::scene::Graphic::Text(text) = graphic else {
            continue;
        };
        text.text = expand_text(&text.text, class, instance_name, defaults);
    }
}

fn expand_text(
    template: &str,
    class: &Class,
    instance_name: &str,
    defaults: &HashMap<String, String>,
) -> String {
    let mut output = String::with_capacity(template.len());
    let chars = template.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '%' {
            output.push(chars[index]);
            index += 1;
            continue;
        }
        if chars.get(index + 1) == Some(&'%') {
            output.push('%');
            index += 2;
            continue;
        }
        let braced = chars.get(index + 1) == Some(&'{');
        let (start, mut end) = if braced {
            let start = index + 2;
            let end = chars[start..]
                .iter()
                .position(|character| *character == '}')
                .map_or(start, |offset| start + offset);
            (start, end)
        } else {
            let start = index + 1;
            let mut end = start;
            while chars
                .get(end)
                .is_some_and(|character| character.is_ascii_alphanumeric() || *character == '_')
            {
                end += 1;
            }
            (start, end)
        };
        if end == start {
            output.push('%');
            index += 1;
            continue;
        }
        let key = chars[start..end].iter().collect::<String>();
        if braced && chars.get(end) == Some(&'}') {
            end += 1;
        }
        match key.as_str() {
            "name" => output.push_str(instance_name),
            "class" => output.push_str(&class.name),
            _ if defaults.contains_key(&key) => {
                output.push_str(defaults.get(&key).expect("checked parameter default key"))
            }
            _ => append_unresolved_macro(&mut output, &key, braced),
        }
        index = end;
    }
    output
}

fn append_unresolved_macro(output: &mut String, key: &str, braced: bool) {
    output.push('%');
    if braced {
        output.push('{');
    }
    output.push_str(key);
    if braced {
        output.push('}');
    }
}

fn parameter_defaults(class: &Class, source: &str) -> HashMap<String, String> {
    let range = class.source_range;
    let Some(class_source) = source.get(range.start..range.end) else {
        return HashMap::new();
    };
    let owned = mask_child_ranges(class, source, class_source);
    let tokens = tokenize(&owned);
    let mut defaults = HashMap::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].text != "parameter" {
            index += 1;
            continue;
        }
        let end = tokens[index..]
            .iter()
            .position(|token| token.text == ";")
            .map_or(tokens.len(), |offset| index + offset);
        let Some(equals) = tokens[index..end]
            .iter()
            .position(|token| token.text == "=")
            .map(|offset| index + offset)
        else {
            index = end.saturating_add(1);
            continue;
        };
        let Some(name) = tokens[index..equals]
            .iter()
            .rev()
            .find(|token| matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword))
        else {
            index = end.saturating_add(1);
            continue;
        };
        let value_start = tokens[equals].end;
        let value_end = tokens.get(end).map_or(owned.len(), |token| token.start);
        let Some(value_source) = owned.get(value_start..value_end) else {
            index = end.saturating_add(1);
            continue;
        };
        let wrapped = format!("__value({value_source})");
        if let Ok(call) = parse_call(&wrapped)
            && let Some(value) = call.positional(0).and_then(scalar_value)
        {
            defaults.insert(name.text.clone(), value);
        }
        index = end.saturating_add(1);
    }
    defaults
}

fn scalar_value(value: &AnnotationValue) -> Option<String> {
    match value {
        AnnotationValue::Number(value) => Some(value.to_string()),
        AnnotationValue::String(value) | AnnotationValue::Name(value) => Some(value.clone()),
        AnnotationValue::Bool(value) => Some(value.to_string()),
        AnnotationValue::Array(_) | AnnotationValue::Call(_) => None,
    }
}

fn fallback_base_icon(base_name: &str) -> Option<IconScene> {
    if !base_name.starts_with("Modelica.Icons.") {
        return None;
    }
    Some(IconScene {
        owner_qualified_name: Some(base_name.to_owned()),
        coordinate_system: CoordinateSystem::default(),
        graphics: vec![crate::scene::Graphic::Rectangle(
            crate::scene::RectangleGraphic {
                origin: crate::scene::Point { x: 0.0, y: 0.0 },
                rotation: 0.0,
                extent: crate::scene::Extent {
                    p1: crate::scene::Point { x: -80.0, y: -60.0 },
                    p2: crate::scene::Point { x: 80.0, y: 60.0 },
                },
                line_color: [0, 0, 127],
                fill_color: [255, 255, 255],
                line_pattern: None,
                line_thickness: Some(0.25),
                fill_pattern: Some("FillPattern.Solid".into()),
                radius: None,
            },
        )],
        diagnostics: Vec::new(),
    })
}

fn empty_scene(class: &Class, mut diagnostics: Vec<Diagnostic>) -> IconScene {
    let mut scene = IconScene {
        owner_qualified_name: Some(class.qualified_name.clone()),
        coordinate_system: CoordinateSystem::default(),
        graphics: Vec::new(),
        diagnostics: Vec::new(),
    };
    scene.diagnostics.append(&mut diagnostics);
    scene
}

fn find_own_icon(class: &Class, source: &str) -> Option<(bool, AnnotationCall, IconScene)> {
    let range = class.source_range;
    let class_source = source.get(range.start..range.end)?;
    let owned = mask_child_ranges(class, source, class_source);
    let tokens = tokenize(&owned);
    for (index, token) in tokens.iter().enumerate() {
        if token.text != "annotation" || token.kind != TokenKind::Keyword {
            continue;
        }
        let open_index = next_significant(&tokens, index + 1)?;
        if tokens[open_index].text != "(" {
            continue;
        }
        let close_index = matching_paren(&tokens, open_index)?;
        let call_source = owned.get(token.start..tokens[close_index].end)?;
        let Ok(annotation) = parse_call(call_source) else {
            continue;
        };
        let Some(icon) = annotation
            .args
            .iter()
            .find_map(|entry| entry.value.as_call().filter(|call| call.name == "Icon"))
        else {
            continue;
        };
        let has_coordinate_system = icon.named("coordinateSystem").is_some()
            || icon.named("extent").is_some()
            || icon.args.iter().any(|entry| {
                entry.name.is_none()
                    && entry
                        .value
                        .as_call()
                        .is_some_and(|call| call.name == "coordinateSystem")
            });
        let scene = resolve_icon_call(icon);
        return Some((has_coordinate_system, annotation, scene));
    }
    None
}

fn mask_child_ranges(class: &Class, source: &str, class_source: &str) -> String {
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
    let _ = source;
    String::from_utf8(bytes).expect("source masking preserves UTF-8")
}

fn next_significant(tokens: &[crate::lexer::Token], mut index: usize) -> Option<usize> {
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

fn matching_paren(tokens: &[crate::lexer::Token], open: usize) -> Option<usize> {
    let mut depth = 0_i32;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if token.text == "(" {
            depth += 1;
        }
        if token.text == ")" {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::IconResolver;
    use crate::library::LibraryRegistry;
    use crate::parser::parse;
    use crate::scene::Graphic;

    #[test]
    fn resolves_only_selected_class_icon_and_masks_child_icons() {
        let source = "package P model Boundary annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})})); end Boundary; model Child annotation(Icon(graphics={Ellipse(extent={{-20,-20},{20,20}})})); end Child; end P;";
        let file = parse(source, "P.mo").expect("parse");
        let package = &file.classes[0];
        let boundary = &package.children[0];
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("P.mo", source)
            .expect("index source");
        let scene = IconResolver::new(&mut registry).resolve(boundary, source);
        assert_eq!(scene.graphics.len(), 1);
        assert!(matches!(scene.graphics[0], Graphic::Rectangle(_)));
    }

    #[test]
    fn merges_extends_icons_and_detects_cycles() {
        let source = "model Base annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})})); end Base; model Child extends Base annotation(Icon(graphics={Ellipse(extent={{-5,-5},{5,5}})})); end Child;";
        let file = parse(source, "Icons.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("Icons.mo", source)
            .expect("index source");
        let scene = IconResolver::new(&mut registry).resolve(&file.classes[1], source);
        assert_eq!(scene.graphics.len(), 2);
    }

    #[test]
    fn lazily_resolves_real_input_from_bundled_msl() {
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
        let (class, source) = registry
            .resolve_class("Modelica.Blocks.Interfaces.RealInput")
            .expect("bundled RealInput");
        assert!(class.is_short);
        let scene = IconResolver::new(&mut registry).resolve(&class, &source);
        assert_eq!(scene.graphics.len(), 1);
        assert!(scene.diagnostics.is_empty(), "{:?}", scene.diagnostics);
    }

    #[test]
    fn skips_non_icon_annotations_before_class_icon() {
        let source = r#"model WithDialog
  parameter Real value = 1 annotation(Dialog(tab = "General"));
  annotation(
    Documentation(info = "metadata"),
    Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})})
  );
end WithDialog;"#;
        let file = parse(source, "WithDialog.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("WithDialog.mo", source)
            .expect("index source");
        let scene = IconResolver::new(&mut registry).resolve(&file.classes[0], source);
        assert_eq!(scene.graphics.len(), 1);
    }

    #[test]
    fn resolves_builtin_icon_inheritance_by_qualified_name() {
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
        let (class, source) = registry
            .resolve_class("Modelica.Icons.ExamplesPackage")
            .expect("bundled ExamplesPackage");
        let scene = IconResolver::new(&mut registry).resolve(&class, &source);
        assert!(!scene.graphics.is_empty());
        assert!(
            !scene
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "ICON_BASE_NOT_FOUND"),
            "{:?}",
            scene.diagnostics
        );
    }

    #[test]
    fn expands_text_macros_and_composes_public_connector_icons() {
        let source = r#"
connector Pin
  annotation(Icon(graphics={
    Text(extent={{-20,-10},{20,10}}, textString="%name/%class"),
    Rectangle(extent={{-5,-5},{5,5}})
  }));
end Pin;

model Parent
  parameter Real label = 42;
  Pin leftPin annotation(Placement(transformation(extent={{-100,-10},{-80,10}})));
  annotation(Icon(graphics={Text(extent={{-40,-10},{40,10}}, textString="%name/%label") }));
end Parent;
"#;
        let file = parse(source, "Icons.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("Icons.mo", source)
            .expect("index source");
        let scene = IconResolver::new(&mut registry).resolve(&file.classes[1], source);
        assert_eq!(scene.graphics.len(), 3);
        let texts = scene
            .graphics
            .iter()
            .filter_map(|graphic| match graphic {
                Graphic::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(texts.contains(&"Parent/42"));
        assert!(texts.contains(&"leftPin/Pin"));
    }
}
