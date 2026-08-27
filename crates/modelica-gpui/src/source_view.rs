use modelica_core::lexer::{TokenKind, tokenize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HighlightKind {
    Keyword,
    Identifier,
    Number,
    String,
    Comment,
    Operator,
    Punctuation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub kind: HighlightKind,
}

pub fn highlight_spans(source: &str) -> Vec<HighlightSpan> {
    tokenize(source)
        .into_iter()
        .filter_map(|token| {
            let kind = match token.kind {
                TokenKind::Keyword => HighlightKind::Keyword,
                TokenKind::Identifier => HighlightKind::Identifier,
                TokenKind::Number => HighlightKind::Number,
                TokenKind::String => HighlightKind::String,
                TokenKind::Comment => HighlightKind::Comment,
                TokenKind::Punctuation if is_operator(&token.text) => HighlightKind::Operator,
                TokenKind::Punctuation | TokenKind::Unknown => HighlightKind::Punctuation,
                TokenKind::Whitespace => return None,
            };
            Some(HighlightSpan {
                start: token.start,
                end: token.end,
                kind,
            })
        })
        .collect()
}

fn is_operator(text: &str) -> bool {
    text.chars().all(|character| "=:+-*/".contains(character))
}

#[cfg(test)]
mod tests {
    use super::{HighlightKind, highlight_spans};

    #[test]
    fn highlights_modelica_tokens_without_changing_ranges() {
        let source = "model Demo\n  parameter Real value = 2; // note";
        let spans = highlight_spans(source);
        assert_eq!(&source[spans[0].start..spans[0].end], "model");
        assert_eq!(spans[0].kind, HighlightKind::Keyword);
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Number));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Comment));
        assert!(spans.windows(2).all(|pair| pair[0].end <= pair[1].start));
    }
}
