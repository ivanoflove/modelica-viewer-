use std::path::Path;

use crate::ast::SourceRange;
use crate::lexer::{Token, tokenize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEdit {
    pub start: usize,
    pub end: usize,
    pub expected_text: Option<String>,
    pub replacement: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransaction {
    pub edits: Vec<SourceEdit>,
    pub source_version: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceTransactionError {
    InvalidEditRange {
        start: usize,
        end: usize,
    },
    StaleSourceRange {
        start: usize,
        end: usize,
        expected: String,
        actual: String,
    },
    SourceVersionMismatch {
        expected: u64,
        current: u64,
    },
    OverlappingEdits,
}

impl std::fmt::Display for SourceTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEditRange { start, end } => {
                write!(formatter, "invalid source edit range {start}:{end}")
            }
            Self::StaleSourceRange {
                start,
                end,
                expected,
                actual,
            } => write!(
                formatter,
                "stale source range at {start}:{end}; expected {expected:?}, actual {actual:?}"
            ),
            Self::SourceVersionMismatch { expected, current } => write!(
                formatter,
                "source version mismatch: expected {expected}, current {current}"
            ),
            Self::OverlappingEdits => formatter.write_str("source edits overlap"),
        }
    }
}

impl std::error::Error for SourceTransactionError {}

/// Validate all byte ranges before changing the source, then apply edits from
/// back to front so earlier ranges remain stable.
pub fn apply_source_transaction(
    source: &str,
    transaction: &SourceTransaction,
    current_version: Option<u64>,
) -> Result<String, SourceTransactionError> {
    if let (Some(expected), Some(current)) = (transaction.source_version, current_version)
        && expected != current
    {
        return Err(SourceTransactionError::SourceVersionMismatch { expected, current });
    }

    let mut edits = transaction.edits.clone();
    edits.sort_by_key(|edit| edit.start);
    let mut previous_end = 0;
    for edit in &edits {
        if edit.start > edit.end
            || edit.end > source.len()
            || source.get(edit.start..edit.end).is_none()
        {
            return Err(SourceTransactionError::InvalidEditRange {
                start: edit.start,
                end: edit.end,
            });
        }
        if edit.start < previous_end {
            return Err(SourceTransactionError::OverlappingEdits);
        }
        if let Some(expected) = &edit.expected_text {
            let actual = source.get(edit.start..edit.end).unwrap_or_default();
            if actual != expected {
                return Err(SourceTransactionError::StaleSourceRange {
                    start: edit.start,
                    end: edit.end,
                    expected: expected.clone(),
                    actual: actual.to_owned(),
                });
            }
        }
        previous_end = edit.end;
    }

    let mut result = source.to_owned();
    for edit in edits.iter().rev() {
        result.replace_range(edit.start..edit.end, &edit.replacement);
    }
    Ok(result)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDocument {
    pub path: std::path::PathBuf,
    pub text: String,
    pub version: u64,
}

impl SourceDocument {
    pub fn new(path: impl Into<std::path::PathBuf>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
            version: 0,
        }
    }

    pub fn tokens(&self) -> Vec<Token> {
        tokenize(&self.text)
    }

    pub fn slice(&self, range: SourceRange) -> Option<&str> {
        self.text.get(range.start..range.end)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn apply(&mut self, transaction: &SourceTransaction) -> Result<(), SourceTransactionError> {
        self.text = apply_source_transaction(&self.text, transaction, Some(self.version))?;
        self.version = self.version.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SourceDocument, SourceEdit, SourceTransaction, SourceTransactionError,
        apply_source_transaction,
    };

    fn edit(start: usize, end: usize, expected_text: &str, replacement: &str) -> SourceEdit {
        SourceEdit {
            start,
            end,
            expected_text: Some(expected_text.into()),
            replacement: replacement.into(),
        }
    }

    #[test]
    fn applies_multiple_edits_back_to_front() {
        let transaction = SourceTransaction {
            edits: vec![edit(0, 1, "a", "A"), edit(2, 3, "c", "C")],
            source_version: None,
        };
        assert_eq!(
            apply_source_transaction("abc", &transaction, None).expect("transaction"),
            "AbC"
        );
    }

    #[test]
    fn rejects_stale_and_overlapping_ranges() {
        let stale = SourceTransaction {
            edits: vec![edit(0, 1, "x", "A")],
            source_version: None,
        };
        assert!(matches!(
            apply_source_transaction("abc", &stale, None),
            Err(SourceTransactionError::StaleSourceRange { .. })
        ));

        let overlapping = SourceTransaction {
            edits: vec![edit(0, 2, "ab", "A"), edit(1, 3, "bc", "B")],
            source_version: None,
        };
        assert_eq!(
            apply_source_transaction("abc", &overlapping, None),
            Err(SourceTransactionError::OverlappingEdits)
        );
    }

    #[test]
    fn document_increments_version_after_successful_apply() {
        let mut document = SourceDocument::new("Demo.mo", "model Demo end Demo;");
        let transaction = SourceTransaction {
            edits: vec![SourceEdit {
                start: 6,
                end: 10,
                expected_text: Some("Demo".into()),
                replacement: "Thing".into(),
            }],
            source_version: Some(0),
        };
        document.apply(&transaction).expect("transaction");
        assert_eq!(document.text, "model Thing end Demo;");
        assert_eq!(document.version, 1);
    }
}
