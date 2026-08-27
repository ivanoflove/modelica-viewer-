use std::path::{Path, PathBuf};

use crate::ast::{Class, ClassKind, ModelicaFile, SourceRange};
use crate::lexer::{Token, TokenKind, tokenize};

const CLASS_WORDS: &[&str] = &[
    "package",
    "model",
    "block",
    "connector",
    "record",
    "function",
    "class",
    "type",
];

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
    source_file: PathBuf,
}

impl Parser {
    fn new(source: &str, source_file: impl Into<PathBuf>) -> Self {
        Self {
            tokens: tokenize(source),
            position: 0,
            source_file: source_file.into(),
        }
    }

    fn skip_trivia(&mut self) {
        while matches!(
            self.tokens.get(self.position).map(|token| token.kind),
            Some(TokenKind::Whitespace | TokenKind::Comment)
        ) {
            self.position += 1;
        }
    }

    fn peek(&mut self) -> Option<&Token> {
        self.skip_trivia();
        self.tokens.get(self.position)
    }

    fn peek_text(&mut self) -> Option<&str> {
        self.peek().map(|token| token.text.as_str())
    }

    fn take(&mut self) -> Option<Token> {
        self.skip_trivia();
        let token = self.tokens.get(self.position).cloned();
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn at_eof(&mut self) -> bool {
        self.peek().is_none()
    }

    fn accept(&mut self, text: &str) -> Option<Token> {
        if self.peek_text() == Some(text) {
            self.take()
        } else {
            None
        }
    }

    fn expect(&mut self, text: &str) -> Result<Token, ParseError> {
        self.accept(text)
            .ok_or_else(|| ParseError(format!("expected `{text}`")))
    }

    fn expect_identifier(&mut self) -> Result<Token, ParseError> {
        let token = self
            .take()
            .ok_or_else(|| ParseError("expected identifier, reached EOF".into()))?;
        if matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword)
            && !is_punctuation(&token.text)
        {
            Ok(token)
        } else {
            Err(ParseError(format!(
                "expected identifier, got `{}`",
                token.text
            )))
        }
    }

    fn qualified_name(&mut self) -> Result<String, ParseError> {
        let mut name = self.expect_identifier()?.text;
        while self.accept(".").is_some() {
            name.push('.');
            name.push_str(&self.expect_identifier()?.text);
        }
        Ok(name)
    }

    fn class_start(&mut self, position: usize) -> Option<ClassStart> {
        let mut index = position;
        while matches!(
            self.tokens.get(index).map(|token| token.kind),
            Some(TokenKind::Whitespace | TokenKind::Comment)
        ) {
            index += 1;
        }
        let start = self.tokens.get(index)?.start;
        let mut partial = false;
        let mut encapsulated = false;
        let mut expandable = false;
        let mut operator = false;
        loop {
            match self.tokens.get(index)?.text.as_str() {
                "partial" => partial = true,
                "encapsulated" => encapsulated = true,
                "expandable" => expandable = true,
                "operator" => operator = true,
                _ => break,
            }
            index += 1;
            while matches!(
                self.tokens.get(index).map(|token| token.kind),
                Some(TokenKind::Whitespace | TokenKind::Comment)
            ) {
                index += 1;
            }
        }
        let word = self.tokens.get(index)?.text.as_str();
        if !CLASS_WORDS.contains(&word) {
            return None;
        }
        index += 1;
        while matches!(
            self.tokens.get(index).map(|token| token.kind),
            Some(TokenKind::Whitespace | TokenKind::Comment)
        ) {
            index += 1;
        }
        let name = self.tokens.get(index)?;
        if !matches!(name.kind, TokenKind::Identifier | TokenKind::Keyword)
            || is_punctuation(&name.text)
        {
            return None;
        }
        let kind = match (operator, expandable, word) {
            (true, _, "record") => ClassKind::OperatorRecord,
            (true, _, "function") => ClassKind::OperatorFunction,
            (true, _, _) => ClassKind::Operator,
            (_, true, "connector") => ClassKind::ExpandableConnector,
            (_, _, "package") => ClassKind::Package,
            (_, _, "model") => ClassKind::Model,
            (_, _, "block") => ClassKind::Block,
            (_, _, "connector") => ClassKind::Connector,
            (_, _, "record") => ClassKind::Record,
            (_, _, "function") => ClassKind::Function,
            (_, _, "type") => ClassKind::Type,
            _ => ClassKind::Class,
        };
        Some(ClassStart {
            start,
            partial,
            encapsulated,
            kind,
            name: name.text.clone(),
        })
    }

    fn parse_class(&mut self, parent: Option<&str>) -> Result<Class, ParseError> {
        self.skip_trivia();
        let start = self
            .class_start(self.position)
            .ok_or_else(|| ParseError("expected class declaration".into()))?;
        while matches!(
            self.peek_text(),
            Some("partial" | "encapsulated" | "expandable" | "operator")
        ) {
            self.take();
        }
        self.take()
            .ok_or_else(|| ParseError("missing class kind".into()))?;
        self.expect_identifier()?;
        let qualified_name = parent.map_or_else(
            || start.name.clone(),
            |parent| format!("{parent}.{}", start.name),
        );
        let mut class = Class {
            kind: start.kind,
            name: start.name,
            qualified_name,
            source_file: self.source_file.clone(),
            source_range: SourceRange::new(start.start, start.start),
            children: Vec::new(),
            is_partial: start.partial,
            is_encapsulated: start.encapsulated,
            extends: Vec::new(),
            is_short: false,
            base_type_name: None,
            base_prefixes: Vec::new(),
        };

        if self.accept("=").is_some() {
            self.parse_short_class(&mut class)?;
            return Ok(class);
        }

        loop {
            self.skip_trivia();
            if self.at_eof() {
                return Err(ParseError(format!("unterminated class `{}`", class.name)));
            }
            if self.class_start(self.position).is_some() {
                let child = self.parse_class(Some(&class.qualified_name))?;
                class.children.push(child);
                continue;
            }
            if self.accept("extends").is_some() {
                class.extends.push(self.qualified_name()?);
                self.skip_balanced_until_semicolon()?;
                continue;
            }
            if self.accept("end").is_some() {
                let end_name = self.expect_identifier()?;
                self.accept(";");
                if end_name.text == class.name {
                    class.source_range.end = self
                        .tokens
                        .get(self.position.saturating_sub(1))
                        .map_or(end_name.end, |token| token.end);
                    break;
                }
                continue;
            }
            self.take();
        }
        Ok(class)
    }

    fn parse_short_class(&mut self, class: &mut Class) -> Result<(), ParseError> {
        let mut parens = 0_i32;
        let mut braces = 0_i32;
        let mut brackets = 0_i32;
        let mut base_type = None;
        let mut prefixes = Vec::new();
        let mut previous_was_dot = false;
        loop {
            let token = self
                .take()
                .ok_or_else(|| ParseError(format!("unterminated short class `{}`", class.name)))?;
            match token.text.as_str() {
                "(" => parens += 1,
                ")" => parens = (parens - 1).max(0),
                "{" => braces += 1,
                "}" => braces = (braces - 1).max(0),
                "[" => brackets += 1,
                "]" => brackets = (brackets - 1).max(0),
                "input" | "output" | "flow" | "stream" => prefixes.push(token.text.clone()),
                "." if base_type.is_some() => previous_was_dot = true,
                ";" if parens == 0 && braces == 0 && brackets == 0 => {
                    class.source_range.end = token.end;
                    class.is_short = true;
                    break;
                }
                _ => {}
            }
            if matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword)
                && !is_punctuation(&token.text)
                && !prefixes.iter().any(|prefix| prefix == &token.text)
            {
                if base_type.is_none() {
                    base_type = Some(token.text);
                } else if previous_was_dot && let Some(base) = base_type.as_mut() {
                    base.push('.');
                    base.push_str(&token.text);
                }
                previous_was_dot = false;
            }
        }
        class.base_type_name = base_type;
        class.base_prefixes = prefixes;
        Ok(())
    }

    fn skip_balanced_until_semicolon(&mut self) -> Result<(), ParseError> {
        let mut parens = 0_i32;
        let mut braces = 0_i32;
        let mut brackets = 0_i32;
        while let Some(token) = self.take() {
            match token.text.as_str() {
                "(" => parens += 1,
                ")" => parens = (parens - 1).max(0),
                "{" => braces += 1,
                "}" => braces = (braces - 1).max(0),
                "[" => brackets += 1,
                "]" => brackets = (brackets - 1).max(0),
                ";" if parens == 0 && braces == 0 && brackets == 0 => return Ok(()),
                _ => {}
            }
        }
        Err(ParseError("unterminated declaration".into()))
    }

    fn parse_file(&mut self) -> Result<ModelicaFile, ParseError> {
        self.skip_trivia();
        let within = if self.accept("within").is_some() {
            if self.accept(";").is_some() {
                None
            } else {
                let name = self.qualified_name()?;
                self.expect(";")?;
                Some(name)
            }
        } else {
            None
        };
        let mut classes = Vec::new();
        while !self.at_eof() {
            if self.class_start(self.position).is_some() {
                classes.push(self.parse_class(within.as_deref())?);
            } else {
                self.take();
            }
        }
        Ok(ModelicaFile { within, classes })
    }
}

#[derive(Clone)]
struct ClassStart {
    start: usize,
    partial: bool,
    encapsulated: bool,
    kind: ClassKind,
    name: String,
}

fn is_punctuation(text: &str) -> bool {
    text.len() == 1 && "{}()[],;.=:+-*/".contains(text)
}

pub fn parse(source: &str, source_file: impl AsRef<Path>) -> Result<ModelicaFile, ParseError> {
    Parser::new(source, source_file.as_ref()).parse_file()
}

pub fn requalify_class_tree(mut class: Class, parent: Option<&str>) -> Class {
    class.qualified_name = parent.map_or_else(
        || class.name.clone(),
        |parent| format!("{parent}.{}", class.name),
    );
    let qualified = class.qualified_name.clone();
    class.children = class
        .children
        .into_iter()
        .map(|child| requalify_class_tree(child, Some(&qualified)))
        .collect();
    class
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::ast::ClassKind;

    #[test]
    fn parses_nested_classes_and_within_without_leaking_ownership() {
        let file = parse(
            "within Modelica.Electrical; package Analog model Resistor equation if true then x=1; end if; end Resistor; end Analog;",
            "Analog/package.mo",
        )
        .expect("valid Modelica source");
        assert_eq!(file.within.as_deref(), Some("Modelica.Electrical"));
        let package = &file.classes[0];
        assert_eq!(package.qualified_name, "Modelica.Electrical.Analog");
        assert_eq!(
            package.children[0].qualified_name,
            "Modelica.Electrical.Analog.Resistor"
        );
        assert_eq!(package.children[0].kind, ClassKind::Model);
    }

    #[test]
    fn parses_short_connectors_and_operator_kinds() {
        let file = parse(
            "within Modelica.Blocks.Interfaces; connector RealInput = input Real annotation(Icon(graphics={})); expandable connector Bus end Bus; operator record Value end Value;",
            "Interfaces.mo",
        )
        .expect("valid Modelica source");
        assert_eq!(file.classes[0].kind, ClassKind::Connector);
        assert!(file.classes[0].is_short);
        assert_eq!(file.classes[0].base_type_name.as_deref(), Some("Real"));
        assert_eq!(file.classes[0].base_prefixes, vec!["input"]);
        assert_eq!(file.classes[1].kind, ClassKind::ExpandableConnector);
        assert_eq!(file.classes[2].kind, ClassKind::OperatorRecord);
    }

    #[test]
    fn ignores_annotation_strings_and_compound_end_names() {
        let source = "package P annotation(Documentation(info=\"package Fake end Fake;\")); model M equation for i in 1:2 loop x=i; end for; end M; end P;";
        let file = parse(source, "test.mo").expect("valid Modelica source");
        assert_eq!(file.classes.len(), 1);
        assert_eq!(file.classes[0].children.len(), 1);
        assert_eq!(file.classes[0].source_range.end, source.len());
    }
}
