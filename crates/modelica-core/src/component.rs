use crate::lexer::{Token, TokenKind};

/// The declaration head shared by Icon and Diagram resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentDeclaration {
    pub declared_type_name: String,
    pub instance_name: String,
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

/// Parse only the declaration head (`Type instanceName`).
///
/// Prefixes are accepted only before the type. Once the first complete
/// qualified type and instance name are found, the function returns
/// immediately; modifiers, dimensions, and annotations cannot replace the
/// declaration with a shorter suffix of the qualified type.
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

    Some(ComponentDeclaration {
        declared_type_name,
        instance_name: instance.text.clone(),
    })
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

#[cfg(test)]
mod tests {
    use super::parse_component_declaration;
    use crate::lexer::tokenize;

    fn parse(value: &str) -> (String, String) {
        let declaration = parse_component_declaration(&tokenize(value)).expect("declaration");
        (declaration.declared_type_name, declaration.instance_name)
    }

    #[test]
    fn preserves_the_first_complete_qualified_type() {
        assert_eq!(
            parse("Interfaces.FluidInterfaces.FluidPortIN port"),
            (
                "Interfaces.FluidInterfaces.FluidPortIN".into(),
                "port".into()
            )
        );
        assert_eq!(
            parse("Modelica.Fluid.Interfaces.FluidPort_a port_a"),
            (
                "Modelica.Fluid.Interfaces.FluidPort_a".into(),
                "port_a".into()
            )
        );
    }

    #[test]
    fn skips_prefixes_but_stops_before_modifiers() {
        assert_eq!(
            parse("replaceable flow Interfaces.FluidPort port constrainedby Base"),
            ("Interfaces.FluidPort".into(), "port".into())
        );
    }
}
