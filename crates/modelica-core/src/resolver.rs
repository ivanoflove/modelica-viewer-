//! Ownership-aware resolver boundary.
//!
//! Resolver implementations will consume parsed classes and produce scenes;
//! they must never aggregate descendant classes when resolving a selected
//! package or class.

use crate::ast::Class;
use crate::diagnostics::Diagnostic;
use crate::scene::{DiagramScene, IconScene};

pub fn resolve_icon(_class: &Class) -> Result<IconScene, Diagnostic> {
    Err(Diagnostic::warning(
        "CORE_RESOLVER_PENDING",
        "Icon resolver migration is not implemented yet",
    ))
}

pub fn resolve_diagram(_class: &Class) -> Result<DiagramScene, Diagnostic> {
    Err(Diagnostic::warning(
        "CORE_RESOLVER_PENDING",
        "Diagram resolver migration is not implemented yet",
    ))
}
