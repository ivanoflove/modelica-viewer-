use crate::annotation::{AnnotationCall, parse_call};
use crate::ast::Class;
use crate::diagnostics::Diagnostic;
use crate::graphics::resolve_icon_call;
use crate::lexer::{TokenKind, tokenize};
use crate::library::LibraryRegistry;
use crate::scene::{CoordinateSystem, IconScene};

pub struct IconResolver<'a> {
    registry: &'a mut LibraryRegistry,
}

impl<'a> IconResolver<'a> {
    pub fn new(registry: &'a mut LibraryRegistry) -> Self {
        Self { registry }
    }

    pub fn resolve(&mut self, class: &Class, source: &str) -> IconScene {
        let mut visiting = Vec::new();
        self.resolve_inner(class, source, &mut visiting)
    }

    fn resolve_inner(
        &mut self,
        class: &Class,
        source: &str,
        visiting: &mut Vec<String>,
    ) -> IconScene {
        if visiting.iter().any(|name| name == &class.qualified_name) {
            return empty_scene(
                class,
                vec![Diagnostic::warning(
                    "ICON_INHERITANCE_CYCLE",
                    format!("Icon inheritance cycle at {}", class.qualified_name),
                )],
            );
        }
        visiting.push(class.qualified_name.clone());
        let own = find_own_icon(class, source);
        let mut diagnostics = Vec::new();
        let mut inherited: Option<IconScene> = None;
        for base_name in &class.extends {
            let Some((base_class, base_source)) = self.resolve_base(class, base_name) else {
                diagnostics.push(Diagnostic::warning(
                    "ICON_BASE_NOT_FOUND",
                    format!("unable to resolve Icon base `{base_name}`"),
                ));
                continue;
            };
            let base = self.resolve_inner(&base_class, &base_source, visiting);
            inherited = Some(match inherited.take() {
                None => base,
                Some(mut current) => {
                    current.graphics.extend(base.graphics);
                    current.diagnostics.extend(base.diagnostics);
                    current
                }
            });
        }
        visiting.pop();

        let mut result = match (inherited, own) {
            (None, None) => empty_scene(class, diagnostics),
            (Some(mut base), None) => {
                base.owner_qualified_name = Some(class.qualified_name.clone());
                base.diagnostics.extend(diagnostics);
                base
            }
            (None, Some((_, _, mut own))) => {
                own.owner_qualified_name = Some(class.qualified_name.clone());
                own.diagnostics.extend(diagnostics);
                own
            }
            (Some(base), Some((has_coordinate_system, _, own))) => {
                let coordinate_system = if has_coordinate_system {
                    own.coordinate_system
                } else {
                    base.coordinate_system
                };
                let mut merged = IconScene {
                    owner_qualified_name: Some(class.qualified_name.clone()),
                    coordinate_system,
                    graphics: base.graphics,
                    diagnostics: base.diagnostics,
                };
                merged.graphics.extend(own.graphics);
                merged.diagnostics.extend(own.diagnostics);
                merged.diagnostics.extend(diagnostics);
                merged
            }
        };
        for diagnostic in &mut result.diagnostics {
            diagnostic.owner = Some(class.qualified_name.clone());
        }
        result
    }

    fn resolve_base(&mut self, class: &Class, base_name: &str) -> Option<(Class, String)> {
        let mut candidates = vec![base_name.to_owned()];
        let parts = class.qualified_name.split('.').collect::<Vec<_>>();
        for length in (1..parts.len()).rev() {
            candidates.push(format!("{}.{}", parts[..length].join("."), base_name));
        }
        candidates.dedup();
        candidates
            .into_iter()
            .find_map(|candidate| self.registry.resolve_class(&candidate))
    }
}

fn empty_scene(class: &Class, mut diagnostics: Vec<Diagnostic>) -> IconScene {
    let mut scene = IconScene {
        owner_qualified_name: Some(class.qualified_name.clone()),
        coordinate_system: CoordinateSystem::default(),
        graphics: Vec::new(),
        diagnostics: Vec::new(),
    };
    scene.diagnostics.append(&mut diagnostics);
    scene
}

fn find_own_icon(class: &Class, source: &str) -> Option<(bool, AnnotationCall, IconScene)> {
    let range = class.source_range;
    let class_source = source.get(range.start..range.end)?;
    let owned = mask_child_ranges(class, source, class_source);
    let tokens = tokenize(&owned);
    for (index, token) in tokens.iter().enumerate() {
        if token.text != "annotation" || token.kind != TokenKind::Keyword {
            continue;
        }
        let open_index = next_significant(&tokens, index + 1)?;
        if tokens[open_index].text != "(" {
            continue;
        }
        let close_index = matching_paren(&tokens, open_index)?;
        let call_source = owned.get(token.start..tokens[close_index].end)?;
        let Ok(annotation) = parse_call(call_source) else {
            continue;
        };
        let Some(icon) = annotation
            .args
            .iter()
            .find_map(|entry| entry.value.as_call().filter(|call| call.name == "Icon"))
        else {
            continue;
        };
        let has_coordinate_system = icon.named("coordinateSystem").is_some()
            || icon.named("extent").is_some()
            || icon.args.iter().any(|entry| {
                entry.name.is_none()
                    && entry
                        .value
                        .as_call()
                        .is_some_and(|call| call.name == "coordinateSystem")
            });
        let scene = resolve_icon_call(icon);
        return Some((has_coordinate_system, annotation, scene));
    }
    None
}

fn mask_child_ranges(class: &Class, source: &str, class_source: &str) -> String {
    let mut bytes = class_source.as_bytes().to_vec();
    for child in &class.children {
        let start = child
            .source_range
            .start
            .saturating_sub(class.source_range.start)
            .min(bytes.len());
        let end = child
            .source_range
            .end
            .saturating_sub(class.source_range.start)
            .min(bytes.len());
        for byte in &mut bytes[start..end] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    let _ = source;
    String::from_utf8(bytes).expect("source masking preserves UTF-8")
}

fn next_significant(tokens: &[crate::lexer::Token], mut index: usize) -> Option<usize> {
    while index < tokens.len()
        && matches!(
            tokens[index].kind,
            TokenKind::Whitespace | TokenKind::Comment
        )
    {
        index += 1;
    }
    (index < tokens.len()).then_some(index)
}

fn matching_paren(tokens: &[crate::lexer::Token], open: usize) -> Option<usize> {
    let mut depth = 0_i32;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if token.text == "(" {
            depth += 1;
        }
        if token.text == ")" {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::IconResolver;
    use crate::library::LibraryRegistry;
    use crate::parser::parse;
    use crate::scene::Graphic;

    #[test]
    fn resolves_only_selected_class_icon_and_masks_child_icons() {
        let source = "package P model Boundary annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})})); end Boundary; model Child annotation(Icon(graphics={Ellipse(extent={{-20,-20},{20,20}})})); end Child; end P;";
        let file = parse(source, "P.mo").expect("parse");
        let package = &file.classes[0];
        let boundary = &package.children[0];
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("P.mo", source)
            .expect("index source");
        let scene = IconResolver::new(&mut registry).resolve(boundary, source);
        assert_eq!(scene.graphics.len(), 1);
        assert!(matches!(scene.graphics[0], Graphic::Rectangle(_)));
    }

    #[test]
    fn merges_extends_icons_and_detects_cycles() {
        let source = "model Base annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})})); end Base; model Child extends Base annotation(Icon(graphics={Ellipse(extent={{-5,-5},{5,5}})})); end Child;";
        let file = parse(source, "Icons.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("Icons.mo", source)
            .expect("index source");
        let scene = IconResolver::new(&mut registry).resolve(&file.classes[1], source);
        assert_eq!(scene.graphics.len(), 2);
    }

    #[test]
    fn lazily_resolves_real_input_from_bundled_msl() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/modelica/msl-4.1.0/Modelica");
        let mut registry = LibraryRegistry::default();
        registry.add(crate::library::Library {
            root,
            name: Some("Modelica Standard Library".into()),
            version: Some("4.1.0".into()),
            kind: crate::library::LibraryKind::Builtin,
            read_only: true,
        });
        let (class, source) = registry
            .resolve_class("Modelica.Blocks.Interfaces.RealInput")
            .expect("bundled RealInput");
        assert!(class.is_short);
        let scene = IconResolver::new(&mut registry).resolve(&class, &source);
        assert_eq!(scene.graphics.len(), 1);
        assert!(scene.diagnostics.is_empty(), "{:?}", scene.diagnostics);
    }

    #[test]
    fn skips_non_icon_annotations_before_class_icon() {
        let source = r#"model WithDialog
  parameter Real value = 1 annotation(Dialog(tab = "General"));
  annotation(
    Documentation(info = "metadata"),
    Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})})
  );
end WithDialog;"#;
        let file = parse(source, "WithDialog.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("WithDialog.mo", source)
            .expect("index source");
        let scene = IconResolver::new(&mut registry).resolve(&file.classes[0], source);
        assert_eq!(scene.graphics.len(), 1);
    }

    #[test]
    fn resolves_builtin_icon_inheritance_by_qualified_name() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/modelica/msl-4.1.0/Modelica");
        let mut registry = LibraryRegistry::default();
        registry.add(crate::library::Library {
            root,
            name: Some("Modelica Standard Library".into()),
            version: Some("4.1.0".into()),
            kind: crate::library::LibraryKind::Builtin,
            read_only: true,
        });
        let (class, source) = registry
            .resolve_class("Modelica.Icons.ExamplesPackage")
            .expect("bundled ExamplesPackage");
        let scene = IconResolver::new(&mut registry).resolve(&class, &source);
        assert!(!scene.graphics.is_empty());
        assert!(
            !scene
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "ICON_BASE_NOT_FOUND"),
            "{:?}",
            scene.diagnostics
        );
    }
}
