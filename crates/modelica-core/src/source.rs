use std::path::Path;

use crate::ast::SourceRange;
use crate::lexer::{Token, tokenize};

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
}
