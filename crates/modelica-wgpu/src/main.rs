use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    path::{Path as FsPath, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc,
    },
    thread::JoinHandle,
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
    ComponentInstance as CoreComponentInstance, ConnectionKey, ConnectorRef,
    DiagramScene as CoreDiagramScene, EllipseGraphic, Graphic as CoreGraphic,
    IconScene as CoreIconScene, LineGraphic, Point as CorePoint, PolygonGraphic, RectangleGraphic,
    ResolvedGraphic, Transform2D,
};
use modelica_core::{
    apply_source_transaction,
    lexer::{tokenize, Token, TokenKind},
    parse, resolve_diagram, Class, ClassKind, IconResolver, Library, LibraryKind, LibraryRegistry,
    PackageLoader, PackageNode, SourceEdit, SourceRange, SourceTransaction,
};
use modelica_render::{
    canonicalize_orthogonal_points, connector_anchor_hit_distance, connector_anchors,
    line_local_to_world, reanchor_connection_points, resolve_connection_endpoints,
    resolved_graphic_contains_point, resolved_graphic_contains_point_with_transform,
    strict_connection_points, world_to_line_local, ConnectorAnchor, PortKey, ORTHOGONAL_EPSILON,
};
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
const CONNECTION_SNAP_ENTER_PIXELS: f32 = 8.0;
const CONNECTION_SNAP_EXIT_PIXELS: f32 = 12.0;
const CONNECTION_HIT_DISTANCE_TIE_EPSILON: f32 = 1.0e-4;
const DIAGRAM_HIT_GRID_CELL_SIZE: f32 = 64.0;
const UI_FONT_MEDIUM: &str = "modelica-ui-medium";
const UI_FONT_SEMIBOLD: &str = "modelica-ui-semibold";
const UI_FONT_MONO: &str = "modelica-ui-mono";
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

    fn key(self) -> &'static str {
        match self {
            Self::Violet => "violet",
            Self::Blue => "blue",
            Self::Cyan => "cyan",
            Self::Orange => "orange",
        }
    }

    fn from_key(value: &str) -> Option<Self> {
        match value {
            "violet" => Some(Self::Violet),
            "blue" => Some(Self::Blue),
            "cyan" => Some(Self::Cyan),
            "orange" => Some(Self::Orange),
            _ => None,
        }
    }
}

// Appearance persistence. Electron keeps the same keys in renderer
// localStorage; WGPU writes an equivalent JSON sidecar so both clients can
// share one look when they run on the same machine.
fn appearance_settings_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let base = if cfg!(target_os = "windows") {
        let roaming = std::env::var_os("APPDATA");
        std::path::PathBuf::from(
            roaming.unwrap_or_else(|| std::path::Path::new(&home).join("AppData/Roaming").into()),
        )
    } else if cfg!(target_os = "macos") {
        std::path::Path::new(&home).join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(&home).join(".config"))
    };
    Some(base.join("modelica-viewer").join("settings.json"))
}

fn parse_appearance_json(content: &str) -> (ThemeMode, AccentTheme) {
    let json_value = |key: &str| -> Option<String> {
        let needle = format!("\"{key}\":");
        let start = content.find(&needle)? + needle.len();
        let tail = content.get(start..)?;
        let tail = tail.trim_start().strip_prefix('"')?;
        let end = tail.find('"')?;
        Some(tail.get(..end)?.to_owned())
    };
    let theme = match json_value("theme").as_deref() {
        Some("light") => ThemeMode::Light,
        Some("dark") => ThemeMode::Dark,
        _ => ThemeMode::System,
    };
    let accent = json_value("accent")
        .as_deref()
        .and_then(AccentTheme::from_key)
        .unwrap_or(AccentTheme::Violet);
    (theme, accent)
}

fn load_appearance() -> (ThemeMode, AccentTheme) {
    let Some(path) = appearance_settings_path() else {
        return (ThemeMode::System, AccentTheme::Violet);
    };
    match fs::read_to_string(&path) {
        Ok(content) => parse_appearance_json(&content),
        Err(_) => (ThemeMode::System, AccentTheme::Violet),
    }
}

fn save_appearance(theme: ThemeMode, accent: AccentTheme) {
    let Some(path) = appearance_settings_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let content = format!(
        "{{\n  \"theme\": \"{}\",\n  \"accent\": \"{}\"\n}}\n",
        theme.label().to_ascii_lowercase(),
        accent.key(),
    );
    if fs::write(&path, content).is_ok() {
        eprintln!("modelica-wgpu: saved appearance to {}", path.display());
    }
}

fn write_file_atomic(path: &std::path::Path, contents: &str) -> Result<(), String> {
    let temp_path = std::path::PathBuf::from(format!(
        "{}.tmp-{}-{}",
        path.display(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    fs::write(&temp_path, contents).map_err(|error| format!("{}: {error}", temp_path.display()))?;
    if cfg!(target_os = "windows") {
        // Windows cannot rename over an existing file; move the original
        // aside first and restore it if the swap fails.
        let backup_path = std::path::PathBuf::from(format!(
            "{}.bak-{}-{}",
            path.display(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        fs::rename(path, &backup_path).map_err(|error| format!("{}: {error}", path.display()))?;
        if let Err(error) = fs::rename(&temp_path, path) {
            let _ = fs::rename(&backup_path, path);
            let _ = fs::remove_file(&temp_path);
            return Err(format!("{}: {error}", path.display()));
        }
        let _ = fs::remove_file(&backup_path);
        Ok(())
    } else {
        fs::rename(&temp_path, path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(())
    }
}

/// Persist every class edited in memory back to its own `.mo` file.
///
/// The save is transactional: each replacement is only applied when the
/// on-disk slice still equals the text the document last loaded or wrote.
/// An externally modified file therefore aborts the save instead of being
/// silently corrupted. Edits touching the same file are applied back-to-front
/// so earlier offsets stay valid.
fn save_edited_classes(document: &mut LoadedDocument) -> Result<usize, String> {
    if document.source_overrides.is_empty() {
        return Ok(0);
    }
    struct PendingEdit {
        start: usize,
        end: usize,
        updated: String,
        qualified: String,
    }
    let mut by_file = std::collections::BTreeMap::<std::path::PathBuf, Vec<PendingEdit>>::new();
    for class in &document.class_sources {
        let Some(updated) = document.source_overrides.get(&class.qualified_name) else {
            continue;
        };
        let range = class.source_range;
        if range.end <= range.start {
            continue;
        }
        by_file
            .entry(class.source_file.clone())
            .or_default()
            .push(PendingEdit {
                start: range.start,
                end: range.end,
                updated: updated.clone(),
                qualified: class.qualified_name.clone(),
            });
    }
    let mut saved_files = 0;
    for (path, mut edits) in by_file {
        // Later ranges first: earlier replacement offsets remain valid because
        // the edits never overlap inside one class and each class range comes
        // from the same parsed document.
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
        let disk_original =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        for edit in &edits {
            if edit.end > disk_original.len() || disk_original.get(edit.start..edit.end).is_none() {
                return Err(format!(
                    "stale source range at byte {} in {}; reload the library first",
                    edit.start,
                    path.display()
                ));
            }
            let expected = document
                .saved_class_text
                .get(&edit.qualified)
                .ok_or_else(|| {
                    format!(
                        "no baseline snapshot for {} in {}; reload the library first",
                        edit.qualified,
                        path.display()
                    )
                })?;
            if &disk_original[edit.start..edit.end] != expected {
                return Err(format!(
                    "{} changed on disk since it was loaded; reload before saving",
                    path.display()
                ));
            }
        }
        let mut disk = disk_original;
        for edit in &edits {
            disk.replace_range(edit.start..edit.end, &edit.updated);
        }
        write_file_atomic(&path, &disk)?;
        for edit in &edits {
            document
                .saved_class_text
                .insert(edit.qualified.clone(), edit.updated.clone());
        }
        saved_files += 1;
    }
    Ok(saved_files)
}

#[cfg(test)]
mod save_tests {
    use super::*;

    fn temp_document(
        directory: &std::path::Path,
        file_name: &str,
        content: &str,
        ranges: &[(&str, &str, &str)],
    ) -> (LoadedDocument, std::path::PathBuf) {
        let path = directory.join(file_name);
        fs::write(&path, content).unwrap();
        let mut class_sources = Vec::new();
        let mut source_overrides = HashMap::new();
        let mut saved_class_text = HashMap::new();
        for (qualified_name, marker, updated) in ranges {
            let start = content.find(marker).unwrap();
            let end = start + marker.len();
            class_sources.push(ClassSource {
                qualified_name: (*qualified_name).to_owned(),
                source_file: path.clone(),
                source_range: SourceRange::new(start, end),
            });
            source_overrides.insert((*qualified_name).to_owned(), (*updated).to_owned());
            saved_class_text.insert((*qualified_name).to_owned(), (*marker).to_owned());
        }
        let document = LoadedDocument {
            path: path.clone(),
            package_name: "SaveTest".to_owned(),
            class_names: Vec::new(),
            diagnostics: 0,
            icons: Vec::new(),
            diagrams: Vec::new(),
            class_sources,
            source_overrides,
            saved_class_text,
            source_versions: HashMap::new(),
        };
        (document, path)
    }

    fn temp_directory(label: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "modelica-wgpu-save-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn save_replaces_only_the_edited_class_range() {
        let directory = temp_directory("single");
        let content = "within X; class A model M end A; class B model N end B;";
        let mut document = temp_document(
            &directory,
            "Single.mo",
            content,
            &[("X.A", "class A model M end A", "class A model M2 end A")],
        )
        .0;
        let saved = save_edited_classes(&mut document).expect("save succeeds");
        assert_eq!(saved, 1);
        let result = fs::read_to_string(&document.path).unwrap();
        assert!(result.contains("class A model M2 end A"));
        assert!(result.contains("class B model N end B"));
        assert!(!result.contains("class A model M end A"));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn save_applies_back_to_front_when_two_classes_share_a_file() {
        let directory = temp_directory("shared");
        let content = "class A model M end A; class B model N end B;";
        let (mut document, path) = temp_document(
            &directory,
            "Shared.mo",
            content,
            &[
                ("B", "class B model N end B", "class B model N2 end B"),
                ("A", "class A model M end A", "class A model M2 end A"),
            ],
        );
        let saved = save_edited_classes(&mut document).expect("save succeeds");
        assert_eq!(saved, 1);
        let result = fs::read_to_string(&path).unwrap();
        assert!(result.contains("class A model M2 end A"));
        assert!(result.contains("class B model N2 end B"));
        // No leftover temp files next to the source.
        let leftovers = fs::read_dir(&directory)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp-")
            })
            .count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn save_refuses_to_overwrite_a_file_changed_on_disk() {
        let directory = temp_directory("tamper");
        let content = "class A model M end A; class B model N end B;";
        let (mut document, path) = temp_document(
            &directory,
            "Tamper.mo",
            content,
            &[
                ("B", "class B model N end B", "class B model N2 end B"),
                ("A", "class A model M end A", "class A model M2 end A"),
            ],
        );
        // Simulate an external editor shifting every offset.
        fs::write(&path, format!("// externally edited\n{content}")).unwrap();
        let error = save_edited_classes(&mut document)
            .expect_err("save must refuse a file changed on disk");
        assert!(
            error.contains("changed on disk"),
            "unexpected error: {error}"
        );
        // The externally edited bytes must be preserved untouched.
        let disk = fs::read_to_string(&path).unwrap();
        assert!(disk.starts_with("// externally edited"));
        assert!(disk.contains("class A model M end A"));
        assert!(disk.contains("class B model N end B"));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn save_with_no_edits_is_a_noop() {
        let directory = temp_directory("empty");
        let content = "class A end A;";
        let path = directory.join("Noop.mo");
        fs::write(&path, content).unwrap();
        let mut document = LoadedDocument {
            path: path.clone(),
            package_name: "Noop".to_owned(),
            class_names: Vec::new(),
            diagnostics: 0,
            icons: Vec::new(),
            diagrams: Vec::new(),
            class_sources: Vec::new(),
            source_overrides: HashMap::new(),
            saved_class_text: HashMap::new(),
            source_versions: HashMap::new(),
        };
        assert_eq!(
            save_edited_classes(&mut document).expect("save succeeds"),
            0
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
        let _ = fs::remove_dir_all(&directory);
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;

    #[test]
    fn parse_defaults_for_empty_and_garbage_input() {
        assert_eq!(
            parse_appearance_json(""),
            (ThemeMode::System, AccentTheme::Violet)
        );
        assert_eq!(
            parse_appearance_json("not json at all"),
            (ThemeMode::System, AccentTheme::Violet)
        );
    }

    #[test]
    fn parse_restores_saved_theme_and_accent() {
        assert_eq!(
            parse_appearance_json(r#"{"theme": "dark", "accent": "cyan"}"#),
            (ThemeMode::Dark, AccentTheme::Cyan)
        );
        assert_eq!(
            parse_appearance_json(r#"{"theme":"light","accent":"orange"}"#),
            (ThemeMode::Light, AccentTheme::Orange)
        );
    }

    #[test]
    fn unknown_fields_fall_back_to_defaults() {
        assert_eq!(
            parse_appearance_json(r#"{"theme": "purple", "accent": "magenta"}"#),
            (ThemeMode::System, AccentTheme::Violet)
        );
    }

    #[test]
    fn accent_key_mapping_round_trips() {
        for accent in [
            AccentTheme::Violet,
            AccentTheme::Blue,
            AccentTheme::Cyan,
            AccentTheme::Orange,
        ] {
            assert_eq!(AccentTheme::from_key(accent.key()), Some(accent));
        }
        assert_eq!(AccentTheme::from_key("nope"), None);
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
    Port(PortKey),
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

#[derive(Clone, Copy, Debug)]
struct HitBounds {
    min: CorePoint,
    max: CorePoint,
}

impl HitBounds {
    fn from_points(points: impl IntoIterator<Item = CorePoint>) -> Option<Self> {
        let mut bounds = Self {
            min: CorePoint {
                x: f32::INFINITY,
                y: f32::INFINITY,
            },
            max: CorePoint {
                x: f32::NEG_INFINITY,
                y: f32::NEG_INFINITY,
            },
        };
        let mut any = false;
        for point in points {
            any = true;
            bounds.min.x = bounds.min.x.min(point.x);
            bounds.min.y = bounds.min.y.min(point.y);
            bounds.max.x = bounds.max.x.max(point.x);
            bounds.max.y = bounds.max.y.max(point.y);
        }
        any.then_some(bounds)
    }

    fn contains(self, point: CorePoint, tolerance: f32) -> bool {
        let tolerance = tolerance.max(0.0);
        point.x >= self.min.x - tolerance
            && point.x <= self.max.x + tolerance
            && point.y >= self.min.y - tolerance
            && point.y <= self.max.y + tolerance
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct GridCell {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionSegmentRef {
    connection_index: usize,
    segment_index: usize,
}

#[derive(Clone, Debug, Default)]
struct HitBucket {
    component_indices: Vec<usize>,
    port_indices: Vec<usize>,
    connection_segments: Vec<ConnectionSegmentRef>,
}

#[derive(Clone, Debug, Default)]
struct SpatialCandidates {
    component_indices: Vec<usize>,
    port_indices: Vec<usize>,
    connection_segments: Vec<ConnectionSegmentRef>,
}

#[derive(Clone, Debug, Default)]
struct DiagramSpatialIndex {
    cells: HashMap<GridCell, HitBucket>,
    connection_cells: HashMap<usize, Vec<GridCell>>,
    port_query_marks: Vec<u32>,
    port_query_generation: u32,
}

impl DiagramSpatialIndex {
    fn cell_for(point: CorePoint) -> GridCell {
        GridCell {
            x: (point.x / DIAGRAM_HIT_GRID_CELL_SIZE).floor() as i32,
            y: (point.y / DIAGRAM_HIT_GRID_CELL_SIZE).floor() as i32,
        }
    }

    fn cell_range(bounds: HitBounds) -> impl Iterator<Item = GridCell> {
        let min = Self::cell_for(bounds.min);
        let max = Self::cell_for(bounds.max);
        (min.x..=max.x).flat_map(move |x| (min.y..=max.y).map(move |y| GridCell { x, y }))
    }

    fn insert_component(&mut self, index: usize, bounds: HitBounds) {
        for cell in Self::cell_range(bounds) {
            self.cells
                .entry(cell)
                .or_default()
                .component_indices
                .push(index);
        }
    }

    fn insert_port(&mut self, index: usize, bounds: HitBounds) {
        if self.port_query_marks.len() <= index {
            self.port_query_marks.resize(index + 1, 0);
        }
        for cell in Self::cell_range(bounds) {
            self.cells.entry(cell).or_default().port_indices.push(index);
        }
    }

    fn insert_connection_segment(&mut self, segment: ConnectionSegmentRef, bounds: HitBounds) {
        for cell in Self::cell_range(bounds) {
            self.cells
                .entry(cell)
                .or_default()
                .connection_segments
                .push(segment);
            self.connection_cells
                .entry(segment.connection_index)
                .or_default()
                .push(cell);
        }
    }

    fn remove_connection(&mut self, connection_index: usize) {
        let Some(mut cells) = self.connection_cells.remove(&connection_index) else {
            return;
        };
        cells.sort_unstable();
        cells.dedup();
        for cell in cells {
            let mut remove_cell = false;
            if let Some(bucket) = self.cells.get_mut(&cell) {
                bucket
                    .connection_segments
                    .retain(|segment| segment.connection_index != connection_index);
                remove_cell = bucket.component_indices.is_empty()
                    && bucket.port_indices.is_empty()
                    && bucket.connection_segments.is_empty();
            }
            if remove_cell {
                self.cells.remove(&cell);
            }
        }
    }

    fn update_connection(
        &mut self,
        connection_index: usize,
        segments: impl IntoIterator<Item = (usize, HitBounds)>,
    ) {
        self.remove_connection(connection_index);
        for (segment_index, bounds) in segments {
            self.insert_connection_segment(
                ConnectionSegmentRef {
                    connection_index,
                    segment_index,
                },
                bounds,
            );
        }
    }

    fn query(&self, point: CorePoint, tolerance: f32) -> SpatialCandidates {
        let tolerance = tolerance.max(0.0);
        let bounds = HitBounds {
            min: CorePoint {
                x: point.x - tolerance,
                y: point.y - tolerance,
            },
            max: CorePoint {
                x: point.x + tolerance,
                y: point.y + tolerance,
            },
        };
        let mut candidates = SpatialCandidates::default();
        for cell in Self::cell_range(bounds) {
            let Some(bucket) = self.cells.get(&cell) else {
                continue;
            };
            candidates
                .component_indices
                .extend(bucket.component_indices.iter().copied());
            candidates
                .port_indices
                .extend(bucket.port_indices.iter().copied());
            candidates
                .connection_segments
                .extend(bucket.connection_segments.iter().copied());
        }
        candidates.component_indices.sort_unstable();
        candidates.component_indices.dedup();
        candidates.port_indices.sort_unstable();
        candidates.port_indices.dedup();
        candidates
            .connection_segments
            .sort_unstable_by(|left, right| {
                left.connection_index
                    .cmp(&right.connection_index)
                    .reverse()
                    .then(left.segment_index.cmp(&right.segment_index))
            });
        candidates.connection_segments.dedup();
        candidates
    }

    fn nearest_port(
        &mut self,
        point: CorePoint,
        tolerance: f32,
        exclude: Option<&PortKey>,
        anchors: &[ConnectorAnchor],
    ) -> Option<usize> {
        let tolerance = tolerance.max(0.0);
        self.port_query_generation = self.port_query_generation.wrapping_add(1);
        if self.port_query_generation == 0 {
            self.port_query_marks.fill(0);
            self.port_query_generation = 1;
        }
        let generation = self.port_query_generation;
        let bounds = HitBounds {
            min: CorePoint {
                x: point.x - tolerance,
                y: point.y - tolerance,
            },
            max: CorePoint {
                x: point.x + tolerance,
                y: point.y + tolerance,
            },
        };
        let mut nearest = None;
        for cell in Self::cell_range(bounds) {
            let Some(bucket) = self.cells.get(&cell) else {
                continue;
            };
            for &port_index in &bucket.port_indices {
                if self.port_query_marks.get(port_index).copied() == Some(generation) {
                    continue;
                }
                if let Some(mark) = self.port_query_marks.get_mut(port_index) {
                    *mark = generation;
                }
                let Some(anchor) = anchors.get(port_index) else {
                    continue;
                };
                if !anchor.editable || exclude.is_some_and(|key| anchor.key == *key) {
                    continue;
                }
                let Some(distance) = connector_anchor_hit_distance(anchor, point, tolerance) else {
                    continue;
                };
                if nearest.is_none_or(|(best_distance, _)| distance < best_distance) {
                    nearest = Some((distance, port_index));
                }
            }
        }
        nearest.map(|(_, index)| index)
    }
}

fn connection_creation_target(
    spatial_index: &mut DiagramSpatialIndex,
    anchors: &[ConnectorAnchor],
    zoom: f32,
    source_port: &PortKey,
    hovered_target: Option<usize>,
    pointer_model: CorePoint,
) -> Option<(usize, CorePoint)> {
    let enter_tolerance = CONNECTION_SNAP_ENTER_PIXELS / zoom.max(MIN_ZOOM);
    let exit_tolerance = CONNECTION_SNAP_EXIT_PIXELS / zoom.max(MIN_ZOOM);
    if let Some(index) = hovered_target {
        if let Some(anchor) = anchors.get(index) {
            if anchor.editable
                && anchor.key != *source_port
                && connector_anchor_hit_distance(anchor, pointer_model, exit_tolerance).is_some()
            {
                return Some((index, anchor.world_position));
            }
        }
    }
    let index =
        spatial_index.nearest_port(pointer_model, enter_tolerance, Some(source_port), anchors)?;
    let anchor = anchors.get(index)?;
    Some((index, anchor.world_position))
}

#[derive(Clone, Debug)]
struct ComponentHitItem {
    scene_index: usize,
    bounds: HitBounds,
}

#[derive(Clone, Debug, Default)]
struct DiagramHitCache {
    components: Vec<ComponentHitItem>,
    ports: Vec<ConnectorAnchor>,
    spatial_index: DiagramSpatialIndex,
}

impl DiagramHitCache {
    fn update_connection(&mut self, connection_index: usize, line: &LineGraphic) {
        let points = connection_world_points(line, &line.points);
        let segments = points
            .windows(2)
            .enumerate()
            .filter_map(|(segment_index, pair)| {
                let [start, end] = pair else {
                    return None;
                };
                HitBounds::from_points([*start, *end]).map(|bounds| (segment_index, bounds))
            });
        self.spatial_index
            .update_connection(connection_index, segments);
    }
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
        semantic_endpoints: (CorePoint, CorePoint),
        snap_axes: Vec<f32>,
        snapped_axis: Option<f32>,
        source_before: String,
    },
    MoveDiagramConnectionCorner {
        button: MouseButton,
        connection_id: String,
        connection_key: ConnectionKey,
        corner_index: usize,
        line_origin: CorePoint,
        line_rotation: f32,
        start_pointer_model: CorePoint,
        original_points: Vec<CorePoint>,
        preview_points: Vec<CorePoint>,
        semantic_endpoints: (CorePoint, CorePoint),
        source_before: String,
    },
    CreateDiagramConnection(ConnectionCreation),
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

#[cfg(test)]
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
    original_line_points: Vec<CorePoint>,
    original_line_origin: CorePoint,
}

#[derive(Clone, Debug)]
struct ConnectionCreation {
    source_port: PortKey,
    source_connector: ConnectorRef,
    committed_points: Vec<CorePoint>,
    cursor_point: CorePoint,
    hovered_target: Option<usize>,
    last_cursor_position: PhysicalPosition<f64>,
    tail_orientation: Option<TailOrientation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TailOrientation {
    HorizontalFirst,
    VerticalFirst,
}

#[derive(Clone, Debug)]
struct ConnectionLineEdit {
    connection_key: ConnectionKey,
    before_points: Vec<CorePoint>,
    after_points: Vec<CorePoint>,
    line_origin: CorePoint,
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
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
    CreateDiagramConnection {
        class_name: String,
        connection_key: ConnectionKey,
        before_source: String,
        after_source: String,
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
    // Text the parser saw when the document was loaded (or what we last wrote
    // to disk). Saving verifies the on-disk slice still matches before it
    // applies an edit, so an externally modified file is never overwritten.
    saved_class_text: HashMap<String, String>,
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
        // Snapshot the original text of every class once, so later disk saves
        // can verify the on-disk slice still matches before applying edits.
        let mut saved_class_text = HashMap::new();
        let mut file_cache = HashMap::<PathBuf, String>::new();
        for class in &class_sources {
            if saved_class_text.contains_key(&class.qualified_name) {
                continue;
            }
            let source = match file_cache.get(&class.source_file) {
                Some(source) => source.clone(),
                None => match fs::read_to_string(&class.source_file) {
                    Ok(source) => {
                        file_cache.insert(class.source_file.clone(), source.clone());
                        source
                    }
                    Err(_) => continue,
                },
            };
            if let Some(slice) = source.get(class.source_range.start..class.source_range.end) {
                saved_class_text.insert(class.qualified_name.clone(), slice.to_owned());
            }
        }
        Ok(Self {
            path: path.to_owned(),
            package_name: package.qualified_name,
            class_names,
            diagnostics: package.diagnostics.len(),
            icons,
            diagrams,
            class_sources,
            source_overrides: HashMap::new(),
            saved_class_text,
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
    // Keep CJK UI text legible on all supported desktops. The environment
    // override is useful for portable builds that ship their own font file.
    let cjk_override = std::env::var("MODELICA_VIEWER_CJK_FONT").ok();
    let cjk_candidates = [
        cjk_override.as_deref().unwrap_or(""),
        "/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc",
        "/usr/share/fonts/sarasa-gothic/Sarasa-Regular.ttc",
        "/usr/share/fonts/sarasa-gothic/Sarasa-SemiBold.ttc",
        "/usr/share/fonts/sarasa-gothic/Sarasa-Bold.ttc",
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyhbd.ttc",
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\NotoSansSC-VF.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansSC-Regular.otf",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/local/share/fonts/NotoSansCJK-Regular.ttc",
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
    let default_monospace = fonts
        .families
        .get(&FontFamily::Monospace)
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
    let mut semibold_fallback = vec![semibold_key, medium_key.clone()];
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
    let mut mono_fallback = default_monospace;
    mono_fallback.insert(0, medium_key.clone());
    if let Some(cjk_key) = cjk_key.as_ref() {
        mono_fallback.insert(1, cjk_key.clone());
    }
    fonts
        .families
        .insert(FontFamily::Name(UI_FONT_MONO.into()), mono_fallback);
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

fn ui_mono_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(UI_FONT_MONO.into()))
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum FillMode {
    Solid = 0,
    HorizontalCylinder = 1,
    VerticalCylinder = 2,
    Sphere = 3,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum DiagramRenderLayer {
    Background,
    Component,
    Connection,
    Connector,
    Overlay,
}

impl DiagramRenderLayer {
    const COUNT: usize = 5;

    fn index(self) -> usize {
        match self {
            Self::Background => 0,
            Self::Component => 1,
            Self::Connection => 2,
            Self::Connector => 3,
            Self::Overlay => 4,
        }
    }
}

const DIAGRAM_RENDER_LAYERS: [DiagramRenderLayer; 4] = [
    DiagramRenderLayer::Background,
    DiagramRenderLayer::Component,
    DiagramRenderLayer::Connection,
    DiagramRenderLayer::Connector,
];
const ICON_RENDER_LAYERS: [DiagramRenderLayer; 1] = [DiagramRenderLayer::Component];

struct Geometry {
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
    style: StyleUniform,
    layer: DiagramRenderLayer,
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
    vertex_capacity: usize,
    index_capacity: usize,
    layer: DiagramRenderLayer,
    edit_key: Option<String>,
    connection: Option<ConnectionGeometry>,
    component: Option<ComponentGeometry>,
}

struct GpuIconScene {
    geometries: Vec<GpuGeometry>,
    layer_indices: [Vec<usize>; DiagramRenderLayer::COUNT],
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

    fn update_connection_points(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        connection_id: &str,
        points: &[CorePoint],
    ) {
        for geometry in &mut self.geometries {
            if geometry.edit_key.as_deref() != Some(connection_id) {
                continue;
            }
            let Some(connection) = geometry.connection.clone() else {
                continue;
            };
            let mut line = connection.line;
            line.points = points.to_vec();
            let Some(updated) = line_geometry(&line, connection.transform)
                .into_iter()
                .next()
            else {
                continue;
            };

            if updated.vertices.len() > geometry.vertex_capacity
                || updated.indices.len() > geometry.index_capacity
            {
                let vertex_capacity = geometry
                    .vertex_capacity
                    .max(updated.vertices.len())
                    .saturating_mul(2)
                    .max(updated.vertices.len());
                let index_capacity = geometry
                    .index_capacity
                    .max(updated.indices.len())
                    .saturating_mul(2)
                    .max(updated.indices.len());
                let mut vertices = vec![
                    Vertex {
                        position: [0.0; 2],
                        local: [0.0; 2],
                    };
                    vertex_capacity
                ];
                vertices[..updated.vertices.len()].copy_from_slice(&updated.vertices);
                let mut indices = vec![0_u16; index_capacity];
                indices[..updated.indices.len()].copy_from_slice(&updated.indices);
                geometry.vertex_buffer =
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("diagram connection preview vertices"),
                        contents: bytemuck::cast_slice(&vertices),
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    });
                geometry.index_buffer =
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("diagram connection preview indices"),
                        contents: bytemuck::cast_slice(&indices),
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    });
                geometry.vertex_capacity = vertex_capacity;
                geometry.index_capacity = index_capacity;
            } else {
                queue.write_buffer(
                    &geometry.vertex_buffer,
                    0,
                    bytemuck::cast_slice(&updated.vertices),
                );
                queue.write_buffer(
                    &geometry.index_buffer,
                    0,
                    bytemuck::cast_slice(&updated.indices),
                );
            }
            geometry.index_count = updated.indices.len() as u32;
            geometry.base_vertices = updated.vertices;
            if let Some(connection) = &mut geometry.connection {
                connection.line.points = points.to_vec();
            }
        }
    }

    fn preview_connection_points(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        connection_id: &str,
        points: &[CorePoint],
    ) {
        self.update_connection_points(device, queue, connection_id, points);
    }

    fn commit_connection_points(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        connection_id: &str,
        points: &[CorePoint],
    ) {
        self.update_connection_points(device, queue, connection_id, points);
    }
}

const CONNECTION_PREVIEW_EXTRA_SEGMENTS: usize = 4;
const MIN_CONNECTION_PREVIEW_SEGMENTS: usize = 8;

fn connection_preview_segment_capacity(segment_count: usize) -> usize {
    segment_count
        .saturating_add(CONNECTION_PREVIEW_EXTRA_SEGMENTS)
        .max(MIN_CONNECTION_PREVIEW_SEGMENTS)
}

fn connection_preview_indices(segment_capacity: usize) -> Vec<u16> {
    (0..segment_capacity)
        .flat_map(|segment| {
            let base = (segment * 4) as u16;
            [base, base + 1, base + 2, base, base + 2, base + 3]
        })
        .collect()
}

/// Persistent variable-topology line mesh used while a connection is being
/// dragged. Endpoint routing can add bridge segments, so the mesh reserves
/// spare quad slots at drag start and only updates the active vertex prefix.
struct ConnectionPreviewMesh {
    connection_id: String,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    active_segment_count: usize,
    segment_capacity: usize,
    style_bind_group: wgpu::BindGroup,
    vertices: Vec<Vertex>,
    line_origin: CorePoint,
    line_rotation: f32,
    line_thickness: f32,
    transform: Transform2D,
}

impl ConnectionPreviewMesh {
    fn new(
        device: &wgpu::Device,
        style_layout: &wgpu::BindGroupLayout,
        connection_id: String,
        line: &LineGraphic,
        transform: Transform2D,
    ) -> Self {
        let segment_count = line.points.len().saturating_sub(1);
        let segment_capacity = connection_preview_segment_capacity(segment_count);
        let initial_vertices = preview_connection_vertices(
            &line.points,
            line.origin,
            line.rotation,
            line.thickness,
            transform,
        );
        let mut vertices = vec![
            Vertex {
                position: [0.0; 2],
                local: [0.0; 2],
            };
            segment_capacity * 4
        ];
        vertices[..initial_vertices.len()].copy_from_slice(&initial_vertices);
        let indices = connection_preview_indices(segment_capacity);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("connection drag preview vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("connection drag preview indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let style = StyleUniform {
            color: color_rgba(line.color),
            edge_color: color_rgba(line.color),
            gradient: [0.0; 4],
            mode: FillMode::Solid as u32,
            _padding: [0; 7],
        };
        let style_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("connection drag preview style"),
            contents: bytemuck::bytes_of(&style),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let style_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("connection drag preview style bind group"),
            layout: style_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: style_buffer.as_entire_binding(),
            }],
        });
        Self {
            connection_id,
            vertex_buffer,
            index_buffer,
            active_segment_count: segment_count,
            segment_capacity,
            style_bind_group,
            vertices,
            line_origin: line.origin,
            line_rotation: line.rotation,
            line_thickness: line.thickness,
            transform,
        }
    }

    fn update(&mut self, queue: &wgpu::Queue, points: &[CorePoint]) {
        let segment_count = points.len().saturating_sub(1);
        if segment_count > self.segment_capacity {
            debug_assert!(
                segment_count <= self.segment_capacity,
                "connection preview route exceeded its reserved capacity"
            );
            return;
        }
        update_preview_connection_vertices(
            &mut self.vertices[..segment_count * 4],
            points,
            self.line_origin,
            self.line_rotation,
            self.line_thickness,
            self.transform,
        );
        self.active_segment_count = segment_count;
        if segment_count > 0 {
            queue.write_buffer(
                &self.vertex_buffer,
                0,
                bytemuck::cast_slice(&self.vertices[..segment_count * 4]),
            );
        }
    }
}

const INITIAL_CONNECTION_CREATION_SEGMENTS: usize = 128;

/// Transient mesh used while creating a new connection from a port. The index
/// topology is allocated once with spare capacity; cursor movement only
/// rewrites the active vertex prefix and changes the draw count.
struct ConnectionCreationPreview {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    active_segment_count: usize,
    segment_capacity: usize,
    style_bind_group: wgpu::BindGroup,
    vertices: Vec<Vertex>,
    frozen_segment_count: usize,
    dynamic_segment_count: usize,
    needs_full_upload: bool,
}

impl ConnectionCreationPreview {
    fn new(device: &wgpu::Device, style_layout: &wgpu::BindGroupLayout) -> Self {
        Self::with_capacity(device, style_layout, INITIAL_CONNECTION_CREATION_SEGMENTS)
    }

    fn with_capacity(
        device: &wgpu::Device,
        style_layout: &wgpu::BindGroupLayout,
        segment_capacity: usize,
    ) -> Self {
        let vertices = vec![
            Vertex {
                position: [0.0; 2],
                local: [0.0; 2],
            };
            segment_capacity * 4
        ];
        let indices = (0..segment_capacity)
            .flat_map(|segment| {
                let base = (segment * 4) as u16;
                [base, base + 1, base + 2, base, base + 2, base + 3]
            })
            .collect::<Vec<_>>();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("connection creation preview vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("connection creation preview indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let style = StyleUniform {
            color: color_rgba([108, 92, 231]),
            edge_color: color_rgba([108, 92, 231]),
            gradient: [0.0; 4],
            mode: FillMode::Solid as u32,
            _padding: [0; 7],
        };
        let style_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("connection creation preview style"),
            contents: bytemuck::bytes_of(&style),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let style_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("connection creation preview style bind group"),
            layout: style_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: style_buffer.as_entire_binding(),
            }],
        });
        Self {
            vertex_buffer,
            index_buffer,
            active_segment_count: 0,
            segment_capacity,
            style_bind_group,
            vertices,
            frozen_segment_count: 0,
            dynamic_segment_count: 0,
            needs_full_upload: true,
        }
    }

    fn ensure_capacity(
        &mut self,
        device: &wgpu::Device,
        style_layout: &wgpu::BindGroupLayout,
        required_segments: usize,
    ) -> bool {
        if required_segments <= self.segment_capacity {
            return false;
        }
        let mut segment_capacity = self.segment_capacity.max(1);
        while segment_capacity < required_segments {
            segment_capacity = segment_capacity.saturating_mul(2);
        }
        let mut replacement = Self::with_capacity(device, style_layout, segment_capacity);
        replacement.active_segment_count = self.active_segment_count;
        replacement.frozen_segment_count = self.frozen_segment_count;
        replacement.dynamic_segment_count = self.dynamic_segment_count;
        replacement.vertices[..self.active_segment_count * 4]
            .copy_from_slice(&self.vertices[..self.active_segment_count * 4]);
        *self = replacement;
        true
    }

    fn update_dynamic_tail(
        &mut self,
        queue: &wgpu::Queue,
        anchor: CorePoint,
        elbow: Option<CorePoint>,
        cursor_point: CorePoint,
    ) -> PreviewUpdateTiming {
        let geometry_started = Instant::now();
        let dynamic_start = self.frozen_segment_count;
        let dynamic_segment_count =
            self.update_tail_vertices(dynamic_start, anchor, elbow, cursor_point);
        self.dynamic_segment_count = dynamic_segment_count;
        self.active_segment_count = dynamic_start + dynamic_segment_count;
        let geometry = geometry_started.elapsed();
        let upload = self.upload_active_range(queue, dynamic_start);
        PreviewUpdateTiming { geometry, upload }
    }

    fn freeze_waypoint(&mut self, queue: &wgpu::Queue) -> PreviewUpdateTiming {
        debug_assert!(self.dynamic_segment_count > 0);
        self.frozen_segment_count += 1;
        self.dynamic_segment_count = self.dynamic_segment_count.saturating_sub(1);
        self.active_segment_count = self.frozen_segment_count + self.dynamic_segment_count;
        // The first dynamic segment is already the segment being frozen and
        // the remaining tail is already resident in the adjacent slot. A
        // normal click therefore changes metadata only. A full upload is
        // needed only when this preview just grew beyond its capacity.
        let upload = if self.needs_full_upload {
            self.upload_active_range(queue, 0)
        } else {
            Duration::ZERO
        };
        PreviewUpdateTiming {
            geometry: Duration::ZERO,
            upload,
        }
    }

    fn update_tail_vertices(
        &mut self,
        segment_index: usize,
        anchor: CorePoint,
        elbow: Option<CorePoint>,
        cursor_point: CorePoint,
    ) -> usize {
        let mut points = [anchor; 3];
        let point_count: usize = if let Some(elbow) = elbow {
            points[1] = elbow;
            points[2] = cursor_point;
            3
        } else if anchor != cursor_point {
            points[1] = cursor_point;
            2
        } else {
            1
        };
        let segment_count = point_count.saturating_sub(1);
        if segment_count != 0 {
            update_preview_connection_vertices(
                &mut self.vertices[segment_index * 4..(segment_index + segment_count) * 4],
                &points[..point_count],
                CorePoint { x: 0.0, y: 0.0 },
                0.0,
                0.35,
                Transform2D {
                    scale_y: -1.0,
                    ..Transform2D::identity()
                },
            );
        }
        segment_count
    }

    fn upload_active_range(&mut self, queue: &wgpu::Queue, start_segment: usize) -> Duration {
        let upload_start = if self.needs_full_upload {
            0
        } else {
            start_segment
        };
        if upload_start >= self.active_segment_count {
            self.needs_full_upload = false;
            return Duration::ZERO;
        }
        let upload_started = Instant::now();
        queue.write_buffer(
            &self.vertex_buffer,
            (upload_start * 4 * std::mem::size_of::<Vertex>()) as wgpu::BufferAddress,
            bytemuck::cast_slice(&self.vertices[upload_start * 4..self.active_segment_count * 4]),
        );
        self.needs_full_upload = false;
        upload_started.elapsed()
    }
}

#[derive(Clone, Copy, Debug)]
struct PreviewUpdateTiming {
    geometry: Duration,
    upload: Duration,
}

const CONNECTION_CREATION_ORIENTATION_SWITCH_PIXELS: f32 = 8.0;

fn update_connection_creation_orientation(
    current: Option<TailOrientation>,
    anchor: CorePoint,
    cursor_point: CorePoint,
    zoom: f32,
) -> Option<TailOrientation> {
    let dx = (cursor_point.x - anchor.x).abs();
    let dy = (cursor_point.y - anchor.y).abs();
    if dx <= ORTHOGONAL_EPSILON || dy <= ORTHOGONAL_EPSILON {
        return current;
    }
    let preferred = || {
        if dx >= dy {
            TailOrientation::HorizontalFirst
        } else {
            TailOrientation::VerticalFirst
        }
    };
    let Some(current) = current else {
        return Some(preferred());
    };
    let switch_threshold = CONNECTION_CREATION_ORIENTATION_SWITCH_PIXELS / zoom.max(MIN_ZOOM);
    Some(match current {
        TailOrientation::HorizontalFirst if dy > dx + switch_threshold => {
            TailOrientation::VerticalFirst
        }
        TailOrientation::VerticalFirst if dx > dy + switch_threshold => {
            TailOrientation::HorizontalFirst
        }
        _ => current,
    })
}

fn connection_creation_elbow(anchor: CorePoint, cursor_point: CorePoint) -> Option<CorePoint> {
    connection_creation_elbow_with_orientation(anchor, cursor_point, None)
}

fn connection_creation_elbow_with_orientation(
    anchor: CorePoint,
    cursor_point: CorePoint,
    orientation: Option<TailOrientation>,
) -> Option<CorePoint> {
    if (anchor.x - cursor_point.x).abs() <= ORTHOGONAL_EPSILON
        || (anchor.y - cursor_point.y).abs() <= ORTHOGONAL_EPSILON
    {
        return None;
    }
    let orientation = orientation.unwrap_or_else(|| {
        if (cursor_point.x - anchor.x).abs() >= (cursor_point.y - anchor.y).abs() {
            TailOrientation::HorizontalFirst
        } else {
            TailOrientation::VerticalFirst
        }
    });
    Some(if orientation == TailOrientation::HorizontalFirst {
        CorePoint {
            x: cursor_point.x,
            y: anchor.y,
        }
    } else {
        CorePoint {
            x: anchor.x,
            y: cursor_point.y,
        }
    })
}

fn connection_creation_tail_segment_count(anchor: CorePoint, cursor_point: CorePoint) -> usize {
    if anchor == cursor_point {
        0
    } else if connection_creation_elbow(anchor, cursor_point).is_some() {
        2
    } else {
        1
    }
}

fn append_connection_creation_tail_with_orientation(
    cursor_point: CorePoint,
    last_point: Option<&CorePoint>,
    orientation: Option<TailOrientation>,
    output: &mut Vec<CorePoint>,
) {
    let Some(&last) = last_point else {
        return;
    };
    if let Some(elbow) = connection_creation_elbow_with_orientation(last, cursor_point, orientation)
    {
        if output.last().is_none_or(|point| *point != elbow) {
            output.push(elbow);
        }
    }
    if output.last().is_none_or(|point| *point != cursor_point) {
        output.push(cursor_point);
    }
}

#[cfg(test)]
fn append_connection_creation_route(
    committed_points: &[CorePoint],
    cursor_point: CorePoint,
    output: &mut Vec<CorePoint>,
) {
    append_connection_creation_route_with_orientation(committed_points, cursor_point, None, output);
}

fn append_connection_creation_route_with_orientation(
    committed_points: &[CorePoint],
    cursor_point: CorePoint,
    orientation: Option<TailOrientation>,
    output: &mut Vec<CorePoint>,
) {
    output.clear();
    let Some(&last) = committed_points.last() else {
        return;
    };
    output.extend_from_slice(committed_points);
    append_connection_creation_tail_with_orientation(
        cursor_point,
        Some(&last),
        orientation,
        output,
    );
}

#[cfg(test)]
fn connection_creation_waypoint(
    committed_points: &[CorePoint],
    cursor_point: CorePoint,
) -> Option<CorePoint> {
    connection_creation_waypoint_with_orientation(committed_points, cursor_point, None)
}

fn connection_creation_waypoint_with_orientation(
    committed_points: &[CorePoint],
    cursor_point: CorePoint,
    orientation: Option<TailOrientation>,
) -> Option<CorePoint> {
    let last = *committed_points.last()?;
    connection_creation_elbow_with_orientation(last, cursor_point, orientation)
        .or_else(|| (last != cursor_point).then_some(cursor_point))
}

fn preview_connection_vertices(
    points: &[CorePoint],
    origin: CorePoint,
    rotation: f32,
    thickness: f32,
    transform: Transform2D,
) -> Vec<Vertex> {
    let mut vertices = vec![
        Vertex {
            position: [0.0; 2],
            local: [0.0; 2],
        };
        points.len().saturating_sub(1) * 4
    ];
    update_preview_connection_vertices(
        &mut vertices,
        points,
        origin,
        rotation,
        thickness,
        transform,
    );
    vertices
}

fn update_preview_connection_vertices(
    vertices: &mut [Vertex],
    points: &[CorePoint],
    origin: CorePoint,
    rotation: f32,
    thickness: f32,
    transform: Transform2D,
) {
    let half_width = (thickness.max(0.1) * transform_scale(transform)) * 0.5;
    let (vertex_chunks, remainder) = vertices.as_chunks_mut::<4>();
    debug_assert!(remainder.is_empty());
    for (segment, vertex_chunk) in points.windows(2).zip(vertex_chunks) {
        let start = transform_graphic_point(segment[0], origin, rotation, transform);
        let end = transform_graphic_point(segment[1], origin, rotation, transform);
        let dx = end[0] - start[0];
        let dy = end[1] - start[1];
        let length = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = if length > f32::EPSILON {
            (-dy * half_width / length, dx * half_width / length)
        } else {
            (0.0, half_width)
        };
        let positions = [
            [start[0] + nx, start[1] + ny],
            [end[0] + nx, end[1] + ny],
            [end[0] - nx, end[1] - ny],
            [start[0] - nx, start[1] - ny],
        ];
        for (vertex, position) in vertex_chunk.iter_mut().zip(positions) {
            vertex.position = position;
            vertex.local = [0.0; 2];
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

struct DragProfile {
    enabled: bool,
    last_report: Instant,
    events: u64,
    frames: u64,
    frame_samples: VecDeque<Duration>,
    preview_update: Duration,
    input: Duration,
    interaction_clone: Duration,
    snap: Duration,
    reanchor: Duration,
    tessellation: Duration,
    gpu_upload: Duration,
    scene_scan: Duration,
    egui_tessellation: Duration,
    ui: Duration,
    encode: Duration,
    present: Duration,
}

impl DragProfile {
    fn new() -> Self {
        Self {
            enabled: std::env::var_os("MODELICA_WGPU_PROFILE_DRAG").is_some(),
            last_report: Instant::now(),
            events: 0,
            frames: 0,
            frame_samples: VecDeque::with_capacity(240),
            preview_update: Duration::ZERO,
            input: Duration::ZERO,
            interaction_clone: Duration::ZERO,
            snap: Duration::ZERO,
            reanchor: Duration::ZERO,
            tessellation: Duration::ZERO,
            gpu_upload: Duration::ZERO,
            scene_scan: Duration::ZERO,
            egui_tessellation: Duration::ZERO,
            ui: Duration::ZERO,
            encode: Duration::ZERO,
            present: Duration::ZERO,
        }
    }

    fn record_event(&mut self) {
        if self.enabled {
            self.events += 1;
        }
    }

    fn record_input(&mut self, delay: Duration) {
        if self.enabled {
            self.input += delay;
        }
    }

    fn record_preview_update(&mut self, duration: Duration) {
        if self.enabled {
            self.preview_update += duration;
        }
    }

    fn record_preview(
        &mut self,
        input: Duration,
        snap: Duration,
        reanchor: Duration,
        tessellation: Duration,
        gpu_upload: Duration,
    ) {
        if !self.enabled {
            return;
        }
        self.input += input;
        self.snap += snap;
        self.reanchor += reanchor;
        self.tessellation += tessellation;
        self.gpu_upload += gpu_upload;
    }

    fn record_frame(
        &mut self,
        ui: Duration,
        encode: Duration,
        present: Duration,
        total: Duration,
        scene_scan: Duration,
        egui_tessellation: Duration,
    ) {
        if !self.enabled {
            return;
        }
        self.frames += 1;
        self.scene_scan += scene_scan;
        self.egui_tessellation += egui_tessellation;
        self.ui += ui;
        self.encode += encode;
        self.present += present;
        if self.frame_samples.len() == 240 {
            self.frame_samples.pop_front();
        }
        self.frame_samples.push_back(total);
        self.report_if_due();
    }

    fn report_if_due(&mut self) {
        if !self.enabled || self.frames == 0 || self.last_report.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        let mut samples = self.frame_samples.iter().copied().collect::<Vec<_>>();
        samples.sort_unstable();
        let percentile = |percent: f32| -> f64 {
            let index = ((samples.len().saturating_sub(1)) as f32 * percent).round() as usize;
            samples
                .get(index)
                .copied()
                .unwrap_or_default()
                .as_secs_f64()
                * 1_000.0
        };
        let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.0;
        eprintln!(
            "drag-profile: events={} frames={} p50_ms={:.2} p95_ms={:.2} worst_ms={:.2} preview_update_us={:.1} input_us={:.1} interaction_clone_us={:.1} snap_us={:.1} reanchor_us={:.1} tessellation_us={:.1} gpu_upload_us={:.1} scene_scan_us={:.1} egui_tessellation_us={:.1} ui_us={:.1} encode_us={:.1} present_us={:.1}",
            self.events,
            self.frames,
            percentile(0.50),
            percentile(0.95),
            percentile(1.0),
            micros(self.preview_update),
            micros(self.input),
            micros(self.interaction_clone),
            micros(self.snap),
            micros(self.reanchor),
            micros(self.tessellation),
            micros(self.gpu_upload),
            micros(self.scene_scan),
            micros(self.egui_tessellation),
            micros(self.ui),
            micros(self.encode),
            micros(self.present),
        );
        self.last_report = Instant::now();
        self.events = 0;
        self.frames = 0;
        self.preview_update = Duration::ZERO;
        self.input = Duration::ZERO;
        self.interaction_clone = Duration::ZERO;
        self.snap = Duration::ZERO;
        self.reanchor = Duration::ZERO;
        self.tessellation = Duration::ZERO;
        self.gpu_upload = Duration::ZERO;
        self.scene_scan = Duration::ZERO;
        self.egui_tessellation = Duration::ZERO;
        self.ui = Duration::ZERO;
        self.encode = Duration::ZERO;
        self.present = Duration::ZERO;
    }
}

struct ConnectionCreationProfile {
    enabled: bool,
    events: u64,
    frames: u64,
    frame_samples: VecDeque<Duration>,
    input_to_frame: Duration,
    cursor_update: Duration,
    port_query: Duration,
    preview_cpu: Duration,
    preview_upload: Duration,
    scene_scan: Duration,
    ui: Duration,
    encode: Duration,
    present: Duration,
    waypoint_input: Duration,
    waypoint_state_update: Duration,
    waypoint_cpu_geometry: Duration,
    waypoint_gpu_upload: Duration,
    waypoint_render_encode: Duration,
    waypoint_present: Duration,
    waypoint_total_frame: Duration,
    waypoint_heap_allocations: u64,
    waypoint_gpu_buffer_allocations: u64,
    waypoint_window_frames: Vec<Duration>,
    cursor_snap: Duration,
    dynamic_tail_cpu: Duration,
    dynamic_tail_upload: Duration,
    cursor_total_frame: Duration,
    final_commit: Duration,
}

impl ConnectionCreationProfile {
    fn new() -> Self {
        Self {
            enabled: std::env::var_os("MODELICA_WGPU_PROFILE_CONNECTION_CREATE").is_some(),
            events: 0,
            frames: 0,
            frame_samples: VecDeque::with_capacity(240),
            input_to_frame: Duration::ZERO,
            cursor_update: Duration::ZERO,
            port_query: Duration::ZERO,
            preview_cpu: Duration::ZERO,
            preview_upload: Duration::ZERO,
            scene_scan: Duration::ZERO,
            ui: Duration::ZERO,
            encode: Duration::ZERO,
            present: Duration::ZERO,
            waypoint_input: Duration::ZERO,
            waypoint_state_update: Duration::ZERO,
            waypoint_cpu_geometry: Duration::ZERO,
            waypoint_gpu_upload: Duration::ZERO,
            waypoint_render_encode: Duration::ZERO,
            waypoint_present: Duration::ZERO,
            waypoint_total_frame: Duration::ZERO,
            waypoint_heap_allocations: 0,
            waypoint_gpu_buffer_allocations: 0,
            waypoint_window_frames: Vec::with_capacity(7),
            cursor_snap: Duration::ZERO,
            dynamic_tail_cpu: Duration::ZERO,
            dynamic_tail_upload: Duration::ZERO,
            cursor_total_frame: Duration::ZERO,
            final_commit: Duration::ZERO,
        }
    }

    fn start(&mut self) {
        if !self.enabled {
            return;
        }
        self.events = 0;
        self.frames = 0;
        self.frame_samples.clear();
        self.input_to_frame = Duration::ZERO;
        self.cursor_update = Duration::ZERO;
        self.port_query = Duration::ZERO;
        self.preview_cpu = Duration::ZERO;
        self.preview_upload = Duration::ZERO;
        self.scene_scan = Duration::ZERO;
        self.ui = Duration::ZERO;
        self.encode = Duration::ZERO;
        self.present = Duration::ZERO;
        self.waypoint_input = Duration::ZERO;
        self.waypoint_state_update = Duration::ZERO;
        self.waypoint_cpu_geometry = Duration::ZERO;
        self.waypoint_gpu_upload = Duration::ZERO;
        self.waypoint_render_encode = Duration::ZERO;
        self.waypoint_present = Duration::ZERO;
        self.waypoint_total_frame = Duration::ZERO;
        self.waypoint_heap_allocations = 0;
        self.waypoint_gpu_buffer_allocations = 0;
        self.waypoint_window_frames.clear();
        self.cursor_snap = Duration::ZERO;
        self.dynamic_tail_cpu = Duration::ZERO;
        self.dynamic_tail_upload = Duration::ZERO;
        self.cursor_total_frame = Duration::ZERO;
        self.final_commit = Duration::ZERO;
    }

    fn record_event(&mut self) {
        if self.enabled {
            self.events += 1;
        }
    }

    fn record_input_to_frame(&mut self, duration: Duration) {
        if self.enabled {
            self.input_to_frame += duration;
        }
    }

    fn record_cursor_update(
        &mut self,
        cursor_update: Duration,
        snap_query: Duration,
        preview_cpu: Duration,
        preview_gpu_upload: Duration,
    ) {
        if !self.enabled {
            return;
        }
        self.cursor_update += cursor_update;
        self.port_query += snap_query;
        self.preview_cpu += preview_cpu;
        self.preview_upload += preview_gpu_upload;
        self.cursor_snap += snap_query;
        self.dynamic_tail_cpu += preview_cpu;
        self.dynamic_tail_upload += preview_gpu_upload;
    }

    fn record_frame(
        &mut self,
        ui: Duration,
        encode: Duration,
        present: Duration,
        total: Duration,
        scene_scan: Duration,
    ) {
        if !self.enabled {
            return;
        }
        self.frames += 1;
        self.ui += ui;
        self.encode += encode;
        self.present += present;
        self.scene_scan += scene_scan;
        self.cursor_total_frame += total;
        if self.frame_samples.len() == 240 {
            self.frame_samples.pop_front();
        }
        self.frame_samples.push_back(total);
    }

    fn record_waypoint(
        &mut self,
        input: Duration,
        state_update: Duration,
        cpu_geometry: Duration,
        gpu_upload: Duration,
    ) {
        if self.enabled {
            self.waypoint_input += input;
            self.waypoint_state_update += state_update;
            self.waypoint_cpu_geometry += cpu_geometry;
            self.waypoint_gpu_upload += gpu_upload;
        }
    }

    fn record_waypoint_frame(&mut self, encode: Duration, present: Duration, total: Duration) {
        if self.enabled {
            self.waypoint_render_encode += encode;
            self.waypoint_present += present;
            self.waypoint_total_frame += total;
        }
    }

    fn begin_waypoint_window(&mut self) {
        if !self.enabled {
            return;
        }
        self.waypoint_window_frames.clear();
        self.waypoint_window_frames
            .extend(self.frame_samples.iter().rev().take(3).rev().copied());
    }

    fn record_waypoint_window_frame(&mut self, total: Duration) {
        if self.enabled {
            self.waypoint_window_frames.push(total);
        }
    }

    fn record_waypoint_heap_allocation(&mut self) {
        if self.enabled {
            self.waypoint_heap_allocations += 1;
        }
    }

    fn record_waypoint_gpu_buffer_allocation(&mut self) {
        if self.enabled {
            self.waypoint_gpu_buffer_allocations += 1;
        }
    }

    fn record_final_commit(&mut self, duration: Duration) {
        if self.enabled {
            self.final_commit += duration;
        }
    }

    fn finish(&mut self) {
        if !self.enabled {
            return;
        }
        let mut samples = self.frame_samples.iter().copied().collect::<Vec<_>>();
        samples.sort_unstable();
        let percentile = |percent: f32| -> f64 {
            let index = ((samples.len().saturating_sub(1)) as f32 * percent).round() as usize;
            samples
                .get(index)
                .copied()
                .unwrap_or_default()
                .as_secs_f64()
                * 1_000.0
        };
        let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.0;
        let millis = |duration: Duration| duration.as_secs_f64() * 1_000.0;
        let waypoint_window_ms = self
            .waypoint_window_frames
            .iter()
            .map(|duration| format!("{:.2}", millis(*duration)))
            .collect::<Vec<_>>()
            .join(",");
        eprintln!(
            "connection-create-profile: create_events={} create_frames={} cursor_p50_ms={:.2} cursor_p95_ms={:.2} cursor_worst_ms={:.2} input_to_frame_us={:.1} cursor_total_frame_ms={:.2} cursor_update_us={:.1} cursor_snap_us={:.1} dynamic_tail_cpu_us={:.1} dynamic_tail_upload_us={:.1} render_scene_scan_us={:.1} egui_us={:.1} encode_us={:.1} present_us={:.1} waypoint_input_us={:.1} waypoint_state_update_us={:.1} waypoint_cpu_geometry_us={:.1} waypoint_gpu_upload_us={:.1} waypoint_render_encode_us={:.1} waypoint_present_us={:.1} waypoint_total_frame_ms={:.2} waypoint_window_frames={} waypoint_window_ms=[{}] waypoint_heap_allocations={} waypoint_gpu_buffer_allocations={} final_commit_ms={:.2}",
            self.events,
            self.frames,
            percentile(0.50),
            percentile(0.95),
            percentile(1.0),
            micros(self.input_to_frame),
            millis(self.cursor_total_frame),
            micros(self.cursor_update),
            micros(self.port_query),
            micros(self.preview_cpu),
            micros(self.preview_upload),
            micros(self.scene_scan),
            micros(self.ui),
            micros(self.encode),
            micros(self.present),
            micros(self.waypoint_input),
            micros(self.waypoint_state_update),
            micros(self.waypoint_cpu_geometry),
            micros(self.waypoint_gpu_upload),
            micros(self.waypoint_render_encode),
            micros(self.waypoint_present),
            millis(self.waypoint_total_frame),
            self.waypoint_window_frames.len(),
            waypoint_window_ms,
            self.waypoint_heap_allocations,
            self.waypoint_gpu_buffer_allocations,
            millis(self.final_commit),
        );
        self.start();
    }
}

struct EditCommitProfile {
    enabled: bool,
    started: Instant,
    source_patch: Duration,
    resolve_candidate: Duration,
    semantic_validation: Duration,
    document_update: Duration,
    gpu_update: Duration,
    hit_index_update: Duration,
}

impl EditCommitProfile {
    fn new() -> Self {
        Self {
            enabled: std::env::var_os("MODELICA_WGPU_PROFILE_EDIT").is_some(),
            started: Instant::now(),
            source_patch: Duration::ZERO,
            resolve_candidate: Duration::ZERO,
            semantic_validation: Duration::ZERO,
            document_update: Duration::ZERO,
            gpu_update: Duration::ZERO,
            hit_index_update: Duration::ZERO,
        }
    }

    fn micros(duration: Duration) -> f64 {
        duration.as_secs_f64() * 1_000_000.0
    }
}

impl Drop for EditCommitProfile {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        eprintln!(
            "connection-commit: patch_us={:.1} resolve_us={:.1} validate_us={:.1} document_us={:.1} gpu_us={:.1} hit_index_us={:.1} total_us={:.1}",
            Self::micros(self.source_patch),
            Self::micros(self.resolve_candidate),
            Self::micros(self.semantic_validation),
            Self::micros(self.document_update),
            Self::micros(self.gpu_update),
            Self::micros(self.hit_index_update),
            Self::micros(self.started.elapsed()),
        );
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
    background_dirty: bool,
    background_is_dark: bool,
    pipeline: wgpu::RenderPipeline,
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    scene: GpuIconScene,
    style_layout: wgpu::BindGroupLayout,
    document: Option<LoadedDocument>,
    loading_document: Option<JoinHandle<Result<LoadedDocument, String>>>,
    load_error: Option<String>,
    selected_class: Option<String>,
    ui_document: Option<UiDocument>,
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
    hovered_port: Option<PortKey>,
    diagram_hit_cache: DiagramHitCache,
    pointer_interaction: PointerInteraction,
    connection_preview: Option<ConnectionPreviewMesh>,
    connection_creation_preview: Option<ConnectionCreationPreview>,
    pending_drag_position: Option<(PhysicalPosition<f64>, Instant)>,
    pending_waypoint: bool,
    pending_waypoint_queued_at: Option<Instant>,
    waypoint_frame_pending: bool,
    waypoint_profile_frames_remaining: u8,
    suppress_next_creation_left_release_redraw: bool,
    drag_profile: DragProfile,
    connection_creation_profile: ConnectionCreationProfile,
    history: Vec<EditCommand>,
    redo_history: Vec<EditCommand>,
    stats: FrameStats,
}

fn select_present_mode(
    supported_modes: &[wgpu::PresentMode],
    low_latency_requested: bool,
) -> wgpu::PresentMode {
    if low_latency_requested {
        for mode in [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate] {
            if supported_modes.contains(&mode) {
                return mode;
            }
        }
        eprintln!(
            "modelica-wgpu: low-latency present modes are unavailable; using a stable vsync mode"
        );
    }

    [wgpu::PresentMode::Fifo, wgpu::PresentMode::AutoVsync]
        .into_iter()
        .find(|mode| supported_modes.contains(mode))
        .or_else(|| supported_modes.first().copied())
        .unwrap_or(wgpu::PresentMode::Fifo)
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
        if std::env::var_os("MODELICA_WGPU_DEBUG_DIAGRAM").is_some() {
            eprintln!(
                "modelica-wgpu surface: format={format:?}, srgb={} (annotation colors use linear output)",
                format.is_srgb()
            );
        }
        let no_vsync = std::env::var("MODELICA_WGPU_VSYNC")
            .map(|value| matches!(value.to_ascii_lowercase().as_str(), "0" | "off" | "false"))
            .unwrap_or(false);
        let present_mode = select_present_mode(&capabilities.present_modes, no_vsync);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 1,
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
        set_theme(
            ThemeMode::System.is_dark(window.theme()),
            AccentTheme::Violet,
        );
        configure_egui_style(&egui_ctx);
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
        // Match the Electron client defaults: follow the OS, with its violet
        // accent as the stable cross-platform baseline. Any saved appearance
        // from a previous run is restored here.
        let (initial_theme_mode, initial_accent_theme) = load_appearance();
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
            background_dirty: true,
            background_is_dark: false,
            pipeline,
            view_buffer,
            view_bind_group,
            scene,
            style_layout,
            document,
            loading_document: None,
            load_error: None,
            selected_class: None,
            ui_document: None,
            expanded_nodes: HashSet::new(),
            egui_ctx,
            egui_state,
            egui_renderer,
            theme_mode: initial_theme_mode,
            accent_theme: initial_accent_theme,
            main_view: MainView::Icon,
            diagram_scene,
            zoom: INITIAL_ZOOM,
            pan: [0.0, 0.0],
            cursor: PhysicalPosition::new(0.0, 0.0),
            modifiers: ModifiersState::default(),
            canvas_rect: None,
            diagram_selection: DiagramSelection::None,
            hovered_port: None,
            diagram_hit_cache: DiagramHitCache::default(),
            pointer_interaction: PointerInteraction::None,
            connection_preview: None,
            connection_creation_preview: None,
            pending_drag_position: None,
            pending_waypoint: false,
            pending_waypoint_queued_at: None,
            waypoint_frame_pending: false,
            waypoint_profile_frames_remaining: 0,
            suppress_next_creation_left_release_redraw: false,
            drag_profile: DragProfile::new(),
            connection_creation_profile: ConnectionCreationProfile::new(),
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

    fn refresh_ui_document(&mut self) {
        self.ui_document = self
            .document
            .as_ref()
            .map(|document| document.ui_summary(self.selected_class.as_deref()));
    }

    fn selected_connection_id(&self) -> Option<&str> {
        match &self.diagram_selection {
            DiagramSelection::Connection(connection_id) => Some(connection_id),
            _ => None,
        }
    }

    fn set_diagram_selection(&mut self, selection: DiagramSelection) -> bool {
        if self.diagram_selection == selection {
            return false;
        }
        self.diagram_selection = selection;
        true
    }

    fn hit_test_diagram_connection(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
    ) -> Option<ConnectionHit> {
        let started = Instant::now();
        let class_name = self.selected_class_name()?;
        let scene = self.document.as_ref()?.diagram(class_name)?;
        let query_started = Instant::now();
        let candidates = self
            .diagram_hit_cache
            .spatial_index
            .query(pointer_model, tolerance);
        let spatial_query_us = query_started.elapsed().as_secs_f64() * 1_000_000.0;
        let precise_started = Instant::now();
        let selected_connection_id = self.selected_connection_id();
        let mut best: Option<(ConnectionHit, f32, bool, usize, usize)> = None;
        for segment in &candidates.connection_segments {
            let Some(connection) = scene.connections.get(segment.connection_index) else {
                continue;
            };
            let Some((hit, distance)) = hit_test_connection_segment_with_distance(
                connection,
                segment.segment_index,
                pointer_model,
                tolerance,
            ) else {
                continue;
            };
            let selected = selected_connection_id == Some(connection.id.as_str());
            let best_key =
                best.as_ref()
                    .map(|(_, distance, selected, connection_index, segment_index)| {
                        (*distance, *selected, *connection_index, *segment_index)
                    });
            if connection_hit_candidate_is_better(
                distance,
                selected,
                segment.connection_index,
                segment.segment_index,
                best_key,
            ) {
                best = Some((
                    hit,
                    distance,
                    selected,
                    segment.connection_index,
                    segment.segment_index,
                ));
            }
        }
        if std::env::var_os("MODELICA_WGPU_PROFILE_HIT_TEST").is_some() {
            eprintln!(
                "hit-test connection: spatial_query_us={:.1} connection_segment_candidates={} precise_test_us={:.1} total_mouse_down_us={:.1}",
                spatial_query_us,
                candidates.connection_segments.len(),
                precise_started.elapsed().as_secs_f64() * 1_000_000.0,
                started.elapsed().as_secs_f64() * 1_000_000.0
            );
        }
        best.map(|(hit, _, _, _, _)| hit)
    }

    fn diagram_connector_anchors(&self) -> Option<&[ConnectorAnchor]> {
        self.selected_class_name()
            .filter(|_| !self.diagram_hit_cache.ports.is_empty())
            .map(|_| self.diagram_hit_cache.ports.as_slice())
    }

    fn diagram_anchor(&self, key: &PortKey) -> Option<&ConnectorAnchor> {
        self.diagram_hit_cache
            .ports
            .iter()
            .find(|anchor| anchor.key == *key)
    }

    fn hit_test_diagram_port(&self, pointer_model: CorePoint, tolerance: f32) -> Option<PortKey> {
        let started = Instant::now();
        let anchors = self.diagram_connector_anchors()?;
        let query_started = Instant::now();
        let candidates = self
            .diagram_hit_cache
            .spatial_index
            .query(pointer_model, tolerance);
        let spatial_query_us = query_started.elapsed().as_secs_f64() * 1_000_000.0;
        let precise_started = Instant::now();
        let result = candidates
            .port_indices
            .iter()
            .filter_map(|index| anchors.get(*index))
            .filter_map(|anchor| {
                connector_anchor_hit_distance(anchor, pointer_model, tolerance)
                    .map(|distance| (distance, anchor))
            })
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map(|(_, anchor)| anchor.key.clone());
        if std::env::var_os("MODELICA_WGPU_PROFILE_HIT_TEST").is_some() {
            eprintln!(
                "hit-test port: spatial_query_us={:.1} port_candidates={} precise_test_us={:.1} total_mouse_down_us={:.1}",
                spatial_query_us,
                candidates.port_indices.len(),
                precise_started.elapsed().as_secs_f64() * 1_000_000.0,
                started.elapsed().as_secs_f64() * 1_000_000.0
            );
        }
        result
    }

    fn hit_test_diagram_component(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
    ) -> Option<(String, String, CorePoint)> {
        let started = Instant::now();
        let class_name = self.selected_class_name()?;
        let scene = self.document.as_ref()?.diagram(class_name)?;
        let query_started = Instant::now();
        let candidates = self
            .diagram_hit_cache
            .spatial_index
            .query(pointer_model, tolerance);
        let spatial_query_us = query_started.elapsed().as_secs_f64() * 1_000_000.0;
        let precise_started = Instant::now();
        let result = candidates.component_indices.iter().rev().find_map(|index| {
            let item = self.diagram_hit_cache.components.get(*index)?;
            if !item.bounds.contains(pointer_model, tolerance) {
                return None;
            }
            let component = scene.components.get(item.scene_index)?;
            if !component.editable
                || !component.visible
                || !diagram_component_contains_point(component, pointer_model, tolerance)
            {
                return None;
            }
            Some((
                component.id.clone(),
                component.name.clone(),
                component.origin,
            ))
        });
        if std::env::var_os("MODELICA_WGPU_PROFILE_HIT_TEST").is_some() {
            eprintln!(
                "hit-test component: spatial_query_us={:.1} component_candidates={} precise_test_us={:.1} total_mouse_down_us={:.1}",
                spatial_query_us,
                candidates.component_indices.len(),
                precise_started.elapsed().as_secs_f64() * 1_000_000.0,
                started.elapsed().as_secs_f64() * 1_000_000.0
            );
        }
        result
    }

    fn update_hovered_diagram_port(&mut self) -> bool {
        let next_hovered = if self.main_view == MainView::Diagram && self.pointer_over_canvas() {
            let pointer_model = self.screen_to_model(self.cursor);
            let tolerance = 8.0 / self.zoom.max(MIN_ZOOM);
            self.hit_test_diagram_port(pointer_model, tolerance)
        } else {
            None
        };
        if self.hovered_port == next_hovered {
            return false;
        }
        self.hovered_port = next_hovered;
        true
    }

    fn begin_connection_edit(&mut self, hit: ConnectionHit, pointer_model: CorePoint) {
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            return;
        };
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let Some((connection_key, line, original_points, semantic_endpoints, source_before)) =
            (|| {
                let scene = document.diagram(&class_name)?;
                let connection = scene
                    .connections
                    .iter()
                    .find(|connection| connection.id == hit.connection_id)?;
                let line = connection.line.as_ref()?;
                let source_before = document.class_text(&class_name)?;
                // Connector resolution belongs at drag-start. The endpoints
                // are immutable while a connection segment/corner is moved.
                let semantic_endpoints = strict_connection_points(scene, connection)
                    .unwrap_or((line.points.first().copied()?, line.points.last().copied()?));
                Some((
                    connection.key.clone(),
                    line.clone(),
                    line.points.clone(),
                    semantic_endpoints,
                    source_before,
                ))
            })()
        else {
            return;
        };
        let line_origin = line.origin;
        let line_rotation = line.rotation;
        let snap_axes = match &hit.target {
            ConnectionHitTarget::Segment { index, orientation } => {
                connection_segment_snap_axes(&original_points, *index, *orientation)
            }
            ConnectionHitTarget::Line => Vec::new(),
        };
        self.connection_preview = Some(ConnectionPreviewMesh::new(
            &self.device,
            &self.style_layout,
            hit.connection_id.clone(),
            &line,
            Transform2D {
                scale_y: -1.0,
                ..Transform2D::identity()
            },
        ));
        self.set_diagram_selection(DiagramSelection::Connection(hit.connection_id.clone()));
        // A corner (inner polyline vertex) can be dragged freely. Prefer the
        // vertex handle over a segment slide when the pointer is close to it,
        // so the highlighted vertex dots act as real grab handles.
        if original_points.len() >= 4 {
            let tolerance = 10.0 / self.zoom.max(MIN_ZOOM);
            let line = temporary_line(line_origin, line_rotation);
            let world_points = connection_world_points(&line, &original_points);
            let mut best: Option<(usize, f32)> = None;
            for (index, point) in world_points
                .iter()
                .enumerate()
                .skip(1)
                .take(world_points.len().saturating_sub(2))
            {
                let distance = distance_between(*point, pointer_model);
                if distance <= tolerance && best.is_none_or(|(_, best)| distance < best) {
                    best = Some((index, distance));
                }
            }
            if let Some((corner_index, _)) = best {
                self.pointer_interaction = PointerInteraction::MoveDiagramConnectionCorner {
                    button: MouseButton::Left,
                    connection_id: hit.connection_id,
                    connection_key,
                    corner_index,
                    line_origin,
                    line_rotation,
                    start_pointer_model: pointer_model,
                    original_points: original_points.clone(),
                    preview_points: original_points,
                    semantic_endpoints,
                    source_before,
                };
                return;
            }
        }
        match hit.target {
            ConnectionHitTarget::Segment { index, orientation }
                if index + 1 < original_points.len() =>
            {
                self.pointer_interaction = PointerInteraction::MoveDiagramConnectionSegment {
                    button: MouseButton::Left,
                    connection_id: hit.connection_id,
                    connection_key,
                    segment_index: index,
                    orientation,
                    line_origin,
                    line_rotation,
                    start_pointer_model: pointer_model,
                    original_points: original_points.clone(),
                    preview_points: original_points,
                    semantic_endpoints,
                    snap_axes,
                    snapped_axis: None,
                    source_before,
                };
            }
            _ => self.connection_preview = None,
        }
    }

    fn begin_connection_creation(&mut self, source: &ConnectorAnchor) {
        self.connection_creation_profile.start();
        self.connection_preview = None;
        self.pending_waypoint = false;
        self.pending_waypoint_queued_at = None;
        self.waypoint_frame_pending = false;
        self.waypoint_profile_frames_remaining = 0;
        self.suppress_next_creation_left_release_redraw = false;
        self.pointer_interaction =
            PointerInteraction::CreateDiagramConnection(ConnectionCreation {
                source_port: source.key.clone(),
                source_connector: source.connector_ref.clone(),
                committed_points: {
                    let mut points = Vec::with_capacity(INITIAL_CONNECTION_CREATION_SEGMENTS + 1);
                    points.push(source.world_position);
                    points
                },
                cursor_point: source.world_position,
                hovered_target: None,
                last_cursor_position: self.cursor,
                tail_orientation: None,
            });
        self.connection_creation_preview = Some(ConnectionCreationPreview::new(
            &self.device,
            &self.style_layout,
        ));
        self.hovered_port = None;
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
            PointerInteraction::MoveDiagramConnectionCorner {
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
                let hit_test_started = Instant::now();
                let Some(document) = self.document.as_ref() else {
                    return;
                };
                let spatial_query_started = Instant::now();
                let spatial_candidates = self
                    .diagram_hit_cache
                    .spatial_index
                    .query(pointer_model, tolerance);
                if spatial_candidates.component_indices.is_empty()
                    && spatial_candidates.port_indices.is_empty()
                    && spatial_candidates.connection_segments.is_empty()
                {
                    self.set_diagram_selection(DiagramSelection::None);
                    self.pointer_interaction = PointerInteraction::None;
                    if std::env::var_os("MODELICA_WGPU_PROFILE_HIT_TEST").is_some() {
                        eprintln!(
                            "hit-test blank: spatial_query_us={:.1} port_candidates=0 component_candidates=0 connection_segment_candidates=0 precise_test_us=0.0 total_mouse_down_us={:.1}",
                            spatial_query_started.elapsed().as_secs_f64() * 1_000_000.0,
                            hit_test_started.elapsed().as_secs_f64() * 1_000_000.0
                        );
                    }
                    return;
                }
                if let Some(port) = self.hit_test_diagram_port(pointer_model, tolerance) {
                    let source = self
                        .diagram_anchor(&port)
                        .filter(|anchor| anchor.editable)
                        .cloned();
                    self.set_diagram_selection(DiagramSelection::Port(port.clone()));
                    self.hovered_port = Some(port);
                    if let Some(source) = source {
                        self.begin_connection_creation(&source);
                    } else {
                        self.pointer_interaction = PointerInteraction::None;
                    }
                    return;
                }
                if let Some((component_name, handle)) =
                    self.hit_test_selected_component_handle(pointer_model, tolerance)
                {
                    self.begin_component_resize(component_name, handle);
                    return;
                }
                let Some((component_id, component_name, original_origin)) =
                    self.hit_test_diagram_component(pointer_model, tolerance)
                else {
                    if let Some(hit) = self.hit_test_diagram_connection(pointer_model, tolerance) {
                        self.begin_connection_edit(hit, pointer_model);
                    } else {
                        self.set_diagram_selection(DiagramSelection::None);
                    }
                    if std::env::var_os("MODELICA_WGPU_PROFILE_HIT_TEST").is_some() {
                        eprintln!(
                            "hit-test mouse-down: total_mouse_down_us={:.1}",
                            hit_test_started.elapsed().as_secs_f64() * 1_000_000.0
                        );
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
                        if connection.lhs.component_name != component_name
                            && connection.rhs.component_name != component_name
                        {
                            return None;
                        }
                        let line = connection.line.as_ref()?;
                        Some(ConnectionDragSnapshot {
                            connection_id: connection.id.clone(),
                            connection_key: connection.key.clone(),
                            original_line_points: line.points.clone(),
                            original_line_origin: line.origin,
                        })
                    })
                    .collect();
                self.set_diagram_selection(DiagramSelection::Component(component_name.clone()));
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

    fn connection_drag_active(&self) -> bool {
        matches!(
            self.pointer_interaction,
            PointerInteraction::MoveDiagramConnectionSegment { .. }
                | PointerInteraction::MoveDiagramConnectionCorner { .. }
                | PointerInteraction::CreateDiagramConnection(..)
        )
    }

    fn connection_creation_active(&self) -> bool {
        matches!(
            self.pointer_interaction,
            PointerInteraction::CreateDiagramConnection(..)
        )
    }

    fn flush_drag_preview(&mut self) {
        let Some((position, queued_at)) = self.pending_drag_position.take() else {
            return;
        };
        let input_delay = queued_at.elapsed();
        self.drag_profile.record_input(input_delay);
        if self.connection_creation_active() {
            self.connection_creation_profile
                .record_input_to_frame(input_delay);
        }
        let preview_started = Instant::now();
        self.update_model_drag_preview(position);
        self.drag_profile
            .record_preview_update(preview_started.elapsed());
    }

    fn update_model_drag_preview(&mut self, position: PhysicalPosition<f64>) -> bool {
        // Connection drags are the high-frequency path. Borrow only their
        // small mutable state and return before the legacy interaction clone
        // used by the other, lower-frequency edit modes.
        let current = self.screen_to_model(position);
        let zoom = self.zoom;
        if let PointerInteraction::CreateDiagramConnection(creation) = &self.pointer_interaction {
            if creation.last_cursor_position == position {
                return false;
            }
            let cursor_started = Instant::now();
            let snap_started = Instant::now();
            let target = connection_creation_target(
                &mut self.diagram_hit_cache.spatial_index,
                &self.diagram_hit_cache.ports,
                zoom,
                &creation.source_port,
                creation.hovered_target,
                current,
            );
            let snap_elapsed = snap_started.elapsed();
            let target_index = target.map(|(index, _)| index);
            let target_point = target.map(|(_, point)| point).unwrap_or(current);
            let anchor = creation
                .committed_points
                .last()
                .copied()
                .unwrap_or(target_point);
            let required_segments = creation
                .committed_points
                .len()
                .saturating_sub(1)
                .saturating_add(connection_creation_tail_segment_count(anchor, target_point));
            let buffer_reallocated =
                self.connection_creation_preview
                    .as_mut()
                    .is_some_and(|preview| {
                        preview.ensure_capacity(&self.device, &self.style_layout, required_segments)
                    });
            if buffer_reallocated {
                self.connection_creation_profile
                    .record_waypoint_gpu_buffer_allocation();
                self.connection_creation_profile
                    .record_waypoint_heap_allocation();
            }
            let preview_started = Instant::now();
            let preview_timing = {
                let pointer = &mut self.pointer_interaction;
                let preview = &mut self.connection_creation_preview;
                let queue = &self.queue;
                let PointerInteraction::CreateDiagramConnection(creation) = pointer else {
                    unreachable!("connection creation ended while updating its preview")
                };
                creation.last_cursor_position = position;
                creation.cursor_point = target_point;
                creation.hovered_target = target_index;
                creation.tail_orientation = update_connection_creation_orientation(
                    creation.tail_orientation,
                    anchor,
                    creation.cursor_point,
                    zoom,
                );
                preview
                    .as_mut()
                    .map(|preview| {
                        preview.update_dynamic_tail(
                            queue,
                            anchor,
                            connection_creation_elbow_with_orientation(
                                anchor,
                                creation.cursor_point,
                                creation.tail_orientation,
                            ),
                            creation.cursor_point,
                        )
                    })
                    .unwrap_or(PreviewUpdateTiming {
                        geometry: Duration::ZERO,
                        upload: Duration::ZERO,
                    })
            };
            let preview_elapsed = preview_started.elapsed();
            self.connection_creation_profile.record_cursor_update(
                cursor_started.elapsed(),
                snap_elapsed,
                preview_timing
                    .geometry
                    .max(preview_elapsed.saturating_sub(snap_elapsed + preview_timing.upload)),
                preview_timing.upload,
            );
            return true;
        }
        {
            let (pointer, preview, queue, profile) = (
                &mut self.pointer_interaction,
                &mut self.connection_preview,
                &self.queue,
                &mut self.drag_profile,
            );
            match pointer {
                PointerInteraction::MoveDiagramConnectionSegment {
                    segment_index,
                    orientation,
                    line_origin,
                    line_rotation,
                    start_pointer_model,
                    original_points,
                    semantic_endpoints,
                    snap_axes,
                    snapped_axis,
                    preview_points,
                    ..
                } => {
                    let raw_delta = CorePoint {
                        x: current.x - start_pointer_model.x,
                        y: current.y - start_pointer_model.y,
                    };
                    let snap_started = Instant::now();
                    let base_axis =
                        connection_segment_axis(original_points, *segment_index, *orientation);
                    let delta = snap_connection_segment_delta_cached(
                        base_axis,
                        *orientation,
                        *line_rotation,
                        raw_delta,
                        snap_axes,
                        snapped_axis,
                        (
                            connection_snap_tolerance(zoom),
                            connection_snap_exit_tolerance(zoom),
                        ),
                    );
                    let snap_elapsed = snap_started.elapsed();
                    let reanchor_started = Instant::now();
                    let next_points = build_connection_segment_drag_route(
                        original_points,
                        *segment_index,
                        *orientation,
                        *line_origin,
                        *line_rotation,
                        delta,
                        *semantic_endpoints,
                    );
                    let reanchor_elapsed = reanchor_started.elapsed();
                    let upload_started = Instant::now();
                    if let Some(preview) = preview.as_mut() {
                        preview.update(queue, &next_points);
                    }
                    profile.record_preview(
                        Duration::ZERO,
                        snap_elapsed,
                        reanchor_elapsed,
                        Duration::ZERO,
                        upload_started.elapsed(),
                    );
                    *preview_points = next_points;
                    return true;
                }
                PointerInteraction::MoveDiagramConnectionCorner {
                    corner_index,
                    line_origin,
                    line_rotation,
                    start_pointer_model,
                    original_points,
                    semantic_endpoints,
                    preview_points,
                    ..
                } => {
                    let delta = CorePoint {
                        x: current.x - start_pointer_model.x,
                        y: current.y - start_pointer_model.y,
                    };
                    let reanchor_started = Instant::now();
                    let next_points = build_connection_corner_drag_route(
                        original_points,
                        *corner_index,
                        *line_origin,
                        *line_rotation,
                        delta,
                        *semantic_endpoints,
                    );
                    let reanchor_elapsed = reanchor_started.elapsed();
                    let upload_started = Instant::now();
                    if let Some(preview) = preview.as_mut() {
                        preview.update(queue, &next_points);
                    }
                    profile.record_preview(
                        Duration::ZERO,
                        Duration::ZERO,
                        reanchor_elapsed,
                        Duration::ZERO,
                        upload_started.elapsed(),
                    );
                    *preview_points = next_points;
                    return true;
                }
                _ => {}
            }
        }
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
                true
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
                true
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
                if let Some(document) = self.document.as_ref() {
                    if let Some(class_name) = self.selected_class_name() {
                        if let Some(scene) = document.diagram(class_name) {
                            let mut preview_scene = scene.clone();
                            if let Some(component) = preview_scene
                                .components
                                .iter_mut()
                                .find(|component| component.id == component_id)
                            {
                                component.origin.x += delta.x;
                                component.origin.y += delta.y;
                            }
                            for snapshot in &connected_connections {
                                let Some(connection) = preview_scene
                                    .connections
                                    .iter()
                                    .find(|connection| connection.key == snapshot.connection_key)
                                else {
                                    continue;
                                };
                                let Ok(preview_points) = reanchor_connection_points(
                                    &preview_scene,
                                    connection,
                                    &snapshot.original_line_points,
                                ) else {
                                    continue;
                                };
                                self.diagram_scene.preview_connection_points(
                                    &self.device,
                                    &self.queue,
                                    &snapshot.connection_id,
                                    &preview_points,
                                );
                            }
                        }
                    }
                }
                if let PointerInteraction::MoveDiagramComponent { preview_delta, .. } =
                    &mut self.pointer_interaction
                {
                    *preview_delta = delta;
                }
                true
            }
            PointerInteraction::MoveDiagramConnectionSegment { .. }
            | PointerInteraction::MoveDiagramConnectionCorner { .. } => {
                unreachable!("connection drags return before the clone fallback")
            }
            PointerInteraction::CreateDiagramConnection(_) => {
                unreachable!("connection creation returns before the clone fallback")
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
                let Some(icon) = original_component.diagram_layer() else {
                    return false;
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
                if let Some(document) = self.document.as_ref() {
                    if let Some(class_name) = self.selected_class_name() {
                        if let Some(scene) = document.diagram(class_name) {
                            let mut preview_scene = scene.clone();
                            if let Some(component) = preview_scene
                                .components
                                .iter_mut()
                                .find(|component| component.id == component_id)
                            {
                                component.placement_extent = Some(preview_extent);
                            }
                            for snapshot in &connected_connections {
                                let Some(connection) = preview_scene
                                    .connections
                                    .iter()
                                    .find(|connection| connection.key == snapshot.connection_key)
                                else {
                                    continue;
                                };
                                let Ok(preview_points) = reanchor_connection_points(
                                    &preview_scene,
                                    connection,
                                    &snapshot.original_line_points,
                                ) else {
                                    continue;
                                };
                                self.diagram_scene.preview_connection_points(
                                    &self.device,
                                    &self.queue,
                                    &snapshot.connection_id,
                                    &preview_points,
                                );
                            }
                        }
                    }
                }
                if let PointerInteraction::ResizeDiagramComponent {
                    preview_extent: active_preview,
                    ..
                } = &mut self.pointer_interaction
                {
                    *active_preview = preview_extent;
                }
                true
            }
            PointerInteraction::None => false,
        }
    }

    fn rebuild_selected_scenes(&mut self) {
        self.scene = build_scene(
            &self.device,
            &self.style_layout,
            self.document.as_ref(),
            self.selected_class.as_deref(),
        );
        self.connection_preview = None;
        self.diagram_scene = build_diagram_scene(
            &self.device,
            &self.style_layout,
            self.document.as_ref(),
            self.selected_class.as_deref(),
        );
        self.diagram_hit_cache =
            build_diagram_hit_cache(self.document.as_ref(), self.selected_class.as_deref());
        self.refresh_ui_document();
    }

    fn cancel_connection_creation(&mut self) {
        if !self.connection_creation_active() {
            return;
        }
        self.pointer_interaction = PointerInteraction::None;
        self.pending_drag_position = None;
        self.pending_waypoint = false;
        self.pending_waypoint_queued_at = None;
        self.waypoint_frame_pending = false;
        self.waypoint_profile_frames_remaining = 0;
        self.connection_creation_preview = None;
        self.hovered_port = None;
        self.connection_creation_profile.finish();
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
            | PointerInteraction::MoveDiagramConnectionCorner {
                button: active_button,
                ..
            }
            | PointerInteraction::ResizeDiagramComponent {
                button: active_button,
                ..
            } => *active_button == button,
            PointerInteraction::CreateDiagramConnection(_) => button == MouseButton::Left,
            PointerInteraction::None => false,
        };
        if !matches_button {
            return;
        }
        let interaction =
            std::mem::replace(&mut self.pointer_interaction, PointerInteraction::None);
        self.pending_drag_position = None;
        // Pointer-up is the boundary where the static Lyon scene may be
        // committed; the persistent preview mesh is never used for committed output.
        self.connection_preview = None;
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
                semantic_endpoints,
                snap_axes,
                mut snapped_axis,
                source_before,
                ..
            } => {
                let current = self.screen_to_model(self.cursor);
                let delta = CorePoint {
                    x: current.x - start_pointer_model.x,
                    y: current.y - start_pointer_model.y,
                };
                let delta = snap_connection_segment_delta_cached(
                    connection_segment_axis(&original_points, segment_index, orientation),
                    orientation,
                    line_rotation,
                    delta,
                    &snap_axes,
                    &mut snapped_axis,
                    (
                        connection_snap_tolerance(self.zoom),
                        connection_snap_exit_tolerance(self.zoom),
                    ),
                );
                let raw_after_points = build_connection_segment_drag_route(
                    &original_points,
                    segment_index,
                    orientation,
                    line_origin,
                    line_rotation,
                    delta,
                    semantic_endpoints,
                );
                self.commit_diagram_connection_move(
                    connection_key,
                    original_points,
                    raw_after_points,
                    source_before,
                );
            }
            PointerInteraction::MoveDiagramConnectionCorner {
                connection_id: _connection_id,
                connection_key,
                corner_index,
                line_origin,
                line_rotation,
                start_pointer_model,
                original_points,
                semantic_endpoints,
                source_before,
                ..
            } => {
                let current = self.screen_to_model(self.cursor);
                let delta = CorePoint {
                    x: current.x - start_pointer_model.x,
                    y: current.y - start_pointer_model.y,
                };
                let raw_after_points = build_connection_corner_drag_route(
                    &original_points,
                    corner_index,
                    line_origin,
                    line_rotation,
                    delta,
                    semantic_endpoints,
                );
                self.commit_diagram_connection_move(
                    connection_key,
                    original_points,
                    raw_after_points,
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
            PointerInteraction::CreateDiagramConnection(creation) => {
                // Creation clicks are queued by MouseInput and consumed from
                // RedrawRequested. Keep this fallback allocation-free if an
                // unexpected caller reaches the old pointer-up path.
                self.pointer_interaction = PointerInteraction::CreateDiagramConnection(creation);
            }
            PointerInteraction::None => {}
        }
    }

    fn process_pending_waypoint(&mut self) {
        if !self.pending_waypoint {
            return;
        }
        self.pending_waypoint = false;
        let input_delay = self
            .pending_waypoint_queued_at
            .take()
            .map(|queued_at| queued_at.elapsed())
            .unwrap_or_default();
        if !self.connection_creation_active() {
            return;
        }

        // flush_drag_preview() runs immediately before this method from the
        // redraw handler. The cached target therefore represents the latest
        // cursor position, without repeating the port spatial query here.
        let started = Instant::now();
        let interaction =
            std::mem::replace(&mut self.pointer_interaction, PointerInteraction::None);
        let PointerInteraction::CreateDiagramConnection(mut creation) = interaction else {
            return;
        };
        let current = creation.cursor_point;
        let cached_target = creation.hovered_target.and_then(|target_index| {
            self.diagram_hit_cache
                .ports
                .get(target_index)
                .filter(|anchor| anchor.editable && anchor.key != creation.source_port)
                .map(|anchor| (anchor.key.clone(), anchor.world_position))
        });
        if let Some((target_port, target_point)) = cached_target {
            let mut raw_points = Vec::with_capacity(creation.committed_points.len() + 2);
            append_connection_creation_route_with_orientation(
                &creation.committed_points,
                target_point,
                creation.tail_orientation,
                &mut raw_points,
            );
            self.connection_creation_preview = None;
            self.commit_connection_creation(
                creation.source_port,
                creation.source_connector,
                target_port,
                raw_points,
            );
            self.connection_creation_profile
                .record_final_commit(started.elapsed());
            self.connection_creation_profile.finish();
            return;
        }

        let Some(waypoint) = connection_creation_waypoint_with_orientation(
            &creation.committed_points,
            current,
            creation.tail_orientation,
        ) else {
            self.connection_creation_preview = None;
            self.hovered_port = None;
            self.connection_creation_profile.finish();
            return;
        };
        let state_started = Instant::now();
        let mut heap_allocated = false;
        if creation.committed_points.len() == creation.committed_points.capacity() {
            creation.committed_points.reserve(
                creation
                    .committed_points
                    .capacity()
                    .max(INITIAL_CONNECTION_CREATION_SEGMENTS),
            );
            heap_allocated = true;
        }
        creation.committed_points.push(waypoint);
        creation.hovered_target = None;
        let state_update = state_started.elapsed();
        if heap_allocated {
            self.connection_creation_profile
                .record_waypoint_heap_allocation();
        }

        let buffer_reallocated = self
            .connection_creation_preview
            .as_mut()
            .is_some_and(|preview| {
                preview.ensure_capacity(
                    &self.device,
                    &self.style_layout,
                    creation.committed_points.len().saturating_add(1),
                )
            });
        if buffer_reallocated {
            self.connection_creation_profile
                .record_waypoint_gpu_buffer_allocation();
            self.connection_creation_profile
                .record_waypoint_heap_allocation();
        }
        let timing = self
            .connection_creation_preview
            .as_mut()
            .map(|preview| preview.freeze_waypoint(&self.queue))
            .unwrap_or(PreviewUpdateTiming {
                geometry: Duration::ZERO,
                upload: Duration::ZERO,
            });
        self.pointer_interaction = PointerInteraction::CreateDiagramConnection(creation);
        self.waypoint_frame_pending = true;
        self.connection_creation_profile.begin_waypoint_window();
        self.waypoint_profile_frames_remaining = if self.connection_creation_profile.enabled {
            4
        } else {
            0
        };
        self.connection_creation_profile.record_waypoint(
            input_delay,
            state_update,
            timing.geometry,
            timing.upload,
        );
    }

    fn commit_connection_creation(
        &mut self,
        source_port: PortKey,
        source_connector: ConnectorRef,
        target_port: PortKey,
        raw_points: Vec<CorePoint>,
    ) {
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            return;
        };
        if source_port == target_port {
            self.load_error = Some("A connection needs two different ports".to_owned());
            return;
        }
        let Some(target_connector) = self
            .diagram_anchor(&target_port)
            .filter(|anchor| anchor.editable)
            .map(|anchor| anchor.connector_ref.clone())
        else {
            self.load_error = Some("Connection target is no longer available".to_owned());
            return;
        };
        let Some((source_before, version, occurrence)) =
            self.document.as_ref().and_then(|document| {
                let source_before = document.class_text(&class_name)?;
                let scene = document.diagram(&class_name)?;
                let occurrence = scene
                    .connections
                    .iter()
                    .filter(|connection| {
                        connection.lhs == source_connector && connection.rhs == target_connector
                    })
                    .count();
                Some((
                    source_before,
                    document.source_version(&class_name),
                    occurrence,
                ))
            })
        else {
            self.load_error = Some("Connection source document is unavailable".to_owned());
            return;
        };
        let points = canonicalize_orthogonal_points(&raw_points);
        if points.len() < 2
            || !is_orthogonal_polyline(&points)
            || points
                .windows(2)
                .any(|pair| distance_between(pair[0], pair[1]) <= ORTHOGONAL_EPSILON)
        {
            self.load_error = Some("Connection route is not a valid orthogonal path".to_owned());
            return;
        }
        let edit = match new_connection_source_edit(
            &source_before,
            &source_connector,
            &target_connector,
            &points,
        ) {
            Ok(edit) => edit,
            Err(error) => {
                self.load_error = Some(format!("Connection creation rejected: {error}"));
                return;
            }
        };
        let candidate = match apply_validated_source_edits(&source_before, vec![edit], version) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.load_error = Some(format!("Connection creation rejected: {error}"));
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
                    Some("Connection creation could not resolve candidate source".into());
                return;
            }
        };
        let Some(connection) = resolved_diagram.connections.iter().find(|connection| {
            connection.key.occurrence == occurrence
                && connection.lhs == source_connector
                && connection.rhs == target_connector
        }) else {
            self.load_error = Some("Connection creation lost its new connection identity".into());
            return;
        };
        if let Some(reason) = connection_invariant_failure(&resolved_diagram, connection, &points) {
            self.load_error = Some(format!("Connection creation failed: {reason}"));
            return;
        }
        let connection_id = connection.id.clone();
        let connection_key = connection.key.clone();
        let Some(document) = self.document.as_mut() else {
            return;
        };
        document.set_class_text(&class_name, candidate.clone());
        if let Some(scene) = document.icon_mut(&class_name) {
            *scene = resolved_icon;
        }
        if let Some(scene) = document.diagram_mut(&class_name) {
            *scene = resolved_diagram;
        }
        self.history.push(EditCommand::CreateDiagramConnection {
            class_name,
            connection_key,
            before_source: source_before,
            after_source: candidate,
        });
        self.redo_history.clear();
        self.load_error = None;
        self.rebuild_selected_scenes();
        self.set_diagram_selection(DiagramSelection::Connection(connection_id));
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
        let mut reanchored_scene = current_scene.clone();
        let Some(component) = reanchored_scene
            .components
            .iter_mut()
            .find(|component| component.id == component_id)
        else {
            self.load_error = Some("Diagram edit failed: component origin mismatch".into());
            self.rebuild_selected_scenes();
            return;
        };
        component.origin = after_origin;
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
            let Some(connection) = reanchored_scene
                .connections
                .iter()
                .find(|connection| connection.key == snapshot.connection_key)
            else {
                self.load_error = Some("Diagram edit lost a connection".into());
                self.rebuild_selected_scenes();
                return;
            };
            let after_points = match reanchor_connection_points(
                &reanchored_scene,
                connection,
                &snapshot.original_line_points,
            ) {
                Ok(points) => points,
                Err(error) => {
                    self.load_error = Some(format!(
                        "Diagram edit could not resolve connector: {error:?}"
                    ));
                    self.rebuild_selected_scenes();
                    return;
                }
            };
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
        let Some(resolved_component) = resolved_diagram
            .components
            .iter()
            .find(|component| component.id == component_id)
        else {
            self.load_error = Some("Diagram edit failed: component origin mismatch".into());
            self.rebuild_selected_scenes();
            return;
        };
        if !point_nearly_equal(resolved_component.origin, after_origin) {
            self.load_error = Some("Diagram edit failed: component origin mismatch".into());
            self.rebuild_selected_scenes();
            return;
        }
        let canonical_after_origin = resolved_component.origin;
        for edit in &mut connection_edits {
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
                .is_none_or(|line| !point_nearly_equal(line.origin, edit.line_origin))
            {
                self.load_error = Some("Diagram edit failed: connection points mismatch".into());
                self.rebuild_selected_scenes();
                return;
            }
            if let Some(reason) =
                connection_invariant_failure(&resolved_diagram, connection, &edit.after_points)
            {
                self.load_error = Some(format!("Diagram edit failed: {reason}"));
                self.rebuild_selected_scenes();
                return;
            }
            let line = connection
                .line
                .as_ref()
                .expect("connection invariant check requires a line");
            edit.after_points = line.points.clone();
            edit.line_origin = line.origin;
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
            after_origin: canonical_after_origin,
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
            return;
        }
        let mut profile = EditCommitProfile::new();
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
        let Some(cache_connection_index) = current_scene
            .connections
            .iter()
            .position(|connection| connection.key == connection_key)
        else {
            self.load_error = Some("Connection edit lost its connection identity".into());
            self.rebuild_selected_scenes();
            return;
        };
        let Some(connection) = current_scene.connections.get(cache_connection_index) else {
            self.load_error = Some("Connection edit lost its connection identity".into());
            self.rebuild_selected_scenes();
            return;
        };
        let after_points = match finalize_connection_route(current_scene, connection, &after_points)
        {
            Ok(points) => points,
            Err(error) => {
                self.load_error = Some(format!("Connection edit rejected: {error}"));
                self.rebuild_selected_scenes();
                return;
            }
        };
        let patch_started = Instant::now();
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
        if profile.enabled {
            profile.source_patch = patch_started.elapsed();
        }
        let resolve_started = Instant::now();
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
        if profile.enabled {
            profile.resolve_candidate = resolve_started.elapsed();
        }
        let validation_started = Instant::now();
        let Some(resolved_connection_index) = resolved_diagram
            .connections
            .iter()
            .position(|connection| connection.key == connection_key)
        else {
            self.load_error = Some("Connection edit lost its connection identity".into());
            self.rebuild_selected_scenes();
            return;
        };
        let Some(connection) = resolved_diagram.connections.get(resolved_connection_index) else {
            self.load_error = Some("Connection edit lost its connection identity".into());
            self.rebuild_selected_scenes();
            return;
        };
        if let Some(reason) =
            connection_invariant_failure(&resolved_diagram, connection, &after_points)
        {
            self.load_error = Some(format!("Connection edit failed: {reason}"));
            self.rebuild_selected_scenes();
            return;
        }
        let Some(canonical_line) = connection.line.clone() else {
            self.load_error = Some("Connection edit lost its Line annotation".into());
            self.rebuild_selected_scenes();
            return;
        };
        let connection_id = connection.id.clone();
        if profile.enabled {
            profile.semantic_validation = validation_started.elapsed();
        }
        let document_started = Instant::now();
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
        if profile.enabled {
            profile.document_update = document_started.elapsed();
        }
        let gpu_started = Instant::now();
        self.diagram_scene.commit_connection_points(
            &self.device,
            &self.queue,
            &connection_id,
            &after_points,
        );
        if profile.enabled {
            profile.gpu_update = gpu_started.elapsed();
        }
        let hit_index_started = Instant::now();
        self.diagram_hit_cache
            .update_connection(cache_connection_index, &canonical_line);
        if profile.enabled {
            profile.hit_index_update = hit_index_started.elapsed();
        }
        self.history.push(EditCommand::MoveDiagramConnection {
            class_name,
            connection_key,
            before_points,
            after_points,
        });
        self.redo_history.clear();
        self.load_error = None;
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_diagram_component_resize(
        &mut self,
        component_id: String,
        component_name: String,
        _original_component: CoreComponentInstance,
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
        let mut reanchored_scene = current_scene.clone();
        if let Some(component) = reanchored_scene
            .components
            .iter_mut()
            .find(|component| component.id == component_id)
        {
            component.placement_extent = Some(after_extent);
        }
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
            let Some(connection) = reanchored_scene
                .connections
                .iter()
                .find(|connection| connection.key == snapshot.connection_key)
            else {
                self.load_error = Some("Component resize lost a connection".into());
                self.rebuild_selected_scenes();
                return;
            };
            let after_points = match reanchor_connection_points(
                &reanchored_scene,
                connection,
                &snapshot.original_line_points,
            ) {
                Ok(points) => points,
                Err(error) => {
                    self.load_error = Some(format!(
                        "Component resize could not resolve connector: {error:?}"
                    ));
                    self.rebuild_selected_scenes();
                    return;
                }
            };
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
                .is_none_or(|line| !point_nearly_equal(line.origin, edit.line_origin))
            {
                self.load_error =
                    Some("Component resize failed: connection points mismatch".into());
                self.rebuild_selected_scenes();
                return;
            }
            if let Some(reason) =
                connection_invariant_failure(&resolved_diagram, connection, &edit.after_points)
            {
                self.load_error = Some(format!("Component resize failed: {reason}"));
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
                    component.id == *component_id
                        && point_nearly_equal(component.origin, expected_origin)
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
                    if !point_nearly_equal(line.origin, edit.line_origin)
                        || !connection_points_match_invariants(
                            &resolved_diagram,
                            connection,
                            expected_points,
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
            EditCommand::CreateDiagramConnection {
                class_name,
                connection_key,
                before_source,
                after_source,
            } => {
                let source = if after { after_source } else { before_source };
                let Some((resolved_icon, resolved_diagram)) =
                    self.document.as_ref().and_then(|document| {
                        document.resolve_candidate_scenes(class_name, source).ok()
                    })
                else {
                    return;
                };
                let has_connection = resolved_diagram
                    .connections
                    .iter()
                    .any(|connection| connection.key == *connection_key);
                if has_connection != after {
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
                    if !point_nearly_equal(line.origin, edit.line_origin)
                        || !connection_points_match_invariants(
                            &resolved_diagram,
                            connection,
                            expected_points,
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

    fn persist_edits(&mut self) -> Result<usize, String> {
        match self.document.as_mut() {
            Some(document) => save_edited_classes(document),
            None => Err("no Modelica document is open".to_owned()),
        }
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

    /// Install a freshly parsed document into the viewer state and reset all
    /// per-class editing/selection state for the new library.
    fn begin_document_load(&mut self, path: PathBuf) {
        if self.loading_document.is_some() {
            return;
        }
        eprintln!(
            "modelica-wgpu: loading document in background: {}",
            path.display()
        );
        self.load_error = None;
        self.loading_document = Some(std::thread::spawn(move || LoadedDocument::load(&path)));
        self.window.request_redraw();
    }

    fn poll_document_load(&mut self) {
        let Some(handle) = self.loading_document.as_ref() else {
            return;
        };
        if !handle.is_finished() {
            self.window.request_redraw();
            return;
        }
        let handle = self
            .loading_document
            .take()
            .expect("document load handle still present");
        match handle.join() {
            Ok(Ok(document)) => self.adopt_loaded_document(document),
            Ok(Err(error)) => self.load_error = Some(error),
            Err(_) => self.load_error = Some("Modelica document loading thread panicked".into()),
        }
        self.window.request_redraw();
    }

    /// Install a freshly parsed document into the viewer state and reset all
    /// per-class editing/selection state for the new library.
    fn adopt_loaded_document(&mut self, document: LoadedDocument) {
        self.scene = build_scene(&self.device, &self.style_layout, Some(&document), None);
        self.diagram_scene =
            build_diagram_scene(&self.device, &self.style_layout, Some(&document), None);
        self.document = Some(document);
        self.selected_class = None;
        self.refresh_ui_document();
        self.expanded_nodes.clear();
        if let Some(doc) = self.document.as_ref() {
            expand_top_level(
                &mut self.expanded_nodes,
                &doc.package_name,
                &doc.class_names,
            );
        }
        self.canvas_rect = None;
        self.pointer_interaction = PointerInteraction::None;
        self.connection_creation_preview = None;
        self.pending_waypoint = false;
        self.pending_waypoint_queued_at = None;
        self.waypoint_frame_pending = false;
        self.waypoint_profile_frames_remaining = 0;
        self.suppress_next_creation_left_release_redraw = false;
        self.diagram_selection = DiagramSelection::None;
        self.hovered_port = None;
        self.diagram_hit_cache = DiagramHitCache::default();
        self.load_error = None;
        self.update_title(None);
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        self.poll_document_load();
        let document_loading = self.loading_document.is_some();
        let frame_started = Instant::now();
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let is_dark = self.theme_mode.is_dark(self.window.theme());
        if is_dark != self.background_is_dark {
            self.background_dirty = true;
            self.background_is_dark = is_dark;
        }
        set_theme(is_dark, self.accent_theme);
        let mut main_view = self.main_view;
        let mut theme_mode = self.theme_mode;
        let mut accent_theme = self.accent_theme;
        let selected_class = self.selected_class.clone();
        let document_summary = self.ui_document.as_ref();
        let mut expanded_nodes = self.expanded_nodes.clone();
        let load_error = self.load_error.clone();
        let mut open_requested = false;
        let mut open_directory_requested = false;
        let mut class_clicked = None;
        let mut fit_requested = false;
        let mut view_changed = false;
        let mut icon_clip_rect = None;
        let mut expand_all_requested = false;
        let mut collapse_all_requested = false;
        let selected_connection_points = self.selected_connection_overlay_points();
        let selected_component_overlay = self.selected_component_overlay();
        let hovered_anchor = if self.connection_creation_active() {
            match &self.pointer_interaction {
                PointerInteraction::CreateDiagramConnection(creation) => creation
                    .hovered_target
                    .and_then(|index| self.diagram_hit_cache.ports.get(index))
                    .filter(|anchor| anchor.editable),
                _ => None,
            }
        } else {
            self.hovered_port
                .as_ref()
                .and_then(|key| self.diagram_anchor(key))
        };
        let selected_anchor = match &self.diagram_selection {
            DiagramSelection::Port(key) => self.diagram_anchor(key),
            _ => None,
        };
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
                document_summary,
                &mut expanded_nodes,
                &mut open_requested,
                &mut open_directory_requested,
                &mut class_clicked,
                &mut fit_requested,
                &mut view_changed,
                &mut icon_clip_rect,
                &mut expand_all_requested,
                &mut collapse_all_requested,
                load_error.as_deref(),
                document_loading,
            );
            if main_view == MainView::Diagram {
                draw_diagram_selection_overlay(
                    ctx,
                    icon_clip_rect,
                    selected_connection_points.as_deref(),
                    selected_component_overlay,
                    hovered_anchor,
                    selected_anchor,
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
            let is_dark = self.theme_mode.is_dark(self.window.theme());
            if is_dark != self.background_is_dark {
                self.background_dirty = true;
                self.background_is_dark = is_dark;
            }
            set_theme(is_dark, self.accent_theme);
            save_appearance(self.theme_mode, self.accent_theme);
            self.window.request_redraw();
        }
        let previous_main_view = self.main_view;
        self.main_view = main_view;
        self.canvas_rect = icon_clip_rect;
        if previous_main_view != self.main_view {
            self.pointer_interaction = PointerInteraction::None;
            self.connection_preview = None;
            self.connection_creation_preview = None;
            self.diagram_selection = DiagramSelection::None;
            self.hovered_port = None;
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

        if (open_requested || open_directory_requested) && !document_loading {
            let picked = if open_directory_requested {
                FileDialog::new().pick_folder()
            } else {
                FileDialog::new()
                    .add_filter("Modelica", &["mo"])
                    .pick_file()
            };
            if let Some(path) = picked {
                self.begin_document_load(path);
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
            self.refresh_ui_document();
            self.main_view = MainView::Source;
            self.canvas_rect = None;
            self.pointer_interaction = PointerInteraction::None;
            self.connection_preview = None;
            self.connection_creation_preview = None;
            self.diagram_selection = DiagramSelection::None;
            self.hovered_port = None;
            self.diagram_hit_cache =
                build_diagram_hit_cache(self.document.as_ref(), self.selected_class.as_deref());
            self.fit_scene();
            self.update_title(None);
            self.window.request_redraw();
        }

        // Time the expensive stages of this frame so a freeze can be traced to
        // egui/UI work, GPU upload/encode, or file-dialog blocking.
        let ui_done = frame_started.elapsed();

        // The base glass/background belongs below the native GPU scene. If it
        // is emitted by egui instead, it is composited after the scene and
        // makes Modelica colors and strokes look washed out.
        let mut scene_scan = Duration::ZERO;
        let mut egui_tessellation = Duration::ZERO;
        let render_encode_started = Instant::now();
        if self.background_dirty {
            self.queue.write_buffer(
                &self.background_buffer,
                0,
                bytemuck::bytes_of(&background_uniform()),
            );
            self.background_dirty = false;
        }
        let profile_enabled = self.drag_profile.enabled || self.connection_creation_profile.enabled;
        let egui_tessellation_started = profile_enabled.then(Instant::now);
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        if let Some(started) = egui_tessellation_started {
            egui_tessellation = started.elapsed();
        }
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
                    let render_layers: &[DiagramRenderLayer] =
                        if self.main_view == MainView::Diagram {
                            &DIAGRAM_RENDER_LAYERS
                        } else {
                            &ICON_RENDER_LAYERS
                        };
                    let preview_connection_id = self
                        .connection_preview
                        .as_ref()
                        .map(|preview| preview.connection_id.as_str());
                    let scene_scan_started = profile_enabled.then(Instant::now);
                    for layer in render_layers {
                        for &geometry_index in &active_scene.layer_indices[layer.index()] {
                            let geometry = &active_scene.geometries[geometry_index];
                            if geometry.edit_key.as_deref() == preview_connection_id {
                                continue;
                            }
                            pass.set_bind_group(1, &geometry.style_bind_group, &[]);
                            pass.set_vertex_buffer(0, geometry.vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                geometry.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            pass.draw_indexed(0..geometry.index_count, 0, 0..1);
                        }
                    }
                    if let Some(started) = scene_scan_started {
                        scene_scan = started.elapsed();
                    }
                    if let Some(preview) = self.connection_preview.as_ref() {
                        pass.set_bind_group(1, &preview.style_bind_group, &[]);
                        pass.set_vertex_buffer(0, preview.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            preview.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint16,
                        );
                        pass.draw_indexed(0..(preview.active_segment_count * 6) as u32, 0, 0..1);
                    }
                    if let Some(preview) = self.connection_creation_preview.as_ref() {
                        pass.set_bind_group(1, &preview.style_bind_group, &[]);
                        pass.set_vertex_buffer(0, preview.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            preview.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint16,
                        );
                        pass.draw_indexed(0..(preview.active_segment_count * 6) as u32, 0, 0..1);
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
        let render_encode = render_encode_started.elapsed();
        let gpu_uploaded = frame_started.elapsed();
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        frame.present();

        let total = frame_started.elapsed();
        let total_ms = total.as_secs_f64() * 1000.0;
        if self.connection_drag_active() {
            self.drag_profile.record_frame(
                ui_done,
                render_encode,
                total.saturating_sub(gpu_uploaded),
                total,
                scene_scan,
                egui_tessellation,
            );
        }
        if self.connection_creation_active() {
            self.connection_creation_profile.record_frame(
                ui_done,
                render_encode,
                total.saturating_sub(gpu_uploaded),
                total,
                scene_scan,
            );
        }
        if self.waypoint_profile_frames_remaining > 0 {
            self.connection_creation_profile
                .record_waypoint_window_frame(total);
            self.waypoint_profile_frames_remaining -= 1;
        }
        if self.waypoint_frame_pending {
            self.connection_creation_profile.record_waypoint_frame(
                render_encode,
                total.saturating_sub(gpu_uploaded),
                total,
            );
            self.waypoint_frame_pending = false;
        }
        if total_ms > 120.0 {
            let ui_ms = ui_done.as_secs_f64() * 1000.0;
            let gpu_ms = (gpu_uploaded - ui_done).as_secs_f64() * 1000.0;
            let present_ms = total_ms - ui_ms - gpu_ms;
            static LAST_SLOW_LOG: AtomicU64 = AtomicU64::new(0);
            let bucket = (frame_started.elapsed().as_millis() as u64) / 500;
            let previous = LAST_SLOW_LOG.swap(bucket, Ordering::Relaxed);
            if previous != bucket {
                eprintln!(
                    "[SLOW FRAME] total {total_ms:.1} ms | egui/ui {ui_ms:.1} ms | upload/encode {gpu_ms:.1} ms | present/wait {present_ms:.1} ms"
                );
            }
        }

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

// These tokens intentionally mirror src/renderer/src/styles.css so Electron
// and WGPU retain the same visual hierarchy on Windows and Linux.
fn theme_surface() -> Color32 {
    if is_dark_theme() {
        theme_rgb(34, 36, 43) // --surface: #22242b
    } else {
        theme_rgb(255, 255, 255) // --surface: #ffffff
    }
}

fn theme_surface_soft(alpha: u8) -> Color32 {
    if is_dark_theme() {
        theme_rgba(31, 33, 40, alpha) // --surface-soft: #1f2128
    } else {
        theme_rgba(248, 249, 252, alpha) // --surface-soft: #f8f9fc
    }
}

fn theme_surface_raised(alpha: u8) -> Color32 {
    if is_dark_theme() {
        theme_rgba(37, 39, 47, alpha) // --surface-raised: #25272f
    } else {
        theme_rgba(255, 255, 255, alpha)
    }
}

fn theme_text_primary() -> Color32 {
    if is_dark_theme() {
        theme_rgb(241, 241, 244) // --text-primary: #f1f1f4
    } else {
        theme_rgb(32, 33, 40) // --text-primary: #202128
    }
}

// Light-theme UI text tokens: primary #20232B, secondary #505664,
// muted #737A89, disabled #A1A6B2. These colors apply to UI chrome only.
fn theme_text_secondary() -> Color32 {
    if is_dark_theme() {
        theme_rgb(169, 171, 182) // --text-secondary: #a9abb6
    } else {
        theme_rgb(112, 114, 125) // --text-secondary: #70727d
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
        theme_rgb(126, 128, 140) // --text-tertiary: #7e808c
    } else {
        theme_rgb(151, 153, 164) // --text-tertiary: #9799a4
    }
}

fn theme_border(alpha: u8) -> Color32 {
    if is_dark_theme() {
        // --border-subtle: rgb(255 255 255 / 0.085)
        theme_rgba(255, 255, 255, ((alpha as f32) * 0.56).round() as u8)
    } else {
        // --border-subtle: rgb(32 33 40 / 0.09)
        theme_rgba(32, 33, 40, ((alpha as f32) * 0.60).round() as u8)
    }
}

fn theme_accent() -> Color32 {
    let [red, green, blue]: [u8; 3] = match active_accent() {
        AccentTheme::Violet => [108, 92, 231],
        AccentTheme::Blue => [49, 57, 251],
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
        AccentTheme::Blue => [0, 3, 84],
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
    // Electron's body background is #f3f4f8 with very soft accent washes;
    // keep the GPU backdrop restrained so it does not tint the canvas content.
    let base = if is_dark_theme() {
        [0.090, 0.094, 0.114] // --app-background: #17181d, slightly lifted for canvas
    } else {
        [0.953, 0.957, 0.973] // --app-background: #f3f4f8
    };
    let strength = if is_dark_theme() { 0.12 } else { 0.025 };
    let color = |factor: f32| {
        [
            base[0] * (1.0 - strength * factor),
            base[1] * (1.0 - strength * factor),
            base[2] * (1.0 - strength * factor),
            1.0,
        ]
    };
    BackgroundUniform {
        top_left: color(0.0),
        top_right: color(0.25),
        bottom_left: color(0.75),
        bottom_right: color(0.35),
    }
}

fn theme_live() -> Color32 {
    if is_dark_theme() {
        theme_rgb(85, 207, 163)
    } else {
        theme_rgb(54, 174, 124)
    }
}

fn configure_egui_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(7.0, 7.0);
    style.spacing.window_margin = Margin::same(14.0);
    style.spacing.menu_margin = Margin::same(8.0);
    style.spacing.button_padding = Vec2::new(11.0, 8.0);
    style.spacing.interact_size = Vec2::new(40.0, 30.0);
    style.spacing.indent = 14.0;
    style
        .text_styles
        .insert(egui::TextStyle::Small, ui_font(10.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, ui_font(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, ui_font(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, ui_semibold_font(18.0));
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        FontId::new(12.0, FontFamily::Name(UI_FONT_MONO.into())),
    );

    let mut visuals = if is_dark_theme() {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.override_text_color = Some(theme_text_primary());
    visuals.window_rounding = Rounding::same(16.0);
    visuals.menu_rounding = Rounding::same(12.0);
    visuals.window_fill = theme_surface_raised(245);
    visuals.window_stroke = Stroke::new(1.0_f32, theme_border(28));
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.faint_bg_color = theme_surface_soft(72);
    visuals.extreme_bg_color = theme_surface();
    visuals.code_bg_color = theme_surface_soft(170);
    visuals.warn_fg_color = theme_rgb(154, 116, 31);
    visuals.error_fg_color = theme_rgb(193, 72, 90);
    visuals.selection = egui::style::Selection {
        bg_fill: theme_accent_soft(26),
        stroke: Stroke::new(1.0_f32, theme_accent()),
    };

    let border = Stroke::new(1.0_f32, theme_border(26));
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.bg_stroke = border;
        widget.rounding = Rounding::same(8.0);
        widget.expansion = 0.0;
    }
    visuals.widgets.noninteractive.bg_fill = theme_surface_soft(210);
    visuals.widgets.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, theme_text_primary());
    visuals.widgets.inactive.bg_fill = theme_surface();
    visuals.widgets.inactive.weak_bg_fill = theme_surface_soft(185);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, theme_text_secondary());
    visuals.widgets.hovered.bg_fill = theme_accent_soft(20);
    visuals.widgets.hovered.weak_bg_fill = theme_accent_soft(16);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, theme_accent_soft(90));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, theme_accent());
    visuals.widgets.active.bg_fill = theme_accent();
    visuals.widgets.active.weak_bg_fill = theme_accent_soft(110);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, theme_accent());
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.open = visuals.widgets.hovered;
    style.visuals = visuals;
    ctx.set_style(style);
}

#[allow(clippy::too_many_arguments)]
fn draw_preview_ui(
    ctx: &egui::Context,
    main_view: &mut MainView,
    theme_mode: &mut ThemeMode,
    accent_theme: &mut AccentTheme,
    selected_class: Option<&str>,
    document: Option<&UiDocument>,
    expanded_nodes: &mut HashSet<String>,
    open_requested: &mut bool,
    open_directory_requested: &mut bool,
    class_clicked: &mut Option<String>,
    fit_requested: &mut bool,
    view_changed: &mut bool,
    icon_clip_rect: &mut Option<egui::Rect>,
    expand_all_requested: &mut bool,
    collapse_all_requested: &mut bool,
    load_error: Option<&str>,
    document_loading: bool,
) {
    // Keep the global Electron-aligned spacing and widget treatment intact;
    // only update the palette when the user changes light/dark or accent.
    let mut visuals = ctx.style().visuals.clone();
    visuals.override_text_color = Some(theme_text_primary());
    visuals.window_fill = theme_surface_raised(245);
    visuals.window_stroke = Stroke::new(1.0_f32, theme_border(28));
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.faint_bg_color = theme_surface_soft(72);
    visuals.extreme_bg_color = theme_surface();
    visuals.code_bg_color = theme_surface_soft(170);
    visuals.selection = egui::style::Selection {
        bg_fill: theme_accent_soft(26),
        stroke: Stroke::new(1.0_f32, theme_accent()),
    };
    visuals.widgets.hovered.bg_fill = theme_accent_soft(20);
    visuals.widgets.hovered.weak_bg_fill = theme_accent_soft(16);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, theme_accent_soft(90));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, theme_accent());
    visuals.widgets.active.bg_fill = theme_accent();
    visuals.widgets.active.weak_bg_fill = theme_accent_soft(110);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, theme_accent());
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.open = visuals.widgets.hovered;
    ctx.set_visuals(visuals);
    egui::TopBottomPanel::top("arc_topbar")
        .exact_height(52.0)
        .frame(
            Frame::none()
                .fill(theme_surface())
                .stroke(Stroke::new(1.0_f32, theme_border(28)))
                .inner_margin(Margin::symmetric(14.0, 0.0)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Brand mark: an accent-tinted rounded square with the wave
                // glyph, mirroring the Electron .brand-mark.
                let (mark_rect, _) =
                    ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(mark_rect, Rounding::same(11.0), theme_accent_soft(26));
                ui.painter().rect_stroke(
                    mark_rect,
                    Rounding::same(11.0),
                    Stroke::new(1.0_f32, theme_accent_soft(80)),
                );
                ui.painter().text(
                    mark_rect.center(),
                    Align2::CENTER_CENTER,
                    "∿",
                    FontId::proportional(18.0),
                    theme_accent(),
                );
                ui.add_space(9.0);
                ui.label(
                    RichText::new("Modelica Viewer")
                        .size(13.0)
                        .strong()
                        .color(theme_text_primary()),
                );
                if let Some(document) = document {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(format!(
                            "{}  ·  {} classes",
                            document.package_name,
                            document.class_names.len()
                        ))
                        .size(10.0)
                        .font(ui_mono_font(10.0))
                        .color(theme_text_tertiary()),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.menu_button(
                        RichText::new("◐  Appearance  ▾")
                            .size(12.0)
                            .font(ui_font(13.0))
                            .color(theme_text_secondary()),
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
                    let open_library = egui::Button::new(
                        RichText::new("打开库目录")
                            .size(12.0)
                            .font(ui_font(12.0))
                            .color(theme_text_secondary()),
                    )
                    .fill(theme_surface_soft(210))
                    .stroke(Stroke::new(1.0_f32, theme_border(26)))
                    .rounding(Rounding::same(8.0));
                    if ui.add_enabled(!document_loading, open_library).clicked() {
                        *open_directory_requested = true;
                    }
                    let open_file = egui::Button::new(
                        RichText::new("打开 .mo 文件")
                            .size(12.0)
                            .font(ui_semibold_font(12.0))
                            .color(Color32::WHITE),
                    )
                    .fill(theme_accent())
                    .stroke(Stroke::new(1.0_f32, theme_accent()))
                    .rounding(Rounding::same(8.0));
                    if ui.add_enabled(!document_loading, open_file).clicked() {
                        *open_requested = true;
                    }
                });
            });
        });

    egui::SidePanel::left("library_panel")
        .resizable(true)
        .default_width(278.0)
        .min_width(238.0)
        .max_width(320.0)
        .frame(
            Frame::none()
                .fill(theme_surface_soft(255))
                .stroke(Stroke::new(1.0_f32, theme_border(20)))
                .inner_margin(Margin::symmetric(13.0, 12.0)),
        )
        .show(ctx, |ui| {
            ui.add_space(18.0);
            ui.label(
                RichText::new("LIBRARY")
                    .size(10.0)
                    .font(ui_semibold_font(10.0))
                    .strong()
                    .color(theme_accent()),
            );
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(document.map_or("Modelica", |doc| doc.package_name.as_str()))
                        .size(18.0)
                        .font(ui_semibold_font(18.0))
                        .strong()
                        .color(theme_text_primary()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let class_count = document.map_or(70, |doc| doc.class_names.len());
                    ui.label(
                        RichText::new(class_count.to_string())
                            .size(11.0)
                            .font(ui_font(12.0))
                            .color(theme_text_tertiary()),
                    );
                });
            });
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(10.0);
            if let Some(document) = document {
                ui.label(
                    RichText::new("BROWSE CLASSES")
                        .size(10.0)
                        .font(ui_semibold_font(10.0))
                        .color(theme_text_tertiary()),
                );
                ui.add_space(7.0);
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
            if document_loading {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("正在加载 Modelica 库…")
                        .size(10.0)
                        .color(theme_accent()),
                );
            } else if let Some(error) = load_error {
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
                // Keep the GPU canvas colors untouched. A translucent panel
                // here is drawn after wgpu and washes out every icon and
                // Diagram line; the inner glass card supplies the border.
                .fill(Color32::TRANSPARENT)
                .inner_margin(Margin::symmetric(16.0, 12.0)),
        )
        .show(ctx, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let package_name = document.map_or("Modelica", |doc| doc.package_name.as_str());
                let title = selected_class
                    .and_then(|class_name| class_name.rsplit('.').next())
                    .unwrap_or(package_name);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("MODEL / PACKAGE")
                            .size(10.0)
                            .font(ui_semibold_font(10.0))
                            .color(theme_accent()),
                    );
                    ui.label(
                        RichText::new(title)
                            .size(28.0)
                            .font(ui_semibold_font(28.0))
                            .strong()
                            .color(theme_text_primary()),
                    );
                    ui.label(
                        RichText::new(if let Some(class_name) = selected_class {
                            format!("{}  /  {}", package_name, class_name)
                        } else {
                            package_name.to_owned()
                        })
                        .size(11.0)
                        .font(ui_font(11.0))
                        .color(theme_text_tertiary()),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(match document {
                            Some(doc) => format!("READY  ·  {} CLASSES", doc.class_names.len()),
                            None => "GPU CANVAS READY".to_owned(),
                        })
                        .size(9.0)
                        .font(ui_font(12.0))
                        .color(theme_live()),
                    );
                });
            });
            ui.add_space(18.0);
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
                            theme_accent_soft(26)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::new(
                            1.0_f32,
                            if selected {
                                theme_accent_soft(70)
                            } else {
                                Color32::TRANSPARENT
                            },
                        ))
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
                            .stroke(Stroke::new(1.0_f32, theme_border(34)))
                            .rounding(Rounding::same(4.0)),
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
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::same(10.0))
}

fn tree_row(
    ui: &mut egui::Ui,
    marker: &str,
    label: &str,
    selected: bool,
    indent: f32,
) -> egui::Response {
    let row_width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(row_width, 29.0), Sense::click());
    let fill = if selected {
        theme_accent_soft(28)
    } else if response.hovered() {
        theme_surface_raised(80)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, Rounding::same(4.0), fill);
    ui.painter().text(
        Pos2::new(rect.left() + indent * 12.0 + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        format!("{marker}  {label}"),
        if selected {
            ui_semibold_font(13.0)
        } else {
            ui_font(13.0)
        },
        if selected {
            theme_accent()
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

fn expand_top_level(expanded: &mut HashSet<String>, package_name: &str, class_names: &[String]) {
    let root = build_tree(package_name, class_names);
    if !root.children.is_empty() {
        expanded.insert(root.qualified_name.clone());
    }
    for child in &root.children {
        if !child.children.is_empty() {
            expanded.insert(child.qualified_name.clone());
        }
    }
}

fn source_preview(ui: &mut egui::Ui, document: Option<&UiDocument>) {
    let frame = Frame::none()
        .fill(theme_surface())
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::same(16.0));
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
    let font_id = ui_mono_font(12.0);
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

#[allow(clippy::too_many_arguments)]
fn draw_diagram_selection_overlay(
    ctx: &egui::Context,
    canvas_rect: Option<egui::Rect>,
    points: Option<&[CorePoint]>,
    component: Option<ComponentSelectionOverlay>,
    hovered_anchor: Option<&ConnectorAnchor>,
    selected_anchor: Option<&ConnectorAnchor>,
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
    let draw_anchor = |anchor: &ConnectorAnchor, selected: bool| {
        let center = to_screen(anchor.world_position);
        let color = if selected {
            accent
        } else {
            theme_accent_soft(190)
        };
        if let Some(bounds) = anchor.visual_bounds {
            let corners = [
                to_screen(CorePoint {
                    x: bounds.x,
                    y: bounds.y,
                }),
                to_screen(CorePoint {
                    x: bounds.x + bounds.width,
                    y: bounds.y,
                }),
                to_screen(CorePoint {
                    x: bounds.x + bounds.width,
                    y: bounds.y + bounds.height,
                }),
                to_screen(CorePoint {
                    x: bounds.x,
                    y: bounds.y + bounds.height,
                }),
            ];
            for index in 0..corners.len() {
                painter.line_segment(
                    [corners[index], corners[(index + 1) % corners.len()]],
                    Stroke::new(if selected { 1.5_f32 } else { 1.0_f32 }, color),
                );
            }
        }
        painter.circle_filled(center, if selected { 6.0 } else { 5.0 }, color);
        painter.circle_stroke(
            center,
            if selected { 10.0 } else { 8.0 },
            Stroke::new(1.5_f32, theme_surface()),
        );
        painter.circle_stroke(
            center,
            if selected { 10.0 } else { 8.0 },
            Stroke::new(1.5_f32, color),
        );
    };
    if let Some(anchor) = selected_anchor {
        draw_anchor(anchor, true);
    }
    if let Some(anchor) = hovered_anchor {
        if selected_anchor.is_none_or(|selected| selected.key != anchor.key) {
            draw_anchor(anchor, false);
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
    let diagram =
        document.and_then(|document| selected_class.and_then(|name| document.diagram(name)));
    let geometries = diagram.map(core_diagram_geometry).unwrap_or_default();
    if std::env::var_os("MODELICA_WGPU_DEBUG_DIAGRAM").is_some() {
        if let Some(scene) = diagram {
            log_diagram_geometry_diagnostics(scene, &geometries);
        } else {
            eprintln!("diagram diagnostic: no selected DiagramScene");
        }
    }
    gpu_scene_from_geometries(device, style_layout, geometries, "diagram")
}

fn build_diagram_hit_cache(
    document: Option<&LoadedDocument>,
    selected_class: Option<&str>,
) -> DiagramHitCache {
    let Some(scene) = document
        .and_then(|document| selected_class.and_then(|class_name| document.diagram(class_name)))
    else {
        return DiagramHitCache::default();
    };
    let mut components = Vec::new();
    let mut spatial_index = DiagramSpatialIndex::default();
    for (scene_index, component) in scene.components.iter().enumerate() {
        let extent = component
            .placement_extent
            .unwrap_or_else(default_component_extent);
        let bounds = HitBounds::from_points(component_extent_corners(
            component.origin,
            extent,
            component.rotation,
        ));
        let Some(bounds) = bounds else {
            continue;
        };
        let cache_index = components.len();
        components.push(ComponentHitItem {
            scene_index,
            bounds,
        });
        spatial_index.insert_component(cache_index, bounds);
    }
    let ports = connector_anchors(scene);
    for (port_index, anchor) in ports.iter().enumerate() {
        spatial_index.insert_port(port_index, connector_anchor_bounds(anchor));
    }
    for (connection_index, connection) in scene.connections.iter().enumerate() {
        let Some(line) = connection.line.as_ref() else {
            continue;
        };
        let points = connection_world_points(line, &line.points);
        for (segment_index, pair) in points.windows(2).enumerate() {
            let [start, end] = pair else {
                continue;
            };
            let Some(bounds) = HitBounds::from_points([*start, *end]) else {
                continue;
            };
            spatial_index.insert_connection_segment(
                ConnectionSegmentRef {
                    connection_index,
                    segment_index,
                },
                bounds,
            );
        }
    }
    DiagramHitCache {
        components,
        ports,
        spatial_index,
    }
}

fn connector_anchor_bounds(anchor: &ConnectorAnchor) -> HitBounds {
    let mut points = vec![anchor.world_position];
    if let Some(bounds) = anchor.visual_bounds {
        points.extend([
            CorePoint {
                x: bounds.x,
                y: bounds.y,
            },
            CorePoint {
                x: bounds.x + bounds.width,
                y: bounds.y + bounds.height,
            },
        ]);
    }
    HitBounds::from_points(points).expect("anchor has a world position")
}

fn log_diagram_geometry_diagnostics(scene: &CoreDiagramScene, geometries: &[Geometry]) {
    eprintln!(
        "diagram diagnostic: class={} components={} background={} connections={} gpu_geometries={}",
        scene.class_qualified_name.as_deref().unwrap_or("<unnamed>"),
        scene.components.len(),
        scene.background_graphics.len(),
        scene.connections.len(),
        geometries.len()
    );
    for component in &scene.components {
        let Some(icon) = component.diagram_layer() else {
            eprintln!(
                "diagram diagnostic component={} type={:?} owner={} has no diagram layer (icon_graphics={} diagram_graphics={})",
                component.name, component.resolved_type_qualified_name, component.source_owner
                    , component.resolved_icon.as_deref().map_or(0, |scene| scene.graphics.len())
                    , component.resolved_diagram.as_deref().map_or(0, |scene| scene.graphics.len())
            );
            continue;
        };
        let placement = diagram_placement_transform(icon, component);
        let component_geometries = geometries
            .iter()
            .filter(|geometry| geometry.edit_key.as_deref() == Some(component.id.as_str()))
            .collect::<Vec<_>>();
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        let mut vertices = 0;
        for geometry in &component_geometries {
            vertices += geometry.vertices.len();
            for vertex in &geometry.vertices {
                min[0] = min[0].min(vertex.position[0]);
                min[1] = min[1].min(vertex.position[1]);
                max[0] = max[0].max(vertex.position[0]);
                max[1] = max[1].max(vertex.position[1]);
            }
        }
        let bounds = (vertices > 0).then_some((min, max));
        eprintln!(
            "diagram diagnostic component={} type={:?} class_kind={:?} editable={} owner={} origin=({:.2},{:.2}) rotation={:.2} placement={:?} layer_extent={:?} icon_graphics={} diagram_graphics={} transform=translation({:.2},{:.2}) rotation({:.2}) scale({:.4},{:.4}) gpu_geometries={} vertices={} bounds={bounds:?}",
            component.name,
            component.resolved_type_qualified_name,
            component.class_kind,
            component.editable,
            component.source_owner,
            component.origin.x,
            component.origin.y,
            component.rotation,
            component.placement_extent,
            icon.coordinate_system.extent,
            component.resolved_icon.as_deref().map_or(0, |scene| scene.graphics.len()),
            component
                .resolved_diagram
                .as_deref()
                .map_or(0, |scene| scene.graphics.len()),
            placement.translation.x,
            placement.translation.y,
            placement.rotation,
            placement.scale_x,
            placement.scale_y,
            component_geometries.len(),
            vertices,
        );
    }
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
    let gpu_geometries = geometries
        .into_iter()
        .map(|geometry| {
            let (vertex_capacity, index_capacity) = if geometry.connection.is_some() {
                connection_mesh_buffer_capacity(geometry.vertices.len(), geometry.indices.len())
            } else {
                (geometry.vertices.len(), geometry.indices.len())
            };
            let mut vertex_contents = vec![
                Vertex {
                    position: [0.0; 2],
                    local: [0.0; 2],
                };
                vertex_capacity
            ];
            vertex_contents[..geometry.vertices.len()].copy_from_slice(&geometry.vertices);
            let mut index_contents = vec![0_u16; index_capacity];
            index_contents[..geometry.indices.len()].copy_from_slice(&geometry.indices);
            let index_usage = if geometry.connection.is_some() {
                wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST
            } else {
                wgpu::BufferUsages::INDEX
            };
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label} vertices")),
                contents: bytemuck::cast_slice(&vertex_contents),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label} indices")),
                contents: bytemuck::cast_slice(&index_contents),
                usage: index_usage,
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
                vertex_capacity,
                index_capacity,
                layer: geometry.layer,
                edit_key: geometry.edit_key,
                connection: geometry.connection,
                component: geometry.component,
            }
        })
        .collect::<Vec<_>>();
    let mut layer_indices = std::array::from_fn(|_| Vec::new());
    for (index, geometry) in gpu_geometries.iter().enumerate() {
        layer_indices[geometry.layer.index()].push(index);
    }
    GpuIconScene {
        geometries: gpu_geometries,
        layer_indices,
        bounds,
    }
}

fn connection_mesh_buffer_capacity(vertex_count: usize, index_count: usize) -> (usize, usize) {
    (
        vertex_count.saturating_mul(2).saturating_add(32),
        index_count.saturating_mul(2).saturating_add(48),
    )
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
        .map(|mut geometry| {
            geometry.layer = DiagramRenderLayer::Background;
            geometry
        })
        .collect::<Vec<_>>();
    for connection in &scene.connections {
        if let Some(line) = &connection.line {
            geometries.extend(
                line_geometry(line, diagram_flip)
                    .into_iter()
                    .map(|mut geometry| {
                        geometry.layer = DiagramRenderLayer::Connection;
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
        let Some(icon) = component.diagram_layer() else {
            continue;
        };
        let placement = diagram_placement_transform(icon, component);
        let parent_component_transform = compose_transform(diagram_flip, placement);
        let layer = if matches!(
            component.class_kind,
            Some(ClassKind::Connector | ClassKind::ExpandableConnector)
        ) {
            DiagramRenderLayer::Connector
        } else {
            DiagramRenderLayer::Component
        };
        for resolved in &icon.graphics {
            let graphic_transform = compose_transform(placement, resolved.transform);
            let transform = compose_transform(diagram_flip, graphic_transform);
            geometries.extend(
                core_graphic_geometry_from_graphic(&resolved.graphic, transform)
                    .into_iter()
                    .map(|mut geometry| {
                        geometry.layer = layer;
                        geometry.edit_key = Some(component.id.clone());
                        geometry.component = Some(ComponentGeometry {
                            transform: parent_component_transform,
                        });
                        geometry
                    }),
            );
        }
    }
    // Keep z-order independent from source/component iteration order. The
    // egui selection handles are rendered in a later foreground pass.
    geometries.sort_by_key(|geometry| geometry.layer);
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
        layer: DiagramRenderLayer::Component,
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
        layer: DiagramRenderLayer::Component,
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
    let Some(icon) = component.diagram_layer() else {
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
        let transform = compose_transform(
            diagram_flip,
            compose_transform(placement, resolved.transform),
        );
        resolved_graphic_contains_point_with_transform(resolved, transform, render_point, tolerance)
    })
}

fn connection_world_points(line: &LineGraphic, points: &[CorePoint]) -> Vec<CorePoint> {
    points
        .iter()
        .map(|point| line_local_to_world(line, *point))
        .collect()
}

#[cfg(test)]
fn hit_test_connection(
    connections: &[modelica_core::scene::DiagramConnection],
    pointer: CorePoint,
    tolerance: f32,
) -> Option<ConnectionHit> {
    let mut best: Option<(ConnectionHit, f32, usize, usize)> = None;
    for (connection_index, connection) in connections.iter().enumerate() {
        let line = connection.line.as_ref();
        for segment_index in 0..line.map_or(0, |line| line.points.len().saturating_sub(1)) {
            let Some((hit, distance)) = hit_test_connection_segment_with_distance(
                connection,
                segment_index,
                pointer,
                tolerance,
            ) else {
                continue;
            };
            let best_key = best
                .as_ref()
                .map(|(_, distance, connection_index, segment_index)| {
                    (*distance, false, *connection_index, *segment_index)
                });
            if connection_hit_candidate_is_better(
                distance,
                false,
                connection_index,
                segment_index,
                best_key,
            ) {
                best = Some((hit, distance, connection_index, segment_index));
            }
        }
    }
    best.map(|(hit, _, _, _)| hit)
}

fn hit_test_connection_segment_with_distance(
    connection: &modelica_core::scene::DiagramConnection,
    segment_index: usize,
    pointer: CorePoint,
    tolerance: f32,
) -> Option<(ConnectionHit, f32)> {
    let line = connection.line.as_ref()?;
    let start = line
        .points
        .get(segment_index)
        .map(|point| line_local_to_world(line, *point))?;
    let end = line
        .points
        .get(segment_index + 1)
        .map(|point| line_local_to_world(line, *point))?;
    let distance = distance_to_segment(pointer, start, end);
    if distance > tolerance {
        return None;
    }
    let target = match line
        .points
        .get(segment_index..=segment_index.saturating_add(1))
        .and_then(|pair| pair.first().zip(pair.get(1)))
        .and_then(|(start, end)| segment_orientation(*start, *end))
    {
        Some(orientation) => ConnectionHitTarget::Segment {
            index: segment_index,
            orientation,
        },
        None => ConnectionHitTarget::Line,
    };
    Some((
        ConnectionHit {
            connection_id: connection.id.clone(),
            target,
        },
        distance,
    ))
}

fn connection_hit_candidate_is_better(
    candidate_distance: f32,
    candidate_selected: bool,
    candidate_connection_index: usize,
    candidate_segment_index: usize,
    best: Option<(f32, bool, usize, usize)>,
) -> bool {
    let Some((best_distance, best_selected, best_connection_index, best_segment_index)) = best
    else {
        return true;
    };
    if candidate_distance + CONNECTION_HIT_DISTANCE_TIE_EPSILON < best_distance {
        return true;
    }
    if (candidate_distance - best_distance).abs() > CONNECTION_HIT_DISTANCE_TIE_EPSILON {
        return false;
    }
    if candidate_selected != best_selected {
        return candidate_selected;
    }
    candidate_connection_index > best_connection_index
        || (candidate_connection_index == best_connection_index
            && candidate_segment_index < best_segment_index)
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

const DIAGRAM_GEOMETRY_EPSILON: f32 = 1.0e-4;

fn point_nearly_equal(first: CorePoint, second: CorePoint) -> bool {
    (first.x - second.x).abs() <= DIAGRAM_GEOMETRY_EPSILON
        && (first.y - second.y).abs() <= DIAGRAM_GEOMETRY_EPSILON
}

fn points_nearly_equal(first: &[CorePoint], second: &[CorePoint]) -> bool {
    first.len() == second.len()
        && first
            .iter()
            .zip(second)
            .all(|(first, second)| point_nearly_equal(*first, *second))
}

fn connection_endpoints_match(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
) -> bool {
    let Ok(endpoints) = resolve_connection_endpoints(scene, connection) else {
        return false;
    };
    endpoints.lhs_distance <= DIAGRAM_GEOMETRY_EPSILON
        && endpoints.rhs_distance <= DIAGRAM_GEOMETRY_EPSILON
}

/// Anchor, simplify, and validate a route before it is serialized.
///
/// Re-anchoring is intentionally part of the fixed-point loop: replacing a
/// bridge with its semantic endpoint can create a new duplicate or collinear
/// vertex. Canonicalization therefore never gets to remove an endpoint, and
/// the route is only accepted once both operations are stable.
fn finalize_connection_route(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    raw_points: &[CorePoint],
) -> Result<Vec<CorePoint>, String> {
    if raw_points.len() < 2 {
        return Err("connection route must contain at least two points".to_owned());
    }

    let mut points = raw_points.to_vec();
    loop {
        let anchored = reanchor_connection_points(scene, connection, &points)
            .map_err(|error| format!("unable to resolve connector anchors: {error:?}"))?;
        let canonical = canonicalize_orthogonal_points(&anchored);
        if canonical == points {
            points = canonical;
            break;
        }
        points = canonical;
    }

    if points.len() < 2 {
        return Err("connection route must contain at least two points".to_owned());
    }
    if !is_orthogonal_polyline(&points) {
        return Err("connection route must remain orthogonal".to_owned());
    }
    if points
        .windows(2)
        .any(|pair| distance_between(pair[0], pair[1]) <= ORTHOGONAL_EPSILON)
    {
        return Err("connection route contains a zero-length segment".to_owned());
    }

    let (lhs, rhs) = strict_connection_points(scene, connection)
        .map_err(|error| format!("unable to resolve connector anchors: {error:?}"))?;
    if distance_between(points[0], lhs) > DIAGRAM_GEOMETRY_EPSILON
        || distance_between(*points.last().expect("at least two points"), rhs)
            > DIAGRAM_GEOMETRY_EPSILON
    {
        return Err("connection route endpoints are not anchored".to_owned());
    }
    Ok(points)
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
) -> bool {
    connection_invariant_failure(scene, connection, expected_points).is_none()
}

fn connection_invariant_failure(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    expected_points: &[CorePoint],
) -> Option<&'static str> {
    let Some(line) = connection.line.as_ref() else {
        return Some("connection points mismatch");
    };
    let Some((first, last)) = line.points.first().zip(line.points.last()) else {
        return Some("connection points mismatch");
    };
    if !points_nearly_equal(&line.points, expected_points)
        || line.points.len() < 2
        || distance_between(*first, *last) <= ORTHOGONAL_EPSILON
        || !is_orthogonal_polyline(&line.points)
    {
        return Some("connection points mismatch");
    }
    if !connection_endpoints_match(scene, connection) {
        return Some("endpoint anchor mismatch");
    }
    None
}

fn connection_drag_snapshots(
    scene: &CoreDiagramScene,
    component_name: &str,
) -> Vec<ConnectionDragSnapshot> {
    scene
        .connections
        .iter()
        .filter_map(|connection| {
            if connection.lhs.component_name != component_name
                && connection.rhs.component_name != component_name
            {
                return None;
            }
            let line = connection.line.as_ref()?;
            Some(ConnectionDragSnapshot {
                connection_id: connection.id.clone(),
                connection_key: connection.key.clone(),
                original_line_points: line.points.clone(),
                original_line_origin: line.origin,
            })
        })
        .collect()
}

#[cfg(test)]
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

#[cfg(test)]
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

fn connection_snap_tolerance(zoom: f32) -> f32 {
    CONNECTION_SNAP_ENTER_PIXELS / zoom.max(MIN_ZOOM)
}

fn connection_snap_exit_tolerance(zoom: f32) -> f32 {
    CONNECTION_SNAP_EXIT_PIXELS / zoom.max(MIN_ZOOM)
}

/// Compute the route axes once at drag-start. The original route cannot
/// change while a drag is active, so scanning it per cursor event is wasted.
fn connection_segment_axis(
    original_points: &[CorePoint],
    segment_index: usize,
    orientation: ConnectionSegmentOrientation,
) -> f32 {
    match orientation {
        ConnectionSegmentOrientation::Horizontal => original_points[segment_index].y,
        ConnectionSegmentOrientation::Vertical => original_points[segment_index].x,
    }
}

fn connection_segment_snap_axes(
    original_points: &[CorePoint],
    segment_index: usize,
    orientation: ConnectionSegmentOrientation,
) -> Vec<f32> {
    if original_points.len() < 2 || segment_index + 1 >= original_points.len() {
        return Vec::new();
    }
    let mut run_start = segment_index;
    while run_start > 0
        && segment_orientation(original_points[run_start - 1], original_points[run_start])
            == Some(orientation)
    {
        run_start -= 1;
    }
    let mut run_end = segment_index;
    while run_end + 1 < original_points.len() - 1
        && segment_orientation(original_points[run_end + 1], original_points[run_end + 2])
            == Some(orientation)
    {
        run_end += 1;
    }
    let axis_coordinate = |point: CorePoint| match orientation {
        ConnectionSegmentOrientation::Horizontal => point.y,
        ConnectionSegmentOrientation::Vertical => point.x,
    };
    let mut axes = vec![
        axis_coordinate(original_points[0]),
        axis_coordinate(*original_points.last().expect("at least two points")),
    ];
    axes.extend(
        original_points
            .windows(2)
            .enumerate()
            .filter(|(index, pair)| {
                !(run_start..=run_end).contains(index)
                    && segment_orientation(pair[0], pair[1]) == Some(orientation)
            })
            .map(|(_, pair)| axis_coordinate(pair[0])),
    );
    axes.sort_by(f32::total_cmp);
    axes.dedup_by(|left, right| (*left - *right).abs() <= ORTHOGONAL_EPSILON);
    axes
}

/// Snap from cached axes with screen-space enter/exit hysteresis.
fn snap_connection_segment_delta_cached(
    base_axis: f32,
    orientation: ConnectionSegmentOrientation,
    line_rotation: f32,
    raw_delta: CorePoint,
    axes: &[f32],
    snapped_axis: &mut Option<f32>,
    (enter_tolerance, exit_tolerance): (f32, f32),
) -> CorePoint {
    let local_delta =
        world_delta_to_line_local(CorePoint { x: 0.0, y: 0.0 }, line_rotation, raw_delta);
    let current_axis = match orientation {
        ConnectionSegmentOrientation::Horizontal => base_axis + local_delta.y,
        ConnectionSegmentOrientation::Vertical => base_axis + local_delta.x,
    };
    if let Some(axis) = *snapped_axis {
        if (current_axis - axis).abs() <= exit_tolerance {
            return snapped_connection_delta(
                orientation,
                line_rotation,
                local_delta,
                base_axis,
                axis,
            );
        }
        *snapped_axis = None;
    }
    let Some((_, axis)) = axes
        .iter()
        .copied()
        .map(|axis| ((current_axis - axis).abs(), axis))
        .filter(|(distance, _)| *distance <= enter_tolerance)
        .min_by(|left, right| left.0.total_cmp(&right.0))
    else {
        return raw_delta;
    };
    *snapped_axis = Some(axis);
    snapped_connection_delta(orientation, line_rotation, local_delta, base_axis, axis)
}

fn snapped_connection_delta(
    orientation: ConnectionSegmentOrientation,
    line_rotation: f32,
    local_delta: CorePoint,
    base_axis: f32,
    target_axis: f32,
) -> CorePoint {
    let local_delta = match orientation {
        ConnectionSegmentOrientation::Horizontal => CorePoint {
            x: local_delta.x,
            y: target_axis - base_axis,
        },
        ConnectionSegmentOrientation::Vertical => CorePoint {
            x: target_axis - base_axis,
            y: local_delta.y,
        },
    };
    line_local_delta_to_world(line_rotation, local_delta)
}

/// Snap a segment's normal-axis drag to the nearest parallel axis in this
/// connection. The input and output deltas are in world/model coordinates;
/// candidate coordinates are compared in line-local coordinates so rotated
/// Lines use the same geometry as translation and remain orthogonal.
#[cfg(test)]
fn snap_connection_segment_delta(
    original_points: &[CorePoint],
    segment_index: usize,
    orientation: ConnectionSegmentOrientation,
    line_origin: CorePoint,
    line_rotation: f32,
    raw_delta: CorePoint,
    model_snap_tolerance: f32,
) -> CorePoint {
    if original_points.len() < 2 || segment_index + 1 >= original_points.len() {
        return raw_delta;
    }

    let local_delta = world_delta_to_line_local(line_origin, line_rotation, raw_delta);
    let current_axis = match orientation {
        ConnectionSegmentOrientation::Horizontal => {
            original_points[segment_index].y + local_delta.y
        }
        ConnectionSegmentOrientation::Vertical => original_points[segment_index].x + local_delta.x,
    };

    let mut run_start = segment_index;
    while run_start > 0
        && segment_orientation(original_points[run_start - 1], original_points[run_start])
            == Some(orientation)
    {
        run_start -= 1;
    }
    let mut run_end = segment_index;
    while run_end + 1 < original_points.len() - 1
        && segment_orientation(original_points[run_end + 1], original_points[run_end + 2])
            == Some(orientation)
    {
        run_end += 1;
    }

    let axis_coordinate = |point: CorePoint| match orientation {
        ConnectionSegmentOrientation::Horizontal => point.y,
        ConnectionSegmentOrientation::Vertical => point.x,
    };
    let Some(last_point) = original_points.last().copied() else {
        return raw_delta;
    };
    let mut candidates = vec![
        axis_coordinate(original_points[0]),
        axis_coordinate(last_point),
    ];
    for (index, pair) in original_points.windows(2).enumerate() {
        if (run_start..=run_end).contains(&index) {
            continue;
        }
        if segment_orientation(pair[0], pair[1]) == Some(orientation) {
            candidates.push(axis_coordinate(pair[0]));
        }
    }

    let Some((_, target_axis)) = candidates
        .into_iter()
        .map(|candidate| ((current_axis - candidate).abs(), candidate))
        .filter(|(distance, _)| *distance <= model_snap_tolerance)
        .min_by(|(left, _), (right, _)| left.total_cmp(right))
    else {
        return raw_delta;
    };

    let snapped_local_delta = match orientation {
        ConnectionSegmentOrientation::Horizontal => CorePoint {
            x: local_delta.x,
            y: target_axis - original_points[segment_index].y,
        },
        ConnectionSegmentOrientation::Vertical => CorePoint {
            x: target_axis - original_points[segment_index].x,
            y: local_delta.y,
        },
    };
    line_local_delta_to_world(line_rotation, snapped_local_delta)
}

fn line_local_delta_to_world(line_rotation: f32, delta: CorePoint) -> CorePoint {
    let angle = line_rotation.to_radians();
    let (sin, cos) = angle.sin_cos();
    CorePoint {
        x: delta.x * cos - delta.y * sin,
        y: delta.x * sin + delta.y * cos,
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
    if original_points.len() < 2 || segment_index + 1 >= original_points.len() {
        return original_points.to_vec();
    }
    let local_delta = world_delta_to_line_local(line_origin, line_rotation, delta);
    if local_delta.x.abs() <= ORTHOGONAL_EPSILON && local_delta.y.abs() <= ORTHOGONAL_EPSILON {
        return original_points.to_vec();
    }

    // A segment is translated only on its normal axis: a vertical segment
    // follows horizontal pointer motion, while a horizontal segment follows
    // vertical pointer motion. Treat adjacent collinear pieces as one run;
    // moving only the selected pair would make the next collinear piece
    // diagonal. Endpoint runs receive a perpendicular bridge to keep the
    // semantic port endpoint fixed.
    // Endpoint routing is deliberately orthogonal and source-serializable.
    // Endpoint edits may increase Line.points by adding a bridge.
    let offset = match orientation {
        ConnectionSegmentOrientation::Horizontal => CorePoint {
            x: 0.0,
            y: local_delta.y,
        },
        ConnectionSegmentOrientation::Vertical => CorePoint {
            x: local_delta.x,
            y: 0.0,
        },
    };
    if offset.x.abs() <= ORTHOGONAL_EPSILON && offset.y.abs() <= ORTHOGONAL_EPSILON {
        return original_points.to_vec();
    }

    let mut run_start = segment_index;
    while run_start > 0
        && segment_orientation(original_points[run_start - 1], original_points[run_start])
            == Some(orientation)
    {
        run_start -= 1;
    }
    let mut run_end = segment_index;
    while run_end + 1 < original_points.len() - 1
        && segment_orientation(original_points[run_end + 1], original_points[run_end + 2])
            == Some(orientation)
    {
        run_end += 1;
    }
    let run_last_point = run_end + 1;
    let first_endpoint = run_start == 0;
    let last_endpoint = run_last_point == original_points.len() - 1;

    // A two-point (or fully collinear) route shifts as a whole: both semantic
    // endpoints stay fixed and the two shifted ends are bridged to them.
    if first_endpoint && last_endpoint {
        let lhs = original_points[0];
        let rhs = original_points[original_points.len() - 1];
        return vec![
            lhs,
            CorePoint {
                x: lhs.x + offset.x,
                y: lhs.y + offset.y,
            },
            CorePoint {
                x: rhs.x + offset.x,
                y: rhs.y + offset.y,
            },
            rhs,
        ];
    }

    let mut points = original_points.to_vec();
    for point in &mut points[run_start..=run_last_point] {
        point.x += offset.x;
        point.y += offset.y;
    }

    if first_endpoint {
        let endpoint = original_points[0];
        let shifted_first = points[1];
        points[0] = endpoint;
        let bridge = match orientation {
            ConnectionSegmentOrientation::Horizontal => CorePoint {
                x: endpoint.x,
                y: shifted_first.y,
            },
            ConnectionSegmentOrientation::Vertical => CorePoint {
                x: shifted_first.x,
                y: endpoint.y,
            },
        };
        points.insert(1, bridge);
    }
    if last_endpoint {
        let endpoint_index = points.len() - 1;
        let endpoint = original_points[original_points.len() - 1];
        let shifted_last = points[endpoint_index - 1];
        points[endpoint_index] = endpoint;
        let bridge = match orientation {
            ConnectionSegmentOrientation::Horizontal => CorePoint {
                x: endpoint.x,
                y: shifted_last.y,
            },
            ConnectionSegmentOrientation::Vertical => CorePoint {
                x: shifted_last.x,
                y: endpoint.y,
            },
        };
        points.insert(endpoint_index, bridge);
    }
    points
}

fn connection_drag_points_with_semantic_endpoints(
    original_points: &[CorePoint],
    (lhs, rhs): (CorePoint, CorePoint),
) -> Vec<CorePoint> {
    let mut points = original_points.to_vec();
    if let Some(first) = points.first_mut() {
        *first = lhs;
    }
    if let Some(last) = points.last_mut() {
        *last = rhs;
    }
    points
}

fn build_connection_segment_drag_route(
    original_points: &[CorePoint],
    segment_index: usize,
    orientation: ConnectionSegmentOrientation,
    line_origin: CorePoint,
    line_rotation: f32,
    delta: CorePoint,
    semantic_endpoints: (CorePoint, CorePoint),
) -> Vec<CorePoint> {
    let anchored_points =
        connection_drag_points_with_semantic_endpoints(original_points, semantic_endpoints);
    translated_connection_segment(
        &anchored_points,
        segment_index,
        orientation,
        line_origin,
        line_rotation,
        delta,
    )
}

fn build_connection_corner_drag_route(
    original_points: &[CorePoint],
    corner_index: usize,
    line_origin: CorePoint,
    line_rotation: f32,
    delta: CorePoint,
    semantic_endpoints: (CorePoint, CorePoint),
) -> Vec<CorePoint> {
    let anchored_points =
        connection_drag_points_with_semantic_endpoints(original_points, semantic_endpoints);
    translated_connection_corner(
        &anchored_points,
        corner_index,
        line_origin,
        line_rotation,
        delta,
    )
}

/// Move an inner polyline vertex ("corner") of a connection freely while the
/// rest of the route stays a strictly axis-aligned polyline:
///
/// - the dragged corner follows the pointer on both axes;
/// - an inner neighbour slides along its shared axis so its other edge stays
///   axis-aligned (consecutive edges of a valid route are perpendicular);
/// - a fixed endpoint instead constrains the dragged corner on that axis, so
///   the connection can never detach from a component port.
fn translated_connection_corner(
    original_points: &[CorePoint],
    corner_index: usize,
    line_origin: CorePoint,
    line_rotation: f32,
    delta: CorePoint,
) -> Vec<CorePoint> {
    if original_points.len() < 4 || corner_index == 0 || corner_index + 1 >= original_points.len() {
        return original_points.to_vec();
    }
    let local_delta = world_delta_to_line_local(line_origin, line_rotation, delta);
    let original_corner = original_points[corner_index];
    let proposed = CorePoint {
        x: original_corner.x + local_delta.x,
        y: original_corner.y + local_delta.y,
    };
    let left_original = original_points[corner_index - 1];
    let right_original = original_points[corner_index + 1];
    let left_fixed = corner_index - 1 == 0;
    let right_fixed = corner_index + 2 == original_points.len();
    let horizontal = |a: CorePoint, b: CorePoint| (a.y - b.y).abs() <= ORTHOGONAL_EPSILON;
    let vertical = |a: CorePoint, b: CorePoint| (a.x - b.x).abs() <= ORTHOGONAL_EPSILON;
    let left_horizontal = horizontal(left_original, original_corner);
    let left_vertical = vertical(left_original, original_corner);
    let right_horizontal = horizontal(right_original, original_corner);
    let right_vertical = vertical(right_original, original_corner);

    let mut points = original_points.to_vec();
    let mut corner = proposed;
    if left_fixed && left_horizontal {
        corner.y = left_original.y;
    } else if left_fixed && left_vertical {
        corner.x = left_original.x;
    }
    if right_fixed && right_horizontal {
        corner.y = right_original.y;
    } else if right_fixed && right_vertical {
        corner.x = right_original.x;
    }
    points[corner_index] = corner;
    if !left_fixed && left_horizontal {
        points[corner_index - 1].y = corner.y;
    } else if !left_fixed && left_vertical {
        points[corner_index - 1].x = corner.x;
    }
    if !right_fixed && right_horizontal {
        points[corner_index + 1].y = corner.y;
    } else if !right_fixed && right_vertical {
        points[corner_index + 1].x = corner.x;
    }
    points
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

fn connector_ref_text(reference: &ConnectorRef) -> String {
    if reference.connector_path.is_empty() {
        reference.component_name.clone()
    } else {
        format!("{}.{}", reference.component_name, reference.connector_path)
    }
}

fn new_connection_source_edit(
    source: &str,
    source_connector: &ConnectorRef,
    target_connector: &ConnectorRef,
    points: &[CorePoint],
) -> Result<SourceEdit, String> {
    let insertion = tokenize(source)
        .iter()
        .rev()
        .find(|token| token.kind == TokenKind::Keyword && token.text == "end")
        .map(|token| token.start)
        .ok_or_else(|| "class closing `end` was not found".to_owned())?;
    let line_start = source[..insertion].rfind('\n').map_or(0, |index| index + 1);
    let indent = source
        .get(line_start..insertion)
        .filter(|value| value.bytes().all(|byte| matches!(byte, b' ' | b'\t')))
        .unwrap_or_default();
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let replacement = format!(
        "connect({}, {}) annotation(Line(points={}));{newline}{indent}",
        connector_ref_text(source_connector),
        connector_ref_text(target_connector),
        format_modelica_points(points),
    );
    Ok(SourceEdit {
        start: insertion,
        end: insertion,
        expected_text: Some(String::new()),
        replacement,
    })
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
    if let Some(doc) = app.document.as_ref() {
        expand_top_level(&mut app.expanded_nodes, &doc.package_name, &doc.class_names);
    }
    app.update_title(None);
    window.request_redraw();

    event_loop
        .run(move |event, event_loop| {
            event_loop.set_control_flow(ControlFlow::Wait);
            match event {
                Event::WindowEvent { window_id, event } if window_id == app.window.id() => {
                    let egui_response = app.egui_state.on_window_event(&app.window, &event);
                    let egui_consumed = egui_response.consumed;
                    match event {
                        WindowEvent::CloseRequested => event_loop.exit(),
                        WindowEvent::Resized(size) => {
                            app.resize(size);
                            app.window.request_redraw();
                        }
                        WindowEvent::RedrawRequested => {
                            app.flush_drag_preview();
                            app.process_pending_waypoint();
                            match app.render() {
                            Ok(()) => {}
                            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                                app.resize(app.window.inner_size())
                            }
                            Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                            Err(wgpu::SurfaceError::Timeout) => {}
                            }
                        }
                        WindowEvent::ModifiersChanged(modifiers) => {
                            app.modifiers = modifiers.state();
                        }
                        WindowEvent::ThemeChanged(_) => {
                            app.background_dirty = true;
                            app.window.request_redraw();
                        }
                        WindowEvent::KeyboardInput { event, .. }
                            if event.state == ElementState::Pressed && !event.repeat =>
                        {
                            if !egui_consumed {
                                match event.physical_key {
                                    PhysicalKey::Code(KeyCode::Escape)
                                        if app.connection_creation_active() =>
                                    {
                                        app.cancel_connection_creation();
                                    }
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
                                    PhysicalKey::Code(KeyCode::KeyS)
                                        if app.modifiers.control_key() =>
                                    {
                                        match app.persist_edits() {
                                            Ok(0) => {
                                                eprintln!("modelica-wgpu: nothing to save");
                                            }
                                            Ok(saved) => {
                                                eprintln!(
                                                    "modelica-wgpu: saved {saved} edited file(s) to disk"
                                                );
                                                app.load_error = None;
                                            }
                                            Err(error) => {
                                                app.load_error = Some(error);
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            app.window.request_redraw();
                        }
                        WindowEvent::CursorMoved { position, .. } => {
                            app.cursor = position;
                            if app.connection_drag_active() {
                                // Coalesce high-rate mouse input: one preview update
                                // per redraw, never one GPU update per OS event.
                                app.drag_profile.record_event();
                                if app.connection_creation_active() {
                                    app.connection_creation_profile.record_event();
                                }
                                let request_redraw = app.pending_drag_position.is_none();
                                app.pending_drag_position = Some((position, Instant::now()));
                                if request_redraw {
                                    app.window.request_redraw();
                                }
                            } else {
                                let hover_changed = app.update_hovered_diagram_port();
                                let drag_changed = app.update_model_drag_preview(position);
                                if egui_consumed || hover_changed || drag_changed {
                                    app.window.request_redraw();
                                }
                            }
                        }
                        WindowEvent::MouseInput { state, button, .. } => {
                            // A connection-creation click must not clone the
                            // current selection before entering its hot path.
                            let selection_before = (!app.connection_creation_active())
                                .then(|| app.diagram_selection.clone());
                            let inert_creation_release =
                                if state == ElementState::Released && button == MouseButton::Left {
                                    let suppress =
                                        app.suppress_next_creation_left_release_redraw;
                                    app.suppress_next_creation_left_release_redraw = false;
                                    suppress
                                } else {
                                    false
                                };
                            if state == ElementState::Released {
                                if app.connection_creation_active() && button == MouseButton::Right {
                                    app.cancel_connection_creation();
                                } else if app.connection_creation_active()
                                    && button == MouseButton::Left
                                {
                                    // Waypoints are committed on press. The
                                    // matching release is intentionally inert.
                                } else {
                                    app.finish_model_drag(button);
                                }
                            } else if app.connection_creation_active() {
                                if button == MouseButton::Left {
                                    // Keep input handling tiny. The latest
                                    // cursor/snap state and preview metadata are
                                    // consumed together by RedrawRequested.
                                    app.pending_waypoint = true;
                                    app.pending_waypoint_queued_at = Some(Instant::now());
                                    app.window.request_redraw();
                                }
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
                            if state == ElementState::Pressed
                                && button == MouseButton::Left
                                && app.connection_creation_active()
                            {
                                app.suppress_next_creation_left_release_redraw = true;
                            }
                            let selection_changed = selection_before
                                .is_some_and(|selection_before| app.diagram_selection != selection_before);
                            if egui_consumed
                                || (state == ElementState::Released && !inert_creation_release)
                                || selection_changed
                            {
                                app.window.request_redraw();
                            }
                        }
                        WindowEvent::MouseWheel { delta, .. }
                            if should_zoom_canvas(
                                app.main_view,
                                app.pointer_over_canvas(),
                                app.modifiers.control_key(),
                            ) => {
                                let amount = match delta {
                                    MouseScrollDelta::LineDelta(_, y) => y,
                                    MouseScrollDelta::PixelDelta(position) => {
                                        position.y as f32 / 80.0
                                    }
                                };
                                app.zoom_at_cursor(amount);
                                app.window.request_redraw();
                            }
                        _ => {}
                    }
                }
                Event::AboutToWait => {}
                _ => {}
            }
        })
        .expect("event loop failed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelica_core::scene::{ConnectorRef, DiagramConnection, GraphicOwnerKind};

    fn connection_test_scene(
        lhs_position: CorePoint,
        rhs_position: CorePoint,
        points: Vec<CorePoint>,
    ) -> (CoreDiagramScene, DiagramConnection) {
        let component = |id: &str, name: &str, origin: CorePoint| CoreComponentInstance {
            id: id.to_owned(),
            name: name.to_owned(),
            source_owner: "Test".to_owned(),
            type_name: "Port".to_owned(),
            resolved_type_qualified_name: Some("Test.Port".to_owned()),
            class_kind: Some(ClassKind::Connector),
            origin,
            rotation: 0.0,
            placement_extent: Some(modelica_core::scene::Extent {
                p1: CorePoint {
                    x: -100.0,
                    y: -100.0,
                },
                p2: CorePoint { x: 100.0, y: 100.0 },
            }),
            visible: true,
            editable: true,
            resolved_icon: Some(Box::new(CoreIconScene {
                owner_qualified_name: Some("Test.Port".to_owned()),
                coordinate_system: modelica_core::scene::CoordinateSystem::default(),
                graphics: Vec::new(),
                diagnostics: Vec::new(),
            })),
            resolved_diagram: None,
        };
        let lhs = ConnectorRef {
            component_name: "a".to_owned(),
            connector_path: String::new(),
        };
        let rhs = ConnectorRef {
            component_name: "b".to_owned(),
            connector_path: String::new(),
        };
        let connection = DiagramConnection {
            key: ConnectionKey::new("Test", lhs.clone(), rhs.clone(), 0),
            id: "connection:test".to_owned(),
            lhs,
            rhs,
            from: "a".to_owned(),
            to: "b".to_owned(),
            line: Some(LineGraphic {
                origin: CorePoint { x: 0.0, y: 0.0 },
                rotation: 0.0,
                points,
                color: [0, 0, 0],
                pattern: None,
                thickness: 1.0,
                arrow: Vec::new(),
                arrow_size: None,
                smooth: None,
            }),
            source_range: None,
            line_source_range: None,
        };
        let scene = CoreDiagramScene {
            class_qualified_name: Some("Test".to_owned()),
            class_kind: Some(ClassKind::Model),
            coordinate_system: modelica_core::scene::CoordinateSystem::default(),
            background_graphics: Vec::new(),
            components: vec![
                component("a-id", "a", lhs_position),
                component("b-id", "b", rhs_position),
            ],
            connections: vec![connection.clone()],
            diagnostics: Vec::new(),
            content_bounds: None,
        };
        (scene, connection)
    }

    fn connection_with_points(
        scene: &CoreDiagramScene,
        connection: &DiagramConnection,
        points: Vec<CorePoint>,
    ) -> (CoreDiagramScene, DiagramConnection) {
        let mut connection = connection.clone();
        connection.line.as_mut().expect("test line").points = points;
        let mut scene = scene.clone();
        scene.connections = vec![connection.clone()];
        (scene, connection)
    }

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
    fn diagram_spatial_index_keeps_blank_queries_local() {
        let mut index = DiagramSpatialIndex::default();
        for component_index in 0..500 {
            let x = component_index as f32 * 100.0;
            index.insert_component(
                component_index,
                HitBounds {
                    min: CorePoint { x, y: -10.0 },
                    max: CorePoint {
                        x: x + 20.0,
                        y: 10.0,
                    },
                },
            );
        }
        for port_index in 0..1000 {
            let x = port_index as f32 * 50.0;
            index.insert_port(
                port_index,
                HitBounds {
                    min: CorePoint {
                        x: x - 2.0,
                        y: -2.0,
                    },
                    max: CorePoint { x: x + 2.0, y: 2.0 },
                },
            );
        }
        for connection_index in 0..500 {
            let x = connection_index as f32 * 100.0;
            index.insert_connection_segment(
                ConnectionSegmentRef {
                    connection_index,
                    segment_index: 0,
                },
                HitBounds {
                    min: CorePoint {
                        x: x + 30.0,
                        y: -1.0,
                    },
                    max: CorePoint {
                        x: x + 70.0,
                        y: 1.0,
                    },
                },
            );
        }

        let blank = index.query(
            CorePoint {
                x: -1_000.0,
                y: 500.0,
            },
            8.0,
        );
        assert!(blank.component_indices.is_empty());
        assert!(blank.port_indices.is_empty());
        assert!(blank.connection_segments.is_empty());

        let nearby = index.query(CorePoint { x: 12.0, y: 0.0 }, 8.0);
        assert!(nearby.component_indices.len() <= 1);
        assert!(nearby.port_indices.len() <= 2);
        assert!(nearby.connection_segments.len() <= 1);
    }

    #[test]
    fn diagram_spatial_index_updates_only_moved_connection_segments() {
        let mut index = DiagramSpatialIndex::default();
        index.insert_connection_segment(
            ConnectionSegmentRef {
                connection_index: 7,
                segment_index: 0,
            },
            HitBounds {
                min: CorePoint { x: 0.0, y: 0.0 },
                max: CorePoint { x: 20.0, y: 2.0 },
            },
        );
        index.insert_connection_segment(
            ConnectionSegmentRef {
                connection_index: 8,
                segment_index: 0,
            },
            HitBounds {
                min: CorePoint { x: 0.0, y: 100.0 },
                max: CorePoint { x: 20.0, y: 102.0 },
            },
        );

        index.update_connection(
            7,
            [(
                0,
                HitBounds {
                    min: CorePoint { x: 200.0, y: 0.0 },
                    max: CorePoint { x: 220.0, y: 2.0 },
                },
            )],
        );

        assert!(index
            .query(CorePoint { x: 10.0, y: 1.0 }, 2.0)
            .connection_segments
            .is_empty());
        assert_eq!(
            index
                .query(CorePoint { x: 210.0, y: 1.0 }, 2.0)
                .connection_segments,
            vec![ConnectionSegmentRef {
                connection_index: 7,
                segment_index: 0,
            }]
        );
        assert_eq!(
            index
                .query(CorePoint { x: 10.0, y: 101.0 }, 2.0)
                .connection_segments,
            vec![ConnectionSegmentRef {
                connection_index: 8,
                segment_index: 0,
            }]
        );
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
    fn serialized_geometry_precision_is_tolerated_by_commit_validation() {
        let raw_origin = CorePoint {
            x: 12.345_679,
            y: -27.654_322,
        };
        let serialized_origin = CorePoint {
            x: 12.345679,
            y: -27.654322,
        };
        assert!(point_nearly_equal(raw_origin, serialized_origin));

        let raw_points = [
            raw_origin,
            CorePoint {
                x: 40.123_455,
                y: -27.654_322,
            },
        ];
        let serialized_points = [
            serialized_origin,
            CorePoint {
                x: 40.123_46,
                y: -27.654_322,
            },
        ];
        assert!(points_nearly_equal(&raw_points, &serialized_points));
        assert_eq!(format_modelica_point(raw_origin), "{12.345679, -27.654322}");
        assert!(!point_nearly_equal(
            raw_origin,
            CorePoint {
                x: raw_origin.x + DIAGRAM_GEOMETRY_EPSILON * 2.0,
                y: raw_origin.y,
            }
        ));
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
    fn connection_validation_tolerates_serialized_line_point_quantization() {
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let expected_points = vec![
            CorePoint {
                x: 0.000049,
                y: 0.0,
            },
            CorePoint {
                x: 100.000049,
                y: 0.0,
            },
        ];
        let (serialized_scene, serialized_connection) = connection_with_points(
            &scene,
            &connection,
            vec![
                CorePoint { x: 0.00005, y: 0.0 },
                CorePoint {
                    x: 100.00005,
                    y: 0.0,
                },
            ],
        );

        assert!(connection_points_match_invariants(
            &serialized_scene,
            &serialized_connection,
            &expected_points,
        ));
    }

    #[test]
    fn dragged_horizontal_detour_collapses_to_one_segment() {
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 30.0, y: 20.0 },
            CorePoint { x: 70.0, y: 20.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            before.clone(),
        );
        let raw = translated_connection_segment(
            &before,
            2,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 0.0, y: -20.0 },
        );
        let after = finalize_connection_route(&scene, &connection, &raw).expect("valid route");
        assert_eq!(
            after,
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 },]
        );
        let (after_scene, after_connection) = connection_with_points(&scene, &connection, after);
        assert!(connection_endpoints_match(&after_scene, &after_connection));
    }

    #[test]
    fn dragged_vertical_detour_collapses_to_one_segment() {
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 30.0 },
            CorePoint { x: 20.0, y: 30.0 },
            CorePoint { x: 20.0, y: 70.0 },
            CorePoint { x: 0.0, y: 70.0 },
            CorePoint { x: 0.0, y: 100.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 100.0 },
            before.clone(),
        );
        let raw = translated_connection_segment(
            &before,
            2,
            ConnectionSegmentOrientation::Vertical,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: -20.0, y: 0.0 },
        );
        let after = finalize_connection_route(&scene, &connection, &raw).expect("valid route");
        assert_eq!(
            after,
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 0.0, y: 100.0 },]
        );
    }

    #[test]
    fn final_route_reanchors_endpoints_before_canonicalization() {
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            before,
        );
        let raw = vec![
            CorePoint { x: 2.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 98.0, y: 0.0 },
        ];
        let after = finalize_connection_route(&scene, &connection, &raw).expect("valid route");
        assert_eq!(
            after,
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 },]
        );
        let (after_scene, after_connection) = connection_with_points(&scene, &connection, after);
        assert!(connection_endpoints_match(&after_scene, &after_connection));
    }

    #[test]
    fn final_route_keeps_a_real_non_collinear_corner() {
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 40.0, y: 0.0 },
            CorePoint { x: 40.0, y: 40.0 },
            CorePoint { x: 100.0, y: 40.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 40.0 },
            points.clone(),
        );
        let after = finalize_connection_route(&scene, &connection, &points).expect("valid route");
        assert_eq!(after, points);
        assert!(is_orthogonal_polyline(&after));
        assert_ne!(
            after,
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 100.0, y: 40.0 }
            ]
        );
    }

    #[test]
    fn final_route_repeats_simplification_until_stable() {
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            before.clone(),
        );
        let after = finalize_connection_route(&scene, &connection, &before).expect("valid route");
        assert_eq!(
            after,
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 },]
        );
    }

    #[test]
    fn horizontal_segment_snaps_to_nearby_connection_axis() {
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 30.0, y: 20.0 },
            CorePoint { x: 70.0, y: 20.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            before.clone(),
        );
        let snapped_delta = snap_connection_segment_delta(
            &before,
            2,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 0.0, y: -18.5 },
            2.0,
        );
        assert_eq!(snapped_delta, CorePoint { x: 0.0, y: -20.0 });
        let raw = translated_connection_segment(
            &before,
            2,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            snapped_delta,
        );
        let after = finalize_connection_route(&scene, &connection, &raw).expect("valid route");
        assert_eq!(
            after,
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 },]
        );
        let (after_scene, after_connection) = connection_with_points(&scene, &connection, after);
        assert!(connection_endpoints_match(&after_scene, &after_connection));
    }

    #[test]
    fn horizontal_segment_outside_snap_threshold_follows_pointer() {
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 30.0, y: 20.0 },
            CorePoint { x: 70.0, y: 20.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
        ];
        let delta = snap_connection_segment_delta(
            &points,
            2,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 0.0, y: -17.0 },
            2.0,
        );
        assert_eq!(delta, CorePoint { x: 0.0, y: -17.0 });
        let moved = translated_connection_segment(
            &points,
            2,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            delta,
        );
        assert_eq!(moved[2].y, 3.0);
        assert_eq!(moved[3].y, 3.0);
    }

    #[test]
    fn vertical_segment_snaps_to_nearby_connection_axis() {
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 30.0 },
            CorePoint { x: 20.0, y: 30.0 },
            CorePoint { x: 20.0, y: 70.0 },
            CorePoint { x: 0.0, y: 70.0 },
            CorePoint { x: 0.0, y: 100.0 },
        ];
        let delta = snap_connection_segment_delta(
            &points,
            2,
            ConnectionSegmentOrientation::Vertical,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: -18.5, y: 0.0 },
            2.0,
        );
        assert_eq!(delta, CorePoint { x: -20.0, y: 0.0 });
    }

    #[test]
    fn segment_snap_chooses_nearest_candidate_axis() {
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 40.0, y: 0.0 },
            CorePoint { x: 40.0, y: 10.0 },
            CorePoint { x: 80.0, y: 10.0 },
            CorePoint { x: 80.0, y: 20.0 },
            CorePoint { x: 120.0, y: 20.0 },
        ];
        // The current y is 3, so y=0 is closer than y=20.
        let delta = snap_connection_segment_delta(
            &points,
            2,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 0.0, y: -7.0 },
            20.0,
        );
        assert_eq!(delta, CorePoint { x: 0.0, y: -10.0 });
    }

    #[test]
    fn connection_snap_tolerance_is_screen_pixel_based() {
        assert_eq!(connection_snap_tolerance(1.0), 8.0);
        assert_eq!(connection_snap_tolerance(4.0), 2.0);
    }

    #[test]
    fn finalized_connection_points_serialize_minimal_line() {
        let source = "model Top\n equation\n  connect(a, b) annotation(Line(points={{0, 0}, {30, 0}, {30, 0}, {70, 0}, {70, 0}, {100, 0}}));\nend Top;";
        let raw_points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 30.0, y: 0.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 70.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            raw_points.clone(),
        );
        let points = finalize_connection_route(&scene, &connection, &raw_points)
            .expect("valid finalized route");
        let line_start = source.find("Line(").expect("Line annotation");
        let line_end =
            matching_delimiter(source, line_start + 4, b'(', b')').expect("Line close") + 1;
        let edit = connection_points_edit(source, SourceRange::new(line_start, line_end), &points)
            .expect("connection points edit");
        let candidate = apply_validated_source_edits(source, vec![edit], 0)
            .expect("apply canonical connection edit");
        assert!(candidate.contains("points={{0, 0}, {100, 0}}"));
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
        // The middle vertical run p1-p2 shifts horizontally; the endpoint
        // segments stay fixed, so no bridge point is added.
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
        // The trailing horizontal run p2-p3-p4 shifts down. Its last point is
        // the semantic rhs endpoint, so a perpendicular bridge is inserted to
        // keep that endpoint anchored while every edge stays axis-aligned.
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
                CorePoint { x: 100.0, y: 42.0 },
                CorePoint { x: 100.0, y: 30.0 },
            ]
        );
        assert!(is_orthogonal_polyline(&translated_connection_segment(
            &points,
            2,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 100.0, y: 12.0 },
        )));
    }

    #[test]
    fn endpoint_vertical_segment_drag_inserts_a_bridge_and_keeps_the_port_anchored() {
        // A vertical endpoint segment follows horizontal pointer motion. Its
        // port endpoint stays fixed; the shifted run is reconnected with a
        // short horizontal bridge.
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 40.0 },
            CorePoint { x: 60.0, y: 40.0 },
            CorePoint { x: 60.0, y: 80.0 },
            CorePoint { x: 100.0, y: 80.0 },
        ];
        let after = translated_connection_segment(
            &points,
            0,
            ConnectionSegmentOrientation::Vertical,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 25.0, y: 0.0 },
        );
        assert_eq!(
            after,
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 25.0, y: 0.0 },
                CorePoint { x: 25.0, y: 40.0 },
                CorePoint { x: 60.0, y: 40.0 },
                CorePoint { x: 60.0, y: 80.0 },
                CorePoint { x: 100.0, y: 80.0 },
            ]
        );
        assert_eq!(after[0], points[0]);
        assert!(is_orthogonal_polyline(&after));
    }

    #[test]
    fn two_point_line_drag_inserts_bridges_at_both_ports() {
        let points = vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }];
        let after = translated_connection_segment(
            &points,
            0,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 0.0, y: 40.0 },
        );
        assert_eq!(
            after,
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 0.0, y: 40.0 },
                CorePoint { x: 100.0, y: 40.0 },
                CorePoint { x: 100.0, y: 0.0 },
            ]
        );
        assert_eq!(after[0], points[0]);
        assert_eq!(*after.last().unwrap(), *points.last().unwrap());
        assert!(is_orthogonal_polyline(&after));
    }

    #[test]
    fn connection_segment_drag_route_matches_commit_route_and_adds_bridges() {
        let points = vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }];
        let preview = build_connection_segment_drag_route(
            &points,
            0,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 0.0, y: 40.0 },
            (points[0], points[1]),
        );
        let committed = translated_connection_segment(
            &points,
            0,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 0.0, y: 40.0 },
        );
        assert_eq!(preview, committed);
        assert_eq!(
            preview,
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 0.0, y: 40.0 },
                CorePoint { x: 100.0, y: 40.0 },
                CorePoint { x: 100.0, y: 0.0 },
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
    fn free_corner_drag_keeps_orthogonal_route_and_fixed_endpoints() {
        // S-shaped five-point route: corners are p1=(0,40), p2=(60,40),
        // p3=(60,80). Dragging p2 freely must slide p1 vertically and p3
        // horizontally so every segment stays axis-aligned.
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 40.0 },
            CorePoint { x: 60.0, y: 40.0 },
            CorePoint { x: 60.0, y: 80.0 },
            CorePoint { x: 100.0, y: 80.0 },
        ];
        let after = translated_connection_corner(
            &before,
            2,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 10.0, y: -15.0 },
        );
        assert_eq!(after.len(), before.len());
        assert_eq!(after[0], before[0]);
        assert_eq!(*after.last().unwrap(), *before.last().unwrap());
        assert_eq!(after[2], CorePoint { x: 70.0, y: 25.0 });
        assert_eq!(after[1], CorePoint { x: 0.0, y: 25.0 });
        assert_eq!(after[3], CorePoint { x: 70.0, y: 80.0 });
        assert!(is_orthogonal_polyline(&after));
    }

    #[test]
    fn corner_next_to_fixed_endpoint_slides_along_port_axis() {
        // L-shaped route p0 anchor -> p1 corner -> p2 -> p3 anchor. The corner
        // p1=(0,50) is adjacent to the fixed left endpoint on a vertical
        // segment, so its x must stay on the port axis; the inner neighbour
        // p2 follows on the horizontal segment.
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 50.0 },
            CorePoint { x: 80.0, y: 50.0 },
            CorePoint { x: 80.0, y: 100.0 },
        ];
        let after = translated_connection_corner(
            &before,
            1,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 20.0, y: 10.0 },
        );
        assert_eq!(after.len(), before.len());
        assert_eq!(after[0], before[0]);
        assert_eq!(*after.last().unwrap(), *before.last().unwrap());
        // x is locked to the left port axis; y follows the pointer.
        assert_eq!(after[1], CorePoint { x: 0.0, y: 60.0 });
        assert_eq!(after[2], CorePoint { x: 80.0, y: 60.0 });
        assert!(is_orthogonal_polyline(&after));
    }

    #[test]
    fn corner_drag_is_rejected_for_routes_without_a_free_inner_vertex() {
        let three_point = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 40.0 },
            CorePoint { x: 60.0, y: 40.0 },
        ];
        let after = translated_connection_corner(
            &three_point,
            1,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 10.0, y: 10.0 },
        );
        assert_eq!(after, three_point);
    }

    #[test]
    fn corner_drag_stays_axis_aligned_on_rotated_lines() {
        // A corner drag is performed in line-local coordinates. On a rotated
        // line (or one with a nonzero origin) the same invariants must hold:
        // endpoints fixed, vertex count unchanged, every segment axis-aligned.
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 30.0 },
            CorePoint { x: 70.0, y: 30.0 },
            CorePoint { x: 70.0, y: 90.0 },
            CorePoint { x: 120.0, y: 90.0 },
        ];
        for rotation in [0.0_f32, 0.5, std::f32::consts::FRAC_PI_2, -1.2] {
            for origin in [
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 35.0, y: -22.0 },
            ] {
                let after = translated_connection_corner(
                    &before,
                    2,
                    origin,
                    rotation,
                    CorePoint { x: 8.0, y: -6.0 },
                );
                assert_eq!(after.len(), before.len());
                assert_eq!(after[0], before[0]);
                assert_eq!(*after.last().unwrap(), *before.last().unwrap());
                assert!(
                    is_orthogonal_polyline(&after),
                    "rotation {rotation} origin {origin:?} broke orthogonality"
                );
                assert_ne!(after[2], before[2]);
            }
        }
    }

    #[test]
    fn zero_delta_corner_drag_is_identity() {
        let before = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 30.0 },
            CorePoint { x: 70.0, y: 30.0 },
            CorePoint { x: 70.0, y: 90.0 },
            CorePoint { x: 120.0, y: 90.0 },
        ];
        let after = translated_connection_corner(
            &before,
            2,
            CorePoint { x: 4.0, y: 9.0 },
            0.8,
            CorePoint { x: 0.0, y: 0.0 },
        );
        assert_eq!(after, before);
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
    fn connection_hit_test_prefers_nearest_then_selected_connection() {
        assert!(connection_hit_candidate_is_better(
            1.0,
            false,
            0,
            0,
            Some((2.0, false, 1, 0)),
        ));
        assert!(connection_hit_candidate_is_better(
            1.0,
            true,
            0,
            0,
            Some((1.0, false, 1, 0)),
        ));
        assert!(connection_hit_candidate_is_better(
            1.0,
            false,
            2,
            0,
            Some((1.0, false, 1, 0)),
        ));
        assert!(!connection_hit_candidate_is_better(
            1.0002,
            false,
            2,
            0,
            Some((1.0, false, 1, 0)),
        ));
    }

    #[test]
    fn connection_hit_test_chooses_nearest_over_connection_iteration_order() {
        let (_, mut nearer) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let (_, mut farther) = connection_test_scene(
            CorePoint { x: 0.0, y: 2.0 },
            CorePoint { x: 100.0, y: 2.0 },
            vec![CorePoint { x: 0.0, y: 2.0 }, CorePoint { x: 100.0, y: 2.0 }],
        );
        nearer.id = "connection:nearer".to_owned();
        farther.id = "connection:farther".to_owned();

        let hit = hit_test_connection(&[farther, nearer], CorePoint { x: 50.0, y: 0.25 }, 3.0)
            .expect("nearest connection hit");
        assert_eq!(hit.connection_id, "connection:nearer");
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

    #[test]
    fn diagram_layers_draw_connectors_after_opaque_components() {
        let coordinate_system = modelica_core::scene::CoordinateSystem::default();
        let icon_for = |owner: &str, graphic: CoreGraphic| {
            Box::new(CoreIconScene {
                owner_qualified_name: Some(owner.to_owned()),
                coordinate_system,
                graphics: vec![ResolvedGraphic {
                    id: modelica_core::scene::GraphicId(format!("{owner}::graphic")),
                    graphic,
                    owner: modelica_core::scene::GraphicOwner {
                        qualified_name: owner.to_owned(),
                        kind: GraphicOwnerKind::Own,
                        instance_name: None,
                    },
                    transform: Transform2D::identity(),
                    editable: false,
                }],
                diagnostics: Vec::new(),
            })
        };
        let component_for = |id: &str,
                             name: &str,
                             class_kind: ClassKind,
                             icon: Box<CoreIconScene>,
                             extent: modelica_core::scene::Extent| {
            CoreComponentInstance {
                id: id.to_owned(),
                name: name.to_owned(),
                source_owner: "Synthetic".to_owned(),
                type_name: name.to_owned(),
                resolved_type_qualified_name: Some(name.to_owned()),
                class_kind: Some(class_kind),
                origin: CorePoint { x: 0.0, y: 0.0 },
                rotation: 0.0,
                placement_extent: Some(extent),
                visible: true,
                editable: true,
                resolved_icon: Some(icon),
                resolved_diagram: None,
            }
        };
        let rectangle = CoreGraphic::Rectangle(RectangleGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint {
                    x: -100.0,
                    y: -100.0,
                },
                p2: CorePoint { x: 100.0, y: 100.0 },
            },
            line_color: [0, 0, 0],
            fill_color: [255, 255, 255],
            line_pattern: Some("LinePattern.Solid".to_owned()),
            line_thickness: Some(1.0),
            fill_pattern: Some("FillPattern.Solid".to_owned()),
            radius: None,
        });
        let connector_graphic = |color| {
            CoreGraphic::Ellipse(EllipseGraphic {
                origin: CorePoint { x: 0.0, y: 0.0 },
                rotation: 0.0,
                extent: modelica_core::scene::Extent {
                    p1: CorePoint { x: -20.0, y: -20.0 },
                    p2: CorePoint { x: 20.0, y: 20.0 },
                },
                line_color: [0, 0, 0],
                fill_color: color,
                line_pattern: Some("LinePattern.Solid".to_owned()),
                line_thickness: Some(1.0),
                fill_pattern: Some("FillPattern.Solid".to_owned()),
                start_angle: None,
                end_angle: None,
            })
        };
        let extent = modelica_core::scene::Extent {
            p1: CorePoint {
                x: -100.0,
                y: -20.0,
            },
            p2: CorePoint { x: -60.0, y: 20.0 },
        };
        let extent_b = modelica_core::scene::Extent {
            p1: CorePoint { x: 60.0, y: -20.0 },
            p2: CorePoint { x: 100.0, y: 20.0 },
        };
        // Deliberately put connectors first and the opaque component last.
        let scene = CoreDiagramScene {
            class_qualified_name: Some("Synthetic".to_owned()),
            class_kind: Some(ClassKind::Model),
            coordinate_system,
            background_graphics: Vec::new(),
            components: vec![
                component_for(
                    "connector-a",
                    "port_a",
                    ClassKind::Connector,
                    icon_for("FluidPort_a", connector_graphic([0, 127, 255])),
                    extent,
                ),
                component_for(
                    "connector-b",
                    "port_b",
                    ClassKind::ExpandableConnector,
                    icon_for("FluidPort_b", connector_graphic([255, 127, 0])),
                    extent_b,
                ),
                component_for(
                    "component",
                    "body",
                    ClassKind::Model,
                    icon_for("OpaqueBody", rectangle),
                    modelica_core::scene::Extent {
                        p1: CorePoint {
                            x: -100.0,
                            y: -100.0,
                        },
                        p2: CorePoint { x: 100.0, y: 100.0 },
                    },
                ),
            ],
            connections: Vec::new(),
            diagnostics: Vec::new(),
            content_bounds: None,
        };

        let geometries = core_diagram_geometry(&scene);
        let layers = geometries
            .iter()
            .map(|geometry| geometry.layer)
            .collect::<Vec<_>>();
        assert!(layers.windows(2).all(|pair| pair[0] <= pair[1]));
        let body_index = geometries
            .iter()
            .position(|geometry| geometry.edit_key.as_deref() == Some("component"))
            .expect("opaque component geometry");
        let connector_indices = geometries
            .iter()
            .enumerate()
            .filter(|(_, geometry)| {
                matches!(
                    geometry.edit_key.as_deref(),
                    Some("connector-a") | Some("connector-b")
                )
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(connector_indices.len(), 4);
        assert!(connector_indices.iter().all(|index| *index > body_index));
        assert_eq!(geometries[body_index].layer, DiagramRenderLayer::Component);
        assert!(connector_indices
            .iter()
            .all(|index| geometries[*index].layer == DiagramRenderLayer::Connector));
    }

    #[test]
    fn msl_partial_two_port_connectors_reach_diagram_geometry() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/modelica/msl-4.1.0/Modelica");
        let mut registry = LibraryRegistry::default();
        registry.add(Library {
            root,
            name: Some("Modelica Standard Library".into()),
            version: Some("4.1.0".into()),
            kind: LibraryKind::Builtin,
            read_only: true,
        });

        let (class, source) = registry
            .resolve_class("Modelica.Fluid.Interfaces.PartialTwoPort")
            .expect("bundled MSL PartialTwoPort");
        let scene = resolve_diagram(&class, &source, &mut registry);
        let geometries = core_diagram_geometry(&scene);

        for (name, expected_extent, expected_scale_x) in [
            (
                "port_a",
                modelica_core::scene::Extent {
                    p1: CorePoint {
                        x: -110.0,
                        y: -10.0,
                    },
                    p2: CorePoint { x: -90.0, y: 10.0 },
                },
                0.1,
            ),
            (
                "port_b",
                modelica_core::scene::Extent {
                    p1: CorePoint { x: 110.0, y: -10.0 },
                    p2: CorePoint { x: 90.0, y: 10.0 },
                },
                -0.1,
            ),
        ] {
            let component = scene
                .components
                .iter()
                .find(|component| component.name == name)
                .unwrap_or_else(|| panic!("missing {name}"));
            let icon = component
                .resolved_icon
                .as_deref()
                .unwrap_or_else(|| panic!("{name} has no resolved icon"));
            let diagram = component
                .resolved_diagram
                .as_deref()
                .unwrap_or_else(|| panic!("{name} has no resolved Diagram layer"));
            let layer = component
                .diagram_layer()
                .unwrap_or_else(|| panic!("{name} has no selected Diagram layer"));
            let placement = diagram_placement_transform(layer, component);
            eprintln!(
                "diagram diagnostic {name}: type={:?} owner={} origin=({:.1},{:.1}) rotation={:.1} placement=({:.1},{:.1})..({:.1},{:.1}) icon_extent=({:.1},{:.1})..({:.1},{:.1}) icon_graphics={} diagram_extent=({:.1},{:.1})..({:.1},{:.1}) diagram_graphics={} translation=({:.1},{:.1}) scale=({:.3},{:.3})",
                component.resolved_type_qualified_name,
                component.source_owner,
                component.origin.x,
                component.origin.y,
                component.rotation,
                expected_extent.p1.x,
                expected_extent.p1.y,
                expected_extent.p2.x,
                expected_extent.p2.y,
                icon.coordinate_system.extent.p1.x,
                icon.coordinate_system.extent.p1.y,
                icon.coordinate_system.extent.p2.x,
                icon.coordinate_system.extent.p2.y,
                icon.graphics.len(),
                diagram.coordinate_system.extent.p1.x,
                diagram.coordinate_system.extent.p1.y,
                diagram.coordinate_system.extent.p2.x,
                diagram.coordinate_system.extent.p2.y,
                diagram.graphics.len(),
                placement.translation.x,
                placement.translation.y,
                placement.scale_x,
                placement.scale_y,
            );
            assert!(!icon.graphics.is_empty(), "{name} icon has no graphics");
            assert!(
                !diagram.graphics.is_empty(),
                "{name} Diagram has no graphics"
            );
            assert!((placement.scale_x - expected_scale_x).abs() < 0.001);
            assert!((placement.scale_y - 0.1).abs() < 0.001);
            assert!(
                (placement.translation.x - if name == "port_a" { -100.0 } else { 100.0 }).abs()
                    < 0.001
            );
            assert!(placement.translation.y.abs() < 0.001);

            let component_geometries = geometries
                .iter()
                .filter(|geometry| geometry.edit_key.as_deref() == Some(component.id.as_str()))
                .collect::<Vec<_>>();
            assert!(
                !component_geometries.is_empty(),
                "{name} produced no geometry"
            );
            let mut min = [f32::INFINITY; 2];
            let mut max = [f32::NEG_INFINITY; 2];
            for geometry in component_geometries {
                for vertex in &geometry.vertices {
                    min[0] = min[0].min(vertex.position[0]);
                    min[1] = min[1].min(vertex.position[1]);
                    max[0] = max[0].max(vertex.position[0]);
                    max[1] = max[1].max(vertex.position[1]);
                }
            }
            eprintln!(
                "diagram diagnostic {name}: produced_geometry={} bounds=({:.1},{:.1})..({:.1},{:.1})",
                geometries
                    .iter()
                    .filter(|geometry| geometry.edit_key.as_deref() == Some(component.id.as_str()))
                    .count(),
                min[0],
                min[1],
                max[0],
                max[1]
            );
            let expected_min_x = if name == "port_a" { -104.0 } else { 96.0 };
            let expected_max_x = if name == "port_a" { -96.0 } else { 104.0 };
            // Stroke tessellation expands the filled extent by roughly half
            // the transformed line width, so bounds are checked with a small
            // renderer tolerance rather than against the raw placement box.
            assert!((min[0] - expected_min_x).abs() < 0.1);
            assert!((max[0] - expected_max_x).abs() < 0.1);
            assert!((min[1] + 4.0).abs() < 0.1);
            assert!((max[1] - 4.0).abs() < 0.1);
        }
    }

    #[test]
    fn cached_snap_axes_match_the_uncached_algorithm() {
        let points = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 30.0 },
            CorePoint { x: 20.0, y: 30.0 },
            CorePoint { x: 20.0, y: 70.0 },
            CorePoint { x: 0.0, y: 70.0 },
            CorePoint { x: 0.0, y: 100.0 },
        ];
        let raw = CorePoint { x: -18.5, y: 0.0 };
        let expected = snap_connection_segment_delta(
            &points,
            2,
            ConnectionSegmentOrientation::Vertical,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            raw,
            2.0,
        );
        let mut snapped = None;
        let actual = snap_connection_segment_delta_cached(
            connection_segment_axis(&points, 2, ConnectionSegmentOrientation::Vertical),
            ConnectionSegmentOrientation::Vertical,
            0.0,
            raw,
            &connection_segment_snap_axes(&points, 2, ConnectionSegmentOrientation::Vertical),
            &mut snapped,
            (2.0, 2.0),
        );
        assert_eq!(actual, expected);
        assert_eq!(snapped, Some(0.0));
    }

    #[test]
    fn snap_hysteresis_enters_holds_and_releases() {
        let points = vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 10.0, y: 0.0 }];
        let axes = vec![0.0];
        let mut snapped = None;
        let snap = |x, snapped: &mut Option<f32>| {
            snap_connection_segment_delta_cached(
                connection_segment_axis(&points, 0, ConnectionSegmentOrientation::Vertical),
                ConnectionSegmentOrientation::Vertical,
                0.0,
                CorePoint { x, y: 0.0 },
                &axes,
                snapped,
                (8.0, 12.0),
            )
        };
        assert_eq!(snap(7.0, &mut snapped).x, 0.0);
        assert_eq!(snapped, Some(0.0));
        assert_eq!(snap(11.0, &mut snapped).x, 0.0);
        assert_eq!(snapped, Some(0.0));
        assert_eq!(snap(13.0, &mut snapped).x, 13.0);
        assert_eq!(snapped, None);
    }

    #[test]
    fn semantic_endpoints_are_used_by_connection_drag_route() {
        let points = vec![
            CorePoint { x: 0.00005, y: 0.0 },
            CorePoint { x: 0.0, y: 20.0 },
            CorePoint { x: 60.0, y: 20.0 },
            CorePoint {
                x: 60.00005,
                y: 0.0,
            },
        ];
        let anchored = build_connection_segment_drag_route(
            &points,
            1,
            ConnectionSegmentOrientation::Horizontal,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            CorePoint { x: 0.0, y: 0.0 },
            (CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 60.0, y: 0.0 }),
        );
        assert_eq!(anchored.first(), Some(&CorePoint { x: 0.0, y: 0.0 }));
        assert_eq!(anchored.last(), Some(&CorePoint { x: 60.0, y: 0.0 }));
        assert!(is_orthogonal_polyline(&anchored));
    }

    #[test]
    fn preview_mesh_topology_is_stable_for_drag_updates() {
        let initial = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 20.0 },
            CorePoint { x: 40.0, y: 20.0 },
        ];
        let mut vertices = preview_connection_vertices(
            &initial,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            1.0,
            Transform2D::identity(),
        );
        let capacity = vertices.len();
        let moved = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 36.0 },
            CorePoint { x: 40.0, y: 36.0 },
        ];
        update_preview_connection_vertices(
            &mut vertices,
            &moved,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            1.0,
            Transform2D::identity(),
        );
        assert_eq!(vertices.len(), capacity);
        assert_eq!(capacity, (initial.len() - 1) * 4);
    }

    #[test]
    fn present_mode_uses_immediate_only_when_requested_and_supported() {
        let supported = [wgpu::PresentMode::Fifo, wgpu::PresentMode::Immediate];
        assert_eq!(
            select_present_mode(&supported, true),
            wgpu::PresentMode::Immediate
        );
        assert_eq!(
            select_present_mode(&supported, false),
            wgpu::PresentMode::Fifo
        );
    }

    #[test]
    fn low_latency_present_mode_prefers_mailbox_when_available() {
        let supported = [
            wgpu::PresentMode::Fifo,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Mailbox,
        ];
        assert_eq!(
            select_present_mode(&supported, true),
            wgpu::PresentMode::Mailbox
        );
    }

    #[test]
    fn connection_creation_orientation_uses_hysteresis() {
        let horizontal = CorePoint { x: 100.0, y: 90.0 };
        let orientation = update_connection_creation_orientation(
            None,
            CorePoint { x: 0.0, y: 0.0 },
            horizontal,
            1.0,
        );
        assert_eq!(orientation, Some(TailOrientation::HorizontalFirst));

        let nearly_tied = CorePoint { x: 100.0, y: 104.0 };
        assert_eq!(
            update_connection_creation_orientation(
                orientation,
                CorePoint { x: 0.0, y: 0.0 },
                nearly_tied,
                1.0,
            ),
            orientation
        );
        let switched = CorePoint { x: 100.0, y: 109.0 };
        assert_eq!(
            update_connection_creation_orientation(
                orientation,
                CorePoint { x: 0.0, y: 0.0 },
                switched,
                1.0,
            ),
            Some(TailOrientation::VerticalFirst)
        );
        assert_eq!(
            connection_creation_elbow_with_orientation(
                CorePoint { x: 0.0, y: 0.0 },
                switched,
                orientation,
            ),
            Some(CorePoint { x: 100.0, y: 0.0 })
        );
    }

    #[test]
    fn present_mode_falls_back_to_stable_or_first_supported_mode() {
        let supported = [wgpu::PresentMode::AutoVsync];
        assert_eq!(
            select_present_mode(&supported, true),
            wgpu::PresentMode::AutoVsync
        );

        let supported = [wgpu::PresentMode::Mailbox];
        assert_eq!(
            select_present_mode(&supported, false),
            wgpu::PresentMode::Mailbox
        );
    }

    #[test]
    fn connection_creation_route_keeps_waypoints_and_adds_one_elbow() {
        let committed = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 20.0, y: 0.0 },
            CorePoint { x: 20.0, y: 15.0 },
        ];
        let mut route = Vec::new();
        append_connection_creation_route(&committed, CorePoint { x: 50.0, y: 35.0 }, &mut route);
        assert_eq!(
            route,
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 20.0, y: 0.0 },
                CorePoint { x: 20.0, y: 15.0 },
                CorePoint { x: 50.0, y: 15.0 },
                CorePoint { x: 50.0, y: 35.0 },
            ]
        );
        assert_eq!(
            connection_creation_waypoint(&committed, CorePoint { x: 50.0, y: 35.0 }),
            Some(CorePoint { x: 50.0, y: 15.0 })
        );
    }

    #[test]
    fn connection_creation_source_edit_is_single_parseable_insert() {
        let source = "model Test\nequation\nend Test;\n";
        let lhs = ConnectorRef {
            component_name: "a".to_owned(),
            connector_path: "port".to_owned(),
        };
        let rhs = ConnectorRef {
            component_name: "b".to_owned(),
            connector_path: "port".to_owned(),
        };
        let edit = new_connection_source_edit(
            source,
            &lhs,
            &rhs,
            &[CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 20.0, y: 0.0 }],
        )
        .expect("closing class end");
        let candidate = apply_validated_source_edits(source, vec![edit], 0)
            .expect("single connection insertion");
        assert!(candidate
            .contains("connect(a.port, b.port) annotation(Line(points={{0, 0}, {20, 0}}));"));
    }

    #[test]
    fn nearest_port_uses_port_buckets_and_excludes_source_without_sorting() {
        let anchors = vec![
            ConnectorAnchor {
                key: PortKey::new("a", ""),
                connector_ref: ConnectorRef {
                    component_name: "a".to_owned(),
                    connector_path: String::new(),
                },
                world_position: CorePoint { x: 0.0, y: 0.0 },
                visual_bounds: None,
                qualified_type: None,
                owner_component_id: "a".to_owned(),
                editable: true,
            },
            ConnectorAnchor {
                key: PortKey::new("b", ""),
                connector_ref: ConnectorRef {
                    component_name: "b".to_owned(),
                    connector_path: String::new(),
                },
                world_position: CorePoint { x: 20.0, y: 0.0 },
                visual_bounds: None,
                qualified_type: None,
                owner_component_id: "b".to_owned(),
                editable: true,
            },
        ];
        let mut index = DiagramSpatialIndex::default();
        index.insert_port(
            0,
            HitBounds {
                min: CorePoint { x: -64.0, y: -64.0 },
                max: CorePoint { x: 64.0, y: 64.0 },
            },
        );
        index.insert_port(
            1,
            HitBounds {
                min: CorePoint { x: 19.0, y: -1.0 },
                max: CorePoint { x: 21.0, y: 1.0 },
            },
        );
        assert_eq!(
            index.nearest_port(CorePoint { x: 0.0, y: 0.0 }, 65.0, None, &anchors,),
            Some(0)
        );
        assert_eq!(
            index.nearest_port(
                CorePoint { x: 0.0, y: 0.0 },
                65.0,
                Some(&anchors[0].key),
                &anchors,
            ),
            Some(1)
        );
    }
}
