//! A lossless-friendly lexical boundary for the future parser and highlighter.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Identifier,
    Keyword,
    String,
    Number,
    Punctuation,
    Comment,
    Whitespace,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// Tokenize enough lexical structure for parser/highlighter implementations to
/// share one source coordinate system. Trivia is retained deliberately.
pub fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        let start = index;
        let byte = bytes[index];

        if byte.is_ascii_whitespace() {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            tokens.push(token(TokenKind::Whitespace, source, start, index));
        } else if bytes.get(index..index + 2) == Some(b"//") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            tokens.push(token(TokenKind::Comment, source, start, index));
        } else if bytes.get(index..index + 2) == Some(b"/*") {
            index += 2;
            while index + 1 < bytes.len() && &bytes[index..index + 2] != b"*/" {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            tokens.push(token(TokenKind::Comment, source, start, index));
        } else if byte == b'"' {
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' && bytes.get(index + 1) == Some(&b'"') {
                    index += 2;
                    continue;
                }
                if bytes[index] == b'"' {
                    index += 1;
                    if index < bytes.len() && bytes[index] == b'"' {
                        index += 1;
                        continue;
                    }
                    break;
                }
                index += 1;
            }
            tokens.push(token(TokenKind::String, source, start, index));
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            let kind = if is_keyword(&source[start..index]) {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            tokens.push(token(kind, source, start, index));
        } else if byte.is_ascii_digit()
            || (byte == b'.'
                && bytes
                    .get(index + 1)
                    .is_some_and(|value| value.is_ascii_digit()))
        {
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || matches!(bytes[index], b'.' | b'+' | b'-'))
            {
                index += 1;
            }
            tokens.push(token(TokenKind::Number, source, start, index));
        } else if b"{}()[],;.=:+-*/".contains(&byte) {
            index += 1;
            tokens.push(token(TokenKind::Punctuation, source, start, index));
        } else {
            // Unknown bytes may be the leading byte of a multi-byte UTF-8
            // character. Advance by the character width so token ranges stay
            // valid string boundaries for diagnostics and source highlighting.
            index += source[index..].chars().next().map_or(1, char::len_utf8);
            tokens.push(token(TokenKind::Unknown, source, start, index));
        }
    }

    tokens
}

fn token(kind: TokenKind, source: &str, start: usize, end: usize) -> Token {
    Token {
        kind,
        text: source[start..end].to_owned(),
        start,
        end,
    }
}

fn is_keyword(value: &str) -> bool {
    matches!(
        value,
        "within"
            | "algorithm"
            | "and"
            | "annotation"
            | "assert"
            | "block"
            | "break"
            | "class"
            | "connect"
            | "connector"
            | "constrainedby"
            | "constant"
            | "der"
            | "discrete"
            | "each"
            | "else"
            | "elseif"
            | "elsewhen"
            | "end"
            | "enumeration"
            | "equation"
            | "expandable"
            | "extends"
            | "external"
            | "false"
            | "final"
            | "flow"
            | "for"
            | "function"
            | "if"
            | "import"
            | "impure"
            | "in"
            | "initial"
            | "inner"
            | "input"
            | "loop"
            | "model"
            | "not"
            | "operator"
            | "or"
            | "outer"
            | "output"
            | "package"
            | "parameter"
            | "partial"
            | "protected"
            | "public"
            | "pure"
            | "record"
            | "redeclare"
            | "reinit"
            | "replaceable"
            | "return"
            | "stream"
            | "terminate"
            | "then"
            | "true"
            | "type"
            | "when"
            | "while"
            | "encapsulated"
    )
}

#[cfg(test)]
mod tests {
    use super::{TokenKind, tokenize};

    #[test]
    fn retains_comments_and_strings_without_fake_identifiers() {
        let tokens = tokenize(
            "// package Fake\nmodel RealModel\n  annotation(Documentation(info=\"end Fake;\"));\nend RealModel;",
        );
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Comment));
        assert!(tokens.iter().any(|token| token.kind == TokenKind::String));
        assert!(
            !tokens
                .iter()
                .any(|token| token.text == "Fake" && token.kind == TokenKind::Identifier)
        );
    }

    #[test]
    fn preserves_utf8_boundaries_for_unknown_characters() {
        let tokens = tokenize("model A©;");
        let copyright = tokens
            .iter()
            .find(|token| token.text == "©")
            .expect("unicode character token");
        assert_eq!(copyright.kind, TokenKind::Unknown);
        assert_eq!(&"model A©;"[copyright.start..copyright.end], "©");
    }

    #[test]
    fn recognizes_modelica_control_and_declaration_keywords() {
        let tokens = tokenize("within P; model M parameter Real x; equation connect(a,b); end M;");
        for keyword in ["within", "model", "parameter", "equation", "connect", "end"] {
            assert!(
                tokens
                    .iter()
                    .any(|token| token.text == keyword && token.kind == TokenKind::Keyword)
            );
        }
    }
}
