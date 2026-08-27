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
            index += 1;
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
            | "package"
            | "model"
            | "block"
            | "connector"
            | "record"
            | "function"
            | "class"
            | "type"
            | "partial"
            | "encapsulated"
            | "operator"
            | "extends"
            | "end"
            | "annotation"
            | "equation"
            | "algorithm"
            | "input"
            | "output"
            | "true"
            | "false"
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
}
