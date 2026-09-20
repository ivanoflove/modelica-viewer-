use crate::lexer::{Token, TokenKind};
use std::collections::HashMap;

/// The declaration head shared by Icon and Diagram resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentDeclaration {
    pub declared_type_name: String,
    pub instance_name: String,
    /// Array dimensions written after the component instance, for example
    /// `ports[3]` or `ports[nPorts]`. Expressions are kept as source-like
    /// strings because their values may depend on Modelica parameters.
    pub dimensions: Vec<String>,
}

const DECLARATION_PREFIXES: &[&str] = &[
    "input",
    "output",
    "flow",
    "stream",
    "inner",
    "outer",
    "replaceable",
    "final",
    "each",
    "redeclare",
    "constrainedby",
    "partial",
    "protected",
    "public",
    "parameter",
    "constant",
    "discrete",
];

const NON_COMPONENT_HEADS: &[&str] = &[
    "algorithm",
    "block",
    "class",
    "connector",
    "else",
    "equation",
    "extends",
    "function",
    "if",
    "model",
    "package",
    "record",
    "type",
    "when",
];

/// Parse the declaration head (`Type instanceName[dimensions]`).
///
/// Prefixes are accepted only before the type. Once the first complete
/// qualified type and instance name are found, the function returns
/// immediately; modifiers and annotations cannot replace the declaration
/// with a shorter suffix of the qualified type.
pub fn parse_component_declaration(tokens: &[Token]) -> Option<ComponentDeclaration> {
    let significant = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect::<Vec<_>>();
    let mut index = 0;
    while significant
        .get(index)
        .is_some_and(|token| DECLARATION_PREFIXES.contains(&token.text.as_str()))
    {
        index += 1;
    }

    // The first annotation in a class body can still share the token slice
    // with the enclosing class head (for example `model Top` or `partial
    // model Ports`). Skip that head before reading the first component.
    if significant
        .get(index)
        .is_some_and(|token| NON_COMPONENT_HEADS.contains(&token.text.as_str()))
    {
        index += 1;
        if significant
            .get(index)
            .is_some_and(|token| is_name_token(token))
        {
            index += 1;
        }
        while significant
            .get(index)
            .is_some_and(|token| DECLARATION_PREFIXES.contains(&token.text.as_str()))
        {
            index += 1;
        }
    }

    let first = significant.get(index)?;
    if !is_name_token(first) || NON_COMPONENT_HEADS.contains(&first.text.as_str()) {
        return None;
    }

    let mut declared_type_name = first.text.clone();
    index += 1;
    while significant
        .get(index)
        .is_some_and(|token| token.text == ".")
    {
        let part = significant.get(index + 1)?;
        if !is_name_token(part) {
            return None;
        }
        declared_type_name.push('.');
        declared_type_name.push_str(&part.text);
        index += 2;
    }

    while significant
        .get(index)
        .is_some_and(|token| token.text == "{")
    {
        skip_braced_dimension(&significant, &mut index)?;
    }

    let instance = significant.get(index)?;
    if !is_name_token(instance) || DECLARATION_PREFIXES.contains(&instance.text.as_str()) {
        return None;
    }
    index += 1;
    let mut dimensions = Vec::new();
    while significant
        .get(index)
        .is_some_and(|token| token.text == "[")
    {
        dimensions.push(parse_bracket_dimension(&significant, &mut index)?);
    }

    Some(ComponentDeclaration {
        declared_type_name,
        instance_name: instance.text.clone(),
        dimensions,
    })
}

/// Parse simple named modifier values from a component declaration, such as
/// `Component(gain=2, label="hot") instance`.
///
/// This deliberately keeps source-like scalar text and does not attempt to
/// evaluate Modelica expressions. Complex modifiers are ignored so callers
/// can still resolve the remaining static text macros safely.
pub fn parse_parameter_bindings(tokens: &[Token]) -> HashMap<String, String> {
    let declaration = parse_component_declaration(tokens);
    let Some(declaration) = declaration else {
        return HashMap::new();
    };
    let Some(instance_index) = tokens
        .iter()
        .position(|token| token.text == declaration.instance_name)
    else {
        return HashMap::new();
    };

    let mut index = instance_index + 1;
    while tokens.get(index).is_some_and(|token| token.text == "[") {
        let Some(close) = matching_delimiter(tokens, index, "[", "]") else {
            return HashMap::new();
        };
        index = close + 1;
    }
    if tokens.get(index).is_none_or(|token| token.text != "(") {
        return HashMap::new();
    }
    let Some(close) = matching_delimiter(tokens, index, "(", ")") else {
        return HashMap::new();
    };

    let mut bindings = HashMap::new();
    let mut segment_start = index + 1;
    let mut depth: usize = 0;
    for cursor in (index + 1)..=close {
        let at_end = cursor == close;
        if !at_end {
            match tokens[cursor].text.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if (!at_end && tokens[cursor].text != ",") || depth != 0 {
            continue;
        }
        let segment = &tokens[segment_start..cursor];
        if let Some((name, value)) = parse_named_scalar(segment) {
            bindings.insert(name, value);
        }
        segment_start = cursor + 1;
    }
    bindings
}

fn matching_delimiter(tokens: &[Token], open: usize, left: &str, right: &str) -> Option<usize> {
    let mut depth = 0;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if token.text == left {
            depth += 1;
        } else if token.text == right {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn parse_named_scalar(tokens: &[Token]) -> Option<(String, String)> {
    let equals = tokens.iter().position(|token| token.text == "=")?;
    let name = tokens[..equals]
        .iter()
        .rev()
        .find(|token| token.kind == TokenKind::Identifier)
        .map(|token| token.text.clone())?;
    let value_tokens = tokens[equals + 1..]
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect::<Vec<_>>();
    if value_tokens.is_empty()
        || value_tokens
            .iter()
            .any(|token| matches!(token.text.as_str(), "(" | ")" | "{" | "}"))
    {
        return None;
    }
    Some((
        name,
        value_tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>(),
    ))
}

fn is_name_token(token: &Token) -> bool {
    matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword)
}

fn skip_braced_dimension(tokens: &[&Token], index: &mut usize) -> Option<()> {
    let mut depth = 0;
    while let Some(token) = tokens.get(*index) {
        *index += 1;
        match token.text.as_str() {
            "{" => depth += 1,
            "}" => {
                depth -= 1;
                if depth == 0 {
                    return Some(());
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_bracket_dimension(tokens: &[&Token], index: &mut usize) -> Option<String> {
    if tokens.get(*index)?.text != "[" {
        return None;
    }
    *index += 1;
    let start = *index;
    let mut depth = 1;
    while let Some(token) = tokens.get(*index) {
        match token.text.as_str() {
            "[" => depth += 1,
            "]" => {
                depth -= 1;
                if depth == 0 {
                    let value = tokens[start..*index]
                        .iter()
                        .map(|token| token.text.as_str())
                        .collect::<String>();
                    *index += 1;
                    return Some(value);
                }
            }
            _ => {}
        }
        *index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{parse_component_declaration, parse_parameter_bindings};
    use crate::lexer::tokenize;

    fn parse(value: &str) -> (String, String, Vec<String>) {
        let declaration = parse_component_declaration(&tokenize(value)).expect("declaration");
        (
            declaration.declared_type_name,
            declaration.instance_name,
            declaration.dimensions,
        )
    }

    #[test]
    fn preserves_the_first_complete_qualified_type() {
        assert_eq!(
            parse("Interfaces.FluidInterfaces.FluidPortIN port"),
            (
                "Interfaces.FluidInterfaces.FluidPortIN".into(),
                "port".into(),
                vec![]
            )
        );
        assert_eq!(
            parse("Modelica.Fluid.Interfaces.FluidPort_a port_a"),
            (
                "Modelica.Fluid.Interfaces.FluidPort_a".into(),
                "port_a".into(),
                vec![]
            )
        );
    }

    #[test]
    fn skips_prefixes_but_stops_before_modifiers() {
        assert_eq!(
            parse("replaceable flow Interfaces.FluidPort port constrainedby Base"),
            ("Interfaces.FluidPort".into(), "port".into(), vec![])
        );
    }

    #[test]
    fn preserves_array_dimensions_after_component_name() {
        assert_eq!(
            parse("Modelica.Fluid.Interfaces.FluidPorts_a ports[3]"),
            (
                "Modelica.Fluid.Interfaces.FluidPorts_a".into(),
                "ports".into(),
                vec!["3".into()]
            )
        );
        assert_eq!(
            parse("FluidPorts_a ports[nPorts]"),
            ("FluidPorts_a".into(), "ports".into(), vec!["nPorts".into()])
        );
        assert_eq!(
            parse("FluidPorts_a ports[n,m]"),
            ("FluidPorts_a".into(), "ports".into(), vec!["n,m".into()])
        );
    }

    #[test]
    fn parses_simple_named_parameter_bindings() {
        let bindings = parse_parameter_bindings(&tokenize(
            "Modelica.Blocks.Sources.RealExpression source(y=3.5, description=\"hot\")",
        ));
        assert_eq!(bindings.get("y"), Some(&"3.5".to_owned()));
        assert_eq!(bindings.get("description"), Some(&"\"hot\"".to_owned()));
    }
}
