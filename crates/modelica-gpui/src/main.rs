//! GPUI application entry point.
//!
//! The GPUI dependency is intentionally added in the first application
//! milestone, after the core/render contracts stabilize. Keeping this
//! bootstrap dependency-free lets the workspace compile offline while the
//! GPUI prototype branch is recovered or supplied.

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        println!("modelica-gpui migration workspace is ready; GPUI window bootstrap is pending");
        println!(
            "usage: cargo run -p modelica-gpui -- <file-or-library-directory> [--icon <qualified-name>]"
        );
        return;
    };

    match modelica_core::PackageLoader.load(&path) {
        Ok(package) => {
            let mut registry = modelica_core::LibraryRegistry::default();
            registry.index_package(&package);
            let msl_root = std::path::Path::new("resources/modelica/msl-4.1.0/Modelica");
            if msl_root.is_dir() {
                registry.add(modelica_core::Library {
                    root: msl_root.to_owned(),
                    name: Some("Modelica Standard Library".into()),
                    version: Some("4.1.0".into()),
                    kind: modelica_core::LibraryKind::Builtin,
                    read_only: true,
                });
            }
            if let Ok(source) = std::fs::read_to_string(&package.source_file) {
                let _ = registry.register_source(package.source_file.clone(), source);
            }
            if let Some(icon_name) = args
                .next()
                .filter(|value| value == "--icon")
                .and_then(|_| args.next())
            {
                let Some(class) = find_class(&package, &icon_name) else {
                    eprintln!("class not found: {icon_name}");
                    std::process::exit(2);
                };
                let source = match std::fs::read_to_string(&class.source_file) {
                    Ok(source) => source,
                    Err(error) => {
                        eprintln!("unable to read {}: {error}", class.source_file.display());
                        std::process::exit(1);
                    }
                };
                let _ = registry.register_source(class.source_file.clone(), source.clone());
                let scene =
                    modelica_core::IconResolver::new(&mut registry).resolve(&class, &source);
                println!(
                    "icon {icon_name}: {} graphics, {} diagnostics",
                    scene.graphics.len(),
                    scene.diagnostics.len()
                );
                for diagnostic in scene.diagnostics {
                    println!(
                        "  {:?} {}: {}",
                        diagnostic.severity, diagnostic.code, diagnostic.message
                    );
                }
            } else {
                println!(
                    "loaded {} ({} classes indexed)",
                    package.qualified_name,
                    count_classes(&package)
                );
            }
        }
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            std::process::exit(1);
        }
    }
}

fn find_class(
    package: &modelica_core::PackageNode,
    qualified_name: &str,
) -> Option<modelica_core::Class> {
    for class in &package.classes {
        if class.qualified_name == qualified_name {
            return Some(class.clone());
        }
        if let Some(found) = find_nested_class(class, qualified_name) {
            return Some(found);
        }
    }
    for child in &package.children {
        if let Some(found) = find_class(child, qualified_name) {
            return Some(found);
        }
    }
    None
}

fn find_nested_class(
    class: &modelica_core::Class,
    qualified_name: &str,
) -> Option<modelica_core::Class> {
    for child in &class.children {
        if child.qualified_name == qualified_name {
            return Some(child.clone());
        }
        if let Some(found) = find_nested_class(child, qualified_name) {
            return Some(found);
        }
    }
    None
}

fn count_classes(package: &modelica_core::PackageNode) -> usize {
    package.classes.len() + package.children.iter().map(count_classes).sum::<usize>()
}
