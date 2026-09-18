use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{Class, ClassKind, ClassLocation, ModelicaFile, SourceRange};
use crate::diagnostics::Diagnostic;
use crate::parser::{parse, requalify_class_tree};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryKind {
    Project,
    User,
    Builtin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Library {
    pub root: PathBuf,
    pub name: Option<String>,
    pub version: Option<String>,
    pub kind: LibraryKind,
    pub read_only: bool,
}

#[derive(Default)]
pub struct LibraryRegistry {
    libraries: Vec<Library>,
    classes: HashMap<String, ClassLocation>,
    sources: HashMap<PathBuf, String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PackageNode {
    pub name: String,
    pub qualified_name: String,
    pub description: Option<String>,
    pub source_file: PathBuf,
    pub source_range: Option<SourceRange>,
    /// Members in browser/source order. `children` and `classes` remain as
    /// compatibility indexes for callers that only need one kind of member.
    pub ordered_members: Vec<PackageMember>,
    pub children: Vec<PackageNode>,
    pub classes: Vec<Class>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackageMember {
    Package(PackageNode),
    Class(Class),
}

impl PackageMember {
    pub fn name(&self) -> &str {
        match self {
            Self::Package(package) => &package.name,
            Self::Class(class) => &class.name,
        }
    }

    pub fn qualified_name(&self) -> &str {
        match self {
            Self::Package(package) => &package.qualified_name,
            Self::Class(class) => &class.qualified_name,
        }
    }

    pub fn as_package(&self) -> &PackageNode {
        match self {
            Self::Package(package) => package,
            Self::Class(_) => panic!("package member is not a package"),
        }
    }

    pub fn as_class(&self) -> &Class {
        match self {
            Self::Class(class) => class,
            Self::Package(_) => panic!("package member is not a class"),
        }
    }
}

#[derive(Default)]
pub struct PackageLoader;

impl PackageLoader {
    pub fn load(&self, root: impl AsRef<Path>) -> Result<PackageNode, Diagnostic> {
        let root = root.as_ref();
        if root.is_file() {
            if root
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("package.mo"))
                && let Some(directory) = root.parent()
            {
                return self.load_package_directory(directory, None);
            }
            return self.load_standalone(root);
        }
        let package_file = root.join("package.mo");
        if package_file.is_file() {
            return self.load_package_directory(root, None);
        }

        let modelica_files = entries(root, false)
            .into_iter()
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mo"))
            })
            .collect::<Vec<_>>();
        if modelica_files.len() == 1 {
            return self.load_standalone(&modelica_files[0]);
        }
        Err(Diagnostic::warning(
            "MISSING_PACKAGE",
            format!("no package.mo found in {}", root.display()),
        ))
    }

    fn load_standalone(&self, file: &Path) -> Result<PackageNode, Diagnostic> {
        let source = fs::read_to_string(file)
            .map_err(|error| Diagnostic::warning("SOURCE_READ", error.to_string()))?;
        let parsed = parse(&source, file)
            .map_err(|error| Diagnostic::warning("SOURCE_PARSE", error.to_string()))?;
        let root_class = parsed
            .classes
            .iter()
            .find(|class| class.kind == crate::ast::ClassKind::Package)
            .cloned();
        if let Some(class) = root_class {
            return Ok(package_from_class(
                class,
                file.to_owned(),
                parsed.within.as_deref(),
            ));
        }
        let name = file
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("ModelicaFile")
            .to_owned();
        let qualified_name = parsed
            .within
            .as_ref()
            .map_or_else(|| name.clone(), |within| format!("{within}.{name}"));
        Ok(PackageNode {
            name,
            qualified_name,
            description: None,
            source_file: file.to_owned(),
            source_range: None,
            ordered_members: parsed
                .classes
                .iter()
                .cloned()
                .map(PackageMember::Class)
                .collect(),
            children: Vec::new(),
            classes: parsed.classes,
            diagnostics: Vec::new(),
        })
    }

    fn load_package_directory(
        &self,
        directory: &Path,
        parent: Option<&str>,
    ) -> Result<PackageNode, Diagnostic> {
        let package_file = directory.join("package.mo");
        let source = fs::read_to_string(&package_file).map_err(|error| {
            Diagnostic::warning(
                "SOURCE_READ",
                format!("{}: {error}", package_file.display()),
            )
        })?;
        let parsed = parse(&source, &package_file).map_err(|error| {
            Diagnostic::warning(
                "SOURCE_PARSE",
                format!("{}: {error}", package_file.display()),
            )
        })?;
        let directory_name = directory
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Package");
        let package_class = parsed
            .classes
            .iter()
            .find(|class| {
                class.kind == crate::ast::ClassKind::Package && class.name == directory_name
            })
            .or_else(|| {
                parsed
                    .classes
                    .iter()
                    .find(|class| class.kind == crate::ast::ClassKind::Package)
            });
        let (mut package, qualified_name) = if let Some(class) = package_class {
            let qualified = parent.map_or_else(
                || {
                    parsed.within.as_ref().map_or_else(
                        || class.name.clone(),
                        |within| format!("{within}.{}", class.name),
                    )
                },
                |parent| format!("{parent}.{}", class.name),
            );
            (
                package_from_class(class.clone(), package_file.clone(), parent),
                qualified,
            )
        } else {
            let name = directory_name.to_owned();
            let qualified = parent.map_or_else(
                || {
                    parsed
                        .within
                        .clone()
                        .map_or_else(|| name.clone(), |within| format!("{within}.{name}"))
                },
                |parent| format!("{parent}.{name}"),
            );
            let mut package = PackageNode {
                name,
                qualified_name: qualified.clone(),
                description: None,
                source_file: package_file.clone(),
                source_range: None,
                ordered_members: parsed
                    .classes
                    .iter()
                    .cloned()
                    .map(PackageMember::Class)
                    .collect(),
                children: Vec::new(),
                classes: parsed.classes,
                diagnostics: Vec::new(),
            };
            sync_legacy_members(&mut package);
            (package, qualified)
        };

        let mut external_members = Vec::new();
        let mut diagnostics = Vec::new();
        for entry in entries(directory, true) {
            if entry
                .file_name()
                .is_some_and(|name| name == "Resources" || name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if entry.is_dir() && entry.join("package.mo").is_file() {
                match self.load_package_directory(&entry, Some(&qualified_name)) {
                    Ok(child) => external_members.push(PackageMember::Package(child)),
                    Err(error) => diagnostics.push(error),
                }
            } else if entry.is_file()
                && entry
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mo"))
                && entry.file_name().is_some_and(|name| name != "package.mo")
            {
                match parse_file(&entry) {
                    Ok(file) => external_members.extend(file.classes.into_iter().map(|class| {
                        let class = if file.within.is_some() {
                            class
                        } else {
                            requalify_class_tree(class, Some(&qualified_name))
                        };
                        if class.kind == ClassKind::Package {
                            PackageMember::Package(package_from_class(
                                class,
                                entry.clone(),
                                Some(&qualified_name),
                            ))
                        } else {
                            PackageMember::Class(class)
                        }
                    })),
                    Err(error) => diagnostics.push(error),
                }
            }
        }
        let mut members = package.ordered_members;
        members.extend(external_members);
        let package_order = read_package_order(directory);
        package.ordered_members = merge_package_members(members, package_order, &mut diagnostics);
        package.diagnostics.extend(diagnostics);
        sync_legacy_members(&mut package);
        Ok(package)
    }
}

fn entries(directory: &Path, directories_first: bool) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        if directories_first {
            right
                .is_dir()
                .cmp(&left.is_dir())
                .then_with(|| left.cmp(right))
        } else {
            left.cmp(right)
        }
    });
    paths
}

fn parse_file(path: &Path) -> Result<ModelicaFile, Diagnostic> {
    let source = fs::read_to_string(path).map_err(|error| {
        Diagnostic::warning("SOURCE_READ", format!("{}: {error}", path.display()))
    })?;
    parse(&source, path).map_err(|error| {
        Diagnostic::warning("SOURCE_PARSE", format!("{}: {error}", path.display()))
    })
}

fn package_from_class(class: Class, source_file: PathBuf, parent: Option<&str>) -> PackageNode {
    let qualified = parent.map_or_else(
        || class.qualified_name.clone(),
        |parent| format!("{parent}.{}", class.name),
    );
    let class = requalify_root_class(class, &qualified);
    let description = class.description.clone();
    let mut children = Vec::new();
    let mut classes = Vec::new();
    let mut ordered_members = Vec::new();
    for child in class.children {
        if child.kind == ClassKind::Package {
            let package = package_from_class(child, source_file.clone(), Some(&qualified));
            children.push(package.clone());
            ordered_members.push(PackageMember::Package(package));
        } else {
            classes.push(child.clone());
            ordered_members.push(PackageMember::Class(child));
        }
    }
    PackageNode {
        name: class.name,
        qualified_name: qualified,
        description,
        source_file,
        source_range: Some(class.source_range),
        ordered_members,
        children,
        classes,
        diagnostics: Vec::new(),
    }
}

fn sync_legacy_members(package: &mut PackageNode) {
    package.children = package
        .ordered_members
        .iter()
        .filter_map(|member| match member {
            PackageMember::Package(child) => Some(child.clone()),
            PackageMember::Class(_) => None,
        })
        .collect();
    package.classes = package
        .ordered_members
        .iter()
        .filter_map(|member| match member {
            PackageMember::Package(_) => None,
            PackageMember::Class(class) => Some(class.clone()),
        })
        .collect();
}

fn read_package_order(directory: &Path) -> Option<Vec<String>> {
    let source = fs::read_to_string(directory.join("package.order")).ok()?;
    let names = source
        .lines()
        .filter_map(|line| {
            let line = line.split("//").next().unwrap_or_default().trim();
            (!line.is_empty()).then(|| line.to_owned())
        })
        .collect::<Vec<_>>();
    Some(names)
}

fn merge_package_members(
    members: Vec<PackageMember>,
    package_order: Option<Vec<String>>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<PackageMember> {
    let mut unique = Vec::new();
    for member in members {
        if unique
            .iter()
            .any(|candidate: &PackageMember| candidate.name() == member.name())
        {
            diagnostics.push(Diagnostic::warning(
                "DUPLICATE_PACKAGE_MEMBER",
                format!(
                    "duplicate package member `{}`; keeping the first declaration",
                    member.name()
                ),
            ));
        } else {
            unique.push(member);
        }
    }
    let Some(order) = package_order else {
        return unique;
    };
    let mut remaining = unique;
    let mut ordered = Vec::with_capacity(remaining.len());
    for name in order {
        if let Some(index) = remaining.iter().position(|member| member.name() == name) {
            ordered.push(remaining.remove(index));
        } else {
            diagnostics.push(Diagnostic::warning(
                "PACKAGE_ORDER_MISSING_MEMBER",
                format!("package.order refers to missing member `{name}`"),
            ));
        }
    }
    remaining.sort_by(|left, right| left.name().cmp(right.name()));
    if !remaining.is_empty() {
        diagnostics.push(Diagnostic::warning(
            "PACKAGE_ORDER_UNLISTED_MEMBER",
            format!(
                "{} package member(s) were not listed in package.order; using stable name order",
                remaining.len()
            ),
        ));
    }
    ordered.extend(remaining);
    ordered
}

fn requalify_root_class(mut class: Class, qualified_name: &str) -> Class {
    class.qualified_name = qualified_name.to_owned();
    class.children = class
        .children
        .into_iter()
        .map(|child| requalify_class_tree(child, Some(qualified_name)))
        .collect();
    class
}

impl LibraryRegistry {
    pub fn add(&mut self, library: Library) {
        self.libraries.retain(|item| item.root != library.root);
        self.libraries.push(library);
    }

    pub fn libraries(&self) -> &[Library] {
        &self.libraries
    }

    pub fn index_class(&mut self, location: ClassLocation) {
        self.classes
            .insert(location.qualified_name.clone(), location);
    }

    pub fn register_source(
        &mut self,
        path: impl Into<PathBuf>,
        source: impl Into<String>,
    ) -> Result<(), Diagnostic> {
        let path = path.into();
        let source = source.into();
        let parsed = parse(&source, &path).map_err(|error| {
            Diagnostic::warning("SOURCE_PARSE", format!("{}: {error}", path.display()))
        })?;
        self.classes
            .retain(|_, location| location.source_file != path);
        self.sources.insert(path.clone(), source);
        for class in &parsed.classes {
            self.index_class_location(class);
        }
        Ok(())
    }

    pub fn register_package(&mut self, package: &PackageNode) {
        if let Ok(source) = fs::read_to_string(&package.source_file) {
            let _ = self.register_source(package.source_file.clone(), source);
        }
        for member in &package.ordered_members {
            match member {
                PackageMember::Package(child) => self.register_package(child),
                PackageMember::Class(class) => self.register_class_source(class),
            }
        }
    }

    fn register_class_source(&mut self, class: &Class) {
        let source_file = class.source_file.clone();
        if !self.sources.contains_key(&source_file)
            && let Ok(source) = fs::read_to_string(&source_file)
        {
            let _ = self.register_source(source_file, source);
        }
        self.index_class_location(class);
    }

    pub fn source(&self, path: &Path) -> Option<&str> {
        self.sources.get(path).map(String::as_str)
    }

    pub fn resolve_class(&mut self, qualified_name: &str) -> Option<(Class, String)> {
        if !self.classes.contains_key(qualified_name) {
            self.lazy_load_class(qualified_name);
        }
        let location = self.classes.get(qualified_name)?.clone();
        let source = self.sources.get(&location.source_file)?.clone();
        let parsed = parse(&source, &location.source_file).ok()?;
        let class = find_class(&parsed.classes, qualified_name)?;
        Some((class, source))
    }

    pub fn resolve(&self, qualified_name: &str) -> Option<&ClassLocation> {
        self.classes.get(qualified_name)
    }

    pub fn is_read_only(&self, file: &Path) -> bool {
        self.libraries
            .iter()
            .any(|library| library.read_only && file.starts_with(&library.root))
    }

    pub fn index_package(&mut self, package: &PackageNode) {
        self.index_class(ClassLocation {
            qualified_name: package.qualified_name.clone(),
            source_file: package.source_file.clone(),
            source_range: package.source_range.unwrap_or(SourceRange::new(0, 0)),
        });
        for member in &package.ordered_members {
            match member {
                PackageMember::Package(child) => self.index_package(child),
                PackageMember::Class(class) => self.index_class_location(class),
            }
        }
    }

    fn index_class_location(&mut self, class: &Class) {
        self.index_class(ClassLocation {
            qualified_name: class.qualified_name.clone(),
            source_file: class.source_file.clone(),
            source_range: class.source_range,
        });
        for child in &class.children {
            self.index_class_location(child);
        }
    }

    fn lazy_load_class(&mut self, qualified_name: &str) {
        for library in self.libraries.clone() {
            let Some(relative) = relative_class_path(&library.root, qualified_name) else {
                continue;
            };
            let components = relative.components().collect::<Vec<_>>();
            for length in (1..=components.len()).rev() {
                let prefix = components[..length].iter().collect::<PathBuf>();
                let candidates = [
                    library.root.join(prefix.with_extension("mo")),
                    library.root.join(&prefix).join("package.mo"),
                ];
                for candidate in candidates {
                    if let Ok(source) = fs::read_to_string(&candidate)
                        && self.register_source(candidate, source).is_ok()
                        && self.classes.contains_key(qualified_name)
                    {
                        return;
                    }
                }
            }
        }
    }
}

fn find_class(classes: &[Class], qualified_name: &str) -> Option<Class> {
    for class in classes {
        if class.qualified_name == qualified_name {
            return Some(class.clone());
        }
        if let Some(found) = find_class(&class.children, qualified_name) {
            return Some(found);
        }
    }
    None
}

fn relative_class_path(root: &Path, qualified_name: &str) -> Option<PathBuf> {
    let parts = qualified_name.split('.').collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    let root_name = root.file_name()?.to_string_lossy();
    let relative = if root_name == parts[0] {
        &parts[1..]
    } else {
        &parts[..]
    };
    if relative.is_empty() {
        Some(PathBuf::from("package.mo"))
    } else {
        Some(relative.iter().collect::<PathBuf>())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{LibraryRegistry, PackageLoader, PackageMember};

    fn fixture_root() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("modelica-core-{nonce}"));
        fs::create_dir_all(root.join("Electrical/Analog/Basic")).expect("create fixture");
        fs::write(root.join("package.mo"), "package Modelica end Modelica;").expect("write root");
        fs::write(
            root.join("Electrical/package.mo"),
            "within Modelica; package Electrical end Electrical;",
        )
        .expect("write electrical");
        fs::write(
            root.join("Electrical/Analog/package.mo"),
            "within Modelica.Electrical; package Analog end Analog;",
        )
        .expect("write analog");
        fs::write(root.join("Electrical/Analog/Basic/package.mo"), "within Modelica.Electrical.Analog; package Basic model Resistor end Resistor; end Basic;").expect("write basic");
        fs::write(
            root.join("Electrical/Analog/Basic/Extra.mo"),
            "within Modelica.Electrical.Analog.Basic; connector Pin end Pin;",
        )
        .expect("write extra");
        root
    }

    #[test]
    fn scans_directory_layout_and_builds_stable_class_index() {
        let root = fixture_root();
        let package = PackageLoader.load(&root).expect("load fixture");
        assert_eq!(package.qualified_name, "Modelica");
        let mut registry = LibraryRegistry::default();
        registry.index_package(&package);
        assert!(
            registry
                .resolve("Modelica.Electrical.Analog.Basic.Resistor")
                .is_some()
        );
        assert!(
            registry
                .resolve("Modelica.Electrical.Analog.Basic.Pin")
                .is_some()
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn loading_package_mo_scans_its_sibling_package_tree() {
        let root = fixture_root();
        let package = PackageLoader
            .load(root.join("package.mo"))
            .expect("load package.mo as package directory");
        let mut registry = LibraryRegistry::default();
        registry.index_package(&package);
        assert!(
            registry
                .resolve("Modelica.Electrical.Analog.Basic.Resistor")
                .is_some()
        );
        assert!(
            registry
                .resolve("Modelica.Electrical.Analog.Basic.Pin")
                .is_some()
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn registering_package_caches_sources_for_classes_in_separate_files() {
        let root = fixture_root();
        let package = PackageLoader.load(&root).expect("load fixture");
        let mut registry = LibraryRegistry::default();
        registry.index_package(&package);
        registry.register_package(&package);

        let (_, source) = registry
            .resolve_class("Modelica.Electrical.Analog.Basic.Pin")
            .expect("resolve class source");
        assert!(source.contains("connector Pin"));

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn loads_a_single_standalone_modelica_file() {
        let root = std::env::temp_dir().join("modelica-core-standalone-test.mo");
        fs::write(&root, "package Demo model Thing end Thing; end Demo;").expect("write fixture");
        let package = PackageLoader.load(&root).expect("load fixture");
        assert_eq!(package.qualified_name, "Demo");
        assert_eq!(package.classes[0].qualified_name, "Demo.Thing");
        fs::remove_file(root).expect("remove fixture");
    }

    #[test]
    fn re_registering_source_replaces_class_locations_for_that_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("modelica-core-reregister-{nonce}.mo"));
        let initial = "model A end A; model B end B;";
        let updated = "model A end A; model C end C;";
        fs::write(&path, initial).expect("write fixture");
        let mut registry = LibraryRegistry::default();

        registry
            .register_source(path.clone(), initial)
            .expect("register initial source");
        assert!(registry.resolve("A").is_some());
        assert!(registry.resolve("B").is_some());

        registry
            .register_source(path.clone(), updated)
            .expect("register updated source");
        assert!(registry.resolve("A").is_some());
        assert!(registry.resolve("B").is_none());
        assert!(registry.resolve("C").is_some());

        fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn preserves_mixed_package_and_class_order_for_standalone_sources() {
        let root = std::env::temp_dir().join("modelica-core-ordered-standalone-test.mo");
        fs::write(
            &root,
            r#"package P "root"
              model M "model" end M;
              package Q "package" connector C "connector" end C; end Q;
              function F "function" end F;
              record R "record" end R;
              type T = Real "type";
            end P;"#,
        )
        .expect("write fixture");
        let package = PackageLoader.load(&root).expect("load fixture");
        assert_eq!(package.description.as_deref(), Some("root"));
        assert_eq!(
            package
                .ordered_members
                .iter()
                .map(PackageMember::name)
                .collect::<Vec<_>>(),
            ["M", "Q", "F", "R", "T"]
        );
        assert_eq!(
            package.ordered_members[0].as_class().kind,
            crate::ast::ClassKind::Model
        );
        assert_eq!(
            package.ordered_members[1]
                .as_package()
                .description
                .as_deref(),
            Some("package")
        );
        fs::remove_file(root).expect("remove fixture");
    }

    #[test]
    fn applies_package_order_to_directory_members() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("modelica-core-package-order-{nonce}"));
        fs::create_dir_all(&root).expect("create fixture");
        fs::write(root.join("package.mo"), "package P end P;").expect("write package");
        fs::write(root.join("A.mo"), "model A end A;").expect("write A");
        fs::write(root.join("B.mo"), "model B end B;").expect("write B");
        fs::write(root.join("package.order"), "B\nA\n").expect("write package.order");
        let package = PackageLoader.load(&root).expect("load fixture");
        assert_eq!(
            package
                .ordered_members
                .iter()
                .map(PackageMember::name)
                .collect::<Vec<_>>(),
            ["B", "A"]
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn ieh_cpp_fixture_builds_the_source_ordered_browser_tree() {
        let fixture =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/IEH_CPP.mo");
        let package = PackageLoader.load(fixture).expect("load IEH_CPP fixture");
        assert_eq!(package.name, "IEH_CPP");
        assert_eq!(
            package.description.as_deref(),
            Some("远宽综合能源库 (pure C++ thermopack-cxx backend)")
        );
        assert_eq!(
            package
                .ordered_members
                .iter()
                .map(PackageMember::name)
                .collect::<Vec<_>>(),
            [
                "ThermoMedium",
                "Interfaces",
                "FluidUnits",
                "Converter",
                "FMU"
            ]
        );
        let thermo = package.ordered_members[0].as_package();
        assert_eq!(
            thermo.description.as_deref(),
            Some("Thermodynamic medium package — inner/outer Thermopack CPA/Cubic integration")
        );
        assert_eq!(
            thermo
                .ordered_members
                .iter()
                .map(PackageMember::name)
                .collect::<Vec<_>>(),
            ["Types", "Functions", "MediumWorld", "Units", "Examples"]
        );
        assert_eq!(
            thermo.ordered_members[0].as_package().ordered_members[0]
                .as_class()
                .kind,
            crate::ast::ClassKind::Record
        );
        let interfaces = package.ordered_members[1].as_package();
        assert_eq!(
            interfaces.ordered_members[0]
                .as_package()
                .ordered_members
                .iter()
                .map(PackageMember::name)
                .collect::<Vec<_>>(),
            ["FluidPort", "FluidPortIN", "FluidPortOUT"]
        );
        assert_eq!(
            interfaces.ordered_members[0].as_package().ordered_members[0]
                .as_class()
                .kind,
            crate::ast::ClassKind::Connector
        );
        assert_eq!(
            package.ordered_members[3]
                .as_package()
                .description
                .as_deref(),
            Some("能源转换设备库")
        );
        assert_eq!(
            package.ordered_members[4]
                .as_package()
                .description
                .as_deref(),
            Some("test")
        );
    }
}
