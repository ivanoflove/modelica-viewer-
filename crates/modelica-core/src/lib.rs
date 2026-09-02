//! Modelica source and semantic data.
//!
//! This crate intentionally has no UI or rendering dependencies. The public
//! scene types are the boundary consumed by `modelica-render` and the GPUI
//! application.

pub mod annotation;
pub mod ast;
pub mod component;
pub mod diagnostics;
pub mod diagram;
pub mod graphics;
pub mod icon;
pub mod lexer;
pub mod library;
pub mod parser;
pub mod resolver;
pub mod scene;
pub mod source;

pub use ast::{Class, ClassKind, ClassLocation, ModelicaFile, SourceRange};
pub use component::{ComponentDeclaration, parse_component_declaration};
pub use diagnostics::{Diagnostic, Severity};
pub use diagram::{DiagramResolver, resolve_diagram};
pub use graphics::{resolve_coordinate_system, resolve_graphic_call, resolve_graphics_from_call};
pub use icon::IconResolver;
pub use library::{Library, LibraryKind, LibraryRegistry, PackageLoader, PackageNode};
pub use parser::{ParseError, parse, requalify_class_tree};
pub use scene::{
    ConnectionKey, ConnectorRef, DiagramDebugStats, DiagramScene, Graphic, GraphicId, GraphicOwner,
    GraphicOwnerKind, IconDebugStats, IconScene, ResolvedGraphic, Transform2D,
};
pub use source::{
    SourceDocument, SourceEdit, SourceTransaction, SourceTransactionError, apply_source_transaction,
};
