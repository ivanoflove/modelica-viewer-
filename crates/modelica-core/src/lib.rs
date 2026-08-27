//! Modelica source and semantic data.
//!
//! This crate intentionally has no UI or rendering dependencies. The public
//! scene types are the boundary consumed by `modelica-render` and the GPUI
//! application.

pub mod annotation;
pub mod ast;
pub mod diagnostics;
pub mod graphics;
pub mod icon;
pub mod lexer;
pub mod library;
pub mod parser;
pub mod resolver;
pub mod scene;
pub mod source;

pub use ast::{Class, ClassKind, ClassLocation, ModelicaFile, SourceRange};
pub use diagnostics::{Diagnostic, Severity};
pub use icon::IconResolver;
pub use library::{Library, LibraryKind, LibraryRegistry, PackageLoader, PackageNode};
pub use parser::{ParseError, parse, requalify_class_tree};
pub use scene::{DiagramScene, Graphic, IconScene};
