//! Ownership-aware resolver boundary.
//!
//! Resolver implementations consume parsed classes and produce scenes; they
//! must never aggregate descendant classes when resolving a selected package
//! or class.

use crate::ast::Class;
use crate::library::LibraryRegistry;
use crate::scene::{DiagramScene, IconScene};

pub fn resolve_icon(class: &Class, source: &str, registry: &mut LibraryRegistry) -> IconScene {
    crate::IconResolver::new(registry).resolve(class, source)
}

pub fn resolve_diagram(
    class: &Class,
    source: &str,
    registry: &mut LibraryRegistry,
) -> DiagramScene {
    crate::diagram::resolve_diagram(class, source, registry)
}
