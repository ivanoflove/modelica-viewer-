use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClassKind {
    Package,
    Model,
    Block,
    Connector,
    ExpandableConnector,
    Record,
    Function,
    Type,
    Class,
    Operator,
    OperatorRecord,
    OperatorFunction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

impl SourceRange {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Class {
    pub kind: ClassKind,
    pub name: String,
    pub qualified_name: String,
    pub source_file: PathBuf,
    pub source_range: SourceRange,
    pub children: Vec<Class>,
    pub is_partial: bool,
    pub is_encapsulated: bool,
    pub extends: Vec<String>,
    pub is_short: bool,
    pub base_type_name: Option<String>,
    pub base_prefixes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassLocation {
    pub qualified_name: String,
    pub source_file: PathBuf,
    pub source_range: SourceRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelicaFile {
    pub within: Option<String>,
    pub classes: Vec<Class>,
}
