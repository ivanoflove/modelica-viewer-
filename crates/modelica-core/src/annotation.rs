//! Generic Modelica annotation syntax.
//!
//! This layer deliberately knows nothing about Icon or Diagram rendering. It
//! preserves source ranges so resolvers can validate ownership and diagnostics
//! can point back to the original source.

use crate::ast::SourceRange;
use crate::lexer::{Token, TokenKind, tokenize};
use crate::parser::ParseError;

#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationValue {
    Number(f64),
    String(String),
    Bool(bool),
    Name(String),
    Array(Vec<AnnotationValue>),
    Call(AnnotationCall),
}

impl AnnotationValue {
    pub fn as_array(&self) -> Option<&[AnnotationValue]> {
        if let Self::Array(value) = self {
            Some(value)
        } else {
            None
        }
    }

    pub fn as_call(&self) -> Option<&AnnotationCall> {
        if let Self::Call(value) = self {
            Some(value)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationEntry {
    pub name: Option<String>,
    pub value: AnnotationValue,
    pub source_range: SourceRange,
    pub name_range: Option<SourceRange>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationCall {
    pub name: String,
    pub args: Vec<AnnotationEntry>,
    pub source_range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub entries: Vec<AnnotationEntry>,
    pub source_range: SourceRange,
}

impl AnnotationCall {
    pub fn named(&self, name: &str) -> Option<&AnnotationValue> {
        self.args
            .iter()
            .find(|entry| entry.name.as_deref() == Some(name))
            .map(|entry| &entry.value)
    }

    pub fn positional(&self, index: usize) -> Option<&AnnotationValue> {
        self.args
            .iter()
            .filter(|entry| entry.name.is_none())
            .nth(index)
            .map(|entry| &entry.value)
    }
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn new(source: &str) -> Self {
        let tokens = tokenize(source)
            .into_iter()
            .filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment))
            .collect();
        Self {
            tokens,
            position: 0,
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn take(&mut self) -> Result<Token, ParseError> {
        let token = self
            .tokens
            .get(self.position)
            .cloned()
            .ok_or_else(|| ParseError("unexpected end of annotation".into()))?;
        self.position += 1;
        Ok(token)
    }

    fn accept(&mut self, text: &str) -> Option<Token> {
        if self.peek().is_some_and(|token| token.text == text) {
            self.take().ok()
        } else {
            None
        }
    }

    fn expect(&mut self, text: &str) -> Result<Token, ParseError> {
        self.accept(text)
            .ok_or_else(|| ParseError(format!("expected `{text}` in annotation")))
    }

    fn parse_identifier(&mut self) -> Result<Token, ParseError> {
        let token = self.take()?;
        if matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword)
            && !is_punctuation(&token.text)
        {
            Ok(token)
        } else {
            Err(ParseError(format!(
                "expected annotation name, got `{}`",
                token.text
            )))
        }
    }

    fn parse_name(&mut self) -> Result<(String, SourceRange), ParseError> {
        let first = self.parse_identifier()?;
        let mut name = first.text;
        let mut end = first.end;
        while self.accept(".").is_some() {
            let part = self.parse_identifier()?;
            name.push('.');
            name.push_str(&part.text);
            end = part.end;
        }
        Ok((name, SourceRange::new(first.start, end)))
    }

    fn parse_value(&mut self) -> Result<(AnnotationValue, SourceRange), ParseError> {
        let token = self
            .peek()
            .ok_or_else(|| ParseError("expected annotation value".into()))?
            .clone();
        if token.text == "{" {
            return self.parse_array();
        }
        if token.text == "-" || token.text == "+" || token.kind == TokenKind::Number {
            let first = self.take()?;
            let mut text = first.text;
            let end = if text == "-" || text == "+" {
                let number = self.take()?;
                text.push_str(&number.text);
                number.end
            } else {
                first.end
            };
            let value = text
                .parse::<f64>()
                .map_err(|_| ParseError(format!("invalid number `{text}`")))?;
            return Ok((
                AnnotationValue::Number(value),
                SourceRange::new(first.start, end),
            ));
        }
        if token.kind == TokenKind::String {
            let token = self.take()?;
            return Ok((
                AnnotationValue::String(decode_string(&token.text)),
                SourceRange::new(token.start, token.end),
            ));
        }
        let (name, name_range) = self.parse_name()?;
        if self.peek().is_some_and(|next| next.text == "(") {
            let call = self.parse_call_with_name(name, name_range.start)?;
            let range = call.source_range;
            return Ok((AnnotationValue::Call(call), range));
        }
        if name == "true" || name == "false" {
            return Ok((AnnotationValue::Bool(name == "true"), name_range));
        }
        Ok((AnnotationValue::Name(name), name_range))
    }

    fn parse_array(&mut self) -> Result<(AnnotationValue, SourceRange), ParseError> {
        let left = self.expect("{")?;
        let mut values = Vec::new();
        if self.peek().is_some_and(|token| token.text != "}") {
            loop {
                values.push(self.parse_value()?.0);
                if self.accept(",").is_none() {
                    break;
                }
                if self.peek().is_some_and(|token| token.text == "}") {
                    break;
                }
            }
        }
        let right = self.expect("}")?;
        Ok((
            AnnotationValue::Array(values),
            SourceRange::new(left.start, right.end),
        ))
    }

    fn parse_call(&mut self) -> Result<AnnotationCall, ParseError> {
        let (name, range) = self.parse_name()?;
        self.parse_call_with_name(name, range.start)
    }

    fn parse_call_with_name(
        &mut self,
        name: String,
        start: usize,
    ) -> Result<AnnotationCall, ParseError> {
        self.expect("(")?;
        let mut args = Vec::new();
        if self.peek().is_some_and(|token| token.text != ")") {
            loop {
                let named = self.peek().and_then(|token| {
                    let next = self.tokens.get(self.position + 1)?;
                    if matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword)
                        && next.text == "="
                    {
                        Some((token.text.clone(), SourceRange::new(token.start, token.end)))
                    } else {
                        None
                    }
                });
                let (name, name_range) = if let Some((name, range)) = named {
                    self.take()?;
                    self.expect("=")?;
                    (Some(name), Some(range))
                } else {
                    (None, None)
                };
                let (value, range) = self.parse_value()?;
                args.push(AnnotationEntry {
                    name,
                    value,
                    source_range: SourceRange::new(
                        name_range.map_or(range.start, |range| range.start),
                        range.end,
                    ),
                    name_range,
                });
                if self.accept(",").is_none() {
                    break;
                }
                if self.peek().is_some_and(|token| token.text == ")") {
                    break;
                }
            }
        }
        let right = self.expect(")")?;
        Ok(AnnotationCall {
            name,
            args,
            source_range: SourceRange::new(start, right.end),
        })
    }
}

pub fn parse_call(source: &str) -> Result<AnnotationCall, ParseError> {
    Parser::new(source).parse_call()
}

pub fn parse_annotation(source: &str) -> Result<Annotation, ParseError> {
    let call = parse_call(source)?;
    if call.name != "annotation" {
        return Err(ParseError("expected annotation(...)".into()));
    }
    Ok(Annotation {
        entries: call.args,
        source_range: call.source_range,
    })
}

fn is_punctuation(text: &str) -> bool {
    text.len() == 1 && "{}()[],;.=:+-*/".contains(text)
}

fn decode_string(raw: &str) -> String {
    let body = raw
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(raw);
    body.replace("\\\"", "\"").replace("\"\"", "\"")
}

#[cfg(test)]
mod tests {
    use super::{AnnotationValue, parse_annotation};

    #[test]
    fn parses_nested_arrays_calls_names_and_numbers() {
        let annotation = parse_annotation(r#"annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}, grid={2,2}), graphics={Rectangle(origin={1,-2.5}, extent={{-10,-20},{10,20}}, fillPattern=FillPattern.Solid), Text(extent={{-80,-20},{80,20}}, textString="%name", textStyle={TextStyle.Bold})}), experiment(StopTime=1e-5))"#).expect("annotation");
        let icon = annotation.entries[0].value.as_call().expect("Icon call");
        let graphics = icon
            .named("graphics")
            .expect("graphics")
            .as_array()
            .expect("graphics array");
        assert_eq!(graphics.len(), 2);
        assert!(matches!(graphics[0], AnnotationValue::Call(_)));
        assert_eq!(
            icon.positional(0)
                .expect("coordinate system")
                .as_call()
                .expect("call")
                .named("grid")
                .expect("grid")
                .as_array()
                .expect("grid array")
                .len(),
            2
        );
    }

    #[test]
    fn parses_boolean_and_qualified_enum_values() {
        let annotation = parse_annotation("annotation(Icon(coordinateSystem(preserveAspectRatio=false), graphics={Line(points={{0,0},{1,1}}, smooth=Smooth.Bezier)}))").expect("annotation");
        let icon = annotation.entries[0].value.as_call().expect("Icon call");
        assert!(
            matches!(icon.positional(0).expect("coordinate").as_call().expect("call").named("preserveAspectRatio"), Some(value) if matches!(value, AnnotationValue::Bool(false)))
        );
        assert!(matches!(
            icon.named("graphics")
                .expect("graphics")
                .as_array()
                .expect("array")[0],
            AnnotationValue::Call(_)
        ));
    }
}
