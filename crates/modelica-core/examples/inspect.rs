use std::{env, path::PathBuf, process};

use modelica_core::{
    IconResolver, Library, LibraryKind, LibraryRegistry, PackageLoader, PackageNode,
    resolve_diagram,
};

fn main() {
    let mut args = env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!(
            "usage: cargo run -p modelica-core --example inspect -- <file-or-directory> [qualified-class]"
        );
        process::exit(2);
    };
    let input = PathBuf::from(input);
    let package = match PackageLoader.load(&input) {
        Ok(package) => package,
        Err(diagnostic) => {
            eprintln!("{}: {}", diagnostic.code, diagnostic.message);
            process::exit(1);
        }
    };

    let mut registry = LibraryRegistry::default();
    add_bundled_msl(&mut registry);
    registry.index_package(&package);
    registry.register_package(&package);

    let class_count = count_classes(&package);
    println!(
        "package={} classes={} children={} diagnostics={}",
        package.qualified_name,
        class_count,
        package.children.len(),
        package.diagnostics.len()
    );

    let Some(target) = args.next() else {
        return;
    };
    let Some((class, source)) = registry.resolve_class(&target) else {
        eprintln!("class not found: {target}");
        process::exit(1);
    };
    let scene = IconResolver::new(&mut registry).resolve(&class, &source);
    let stats = scene.debug_stats();
    println!(
        "class={} icon_graphics={} own={} inherited={} connectors={} editable={} diagnostics={}",
        class.qualified_name,
        scene.graphics.len(),
        stats.own_graphics,
        stats.inherited_graphics,
        stats.connector_graphics,
        stats.editable_graphics,
        scene.diagnostics.len()
    );
    for diagnostic in scene.diagnostics {
        println!("diagnostic {}: {}", diagnostic.code, diagnostic.message);
    }
    let diagram = resolve_diagram(&class, &source, &mut registry);
    println!(
        "diagram_background={} components={} connections={} diagnostics={}",
        diagram.background_graphics.len(),
        diagram.components.len(),
        diagram.connections.len(),
        diagram.diagnostics.len()
    );
}

fn add_bundled_msl(registry: &mut LibraryRegistry) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../resources/modelica/msl-4.1.0/Modelica");
    if root.is_dir() {
        registry.add(Library {
            root,
            name: Some("Modelica Standard Library".into()),
            version: Some("4.1.0".into()),
            kind: LibraryKind::Builtin,
            read_only: true,
        });
    }
}

fn count_classes(package: &PackageNode) -> usize {
    package.classes.len() + package.children.iter().map(count_classes).sum::<usize>()
}
