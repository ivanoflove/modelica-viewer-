use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    path::{Path as FsPath, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use bytemuck::{Pod, Zeroable};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Layout, Margin,
    Pos2, RichText, Rounding, Sense, Stroke, Vec2,
};
use egui_wgpu::ScreenDescriptor;
use lyon::{
    math::point,
    path::Path,
    tessellation::{
        BuffersBuilder, FillOptions, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator,
        StrokeVertex, VertexBuffers,
    },
};
use modelica_core::annotation::{parse_call, AnnotationCall, AnnotationValue};
use modelica_core::scene::{
    ComponentInstance as CoreComponentInstance, ConnectionKey, DiagramScene as CoreDiagramScene,
    EllipseGraphic, Graphic as CoreGraphic, GraphicOwnerKind, IconScene as CoreIconScene,
    LineGraphic, Point as CorePoint, PolygonGraphic, RectangleGraphic, ResolvedGraphic,
    Transform2D,
};
use modelica_core::{
    apply_source_transaction,
    lexer::{tokenize, Token, TokenKind},
    parse, resolve_diagram, Class, IconResolver, Library, LibraryKind, LibraryRegistry,
    PackageLoader, PackageNode, SourceEdit, SourceRange, SourceTransaction,
};
use modelica_render::{line_local_to_world, resolved_graphic_contains_point, world_to_line_local};
use rfd::FileDialog;
use wgpu::util::DeviceExt;
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Window, WindowBuilder},
};

const MSAA_SAMPLES: u32 = 4;
const INITIAL_ZOOM: f32 = 3.0;
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 24.0;
const ORTHOGONAL_EPSILON: f32 = 0.001;
const UI_FONT_MEDIUM: &str = "modelica-ui-medium";
const UI_FONT_SEMIBOLD: &str = "modelica-ui-semibold";
const UI_FONT_SYMBOLS: &str = "modelica-ui-symbols";

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    local: [f32; 2],
}

impl Vertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewUniform {
    viewport: [f32; 4],
    view: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BackgroundUniform {
    top_left: [f32; 4],
    top_right: [f32; 4],
    bottom_left: [f32; 4],
    bottom_right: [f32; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThemeMode {
    System,
    Light,
    Dark,
}

impl ThemeMode {
    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn is_dark(self, system_theme: Option<winit::window::Theme>) -> bool {
        match self {
            Self::System => matches!(system_theme, Some(winit::window::Theme::Dark)),
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccentTheme {
    Violet,
    Blue,
    Cyan,
    Orange,
}

impl AccentTheme {
    fn label(self) -> &'static str {
        match self {
            Self::Violet => "Violet",
            Self::Blue => "Blue",
            Self::Cyan => "Cyan",
            Self::Orange => "Orange",
        }
    }

    fn id(self) -> u8 {
        match self {
            Self::Violet => 0,
            Self::Blue => 1,
            Self::Cyan => 2,
            Self::Orange => 3,
        }
    }

    fn from_id(id: u8) -> Self {
        match id {
            1 => Self::Blue,
            2 => Self::Cyan,
            3 => Self::Orange,
            _ => Self::Violet,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct StyleUniform {
    color: [f32; 4],
    edge_color: [f32; 4],
    gradient: [f32; 4],
    mode: u32,
    // WGSL rounds the vec3 tail and the enclosing uniform struct to 16-byte
    // alignment. Keep the host-side buffer at the shader's 80-byte size.
    // Rust arrays are tightly packed while WGSL aligns the trailing vec3 to
    // a 16-byte boundary. The extra words keep the uploaded buffer at the
    // shader's 80-byte size.
    _padding: [u32; 7],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MainView {
    Source,
    Icon,
    Diagram,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum DiagramSelection {
    #[default]
    None,
    Component(String),
    Connection(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResizeHandle {
    Corner(usize),
}

#[derive(Clone, Copy, Debug)]
struct ComponentSelectionOverlay {
    origin: CorePoint,
    extent: modelica_core::scene::Extent,
    rotation: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionSegmentOrientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug)]
enum ConnectionHitTarget {
    Segment {
        index: usize,
        orientation: ConnectionSegmentOrientation,
    },
    Line,
}

#[derive(Clone, Debug)]
struct ConnectionHit {
    connection_id: String,
    target: ConnectionHitTarget,
}

#[derive(Clone, Debug)]
enum PointerInteraction {
    None,
    Pan {
        button: MouseButton,
        start_pointer: PhysicalPosition<f64>,
        start_pan: [f32; 2],
    },
    MoveIconGraphic {
        button: MouseButton,
        graphic_id: String,
        start_pointer_model: CorePoint,
        original_geometry: CoreGraphic,
        preview_delta: CorePoint,
        source_before: String,
    },
    MoveDiagramComponent {
        button: MouseButton,
        component_id: String,
        component_name: String,
        start_pointer_model: CorePoint,
        original_origin: CorePoint,
        preview_delta: CorePoint,
        connected_connections: Vec<ConnectionDragSnapshot>,
        source_before: String,
    },
    MoveDiagramConnectionSegment {
        button: MouseButton,
        connection_id: String,
        connection_key: ConnectionKey,
        segment_index: usize,
        orientation: ConnectionSegmentOrientation,
        line_origin: CorePoint,
        line_rotation: f32,
        start_pointer_model: CorePoint,
        original_points: Vec<CorePoint>,
        preview_points: Vec<CorePoint>,
        source_before: String,
    },
    ResizeDiagramComponent {
        button: MouseButton,
        component_id: String,
        component_name: String,
        handle: ResizeHandle,
        original_component: CoreComponentInstance,
        original_extent: modelica_core::scene::Extent,
        preview_extent: modelica_core::scene::Extent,
        connected_connections: Vec<ConnectionDragSnapshot>,
        source_before: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionEndpoint {
    Lhs,
    Rhs,
    Both,
}

#[derive(Clone, Debug)]
struct ConnectionDragSnapshot {
    connection_id: String,
    connection_key: ConnectionKey,
    endpoint: ConnectionEndpoint,
    original_line_points: Vec<CorePoint>,
    original_line_origin: CorePoint,
    original_line_rotation: f32,
    lhs_connector_path: Option<String>,
    rhs_connector_path: Option<String>,
}

#[derive(Clone, Debug)]
struct ConnectionLineEdit {
    connection_key: ConnectionKey,
    before_points: Vec<CorePoint>,
    after_points: Vec<CorePoint>,
    line_origin: CorePoint,
}

#[derive(Clone, Debug)]
enum EditCommand {
    MoveIconGraphic {
        class_name: String,
        graphic_id: String,
        before_geometry: CoreGraphic,
        after_geometry: CoreGraphic,
        before_source: String,
        after_source: String,
    },
    MoveDiagramComponent {
        class_name: String,
        component_id: String,
        before_origin: CorePoint,
        after_origin: CorePoint,
        before_source: String,
        after_source: String,
        connection_edits: Vec<ConnectionLineEdit>,
    },
    MoveDiagramConnection {
        class_name: String,
        connection_key: ConnectionKey,
        before_points: Vec<CorePoint>,
        after_points: Vec<CorePoint>,
    },
    ResizeDiagramComponent {
        class_name: String,
        component_id: String,
        before_extent: modelica_core::scene::Extent,
        after_extent: modelica_core::scene::Extent,
        before_source: String,
        after_source: String,
        connection_edits: Vec<ConnectionLineEdit>,
    },
}

impl MainView {
    fn label(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::Icon => "Icon",
            Self::Diagram => "Diagram",
        }
    }
}

#[derive(Clone, Debug)]
struct LoadedDocument {
    path: PathBuf,
    package_name: String,
    class_names: Vec<String>,
    diagnostics: usize,
    icons: Vec<(String, CoreIconScene)>,
    diagrams: Vec<(String, CoreDiagramScene)>,
    class_sources: Vec<ClassSource>,
    source_overrides: HashMap<String, String>,
    source_versions: HashMap<String, u64>,
}

#[derive(Clone, Debug)]
struct ClassSource {
    qualified_name: String,
    source_file: PathBuf,
    source_range: SourceRange,
}

#[derive(Clone, Debug)]
struct TreeNode {
    name: String,
    qualified_name: String,
    class_name: Option<String>,
    children: Vec<TreeNode>,
}

#[derive(Clone, Debug)]
struct UiDocument {
    package_name: String,
    class_names: Vec<String>,
    tree: TreeNode,
    selected_class: Option<String>,
    icon_graphics: usize,
    diagram_background: usize,
    diagram_components: usize,
    diagram_own_components: usize,
    diagram_inherited_components: usize,
    diagram_connectors: usize,
    diagram_unresolved_components: usize,
    diagram_unresolved_bases: usize,
    diagram_connections: usize,
    source_name: String,
    source_lines: Vec<String>,
}

impl LoadedDocument {
    fn load(path: &FsPath) -> Result<Self, String> {
        fs::read_to_string(path).map_err(|error| error.to_string())?;
        let package = PackageLoader
            .load(path)
            .map_err(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))?;
        let mut class_names = Vec::new();
        collect_class_names(&package, &mut class_names);
        class_names.sort();
        let mut class_sources = Vec::new();
        collect_class_sources(&package, &mut class_sources);
        let mut registry = LibraryRegistry::default();
        add_bundled_msl(&mut registry);
        registry.index_package(&package);
        registry.register_package(&package);
        let icons = class_names
            .iter()
            .filter_map(|qualified_name| {
                let (class, source) = registry.resolve_class(qualified_name)?;
                Some((
                    qualified_name.clone(),
                    IconResolver::new(&mut registry).resolve(&class, &source),
                ))
            })
            .collect::<Vec<_>>();
        let diagrams = class_names
            .iter()
            .filter_map(|qualified_name| {
                let (class, source) = registry.resolve_class(qualified_name)?;
                Some((
                    qualified_name.clone(),
                    resolve_diagram(&class, &source, &mut registry),
                ))
            })
            .collect::<Vec<_>>();
        Ok(Self {
            path: path.to_owned(),
            package_name: package.qualified_name,
            class_names,
            diagnostics: package.diagnostics.len(),
            icons,
            diagrams,
            class_sources,
            source_overrides: HashMap::new(),
            source_versions: HashMap::new(),
        })
    }

    fn icon(&self, class_name: &str) -> Option<&CoreIconScene> {
        self.icons
            .iter()
            .find(|(candidate, _)| candidate == class_name)
            .map(|(_, scene)| scene)
    }

    fn diagram(&self, class_name: &str) -> Option<&CoreDiagramScene> {
        self.diagrams
            .iter()
            .find(|(candidate, _)| candidate == class_name)
            .map(|(_, scene)| scene)
    }

    fn icon_mut(&mut self, class_name: &str) -> Option<&mut CoreIconScene> {
        self.icons
            .iter_mut()
            .find(|(candidate, _)| candidate == class_name)
            .map(|(_, scene)| scene)
    }

    fn diagram_mut(&mut self, class_name: &str) -> Option<&mut CoreDiagramScene> {
        self.diagrams
            .iter_mut()
            .find(|(candidate, _)| candidate == class_name)
            .map(|(_, scene)| scene)
    }

    fn class_source(&self, qualified_name: &str) -> Option<(String, String)> {
        let class = self
            .class_sources
            .iter()
            .find(|class| class.qualified_name == qualified_name)?;
        let text = if let Some(override_text) = self.source_overrides.get(qualified_name) {
            override_text.clone()
        } else {
            let source = fs::read_to_string(&class.source_file).ok()?;
            source
                .get(class.source_range.start..class.source_range.end)?
                .to_owned()
        };
        let source_name = class
            .source_file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Modelica source")
            .to_owned();
        Some((source_name, text))
    }

    fn class_text(&self, qualified_name: &str) -> Option<String> {
        self.class_source(qualified_name).map(|(_, source)| source)
    }

    fn source_version(&self, qualified_name: &str) -> u64 {
        self.source_versions
            .get(qualified_name)
            .copied()
            .unwrap_or_default()
    }

    fn set_class_text(&mut self, qualified_name: &str, text: String) {
        self.source_overrides
            .insert(qualified_name.to_owned(), text);
        let version = self
            .source_versions
            .entry(qualified_name.to_owned())
            .or_default();
        *version = version.saturating_add(1);
    }

    fn resolve_candidate_scenes(
        &self,
        qualified_name: &str,
        source: &str,
    ) -> Result<(CoreIconScene, CoreDiagramScene), String> {
        let package = PackageLoader
            .load(&self.path)
            .map_err(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))?;
        let mut registry = LibraryRegistry::default();
        add_bundled_msl(&mut registry);
        registry.index_package(&package);
        registry.register_package(&package);
        let (mut class, _) = registry
            .resolve_class(qualified_name)
            .ok_or_else(|| format!("class `{qualified_name}` was not found"))?;
        let parsed = parse(source, &class.source_file)
            .map_err(|error| format!("candidate source does not parse: {error}"))?;
        let parsed_class = parsed
            .classes
            .first()
            .ok_or_else(|| "candidate source contains no class".to_owned())?;
        class.source_range = SourceRange::new(0, source.len());
        class.children = parsed_class.children.clone();
        let icon = IconResolver::new(&mut registry).resolve(&class, source);
        let diagram = resolve_diagram(&class, source, &mut registry);
        Ok((icon, diagram))
    }

    fn ui_summary(&self, selected_class: Option<&str>) -> UiDocument {
        let selected_class = selected_class.map(str::to_owned);
        let (source_name, source) = selected_class
            .as_deref()
            .and_then(|class_name| self.class_source(class_name))
            .unwrap_or_default();
        let icon_graphics = selected_class
            .as_deref()
            .and_then(|class_name| self.icon(class_name))
            .map_or(0, |scene| scene.graphics.len());
        let (
            diagram_background,
            diagram_components,
            diagram_own_components,
            diagram_inherited_components,
            diagram_connectors,
            diagram_unresolved_components,
            diagram_unresolved_bases,
            diagram_connections,
        ) = selected_class
            .as_ref()
            .and_then(|class_name| self.diagram(class_name))
            .map_or((0, 0, 0, 0, 0, 0, 0, 0), |scene| {
                let stats = scene.debug_stats();
                (
                    scene.background_graphics.len(),
                    scene.components.len(),
                    stats.own_components,
                    stats.inherited_components,
                    stats.connector_components,
                    stats.unresolved_components,
                    stats.unresolved_bases,
                    scene.connections.len(),
                )
            });
        UiDocument {
            package_name: self.package_name.clone(),
            class_names: self.class_names.clone(),
            tree: build_tree(&self.package_name, &self.class_names),
            selected_class,
            icon_graphics,
            diagram_background,
            diagram_components,
            diagram_own_components,
            diagram_inherited_components,
            diagram_connectors,
            diagram_unresolved_components,
            diagram_unresolved_bases,
            diagram_connections,
            source_name,
            source_lines: source.lines().map(str::to_owned).collect(),
        }
    }

    fn title(&self, fps: Option<(f32, f32)>) -> String {
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Modelica document");
        let performance = fps
            .map(|(fps, worst_ms)| format!(" | {:.1} FPS | worst {:.1} ms", fps, worst_ms))
            .unwrap_or_default();
        format!(
            "modelica-wgpu | {} | {} | {} classes | drag edit · Ctrl+drag pan",
            self.package_name,
            file_name,
            self.class_names.len(),
        ) + &performance
    }
}

fn collect_class_names(package: &PackageNode, output: &mut Vec<String>) {
    output.extend(
        package
            .classes
            .iter()
            .map(|class| class.qualified_name.clone()),
    );
    for child in &package.children {
        collect_class_names(child, output);
    }
}

fn collect_class_sources(package: &PackageNode, output: &mut Vec<ClassSource>) {
    for class in &package.classes {
        collect_class_source(class, output);
    }
    for child in &package.children {
        collect_class_sources(child, output);
    }
}

fn collect_class_source(class: &Class, output: &mut Vec<ClassSource>) {
    output.push(ClassSource {
        qualified_name: class.qualified_name.clone(),
        source_file: class.source_file.clone(),
        source_range: class.source_range,
    });
    for child in &class.children {
        collect_class_source(child, output);
    }
}

fn build_tree(package_name: &str, class_names: &[String]) -> TreeNode {
    let mut root = TreeNode {
        name: package_name
            .rsplit('.')
            .next()
            .unwrap_or(package_name)
            .to_owned(),
        qualified_name: package_name.to_owned(),
        class_name: None,
        children: Vec::new(),
    };
    for class_name in class_names {
        let segments = class_name.split('.').collect::<Vec<_>>();
        let root_segments = package_name.split('.').count();
        if segments.len() <= root_segments || !class_name.starts_with(package_name) {
            continue;
        }
        let mut node = &mut root;
        for segment in &segments[root_segments..] {
            let qualified_name = format!("{}.{}", node.qualified_name, segment);
            let index = node
                .children
                .iter()
                .position(|child| child.qualified_name == qualified_name);
            let index = index.unwrap_or_else(|| {
                node.children.push(TreeNode {
                    name: (*segment).to_owned(),
                    qualified_name: qualified_name.clone(),
                    class_name: None,
                    children: Vec::new(),
                });
                node.children.len() - 1
            });
            node = &mut node.children[index];
        }
        node.class_name = Some(class_name.clone());
    }
    root
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

fn install_ui_fonts(ctx: &egui::Context) {
    let medium_candidates = [
        r"C:\Windows\Fonts\Inter-Medium.ttf",
        r"C:\Windows\Fonts\NotoSans-Medium.ttf",
        "/usr/share/fonts/inter/Inter-Medium.ttf",
        "/usr/share/fonts/truetype/inter/Inter-Medium.ttf",
        "/usr/share/fonts/opentype/inter/Inter-Medium.otf",
        "/usr/share/fonts/noto/NotoSans-Medium.ttf",
    ];
    let semibold_candidates = [
        r"C:\Windows\Fonts\Inter-SemiBold.ttf",
        r"C:\Windows\Fonts\NotoSans-SemiBold.ttf",
        "/usr/share/fonts/inter/Inter-SemiBold.ttf",
        "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
        "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf",
        "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
    ];
    let cjk_candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\NotoSansSC-VF.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansSC-Regular.otf",
    ];
    let symbol_candidates = [
        r"C:\Windows\Fonts\seguisym.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols-Medium.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
    ];
    let medium = medium_candidates
        .iter()
        .find_map(|path| fs::read(path).ok().map(|bytes| (*path, bytes)));
    let medium_path = medium.as_ref().map(|(path, _)| *path);
    let semibold = semibold_candidates
        .iter()
        .find_map(|path| fs::read(path).ok().map(|bytes| (*path, bytes)));
    let semibold_path = semibold.as_ref().map(|(path, _)| *path);
    let cjk = cjk_candidates
        .iter()
        .find_map(|path| fs::read(path).ok().map(|bytes| (*path, bytes)));
    let cjk_path = cjk.as_ref().map(|(path, _)| *path);
    let symbols = symbol_candidates
        .iter()
        .find_map(|path| fs::read(path).ok().map(|bytes| (*path, bytes)));
    let symbols_path = symbols.as_ref().map(|(path, _)| *path);

    let mut fonts = FontDefinitions::default();
    let default_proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let medium_key = if let Some((_, bytes)) = medium {
        fonts
            .font_data
            .insert(UI_FONT_MEDIUM.to_owned(), FontData::from_owned(bytes));
        UI_FONT_MEDIUM.to_owned()
    } else {
        eprintln!("modelica-wgpu: no medium UI font found; using egui default font");
        default_proportional
            .first()
            .cloned()
            .unwrap_or_else(|| "Hack".to_owned())
    };
    let semibold_key = if let Some((_, bytes)) = semibold {
        fonts
            .font_data
            .insert(UI_FONT_SEMIBOLD.to_owned(), FontData::from_owned(bytes));
        UI_FONT_SEMIBOLD.to_owned()
    } else {
        // The medium font may also be unavailable on a clean Windows install.
        // Reuse the resolved key instead of referring to a missing named font
        // family, otherwise egui panics when the first semibold label is laid
        // out.
        medium_key.clone()
    };
    let cjk_key = cjk.map(|(_, bytes)| {
        let key = "modelica-cjk".to_owned();
        fonts
            .font_data
            .insert(key.clone(), FontData::from_owned(bytes));
        key
    });
    let symbols_key = symbols.map(|(_, bytes)| {
        let key = UI_FONT_SYMBOLS.to_owned();
        fonts
            .font_data
            .insert(key.clone(), FontData::from_owned(bytes));
        key
    });

    let mut ui_fallback = vec![medium_key.clone()];
    if let Some(cjk_key) = cjk_key.as_ref() {
        ui_fallback.push(cjk_key.clone());
    }
    if let Some(symbols_key) = symbols_key.as_ref() {
        ui_fallback.push(symbols_key.clone());
    }
    ui_fallback.extend(default_proportional.clone());
    fonts
        .families
        .insert(FontFamily::Name(UI_FONT_MEDIUM.into()), ui_fallback.clone());
    let mut semibold_fallback = vec![semibold_key, medium_key];
    if let Some(cjk_key) = cjk_key.as_ref() {
        semibold_fallback.push(cjk_key.clone());
    }
    if let Some(symbols_key) = symbols_key.as_ref() {
        semibold_fallback.push(symbols_key.clone());
    }
    semibold_fallback.extend(default_proportional);
    fonts
        .families
        .insert(FontFamily::Name(UI_FONT_SEMIBOLD.into()), semibold_fallback);
    fonts.families.insert(FontFamily::Proportional, ui_fallback);
    ctx.set_fonts(fonts);
    if let Some(path) = medium_path {
        eprintln!("modelica-wgpu: installed medium UI font from {path}");
    }
    if let Some(path) = semibold_path {
        eprintln!("modelica-wgpu: installed semibold UI font from {path}");
    }
    if let Some(path) = cjk_path {
        eprintln!("modelica-wgpu: installed CJK fallback font from {path}");
    } else {
        eprintln!("modelica-wgpu: no CJK font found; Chinese glyphs may be missing");
    }
    if let Some(path) = symbols_path {
        eprintln!("modelica-wgpu: installed symbols fallback font from {path}");
    }
}

fn ui_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(UI_FONT_MEDIUM.into()))
}

fn ui_semibold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(UI_FONT_SEMIBOLD.into()))
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum FillMode {
    Solid = 0,
    HorizontalCylinder = 1,
    VerticalCylinder = 2,
    Sphere = 3,
}

struct Geometry {
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
    style: StyleUniform,
    edit_key: Option<String>,
    connection: Option<ConnectionGeometry>,
    component: Option<ComponentGeometry>,
}

#[derive(Clone)]
struct ConnectionGeometry {
    line: LineGraphic,
    transform: Transform2D,
}

#[derive(Clone, Copy)]
struct ComponentGeometry {
    transform: Transform2D,
}

struct GpuGeometry {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    style_bind_group: wgpu::BindGroup,
    base_vertices: Vec<Vertex>,
    edit_key: Option<String>,
    connection: Option<ConnectionGeometry>,
    component: Option<ComponentGeometry>,
}

struct GpuIconScene {
    geometries: Vec<GpuGeometry>,
    bounds: Option<SceneBounds>,
}

impl GpuIconScene {
    fn preview_translation(&self, queue: &wgpu::Queue, edit_key: &str, translation: [f32; 2]) {
        for geometry in &self.geometries {
            if geometry.edit_key.as_deref() != Some(edit_key) {
                continue;
            }
            let vertices = geometry
                .base_vertices
                .iter()
                .map(|vertex| Vertex {
                    position: [
                        vertex.position[0] + translation[0],
                        vertex.position[1] + translation[1],
                    ],
                    ..*vertex
                })
                .collect::<Vec<_>>();
            queue.write_buffer(&geometry.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        }
    }

    fn preview_component_resize(
        &self,
        queue: &wgpu::Queue,
        component_id: &str,
        new_transform: Transform2D,
    ) {
        for geometry in &self.geometries {
            if geometry.edit_key.as_deref() != Some(component_id) {
                continue;
            }
            let Some(component) = geometry.component else {
                continue;
            };
            let vertices = geometry
                .base_vertices
                .iter()
                .map(|vertex| {
                    let local = inverse_transform_point(
                        CorePoint {
                            x: vertex.position[0],
                            y: vertex.position[1],
                        },
                        component.transform,
                    );
                    let resized = apply_transform_point(local, new_transform);
                    Vertex {
                        position: [resized.x, resized.y],
                        ..*vertex
                    }
                })
                .collect::<Vec<_>>();
            queue.write_buffer(&geometry.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        }
    }

    fn preview_connection_endpoint(
        &self,
        queue: &wgpu::Queue,
        connection_id: &str,
        endpoint: ConnectionEndpoint,
        delta: CorePoint,
    ) {
        for geometry in &self.geometries {
            if geometry.edit_key.as_deref() != Some(connection_id) {
                continue;
            }
            let Some(connection) = &geometry.connection else {
                continue;
            };
            let mut line = connection.line.clone();
            line.points = translated_connection_points(
                &line.points,
                endpoint,
                line.origin,
                line.rotation,
                delta,
            );
            let Some(preview) = line_geometry(&line, connection.transform)
                .into_iter()
                .next()
            else {
                continue;
            };
            if preview.vertices.len() != geometry.base_vertices.len() {
                continue;
            }
            queue.write_buffer(
                &geometry.vertex_buffer,
                0,
                bytemuck::cast_slice(&preview.vertices),
            );
        }
    }

    fn preview_connection_points(
        &self,
        queue: &wgpu::Queue,
        connection_id: &str,
        points: &[CorePoint],
    ) {
        for geometry in &self.geometries {
            if geometry.edit_key.as_deref() != Some(connection_id) {
                continue;
            }
            let Some(connection) = &geometry.connection else {
                continue;
            };
            let mut line = connection.line.clone();
            line.points = points.to_vec();
            let Some(preview) = line_geometry(&line, connection.transform)
                .into_iter()
                .next()
            else {
                continue;
            };
            if preview.vertices.len() != geometry.base_vertices.len() {
                continue;
            }
            queue.write_buffer(
                &geometry.vertex_buffer,
                0,
                bytemuck::cast_slice(&preview.vertices),
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SceneBounds {
    min: [f32; 2],
    max: [f32; 2],
}

impl SceneBounds {
    fn from_geometries(geometries: &[Geometry]) -> Option<Self> {
        let mut bounds = Self {
            min: [f32::INFINITY; 2],
            max: [f32::NEG_INFINITY; 2],
        };
        let mut has_vertex = false;
        for geometry in geometries {
            for vertex in &geometry.vertices {
                has_vertex = true;
                bounds.min[0] = bounds.min[0].min(vertex.position[0]);
                bounds.min[1] = bounds.min[1].min(vertex.position[1]);
                bounds.max[0] = bounds.max[0].max(vertex.position[0]);
                bounds.max[1] = bounds.max[1].max(vertex.position[1]);
            }
        }
        has_vertex.then_some(bounds)
    }

    fn center(self) -> [f32; 2] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
        ]
    }

    fn size(self) -> [f32; 2] {
        [
            (self.max[0] - self.min[0]).max(0.001),
            (self.max[1] - self.min[1]).max(0.001),
        ]
    }
}

struct FrameStats {
    last_frame: Option<Instant>,
    samples: VecDeque<Duration>,
    last_report: Instant,
    frames_since_report: u32,
}

impl FrameStats {
    fn new() -> Self {
        Self {
            last_frame: None,
            samples: VecDeque::with_capacity(120),
            last_report: Instant::now(),
            frames_since_report: 0,
        }
    }

    fn record(&mut self, now: Instant) -> Option<(f32, f32)> {
        if let Some(previous) = self.last_frame.replace(now) {
            let frame_time = now.saturating_duration_since(previous);
            if self.samples.len() == 120 {
                self.samples.pop_front();
            }
            self.samples.push_back(frame_time);
        }
        self.frames_since_report += 1;

        if now.duration_since(self.last_report) < Duration::from_secs(1) {
            return None;
        }

        let elapsed = now.duration_since(self.last_report).as_secs_f32();
        let fps = self.frames_since_report as f32 / elapsed;
        let worst_ms = self
            .samples
            .iter()
            .copied()
            .max()
            .unwrap_or_default()
            .as_secs_f32()
            * 1000.0;
        self.last_report = now;
        self.frames_since_report = 0;
        Some((fps, worst_ms))
    }
}

struct App {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    msaa_view: wgpu::TextureView,
    background_pipeline: wgpu::RenderPipeline,
    background_buffer: wgpu::Buffer,
    background_bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    scene: GpuIconScene,
    style_layout: wgpu::BindGroupLayout,
    document: Option<LoadedDocument>,
    load_error: Option<String>,
    selected_class: Option<String>,
    expanded_nodes: HashSet<String>,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    theme_mode: ThemeMode,
    accent_theme: AccentTheme,
    main_view: MainView,
    diagram_scene: GpuIconScene,
    zoom: f32,
    pan: [f32; 2],
    cursor: PhysicalPosition<f64>,
    modifiers: ModifiersState,
    canvas_rect: Option<egui::Rect>,
    diagram_selection: DiagramSelection,
    pointer_interaction: PointerInteraction,
    history: Vec<EditCommand>,
    redo_history: Vec<EditCommand>,
    stats: FrameStats,
}

impl App {
    async fn new(window: Arc<Window>, document: Option<LoadedDocument>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            dx12_shader_compiler: Default::default(),
            gles_minor_version: wgpu::Gles3MinorVersion::Automatic,
            flags: wgpu::InstanceFlags::default(),
        });
        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create wgpu surface");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no compatible wgpu adapter found");
        let adapter_info = adapter.get_info();
        eprintln!(
            "modelica-wgpu adapter: backend={:?}, name={}, driver={}, driver_type={:?}",
            adapter_info.backend, adapter_info.name, adapter_info.driver, adapter_info.device_type
        );
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("modelica-wgpu device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .expect("failed to create wgpu device");

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let no_vsync = std::env::var("MODELICA_WGPU_VSYNC")
            .map(|value| matches!(value.to_ascii_lowercase().as_str(), "0" | "off" | "false"))
            .unwrap_or(false);
        let present_mode = if no_vsync
            && capabilities
                .present_modes
                .contains(&wgpu::PresentMode::Immediate)
        {
            wgpu::PresentMode::Immediate
        } else if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            wgpu::PresentMode::AutoVsync
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view uniforms layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let style_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("style uniforms layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let view_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("view uniforms"),
            contents: bytemuck::bytes_of(&ViewUniform {
                viewport: [size.width as f32, size.height as f32, 0.0, 0.0],
                view: [INITIAL_ZOOM, 0.0, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view uniforms bind group"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });

        let background_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("background uniforms layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let background_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("background uniforms"),
            contents: bytemuck::bytes_of(&background_uniform()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let background_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("background uniforms bind group"),
            layout: &background_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: background_buffer.as_entire_binding(),
            }],
        });
        let background_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("modelica-wgpu background shader"),
            source: wgpu::ShaderSource::Wgsl(BACKGROUND_SHADER.into()),
        });
        let background_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("modelica-wgpu background pipeline layout"),
                bind_group_layouts: &[&background_layout],
                push_constant_ranges: &[],
            });
        let background_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("modelica-wgpu background pipeline"),
            layout: Some(&background_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &background_shader,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &background_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            depth_stencil: None,
            multiview: None,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("modelica-wgpu shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modelica-wgpu pipeline layout"),
            bind_group_layouts: &[&view_layout, &style_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("modelica-wgpu pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Vertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        let scene = build_scene(&device, &style_layout, document.as_ref(), None);
        let diagram_scene = build_diagram_scene(&device, &style_layout, document.as_ref(), None);
        let msaa_view = create_msaa_view(&device, &config);
        let egui_ctx = egui::Context::default();
        install_ui_fonts(&egui_ctx);
        eprintln!("modelica-wgpu: creating egui window state");
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            None,
            None,
        );
        eprintln!("modelica-wgpu: creating egui renderer");
        // Keep UI text on a single-sample pass. The canvas keeps 4x MSAA, but
        // mixing egui glyphs into the MSAA pass makes small text look doubled
        // on some Windows DPI scales.
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1);
        eprintln!("modelica-wgpu: app initialization complete");
        Self {
            window,
            surface,
            device,
            queue,
            config,
            msaa_view,
            background_pipeline,
            background_buffer,
            background_bind_group,
            pipeline,
            view_buffer,
            view_bind_group,
            scene,
            style_layout,
            document,
            load_error: None,
            selected_class: None,
            expanded_nodes: HashSet::new(),
            egui_ctx,
            egui_state,
            egui_renderer,
            theme_mode: ThemeMode::System,
            accent_theme: AccentTheme::Violet,
            main_view: MainView::Icon,
            diagram_scene,
            zoom: INITIAL_ZOOM,
            pan: [0.0, 0.0],
            cursor: PhysicalPosition::new(0.0, 0.0),
            modifiers: ModifiersState::default(),
            canvas_rect: None,
            diagram_selection: DiagramSelection::None,
            pointer_interaction: PointerInteraction::None,
            history: Vec::new(),
            redo_history: Vec::new(),
            stats: FrameStats::new(),
        }
    }

    fn canvas_navigation_enabled(&self) -> bool {
        canvas_navigation_enabled_for(self.main_view)
    }

    fn pointer_over_canvas(&self) -> bool {
        let Some(rect) = self.canvas_rect else {
            return false;
        };
        let scale_factor = self.window.scale_factor() as f32;
        rect.contains(Pos2::new(
            self.cursor.x as f32 / scale_factor,
            self.cursor.y as f32 / scale_factor,
        ))
    }

    fn canvas_event_allowed(&self) -> bool {
        canvas_event_allowed_for(self.main_view, self.pointer_over_canvas())
    }

    fn screen_to_model(&self, position: PhysicalPosition<f64>) -> CorePoint {
        let x = (position.x as f32 - self.config.width as f32 * 0.5 - self.pan[0]) / self.zoom;
        let screen_y =
            (position.y as f32 - self.config.height as f32 * 0.5 - self.pan[1]) / self.zoom;
        CorePoint {
            x,
            y: if self.main_view == MainView::Diagram {
                -screen_y
            } else {
                screen_y
            },
        }
    }

    fn selected_class_name(&self) -> Option<&str> {
        self.selected_class.as_deref()
    }

    fn selected_connection_id(&self) -> Option<&str> {
        match &self.diagram_selection {
            DiagramSelection::Connection(connection_id) => Some(connection_id),
            _ => None,
        }
    }

    fn hit_test_diagram_connection(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
    ) -> Option<ConnectionHit> {
        let class_name = self.selected_class_name()?;
        let scene = self.document.as_ref()?.diagram(class_name)?;
        hit_test_connection(&scene.connections, pointer_model, tolerance)
    }

    fn begin_connection_edit(&mut self, hit: ConnectionHit, pointer_model: CorePoint) {
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            return;
        };
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let Some(connection) = document.diagram(&class_name).and_then(|scene| {
            scene
                .connections
                .iter()
                .find(|connection| connection.id == hit.connection_id)
        }) else {
            return;
        };
        let Some(line) = connection.line.as_ref() else {
            return;
        };
        let Some(source_before) = document.class_text(&class_name) else {
            return;
        };
        let original_points = line.points.clone();
        self.diagram_selection = DiagramSelection::Connection(hit.connection_id.clone());
        match hit.target {
            ConnectionHitTarget::Segment { index, orientation }
                if index > 0 && index + 1 < original_points.len() =>
            {
                self.pointer_interaction = PointerInteraction::MoveDiagramConnectionSegment {
                    button: MouseButton::Left,
                    connection_id: hit.connection_id,
                    connection_key: connection.key.clone(),
                    segment_index: index,
                    orientation,
                    line_origin: line.origin,
                    line_rotation: line.rotation,
                    start_pointer_model: pointer_model,
                    original_points: original_points.clone(),
                    preview_points: original_points,
                    source_before,
                };
            }
            _ => {}
        }
    }

    fn selected_connection_overlay_points(&self) -> Option<Vec<CorePoint>> {
        let connection_id = self.selected_connection_id()?;
        let class_name = self.selected_class_name()?;
        let connection = self
            .document
            .as_ref()?
            .diagram(class_name)?
            .connections
            .iter()
            .find(|connection| connection.id == connection_id)?;
        let line = connection.line.as_ref()?;
        let points = match &self.pointer_interaction {
            PointerInteraction::MoveDiagramConnectionSegment {
                connection_id: active_id,
                preview_points,
                ..
            } if active_id == connection_id => preview_points,
            _ => &line.points,
        };
        Some(connection_world_points(line, points))
    }

    fn selected_component_overlay(&self) -> Option<ComponentSelectionOverlay> {
        let component_name = match &self.diagram_selection {
            DiagramSelection::Component(component_name) => component_name,
            _ => return None,
        };
        let class_name = self.selected_class_name()?;
        let component = self
            .document
            .as_ref()?
            .diagram(class_name)?
            .components
            .iter()
            .find(|component| component.name == *component_name)?;
        Some(ComponentSelectionOverlay {
            origin: component.origin,
            extent: component
                .placement_extent
                .unwrap_or(default_component_extent()),
            rotation: component.rotation,
        })
    }

    fn hit_test_selected_component_handle(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
    ) -> Option<(String, ResizeHandle)> {
        let component_name = match &self.diagram_selection {
            DiagramSelection::Component(component_name) => component_name,
            _ => return None,
        };
        let overlay = self.selected_component_overlay()?;
        component_extent_corners(overlay.origin, overlay.extent, overlay.rotation)
            .iter()
            .enumerate()
            .find(|(_, corner)| distance_between(**corner, pointer_model) <= tolerance)
            .map(|(index, _)| (component_name.clone(), ResizeHandle::Corner(index)))
    }

    fn begin_component_resize(&mut self, component_name: String, handle: ResizeHandle) {
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            return;
        };
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let Some(component) = document.diagram(&class_name).and_then(|scene| {
            scene
                .components
                .iter()
                .find(|component| component.name == component_name)
                .cloned()
        }) else {
            return;
        };
        if !component.editable {
            return;
        }
        let Some(source_before) = document.class_text(&class_name) else {
            return;
        };
        let original_extent = component
            .placement_extent
            .unwrap_or(default_component_extent());
        let connected_connections = document
            .diagram(&class_name)
            .map(|scene| connection_drag_snapshots(scene, &component_name))
            .unwrap_or_default();
        self.pointer_interaction = PointerInteraction::ResizeDiagramComponent {
            button: MouseButton::Left,
            component_id: component.id.clone(),
            component_name,
            handle,
            original_component: component,
            original_extent,
            preview_extent: original_extent,
            connected_connections,
            source_before,
        };
    }

    fn begin_model_drag(&mut self) {
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            return;
        };
        let pointer_model = self.screen_to_model(self.cursor);
        let tolerance = 8.0 / self.zoom.max(MIN_ZOOM);
        match self.main_view {
            MainView::Icon => {
                let Some(document) = self.document.as_ref() else {
                    return;
                };
                let Some((graphic_id, original_geometry)) =
                    document.icon(&class_name).and_then(|scene| {
                        scene
                            .graphics
                            .iter()
                            .rev()
                            .find(|graphic| {
                                graphic.editable
                                    && resolved_graphic_contains_point(
                                        graphic,
                                        pointer_model,
                                        tolerance,
                                    )
                            })
                            .map(|graphic| (graphic.id.0.clone(), graphic.graphic.clone()))
                    })
                else {
                    return;
                };
                let Some(source_before) = document.class_text(&class_name) else {
                    return;
                };
                self.pointer_interaction = PointerInteraction::MoveIconGraphic {
                    button: MouseButton::Left,
                    graphic_id,
                    start_pointer_model: pointer_model,
                    original_geometry,
                    preview_delta: CorePoint { x: 0.0, y: 0.0 },
                    source_before,
                };
            }
            MainView::Diagram => {
                let Some(document) = self.document.as_ref() else {
                    return;
                };
                if let Some((component_name, handle)) =
                    self.hit_test_selected_component_handle(pointer_model, tolerance)
                {
                    self.begin_component_resize(component_name, handle);
                    return;
                }
                let Some((component_id, component_name, original_origin)) =
                    document.diagram(&class_name).and_then(|scene| {
                        scene
                            .components
                            .iter()
                            .rev()
                            .find(|component| {
                                component.editable
                                    && component.visible
                                    && diagram_component_contains_point(
                                        component,
                                        pointer_model,
                                        tolerance,
                                    )
                            })
                            .map(|component| {
                                (
                                    component.id.clone(),
                                    component.name.clone(),
                                    component.origin,
                                )
                            })
                    })
                else {
                    if let Some(hit) = self.hit_test_diagram_connection(pointer_model, tolerance) {
                        self.begin_connection_edit(hit, pointer_model);
                    } else {
                        self.diagram_selection = DiagramSelection::None;
                    }
                    return;
                };
                let Some(source_before) = document.class_text(&class_name) else {
                    return;
                };
                let connected_connections = document
                    .diagram(&class_name)
                    .into_iter()
                    .flat_map(|scene| scene.connections.iter())
                    .filter_map(|connection| {
                        let endpoint = match (
                            connection.lhs.component_name == component_name,
                            connection.rhs.component_name == component_name,
                        ) {
                            (true, true) => ConnectionEndpoint::Both,
                            (true, false) => ConnectionEndpoint::Lhs,
                            (false, true) => ConnectionEndpoint::Rhs,
                            (false, false) => return None,
                        };
                        let line = connection.line.as_ref()?;
                        Some(ConnectionDragSnapshot {
                            connection_id: connection.id.clone(),
                            connection_key: connection.key.clone(),
                            endpoint,
                            original_line_points: line.points.clone(),
                            original_line_origin: line.origin,
                            original_line_rotation: line.rotation,
                            lhs_connector_path: (connection.lhs.component_name == component_name)
                                .then(|| connection.lhs.connector_path.clone()),
                            rhs_connector_path: (connection.rhs.component_name == component_name)
                                .then(|| connection.rhs.connector_path.clone()),
                        })
                    })
                    .collect();
                self.diagram_selection = DiagramSelection::Component(component_name.clone());
                self.pointer_interaction = PointerInteraction::MoveDiagramComponent {
                    button: MouseButton::Left,
                    component_id,
                    component_name,
                    start_pointer_model: pointer_model,
                    original_origin,
                    preview_delta: CorePoint { x: 0.0, y: 0.0 },
                    connected_connections,
                    source_before,
                };
            }
            MainView::Source => {}
        }
    }

    fn update_model_drag_preview(&mut self, position: PhysicalPosition<f64>) {
        let interaction = self.pointer_interaction.clone();
        match interaction {
            PointerInteraction::Pan {
                start_pointer,
                start_pan,
                ..
            } => {
                self.pan = [
                    start_pan[0] + (position.x - start_pointer.x) as f32,
                    start_pan[1] + (position.y - start_pointer.y) as f32,
                ];
                self.update_view_uniform();
                self.window.request_redraw();
            }
            PointerInteraction::MoveIconGraphic {
                graphic_id,
                start_pointer_model,
                ..
            } => {
                let current = self.screen_to_model(position);
                let delta = CorePoint {
                    x: current.x - start_pointer_model.x,
                    y: current.y - start_pointer_model.y,
                };
                self.scene
                    .preview_translation(&self.queue, &graphic_id, [delta.x, delta.y]);
                if let PointerInteraction::MoveIconGraphic { preview_delta, .. } =
                    &mut self.pointer_interaction
                {
                    *preview_delta = delta;
                }
                self.window.request_redraw();
            }
            PointerInteraction::MoveDiagramComponent {
                component_id,
                start_pointer_model,
                connected_connections,
                ..
            } => {
                let current = self.screen_to_model(position);
                let delta = CorePoint {
                    x: current.x - start_pointer_model.x,
                    y: current.y - start_pointer_model.y,
                };
                self.diagram_scene.preview_translation(
                    &self.queue,
                    &component_id,
                    [delta.x, -delta.y],
                );
                for connection in &connected_connections {
                    self.diagram_scene.preview_connection_endpoint(
                        &self.queue,
                        &connection.connection_id,
                        connection.endpoint,
                        delta,
                    );
                }
                if let PointerInteraction::MoveDiagramComponent { preview_delta, .. } =
                    &mut self.pointer_interaction
                {
                    *preview_delta = delta;
                }
                self.window.request_redraw();
            }
            PointerInteraction::MoveDiagramConnectionSegment {
                connection_id,
                segment_index,
                orientation,
                line_origin,
                line_rotation,
                start_pointer_model,
                original_points,
                ..
            } => {
                let current = self.screen_to_model(position);
                let delta = CorePoint {
                    x: current.x - start_pointer_model.x,
                    y: current.y - start_pointer_model.y,
                };
                let preview_points = translated_connection_segment(
                    &original_points,
                    segment_index,
                    orientation,
                    line_origin,
                    line_rotation,
                    delta,
                );
                self.diagram_scene.preview_connection_points(
                    &self.queue,
                    &connection_id,
                    &preview_points,
                );
                if let PointerInteraction::MoveDiagramConnectionSegment {
                    preview_points: active_preview,
                    ..
                } = &mut self.pointer_interaction
                {
                    *active_preview = preview_points;
                }
                self.window.request_redraw();
            }
            PointerInteraction::ResizeDiagramComponent {
                component_id,
                original_component,
                original_extent,
                handle,
                connected_connections,
                ..
            } => {
                let current = self.screen_to_model(position);
                let preview_extent = resized_extent_from_pointer(
                    original_extent,
                    original_component.origin,
                    original_component.rotation,
                    handle,
                    current,
                );
                let Some(icon) = original_component.resolved_icon.as_deref() else {
                    return;
                };
                let placement = diagram_placement_transform_for_extent(
                    icon,
                    original_component.origin,
                    original_component.rotation,
                    preview_extent,
                );
                self.diagram_scene.preview_component_resize(
                    &self.queue,
                    &component_id,
                    compose_transform(
                        Transform2D {
                            scale_y: -1.0,
                            ..Transform2D::identity()
                        },
                        placement,
                    ),
                );
                for connection in &connected_connections {
                    let preview_points = resized_connection_points(
                        &original_component,
                        original_extent,
                        preview_extent,
                        connection,
                    );
                    self.diagram_scene.preview_connection_points(
                        &self.queue,
                        &connection.connection_id,
                        &preview_points,
                    );
                }
                if let PointerInteraction::ResizeDiagramComponent {
                    preview_extent: active_preview,
                    ..
                } = &mut self.pointer_interaction
                {
                    *active_preview = preview_extent;
                }
                self.window.request_redraw();
            }
            PointerInteraction::None => {}
        }
    }

    fn rebuild_selected_scenes(&mut self) {
        self.scene = build_scene(
            &self.device,
            &self.style_layout,
            self.document.as_ref(),
            self.selected_class.as_deref(),
        );
        self.diagram_scene = build_diagram_scene(
            &self.device,
            &self.style_layout,
            self.document.as_ref(),
            self.selected_class.as_deref(),
        );
    }

    fn finish_model_drag(&mut self, button: MouseButton) {
        let matches_button = match &self.pointer_interaction {
            PointerInteraction::Pan {
                button: active_button,
                ..
            }
            | PointerInteraction::MoveIconGraphic {
                button: active_button,
                ..
            }
            | PointerInteraction::MoveDiagramComponent {
                button: active_button,
                ..
            }
            | PointerInteraction::MoveDiagramConnectionSegment {
                button: active_button,
                ..
            }
            | PointerInteraction::ResizeDiagramComponent {
                button: active_button,
                ..
            } => *active_button == button,
            PointerInteraction::None => false,
        };
        if !matches_button {
            return;
        }
        let interaction =
            std::mem::replace(&mut self.pointer_interaction, PointerInteraction::None);
        match interaction {
            PointerInteraction::Pan { .. } => {}
            PointerInteraction::MoveIconGraphic {
                graphic_id,
                start_pointer_model,
                original_geometry,
                source_before,
                ..
            } => {
                let current = self.screen_to_model(self.cursor);
                let delta = CorePoint {
                    x: current.x - start_pointer_model.x,
                    y: current.y - start_pointer_model.y,
                };
                self.commit_icon_graphic_move(graphic_id, original_geometry, source_before, delta);
            }
            PointerInteraction::MoveDiagramComponent {
                component_id,
                component_name,
                start_pointer_model,
                original_origin,
                connected_connections,
                source_before,
                ..
            } => {
                let current = self.screen_to_model(self.cursor);
                let delta = CorePoint {
                    x: current.x - start_pointer_model.x,
                    y: current.y - start_pointer_model.y,
                };
                self.commit_diagram_component_move(
                    component_id,
                    component_name,
                    original_origin,
                    source_before,
                    delta,
                    connected_connections,
                );
            }
            PointerInteraction::MoveDiagramConnectionSegment {
                connection_id: _connection_id,
                connection_key,
                segment_index,
                orientation,
                line_origin,
                line_rotation,
                start_pointer_model,
                original_points,
                source_before,
                ..
            } => {
                let current = self.screen_to_model(self.cursor);
                let delta = CorePoint {
                    x: current.x - start_pointer_model.x,
                    y: current.y - start_pointer_model.y,
                };
                let after_points = translated_connection_segment(
                    &original_points,
                    segment_index,
                    orientation,
                    line_origin,
                    line_rotation,
                    delta,
                );
                self.commit_diagram_connection_move(
                    connection_key,
                    original_points,
                    after_points,
                    source_before,
                );
            }
            PointerInteraction::ResizeDiagramComponent {
                component_id,
                component_name,
                handle,
                original_component,
                original_extent,
                connected_connections,
                source_before,
                ..
            } => {
                let after_extent = resized_extent_from_pointer(
                    original_extent,
                    original_component.origin,
                    original_component.rotation,
                    handle,
                    self.screen_to_model(self.cursor),
                );
                self.commit_diagram_component_resize(
                    component_id,
                    component_name,
                    original_component,
                    original_extent,
                    after_extent,
                    connected_connections,
                    source_before,
                );
            }
            PointerInteraction::None => {}
        }
    }

    fn commit_icon_graphic_move(
        &mut self,
        graphic_id: String,
        before_geometry: CoreGraphic,
        source_before: String,
        delta: CorePoint,
    ) {
        if delta_is_zero(delta) {
            self.rebuild_selected_scenes();
            return;
        }
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            self.rebuild_selected_scenes();
            return;
        };
        let Some(index) = icon_graphic_index(&graphic_id) else {
            self.rebuild_selected_scenes();
            return;
        };
        let Some(version) = self
            .document
            .as_ref()
            .map(|document| document.source_version(&class_name))
        else {
            self.rebuild_selected_scenes();
            return;
        };
        let after_geometry = translated_graphic(&before_geometry, delta);
        let candidate = match patch_icon_graphic_origin(
            &source_before,
            index,
            graphic_origin(&after_geometry),
            version,
        ) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.load_error = Some(format!("Icon edit rejected: {error}"));
                self.rebuild_selected_scenes();
                return;
            }
        };
        let (resolved_icon, resolved_diagram) = match self.document.as_ref().and_then(|document| {
            document
                .resolve_candidate_scenes(&class_name, &candidate)
                .ok()
        }) {
            Some(scenes) => scenes,
            None => {
                self.load_error = Some("Icon edit could not resolve candidate source".into());
                self.rebuild_selected_scenes();
                return;
            }
        };
        let Some(document) = self.document.as_mut() else {
            self.rebuild_selected_scenes();
            return;
        };
        document.set_class_text(&class_name, candidate.clone());
        if let Some(scene) = document.icon_mut(&class_name) {
            *scene = resolved_icon;
        }
        if let Some(scene) = document.diagram_mut(&class_name) {
            *scene = resolved_diagram;
        }
        self.history.push(EditCommand::MoveIconGraphic {
            class_name,
            graphic_id,
            before_geometry,
            after_geometry,
            before_source: source_before,
            after_source: candidate,
        });
        self.redo_history.clear();
        self.load_error = None;
        self.rebuild_selected_scenes();
    }

    fn commit_diagram_component_move(
        &mut self,
        component_id: String,
        component_name: String,
        before_origin: CorePoint,
        source_before: String,
        delta: CorePoint,
        connected_connections: Vec<ConnectionDragSnapshot>,
    ) {
        if delta_is_zero(delta) {
            self.rebuild_selected_scenes();
            return;
        }
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            self.rebuild_selected_scenes();
            return;
        };
        let Some(version) = self
            .document
            .as_ref()
            .map(|document| document.source_version(&class_name))
        else {
            self.rebuild_selected_scenes();
            return;
        };
        let Some(current_scene) = self
            .document
            .as_ref()
            .and_then(|document| document.diagram(&class_name))
        else {
            self.rebuild_selected_scenes();
            return;
        };
        let after_origin = CorePoint {
            x: before_origin.x + delta.x,
            y: before_origin.y + delta.y,
        };
        let mut source_edits = Vec::with_capacity(connected_connections.len() + 1);
        let component_edit =
            match component_origin_edit(&source_before, &component_name, after_origin) {
                Ok(edit) => edit,
                Err(error) => {
                    self.load_error = Some(format!("Diagram edit rejected: {error}"));
                    self.rebuild_selected_scenes();
                    return;
                }
            };
        source_edits.push(component_edit);
        let mut connection_edits = Vec::with_capacity(connected_connections.len());
        for snapshot in &connected_connections {
            let after_points = translated_connection_points(
                &snapshot.original_line_points,
                snapshot.endpoint,
                snapshot.original_line_origin,
                snapshot.original_line_rotation,
                delta,
            );
            let edit = match connection_points_edit_for_key(
                &source_before,
                current_scene,
                &snapshot.connection_key,
                &after_points,
            ) {
                Ok(edit) => edit,
                Err(error) => {
                    self.load_error = Some(format!("Diagram edit rejected: {error}"));
                    self.rebuild_selected_scenes();
                    return;
                }
            };
            source_edits.push(edit);
            connection_edits.push(ConnectionLineEdit {
                connection_key: snapshot.connection_key.clone(),
                before_points: snapshot.original_line_points.clone(),
                after_points,
                line_origin: snapshot.original_line_origin,
            });
        }
        let candidate = match apply_validated_source_edits(&source_before, source_edits, version) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.load_error = Some(format!("Diagram edit rejected: {error}"));
                self.rebuild_selected_scenes();
                return;
            }
        };
        let (resolved_icon, resolved_diagram) = match self.document.as_ref().and_then(|document| {
            document
                .resolve_candidate_scenes(&class_name, &candidate)
                .ok()
        }) {
            Some(scenes) => scenes,
            None => {
                self.load_error = Some("Diagram edit could not resolve candidate source".into());
                self.rebuild_selected_scenes();
                return;
            }
        };
        for edit in &connection_edits {
            let Some(connection) = resolved_diagram
                .connections
                .iter()
                .find(|connection| connection.key == edit.connection_key)
            else {
                self.load_error = Some("Diagram edit lost a connection".into());
                self.rebuild_selected_scenes();
                return;
            };
            if connection
                .line
                .as_ref()
                .is_none_or(|line| line.origin != edit.line_origin)
                || !connection_points_match_invariants(
                    &resolved_diagram,
                    connection,
                    &edit.after_points,
                    edit.before_points.len(),
                )
            {
                self.load_error = Some("Diagram edit did not preserve connection endpoints".into());
                self.rebuild_selected_scenes();
                return;
            }
        }
        let Some(document) = self.document.as_mut() else {
            self.rebuild_selected_scenes();
            return;
        };
        document.set_class_text(&class_name, candidate.clone());
        if let Some(scene) = document.icon_mut(&class_name) {
            *scene = resolved_icon;
        }
        if let Some(scene) = document.diagram_mut(&class_name) {
            *scene = resolved_diagram;
        }
        self.history.push(EditCommand::MoveDiagramComponent {
            class_name,
            component_id,
            before_origin,
            after_origin,
            before_source: source_before,
            after_source: candidate,
            connection_edits,
        });
        self.redo_history.clear();
        self.load_error = None;
        self.rebuild_selected_scenes();
    }

    fn commit_diagram_connection_move(
        &mut self,
        connection_key: ConnectionKey,
        before_points: Vec<CorePoint>,
        after_points: Vec<CorePoint>,
        source_before: String,
    ) {
        if before_points == after_points {
            self.rebuild_selected_scenes();
            return;
        }
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            self.rebuild_selected_scenes();
            return;
        };
        let Some(version) = self
            .document
            .as_ref()
            .map(|document| document.source_version(&class_name))
        else {
            self.rebuild_selected_scenes();
            return;
        };
        let Some(current_scene) = self
            .document
            .as_ref()
            .and_then(|document| document.diagram(&class_name))
        else {
            self.rebuild_selected_scenes();
            return;
        };
        let edit = match connection_points_edit_for_key(
            &source_before,
            current_scene,
            &connection_key,
            &after_points,
        ) {
            Ok(edit) => edit,
            Err(error) => {
                self.load_error = Some(format!("Connection edit rejected: {error}"));
                self.rebuild_selected_scenes();
                return;
            }
        };
        let candidate = match apply_validated_source_edits(&source_before, vec![edit], version) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.load_error = Some(format!("Connection edit rejected: {error}"));
                self.rebuild_selected_scenes();
                return;
            }
        };
        let (resolved_icon, resolved_diagram) = match self.document.as_ref().and_then(|document| {
            document
                .resolve_candidate_scenes(&class_name, &candidate)
                .ok()
        }) {
            Some(scenes) => scenes,
            None => {
                self.load_error = Some("Connection edit could not resolve candidate source".into());
                self.rebuild_selected_scenes();
                return;
            }
        };
        let Some(connection) = resolved_diagram
            .connections
            .iter()
            .find(|connection| connection.key == connection_key)
        else {
            self.load_error = Some("Connection edit lost its connection identity".into());
            self.rebuild_selected_scenes();
            return;
        };
        if !connection_points_match_invariants(
            &resolved_diagram,
            connection,
            &after_points,
            before_points.len(),
        ) {
            self.load_error = Some("Connection edit did not update Line.points".into());
            self.rebuild_selected_scenes();
            return;
        }
        let Some(document) = self.document.as_mut() else {
            self.rebuild_selected_scenes();
            return;
        };
        document.set_class_text(&class_name, candidate.clone());
        if let Some(scene) = document.icon_mut(&class_name) {
            *scene = resolved_icon;
        }
        if let Some(scene) = document.diagram_mut(&class_name) {
            *scene = resolved_diagram;
        }
        self.history.push(EditCommand::MoveDiagramConnection {
            class_name,
            connection_key,
            before_points,
            after_points,
        });
        self.redo_history.clear();
        self.load_error = None;
        self.rebuild_selected_scenes();
    }

    fn commit_diagram_component_resize(
        &mut self,
        component_id: String,
        component_name: String,
        original_component: CoreComponentInstance,
        before_extent: modelica_core::scene::Extent,
        after_extent: modelica_core::scene::Extent,
        connected_connections: Vec<ConnectionDragSnapshot>,
        source_before: String,
    ) {
        if before_extent == after_extent {
            self.rebuild_selected_scenes();
            return;
        }
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            self.rebuild_selected_scenes();
            return;
        };
        let Some(version) = self
            .document
            .as_ref()
            .map(|document| document.source_version(&class_name))
        else {
            self.rebuild_selected_scenes();
            return;
        };
        let Some(current_scene) = self
            .document
            .as_ref()
            .and_then(|document| document.diagram(&class_name))
        else {
            self.rebuild_selected_scenes();
            return;
        };
        let extent_edit = match component_extent_edit(&source_before, &component_name, after_extent)
        {
            Ok(edit) => edit,
            Err(error) => {
                self.load_error = Some(format!("Component resize rejected: {error}"));
                self.rebuild_selected_scenes();
                return;
            }
        };
        let mut source_edits = vec![extent_edit];
        let mut connection_edits = Vec::with_capacity(connected_connections.len());
        for snapshot in &connected_connections {
            let after_points = resized_connection_points(
                &original_component,
                before_extent,
                after_extent,
                snapshot,
            );
            let edit = match connection_points_edit_for_key(
                &source_before,
                current_scene,
                &snapshot.connection_key,
                &after_points,
            ) {
                Ok(edit) => edit,
                Err(error) => {
                    self.load_error = Some(format!("Component resize rejected: {error}"));
                    self.rebuild_selected_scenes();
                    return;
                }
            };
            source_edits.push(edit);
            connection_edits.push(ConnectionLineEdit {
                connection_key: snapshot.connection_key.clone(),
                before_points: snapshot.original_line_points.clone(),
                after_points,
                line_origin: snapshot.original_line_origin,
            });
        }
        let candidate = match apply_validated_source_edits(&source_before, source_edits, version) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.load_error = Some(format!("Component resize rejected: {error}"));
                self.rebuild_selected_scenes();
                return;
            }
        };
        let (resolved_icon, resolved_diagram) = match self.document.as_ref().and_then(|document| {
            document
                .resolve_candidate_scenes(&class_name, &candidate)
                .ok()
        }) {
            Some(scenes) => scenes,
            None => {
                self.load_error =
                    Some("Component resize could not resolve candidate source".into());
                self.rebuild_selected_scenes();
                return;
            }
        };
        if !resolved_diagram.components.iter().any(|component| {
            component.id == component_id && component.placement_extent == Some(after_extent)
        }) {
            self.load_error = Some("Component resize did not update Placement.extent".into());
            self.rebuild_selected_scenes();
            return;
        }
        for edit in &connection_edits {
            let Some(connection) = resolved_diagram
                .connections
                .iter()
                .find(|connection| connection.key == edit.connection_key)
            else {
                self.load_error = Some("Component resize lost a connection".into());
                self.rebuild_selected_scenes();
                return;
            };
            if connection
                .line
                .as_ref()
                .is_none_or(|line| line.origin != edit.line_origin)
                || !connection_points_match_invariants(
                    &resolved_diagram,
                    connection,
                    &edit.after_points,
                    edit.before_points.len(),
                )
            {
                self.load_error =
                    Some("Component resize did not update connection geometry".into());
                self.rebuild_selected_scenes();
                return;
            }
        }
        let Some(document) = self.document.as_mut() else {
            self.rebuild_selected_scenes();
            return;
        };
        document.set_class_text(&class_name, candidate.clone());
        if let Some(scene) = document.icon_mut(&class_name) {
            *scene = resolved_icon;
        }
        if let Some(scene) = document.diagram_mut(&class_name) {
            *scene = resolved_diagram;
        }
        self.history.push(EditCommand::ResizeDiagramComponent {
            class_name,
            component_id,
            before_extent,
            after_extent,
            before_source: source_before,
            after_source: candidate,
            connection_edits,
        });
        self.redo_history.clear();
        self.load_error = None;
        self.rebuild_selected_scenes();
    }

    fn apply_edit_command(&mut self, command: &EditCommand, after: bool) {
        match command {
            EditCommand::MoveIconGraphic {
                class_name,
                graphic_id,
                before_geometry,
                after_geometry,
                before_source,
                after_source,
            } => {
                let source = if after { after_source } else { before_source };
                let expected_geometry = if after {
                    after_geometry
                } else {
                    before_geometry
                };
                let Some((resolved_icon, resolved_diagram)) =
                    self.document.as_ref().and_then(|document| {
                        document.resolve_candidate_scenes(class_name, source).ok()
                    })
                else {
                    return;
                };
                if !resolved_icon.graphics.iter().any(|graphic| {
                    graphic.id.0 == *graphic_id && graphic.graphic == *expected_geometry
                }) {
                    return;
                }
                let Some(document) = self.document.as_mut() else {
                    return;
                };
                document.set_class_text(class_name, source.clone());
                if let Some(scene) = document.icon_mut(class_name) {
                    *scene = resolved_icon;
                }
                if let Some(scene) = document.diagram_mut(class_name) {
                    *scene = resolved_diagram;
                }
            }
            EditCommand::MoveDiagramComponent {
                class_name,
                component_id,
                before_origin,
                after_origin,
                before_source,
                after_source,
                connection_edits,
            } => {
                let source = if after { after_source } else { before_source };
                let expected_origin = if after { *after_origin } else { *before_origin };
                let Some((resolved_icon, resolved_diagram)) =
                    self.document.as_ref().and_then(|document| {
                        document.resolve_candidate_scenes(class_name, source).ok()
                    })
                else {
                    return;
                };
                if !resolved_diagram.components.iter().any(|component| {
                    component.id == *component_id && component.origin == expected_origin
                }) {
                    return;
                }
                for edit in connection_edits {
                    let expected_points = if after {
                        &edit.after_points
                    } else {
                        &edit.before_points
                    };
                    let Some(connection) = resolved_diagram
                        .connections
                        .iter()
                        .find(|connection| connection.key == edit.connection_key)
                    else {
                        return;
                    };
                    let Some(line) = connection.line.as_ref() else {
                        return;
                    };
                    if line.origin != edit.line_origin || line.points != *expected_points {
                        return;
                    }
                }
                let Some(document) = self.document.as_mut() else {
                    return;
                };
                document.set_class_text(class_name, source.clone());
                if let Some(scene) = document.icon_mut(class_name) {
                    *scene = resolved_icon;
                }
                if let Some(scene) = document.diagram_mut(class_name) {
                    *scene = resolved_diagram;
                }
            }
            EditCommand::MoveDiagramConnection {
                class_name,
                connection_key,
                before_points,
                after_points,
            } => {
                let expected_points = if after { after_points } else { before_points };
                let Some(document) = self.document.as_ref() else {
                    return;
                };
                let Some(source) = document.class_text(class_name) else {
                    return;
                };
                let Some(scene) = document.diagram(class_name) else {
                    return;
                };
                let version = document.source_version(class_name);
                let Ok(edit) =
                    connection_points_edit_for_key(&source, scene, connection_key, expected_points)
                else {
                    return;
                };
                let Ok(candidate) = apply_validated_source_edits(&source, vec![edit], version)
                else {
                    return;
                };
                let Some((resolved_icon, resolved_diagram)) = document
                    .resolve_candidate_scenes(class_name, &candidate)
                    .ok()
                else {
                    return;
                };
                let Some(connection) = resolved_diagram
                    .connections
                    .iter()
                    .find(|connection| connection.key == *connection_key)
                else {
                    return;
                };
                if !connection_points_match_invariants(
                    &resolved_diagram,
                    connection,
                    expected_points,
                    before_points.len(),
                ) {
                    return;
                }
                let Some(document) = self.document.as_mut() else {
                    return;
                };
                document.set_class_text(class_name, candidate);
                if let Some(scene) = document.icon_mut(class_name) {
                    *scene = resolved_icon;
                }
                if let Some(scene) = document.diagram_mut(class_name) {
                    *scene = resolved_diagram;
                }
            }
            EditCommand::ResizeDiagramComponent {
                class_name,
                component_id,
                before_extent,
                after_extent,
                before_source,
                after_source,
                connection_edits,
            } => {
                let source = if after { after_source } else { before_source };
                let expected_extent = if after { after_extent } else { before_extent };
                let Some((resolved_icon, resolved_diagram)) =
                    self.document.as_ref().and_then(|document| {
                        document.resolve_candidate_scenes(class_name, source).ok()
                    })
                else {
                    return;
                };
                if !resolved_diagram.components.iter().any(|component| {
                    component.id == *component_id
                        && component.placement_extent == Some(*expected_extent)
                }) {
                    return;
                }
                for edit in connection_edits {
                    let expected_points = if after {
                        &edit.after_points
                    } else {
                        &edit.before_points
                    };
                    let Some(connection) = resolved_diagram
                        .connections
                        .iter()
                        .find(|connection| connection.key == edit.connection_key)
                    else {
                        return;
                    };
                    let Some(line) = connection.line.as_ref() else {
                        return;
                    };
                    if line.origin != edit.line_origin
                        || !connection_points_match_invariants(
                            &resolved_diagram,
                            connection,
                            expected_points,
                            edit.before_points.len(),
                        )
                    {
                        return;
                    }
                }
                let Some(document) = self.document.as_mut() else {
                    return;
                };
                document.set_class_text(class_name, source.clone());
                if let Some(scene) = document.icon_mut(class_name) {
                    *scene = resolved_icon;
                }
                if let Some(scene) = document.diagram_mut(class_name) {
                    *scene = resolved_diagram;
                }
            }
        }
        self.load_error = None;
        self.rebuild_selected_scenes();
    }

    fn undo(&mut self) {
        let Some(command) = self.history.pop() else {
            return;
        };
        self.apply_edit_command(&command, false);
        self.redo_history.push(command);
    }

    fn redo(&mut self) {
        let Some(command) = self.redo_history.pop() else {
            return;
        };
        self.apply_edit_command(&command, true);
        self.history.push(command);
    }

    fn update_title(&self, fps: Option<(f32, f32)>) {
        if let Some(document) = &self.document {
            self.window.set_title(&document.title(fps));
        } else {
            let performance = fps
                .map(|(fps, worst_ms)| format!(" | {:.1} FPS | worst {:.1} ms", fps, worst_ms))
                .unwrap_or_default();
            self.window.set_title(&format!(
                "modelica-wgpu UI preview | Source / Icon / Diagram{performance}"
            ));
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.msaa_view = create_msaa_view(&self.device, &self.config);
        self.update_view_uniform();
    }

    fn update_view_uniform(&self) {
        self.queue.write_buffer(
            &self.view_buffer,
            0,
            bytemuck::bytes_of(&ViewUniform {
                viewport: [
                    self.config.width as f32,
                    self.config.height as f32,
                    0.0,
                    0.0,
                ],
                view: [self.zoom, self.pan[0], self.pan[1], 0.0],
            }),
        );
    }

    fn fit_scene(&mut self) {
        if !self.canvas_navigation_enabled() {
            return;
        }
        let active_scene = if self.main_view == MainView::Diagram {
            &self.diagram_scene
        } else {
            &self.scene
        };
        let Some(bounds) = active_scene.bounds else {
            self.zoom = INITIAL_ZOOM;
            self.pan = [0.0, 0.0];
            self.update_view_uniform();
            return;
        };
        let size = bounds.size();
        let target_x = self.config.width as f32 * 0.50;
        let target_y = self.config.height as f32 * 0.52;
        let target_size = (self.config.width.min(self.config.height) as f32 * 0.68).max(120.0);
        self.zoom = (target_size / size[0].max(size[1])).clamp(MIN_ZOOM, MAX_ZOOM);
        let center = bounds.center();
        self.pan = [
            target_x - self.config.width as f32 * 0.5 - center[0] * self.zoom,
            target_y - self.config.height as f32 * 0.5 - center[1] * self.zoom,
        ];
        self.update_view_uniform();
    }

    fn zoom_at_cursor(&mut self, wheel_delta: f32) {
        if !self.canvas_navigation_enabled() {
            return;
        }
        let old_zoom = self.zoom;
        self.zoom = (self.zoom * (1.0 + wheel_delta * 0.1)).clamp(MIN_ZOOM, MAX_ZOOM);
        if (self.zoom - old_zoom).abs() < f32::EPSILON {
            return;
        }
        let center = [
            self.config.width as f32 * 0.5,
            self.config.height as f32 * 0.5,
        ];
        let cursor = [self.cursor.x as f32, self.cursor.y as f32];
        let world_before = [
            (cursor[0] - center[0] - self.pan[0]) / old_zoom,
            (cursor[1] - center[1] - self.pan[1]) / old_zoom,
        ];
        let world_after = [world_before[0] * self.zoom, world_before[1] * self.zoom];
        self.pan[0] += cursor[0] - center[0] - self.pan[0] - world_after[0];
        self.pan[1] += cursor[1] - center[1] - self.pan[1] - world_after[1];
        self.update_view_uniform();
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        set_theme(
            self.theme_mode.is_dark(self.window.theme()),
            self.accent_theme,
        );
        let mut main_view = self.main_view;
        let mut theme_mode = self.theme_mode;
        let mut accent_theme = self.accent_theme;
        let selected_class = self.selected_class.clone();
        let document_summary = self
            .document
            .as_ref()
            .map(|document| document.ui_summary(selected_class.as_deref()));
        let mut expanded_nodes = self.expanded_nodes.clone();
        let load_error = self.load_error.clone();
        let mut open_requested = false;
        let mut class_clicked = None;
        let mut fit_requested = false;
        let mut view_changed = false;
        let mut icon_clip_rect = None;
        let mut expand_all_requested = false;
        let mut collapse_all_requested = false;
        let selected_connection_points = self.selected_connection_overlay_points();
        let selected_component_overlay = self.selected_component_overlay();
        let zoom = self.zoom;
        let pan = self.pan;
        let viewport = [self.config.width, self.config.height];
        let pixels_per_point = self.window.scale_factor() as f32;
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            draw_preview_ui(
                ctx,
                &mut main_view,
                &mut theme_mode,
                &mut accent_theme,
                selected_class.as_deref(),
                document_summary.as_ref(),
                &mut expanded_nodes,
                &mut open_requested,
                &mut class_clicked,
                &mut fit_requested,
                &mut view_changed,
                &mut icon_clip_rect,
                &mut expand_all_requested,
                &mut collapse_all_requested,
                load_error.as_deref(),
            );
            if main_view == MainView::Diagram {
                draw_diagram_selection_overlay(
                    ctx,
                    icon_clip_rect,
                    selected_connection_points.as_deref(),
                    selected_component_overlay,
                    zoom,
                    pan,
                    viewport,
                    pixels_per_point,
                );
            }
        });
        if theme_mode != self.theme_mode || accent_theme != self.accent_theme {
            self.theme_mode = theme_mode;
            self.accent_theme = accent_theme;
            set_theme(
                self.theme_mode.is_dark(self.window.theme()),
                self.accent_theme,
            );
            self.window.request_redraw();
        }
        let previous_main_view = self.main_view;
        self.main_view = main_view;
        self.canvas_rect = icon_clip_rect;
        if previous_main_view != self.main_view {
            self.pointer_interaction = PointerInteraction::None;
            self.diagram_selection = DiagramSelection::None;
        }
        if expand_all_requested {
            if let Some(document) = &self.document {
                expanded_nodes.clear();
                collect_expandable_paths(
                    &build_tree(&document.package_name, &document.class_names),
                    &mut expanded_nodes,
                );
            }
        } else if collapse_all_requested {
            expanded_nodes.clear();
        }
        self.expanded_nodes = expanded_nodes;
        if expand_all_requested || collapse_all_requested {
            self.window.request_redraw();
        }
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);

        if fit_requested {
            self.fit_scene();
            self.window.request_redraw();
        }

        if view_changed {
            self.fit_scene();
            self.window.request_redraw();
        }

        if open_requested {
            if let Some(path) = FileDialog::new()
                .add_filter("Modelica", &["mo"])
                .pick_file()
            {
                match LoadedDocument::load(&path) {
                    Ok(document) => {
                        self.scene =
                            build_scene(&self.device, &self.style_layout, Some(&document), None);
                        self.diagram_scene = build_diagram_scene(
                            &self.device,
                            &self.style_layout,
                            Some(&document),
                            None,
                        );
                        self.document = Some(document);
                        self.selected_class = None;
                        self.expanded_nodes.clear();
                        self.canvas_rect = None;
                        self.pointer_interaction = PointerInteraction::None;
                        self.diagram_selection = DiagramSelection::None;
                        self.load_error = None;
                        self.update_title(None);
                    }
                    Err(error) => {
                        self.load_error = Some(error);
                    }
                }
                self.window.request_redraw();
            }
        }

        if let Some(class_name) = class_clicked {
            let has_visual = self.document.as_ref().is_some_and(|document| {
                document.icon(&class_name).is_some() || document.diagram(&class_name).is_some()
            });
            if has_visual {
                self.scene = build_scene(
                    &self.device,
                    &self.style_layout,
                    self.document.as_ref(),
                    Some(&class_name),
                );
            } else {
                self.scene = build_scene(
                    &self.device,
                    &self.style_layout,
                    self.document.as_ref(),
                    None,
                );
            }
            self.diagram_scene = build_diagram_scene(
                &self.device,
                &self.style_layout,
                self.document.as_ref(),
                Some(&class_name),
            );
            self.selected_class = Some(class_name);
            self.main_view = MainView::Source;
            self.canvas_rect = None;
            self.pointer_interaction = PointerInteraction::None;
            self.diagram_selection = DiagramSelection::None;
            self.fit_scene();
            self.update_title(None);
            self.window.request_redraw();
        }

        // The base glass/background belongs below the native GPU scene. If it
        // is emitted by egui instead, it is composited after the scene and
        // makes Modelica colors and strokes look washed out.
        self.queue.write_buffer(
            &self.background_buffer,
            0,
            bytemuck::bytes_of(&background_uniform()),
        );
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: self.window.scale_factor() as f32,
        };
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("modelica-wgpu encoder"),
            });
        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }
        let mut command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("modelica-wgpu render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_view,
                    resolve_target: Some(&view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: if is_dark_theme() { 0.055 } else { 0.953 },
                            g: if is_dark_theme() { 0.059 } else { 0.957 },
                            b: if is_dark_theme() { 0.082 } else { 0.973 },
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_pipeline(&self.background_pipeline);
            pass.set_bind_group(0, &self.background_bind_group, &[]);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.view_bind_group, &[]);
            if matches!(self.main_view, MainView::Icon | MainView::Diagram) {
                if let Some(rect) = icon_clip_rect {
                    let pixels_per_point = self.window.scale_factor() as f32;
                    let left = (rect.left() * pixels_per_point)
                        .floor()
                        .clamp(0.0, self.config.width.saturating_sub(1) as f32)
                        as u32;
                    let top = (rect.top() * pixels_per_point)
                        .floor()
                        .clamp(0.0, self.config.height.saturating_sub(1) as f32)
                        as u32;
                    let right = (rect.right() * pixels_per_point)
                        .ceil()
                        .clamp((left + 1) as f32, self.config.width as f32)
                        as u32;
                    let bottom = (rect.bottom() * pixels_per_point)
                        .ceil()
                        .clamp((top + 1) as f32, self.config.height as f32)
                        as u32;
                    pass.set_scissor_rect(left, top, right - left, bottom - top);
                    let active_scene = if self.main_view == MainView::Diagram {
                        &self.diagram_scene
                    } else {
                        &self.scene
                    };
                    for geometry in &active_scene.geometries {
                        pass.set_bind_group(1, &geometry.style_bind_group, &[]);
                        pass.set_vertex_buffer(0, geometry.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            geometry.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint16,
                        );
                        pass.draw_indexed(0..geometry.index_count, 0, 0..1);
                    }
                }
            }
        }
        {
            let mut ui_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("modelica-wgpu egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.egui_renderer
                .render(&mut ui_pass, &paint_jobs, &screen_descriptor);
        }
        command_buffers.push(encoder.finish());
        self.queue.submit(command_buffers);
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        frame.present();

        if let Some((fps, worst_ms)) = self.stats.record(Instant::now()) {
            self.update_title(Some((fps, worst_ms)));
        }
        Ok(())
    }
}

fn create_msaa_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("modelica-wgpu MSAA color"),
            size: wgpu::Extent3d {
                width: config.width.max(1),
                height: config.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: MSAA_SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn theme_rgb(red: u8, green: u8, blue: u8) -> Color32 {
    Color32::from_rgb(red, green, blue)
}

static DARK_THEME: AtomicBool = AtomicBool::new(false);
static ACTIVE_ACCENT_ID: AtomicU8 = AtomicU8::new(0);

fn set_theme(enabled: bool, accent: AccentTheme) {
    DARK_THEME.store(enabled, Ordering::Relaxed);
    ACTIVE_ACCENT_ID.store(accent.id(), Ordering::Relaxed);
}

fn is_dark_theme() -> bool {
    DARK_THEME.load(Ordering::Relaxed)
}

fn active_accent() -> AccentTheme {
    AccentTheme::from_id(ACTIVE_ACCENT_ID.load(Ordering::Relaxed))
}

fn theme_rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
}

fn theme_surface() -> Color32 {
    if is_dark_theme() {
        theme_rgb(29, 31, 42)
    } else {
        theme_rgb(255, 255, 255)
    }
}

fn theme_surface_soft(alpha: u8) -> Color32 {
    if is_dark_theme() {
        theme_rgba(45, 48, 63, alpha)
    } else {
        theme_rgba(248, 249, 252, alpha)
    }
}

fn theme_surface_raised(alpha: u8) -> Color32 {
    if is_dark_theme() {
        theme_rgba(24, 26, 36, alpha)
    } else {
        theme_rgba(255, 255, 255, alpha)
    }
}

fn theme_text_primary() -> Color32 {
    if is_dark_theme() {
        theme_rgb(238, 239, 247)
    } else {
        theme_rgb(32, 35, 43)
    }
}

// Light-theme UI text tokens: primary #20232B, secondary #505664,
// muted #737A89, disabled #A1A6B2. These colors apply to UI chrome only.
fn theme_text_secondary() -> Color32 {
    if is_dark_theme() {
        theme_rgb(181, 184, 201)
    } else {
        theme_rgb(80, 86, 100)
    }
}

#[allow(dead_code)]
fn theme_text_disabled() -> Color32 {
    if is_dark_theme() {
        theme_rgb(104, 108, 124)
    } else {
        theme_rgb(161, 166, 178)
    }
}

fn theme_text_tertiary() -> Color32 {
    if is_dark_theme() {
        theme_rgb(143, 147, 169)
    } else {
        theme_rgb(115, 122, 137)
    }
}

fn theme_border(alpha: u8) -> Color32 {
    if is_dark_theme() {
        theme_rgba(220, 223, 240, alpha)
    } else {
        theme_rgba(32, 33, 40, alpha)
    }
}

fn theme_accent() -> Color32 {
    let [red, green, blue]: [u8; 3] = match active_accent() {
        AccentTheme::Violet => [108, 92, 231],
        AccentTheme::Blue => [76, 141, 255],
        AccentTheme::Cyan => [39, 174, 186],
        AccentTheme::Orange => [221, 123, 57],
    };
    if is_dark_theme() {
        theme_rgb(
            red.saturating_add(24),
            green.saturating_add(20),
            blue.saturating_add(14),
        )
    } else {
        theme_rgb(red, green, blue)
    }
}

fn theme_accent_strong() -> Color32 {
    let [red, green, blue]: [u8; 3] = match active_accent() {
        AccentTheme::Violet => [91, 75, 214],
        AccentTheme::Blue => [52, 120, 238],
        AccentTheme::Cyan => [22, 141, 153],
        AccentTheme::Orange => [196, 102, 41],
    };
    if is_dark_theme() {
        theme_rgb(
            red.saturating_add(38),
            green.saturating_add(32),
            blue.saturating_add(30),
        )
    } else {
        theme_rgb(red, green, blue)
    }
}

fn theme_accent_soft(alpha: u8) -> Color32 {
    let accent = theme_accent();
    theme_rgba(accent.r(), accent.g(), accent.b(), alpha)
}

fn background_uniform() -> BackgroundUniform {
    let base = if is_dark_theme() {
        [0.055, 0.059, 0.082]
    } else {
        [0.953, 0.957, 0.973]
    };
    let accent = theme_accent();
    let accent = [
        srgb_to_linear(accent.r()),
        srgb_to_linear(accent.g()),
        srgb_to_linear(accent.b()),
    ];
    let color = |strength: f32| {
        [
            base[0] * (1.0 - strength) + accent[0] * strength,
            base[1] * (1.0 - strength) + accent[1] * strength,
            base[2] * (1.0 - strength) + accent[2] * strength,
            1.0,
        ]
    };
    BackgroundUniform {
        top_left: color(0.08),
        top_right: color(0.0),
        bottom_left: color(0.22),
        bottom_right: color(0.035),
    }
}

fn theme_live() -> Color32 {
    if is_dark_theme() {
        theme_rgb(85, 207, 163)
    } else {
        theme_rgb(54, 174, 124)
    }
}

fn draw_preview_ui(
    ctx: &egui::Context,
    main_view: &mut MainView,
    theme_mode: &mut ThemeMode,
    accent_theme: &mut AccentTheme,
    selected_class: Option<&str>,
    document: Option<&UiDocument>,
    expanded_nodes: &mut HashSet<String>,
    open_requested: &mut bool,
    class_clicked: &mut Option<String>,
    fit_requested: &mut bool,
    view_changed: &mut bool,
    icon_clip_rect: &mut Option<egui::Rect>,
    expand_all_requested: &mut bool,
    collapse_all_requested: &mut bool,
    load_error: Option<&str>,
) {
    let mut visuals = if is_dark_theme() {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    // Keep unstyled egui labels as readable as the explicitly colored labels.
    visuals.override_text_color = Some(theme_text_primary());
    ctx.set_visuals(visuals);
    egui::TopBottomPanel::top("arc_topbar")
        .exact_height(68.0)
        .frame(Frame::none().fill(theme_surface_raised(205)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(18.0);
                ui.label(RichText::new("◇").size(28.0).color(theme_accent()));
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("MODELICA")
                            .size(12.0)
                            .strong()
                            .color(theme_accent()),
                    );
                    ui.label(RichText::new("WGPU Studio").size(16.0).strong());
                });
                ui.add_space(28.0);
                ui.add_sized(
                    [410.0, 36.0],
                    egui::Button::new(
                        RichText::new("⌘ K    Search models, classes, commands…")
                            .size(14.0)
                            .font(ui_font(14.0))
                            .color(theme_text_secondary()),
                    )
                    .fill(theme_surface_soft(235))
                    .stroke(Stroke::new(1.0_f32, theme_border(23)))
                    .rounding(Rounding::same(12.0)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(18.0);
                    ui.menu_button(
                        RichText::new(format!("Appearance · {}  ▾", theme_mode.label()))
                            .size(13.0)
                            .font(ui_font(13.0)),
                        |ui| {
                            ui.set_min_width(180.0);
                            ui.label(RichText::new("Theme").size(10.0).strong());
                            for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
                                if ui
                                    .selectable_label(*theme_mode == mode, mode.label())
                                    .clicked()
                                {
                                    *theme_mode = mode;
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            ui.label(RichText::new("Accent").size(10.0).strong());
                            for accent in [
                                AccentTheme::Violet,
                                AccentTheme::Blue,
                                AccentTheme::Cyan,
                                AccentTheme::Orange,
                            ] {
                                if ui
                                    .selectable_label(*accent_theme == accent, accent.label())
                                    .clicked()
                                {
                                    *accent_theme = accent;
                                    ui.close_menu();
                                }
                            }
                        },
                    );
                    ui.label(
                        RichText::new("Preview mode")
                            .size(12.0)
                            .font(ui_font(12.0))
                            .color(theme_text_tertiary()),
                    );
                });
            });
        });

    egui::SidePanel::left("library_panel")
        .resizable(true)
        .default_width(258.0)
        .min_width(224.0)
        .max_width(340.0)
        .frame(
            Frame::none()
                .fill(theme_surface_soft(205))
                .inner_margin(Margin::symmetric(18.0, 12.0)),
        )
        .show(ctx, |ui| {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("MODEL LIBRARY")
                        .size(12.0)
                        .font(ui_semibold_font(12.0))
                        .strong()
                        .color(theme_text_tertiary()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let class_count = document.map_or(70, |doc| doc.class_names.len());
                    ui.label(
                        RichText::new(class_count.to_string())
                            .size(12.0)
                            .font(ui_font(12.0))
                            .color(theme_text_tertiary()),
                    );
                });
            });
            ui.add_space(12.0);
            let open_button = ui.add_sized(
                [ui.available_width(), 34.0],
                egui::Button::new(
                    RichText::new("＋  Open Modelica file")
                        .size(13.0)
                        .font(ui_semibold_font(13.0))
                        .color(theme_surface()),
                )
                .fill(theme_accent())
                .rounding(Rounding::same(9.0)),
            );
            if open_button.clicked() {
                *open_requested = true;
            }
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);
            if let Some(document) = document {
                ui.horizontal(|ui| {
                    let button_width = (ui.available_width() - 6.0) / 2.0;
                    if ui
                        .add_sized(
                            [button_width, 26.0],
                            egui::Button::new(
                                RichText::new("Expand all")
                                    .size(11.0)
                                    .font(ui_semibold_font(11.0)),
                            )
                            .fill(theme_surface_soft(235))
                            .rounding(Rounding::same(7.0)),
                        )
                        .clicked()
                    {
                        *expand_all_requested = true;
                    }
                    if ui
                        .add_sized(
                            [button_width, 26.0],
                            egui::Button::new(
                                RichText::new("Collapse all")
                                    .size(11.0)
                                    .font(ui_semibold_font(11.0)),
                            )
                            .fill(theme_surface_soft(235))
                            .rounding(Rounding::same(7.0)),
                        )
                        .clicked()
                    {
                        *collapse_all_requested = true;
                    }
                });
                ui.add_space(6.0);
                if let Some(clicked) =
                    document_tree(ui, &document.tree, selected_class, expanded_nodes)
                {
                    *class_clicked = Some(clicked);
                }
            }
            if let Some(error) = load_error {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("Load failed: {error}"))
                        .size(10.0)
                        .color(theme_rgb(190, 70, 70)),
                );
            }
        });

    egui::CentralPanel::default()
        .frame(
            Frame::none()
                // Keep the GPU canvas colors untouched. A translucent white
                // panel here is drawn after wgpu and washes out every icon
                // and Diagram line.
                .fill(Color32::TRANSPARENT)
                .inner_margin(Margin::symmetric(18.0, 12.0)),
        )
        .show(ctx, |ui| {
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let package_name = document.map_or("Modelica", |doc| doc.package_name.as_str());
                ui.label(
                    RichText::new(package_name)
                        .size(22.0)
                        .font(ui_semibold_font(22.0))
                        .strong()
                        .color(theme_text_primary()),
                );
                ui.label(
                    RichText::new(if let Some(class_name) = selected_class {
                        format!(
                            "  /  {}",
                            class_name.rsplit('.').next().unwrap_or(class_name)
                        )
                    } else if document.is_some() {
                        "  /  No class selected".to_owned()
                    } else {
                        String::new()
                    })
                    .size(13.0)
                    .font(ui_font(13.0))
                    .color(theme_text_tertiary()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(match document {
                            Some(doc) => format!("Loaded · {} classes", doc.class_names.len()),
                            None => "GPU canvas ready".to_owned(),
                        })
                        .size(12.0)
                        .font(ui_font(12.0))
                        .color(theme_live()),
                    );
                });
            });
            ui.add_space(12.0);
            glass_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for view in [MainView::Source, MainView::Icon, MainView::Diagram] {
                        let selected = *main_view == view;
                        let button = egui::Button::new(
                            RichText::new(view.label())
                                .size(13.0)
                                .font(if selected {
                                    ui_semibold_font(13.0)
                                } else {
                                    ui_font(13.0)
                                })
                                .color(if selected {
                                    theme_accent()
                                } else {
                                    theme_text_secondary()
                                }),
                        )
                        .fill(if selected {
                            theme_accent_soft(235)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .rounding(Rounding::same(8.0));
                        if ui.add(button).clicked() {
                            *main_view = view;
                            *view_changed = true;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let fit_button = ui.add_sized(
                            [54.0, 28.0],
                            egui::Button::new(
                                RichText::new("Fit").size(12.0).font(ui_semibold_font(12.0)),
                            )
                            .fill(theme_surface_soft(230))
                            .rounding(Rounding::same(7.0)),
                        );
                        if fit_button.clicked() {
                            *fit_requested = true;
                        }
                        ui.label(
                            RichText::new("100%")
                                .size(12.0)
                                .font(ui_font(12.0))
                                .color(theme_text_tertiary()),
                        );
                        ui.label(
                            RichText::new("−  +")
                                .size(16.0)
                                .font(ui_font(16.0))
                                .color(theme_text_secondary()),
                        );
                    });
                });
                ui.separator();
                match *main_view {
                    MainView::Source => source_preview(ui, document),
                    MainView::Icon => icon_preview(ui, document, icon_clip_rect),
                    MainView::Diagram => diagram_preview(ui, document, icon_clip_rect),
                }
            });
        });
}

fn glass_frame() -> Frame {
    Frame::none()
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0_f32, theme_border(23)))
        .rounding(Rounding::same(16.0))
        .inner_margin(Margin::same(12.0))
}

fn tree_row(
    ui: &mut egui::Ui,
    marker: &str,
    label: &str,
    selected: bool,
    indent: f32,
) -> egui::Response {
    let row_width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(row_width, 31.0), Sense::click());
    let fill = if selected {
        theme_accent_soft(220)
    } else if response.hovered() {
        theme_surface_raised(80)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, Rounding::same(8.0), fill);
    ui.painter().text(
        Pos2::new(rect.left() + indent * 18.0 + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        format!("{marker}  {label}"),
        if selected {
            ui_semibold_font(13.0)
        } else {
            ui_font(13.0)
        },
        if selected {
            theme_accent_strong()
        } else {
            theme_text_secondary()
        },
    );
    response
}

fn document_tree(
    ui: &mut egui::Ui,
    root: &TreeNode,
    selected_class: Option<&str>,
    expanded_nodes: &mut HashSet<String>,
) -> Option<String> {
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_tree_node(ui, root, 0, selected_class, expanded_nodes, &mut clicked);
        });
    clicked
}

fn render_tree_node(
    ui: &mut egui::Ui,
    node: &TreeNode,
    depth: usize,
    selected_class: Option<&str>,
    expanded_nodes: &mut HashSet<String>,
    clicked: &mut Option<String>,
) {
    let has_children = !node.children.is_empty();
    let expanded = expanded_nodes.contains(&node.qualified_name);
    let marker = if has_children {
        if expanded {
            "▾"
        } else {
            "▸"
        }
    } else {
        "□"
    };
    let selected = selected_class.is_some() && node.class_name.as_deref() == selected_class;
    let response = tree_row(ui, marker, &node.name, selected, depth as f32);
    if response.clicked() {
        if has_children {
            if expanded {
                expanded_nodes.remove(&node.qualified_name);
            } else {
                expanded_nodes.insert(node.qualified_name.clone());
            }
        } else if let Some(class_name) = &node.class_name {
            *clicked = Some(class_name.clone());
        }
    }
    if has_children && expanded_nodes.contains(&node.qualified_name) {
        for child in &node.children {
            render_tree_node(
                ui,
                child,
                depth + 1,
                selected_class,
                expanded_nodes,
                clicked,
            );
        }
    }
}

fn collect_expandable_paths(node: &TreeNode, output: &mut HashSet<String>) {
    if node.children.is_empty() {
        return;
    }
    output.insert(node.qualified_name.clone());
    for child in &node.children {
        collect_expandable_paths(child, output);
    }
}

fn source_preview(ui: &mut egui::Ui, document: Option<&UiDocument>) {
    let frame = Frame::none()
        .fill(theme_surface())
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::same(18.0));
    frame.show(ui, |ui| {
        if let Some(document) = document {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if document.source_name.is_empty() {
                        "No class selected"
                    } else {
                        document.source_name.as_str()
                    })
                    .size(13.0)
                    .strong(),
                );
                ui.label(
                    RichText::new(if document.source_name.is_empty() {
                        "click a model in the library"
                    } else {
                        "read-only source"
                    })
                    .size(11.0)
                    .color(if document.source_name.is_empty() {
                        theme_text_tertiary()
                    } else {
                        theme_live()
                    }),
                );
            });
        }
        ui.add_space(12.0);
        if let Some(document) = document {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (number, line) in document.source_lines.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:>4}", number + 1))
                                    .monospace()
                                    .size(13.0)
                                    .color(theme_text_tertiary()),
                            );
                            ui.add(
                                egui::Label::new(modelica_layout_job(line, &document.class_names))
                                    .selectable(false),
                            );
                        });
                    }
                });
            if document.source_lines.is_empty() {
                ui.add_space(32.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Select a model to view its Modelica source")
                            .size(14.0)
                            .color(theme_text_secondary()),
                    );
                });
            }
        }
    });
}

fn modelica_layout_job(line: &str, class_names: &[String]) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font_id = FontId::monospace(13.0);
    let tokens = tokenize(line);
    for (index, token) in tokens.iter().enumerate() {
        let color = match token.kind {
            TokenKind::Keyword => theme_accent_strong(),
            TokenKind::Number => theme_rgb(185, 105, 35),
            TokenKind::String => theme_rgb(44, 133, 91),
            TokenKind::Comment => theme_text_tertiary(),
            TokenKind::Punctuation => theme_text_secondary(),
            TokenKind::Identifier => identifier_color(&tokens, index, class_names),
            TokenKind::Unknown | TokenKind::Whitespace => theme_text_primary(),
        };
        job.append(
            &token.text,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color,
                ..Default::default()
            },
        );
    }
    job
}

fn identifier_color(tokens: &[Token], index: usize, class_names: &[String]) -> Color32 {
    let token = &tokens[index];
    if is_builtin_type(&token.text)
        || class_names
            .iter()
            .any(|name| name == &token.text || name.rsplit('.').next() == Some(token.text.as_str()))
    {
        return theme_rgb(37, 126, 158);
    }

    if adjacent_punctuation(tokens, index, ".") {
        return theme_rgb(100, 82, 164);
    }

    if next_non_trivia(tokens, index).is_some_and(|next| next.text == "(") {
        return theme_rgb(157, 91, 36);
    }

    theme_text_primary()
}

fn is_builtin_type(value: &str) -> bool {
    matches!(
        value,
        "Real" | "Integer" | "Boolean" | "String" | "Clock" | "Complex"
    )
}

fn adjacent_punctuation(tokens: &[Token], index: usize, punctuation: &str) -> bool {
    let before = index
        .checked_sub(1)
        .and_then(|index| previous_non_trivia(tokens, index));
    let after = next_non_trivia(tokens, index);
    before.is_some_and(|token| token.text == punctuation)
        || after.is_some_and(|token| token.text == punctuation)
}

fn previous_non_trivia(tokens: &[Token], mut index: usize) -> Option<&Token> {
    loop {
        let token = tokens.get(index)?;
        if !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment) {
            return Some(token);
        }
        index = index.checked_sub(1)?;
    }
}

fn next_non_trivia(tokens: &[Token], mut index: usize) -> Option<&Token> {
    index += 1;
    while let Some(token) = tokens.get(index) {
        if !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment) {
            return Some(token);
        }
        index += 1;
    }
    None
}

fn icon_preview(
    ui: &mut egui::Ui,
    document: Option<&UiDocument>,
    icon_clip_rect: &mut Option<egui::Rect>,
) {
    let available = ui.available_size();
    let painter = ui.painter().clone();
    let content_origin = ui.cursor().min;
    let canvas_size = Vec2::new(available.x, (available.y - 8.0).max(160.0));
    let (rect, _) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
    // Keep the native GPU pass inside both the canvas and the parent egui
    // clip rectangle. This prevents the scene from leaking into the tabs,
    // header, or the rounded card margins.
    *icon_clip_rect = Some(rect.intersect(ui.clip_rect()));
    // Keep the GPU layer visible below the glass surface. This matters for
    // valid Modelica icons that use very light fills, such as FluidUnits.Flash.
    // The icon is rendered by the GPU pass underneath egui. Keep this layer
    // transparent so annotation colors and gradient fills are not washed out.
    painter.rect_filled(rect, Rounding::same(12.0), Color32::TRANSPARENT);
    painter.rect_stroke(
        rect,
        Rounding::same(12.0),
        Stroke::new(1.0_f32, theme_border(28)),
    );
    painter.text(
        Pos2::new(content_origin.x + 8.0, content_origin.y + 8.0),
        Align2::LEFT_TOP,
        format!(
            "ICON CANVAS  ·  smooth GPU rendering  ·  {}",
            match document {
                Some(document) if document.selected_class.is_some() => {
                    format!("{} graphics", document.icon_graphics)
                }
                Some(_) => "No class selected".to_owned(),
                None => "No file loaded".to_owned(),
            }
        ),
        ui_font(12.0),
        theme_text_tertiary(),
    );
    if document.is_some_and(|document| document.selected_class.is_none()) {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Select a model to preview its Icon",
            ui_font(14.0),
            theme_text_secondary(),
        );
    }
    painter.text(
        Pos2::new(rect.left() + 12.0, rect.bottom() - 12.0),
        Align2::LEFT_BOTTOM,
        "Drag graphic to move   ·   Ctrl + drag / middle drag to pan   ·   Ctrl + wheel to zoom",
        ui_font(11.0),
        theme_text_tertiary(),
    );
}

fn diagram_preview(
    ui: &mut egui::Ui,
    document: Option<&UiDocument>,
    diagram_clip_rect: &mut Option<egui::Rect>,
) {
    let available = ui.available_size();
    let painter = ui.painter().clone();
    let content_origin = ui.cursor().min;
    let canvas_size = Vec2::new(available.x, (available.y - 8.0).max(160.0));
    let (rect, _) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
    *diagram_clip_rect = Some(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, Rounding::same(12.0), Color32::TRANSPARENT);
    painter.rect_stroke(
        rect,
        Rounding::same(12.0),
        Stroke::new(1.0_f32, theme_border(28)),
    );
    let counts = document.map(|document| {
        format!(
            "{} background  ·  {} components ({} own / {} inherited)  ·  {} connectors  ·  {} connections  ·  {} unresolved components / {} unresolved bases",
            document.diagram_background,
            document.diagram_components,
            document.diagram_own_components,
            document.diagram_inherited_components,
            document.diagram_connectors,
            document.diagram_connections,
            document.diagram_unresolved_components,
            document.diagram_unresolved_bases,
        )
    });
    painter.text(
        Pos2::new(content_origin.x + 8.0, content_origin.y + 8.0),
        Align2::LEFT_TOP,
        format!(
            "DIAGRAM CANVAS  ·  {}",
            counts.unwrap_or_else(|| "No file loaded".to_owned())
        ),
        ui_font(12.0),
        theme_text_tertiary(),
    );
    let has_selected_class = document.is_some_and(|document| document.selected_class.is_some());
    let has_diagram_content = document.is_some_and(|document| {
        document.diagram_background > 0
            || document.diagram_components > 0
            || document.diagram_connections > 0
    });
    if !has_selected_class {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Select a model to preview its Diagram",
            ui_font(14.0),
            theme_text_secondary(),
        );
    } else if !has_diagram_content {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No Diagram graphics in the selected model",
            ui_font(14.0),
            theme_text_secondary(),
        );
    }
    painter.text(
        Pos2::new(rect.left() + 12.0, rect.bottom() - 12.0),
        Align2::LEFT_BOTTOM,
        "Click connection to edit   ·   Drag component to move   ·   Ctrl + drag / middle drag to pan   ·   Ctrl + wheel to zoom",
        ui_font(11.0),
        theme_text_tertiary(),
    );
}

fn draw_diagram_selection_overlay(
    ctx: &egui::Context,
    canvas_rect: Option<egui::Rect>,
    points: Option<&[CorePoint]>,
    component: Option<ComponentSelectionOverlay>,
    zoom: f32,
    pan: [f32; 2],
    viewport: [u32; 2],
    pixels_per_point: f32,
) {
    let Some(canvas_rect) = canvas_rect else {
        return;
    };
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("diagram-selection-overlay"),
        ))
        .with_clip_rect(canvas_rect);
    let to_screen = |point: CorePoint| {
        Pos2::new(
            (viewport[0] as f32 * 0.5 + pan[0] + point.x * zoom) / pixels_per_point,
            (viewport[1] as f32 * 0.5 + pan[1] - point.y * zoom) / pixels_per_point,
        )
    };
    let accent = theme_accent();
    if let Some(points) = points.filter(|points| points.len() >= 2) {
        let screen_points = points.iter().copied().map(to_screen).collect::<Vec<_>>();
        for pair in screen_points.windows(2) {
            let [start, end] = pair else {
                continue;
            };
            painter.line_segment([*start, *end], Stroke::new(3.0_f32, theme_accent_soft(170)));
        }
        for (index, point) in screen_points.iter().enumerate() {
            let endpoint = index == 0 || index + 1 == screen_points.len();
            let radius = if endpoint { 4.5 } else { 5.5 };
            painter.circle_filled(
                *point,
                radius,
                if endpoint {
                    theme_surface_raised(245)
                } else {
                    accent
                },
            );
            painter.circle_stroke(
                *point,
                radius,
                Stroke::new(1.5_f32, if endpoint { accent } else { theme_surface() }),
            );
        }
    }
    if let Some(component) = component {
        let corners =
            component_extent_corners(component.origin, component.extent, component.rotation);
        let screen_corners = corners.iter().copied().map(to_screen).collect::<Vec<_>>();
        for index in 0..screen_corners.len() {
            painter.line_segment(
                [
                    screen_corners[index],
                    screen_corners[(index + 1) % screen_corners.len()],
                ],
                Stroke::new(1.5_f32, theme_accent_soft(210)),
            );
            painter.rect_filled(
                egui::Rect::from_center_size(screen_corners[index], Vec2::splat(10.0)),
                Rounding::same(2.0),
                accent,
            );
            painter.rect_stroke(
                egui::Rect::from_center_size(screen_corners[index], Vec2::splat(10.0)),
                Rounding::same(2.0),
                Stroke::new(1.0_f32, theme_surface()),
            );
        }
    }
}

fn build_scene(
    device: &wgpu::Device,
    style_layout: &wgpu::BindGroupLayout,
    document: Option<&LoadedDocument>,
    selected_class: Option<&str>,
) -> GpuIconScene {
    let geometries = document
        .and_then(|document| selected_class.and_then(|name| document.icon(name)))
        .map(core_icon_geometry)
        .unwrap_or_default();
    gpu_scene_from_geometries(device, style_layout, geometries, "icon")
}

fn build_diagram_scene(
    device: &wgpu::Device,
    style_layout: &wgpu::BindGroupLayout,
    document: Option<&LoadedDocument>,
    selected_class: Option<&str>,
) -> GpuIconScene {
    let geometries = document
        .and_then(|document| selected_class.and_then(|name| document.diagram(name)))
        .map(core_diagram_geometry)
        .unwrap_or_default();
    gpu_scene_from_geometries(device, style_layout, geometries, "diagram")
}

fn gpu_scene_from_geometries(
    device: &wgpu::Device,
    style_layout: &wgpu::BindGroupLayout,
    geometries: Vec<Geometry>,
    label: &str,
) -> GpuIconScene {
    let geometries = geometries
        .into_iter()
        .filter(|geometry| !geometry.vertices.is_empty() && !geometry.indices.is_empty())
        .collect::<Vec<_>>();
    let bounds = SceneBounds::from_geometries(&geometries);
    let geometries = geometries
        .into_iter()
        .map(|geometry| {
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label} vertices")),
                contents: bytemuck::cast_slice(&geometry.vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label} indices")),
                contents: bytemuck::cast_slice(&geometry.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let style_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label} style")),
                contents: bytemuck::bytes_of(&geometry.style),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let style_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("{label} style bind group")),
                layout: style_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: style_buffer.as_entire_binding(),
                }],
            });
            let base_vertices = geometry.vertices.clone();
            GpuGeometry {
                vertex_buffer,
                index_buffer,
                index_count: geometry.indices.len() as u32,
                style_bind_group,
                base_vertices,
                edit_key: geometry.edit_key,
                connection: geometry.connection,
                component: geometry.component,
            }
        })
        .collect();
    GpuIconScene { geometries, bounds }
}

fn core_icon_geometry(scene: &CoreIconScene) -> Vec<Geometry> {
    scene
        .graphics
        .iter()
        .flat_map(|resolved| {
            let edit_key = resolved.editable.then(|| resolved.id.0.clone());
            core_graphic_geometry(resolved)
                .into_iter()
                .map(move |mut geometry| {
                    geometry.edit_key = edit_key.clone();
                    geometry
                })
        })
        .collect()
}

fn core_diagram_geometry(scene: &CoreDiagramScene) -> Vec<Geometry> {
    let diagram_flip = Transform2D {
        scale_y: -1.0,
        ..Transform2D::identity()
    };
    let mut geometries = scene
        .background_graphics
        .iter()
        .flat_map(|graphic| core_graphic_geometry_from_graphic(graphic, diagram_flip))
        .collect::<Vec<_>>();
    for connection in &scene.connections {
        if let Some(line) = &connection.line {
            geometries.extend(
                line_geometry(line, diagram_flip)
                    .into_iter()
                    .map(|mut geometry| {
                        geometry.edit_key = Some(connection.id.clone());
                        geometry.connection = Some(ConnectionGeometry {
                            line: line.clone(),
                            transform: diagram_flip,
                        });
                        geometry
                    }),
            );
        }
    }
    for component in &scene.components {
        if !component.visible {
            continue;
        }
        let Some(icon) = component.resolved_icon.as_deref() else {
            continue;
        };
        let placement = diagram_placement_transform(icon, component);
        let parent_component_transform = compose_transform(diagram_flip, placement);
        for resolved in &icon.graphics {
            let graphic_transform = compose_transform(placement, resolved.transform);
            let transform = compose_transform(diagram_flip, graphic_transform);
            geometries.extend(
                core_graphic_geometry_from_graphic(&resolved.graphic, transform)
                    .into_iter()
                    .map(|mut geometry| {
                        geometry.edit_key = Some(component.id.clone());
                        geometry.component = Some(ComponentGeometry {
                            transform: parent_component_transform,
                        });
                        geometry
                    }),
            );
        }
    }
    geometries
}

fn core_graphic_geometry(resolved: &ResolvedGraphic) -> Vec<Geometry> {
    core_graphic_geometry_from_graphic(&resolved.graphic, resolved.transform)
}

fn core_graphic_geometry_from_graphic(
    graphic: &CoreGraphic,
    transform: Transform2D,
) -> Vec<Geometry> {
    match graphic {
        CoreGraphic::Line(line) => line_geometry(line, transform),
        CoreGraphic::Polygon(polygon) => polygon_geometry(polygon, transform),
        CoreGraphic::Rectangle(rectangle) => rectangle_geometry(rectangle, transform),
        CoreGraphic::Ellipse(ellipse) => ellipse_geometry(ellipse, transform),
        CoreGraphic::Text(_) | CoreGraphic::Bitmap(_) => Vec::new(),
    }
}

fn diagram_placement_transform(
    icon: &CoreIconScene,
    component: &CoreComponentInstance,
) -> Transform2D {
    diagram_placement_transform_for_extent(
        icon,
        component.origin,
        component.rotation,
        component
            .placement_extent
            .unwrap_or(modelica_core::scene::Extent {
                p1: CorePoint { x: -10.0, y: -10.0 },
                p2: CorePoint { x: 10.0, y: 10.0 },
            }),
    )
}

fn diagram_placement_transform_for_extent(
    icon: &CoreIconScene,
    origin: CorePoint,
    rotation: f32,
    target: modelica_core::scene::Extent,
) -> Transform2D {
    let source = icon.coordinate_system.extent;
    let source_width = if (source.p2.x - source.p1.x).abs() <= f32::EPSILON {
        1.0
    } else {
        source.p2.x - source.p1.x
    };
    let source_height = if (source.p2.y - source.p1.y).abs() <= f32::EPSILON {
        1.0
    } else {
        source.p2.y - source.p1.y
    };
    let scale_x = (target.p2.x - target.p1.x) / source_width;
    let scale_y = (target.p2.y - target.p1.y) / source_height;
    Transform2D {
        translation: CorePoint {
            x: origin.x + target.p1.x - source.p1.x * scale_x,
            y: origin.y + target.p1.y - source.p1.y * scale_y,
        },
        rotation,
        scale_x,
        scale_y,
    }
}

fn compose_transform(parent: Transform2D, child: Transform2D) -> Transform2D {
    let angle = parent.rotation.to_radians();
    let child_translation = CorePoint {
        x: child.translation.x * parent.scale_x,
        y: child.translation.y * parent.scale_y,
    };
    Transform2D {
        translation: CorePoint {
            x: parent.translation.x + child_translation.x * angle.cos()
                - child_translation.y * angle.sin(),
            y: parent.translation.y
                + child_translation.x * angle.sin()
                + child_translation.y * angle.cos(),
        },
        rotation: parent.rotation + child.rotation,
        scale_x: parent.scale_x * child.scale_x,
        scale_y: parent.scale_y * child.scale_y,
    }
}

fn apply_transform_point(point: CorePoint, transform: Transform2D) -> CorePoint {
    let scaled = CorePoint {
        x: point.x * transform.scale_x,
        y: point.y * transform.scale_y,
    };
    let radians = transform.rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    CorePoint {
        x: scaled.x * cos - scaled.y * sin + transform.translation.x,
        y: scaled.x * sin + scaled.y * cos + transform.translation.y,
    }
}

fn inverse_transform_point(point: CorePoint, transform: Transform2D) -> CorePoint {
    let translated = CorePoint {
        x: point.x - transform.translation.x,
        y: point.y - transform.translation.y,
    };
    let radians = transform.rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let unrotated = CorePoint {
        x: translated.x * cos + translated.y * sin,
        y: -translated.x * sin + translated.y * cos,
    };
    CorePoint {
        x: unrotated.x / nonzero_scale(transform.scale_x),
        y: unrotated.y / nonzero_scale(transform.scale_y),
    }
}

fn component_extent_corners(
    origin: CorePoint,
    extent: modelica_core::scene::Extent,
    rotation: f32,
) -> [CorePoint; 4] {
    let transform = Transform2D {
        translation: origin,
        rotation,
        scale_x: 1.0,
        scale_y: 1.0,
    };
    [
        extent.p1,
        CorePoint {
            x: extent.p2.x,
            y: extent.p1.y,
        },
        extent.p2,
        CorePoint {
            x: extent.p1.x,
            y: extent.p2.y,
        },
    ]
    .map(|point| apply_transform_point(point, transform))
}

fn resized_extent_from_pointer(
    original: modelica_core::scene::Extent,
    origin: CorePoint,
    rotation: f32,
    handle: ResizeHandle,
    pointer_model: CorePoint,
) -> modelica_core::scene::Extent {
    let local = inverse_transform_point(
        pointer_model,
        Transform2D {
            translation: origin,
            rotation,
            scale_x: 1.0,
            scale_y: 1.0,
        },
    );
    let mut extent = original;
    match handle {
        ResizeHandle::Corner(0) => {
            extent.p1 = local;
        }
        ResizeHandle::Corner(1) => {
            extent.p2.x = local.x;
            extent.p1.y = local.y;
        }
        ResizeHandle::Corner(2) => {
            extent.p2 = local;
        }
        ResizeHandle::Corner(3) => {
            extent.p1.x = local.x;
            extent.p2.y = local.y;
        }
        ResizeHandle::Corner(_) => {}
    }
    keep_extent_nonzero(&mut extent, original, handle);
    extent
}

fn keep_extent_nonzero(
    extent: &mut modelica_core::scene::Extent,
    original: modelica_core::scene::Extent,
    handle: ResizeHandle,
) {
    let minimum = ORTHOGONAL_EPSILON;
    let x_direction = if original.p2.x < original.p1.x {
        -1.0
    } else {
        1.0
    };
    let y_direction = if original.p2.y < original.p1.y {
        -1.0
    } else {
        1.0
    };
    if (extent.p2.x - extent.p1.x).abs() < minimum {
        if matches!(handle, ResizeHandle::Corner(0) | ResizeHandle::Corner(3)) {
            extent.p1.x = extent.p2.x - x_direction * minimum;
        } else {
            extent.p2.x = extent.p1.x + x_direction * minimum;
        }
    }
    if (extent.p2.y - extent.p1.y).abs() < minimum {
        if matches!(handle, ResizeHandle::Corner(0) | ResizeHandle::Corner(1)) {
            extent.p1.y = extent.p2.y - y_direction * minimum;
        } else {
            extent.p2.y = extent.p1.y + y_direction * minimum;
        }
    }
}

fn nonzero_scale(scale: f32) -> f32 {
    if scale.abs() <= f32::EPSILON {
        if scale.is_sign_negative() {
            -1.0
        } else {
            1.0
        }
    } else {
        scale
    }
}

fn line_geometry(line: &LineGraphic, transform: Transform2D) -> Vec<Geometry> {
    let points = line
        .points
        .iter()
        .map(|point| transform_graphic_point(*point, line.origin, line.rotation, transform))
        .collect::<Vec<_>>();
    if points.len() < 2 || line_pattern_is_none(line.pattern.as_deref()) {
        return Vec::new();
    }
    vec![stroke_geometry(
        &polyline_path(&points),
        |_| [0.0, 0.0],
        line.thickness.max(0.1) * transform_scale(transform),
        color_rgba(line.color),
    )]
}

fn polygon_geometry(polygon: &PolygonGraphic, transform: Transform2D) -> Vec<Geometry> {
    let points = polygon
        .points
        .iter()
        .map(|point| transform_graphic_point(*point, polygon.origin, polygon.rotation, transform))
        .collect::<Vec<_>>();
    closed_shape_geometry(
        &points,
        polygon.fill_color,
        polygon.fill_pattern.as_deref(),
        polygon.line_color,
        polygon
            .line_pattern
            .as_deref()
            .or(Some("LinePattern.Solid")),
        polygon.line_thickness,
        transform,
    )
}

fn rectangle_geometry(rectangle: &RectangleGraphic, transform: Transform2D) -> Vec<Geometry> {
    let extent = rectangle.extent;
    let points = [
        extent.p1,
        CorePoint {
            x: extent.p2.x,
            y: extent.p1.y,
        },
        extent.p2,
        CorePoint {
            x: extent.p1.x,
            y: extent.p2.y,
        },
    ]
    .into_iter()
    .map(|point| transform_graphic_point(point, rectangle.origin, rectangle.rotation, transform))
    .collect::<Vec<_>>();
    closed_shape_geometry(
        &points,
        rectangle.fill_color,
        rectangle.fill_pattern.as_deref(),
        rectangle.line_color,
        rectangle
            .line_pattern
            .as_deref()
            .or(Some("LinePattern.Solid")),
        rectangle.line_thickness,
        transform,
    )
}

fn ellipse_geometry(ellipse: &EllipseGraphic, transform: Transform2D) -> Vec<Geometry> {
    let points = ellipse_points(ellipse, transform);
    let mut geometry = Vec::new();
    if !fill_pattern_is_none(ellipse.fill_pattern.as_deref()) && points.len() >= 3 {
        let path = closed_path(&points);
        geometry.push(fill_geometry(
            &path,
            local_coordinates(&points),
            fill_style(
                ellipse.fill_color,
                ellipse.line_color,
                ellipse.fill_pattern.as_deref(),
            ),
        ));
    }
    // In Modelica, an omitted linePattern means Solid. Keep an explicit
    // LinePattern.None invisible, but do not drop the default outline for
    // light-filled ellipses such as FluidUnits.Flash.
    let line_pattern = ellipse
        .line_pattern
        .as_deref()
        .or(Some("LinePattern.Solid"));
    if !line_pattern_is_none(line_pattern) && points.len() >= 2 {
        geometry.push(stroke_geometry(
            &polyline_path(&points),
            |_| [0.0, 0.0],
            ellipse.line_thickness.unwrap_or(0.25).max(0.1) * transform_scale(transform),
            color_rgba(ellipse.line_color),
        ));
    }
    geometry
}

fn closed_shape_geometry(
    points: &[[f32; 2]],
    fill_color: [u8; 3],
    fill_pattern: Option<&str>,
    line_color: [u8; 3],
    line_pattern: Option<&str>,
    line_thickness: Option<f32>,
    transform: Transform2D,
) -> Vec<Geometry> {
    if points.len() < 3 {
        return Vec::new();
    }
    let path = closed_path(points);
    let mut geometry = Vec::new();
    if !fill_pattern_is_none(fill_pattern) {
        geometry.push(fill_geometry(
            &path,
            local_coordinates(points),
            fill_style(fill_color, line_color, fill_pattern),
        ));
    }
    if !line_pattern_is_none(line_pattern) {
        geometry.push(stroke_geometry(
            &polyline_path(points),
            |_| [0.0, 0.0],
            line_thickness.unwrap_or(0.25).max(0.1) * transform_scale(transform),
            color_rgba(line_color),
        ));
    }
    geometry
}

fn transform_graphic_point(
    point: CorePoint,
    origin: CorePoint,
    rotation: f32,
    transform: Transform2D,
) -> [f32; 2] {
    let radians = rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let local_x = point.x * cos - point.y * sin + origin.x;
    let local_y = point.x * sin + point.y * cos + origin.y;
    let scaled_x = local_x * transform.scale_x;
    let scaled_y = local_y * transform.scale_y;
    let transform_radians = transform.rotation.to_radians();
    let (sin, cos) = transform_radians.sin_cos();
    [
        scaled_x * cos - scaled_y * sin + transform.translation.x,
        scaled_x * sin + scaled_y * cos + transform.translation.y,
    ]
}

fn transform_scale(transform: Transform2D) -> f32 {
    ((transform.scale_x.abs() + transform.scale_y.abs()) * 0.5).max(0.01)
}

fn ellipse_points(ellipse: &EllipseGraphic, transform: Transform2D) -> Vec<[f32; 2]> {
    let center = CorePoint {
        x: (ellipse.extent.p1.x + ellipse.extent.p2.x) * 0.5,
        y: (ellipse.extent.p1.y + ellipse.extent.p2.y) * 0.5,
    };
    let radius_x = (ellipse.extent.p2.x - ellipse.extent.p1.x).abs() * 0.5;
    let radius_y = (ellipse.extent.p2.y - ellipse.extent.p1.y).abs() * 0.5;
    let start = ellipse.start_angle.unwrap_or(0.0);
    let mut end = ellipse.end_angle.unwrap_or(360.0);
    if end <= start {
        end += 360.0;
    }
    let segments = ((end - start).abs() / 6.0).ceil().max(12.0) as usize;
    (0..=segments)
        .map(|index| {
            let angle = (start + (end - start) * index as f32 / segments as f32).to_radians();
            transform_graphic_point(
                CorePoint {
                    x: center.x + radius_x * angle.cos(),
                    y: center.y + radius_y * angle.sin(),
                },
                ellipse.origin,
                ellipse.rotation,
                transform,
            )
        })
        .collect()
}

fn closed_path(points: &[[f32; 2]]) -> Path {
    let mut builder = Path::builder();
    builder.begin(point(points[0][0], points[0][1]));
    for position in &points[1..] {
        builder.line_to(point(position[0], position[1]));
    }
    builder.close();
    builder.build()
}

fn local_coordinates(points: &[[f32; 2]]) -> impl Fn([f32; 2]) -> [f32; 2] + '_ {
    let min_x = points
        .iter()
        .map(|point| point[0])
        .fold(f32::INFINITY, f32::min);
    let max_x = points
        .iter()
        .map(|point| point[0])
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = points
        .iter()
        .map(|point| point[1])
        .fold(f32::INFINITY, f32::min);
    let max_y = points
        .iter()
        .map(|point| point[1])
        .fold(f32::NEG_INFINITY, f32::max);
    let width = (max_x - min_x).max(0.001);
    let height = (max_y - min_y).max(0.001);
    move |position| {
        [
            (position[0] - min_x) / width * 2.0 - 1.0,
            (position[1] - min_y) / height * 2.0 - 1.0,
        ]
    }
}

fn color_rgba(color: [u8; 3]) -> [f32; 4] {
    [
        srgb_to_linear(color[0]),
        srgb_to_linear(color[1]),
        srgb_to_linear(color[2]),
        1.0,
    ]
}

fn srgb_to_linear(value: u8) -> f32 {
    let srgb = value as f32 / 255.0;
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

fn fill_style(color: [u8; 3], edge_color: [u8; 3], pattern: Option<&str>) -> StyleUniform {
    let mode = match pattern.unwrap_or_default() {
        value if value.contains("HorizontalCylinder") => FillMode::HorizontalCylinder,
        value if value.contains("VerticalCylinder") => FillMode::VerticalCylinder,
        value if value.contains("Sphere") => FillMode::Sphere,
        _ => FillMode::Solid,
    };
    StyleUniform {
        color: color_rgba(color),
        edge_color: color_rgba(edge_color),
        gradient: [-0.22, -0.28, 1.0, 0.0],
        mode: mode as u32,
        _padding: [0; 7],
    }
}

fn fill_pattern_is_none(pattern: Option<&str>) -> bool {
    pattern.is_some_and(|value| value.contains("None"))
}

fn line_pattern_is_none(pattern: Option<&str>) -> bool {
    pattern.is_some_and(|value| value.contains("None"))
}

fn fill_geometry<F>(path: &Path, local: F, style: StyleUniform) -> Geometry
where
    F: Fn([f32; 2]) -> [f32; 2],
{
    let mut buffers: VertexBuffers<[f32; 2], u16> = VertexBuffers::new();
    FillTessellator::new()
        .tessellate_path(
            path,
            &FillOptions::default(),
            &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| {
                vertex.position().to_array()
            }),
        )
        .expect("fill tessellation failed");
    Geometry {
        vertices: buffers
            .vertices
            .into_iter()
            .map(|position| Vertex {
                position,
                local: local(position),
            })
            .collect(),
        indices: buffers.indices,
        style,
        edit_key: None,
        connection: None,
        component: None,
    }
}

fn stroke_geometry<F>(path: &Path, local: F, width: f32, color: [f32; 4]) -> Geometry
where
    F: Fn([f32; 2]) -> [f32; 2],
{
    let mut buffers: VertexBuffers<[f32; 2], u16> = VertexBuffers::new();
    StrokeTessellator::new()
        .tessellate_path(
            path,
            &StrokeOptions::default().with_line_width(width),
            &mut BuffersBuilder::new(&mut buffers, |vertex: StrokeVertex| {
                vertex.position().to_array()
            }),
        )
        .expect("stroke tessellation failed");
    Geometry {
        vertices: buffers
            .vertices
            .into_iter()
            .map(|position| Vertex {
                position,
                local: local(position),
            })
            .collect(),
        indices: buffers.indices,
        style: StyleUniform {
            color,
            edge_color: color,
            gradient: [0.0; 4],
            mode: FillMode::Solid as u32,
            _padding: [0; 7],
        },
        edit_key: None,
        connection: None,
        component: None,
    }
}

fn polyline_path(points: &[[f32; 2]]) -> Path {
    let mut builder = Path::builder();
    builder.begin(point(points[0][0], points[0][1]));
    for position in &points[1..] {
        builder.line_to(point(position[0], position[1]));
    }
    builder.end(false);
    builder.build()
}

const BACKGROUND_SHADER: &str = r#"
struct BackgroundUniform {
    top_left: vec4<f32>,
    top_right: vec4<f32>,
    bottom_left: vec4<f32>,
    bottom_right: vec4<f32>,
};

@group(0) @binding(0) var<uniform> background: BackgroundUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let position = positions[index];
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = vec2<f32>((position.x + 1.0) * 0.5, (1.0 - position.y) * 0.5);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let top = mix(background.top_left, background.top_right, input.uv.x);
    let bottom = mix(background.bottom_left, background.bottom_right, input.uv.x);
    return mix(top, bottom, input.uv.y);
}
"#;

const SHADER: &str = r#"
struct ViewUniform {
    viewport: vec4<f32>,
    view: vec4<f32>,
};

struct StyleUniform {
    color: vec4<f32>,
    edge_color: vec4<f32>,
    gradient: vec4<f32>,
    mode: u32,
    _padding: vec3<u32>,
};

@group(0) @binding(0) var<uniform> view: ViewUniform;
@group(1) @binding(0) var<uniform> style: StyleUniform;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) local: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let pixel = vec2<f32>(
        view.viewport.x * 0.5 + input.position.x * view.view.x + view.view.y,
        view.viewport.y * 0.5 + input.position.y * view.view.x + view.view.z,
    );
    let clip = vec2<f32>(
        pixel.x / view.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / view.viewport.y * 2.0,
    );
    var output: VertexOutput;
    output.position = vec4<f32>(clip, 0.0, 1.0);
    output.local = input.local;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if style.mode == 0u {
        return style.color;
    }

    // Electron's SVG renderer uses the annotation lineColor at the rim and
    // fillColor at the center for cylinder fills. Keep the same semantics in
    // the GPU path instead of deriving a gray shade from fillColor alone.
    var edge_amount = 0.0;
    if style.mode == 1u {
        edge_amount = abs(input.local.y);
    } else if style.mode == 2u {
        edge_amount = abs(input.local.x);
    } else if style.mode == 3u {
        let distance_from_center = length(input.local - style.gradient.xy);
        edge_amount = smoothstep(0.45, 1.0, distance_from_center);
    }
    return mix(style.color, style.edge_color, clamp(edge_amount, 0.0, 1.0));
}
"#;

fn canvas_navigation_enabled_for(main_view: MainView) -> bool {
    matches!(main_view, MainView::Icon | MainView::Diagram)
}

fn canvas_event_allowed_for(main_view: MainView, pointer_over_canvas: bool) -> bool {
    canvas_navigation_enabled_for(main_view) && pointer_over_canvas
}

fn should_zoom_canvas(
    main_view: MainView,
    pointer_over_canvas: bool,
    control_pressed: bool,
) -> bool {
    canvas_event_allowed_for(main_view, pointer_over_canvas) && control_pressed
}

fn delta_is_zero(delta: CorePoint) -> bool {
    delta.x.abs() <= f32::EPSILON && delta.y.abs() <= f32::EPSILON
}

fn graphic_origin(graphic: &CoreGraphic) -> CorePoint {
    match graphic {
        CoreGraphic::Line(value) => value.origin,
        CoreGraphic::Polygon(value) => value.origin,
        CoreGraphic::Rectangle(value) => value.origin,
        CoreGraphic::Ellipse(value) => value.origin,
        CoreGraphic::Text(value) => value.origin,
        CoreGraphic::Bitmap(value) => value.origin,
    }
}

fn translated_graphic(graphic: &CoreGraphic, delta: CorePoint) -> CoreGraphic {
    let mut translated = graphic.clone();
    match &mut translated {
        CoreGraphic::Line(value) => {
            value.origin.x += delta.x;
            value.origin.y += delta.y;
        }
        CoreGraphic::Polygon(value) => {
            value.origin.x += delta.x;
            value.origin.y += delta.y;
        }
        CoreGraphic::Rectangle(value) => {
            value.origin.x += delta.x;
            value.origin.y += delta.y;
        }
        CoreGraphic::Ellipse(value) => {
            value.origin.x += delta.x;
            value.origin.y += delta.y;
        }
        CoreGraphic::Text(value) => {
            value.origin.x += delta.x;
            value.origin.y += delta.y;
        }
        CoreGraphic::Bitmap(value) => {
            value.origin.x += delta.x;
            value.origin.y += delta.y;
        }
    }
    translated
}

fn diagram_component_contains_point(
    component: &CoreComponentInstance,
    point: CorePoint,
    tolerance: f32,
) -> bool {
    let Some(icon) = component.resolved_icon.as_deref() else {
        return false;
    };
    let diagram_flip = Transform2D {
        scale_y: -1.0,
        ..Transform2D::identity()
    };
    let placement = diagram_placement_transform(icon, component);
    let render_point = CorePoint {
        x: point.x,
        y: -point.y,
    };
    icon.graphics.iter().any(|resolved| {
        let mut candidate = resolved.clone();
        candidate.transform = compose_transform(
            diagram_flip,
            compose_transform(placement, resolved.transform),
        );
        resolved_graphic_contains_point(&candidate, render_point, tolerance)
    })
}

fn connection_world_points(line: &LineGraphic, points: &[CorePoint]) -> Vec<CorePoint> {
    points
        .iter()
        .map(|point| line_local_to_world(line, *point))
        .collect()
}

fn hit_test_connection(
    connections: &[modelica_core::scene::DiagramConnection],
    pointer: CorePoint,
    tolerance: f32,
) -> Option<ConnectionHit> {
    for connection in connections.iter().rev() {
        let Some(line) = connection.line.as_ref() else {
            continue;
        };
        let points = connection_world_points(line, &line.points);
        for (index, pair) in points.windows(2).enumerate() {
            let [start, end] = pair else {
                continue;
            };
            if distance_to_segment(pointer, *start, *end) > tolerance {
                continue;
            }
            let target = if index > 0 && index + 1 < points.len() - 1 {
                match segment_orientation(*start, *end) {
                    Some(orientation) => ConnectionHitTarget::Segment { index, orientation },
                    None => ConnectionHitTarget::Line,
                }
            } else {
                ConnectionHitTarget::Line
            };
            return Some(ConnectionHit {
                connection_id: connection.id.clone(),
                target,
            });
        }
    }
    None
}

fn distance_between(first: CorePoint, second: CorePoint) -> f32 {
    ((first.x - second.x).powi(2) + (first.y - second.y).powi(2)).sqrt()
}

fn distance_to_segment(point: CorePoint, start: CorePoint, end: CorePoint) -> f32 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= f32::EPSILON {
        return distance_between(point, start);
    }
    let projection =
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
    distance_between(
        point,
        CorePoint {
            x: start.x + projection * dx,
            y: start.y + projection * dy,
        },
    )
}

fn segment_orientation(start: CorePoint, end: CorePoint) -> Option<ConnectionSegmentOrientation> {
    if (start.y - end.y).abs() <= ORTHOGONAL_EPSILON {
        Some(ConnectionSegmentOrientation::Horizontal)
    } else if (start.x - end.x).abs() <= ORTHOGONAL_EPSILON {
        Some(ConnectionSegmentOrientation::Vertical)
    } else {
        None
    }
}

fn annotation_calls(source: &str) -> Vec<(usize, AnnotationCall)> {
    let tokens = tokenize(source);
    let mut calls = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.text != "annotation" || token.kind != TokenKind::Keyword {
            continue;
        }
        let Some(open) = next_significant_token(&tokens, index + 1) else {
            continue;
        };
        if tokens[open].text != "(" {
            continue;
        }
        let Some(close) = matching_paren_tokens(&tokens, open) else {
            continue;
        };
        let Some(call_source) = source.get(token.start..tokens[close].end) else {
            continue;
        };
        let Ok(call) = parse_call(call_source) else {
            continue;
        };
        calls.push((token.start, call));
    }
    calls
}

fn next_significant_token(tokens: &[Token], mut index: usize) -> Option<usize> {
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

fn matching_paren_tokens(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if token.text == "(" {
            depth += 1;
        } else if token.text == ")" {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn nested_call<'a>(call: &'a AnnotationCall, name: &str) -> Option<&'a AnnotationCall> {
    call.args.iter().find_map(|entry| {
        entry
            .value
            .as_call()
            .filter(|candidate| candidate.name == name)
    })
}

fn is_graphic_call(call: &AnnotationCall) -> bool {
    matches!(
        call.name.as_str(),
        "Line" | "Polygon" | "Rectangle" | "Ellipse" | "Text" | "Bitmap"
    )
}

fn matching_delimiter(source: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0;
    let mut quoted = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        if *byte == b'"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if *byte == open {
            depth += 1;
        } else if *byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn value_range_for_entry(
    source: &str,
    annotation_start: usize,
    entry: &modelica_core::annotation::AnnotationEntry,
) -> Option<(usize, usize)> {
    let entry_start = annotation_start + entry.source_range.start;
    let entry_end = annotation_start + entry.source_range.end;
    let entry_source = source.get(entry_start..entry_end)?;
    let equals = entry_source.find('=')?;
    let mut value_start = entry_start + equals + 1;
    while source
        .as_bytes()
        .get(value_start)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        value_start += 1;
    }
    let value_end = match source.as_bytes().get(value_start).copied()? {
        b'{' => matching_delimiter(source, value_start, b'{', b'}')?.saturating_add(1),
        b'(' => matching_delimiter(source, value_start, b'(', b')')?.saturating_add(1),
        _ => entry_end,
    };
    Some((value_start, value_end))
}

fn insertion_after_call_open(source: &str, call_start: usize, call_end: usize) -> Option<usize> {
    source
        .get(call_start..call_end)?
        .find('(')
        .map(|offset| call_start + offset + 1)
}

fn origin_edit_for_call(
    source: &str,
    annotation_start: usize,
    call: &AnnotationCall,
    origin: CorePoint,
) -> Option<SourceEdit> {
    let origin_text = format_modelica_point(origin);
    if let Some(entry) = call
        .args
        .iter()
        .find(|entry| entry.name.as_deref() == Some("origin"))
    {
        let (start, end) = value_range_for_entry(source, annotation_start, entry)?;
        return Some(SourceEdit {
            start,
            end,
            expected_text: Some(source.get(start..end)?.to_owned()),
            replacement: origin_text,
        });
    }
    let call_start = annotation_start + call.source_range.start;
    let call_end = annotation_start + call.source_range.end;
    let insertion = insertion_after_call_open(source, call_start, call_end)?;
    Some(SourceEdit {
        start: insertion,
        end: insertion,
        expected_text: Some(String::new()),
        replacement: format!("origin={origin_text}, "),
    })
}

fn format_modelica_point(point: CorePoint) -> String {
    format!(
        "{{{}, {}}}",
        format_modelica_number(point.x),
        format_modelica_number(point.y)
    )
}

fn format_modelica_number(value: f32) -> String {
    let value = if value.abs() < 0.000_001 { 0.0 } else { value };
    let mut text = format!("{value:.6}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn icon_graphic_index(graphic_id: &str) -> Option<usize> {
    graphic_id
        .rsplit_once(":Icon.graphics:")
        .and_then(|(_, index)| index.parse().ok())
}

fn mask_nested_class_ranges(source: &str) -> String {
    let Ok(file) = parse(source, "<candidate>") else {
        return source.to_owned();
    };
    let Some(root) = file.classes.first() else {
        return source.to_owned();
    };
    let mut bytes = source.as_bytes().to_vec();
    for child in &root.children {
        let start = child.source_range.start.min(bytes.len());
        let end = child.source_range.end.min(bytes.len());
        for byte in &mut bytes[start..end] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| source.to_owned())
}

fn patch_icon_graphic_origin(
    source: &str,
    graphic_index: usize,
    origin: CorePoint,
    version: u64,
) -> Result<String, String> {
    let scan_source = mask_nested_class_ranges(source);
    let mut valid_graphics = 0;
    for (annotation_start, annotation) in annotation_calls(&scan_source) {
        let Some(icon) = nested_call(&annotation, "Icon") else {
            continue;
        };
        let Some(graphics) = icon.named("graphics").and_then(AnnotationValue::as_array) else {
            continue;
        };
        for entry in graphics {
            let Some(graphic) = entry.as_call().filter(|call| is_graphic_call(call)) else {
                continue;
            };
            if valid_graphics == graphic_index {
                let edit = origin_edit_for_call(source, annotation_start, graphic, origin)
                    .ok_or_else(|| "unable to locate graphic origin".to_owned())?;
                return apply_validated_source_edit(source, edit, version);
            }
            valid_graphics += 1;
        }
    }
    Err(format!(
        "graphic index {graphic_index} was not found in source"
    ))
}

#[cfg(test)]
fn patch_component_origin(
    source: &str,
    component_name: &str,
    origin: CorePoint,
    version: u64,
) -> Result<String, String> {
    let edit = component_origin_edit(source, component_name, origin)?;
    apply_validated_source_edit(source, edit, version)
}

fn component_origin_edit(
    source: &str,
    component_name: &str,
    origin: CorePoint,
) -> Result<SourceEdit, String> {
    let scan_source = mask_nested_class_ranges(source);
    for (annotation_start, annotation) in annotation_calls(&scan_source) {
        let statement_start = source[..annotation_start]
            .rfind(';')
            .map_or(0, |index| index + 1);
        let statement = &source[statement_start..annotation_start];
        let last_name = tokenize(statement)
            .into_iter()
            .filter(|token| matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword))
            .map(|token| token.text)
            .next_back();
        if last_name.as_deref() != Some(component_name) {
            continue;
        }
        let Some(placement) = nested_call(&annotation, "Placement") else {
            continue;
        };
        let Some(transformation) = nested_call(placement, "transformation") else {
            continue;
        };
        return origin_edit_for_call(source, annotation_start, transformation, origin)
            .ok_or_else(|| "unable to locate component origin".to_owned());
    }
    Err(format!(
        "component `{component_name}` placement was not found in source"
    ))
}

fn component_extent_edit(
    source: &str,
    component_name: &str,
    extent: modelica_core::scene::Extent,
) -> Result<SourceEdit, String> {
    let scan_source = mask_nested_class_ranges(source);
    for (annotation_start, annotation) in annotation_calls(&scan_source) {
        let statement_start = source[..annotation_start]
            .rfind(';')
            .map_or(0, |index| index + 1);
        let statement = &source[statement_start..annotation_start];
        let last_name = tokenize(statement)
            .into_iter()
            .filter(|token| matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword))
            .map(|token| token.text)
            .next_back();
        if last_name.as_deref() != Some(component_name) {
            continue;
        }
        let Some(placement) = nested_call(&annotation, "Placement") else {
            continue;
        };
        let Some(transformation) = nested_call(placement, "transformation") else {
            continue;
        };
        return extent_edit_for_call(source, annotation_start, transformation, extent)
            .ok_or_else(|| "unable to locate component extent".to_owned());
    }
    Err(format!(
        "component `{component_name}` placement was not found in source"
    ))
}

fn extent_edit_for_call(
    source: &str,
    annotation_start: usize,
    call: &AnnotationCall,
    extent: modelica_core::scene::Extent,
) -> Option<SourceEdit> {
    let extent_text = format_modelica_extent(extent);
    if let Some(entry) = call
        .args
        .iter()
        .find(|entry| entry.name.as_deref() == Some("extent"))
    {
        let (start, end) = value_range_for_entry(source, annotation_start, entry)?;
        return Some(SourceEdit {
            start,
            end,
            expected_text: Some(source.get(start..end)?.to_owned()),
            replacement: extent_text,
        });
    }
    let call_start = annotation_start + call.source_range.start;
    let call_end = annotation_start + call.source_range.end;
    let insertion = insertion_after_call_open(source, call_start, call_end)?;
    Some(SourceEdit {
        start: insertion,
        end: insertion,
        expected_text: Some(String::new()),
        replacement: format!("extent={extent_text}, "),
    })
}

fn format_modelica_extent(extent: modelica_core::scene::Extent) -> String {
    format!(
        "{{{}, {}}}",
        format_modelica_point(extent.p1),
        format_modelica_point(extent.p2)
    )
}

fn default_component_extent() -> modelica_core::scene::Extent {
    modelica_core::scene::Extent {
        p1: CorePoint { x: -10.0, y: -10.0 },
        p2: CorePoint { x: 10.0, y: 10.0 },
    }
}

fn connector_lookup_name(path: &str) -> &str {
    path.split('[').next().unwrap_or(path)
}

fn connector_world_position(
    component: &CoreComponentInstance,
    extent: modelica_core::scene::Extent,
    connector_path: &str,
) -> Option<CorePoint> {
    let icon = component.resolved_icon.as_deref()?;
    let lookup_name = connector_lookup_name(connector_path);
    let graphic = icon.graphics.iter().find(|graphic| {
        graphic.owner.kind == GraphicOwnerKind::Connector
            && graphic
                .owner
                .instance_name
                .as_deref()
                .is_some_and(|name| name == connector_path || name == lookup_name)
    })?;
    let placement =
        diagram_placement_transform_for_extent(icon, component.origin, component.rotation, extent);
    Some(apply_transform_point(
        CorePoint { x: 0.0, y: 0.0 },
        compose_transform(placement, graphic.transform),
    ))
}

fn connection_endpoints_match(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
) -> bool {
    let Some(line) = connection.line.as_ref() else {
        return true;
    };
    let Some(rhs_index) = line.points.len().checked_sub(1) else {
        return true;
    };
    let endpoints = [
        (&connection.lhs, line.points[0]),
        (&connection.rhs, line.points[rhs_index]),
    ];
    endpoints.into_iter().all(|(connector, local_point)| {
        let Some(component) = scene
            .components
            .iter()
            .find(|component| component.name == connector.component_name)
        else {
            return true;
        };
        let Some(extent) = component.placement_extent else {
            return true;
        };
        let Some(connector_position) =
            connector_world_position(component, extent, &connector.connector_path)
        else {
            return true;
        };
        let line_position = line_local_to_world(line, local_point);
        distance_between(line_position, connector_position) <= 1.0e-4
    })
}

fn is_orthogonal_polyline(points: &[CorePoint]) -> bool {
    points.windows(2).all(|pair| {
        let [first, second] = pair else {
            return true;
        };
        (first.x - second.x).abs() <= ORTHOGONAL_EPSILON
            || (first.y - second.y).abs() <= ORTHOGONAL_EPSILON
    })
}

fn connection_points_match_invariants(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    expected_points: &[CorePoint],
    expected_count: usize,
) -> bool {
    let Some(line) = connection.line.as_ref() else {
        return false;
    };
    line.points == expected_points
        && line.points.len() == expected_count
        && is_orthogonal_polyline(&line.points)
        && connection_endpoints_match(scene, connection)
}

fn line_local_point_from_world(
    world: CorePoint,
    line_origin: CorePoint,
    line_rotation: f32,
) -> CorePoint {
    world_to_line_local(&temporary_line(line_origin, line_rotation), world)
}

fn temporary_line(origin: CorePoint, rotation: f32) -> LineGraphic {
    LineGraphic {
        origin,
        rotation,
        points: Vec::new(),
        color: [0, 0, 0],
        pattern: None,
        thickness: 0.25,
        arrow: Vec::new(),
        arrow_size: None,
        smooth: None,
    }
}

fn resized_connection_points(
    component: &CoreComponentInstance,
    before_extent: modelica_core::scene::Extent,
    after_extent: modelica_core::scene::Extent,
    snapshot: &ConnectionDragSnapshot,
) -> Vec<CorePoint> {
    let mut points = snapshot.original_line_points.clone();
    let endpoints = [
        (0usize, snapshot.lhs_connector_path.as_deref()),
        (
            points.len().saturating_sub(1),
            snapshot.rhs_connector_path.as_deref(),
        ),
    ];
    for (index, connector_path) in endpoints {
        let Some(connector_path) = connector_path else {
            continue;
        };
        let Some(before_world) = connector_world_position(component, before_extent, connector_path)
        else {
            continue;
        };
        let Some(after_world) = connector_world_position(component, after_extent, connector_path)
        else {
            continue;
        };
        if index >= points.len() {
            continue;
        }
        let before_local = line_local_point_from_world(
            before_world,
            snapshot.original_line_origin,
            snapshot.original_line_rotation,
        );
        let after_local = line_local_point_from_world(
            after_world,
            snapshot.original_line_origin,
            snapshot.original_line_rotation,
        );
        let delta = CorePoint {
            x: after_local.x - before_local.x,
            y: after_local.y - before_local.y,
        };
        let original_endpoint = snapshot.original_line_points[index];
        points[index] = after_local;
        if index == 0 && points.len() >= 2 {
            preserve_orthogonal_neighbor(
                &mut points[1],
                original_endpoint,
                snapshot.original_line_points[1],
                delta,
            );
        } else if index + 1 == points.len() && points.len() >= 2 {
            preserve_orthogonal_neighbor(
                &mut points[index - 1],
                original_endpoint,
                snapshot.original_line_points[index - 1],
                delta,
            );
        }
    }
    points
}

fn connection_drag_snapshots(
    scene: &CoreDiagramScene,
    component_name: &str,
) -> Vec<ConnectionDragSnapshot> {
    scene
        .connections
        .iter()
        .filter_map(|connection| {
            let endpoint = match (
                connection.lhs.component_name == component_name,
                connection.rhs.component_name == component_name,
            ) {
                (true, true) => ConnectionEndpoint::Both,
                (true, false) => ConnectionEndpoint::Lhs,
                (false, true) => ConnectionEndpoint::Rhs,
                (false, false) => return None,
            };
            let line = connection.line.as_ref()?;
            Some(ConnectionDragSnapshot {
                connection_id: connection.id.clone(),
                connection_key: connection.key.clone(),
                endpoint,
                original_line_points: line.points.clone(),
                original_line_origin: line.origin,
                original_line_rotation: line.rotation,
                lhs_connector_path: (connection.lhs.component_name == component_name)
                    .then(|| connection.lhs.connector_path.clone()),
                rhs_connector_path: (connection.rhs.component_name == component_name)
                    .then(|| connection.rhs.connector_path.clone()),
            })
        })
        .collect()
}

fn translated_connection_points(
    original_points: &[CorePoint],
    endpoint: ConnectionEndpoint,
    line_origin: CorePoint,
    line_rotation: f32,
    delta: CorePoint,
) -> Vec<CorePoint> {
    let local_delta = world_delta_to_line_local(line_origin, line_rotation, delta);
    let mut points = original_points.to_vec();
    match endpoint {
        ConnectionEndpoint::Lhs => {
            if points.len() >= 2 {
                let original_endpoint = points[0];
                let original_neighbor = points[1];
                points[0].x += local_delta.x;
                points[0].y += local_delta.y;
                preserve_orthogonal_neighbor(
                    &mut points[1],
                    original_endpoint,
                    original_neighbor,
                    local_delta,
                );
            } else if let Some(point) = points.first_mut() {
                point.x += local_delta.x;
                point.y += local_delta.y;
            }
        }
        ConnectionEndpoint::Rhs => {
            if points.len() >= 2 {
                let endpoint_index = points.len() - 1;
                let neighbor_index = points.len() - 2;
                let original_endpoint = points[endpoint_index];
                let original_neighbor = points[neighbor_index];
                points[endpoint_index].x += local_delta.x;
                points[endpoint_index].y += local_delta.y;
                preserve_orthogonal_neighbor(
                    &mut points[neighbor_index],
                    original_endpoint,
                    original_neighbor,
                    local_delta,
                );
            } else if let Some(point) = points.last_mut() {
                point.x += local_delta.x;
                point.y += local_delta.y;
            }
        }
        ConnectionEndpoint::Both => {
            for point in &mut points {
                point.x += local_delta.x;
                point.y += local_delta.y;
            }
        }
    }
    points
}

fn preserve_orthogonal_neighbor(
    neighbor: &mut CorePoint,
    original_endpoint: CorePoint,
    original_neighbor: CorePoint,
    delta: CorePoint,
) {
    if (original_endpoint.y - original_neighbor.y).abs() <= ORTHOGONAL_EPSILON {
        neighbor.y += delta.y;
    } else if (original_endpoint.x - original_neighbor.x).abs() <= ORTHOGONAL_EPSILON {
        neighbor.x += delta.x;
    }
}

fn translated_connection_segment(
    original_points: &[CorePoint],
    segment_index: usize,
    orientation: ConnectionSegmentOrientation,
    line_origin: CorePoint,
    line_rotation: f32,
    delta: CorePoint,
) -> Vec<CorePoint> {
    let mut points = original_points.to_vec();
    if segment_index == 0 || segment_index + 1 >= points.len().saturating_sub(1) {
        return points;
    }
    let Some((first, second)) = points
        .get_mut(segment_index..=segment_index.saturating_add(1))
        .and_then(|points| points.split_first_mut())
    else {
        return points;
    };
    let second = &mut second[0];
    let local_delta = world_delta_to_line_local(line_origin, line_rotation, delta);
    match orientation {
        ConnectionSegmentOrientation::Horizontal => {
            first.y += local_delta.y;
            second.y += local_delta.y;
        }
        ConnectionSegmentOrientation::Vertical => {
            first.x += local_delta.x;
            second.x += local_delta.x;
        }
    }
    points
}

fn world_delta_to_line_local(
    line_origin: CorePoint,
    line_rotation: f32,
    delta: CorePoint,
) -> CorePoint {
    let line = temporary_line(line_origin, line_rotation);
    let world_origin = line_local_to_world(&line, CorePoint { x: 0.0, y: 0.0 });
    let world_target = CorePoint {
        x: world_origin.x + delta.x,
        y: world_origin.y + delta.y,
    };
    let local_origin = world_to_line_local(&line, world_origin);
    let local_target = world_to_line_local(&line, world_target);
    CorePoint {
        x: local_target.x - local_origin.x,
        y: local_target.y - local_origin.y,
    }
}

#[cfg(test)]
fn normalize_orthogonal_points(points: &[CorePoint]) -> Vec<CorePoint> {
    let mut normalized = Vec::with_capacity(points.len());
    for point in points {
        if normalized
            .last()
            .is_some_and(|previous| distance_between(*previous, *point) <= ORTHOGONAL_EPSILON)
        {
            continue;
        }
        normalized.push(*point);
    }
    let mut index = 1;
    while index + 1 < normalized.len() {
        let previous = normalized[index - 1];
        let current = normalized[index];
        let next = normalized[index + 1];
        let collinear = (previous.y - current.y).abs() <= ORTHOGONAL_EPSILON
            && (current.y - next.y).abs() <= ORTHOGONAL_EPSILON
            || (previous.x - current.x).abs() <= ORTHOGONAL_EPSILON
                && (current.x - next.x).abs() <= ORTHOGONAL_EPSILON;
        if collinear {
            normalized.remove(index);
        } else {
            index += 1;
        }
    }
    normalized
}

fn format_modelica_points(points: &[CorePoint]) -> String {
    format!(
        "{{{}}}",
        points
            .iter()
            .map(|point| format_modelica_point(*point))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn connection_points_edit(
    source: &str,
    line_source_range: SourceRange,
    points: &[CorePoint],
) -> Result<SourceEdit, String> {
    let line_source = source
        .get(line_source_range.start..line_source_range.end)
        .ok_or_else(|| "connection Line source range is stale".to_owned())?;
    let line = parse_call(line_source)
        .map_err(|error| format!("connection Line annotation cannot be parsed: {error}"))?;
    let entry = line
        .args
        .iter()
        .find(|entry| entry.name.as_deref() == Some("points"))
        .ok_or_else(|| "connection Line has no points argument".to_owned())?;
    let (start, end) = value_range_for_entry(source, line_source_range.start, entry)
        .ok_or_else(|| "unable to locate connection points".to_owned())?;
    Ok(SourceEdit {
        start,
        end,
        expected_text: Some(
            source
                .get(start..end)
                .ok_or_else(|| "connection points range is invalid".to_owned())?
                .to_owned(),
        ),
        replacement: format_modelica_points(points),
    })
}

fn connection_points_edit_for_key(
    source: &str,
    scene: &CoreDiagramScene,
    key: &ConnectionKey,
    points: &[CorePoint],
) -> Result<SourceEdit, String> {
    let connection = scene
        .connections
        .iter()
        .find(|connection| connection.key == *key)
        .ok_or_else(|| "connection identity is no longer present".to_owned())?;
    let line_source_range = connection
        .line_source_range
        .ok_or_else(|| "connection has no editable Line annotation".to_owned())?;
    connection_points_edit(source, line_source_range, points)
}

fn apply_validated_source_edit(
    source: &str,
    edit: SourceEdit,
    version: u64,
) -> Result<String, String> {
    apply_validated_source_edits(source, vec![edit], version)
}

fn apply_validated_source_edits(
    source: &str,
    edits: Vec<SourceEdit>,
    version: u64,
) -> Result<String, String> {
    let transaction = SourceTransaction {
        edits,
        source_version: Some(version),
    };
    let candidate = apply_source_transaction(source, &transaction, Some(version))
        .map_err(|error| error.to_string())?;
    parse(&candidate, "<candidate>")
        .map_err(|error| format!("candidate source does not parse: {error}"))?;
    Ok(candidate)
}

fn wants_pan(button: MouseButton, control_pressed: bool) -> bool {
    button == MouseButton::Middle || (button == MouseButton::Left && control_pressed)
}

#[allow(deprecated)]
fn main() {
    let input = env::args_os().nth(1).map(PathBuf::from);
    let document = match input.as_deref() {
        Some(path) => match LoadedDocument::load(path) {
            Ok(document) => {
                eprintln!(
                    "modelica-wgpu document: package={}, classes={}, diagnostics={}, path={}",
                    document.package_name,
                    document.class_names.len(),
                    document.diagnostics,
                    document.path.display()
                );
                Some(document)
            }
            Err(error) => {
                eprintln!("modelica-wgpu document load failed: {error}");
                None
            }
        },
        None => {
            eprintln!("modelica-wgpu: no Modelica path supplied; showing prototype scene");
            None
        }
    };
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("modelica-wgpu UI preview")
            .with_inner_size(PhysicalSize::new(1360, 860))
            .build(&event_loop)
            .expect("failed to create window"),
    );
    let mut app = pollster::block_on(App::new(window.clone(), document));
    app.update_title(None);

    event_loop
        .run(move |event, event_loop| {
            event_loop.set_control_flow(ControlFlow::Poll);
            match event {
                Event::WindowEvent { window_id, event } if window_id == app.window.id() => {
                    let egui_response = app.egui_state.on_window_event(&app.window, &event);
                    let egui_consumed = egui_response.consumed;
                    match event {
                        WindowEvent::CloseRequested => event_loop.exit(),
                        WindowEvent::Resized(size) => app.resize(size),
                        WindowEvent::RedrawRequested => {
                            match app.render() {
                                Ok(()) => {}
                                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                                    app.resize(app.window.inner_size())
                                }
                                Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                                Err(wgpu::SurfaceError::Timeout) => {}
                            }
                            app.window.request_redraw();
                        }
                        WindowEvent::ModifiersChanged(modifiers) => {
                            app.modifiers = modifiers.state();
                        }
                        WindowEvent::KeyboardInput { event, .. }
                            if event.state == ElementState::Pressed && !event.repeat =>
                        {
                            if !egui_consumed {
                                match event.physical_key {
                                    PhysicalKey::Code(KeyCode::KeyR) => {
                                        app.fit_scene();
                                    }
                                    PhysicalKey::Code(KeyCode::KeyZ)
                                        if app.modifiers.control_key() =>
                                    {
                                        if app.modifiers.shift_key() {
                                            app.redo();
                                        } else {
                                            app.undo();
                                        }
                                    }
                                    PhysicalKey::Code(KeyCode::KeyY)
                                        if app.modifiers.control_key() =>
                                    {
                                        app.redo();
                                    }
                                    _ => {}
                                }
                            }
                            app.window.request_redraw();
                        }
                        WindowEvent::CursorMoved { position, .. } => {
                            app.cursor = position;
                            app.update_model_drag_preview(position);
                        }
                        WindowEvent::MouseInput { state, button, .. } => {
                            if state == ElementState::Released {
                                app.finish_model_drag(button);
                            } else if app.canvas_event_allowed() {
                                if wants_pan(button, app.modifiers.control_key()) {
                                    app.pointer_interaction = PointerInteraction::Pan {
                                        button,
                                        start_pointer: app.cursor,
                                        start_pan: app.pan,
                                    };
                                } else if button == MouseButton::Left
                                    && !app.modifiers.control_key()
                                {
                                    app.begin_model_drag();
                                }
                            }
                        }
                        WindowEvent::MouseWheel { delta, .. } => {
                            if should_zoom_canvas(
                                app.main_view,
                                app.pointer_over_canvas(),
                                app.modifiers.control_key(),
                            ) {
                                let amount = match delta {
                                    MouseScrollDelta::LineDelta(_, y) => y,
                                    MouseScrollDelta::PixelDelta(position) => {
                                        position.y as f32 / 80.0
                                    }
                                };
                                app.zoom_at_cursor(amount);
                                app.window.request_redraw();
                            }
                        }
                        _ => {}
                    }
                }
                Event::AboutToWait => app.window.request_redraw(),
                _ => {}
            }
        })
        .expect("event loop failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_navigation_is_limited_to_icon_and_diagram_canvas_events() {
        assert!(!canvas_navigation_enabled_for(MainView::Source));
        assert!(canvas_navigation_enabled_for(MainView::Icon));
        assert!(canvas_navigation_enabled_for(MainView::Diagram));

        assert!(!canvas_event_allowed_for(MainView::Source, true));
        assert!(!canvas_event_allowed_for(MainView::Icon, false));
        assert!(canvas_event_allowed_for(MainView::Icon, true));
        assert!(canvas_event_allowed_for(MainView::Diagram, true));
    }

    #[test]
    fn canvas_zoom_requires_ctrl_and_never_uses_plain_wheel() {
        assert!(!should_zoom_canvas(MainView::Icon, true, false));
        assert!(should_zoom_canvas(MainView::Icon, true, true));
        assert!(!should_zoom_canvas(MainView::Source, true, true));
    }

    #[test]
    fn pan_buttons_match_canvas_navigation_contract() {
        assert!(wants_pan(MouseButton::Middle, false));
        assert!(wants_pan(MouseButton::Middle, true));
        assert!(wants_pan(MouseButton::Left, true));
        assert!(!wants_pan(MouseButton::Left, false));
        assert!(!wants_pan(MouseButton::Right, true));
    }

    #[test]
    fn source_wheel_and_drag_keep_view_state_unchanged() {
        let initial_zoom = INITIAL_ZOOM;
        let initial_pan = [37.0_f32, -19.0_f32];
        let mut zoom = initial_zoom;
        let mut pan = initial_pan;

        for _ in 0..100 {
            if should_zoom_canvas(MainView::Source, true, true) {
                zoom *= 1.1;
            }
            if canvas_event_allowed_for(MainView::Source, true)
                && wants_pan(MouseButton::Middle, false)
            {
                pan[0] += 4.0;
                pan[1] += 2.0;
            }
        }

        assert_eq!(zoom, initial_zoom);
        assert_eq!(pan, initial_pan);
    }

    #[test]
    fn icon_edit_patches_only_graphic_origin() {
        let source = "model Demo annotation(Icon(graphics={Rectangle(origin={1, 2}, extent={{-10, -20}, {10, 20}}, rotation=15)})); end Demo;";
        let candidate = patch_icon_graphic_origin(source, 0, CorePoint { x: 11.0, y: 7.0 }, 0)
            .expect("icon source patch");
        assert!(candidate.contains("origin={11, 7}"));
        assert!(candidate.contains("extent={{-10, -20}, {10, 20}}"));
        assert!(candidate.contains("rotation=15"));
    }

    #[test]
    fn component_edit_patches_only_placement_origin() {
        let source = "model Parent\n  Child p annotation(Placement(transformation(origin={20, 30}, extent={{-5, -6}, {5, 6}}, rotation=12)));\nend Parent;";
        let candidate = patch_component_origin(source, "p", CorePoint { x: 40.0, y: 50.0 }, 0)
            .expect("component source patch");
        assert!(candidate.contains("origin={40, 50}"));
        assert!(candidate.contains("extent={{-5, -6}, {5, 6}}"));
        assert!(candidate.contains("rotation=12"));
    }

    #[test]
    fn component_resize_patches_only_placement_extent_and_preserves_mirror_order() {
        let source = "model Parent\n  Child p annotation(Placement(transformation(origin={20, 30}, extent={{10, 10}, {-10, -10}}, rotation=12)));\nend Parent;";
        let extent = modelica_core::scene::Extent {
            p1: CorePoint { x: 20.0, y: 30.0 },
            p2: CorePoint { x: -10.0, y: -10.0 },
        };
        let edit = component_extent_edit(source, "p", extent).expect("extent edit");
        let candidate =
            apply_validated_source_edits(source, vec![edit], 0).expect("candidate source");
        assert!(candidate.contains("origin={20, 30}"));
        assert!(candidate.contains("extent={{20, 30}, {-10, -10}}"));
        assert!(candidate.contains("rotation=12"));
    }

    #[test]
    fn resize_corner_keeps_extent_orientation_without_sorting_mirrored_values() {
        let original = modelica_core::scene::Extent {
            p1: CorePoint { x: 10.0, y: 10.0 },
            p2: CorePoint { x: -10.0, y: -10.0 },
        };
        let resized = resized_extent_from_pointer(
            original,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            ResizeHandle::Corner(0),
            CorePoint { x: 20.0, y: 30.0 },
        );
        assert_eq!(resized.p1, CorePoint { x: 20.0, y: 30.0 });
        assert_eq!(resized.p2, original.p2);
        assert!(resized.p2.x < resized.p1.x);
        assert!(resized.p2.y < resized.p1.y);
    }

    #[test]
    fn connected_endpoint_translation_keeps_other_points_and_origin() {
        let points = vec![
            CorePoint { x: -40.0, y: 0.0 },
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 40.0, y: 20.0 },
        ];
        let moved = translated_connection_points(
            &points,
            ConnectionEndpoint::Lhs,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 10.0, y: 5.0 },
        );
        assert_eq!(moved[0], CorePoint { x: -30.0, y: 5.0 });
        assert_eq!(moved[1], CorePoint { x: 0.0, y: 5.0 });
        assert_eq!(moved[2], points[2]);

        let vertical = vec![
            CorePoint { x: 0.0, y: -40.0 },
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 40.0, y: 0.0 },
        ];
        let moved_vertical = translated_connection_points(
            &vertical,
            ConnectionEndpoint::Lhs,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 10.0, y: 5.0 },
        );
        assert_eq!(moved_vertical[0], CorePoint { x: 10.0, y: -35.0 });
        assert_eq!(moved_vertical[1], CorePoint { x: 10.0, y: 0.0 });
        assert_eq!(moved_vertical[2], vertical[2]);

        let rhs_horizontal = vec![
            CorePoint { x: -40.0, y: 0.0 },
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 40.0, y: 0.0 },
        ];
        let moved_rhs = translated_connection_points(
            &rhs_horizontal,
            ConnectionEndpoint::Rhs,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 10.0, y: 5.0 },
        );
        assert_eq!(moved_rhs[0], rhs_horizontal[0]);
        assert_eq!(moved_rhs[1], CorePoint { x: 0.0, y: 5.0 });
        assert_eq!(moved_rhs[2], CorePoint { x: 50.0, y: 5.0 });

        let moved_both = translated_connection_points(
            &points,
            ConnectionEndpoint::Both,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 10.0, y: 5.0 },
        );
        assert_eq!(
            moved_both,
            vec![
                CorePoint { x: -30.0, y: 5.0 },
                CorePoint { x: 10.0, y: 5.0 },
                CorePoint { x: 50.0, y: 25.0 },
            ]
        );
    }

    #[test]
    fn connection_points_edit_changes_only_line_points() {
        let source = "model Top\n equation\n  connect(a.port, b.port) annotation(Line(origin={5, 6}, points={{-40, 0}, {0, 0}, {40, 20}}, color={10, 20, 30}, thickness=1.5, pattern=LinePattern.Dash, smooth=Smooth.Bezier, arrow={Arrow.Start}, arrowSize=4));\nend Top;";
        let line_start = source.find("Line(").expect("Line annotation");
        let line_end =
            matching_delimiter(source, line_start + 4, b'(', b')').expect("Line close") + 1;
        let edit = connection_points_edit(
            source,
            SourceRange::new(line_start, line_end),
            &[
                CorePoint { x: -30.0, y: 5.0 },
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 40.0, y: 20.0 },
            ],
        )
        .expect("connection points edit");
        let candidate =
            apply_validated_source_edits(source, vec![edit], 0).expect("candidate source");
        assert!(candidate.contains("origin={5, 6}"));
        assert!(candidate.contains("points={{-30, 5}, {0, 0}, {40, 20}}"));
        assert!(candidate.contains("color={10, 20, 30}"));
        assert!(candidate.contains("thickness=1.5"));
        assert!(candidate.contains("pattern=LinePattern.Dash"));
        assert!(candidate.contains("smooth=Smooth.Bezier"));
        assert!(candidate.contains("arrow={Arrow.Start}"));
        assert!(candidate.contains("arrowSize=4"));
        assert!(candidate.contains("connect(a.port, b.port)"));
    }

    #[test]
    fn connection_points_edit_resolves_the_current_line_range_by_key() {
        let source = "model Top\n equation\n  connect(a, b) annotation(Line(points={{0, 0}, {10, 0}}));\nend Top;";
        let shifted = source.replace("connect(a, b)", "\n\n  connect(a, b)");
        let original_file = parse(source, "Stable.mo").expect("parse original");
        let shifted_file = parse(&shifted, "Stable.mo").expect("parse shifted");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("Stable.mo", &shifted)
            .expect("index shifted");
        let scene = resolve_diagram(&shifted_file.classes[0], &shifted, &mut registry);
        let key = scene.connections[0].key.clone();
        let edit = connection_points_edit_for_key(
            &shifted,
            &scene,
            &key,
            &[CorePoint { x: 2.0, y: 3.0 }, CorePoint { x: 12.0, y: 3.0 }],
        )
        .expect("keyed line edit");
        let candidate =
            apply_validated_source_edits(&shifted, vec![edit], 0).expect("candidate source");
        assert_eq!(original_file.classes[0].name, shifted_file.classes[0].name);
        assert!(candidate.contains("points={{2, 3}, {12, 3}}"));
        assert_eq!(candidate.matches("connect(a, b)").count(), 1);
    }

    #[test]
    fn repeated_connection_edits_never_reuse_a_stale_source_range() {
        let mut source = "model Top\n equation\n  connect(a, b) annotation(Line(points={{0, 0}, {10, 0}}));\nend Top;".to_owned();
        let file = parse(&source, "Repeated.mo").expect("parse");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("Repeated.mo", &source)
            .expect("index source");
        let initial_scene = resolve_diagram(&file.classes[0], &source, &mut registry);
        let key = initial_scene.connections[0].key.clone();

        for index in 0..100 {
            let file = parse(&source, "Repeated.mo").expect("parse iteration");
            registry
                .register_source("Repeated.mo", &source)
                .expect("reindex iteration");
            let scene = resolve_diagram(&file.classes[0], &source, &mut registry);
            let edit = connection_points_edit_for_key(
                &source,
                &scene,
                &key,
                &[
                    CorePoint {
                        x: index as f32,
                        y: 0.0,
                    },
                    CorePoint {
                        x: 10.0 + index as f32,
                        y: 0.0,
                    },
                ],
            )
            .expect("current Line.points range");
            source =
                apply_validated_source_edits(&source, vec![edit], 0).expect("apply current edit");
        }
        assert!(source.contains("points={{99, 0}, {109, 0}}"));
    }

    #[test]
    fn rotated_connection_segment_translation_stays_in_line_local_coordinates() {
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 40.0, y: 0.0 },
            CorePoint { x: 40.0, y: 30.0 },
            CorePoint { x: 80.0, y: 30.0 },
            CorePoint { x: 100.0, y: 30.0 },
        ];
        let moved = translated_connection_segment(
            &points,
            1,
            ConnectionSegmentOrientation::Vertical,
            CorePoint { x: 20.0, y: 30.0 },
            90.0,
            CorePoint { x: 10.0, y: 5.0 },
        );
        assert_eq!(moved[0], points[0]);
        assert_eq!(moved[1], CorePoint { x: 45.0, y: 0.0 });
        assert_eq!(moved[2], CorePoint { x: 45.0, y: 30.0 });
        assert_eq!(moved[3], points[3]);
        assert_eq!(moved[4], points[4]);
    }

    #[test]
    fn connection_segment_translation_moves_only_its_orientation_axis() {
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 40.0, y: 0.0 },
            CorePoint { x: 40.0, y: 30.0 },
            CorePoint { x: 80.0, y: 30.0 },
            CorePoint { x: 100.0, y: 30.0 },
        ];
        assert_eq!(
            translated_connection_segment(
                &points,
                1,
                ConnectionSegmentOrientation::Vertical,
                CorePoint { x: 0.0, y: 0.0 },
                0.0,
                CorePoint { x: 20.0, y: 100.0 },
            ),
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 60.0, y: 0.0 },
                CorePoint { x: 60.0, y: 30.0 },
                CorePoint { x: 80.0, y: 30.0 },
                CorePoint { x: 100.0, y: 30.0 },
            ]
        );
        assert_eq!(
            translated_connection_segment(
                &points,
                2,
                ConnectionSegmentOrientation::Horizontal,
                CorePoint { x: 0.0, y: 0.0 },
                0.0,
                CorePoint { x: 100.0, y: 12.0 },
            ),
            vec![
                points[0],
                points[1],
                CorePoint { x: 40.0, y: 42.0 },
                CorePoint { x: 80.0, y: 42.0 },
                points[4],
            ]
        );
    }

    #[test]
    fn diagonal_connection_points_are_rejected_by_routing_policy() {
        assert!(!is_orthogonal_polyline(&[
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 20.0, y: 13.0 },
        ]));
        assert!(is_orthogonal_polyline(&[
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 20.0, y: 0.0 },
            CorePoint { x: 20.0, y: 13.0 },
        ]));
    }

    #[test]
    fn normalize_orthogonal_points_removes_duplicates_and_collinear_points() {
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 10.0, y: 0.0 },
            CorePoint { x: 20.0, y: 0.0 },
            CorePoint { x: 20.0, y: 0.0 },
            CorePoint { x: 20.0, y: 20.0 },
        ];
        assert_eq!(
            normalize_orthogonal_points(&points),
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 20.0, y: 0.0 },
                CorePoint { x: 20.0, y: 20.0 },
            ]
        );
    }

    #[test]
    fn connection_hit_test_uses_screen_scaled_tolerance_and_finds_middle_segment() {
        let connection = modelica_core::scene::DiagramConnection {
            key: ConnectionKey::new(
                "Top",
                modelica_core::scene::ConnectorRef {
                    component_name: "a".to_owned(),
                    connector_path: "port".to_owned(),
                },
                modelica_core::scene::ConnectorRef {
                    component_name: "b".to_owned(),
                    connector_path: "port".to_owned(),
                },
                0,
            ),
            id: "connection:test".to_owned(),
            lhs: modelica_core::scene::ConnectorRef {
                component_name: "a".to_owned(),
                connector_path: "port".to_owned(),
            },
            rhs: modelica_core::scene::ConnectorRef {
                component_name: "b".to_owned(),
                connector_path: "port".to_owned(),
            },
            from: "a.port".to_owned(),
            to: "b.port".to_owned(),
            line: Some(LineGraphic {
                origin: CorePoint { x: 0.0, y: 0.0 },
                rotation: 0.0,
                points: vec![
                    CorePoint { x: 0.0, y: 0.0 },
                    CorePoint { x: 40.0, y: 0.0 },
                    CorePoint { x: 40.0, y: 30.0 },
                    CorePoint { x: 100.0, y: 30.0 },
                ],
                color: [0, 0, 0],
                pattern: Some("LinePattern.Solid".to_owned()),
                thickness: 1.0,
                arrow: Vec::new(),
                arrow_size: None,
                smooth: None,
            }),
            source_range: None,
            line_source_range: None,
        };
        let hit = hit_test_connection(&[connection], CorePoint { x: 40.0, y: 12.0 }, 1.0)
            .expect("middle segment hit");
        assert_eq!(hit.connection_id, "connection:test");
        assert!(matches!(
            hit.target,
            ConnectionHitTarget::Segment {
                index: 1,
                orientation: ConnectionSegmentOrientation::Vertical,
            }
        ));
    }

    #[test]
    fn zoom_limits_are_safe_for_repeated_wheel_input() {
        let mut zoom = INITIAL_ZOOM;
        for _ in 0..1000 {
            zoom = (zoom * 1.1).clamp(MIN_ZOOM, MAX_ZOOM);
        }
        assert_eq!(zoom, MAX_ZOOM);
        for _ in 0..1000 {
            zoom = (zoom * 0.9).clamp(MIN_ZOOM, MAX_ZOOM);
        }
        assert_eq!(zoom, MIN_ZOOM);
    }

    #[test]
    fn ellipse_without_line_pattern_keeps_modelica_default_outline() {
        let ellipse = EllipseGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: -40.0, y: -25.0 },
                p2: CorePoint { x: 40.0, y: 25.0 },
            },
            line_color: [0, 0, 0],
            fill_color: [240, 249, 255],
            line_pattern: None,
            line_thickness: None,
            fill_pattern: Some("FillPattern.VerticalCylinder".to_owned()),
            start_angle: None,
            end_angle: None,
        };
        let geometry = ellipse_geometry(&ellipse, Transform2D::identity());
        assert_eq!(geometry.len(), 2);
        assert!(geometry[1].indices.len() >= 3);
    }

    #[test]
    fn cylinder_fill_keeps_fill_and_annotation_edge_colors() {
        let style = fill_style(
            [225, 249, 255],
            [0, 0, 0],
            Some("FillPattern.HorizontalCylinder"),
        );
        assert_eq!(style.mode, FillMode::HorizontalCylinder as u32);
        assert_eq!(style.color, color_rgba([225, 249, 255]));
        assert_eq!(style.edge_color, color_rgba([0, 0, 0]));

        let solid = fill_style([255, 0, 0], [0, 0, 0], Some("FillPattern.Solid"));
        assert_eq!(solid.mode, FillMode::Solid as u32);
        assert_eq!(solid.color, color_rgba([255, 0, 0]));
    }

    #[test]
    fn annotation_colors_are_converted_from_srgb_for_srgb_surface() {
        let color = color_rgba([128, 64, 255]);
        assert!((color[0] - 0.21586).abs() < 0.001);
        assert!((color[1] - 0.05127).abs() < 0.001);
        assert_eq!(color[2], 1.0);
    }
}
