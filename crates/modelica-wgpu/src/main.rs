use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    path::{Path as FsPath, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, OnceLock,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use bytemuck::{Pod, Zeroable};
use egui::epaint::TextShape;
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Layout, Margin,
    Pos2, Rect, RichText, Rounding, Sense, Stroke, Vec2,
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
    parse, parse_component_declaration, resolve_diagram, resolve_modelica_text, Class, ClassKind,
    IconResolver, Library, LibraryKind, LibraryRegistry, ModelTextContext, ModelicaFile,
    PackageLoader, PackageMember, PackageNode, SourceEdit, SourceRange, SourceTransaction,
};
use modelica_render::{
    canonicalize_orthogonal_points, connector_anchor_hit_distance, connector_anchors,
    line_local_to_world, reanchor_connection_points, resolve_connection_endpoints,
    resolved_graphic_contains_point, resolved_graphic_contains_point_with_transform,
    strict_connection_points, world_to_line_local, ConnectionPointOrder, ConnectorAnchor, PortKey,
    ORTHOGONAL_EPSILON,
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

const DEFAULT_MSAA_SAMPLES: u32 = 4;
const INITIAL_ZOOM: f32 = 3.0;
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 24.0;
const CONNECTION_SNAP_ENTER_PIXELS: f32 = 8.0;
const CONNECTION_SNAP_EXIT_PIXELS: f32 = 12.0;
const MIN_SCREEN_CONNECTION_STROKE_PX: f32 = 1.25;
const MIN_SCREEN_ICON_STROKE_PX: f32 = 1.05;
const CONNECTION_ENDPOINT_OVERDRAW_PX: f32 = 3.0;
const MIN_SCREEN_MODEL_TEXT_PX: f32 = 10.5;
const MIN_SCREEN_MODEL_NAME_TEXT_PX: f32 = 11.5;
const FIT_SCENE_FILL: f32 = 0.86;
const CONNECTION_HIT_DISTANCE_TIE_EPSILON: f32 = 1.0e-4;
const PORT_HIT_DISTANCE_TIE_EPSILON: f32 = 1.0e-4;
const COMPONENT_DRAG_PORT_HIT_PIXELS: f32 = 4.0;
// Modelica source coordinates are serialized as decimals, so small endpoint
// differences are expected. A larger drift means the source route belongs to
// an older placement and its interior points must not be trusted for display.
const ROUTE_REPAIR_ENDPOINT_DRIFT_UNITS: f32 = 2.0;
// Keep ordinary routed detours, but reject a source route whose length is more
// than four times the direct Manhattan distance between the current anchors.
const ROUTE_REPAIR_MAX_DETOUR_RATIO: f32 = 4.0;
const DIAGRAM_HIT_GRID_CELL_SIZE: f32 = 64.0;
const UI_FONT_MEDIUM: &str = "modelica-ui-medium";
const UI_FONT_SEMIBOLD: &str = "modelica-ui-semibold";
const UI_FONT_ITALIC: &str = "modelica-ui-italic";
const UI_FONT_SEMIBOLD_ITALIC: &str = "modelica-ui-semibold-italic";
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

struct PendingFileSave {
    path: PathBuf,
    contents: String,
    class_sources: Vec<ClassSource>,
}

fn ranges_strictly_contain(outer: SourceRange, inner: SourceRange) -> bool {
    outer.start <= inner.start
        && inner.end <= outer.end
        && (outer.start < inner.start || inner.end < outer.end)
}

fn nested_edit_conflict(parent: &str, child: &str, reason: impl Into<String>) -> String {
    format!(
        "cannot merge nested edits for {parent} and {child}: {}",
        reason.into()
    )
}

fn find_nested_class_range(source: &str, parent: &str, child: &str) -> Result<SourceRange, String> {
    let relative = child.strip_prefix(&format!("{parent}."));
    let Some(relative) = relative else {
        return Err(nested_edit_conflict(
            parent,
            child,
            "the child is not nested below the parent",
        ));
    };
    let relative_parts = relative.split('.').collect::<Vec<_>>();
    let parsed = parsed_class_sources(source, FsPath::new("<nested-edit>"))?;
    let candidates = parsed
        .into_iter()
        .filter(|class| {
            let parts = class.qualified_name.split('.').collect::<Vec<_>>();
            parts.len() > relative_parts.len()
                && parts[parts.len() - relative_parts.len()..] == relative_parts
        })
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [class] => Ok(class.source_range),
        [] => Err(nested_edit_conflict(
            parent,
            child,
            "the child class could not be located in the parent override",
        )),
        _ => Err(nested_edit_conflict(
            parent,
            child,
            "the child identity is ambiguous in the parent override",
        )),
    }
}

fn compose_dirty_class(
    document: &LoadedDocument,
    file_classes: &[ClassSource],
    class: &ClassSource,
) -> Result<String, String> {
    let updated = document
        .source_overrides
        .get(&class.qualified_name)
        .ok_or_else(|| {
            format!(
                "missing source override for {} while composing nested edits",
                class.qualified_name
            )
        })?;
    let dirty_children = file_classes
        .iter()
        .filter(|candidate| {
            candidate.qualified_name != class.qualified_name
                && document
                    .source_overrides
                    .contains_key(&candidate.qualified_name)
                && ranges_strictly_contain(class.source_range, candidate.source_range)
        })
        .filter(|candidate| {
            !file_classes.iter().any(|intermediate| {
                intermediate.qualified_name != class.qualified_name
                    && intermediate.qualified_name != candidate.qualified_name
                    && document
                        .source_overrides
                        .contains_key(&intermediate.qualified_name)
                    && ranges_strictly_contain(class.source_range, intermediate.source_range)
                    && ranges_strictly_contain(intermediate.source_range, candidate.source_range)
            })
        })
        .collect::<Vec<_>>();
    if dirty_children.is_empty() {
        return Ok(updated.clone());
    }

    let mut child_edits = Vec::new();
    for child in dirty_children {
        let child_range =
            find_nested_class_range(updated, &class.qualified_name, &child.qualified_name)?;
        let child_current = compose_dirty_class(document, file_classes, child)?;
        let child_raw = document
            .source_overrides
            .get(&child.qualified_name)
            .expect("dirty child override checked above");
        let child_saved = document
            .saved_class_text
            .get(&child.qualified_name)
            .ok_or_else(|| {
                format!(
                    "no baseline snapshot for nested class {}",
                    child.qualified_name
                )
            })?;
        let observed = updated
            .get(child_range.start..child_range.end)
            .ok_or_else(|| {
                nested_edit_conflict(
                    &class.qualified_name,
                    &child.qualified_name,
                    "the child range is not valid in the parent override",
                )
            })?;
        if observed == child_current {
            continue;
        }
        if observed != child_saved && observed != child_raw {
            return Err(nested_edit_conflict(
                &class.qualified_name,
                &child.qualified_name,
                "both overrides changed the child body differently",
            ));
        }
        if observed != child_current {
            child_edits.push((child_range, child_current));
        }
    }

    let mut merged = updated.clone();
    child_edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, replacement) in child_edits {
        if merged.get(range.start..range.end).is_none() {
            return Err(nested_edit_conflict(
                &class.qualified_name,
                "nested child",
                "the merged child range is not valid",
            ));
        }
        merged.replace_range(range.start..range.end, &replacement);
    }
    Ok(merged)
}

/// Compose the current in-memory view of one source file.
///
/// Dirty classes whose ranges are nested are merged from the deepest class
/// outward. Only the outermost dirty class in a containment group becomes a
/// file replacement, so the registry preview and the save candidate use the
/// same source text.
fn compose_current_file_source(
    document: &LoadedDocument,
    path: &FsPath,
    disk_original: &str,
) -> Result<String, String> {
    let file_classes = document
        .class_sources
        .iter()
        .filter(|class| class.source_file == path)
        .cloned()
        .collect::<Vec<_>>();
    let mut dirty_classes = Vec::new();
    for qualified_name in document.source_overrides.keys() {
        let Some(class) = file_classes
            .iter()
            .find(|class| class.qualified_name == *qualified_name)
        else {
            continue;
        };
        let range = class.source_range;
        if range.start > range.end
            || range.end > disk_original.len()
            || disk_original.get(range.start..range.end).is_none()
        {
            return Err(format!(
                "stale source range at byte {} in {}; reload the library first",
                range.start,
                path.display()
            ));
        }
        let expected = document
            .saved_class_text
            .get(qualified_name)
            .ok_or_else(|| {
                format!(
                    "no baseline snapshot for {} in {}; reload the library first",
                    qualified_name,
                    path.display()
                )
            })?;
        if &disk_original[range.start..range.end] != expected {
            return Err(format!(
                "{} changed on disk since it was loaded; reload before saving",
                path.display()
            ));
        }
        dirty_classes.push(class.clone());
    }
    if dirty_classes.is_empty() {
        return Ok(disk_original.to_owned());
    }

    let roots = dirty_classes
        .iter()
        .filter(|class| {
            !dirty_classes.iter().any(|candidate| {
                candidate.qualified_name != class.qualified_name
                    && ranges_strictly_contain(candidate.source_range, class.source_range)
            })
        })
        .collect::<Vec<_>>();
    let mut sorted_roots = roots.clone();
    sorted_roots.sort_by_key(|class| class.source_range.start);
    for pair in sorted_roots.windows(2) {
        if pair[0].source_range.end > pair[1].source_range.start {
            return Err(format!(
                "overlapping source edits in {}: {} overlaps {}",
                path.display(),
                pair[0].qualified_name,
                pair[1].qualified_name
            ));
        }
    }

    let mut edits = Vec::new();
    for root in roots {
        edits.push((
            root.source_range,
            compose_dirty_class(document, &file_classes, root)?,
        ));
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut composed = disk_original.to_owned();
    for (range, replacement) in edits {
        composed.replace_range(range.start..range.end, &replacement);
    }
    Ok(composed)
}

/// Build and validate every file's save plan before writing any file.
///
/// The plan validates each replacement when the
/// on-disk slice still equals the text the document last loaded or wrote.
/// An externally modified file therefore aborts the plan instead of being
/// silently corrupted. Edits touching the same file are applied back-to-front
/// so earlier offsets stay valid.
fn build_save_plan(document: &LoadedDocument) -> Result<Vec<PendingFileSave>, String> {
    if document.source_overrides.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_file = std::collections::BTreeSet::new();
    for qualified_name in document.source_overrides.keys() {
        let class = document
            .class_sources
            .iter()
            .find(|class| class.qualified_name == *qualified_name)
            .ok_or_else(|| format!("no source location for edited class {qualified_name}"))?;
        by_file.insert(class.source_file.clone());
    }
    let mut plan = Vec::with_capacity(by_file.len());
    for path in by_file {
        let disk_original =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let contents = compose_current_file_source(document, &path, &disk_original)?;
        let class_sources = parsed_class_sources(&contents, &path)?;
        plan.push(PendingFileSave {
            path,
            contents,
            class_sources,
        });
    }
    Ok(plan)
}

/// Persist every class edited in memory back to its own `.mo` file.
///
/// All validation happens in `build_save_plan` before the first write. Writes
/// are still performed one file at a time, so a filesystem failure during the
/// write phase may leave an accurately reported partial save.
fn save_edited_classes(document: &mut LoadedDocument) -> Result<usize, String> {
    let plan = build_save_plan(document)?;
    apply_save_plan(document, plan, write_file_atomic)
}

fn apply_save_plan<F>(
    document: &mut LoadedDocument,
    plan: Vec<PendingFileSave>,
    mut write_file: F,
) -> Result<usize, String>
where
    F: FnMut(&std::path::Path, &str) -> Result<(), String>,
{
    let total_files = plan.len();
    let mut saved_files = 0;
    for file in plan {
        let path = file.path.clone();
        if let Err(error) = write_file(&path, &file.contents) {
            return Err(format!(
                "save partially completed: {saved_files} of {total_files} file(s) saved; failed to save {}: {error}; remaining edits were kept in memory",
                path.display()
            ));
        }
        if let Err(error) = document.apply_saved_file(file) {
            return Err(format!(
                "save partially completed: {saved_files} of {total_files} file(s) saved; file {} was written but the in-memory state could not be refreshed: {error}",
                path.display()
            ));
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
            model_tree: TreeNode::default(),
            diagnostics: 0,
            registry: RefCell::new(LibraryRegistry::default()),
            icon_cache: HashMap::new(),
            diagram_cache: HashMap::new(),
            scene_resolution_stats: RefCell::new(SceneResolutionStats::default()),
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

    fn nested_document(label: &str) -> (LoadedDocument, PathBuf, String) {
        let directory = temp_directory(label);
        let path = directory.join("Nested.mo");
        let source = "model Parent\n  Real parent_value;\n  model Child\n    Real child_value;\n    model Grandchild\n      Real grand_value;\n    end Grandchild;\n  end Child;\nend Parent;\n";
        fs::write(&path, source).unwrap();
        let document = LoadedDocument::load(&path).expect("load nested fixture");
        (document, path, source.to_owned())
    }

    fn assert_candidate_contains(candidate: &str, snippets: &[&str]) {
        for snippet in snippets {
            assert!(
                candidate.contains(snippet),
                "candidate is missing {snippet:?}"
            );
        }
    }

    #[test]
    fn save_replaces_only_the_edited_class_range() {
        let directory = temp_directory("single");
        let content = "within X; model A\n  Real value;\nend A; model B\n  Real untouched;\nend B;";
        let mut document = temp_document(
            &directory,
            "Single.mo",
            content,
            &[(
                "X.A",
                "model A\n  Real value;\nend A;",
                "model A\n  Real value = 2;\nend A;",
            )],
        )
        .0;
        let saved = save_edited_classes(&mut document).expect("save succeeds");
        assert_eq!(saved, 1);
        let result = fs::read_to_string(&document.path).unwrap();
        assert!(result.contains("model A\n  Real value = 2;\nend A;"));
        assert!(result.contains("model B\n  Real untouched;\nend B;"));
        assert!(!result.contains("model A\n  Real value;\nend A;"));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn save_applies_back_to_front_when_two_classes_share_a_file() {
        let directory = temp_directory("shared");
        let content = "model A\n  Real value;\nend A; model B\n  Real value;\nend B;";
        let (mut document, path) = temp_document(
            &directory,
            "Shared.mo",
            content,
            &[
                (
                    "B",
                    "model B\n  Real value;\nend B;",
                    "model B\n  Real value = 2;\nend B;",
                ),
                (
                    "A",
                    "model A\n  Real value;\nend A;",
                    "model A\n  Real value = 2;\nend A;",
                ),
            ],
        );
        let saved = save_edited_classes(&mut document).expect("save succeeds");
        assert_eq!(saved, 1);
        let result = fs::read_to_string(&path).unwrap();
        assert!(result.contains("model A\n  Real value = 2;\nend A;"));
        assert!(result.contains("model B\n  Real value = 2;\nend B;"));
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

    // These regressions cover saves whose replacement changes the byte length
    // of the edited class. The following class must remain addressable after
    // the save, and a repeated save must be a no-op.
    fn assert_repeat_save_after_length_change(label: &str, before: &str, after: &str) {
        struct Fixture {
            directory: PathBuf,
            path: PathBuf,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                // Only remove this test's file and its now-empty directory,
                // including when a regression assertion unwinds.
                let _ = fs::remove_file(&self.path);
                let _ = fs::remove_dir(&self.directory);
            }
        }

        assert_ne!(before.len(), after.len(), "fixture must change byte length");
        let directory = temp_directory(label);
        let fixture = Fixture {
            path: directory.join("Repeated.mo"),
            directory,
        };
        let header = "// Preserve the file header.\n";
        let neighbor = "\n\nmodel Neighbor\n  Real untouched;\nend Neighbor;\n";
        let original = format!("{header}{before}{neighbor}");
        let expected = format!("{header}{after}{neighbor}");
        fs::write(&fixture.path, &original).expect("write regression fixture");

        let mut document = LoadedDocument::load(&fixture.path).expect("load regression fixture");
        assert_eq!(document.class_text("A").as_deref(), Some(before));
        document.set_class_text("A", after.to_owned());
        assert_eq!(save_edited_classes(&mut document).expect("first save"), 1);
        assert_eq!(fs::read_to_string(&fixture.path).unwrap(), expected);

        // No external writes or additional edits occur between the two saves.
        let second_save = save_edited_classes(&mut document);
        assert_eq!(
            fs::read_to_string(&fixture.path).unwrap(),
            expected,
            "a repeated save must preserve the edited class and its neighbor"
        );
        assert_eq!(
            second_save.expect("second save must accept the text just saved by this document"),
            0,
            "a second save without new edits must not rewrite the file"
        );
    }

    #[test]
    fn repeat_save_after_class_grows_is_a_noop() {
        assert_repeat_save_after_length_change(
            "repeat-grow",
            "model A\n  parameter Real value = 1;\nend A;",
            "model A\n  parameter Real value = 123456789;\nend A;",
        );
    }

    #[test]
    fn repeat_save_after_class_shrinks_is_a_noop() {
        assert_repeat_save_after_length_change(
            "repeat-shrink",
            "model A\n  parameter Real value = 123456789;\nend A;",
            "model A\n  parameter Real value = 1;\nend A;",
        );
    }

    #[test]
    fn repeat_save_after_utf8_byte_length_changes_is_a_noop() {
        assert_repeat_save_after_length_change(
            "repeat-utf8",
            "model A \"Temperature\"\n  Real value;\nend A;",
            "model A \"温度设定值与测量值\"\n  Real value;\nend A;",
        );
    }

    #[test]
    fn restoring_saved_class_text_clears_the_pending_override() {
        let directory = temp_directory("restore-saved-text");
        let source = "model A\n  Real value;\nend A;\n";
        let (mut document, path) = temp_document(
            &directory,
            "Restore.mo",
            source,
            &[("A", source, "model A\n  Real changed;\nend A;\n")],
        );

        document.set_class_text("A", source.to_owned());
        assert!(!document.source_overrides.contains_key("A"));
        assert_eq!(save_edited_classes(&mut document).unwrap(), 0);
        assert_eq!(fs::read_to_string(&path).unwrap(), source);

        document.set_class_text("A", "model A\n  Real changed again;\nend A;\n".to_owned());
        assert!(document.source_overrides.contains_key("A"));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn unsaved_state_follows_content_not_history() {
        let directory = temp_directory("dirty-content");
        let source = "model A\n  Real value;\nend A;\n";
        let (mut document, path) = temp_document(
            &directory,
            "Dirty.mo",
            source,
            &[("A", source, "model A\n  Real changed;\nend A;\n")],
        );

        assert!(document.has_unsaved_changes());
        assert!(document.title(None).contains("modelica-wgpu *"));
        document.set_class_text("A", source.to_owned());
        assert!(!document.has_unsaved_changes());
        assert!(!document.title(None).contains("modelica-wgpu *"));
        assert_eq!(save_edited_classes(&mut document).unwrap(), 0);

        document.set_class_text("A", "model A\n  Real changed;\nend A;\n".to_owned());
        assert!(document.has_unsaved_changes());
        assert_eq!(save_edited_classes(&mut document).unwrap(), 1);
        assert!(!document.has_unsaved_changes());
        assert!(!document.title(None).contains("modelica-wgpu *"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "model A\n  Real changed;\nend A;\n"
        );

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn discard_unsaved_changes_restores_saved_baseline() {
        let directory = temp_directory("discard-dirty");
        let source = "model A\n  Real value;\nend A;\n";
        let (mut document, path) = temp_document(
            &directory,
            "Discard.mo",
            source,
            &[("A", source, "model A\n  Real changed;\nend A;\n")],
        );

        assert!(document.has_unsaved_changes());
        document.discard_unsaved_changes();
        assert!(!document.has_unsaved_changes());
        assert_eq!(document.class_text("A").as_deref(), Some(source));
        assert_eq!(fs::read_to_string(&path).unwrap(), source);

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn save_after_undo_and_redo_sequence_keeps_source_state_consistent() {
        let directory = temp_directory("save-undo-redo");
        let source = "model A\n  Real value;\nend A;\n";
        let changed = "model A\n  Real changed;\nend A;";
        let path = directory.join("UndoRedo.mo");
        fs::write(&path, source).unwrap();
        let mut document = LoadedDocument::load(&path).expect("load undo/redo fixture");
        let original_text = document.class_text("A").expect("original class text");

        document.set_class_text("A", changed.to_owned());
        assert_eq!(save_edited_classes(&mut document).unwrap(), 1);
        let saved_disk = fs::read_to_string(&path).unwrap();
        assert!(saved_disk.contains("Real changed;"));
        document.set_class_text("A", original_text);
        assert_eq!(save_edited_classes(&mut document).unwrap(), 1);
        let undone_disk = fs::read_to_string(&path).unwrap();
        assert!(undone_disk.contains("Real value;"));

        document.set_class_text("A", changed.to_owned());
        assert_eq!(save_edited_classes(&mut document).unwrap(), 1);
        assert_eq!(fs::read_to_string(&path).unwrap(), saved_disk);
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
    fn save_validates_all_files_before_writing_any_file() {
        let directory = temp_directory("plan-all-files");
        let first_path = directory.join("First.mo");
        let second_path = directory.join("Second.mo");
        let first_source = "model First\n  Real value;\nend First;\n";
        let second_source = "model Second\n  Real value;\nend Second;\n";
        fs::write(&first_path, first_source).unwrap();
        fs::write(&second_path, second_source).unwrap();

        let mut source_overrides = HashMap::new();
        source_overrides.insert(
            "First".to_owned(),
            "model First\n  Real changed;\nend First;\n".to_owned(),
        );
        source_overrides.insert(
            "Second".to_owned(),
            "model Second\n  Real changed;\nend Second;\n".to_owned(),
        );
        let class_sources = vec![
            ClassSource {
                qualified_name: "First".to_owned(),
                source_file: first_path.clone(),
                source_range: SourceRange::new(0, first_source.len()),
            },
            ClassSource {
                qualified_name: "Second".to_owned(),
                source_file: second_path.clone(),
                source_range: SourceRange::new(0, second_source.len()),
            },
        ];
        let saved_class_text = HashMap::from([
            ("First".to_owned(), first_source.to_owned()),
            ("Second".to_owned(), second_source.to_owned()),
        ]);
        let mut document = LoadedDocument {
            path: first_path.clone(),
            package_name: "PlanTest".to_owned(),
            class_names: Vec::new(),
            model_tree: TreeNode::default(),
            diagnostics: 0,
            registry: RefCell::new(LibraryRegistry::default()),
            icon_cache: HashMap::new(),
            diagram_cache: HashMap::new(),
            scene_resolution_stats: RefCell::new(SceneResolutionStats::default()),
            class_sources,
            source_overrides,
            saved_class_text,
            source_versions: HashMap::new(),
        };

        let externally_changed_second = "// changed externally\n".to_owned() + second_source;
        fs::write(&second_path, &externally_changed_second).unwrap();
        let error = save_edited_classes(&mut document).expect_err("plan must reject conflict");
        assert!(
            error.contains("changed on disk"),
            "unexpected error: {error}"
        );
        assert_eq!(fs::read_to_string(&first_path).unwrap(), first_source);
        assert_eq!(
            fs::read_to_string(&second_path).unwrap(),
            externally_changed_second
        );
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn save_reports_partial_progress_when_a_later_file_write_fails() {
        let directory = temp_directory("partial-write");
        let first_path = directory.join("First.mo");
        let second_path = directory.join("Second.mo");
        let first_source = "model First\n  Real value;\nend First;\n";
        let second_source = "model Second\n  Real value;\nend Second;\n";
        let first_updated = "model First\n  Real changed;\nend First;\n";
        let second_updated = "model Second\n  Real changed;\nend Second;\n";
        fs::write(&first_path, first_source).unwrap();
        fs::write(&second_path, second_source).unwrap();

        let mut document = LoadedDocument {
            path: first_path.clone(),
            package_name: "PartialSaveTest".to_owned(),
            class_names: Vec::new(),
            model_tree: TreeNode::default(),
            diagnostics: 0,
            registry: RefCell::new(LibraryRegistry::default()),
            icon_cache: HashMap::new(),
            diagram_cache: HashMap::new(),
            scene_resolution_stats: RefCell::new(SceneResolutionStats::default()),
            class_sources: vec![
                ClassSource {
                    qualified_name: "First".to_owned(),
                    source_file: first_path.clone(),
                    source_range: SourceRange::new(0, first_source.len()),
                },
                ClassSource {
                    qualified_name: "Second".to_owned(),
                    source_file: second_path.clone(),
                    source_range: SourceRange::new(0, second_source.len()),
                },
            ],
            source_overrides: HashMap::from([
                ("First".to_owned(), first_updated.to_owned()),
                ("Second".to_owned(), second_updated.to_owned()),
            ]),
            saved_class_text: HashMap::from([
                ("First".to_owned(), first_source.to_owned()),
                ("Second".to_owned(), second_source.to_owned()),
            ]),
            source_versions: HashMap::new(),
        };

        let plan = build_save_plan(&document).expect("save plan should validate");
        let error = apply_save_plan(&mut document, plan, |path, contents| {
            if path == second_path {
                Err("simulated write failure".to_owned())
            } else {
                write_file_atomic(path, contents)
            }
        })
        .expect_err("the second file should fail");

        assert!(
            error.contains("save partially completed"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("1 of 2 file(s) saved"),
            "unexpected error: {error}"
        );
        assert!(error.contains("Second.mo"), "unexpected error: {error}");
        assert!(
            error.contains("simulated write failure"),
            "unexpected error: {error}"
        );
        assert_eq!(fs::read_to_string(&first_path).unwrap(), first_updated);
        assert_eq!(fs::read_to_string(&second_path).unwrap(), second_source);
        assert!(!document.source_overrides.contains_key("First"));
        assert_eq!(
            document.source_overrides.get("Second"),
            Some(&second_updated.to_owned())
        );
        assert_eq!(
            document.saved_class_text.get("First"),
            Some(&first_updated.trim_end_matches('\n').to_owned())
        );
        assert_eq!(
            document.saved_class_text.get("Second"),
            Some(&second_source.to_owned())
        );

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn save_rejects_conflicting_nested_class_edits() {
        let directory = temp_directory("plan-overlap");
        let path = directory.join("Nested.mo");
        let source = "model Parent\n  model Child\n    Real value;\n  end Child;\nend Parent;\n";
        fs::write(&path, source).unwrap();
        let parent_start = source.find("model Parent").unwrap();
        let parent_end = source.len();
        let child_start = source.find("model Child").unwrap();
        let child_end = source.find("end Child;").unwrap() + "end Child;".len();
        let parent_text = source[parent_start..parent_end].to_owned();
        let child_text = source[child_start..child_end].to_owned();
        let parent_override = parent_text.replace("Real value;", "Real parent_value;");
        let child_override = child_text.replace("Real value;", "Real child_value;");
        let mut document = LoadedDocument {
            path: path.clone(),
            package_name: "OverlapTest".to_owned(),
            class_names: Vec::new(),
            model_tree: TreeNode::default(),
            diagnostics: 0,
            registry: RefCell::new(LibraryRegistry::default()),
            icon_cache: HashMap::new(),
            diagram_cache: HashMap::new(),
            scene_resolution_stats: RefCell::new(SceneResolutionStats::default()),
            class_sources: vec![
                ClassSource {
                    qualified_name: "Parent".to_owned(),
                    source_file: path.clone(),
                    source_range: SourceRange::new(parent_start, parent_end),
                },
                ClassSource {
                    qualified_name: "Parent.Child".to_owned(),
                    source_file: path.clone(),
                    source_range: SourceRange::new(child_start, child_end),
                },
            ],
            source_overrides: HashMap::from([
                ("Parent".to_owned(), parent_override),
                ("Parent.Child".to_owned(), child_override),
            ]),
            saved_class_text: HashMap::from([
                ("Parent".to_owned(), parent_text),
                ("Parent.Child".to_owned(), child_text),
            ]),
            source_versions: HashMap::new(),
        };

        let error =
            save_edited_classes(&mut document).expect_err("nested conflict must be rejected");
        assert!(
            error.contains("cannot merge nested edits"),
            "unexpected error: {error}"
        );
        assert!(error.contains("Parent"), "parent name missing: {error}");
        assert!(
            error.contains("Parent.Child"),
            "child name missing: {error}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn child_then_parent_preserves_both_nested_changes() {
        let (mut document, path, _) = nested_document("nested-child-parent");
        let child_base = document.class_text("Parent.Child").unwrap();
        let parent_base = document.class_text("Parent").unwrap();
        let child_changed = child_base.replace("Real child_value;", "Real child_changed;");
        let parent_changed = parent_base.replace("Real parent_value;", "Real parent_changed;");

        document.set_class_text("Parent.Child", child_changed);
        document.set_class_text("Parent", parent_changed);
        assert_eq!(
            save_edited_classes(&mut document).expect("nested edits should save"),
            1
        );
        let saved = fs::read_to_string(&path).unwrap();
        assert_candidate_contains(&saved, &["Real parent_changed;", "Real child_changed;"]);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn parent_then_child_preserves_both_nested_changes() {
        let (mut document, path, _) = nested_document("nested-parent-child");
        let child_base = document.class_text("Parent.Child").unwrap();
        let parent_base = document.class_text("Parent").unwrap();
        let child_changed = child_base.replace("Real child_value;", "Real child_changed;");
        let parent_changed = parent_base.replace("Real parent_value;", "Real parent_changed;");

        document.set_class_text("Parent", parent_changed);
        document.set_class_text("Parent.Child", child_changed);
        let plan = build_save_plan(&document).expect("nested edits should merge");
        assert_eq!(plan.len(), 1);
        assert_candidate_contains(
            &plan[0].contents,
            &["Real parent_changed;", "Real child_changed;"],
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn non_conflicting_parent_and_child_edits_are_merged() {
        let (mut document, path, _) = nested_document("nested-non-conflicting");
        let child_base = document.class_text("Parent.Child").unwrap();
        let parent_base = document.class_text("Parent").unwrap();
        document.set_class_text(
            "Parent",
            parent_base.replace("Real parent_value;", "Real parent_changed;"),
        );
        document.set_class_text(
            "Parent.Child",
            child_base.replace("Real child_value;", "Real child_changed;"),
        );
        let candidate =
            compose_current_file_source(&document, &path, &fs::read_to_string(&path).unwrap())
                .expect("non-conflicting edits should merge");
        assert_candidate_contains(&candidate, &["Real parent_changed;", "Real child_changed;"]);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn parent_already_containing_child_change_is_not_duplicated() {
        let (mut document, path, _) = nested_document("nested-already-merged");
        let child_base = document.class_text("Parent.Child").unwrap();
        let parent_base = document.class_text("Parent").unwrap();
        let child_changed = child_base.replace("Real child_value;", "Real child_changed;");
        let parent_changed = parent_base
            .replace("Real parent_value;", "Real parent_changed;")
            .replace(&child_base, &child_changed);

        document.set_class_text("Parent.Child", child_changed.clone());
        document.set_class_text("Parent", parent_changed);
        let candidate =
            compose_current_file_source(&document, &path, &fs::read_to_string(&path).unwrap())
                .expect("already merged child should be accepted");
        assert_eq!(candidate.matches("Real child_changed;").count(), 1);
        assert!(candidate.contains("Real parent_changed;"));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn nested_registry_source_matches_save_candidate() {
        let (mut document, path, _) = nested_document("nested-registry-candidate");
        let child_base = document.class_text("Parent.Child").unwrap();
        let parent_base = document.class_text("Parent").unwrap();
        document.set_class_text(
            "Parent.Child",
            child_base.replace("Real child_value;", "Real child_changed;"),
        );
        document.set_class_text(
            "Parent",
            parent_base.replace("Real parent_value;", "Real parent_changed;"),
        );
        let disk = fs::read_to_string(&path).unwrap();
        let candidate = build_save_plan(&document).expect("save candidate");
        let registry = document
            .registry
            .borrow()
            .source(&path)
            .expect("composed source in registry")
            .to_owned();
        assert_eq!(candidate.len(), 1);
        assert_eq!(candidate[0].contents, registry);
        assert_eq!(
            candidate[0].contents,
            compose_current_file_source(&document, &path, &disk).unwrap()
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn three_level_nested_edits_merge_from_grandchild_outward() {
        let (mut document, path, _) = nested_document("nested-three-level");
        let grandchild_base = document.class_text("Parent.Child.Grandchild").unwrap();
        let child_base = document.class_text("Parent.Child").unwrap();
        let parent_base = document.class_text("Parent").unwrap();
        document.set_class_text(
            "Parent.Child.Grandchild",
            grandchild_base.replace("Real grand_value;", "Real grand_changed;"),
        );
        document.set_class_text(
            "Parent.Child",
            child_base.replace("Real child_value;", "Real child_changed;"),
        );
        document.set_class_text(
            "Parent",
            parent_base.replace("Real parent_value;", "Real parent_changed;"),
        );
        let plan = build_save_plan(&document).expect("three-level edits should merge");
        assert_candidate_contains(
            &plan[0].contents,
            &[
                "Real parent_changed;",
                "Real child_changed;",
                "Real grand_changed;",
            ],
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_updates_following_class_ranges_after_length_change() {
        let directory = temp_directory("updated-ranges");
        let source = "model A\n  Real value;\nend A;\n\nmodel B\n  Real value;\nend B;\n";
        let a_source = "model A\n  Real value;\nend A;";
        let b_source = "model B\n  Real value;\nend B;";
        let (mut document, path) = temp_document(
            &directory,
            "Ranges.mo",
            source,
            &[
                ("A", a_source, "model A\n  Real value = 123456789;\nend A;"),
                ("B", b_source, "model B\n  Real value = 2;\nend B;"),
            ],
        );
        document.source_overrides.remove("B");

        assert_eq!(save_edited_classes(&mut document).expect("first save"), 1);
        let disk_after_first = fs::read_to_string(&path).unwrap();
        let b_range = document
            .class_sources
            .iter()
            .find(|class| class.qualified_name == "B")
            .map(|class| class.source_range)
            .expect("updated B range");
        assert_eq!(&disk_after_first[b_range.start..b_range.end], b_source);
        assert_eq!(
            document
                .registry
                .borrow()
                .resolve("B")
                .expect("registry B range")
                .source_range,
            b_range
        );

        document.source_overrides.remove("A");
        document.source_overrides.insert(
            "B".to_owned(),
            "model B\n  Real value = 222222222;\nend B;".to_owned(),
        );
        assert_eq!(save_edited_classes(&mut document).expect("second save"), 1);
        let disk_after_second = fs::read_to_string(&path).unwrap();
        assert!(disk_after_second.contains("Real value = 123456789;"));
        assert!(disk_after_second.contains("Real value = 222222222;"));
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
            model_tree: TreeNode::default(),
            diagnostics: 0,
            registry: RefCell::new(LibraryRegistry::default()),
            icon_cache: HashMap::new(),
            diagram_cache: HashMap::new(),
            scene_resolution_stats: RefCell::new(SceneResolutionStats::default()),
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

#[derive(Clone, Debug)]
enum PendingDocumentAction {
    Open(PathBuf),
    Close,
}

impl PendingDocumentAction {
    fn description(&self) -> &'static str {
        match self {
            Self::Open(_) => "打开另一个文档",
            Self::Close => "关闭窗口",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaveDecision {
    Save,
    Discard,
    Cancel,
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum ConnectionEndpointConstraint {
    Semantic { lhs: CorePoint, rhs: CorePoint },
    FixedExisting { lhs: CorePoint, rhs: CorePoint },
}

impl ConnectionEndpointConstraint {
    fn points(self) -> (CorePoint, CorePoint) {
        match self {
            Self::Semantic { lhs, rhs } | Self::FixedExisting { lhs, rhs } => (lhs, rhs),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Semantic { .. } => "Semantic",
            Self::FixedExisting { .. } => "FixedExisting",
        }
    }
}

fn connection_edit_diagnostic(
    connection: &modelica_core::scene::DiagramConnection,
    selected_class: &str,
    endpoint_constraint: Option<ConnectionEndpointConstraint>,
    stage: &str,
    detail: impl std::fmt::Display,
) -> String {
    let line_endpoints = connection.line.as_ref().and_then(|line| {
        line.points
            .first()
            .copied()
            .zip(line.points.last().copied())
    });
    let (lhs, rhs) = endpoint_constraint
        .map(ConnectionEndpointConstraint::points)
        .or(line_endpoints)
        .map_or((None, None), |(lhs, rhs)| (Some(lhs), Some(rhs)));
    let constraint = endpoint_constraint.map_or("Unavailable", |value| value.label());
    format!(
        "Connection edit {stage} failed: {detail}; id={}; key={:?}; owner={}; selected={}; lhs={:?}; rhs={:?}; line_source_range={:?}; constraint={constraint}",
        connection.id,
        connection.key,
        connection.key.owner_class,
        selected_class,
        lhs,
        rhs,
        connection.line_source_range,
    )
}

fn trace_connection_edit(
    stage: &str,
    connection_key: &ConnectionKey,
    endpoint_constraint: ConnectionEndpointConstraint,
    detail: impl std::fmt::Display,
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_CONNECTION_EDIT").is_some() {
        eprintln!(
            "[CONNECTION EDIT] stage={stage} key={connection_key:?} constraint={} {detail}",
            endpoint_constraint.label(),
        );
    }
}

fn trace_component_edit(
    stage: &str,
    component_id: &str,
    component_name: &str,
    detail: impl std::fmt::Display,
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_COMPONENT_EDIT").is_some()
        || std::env::var_os("MODELICA_WGPU_TRACE_CONNECTION_EDIT").is_some()
    {
        eprintln!(
            "[COMPONENT EDIT] stage={stage} id={component_id} name={component_name} {detail}"
        );
    }
}

fn trace_component_drag_issue(
    component_id: &str,
    connection_key: &ConnectionKey,
    reason: &str,
    snapshot: &ConnectionDragSnapshot,
    preview_points: &[CorePoint],
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_COMPONENT_DRAG").is_some() {
        eprintln!(
            "[COMPONENT DRAG] component_id={component_id} connection_key={connection_key:?} reason={reason} source_points={:?} base_route={:?} preview_points={preview_points:?} first_anchor={:?} last_anchor={:?} moved_first={} moved_last={}",
            snapshot.source_line_points,
            snapshot.base_route_points,
            snapshot.original_endpoint_points.0,
            snapshot.original_endpoint_points.1,
            snapshot.moved_first_endpoint,
            snapshot.moved_last_endpoint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn trace_cancel_profile(
    interaction: &str,
    connections: usize,
    total: Duration,
    rollback_component: Duration,
    rollback_connections: Duration,
    preview_cleanup: Duration,
    selection_update: Duration,
    hit_cache_rebuild: Duration,
    scene_rebuild: Duration,
    gpu_upload: Duration,
) {
    if std::env::var_os("MODELICA_WGPU_PROFILE_CANCEL").is_none() {
        return;
    }
    let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.0;
    eprintln!(
        "[CANCEL PROFILE] interaction={interaction} connections={connections} total_us={:.1} rollback_component_us={:.1} rollback_connections_us={:.1} preview_cleanup_us={:.1} selection_update_us={:.1} hit_cache_rebuild_us={:.1} scene_rebuild_us={:.1} gpu_upload_us={:.1}",
        micros(total),
        micros(rollback_component),
        micros(rollback_connections),
        micros(preview_cleanup),
        micros(selection_update),
        micros(hit_cache_rebuild),
        micros(scene_rebuild),
        micros(gpu_upload),
    );
}

#[derive(Clone, Copy, Debug)]
struct PendingCancelProfile {
    generation: u64,
    started: Instant,
    cancel_completed_at: Instant,
    interaction: &'static str,
    connection_count: usize,
    preview_stats: PreviewResourceStats,
    preview_cleanup: Duration,
    cancel_cpu: Duration,
    first_redraw_at: Option<Instant>,
    redraw_count: u32,
}

#[derive(Clone, Debug)]
struct DeselectSelectionMetadata {
    selected_kind: &'static str,
    selected_component_id: Option<String>,
    selected_component_graphic_count: usize,
    selected_component_port_count: usize,
}

#[derive(Debug)]
struct PendingDeselectProfile {
    generation: u64,
    started: Instant,
    selection_completed_at: Instant,
    selection_state: Duration,
    hover_update: Duration,
    selected_kind: &'static str,
    selected_component_id: Option<String>,
    selected_component_graphic_count: usize,
    selected_component_port_count: usize,
    first_redraw_at: Option<Instant>,
    request_count: u32,
    redraw_count: u32,
}

#[derive(Clone, Copy, Debug)]
struct DeselectRedrawTiming {
    generation: u64,
    first_redraw_at: Instant,
    request_count: u32,
    redraw_count: u32,
}

#[derive(Clone, Copy, Debug)]
struct DeselectFrameTiming {
    overlay_update: Duration,
    ui_build: Duration,
    egui_tessellation: Duration,
    scene_encode: Duration,
    queue_submit: Duration,
    present: Duration,
    frame_total: Duration,
}

#[derive(Clone, Copy, Debug)]
struct DeselectSample {
    selection_state: Duration,
    overlay_update: Duration,
    hover_update: Duration,
    ui_build: Duration,
    egui_tessellation: Duration,
    scene_encode: Duration,
    queue_submit: Duration,
    present: Duration,
    frame_total: Duration,
    end_to_end: Duration,
}

struct DeselectProfile {
    enabled: bool,
    samples: Vec<DeselectSample>,
}

impl DeselectProfile {
    fn new() -> Self {
        Self {
            enabled: std::env::var_os("MODELICA_WGPU_PROFILE_DESELECT").is_some(),
            samples: Vec::with_capacity(128),
        }
    }

    fn record(
        &mut self,
        pending: PendingDeselectProfile,
        redraw: DeselectRedrawTiming,
        frame: DeselectFrameTiming,
        finished_at: Instant,
    ) {
        if !self.enabled {
            return;
        }
        let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.0;
        let redraw_wait = redraw
            .first_redraw_at
            .saturating_duration_since(pending.selection_completed_at);
        let end_to_end = finished_at.saturating_duration_since(pending.started);
        eprintln!(
            "[DESELECT REDRAW] generation={} request_count={} redraw_count={}",
            redraw.generation, redraw.request_count, redraw.redraw_count,
        );
        eprintln!(
            "[DESELECT PROFILE] generation={} selected_kind={} selected_component_id={} selected_component_graphic_count={} selected_component_port_count={} selection_state_us={:.1} overlay_update_us={:.1} hover_update_us={:.1} ui_build_us={:.1} egui_tessellation_us={:.1} scene_encode_us={:.1} queue_submit_us={:.1} present_us={:.1} frame_total_us={:.1} end_to_end_us={:.1} redraw_wait_us={:.1} request_count={} redraw_count={}",
            pending.generation,
            pending.selected_kind,
            pending.selected_component_id.as_deref().unwrap_or("-"),
            pending.selected_component_graphic_count,
            pending.selected_component_port_count,
            micros(pending.selection_state),
            micros(frame.overlay_update),
            micros(pending.hover_update),
            micros(frame.ui_build),
            micros(frame.egui_tessellation),
            micros(frame.scene_encode),
            micros(frame.queue_submit),
            micros(frame.present),
            micros(frame.frame_total),
            micros(end_to_end),
            micros(redraw_wait),
            redraw.request_count,
            redraw.redraw_count,
        );
        self.samples.push(DeselectSample {
            selection_state: pending.selection_state,
            overlay_update: frame.overlay_update,
            hover_update: pending.hover_update,
            ui_build: frame.ui_build,
            egui_tessellation: frame.egui_tessellation,
            scene_encode: frame.scene_encode,
            queue_submit: frame.queue_submit,
            present: frame.present,
            frame_total: frame.frame_total,
            end_to_end,
        });
        if self.samples.len() >= 20 && self.samples.len().is_multiple_of(20) {
            self.report_summary();
        }
    }

    fn report_summary(&self) {
        let percentile = |mut values: Vec<Duration>, percent: f32| {
            values.sort_unstable();
            let index = ((values.len().saturating_sub(1)) as f32 * percent).round() as usize;
            values.get(index).copied().unwrap_or_default()
        };
        let summarize = |select: fn(&DeselectSample) -> Duration| {
            (
                percentile(self.samples.iter().map(select).collect(), 0.50),
                percentile(self.samples.iter().map(select).collect(), 0.95),
                percentile(self.samples.iter().map(select).collect(), 1.0),
            )
        };
        let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.0;
        let format_triplet = |triplet: (Duration, Duration, Duration)| {
            format!(
                "{:.1}/{:.1}/{:.1}",
                micros(triplet.0),
                micros(triplet.1),
                micros(triplet.2),
            )
        };
        eprintln!(
            "[DESELECT SUMMARY] samples={} selection_state_us={} overlay_update_us={} hover_update_us={} ui_build_us={} egui_tessellation_us={} scene_encode_us={} queue_submit_us={} present_us={} frame_total_us={} end_to_end_us={} (p50/p95/worst)",
            self.samples.len(),
            format_triplet(summarize(|sample| sample.selection_state)),
            format_triplet(summarize(|sample| sample.overlay_update)),
            format_triplet(summarize(|sample| sample.hover_update)),
            format_triplet(summarize(|sample| sample.ui_build)),
            format_triplet(summarize(|sample| sample.egui_tessellation)),
            format_triplet(summarize(|sample| sample.scene_encode)),
            format_triplet(summarize(|sample| sample.queue_submit)),
            format_triplet(summarize(|sample| sample.present)),
            format_triplet(summarize(|sample| sample.frame_total)),
            format_triplet(summarize(|sample| sample.end_to_end)),
        );
    }
}

#[derive(Clone, Copy, Debug)]
struct CancelRedrawTiming {
    generation: u64,
    first_redraw_at: Instant,
    redraw_count: u32,
    flush_drag_preview: Duration,
}

#[derive(Clone, Copy, Debug)]
struct CancelFrameTiming {
    ui: Duration,
    egui_tessellation: Duration,
    scene_encode: Duration,
    queue_submit: Duration,
    present: Duration,
    frame_total: Duration,
}

#[derive(Clone, Copy, Debug, Default)]
struct PreviewResourceStats {
    preview_count: usize,
    preview_buffer_count: usize,
    preview_segment_count: usize,
}

#[derive(Clone, Copy, Debug)]
struct CancelE2ESample {
    interaction: &'static str,
    connection_count: usize,
    cancel_cpu: Duration,
    first_frame: Duration,
    end_to_end: Duration,
    preview_cleanup: Duration,
    scene_encode: Duration,
    queue_submit: Duration,
    present: Duration,
}

struct CancelE2EProfile {
    enabled: bool,
    samples: Vec<CancelE2ESample>,
}

impl CancelE2EProfile {
    fn new() -> Self {
        Self {
            enabled: std::env::var_os("MODELICA_WGPU_PROFILE_CANCEL").is_some(),
            samples: Vec::with_capacity(32),
        }
    }

    fn record(
        &mut self,
        pending: PendingCancelProfile,
        redraw: CancelRedrawTiming,
        frame: CancelFrameTiming,
        finished_at: Instant,
    ) {
        if !self.enabled {
            return;
        }
        let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.0;
        let input_to_cancel_end = pending
            .cancel_completed_at
            .saturating_duration_since(pending.started);
        let redraw_wait = redraw
            .first_redraw_at
            .saturating_duration_since(pending.cancel_completed_at);
        let first_frame = redraw_wait + frame.frame_total;
        let end_to_end = finished_at.saturating_duration_since(pending.started);
        eprintln!(
            "[CANCEL REDRAW] generation={} redraw_count={} redraw_wait_us={:.1}",
            redraw.generation,
            redraw.redraw_count,
            micros(redraw_wait),
        );
        eprintln!(
            "[CANCEL E2E] generation={} interaction={} connections={} preview_count={} preview_buffer_count={} preview_segment_count={} input_to_cancel_end_us={:.1} cancel_cpu_us={:.1} preview_cleanup_us={:.1} redraw_wait_us={:.1} flush_drag_preview_us={:.1} ui_us={:.1} scene_encode_us={:.1} queue_submit_us={:.1} present_us={:.1} frame_total_us={:.1} first_frame_us={:.1} end_to_end_us={:.1} redraw_count={}",
            pending.generation,
            pending.interaction,
            pending.connection_count,
            pending.preview_stats.preview_count,
            pending.preview_stats.preview_buffer_count,
            pending.preview_stats.preview_segment_count,
            micros(input_to_cancel_end),
            micros(pending.cancel_cpu),
            micros(pending.preview_cleanup),
            micros(redraw_wait),
            micros(redraw.flush_drag_preview),
            micros(frame.ui + frame.egui_tessellation),
            micros(frame.scene_encode),
            micros(frame.queue_submit),
            micros(frame.present),
            micros(frame.frame_total),
            micros(first_frame),
            micros(end_to_end),
            redraw.redraw_count,
        );
        self.samples.push(CancelE2ESample {
            interaction: pending.interaction,
            connection_count: pending.connection_count,
            cancel_cpu: pending.cancel_cpu,
            first_frame,
            end_to_end,
            preview_cleanup: pending.preview_cleanup,
            scene_encode: frame.scene_encode,
            queue_submit: frame.queue_submit,
            present: frame.present,
        });
        if self.samples.len() >= 20 && self.samples.len().is_multiple_of(20) {
            self.report_summary();
        }
    }

    fn report_summary(&self) {
        let mut samples = self
            .samples
            .iter()
            .map(|sample| sample.end_to_end)
            .collect::<Vec<_>>();
        samples.sort_unstable();
        let percentile = |percent: f32| {
            let index = ((samples.len().saturating_sub(1)) as f32 * percent).round() as usize;
            samples.get(index).copied().unwrap_or_default()
        };
        let micros = |duration: Duration| duration.as_secs_f64() * 1_000_000.0;
        eprintln!(
            "[CANCEL E2E SUMMARY] samples={} p50_us={:.1} p95_us={:.1} worst_us={:.1}",
            samples.len(),
            micros(percentile(0.50)),
            micros(percentile(0.95)),
            micros(percentile(1.0)),
        );
        let mut connection_counts = self
            .samples
            .iter()
            .map(|sample| sample.connection_count)
            .collect::<Vec<_>>();
        connection_counts.sort_unstable();
        connection_counts.dedup();
        for connection_count in connection_counts {
            let group = self
                .samples
                .iter()
                .filter(|sample| sample.connection_count == connection_count)
                .collect::<Vec<_>>();
            let percentile = |values: Vec<Duration>, percent: f32| {
                let mut values = values;
                values.sort_unstable();
                let index = ((values.len().saturating_sub(1)) as f32 * percent).round() as usize;
                values.get(index).copied().unwrap_or_default()
            };
            let summarize = |select: fn(&CancelE2ESample) -> Duration| {
                (
                    percentile(group.iter().map(|sample| select(sample)).collect(), 0.50),
                    percentile(group.iter().map(|sample| select(sample)).collect(), 0.95),
                    percentile(group.iter().map(|sample| select(sample)).collect(), 1.0),
                )
            };
            let (cancel_p50, cancel_p95, cancel_worst) = summarize(|sample| sample.cancel_cpu);
            let (first_p50, first_p95, first_worst) = summarize(|sample| sample.first_frame);
            let (e2e_p50, e2e_p95, e2e_worst) = summarize(|sample| sample.end_to_end);
            let (cleanup_p50, cleanup_p95, cleanup_worst) =
                summarize(|sample| sample.preview_cleanup);
            let (encode_p50, encode_p95, encode_worst) = summarize(|sample| sample.scene_encode);
            let (submit_p50, submit_p95, submit_worst) = summarize(|sample| sample.queue_submit);
            let (present_p50, present_p95, present_worst) = summarize(|sample| sample.present);
            eprintln!(
                "[CANCEL SCALE] connections={} samples={} cancel_cpu_us={:.1}/{:.1}/{:.1} first_frame_us={:.1}/{:.1}/{:.1} end_to_end_us={:.1}/{:.1}/{:.1} preview_cleanup_us={:.1}/{:.1}/{:.1} scene_encode_us={:.1}/{:.1}/{:.1} queue_submit_us={:.1}/{:.1}/{:.1} present_us={:.1}/{:.1}/{:.1}",
                connection_count,
                group.len(),
                micros(cancel_p50),
                micros(cancel_p95),
                micros(cancel_worst),
                micros(first_p50),
                micros(first_p95),
                micros(first_worst),
                micros(e2e_p50),
                micros(e2e_p95),
                micros(e2e_worst),
                micros(cleanup_p50),
                micros(cleanup_p95),
                micros(cleanup_worst),
                micros(encode_p50),
                micros(encode_p95),
                micros(encode_worst),
                micros(submit_p50),
                micros(submit_p95),
                micros(submit_worst),
                micros(present_p50),
                micros(present_p95),
                micros(present_worst),
            );
        }
        for interaction in [
            "MoveDiagramComponent",
            "ResizeDiagramComponent",
            "MoveDiagramConnectionSegment",
            "MoveDiagramConnectionCorner",
            "CreateDiagramConnection",
            "DeselectDiagram",
        ] {
            let mut interaction_samples = self
                .samples
                .iter()
                .filter(|sample| sample.interaction == interaction)
                .map(|sample| sample.end_to_end)
                .collect::<Vec<_>>();
            if interaction_samples.is_empty() {
                continue;
            }
            interaction_samples.sort_unstable();
            let percentile = |percent: f32| {
                let index = ((interaction_samples.len().saturating_sub(1)) as f32 * percent).round()
                    as usize;
                interaction_samples.get(index).copied().unwrap_or_default()
            };
            eprintln!(
                "[CANCEL E2E SUMMARY] interaction={} samples={} p50_us={:.1} p95_us={:.1} worst_us={:.1}",
                interaction,
                interaction_samples.len(),
                micros(percentile(0.50)),
                micros(percentile(0.95)),
                micros(percentile(1.0)),
            );
        }
    }
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
                let Some(distance) = connector_anchor_active_hit_distance(anchor, point, tolerance)
                else {
                    continue;
                };
                if nearest.is_none_or(|(best_distance, best_index)| {
                    let Some(best_anchor) = anchors.get(best_index) else {
                        return true;
                    };
                    compare_connector_anchor_hits(distance, anchor, best_distance, best_anchor)
                        == std::cmp::Ordering::Less
                }) {
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
                && connector_anchor_active_hit_distance(anchor, pointer_model, exit_tolerance)
                    .is_some()
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
        preview_origin: CorePoint,
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
        endpoint_constraint: ConnectionEndpointConstraint,
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
        endpoint_constraint: ConnectionEndpointConstraint,
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
    source_editable: bool,
    source_edit_error: Option<String>,
    source_line_points: Vec<CorePoint>,
    base_route_points: Vec<CorePoint>,
    original_line_origin: CorePoint,
    original_line_rotation: f32,
    original_endpoint_points: (CorePoint, CorePoint),
    preview_points: Vec<CorePoint>,
    preview_route_valid: bool,
    moved_first_endpoint: bool,
    moved_last_endpoint: bool,
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
        endpoint_constraint: ConnectionEndpointConstraint,
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

fn record_successful_edit(
    history: &mut Vec<EditCommand>,
    redo_history: &mut Vec<EditCommand>,
    command: EditCommand,
) {
    history.push(command);
    redo_history.clear();
}

fn reset_edit_history(history: &mut Vec<EditCommand>, redo_history: &mut Vec<EditCommand>) {
    history.clear();
    redo_history.clear();
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

struct LoadedDocument {
    path: PathBuf,
    package_name: String,
    class_names: Vec<String>,
    model_tree: TreeNode,
    diagnostics: usize,
    registry: RefCell<LibraryRegistry>,
    icon_cache: HashMap<String, OnceLock<Option<CoreIconScene>>>,
    diagram_cache: HashMap<String, OnceLock<Option<CoreDiagramScene>>>,
    scene_resolution_stats: RefCell<SceneResolutionStats>,
    class_sources: Vec<ClassSource>,
    source_overrides: HashMap<String, String>,
    // Text the parser saw when the document was loaded (or what we last wrote
    // to disk). Saving verifies the on-disk slice still matches before it
    // applies an edit, so an externally modified file is never overwritten.
    saved_class_text: HashMap<String, String>,
    source_versions: HashMap<String, u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SceneResolutionStats {
    icon_resolve_count: u64,
    icon_resolve_time: Duration,
    diagram_resolve_count: u64,
    diagram_resolve_time: Duration,
}

#[derive(Clone, Copy, Debug, Default)]
struct LibraryLoadStats {
    package_load_time: Duration,
    tree_build_time: Duration,
    registry_build_time: Duration,
    total_time: Duration,
    file_count: usize,
    class_count: usize,
}

#[derive(Clone, Debug)]
struct ClassSource {
    qualified_name: String,
    source_file: PathBuf,
    source_range: SourceRange,
}

#[derive(Clone, Debug, Default)]
struct TreeNode {
    name: String,
    qualified_name: String,
    class_name: Option<String>,
    kind: Option<ClassKind>,
    // Kept as model metadata for future properties, tooltip, search, or docs
    // views; the model tree intentionally does not render it inline.
    #[allow(dead_code)]
    description: Option<String>,
    children: Vec<TreeNode>,
}

#[derive(Clone, Debug)]
struct UiDocument {
    package_name: String,
    class_names: Vec<String>,
    full_class_names: HashSet<String>,
    short_class_names: HashSet<String>,
    dirty: bool,
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
    source_max_line_chars: usize,
    source_version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceHighlightCacheKey {
    selected_class: Option<String>,
    source_version: u64,
    dark_theme: bool,
    accent_theme: AccentTheme,
    pixels_per_point_bits: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum FoldDelimiter {
    Parenthesis,
    Brace,
    Bracket,
}

impl FoldDelimiter {
    fn closing(self) -> &'static str {
        match self {
            Self::Parenthesis => ")",
            Self::Brace => "}",
            Self::Bracket => "]",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum SourceFoldKind {
    Annotation,
    Call,
    Array,
    Generic,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct FoldId {
    selected_class: String,
    source_version: u64,
    open_start: usize,
    close_end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceFoldRange {
    id: FoldId,
    start_line: usize,
    end_line: usize,
    open_token: FoldDelimiter,
    kind: Option<SourceFoldKind>,
    open_end_column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VisibleSourceRow {
    original_line: usize,
    fold_range_index: Option<usize>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct VisibleSourceRows {
    rows: Vec<VisibleSourceRow>,
}

#[derive(Default)]
struct SourceFoldState {
    selected_class: Option<String>,
    source_version: u64,
    initialized: bool,
    ranges: Vec<SourceFoldRange>,
    collapsed: HashSet<FoldId>,
    range_by_start_line: HashMap<usize, usize>,
    range_by_id: HashMap<FoldId, usize>,
    visible_rows: VisibleSourceRows,
    fold_layout_rebuild_count: u64,
    fold_layout_rebuild_time: Duration,
    fold_sync_time: Duration,
}

impl SourceFoldState {
    fn sync_document(&mut self, document: &UiDocument) {
        let selected_class = document.selected_class.clone();
        let new_class = !self.initialized || self.selected_class != selected_class;
        if self.initialized
            && self.selected_class == selected_class
            && self.source_version == document.source_version
        {
            return;
        }

        let sync_started = Instant::now();
        let previous_collapsed = std::mem::take(&mut self.collapsed);
        self.selected_class = selected_class;
        self.source_version = document.source_version;
        self.ranges = discover_source_fold_ranges(
            self.selected_class.as_deref(),
            self.source_version,
            &document.source_lines,
        );
        let current_ids = self
            .ranges
            .iter()
            .map(|range| range.id.clone())
            .collect::<HashSet<_>>();
        self.collapsed = if new_class {
            current_ids
        } else {
            previous_collapsed
                .into_iter()
                .filter(|id| current_ids.contains(id))
                .collect()
        };
        self.initialized = true;
        self.rebuild_fold_layout(document.source_lines.len());
        self.fold_sync_time += sync_started.elapsed();
    }

    fn rebuild_fold_layout(&mut self, line_count: usize) {
        let started = Instant::now();
        self.range_by_start_line.clear();
        self.range_by_id.clear();
        for (index, range) in self.ranges.iter().enumerate() {
            self.range_by_id.insert(range.id.clone(), index);
            let replace = self
                .range_by_start_line
                .get(&range.start_line)
                .and_then(|current| self.ranges.get(*current))
                .is_none_or(|current| {
                    (range.end_line, range.id.open_start)
                        > (current.end_line, current.id.open_start)
                });
            if replace {
                self.range_by_start_line.insert(range.start_line, index);
            }
        }
        let mut collapsed_at_start = HashMap::<usize, usize>::new();
        for (index, range) in self.ranges.iter().enumerate() {
            if !self.collapsed.contains(&range.id) {
                continue;
            }
            let replace = collapsed_at_start
                .get(&range.start_line)
                .and_then(|current| self.ranges.get(*current))
                .is_none_or(|current| {
                    (range.end_line, range.id.open_start)
                        > (current.end_line, current.id.open_start)
                });
            if replace {
                collapsed_at_start.insert(range.start_line, index);
            }
        }
        let mut rows = Vec::with_capacity(line_count);
        let mut line = 0;
        while line < line_count {
            if let Some(&range_index) = collapsed_at_start.get(&line) {
                let range = &self.ranges[range_index];
                rows.push(VisibleSourceRow {
                    original_line: line,
                    fold_range_index: Some(range_index),
                });
                line = range.end_line.saturating_add(1).min(line_count);
            } else {
                rows.push(VisibleSourceRow {
                    original_line: line,
                    fold_range_index: None,
                });
                line += 1;
            }
        }
        self.visible_rows = VisibleSourceRows { rows };
        self.fold_layout_rebuild_count += 1;
        self.fold_layout_rebuild_time += started.elapsed();
    }

    fn range_starting_at(&self, line: usize) -> Option<&SourceFoldRange> {
        self.range_by_start_line
            .get(&line)
            .and_then(|index| self.ranges.get(*index))
    }

    fn toggle_line(&mut self, line: usize, line_count: usize) -> bool {
        let Some(id) = self.range_starting_at(line).map(|range| range.id.clone()) else {
            return false;
        };
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
        self.rebuild_fold_layout(line_count);
        true
    }
}

fn source_fold_delimiter(text: &str) -> Option<FoldDelimiter> {
    match text {
        "(" => Some(FoldDelimiter::Parenthesis),
        "{" => Some(FoldDelimiter::Brace),
        "[" => Some(FoldDelimiter::Bracket),
        _ => None,
    }
}

fn closing_fold_delimiter(text: &str) -> Option<FoldDelimiter> {
    match text {
        ")" => Some(FoldDelimiter::Parenthesis),
        "}" => Some(FoldDelimiter::Brace),
        "]" => Some(FoldDelimiter::Bracket),
        _ => None,
    }
}

fn source_fold_kind(
    tokens: &[Token],
    open_index: usize,
    delimiter: FoldDelimiter,
) -> Option<SourceFoldKind> {
    let previous = tokens[..open_index]
        .iter()
        .rev()
        .find(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment));
    let name = previous.map(|token| token.text.as_str());
    Some(match name {
        Some("annotation") => SourceFoldKind::Annotation,
        Some(
            "Icon" | "Diagram" | "Placement" | "transformation" | "iconTransformation" | "Line"
            | "Polygon" | "Rectangle" | "Ellipse" | "Text" | "Bitmap",
        ) => SourceFoldKind::Call,
        _ if delimiter == FoldDelimiter::Brace => SourceFoldKind::Array,
        _ => SourceFoldKind::Generic,
    })
}

fn source_line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(source.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

fn source_line_for_offset(starts: &[usize], offset: usize) -> usize {
    match starts.binary_search(&offset) {
        Ok(line) => line,
        Err(line) => line.saturating_sub(1),
    }
}

fn discover_source_fold_ranges(
    selected_class: Option<&str>,
    source_version: u64,
    source_lines: &[String],
) -> Vec<SourceFoldRange> {
    let source = source_lines.join("\n");
    let starts = source_line_starts(&source);
    let tokens = tokenize(&source);
    let mut stack = Vec::<(FoldDelimiter, usize)>::new();
    let mut ranges = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Punctuation {
            continue;
        }
        if let Some(delimiter) = source_fold_delimiter(&token.text) {
            stack.push((delimiter, index));
            continue;
        }
        let Some(delimiter) = closing_fold_delimiter(&token.text) else {
            continue;
        };
        let Some((open_delimiter, open_index)) = stack.pop() else {
            continue;
        };
        if open_delimiter != delimiter {
            continue;
        }
        let open_token = &tokens[open_index];
        let start_line = source_line_for_offset(&starts, open_token.start);
        let end_line = source_line_for_offset(&starts, token.start);
        if end_line <= start_line {
            continue;
        }
        let line_start = starts.get(start_line).copied().unwrap_or_default();
        ranges.push(SourceFoldRange {
            id: FoldId {
                selected_class: selected_class.unwrap_or_default().to_owned(),
                source_version,
                open_start: open_token.start,
                close_end: token.end,
            },
            start_line,
            end_line,
            open_token: delimiter,
            kind: source_fold_kind(&tokens, open_index, delimiter),
            open_end_column: open_token.end.saturating_sub(line_start),
        });
    }
    ranges.sort_by_key(|range| (range.start_line, range.end_line, range.id.open_start));
    ranges
}

fn collapsed_source_line(line: &str, range: &SourceFoldRange) -> String {
    let prefix_end = range.open_end_column.min(line.len());
    format!("{} … {}", &line[..prefix_end], range.open_token.closing())
}

#[derive(Default)]
struct SourceLineCache {
    number_galley: Option<Arc<egui::Galley>>,
    code_galley: Option<Arc<egui::Galley>>,
}

#[derive(Default)]
struct SourceHighlightCache {
    key: Option<SourceHighlightCacheKey>,
    lines: Vec<SourceLineCache>,
    collapsed_lines: HashMap<FoldId, Arc<egui::Galley>>,
    fold_marker_galleys: Option<(Arc<egui::Galley>, Arc<egui::Galley>)>,
    max_line_width: f32,
    cache_hits: usize,
    cache_misses: usize,
}

impl SourceHighlightCache {
    fn prepare(&mut self, document: &UiDocument, pixels_per_point: f32) {
        let key = SourceHighlightCacheKey {
            selected_class: document.selected_class.clone(),
            source_version: document.source_version,
            dark_theme: is_dark_theme(),
            accent_theme: active_accent(),
            pixels_per_point_bits: pixels_per_point.to_bits(),
        };
        if self.key.as_ref() != Some(&key) || self.lines.len() != document.source_lines.len() {
            self.key = Some(key);
            self.lines = (0..document.source_lines.len())
                .map(|_| SourceLineCache::default())
                .collect();
            self.collapsed_lines.clear();
            self.fold_marker_galleys = None;
            self.max_line_width = 0.0;
            self.cache_hits = 0;
            self.cache_misses = 0;
        }
    }

    fn content_width(&mut self, ui: &egui::Ui, document: &UiDocument) -> f32 {
        self.prepare(document, ui.ctx().pixels_per_point());
        if self.max_line_width == 0.0 {
            let probe = ui.painter().layout_no_wrap(
                "M".to_owned(),
                ui_mono_font(12.0),
                Color32::PLACEHOLDER,
            );
            // Keep the horizontal extent stable before long or Unicode rows
            // become visible during a virtualized scroll.
            self.max_line_width =
                document.source_max_line_chars as f32 * probe.size().x * 1.25 + 8.0;
        }
        (SOURCE_FOLD_GUTTER_WIDTH
            + SOURCE_LINE_NUMBER_WIDTH
            + SOURCE_LINE_GAP
            + self.max_line_width)
            .max(ui.available_width())
    }

    fn galleys(
        &mut self,
        ui: &egui::Ui,
        document: &UiDocument,
        row: usize,
    ) -> (Arc<egui::Galley>, Arc<egui::Galley>, Duration) {
        if let Some(line) = self.lines.get(row) {
            if let (Some(number_galley), Some(code_galley)) =
                (&line.number_galley, &line.code_galley)
            {
                self.cache_hits += 1;
                return (number_galley.clone(), code_galley.clone(), Duration::ZERO);
            }
        }
        let started = Instant::now();
        let job = modelica_layout_job(
            &document.source_lines[row],
            &document.full_class_names,
            &document.short_class_names,
        );
        let code_galley = ui.painter().layout_job(job);
        let number_galley = ui.painter().layout_no_wrap(
            format!("{:>4}", row + 1),
            ui_mono_font(13.0),
            theme_text_tertiary(),
        );
        let elapsed = started.elapsed();
        self.cache_misses += 1;
        self.max_line_width = self.max_line_width.max(code_galley.size().x);
        if let Some(line) = self.lines.get_mut(row) {
            line.number_galley = Some(number_galley.clone());
            line.code_galley = Some(code_galley.clone());
        }
        (number_galley, code_galley, elapsed)
    }

    fn collapsed_galley(
        &mut self,
        ui: &egui::Ui,
        document: &UiDocument,
        range: &SourceFoldRange,
    ) -> (Arc<egui::Galley>, Duration) {
        if let Some(galley) = self.collapsed_lines.get(&range.id) {
            self.cache_hits += 1;
            return (galley.clone(), Duration::ZERO);
        }
        let started = Instant::now();
        let line = document
            .source_lines
            .get(range.start_line)
            .map_or("", String::as_str);
        let display_line = collapsed_source_line(line, range);
        let job = modelica_layout_job(
            &display_line,
            &document.full_class_names,
            &document.short_class_names,
        );
        let galley = ui.painter().layout_job(job);
        let elapsed = started.elapsed();
        self.cache_misses += 1;
        self.max_line_width = self.max_line_width.max(galley.size().x);
        self.collapsed_lines
            .insert(range.id.clone(), galley.clone());
        (galley, elapsed)
    }

    fn fold_marker_galleys(&mut self, ui: &egui::Ui) -> (Arc<egui::Galley>, Arc<egui::Galley>) {
        if let Some((expanded, collapsed)) = &self.fold_marker_galleys {
            return (expanded.clone(), collapsed.clone());
        }
        let expanded =
            ui.painter()
                .layout_no_wrap("▾".to_owned(), ui_mono_font(12.0), theme_text_secondary());
        let collapsed =
            ui.painter()
                .layout_no_wrap("▸".to_owned(), ui_mono_font(12.0), theme_text_secondary());
        let markers = (expanded, collapsed);
        self.fold_marker_galleys = Some(markers.clone());
        markers
    }
}

impl LoadedDocument {
    fn load(path: &FsPath) -> Result<Self, String> {
        let load_started = Instant::now();
        let package_started = Instant::now();
        let package = PackageLoader
            .load(path)
            .map_err(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))?;
        let package_load_time = package_started.elapsed();
        let mut class_names = Vec::new();
        collect_class_names(&package, &mut class_names);
        let tree_started = Instant::now();
        let model_tree = build_model_tree(&package);
        let tree_build_time = tree_started.elapsed();
        let mut class_sources = Vec::new();
        collect_class_sources(&package, &mut class_sources);
        let registry_started = Instant::now();
        let mut registry = LibraryRegistry::default();
        add_bundled_msl(&mut registry);
        registry.index_package(&package);
        registry.register_package(&package);
        let registry_build_time = registry_started.elapsed();
        let file_count = class_sources
            .iter()
            .map(|class| class.source_file.clone())
            .collect::<HashSet<_>>()
            .len();
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
        let icon_cache = class_names
            .iter()
            .cloned()
            .map(|class_name| (class_name, OnceLock::new()))
            .collect();
        let diagram_cache = class_names
            .iter()
            .cloned()
            .map(|class_name| (class_name, OnceLock::new()))
            .collect();
        let load_stats = LibraryLoadStats {
            package_load_time,
            tree_build_time,
            registry_build_time,
            total_time: load_started.elapsed(),
            file_count,
            class_count: class_names.len(),
        };
        let document = Self {
            path: path.to_owned(),
            package_name: package.qualified_name,
            class_names,
            model_tree,
            diagnostics: package.diagnostics.len(),
            registry: RefCell::new(registry),
            icon_cache,
            diagram_cache,
            scene_resolution_stats: RefCell::new(SceneResolutionStats::default()),
            class_sources,
            source_overrides: HashMap::new(),
            saved_class_text,
            source_versions: HashMap::new(),
        };
        trace_library_load(&load_stats);
        Ok(document)
    }

    fn icon(&self, class_name: &str) -> Option<&CoreIconScene> {
        let cache = self.icon_cache.get(class_name)?;
        cache
            .get_or_init(|| self.resolve_icon_scene(class_name))
            .as_ref()
    }

    fn diagram(&self, class_name: &str) -> Option<&CoreDiagramScene> {
        let cache = self.diagram_cache.get(class_name)?;
        cache
            .get_or_init(|| self.resolve_diagram_scene(class_name))
            .as_ref()
    }

    fn replace_icon(&mut self, class_name: &str, scene: CoreIconScene) {
        let Some(cache) = self.icon_cache.get_mut(class_name) else {
            return;
        };
        *cache = OnceLock::new();
        let _ = cache.set(Some(scene));
    }

    fn replace_diagram(&mut self, class_name: &str, scene: CoreDiagramScene) {
        let Some(cache) = self.diagram_cache.get_mut(class_name) else {
            return;
        };
        *cache = OnceLock::new();
        let _ = cache.set(Some(scene));
    }

    fn icon_cache_hit(&self, class_name: &str) -> bool {
        self.icon_cache
            .get(class_name)
            .is_some_and(|cache| cache.get().is_some())
    }

    fn diagram_cache_hit(&self, class_name: &str) -> bool {
        self.diagram_cache
            .get(class_name)
            .is_some_and(|cache| cache.get().is_some())
    }

    fn scene_resolution_stats(&self) -> SceneResolutionStats {
        *self.scene_resolution_stats.borrow()
    }

    fn resolve_icon_scene(&self, class_name: &str) -> Option<CoreIconScene> {
        let started = Instant::now();
        let result = {
            let mut registry = self.registry.borrow_mut();
            registry
                .resolve_class(class_name)
                .map(|(class, source)| IconResolver::new(&mut registry).resolve(&class, &source))
        };
        let mut stats = self.scene_resolution_stats.borrow_mut();
        stats.icon_resolve_count = stats.icon_resolve_count.saturating_add(1);
        stats.icon_resolve_time += started.elapsed();
        result
    }

    fn resolve_diagram_scene(&self, class_name: &str) -> Option<CoreDiagramScene> {
        let started = Instant::now();
        let result = {
            let mut registry = self.registry.borrow_mut();
            registry
                .resolve_class(class_name)
                .map(|(class, source)| resolve_diagram(&class, &source, &mut registry))
        };
        let mut stats = self.scene_resolution_stats.borrow_mut();
        stats.diagram_resolve_count = stats.diagram_resolve_count.saturating_add(1);
        stats.diagram_resolve_time += started.elapsed();
        result
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

    /// Return whether the in-memory document differs from the last successful
    /// save. This deliberately compares edited class text with the saved
    /// baseline instead of using history length: undo can legitimately return
    /// the document to its saved content.
    fn has_unsaved_changes(&self) -> bool {
        self.source_overrides.iter().any(|(qualified_name, text)| {
            self.saved_class_text
                .get(qualified_name)
                .is_none_or(|saved| saved != text)
        })
    }

    fn discard_unsaved_changes(&mut self) {
        self.source_overrides.clear();
        self.invalidate_scene_caches();
    }

    fn set_class_text(&mut self, qualified_name: &str, text: String) {
        let matches_saved_text = self
            .saved_class_text
            .get(qualified_name)
            .is_some_and(|saved| saved == &text);
        if matches_saved_text {
            self.source_overrides.remove(qualified_name);
        } else {
            self.source_overrides
                .insert(qualified_name.to_owned(), text);
        }
        self.invalidate_scene_caches();
        self.refresh_registry_source(qualified_name);
        let version = self
            .source_versions
            .entry(qualified_name.to_owned())
            .or_default();
        *version = version.saturating_add(1);
    }

    fn apply_saved_file(&mut self, file: PendingFileSave) -> Result<(), String> {
        let old_names = self
            .class_sources
            .iter()
            .filter(|class| class.source_file == file.path)
            .map(|class| class.qualified_name.clone())
            .collect::<HashSet<_>>();
        let new_names = file
            .class_sources
            .iter()
            .map(|class| class.qualified_name.clone())
            .collect::<HashSet<_>>();

        self.class_sources
            .retain(|class| class.source_file != file.path);
        self.class_sources.extend(file.class_sources.clone());

        for removed in old_names.difference(&new_names) {
            self.saved_class_text.remove(removed);
            self.source_overrides.remove(removed);
            self.source_versions.remove(removed);
        }
        for class in &file.class_sources {
            let text = file
                .contents
                .get(class.source_range.start..class.source_range.end)
                .ok_or_else(|| {
                    format!(
                        "saved class range is invalid for {} in {}",
                        class.qualified_name,
                        file.path.display()
                    )
                })?;
            self.saved_class_text
                .insert(class.qualified_name.clone(), text.to_owned());
            self.source_overrides.remove(&class.qualified_name);
        }

        self.registry
            .borrow_mut()
            .register_source(file.path, file.contents)
            .map_err(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))?;
        Ok(())
    }

    fn invalidate_scene_caches(&mut self) {
        for cache in self.icon_cache.values_mut() {
            *cache = OnceLock::new();
        }
        for cache in self.diagram_cache.values_mut() {
            *cache = OnceLock::new();
        }
    }

    fn refresh_registry_source(&mut self, qualified_name: &str) {
        let Some(source_file) = self
            .class_sources
            .iter()
            .find(|class| class.qualified_name == qualified_name)
            .map(|class| class.source_file.clone())
        else {
            return;
        };
        let Ok(source) = fs::read_to_string(&source_file) else {
            return;
        };
        let Ok(source) = compose_current_file_source(self, &source_file, &source) else {
            return;
        };
        let _ = self
            .registry
            .borrow_mut()
            .register_source(source_file, source);
    }

    fn resolve_candidate_scenes(
        &self,
        qualified_name: &str,
        source: &str,
    ) -> Result<(CoreIconScene, CoreDiagramScene), String> {
        let parsed = parse(source, "<candidate>")
            .map_err(|error| format!("candidate source does not parse: {error}"))?;
        self.resolve_candidate_scenes_from_parsed(qualified_name, source, &parsed)
    }

    fn resolve_candidate_scenes_from_parsed(
        &self,
        qualified_name: &str,
        source: &str,
        parsed: &ModelicaFile,
    ) -> Result<(CoreIconScene, CoreDiagramScene), String> {
        let mut registry = self.registry.borrow_mut();
        let (mut class, _) = registry
            .resolve_class(qualified_name)
            .ok_or_else(|| format!("class `{qualified_name}` was not found"))?;
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
        let source_version = selected_class
            .as_deref()
            .map_or(0, |class_name| self.source_version(class_name));
        let full_class_names = self.class_names.iter().cloned().collect::<HashSet<_>>();
        let short_class_names = self
            .class_names
            .iter()
            .filter_map(|name| name.rsplit('.').next())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let (source_name, source) = selected_class
            .as_deref()
            .and_then(|class_name| self.class_source(class_name))
            .unwrap_or_default();
        let source_lines = source.lines().map(str::to_owned).collect::<Vec<_>>();
        let source_max_line_chars = source_lines
            .iter()
            .map(|line| line.chars().count())
            .max()
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
            full_class_names,
            short_class_names,
            dirty: self.has_unsaved_changes(),
            tree: self.model_tree.clone(),
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
            source_lines,
            source_max_line_chars,
            source_version,
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
            "modelica-wgpu{} | {} | {} | {} classes | drag edit · Ctrl+drag pan",
            if self.has_unsaved_changes() { " *" } else { "" },
            self.package_name,
            file_name,
            self.class_names.len(),
        ) + &performance
    }
}

fn trace_library_load(stats: &LibraryLoadStats) {
    if std::env::var_os("MODELICA_WGPU_PROFILE_LIBRARY_LOAD").is_none() {
        return;
    }
    eprintln!(
        "[LIBRARY LOAD] files={} classes={} package_load_us={:.1} tree_build_us={:.1} registry_build_us={:.1} icon_resolve_count=0 icon_resolve_us=0.0 diagram_resolve_count=0 diagram_resolve_us=0.0 total_us={:.1}",
        stats.file_count,
        stats.class_count,
        stats.package_load_time.as_secs_f64() * 1_000_000.0,
        stats.tree_build_time.as_secs_f64() * 1_000_000.0,
        stats.registry_build_time.as_secs_f64() * 1_000_000.0,
        stats.total_time.as_secs_f64() * 1_000_000.0,
    );
}

fn trace_class_open(
    class_name: &str,
    icon_cache_hit: bool,
    diagram_cache_hit: bool,
    before: SceneResolutionStats,
    after: SceneResolutionStats,
    gpu_scene_build_time: Duration,
    total_time: Duration,
) {
    if std::env::var_os("MODELICA_WGPU_PROFILE_LIBRARY_LOAD").is_none()
        && std::env::var_os("MODELICA_WGPU_PROFILE_CLASS_OPEN").is_none()
    {
        return;
    }
    let icon_resolve_time = after
        .icon_resolve_time
        .saturating_sub(before.icon_resolve_time);
    let diagram_resolve_time = after
        .diagram_resolve_time
        .saturating_sub(before.diagram_resolve_time);
    eprintln!(
        "[CLASS OPEN] class={class_name} icon_cache_hit={icon_cache_hit} diagram_cache_hit={diagram_cache_hit} icon_resolve_us={:.1} diagram_resolve_us={:.1} gpu_scene_build_us={:.1} total_us={:.1}",
        icon_resolve_time.as_secs_f64() * 1_000_000.0,
        diagram_resolve_time.as_secs_f64() * 1_000_000.0,
        gpu_scene_build_time.as_secs_f64() * 1_000_000.0,
        total_time.as_secs_f64() * 1_000_000.0,
    );
}

fn collect_class_names(package: &PackageNode, output: &mut Vec<String>) {
    output.push(package.qualified_name.clone());
    for member in &package.ordered_members {
        match member {
            PackageMember::Package(child) => collect_class_names(child, output),
            PackageMember::Class(class) => collect_class_names_from_class(class, output),
        }
    }
}

fn collect_class_names_from_class(class: &Class, output: &mut Vec<String>) {
    output.push(class.qualified_name.clone());
    for child in &class.children {
        collect_class_names_from_class(child, output);
    }
}

fn collect_class_sources(package: &PackageNode, output: &mut Vec<ClassSource>) {
    output.push(ClassSource {
        qualified_name: package.qualified_name.clone(),
        source_file: package.source_file.clone(),
        source_range: package.source_range.unwrap_or(SourceRange::new(0, 0)),
    });
    for member in &package.ordered_members {
        match member {
            PackageMember::Package(child) => collect_class_sources(child, output),
            PackageMember::Class(class) => collect_class_source(class, output),
        }
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

fn parsed_class_sources(source: &str, source_file: &FsPath) -> Result<Vec<ClassSource>, String> {
    let parsed = parse(source, source_file)
        .map_err(|error| format!("{}: {error}", source_file.display()))?;
    let mut class_sources = Vec::new();
    for class in &parsed.classes {
        collect_parsed_class_source(class, source_file, &mut class_sources);
    }
    Ok(class_sources)
}

fn collect_parsed_class_source(class: &Class, source_file: &FsPath, output: &mut Vec<ClassSource>) {
    output.push(ClassSource {
        qualified_name: class.qualified_name.clone(),
        source_file: source_file.to_owned(),
        source_range: class.source_range,
    });
    for child in &class.children {
        collect_parsed_class_source(child, source_file, output);
    }
}

fn build_model_tree(package: &PackageNode) -> TreeNode {
    TreeNode {
        name: package.name.clone(),
        qualified_name: package.qualified_name.clone(),
        class_name: Some(package.qualified_name.clone()),
        kind: Some(ClassKind::Package),
        description: package.description.clone(),
        children: package
            .ordered_members
            .iter()
            .map(build_model_tree_member)
            .collect(),
    }
}

fn build_model_tree_member(member: &PackageMember) -> TreeNode {
    match member {
        PackageMember::Package(package) => build_model_tree(package),
        PackageMember::Class(class) => TreeNode {
            name: class.name.clone(),
            qualified_name: class.qualified_name.clone(),
            class_name: Some(class.qualified_name.clone()),
            kind: Some(class.kind),
            description: class.description.clone(),
            children: class
                .children
                .iter()
                .map(|child| build_model_tree_member(&PackageMember::Class(child.clone())))
                .collect(),
        },
    }
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

fn build_font_fallback_chain(
    primary_keys: &[String],
    role_fallback_keys: &[Option<&str>],
    system_fallback_keys: &[String],
) -> Vec<String> {
    let mut keys = Vec::with_capacity(
        primary_keys.len() + role_fallback_keys.len() + system_fallback_keys.len(),
    );
    for key in primary_keys
        .iter()
        .map(String::as_str)
        .chain(role_fallback_keys.iter().filter_map(|key| *key))
        .chain(system_fallback_keys.iter().map(String::as_str))
    {
        if !keys.iter().any(|existing| existing == key) {
            keys.push(key.to_owned());
        }
    }
    keys
}

fn trace_font_fallback_role(
    role: &str,
    requested_family: &str,
    primary_key: &str,
    primary_path: Option<&str>,
    fallback_keys: &[String],
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_TEXT_LAYOUT").is_none() {
        return;
    }
    eprintln!(
        "[TEXT FONT] role={role} requested_family={requested_family} primary_key={primary_key:?} primary_file={primary_path:?} fallback_order={fallback_keys:?}"
    );
}

fn install_ui_fonts(ctx: &egui::Context) {
    let medium_candidates = [
        r"C:\Windows\Fonts\Inter-Medium.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\NotoSans-Medium.ttf",
        "/usr/share/fonts/inter/Inter-Medium.ttf",
        "/usr/share/fonts/truetype/inter/Inter-Medium.ttf",
        "/usr/share/fonts/opentype/inter/Inter-Medium.otf",
        "/usr/share/fonts/noto/NotoSans-Medium.ttf",
    ];
    let semibold_candidates = [
        r"C:\Windows\Fonts\Inter-SemiBold.ttf",
        r"C:\Windows\Fonts\seguisb.ttf",
        r"C:\Windows\Fonts\segoeuib.ttf",
        r"C:\Windows\Fonts\NotoSans-SemiBold.ttf",
        "/usr/share/fonts/inter/Inter-SemiBold.ttf",
        "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
        "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf",
        "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
    ];
    let italic_candidates = [
        r"C:\Windows\Fonts\Inter-Italic.ttf",
        r"C:\Windows\Fonts\segoeuii.ttf",
        r"C:\Windows\Fonts\NotoSans-Italic.ttf",
        "/usr/share/fonts/inter/Inter-Italic.ttf",
        "/usr/share/fonts/truetype/inter/Inter-Italic.ttf",
        "/usr/share/fonts/opentype/inter/Inter-Italic.otf",
        "/usr/share/fonts/noto/NotoSans-Italic.ttf",
    ];
    let semibold_italic_candidates = [
        r"C:\Windows\Fonts\Inter-SemiBoldItalic.ttf",
        r"C:\Windows\Fonts\seguisbi.ttf",
        r"C:\Windows\Fonts\segoeuiz.ttf",
        r"C:\Windows\Fonts\NotoSans-SemiBoldItalic.ttf",
        "/usr/share/fonts/inter/Inter-SemiBoldItalic.ttf",
        "/usr/share/fonts/truetype/inter/Inter-SemiBoldItalic.ttf",
        "/usr/share/fonts/opentype/inter/Inter-SemiBoldItalic.otf",
        "/usr/share/fonts/noto/NotoSans-SemiBoldItalic.ttf",
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
    let italic = italic_candidates
        .iter()
        .find_map(|path| fs::read(path).ok().map(|bytes| (*path, bytes)));
    let italic_path = italic.as_ref().map(|(path, _)| *path);
    let semibold_italic = semibold_italic_candidates
        .iter()
        .find_map(|path| fs::read(path).ok().map(|bytes| (*path, bytes)));
    let semibold_italic_path = semibold_italic.as_ref().map(|(path, _)| *path);
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
    let italic_key = if let Some((_, bytes)) = italic {
        fonts
            .font_data
            .insert(UI_FONT_ITALIC.to_owned(), FontData::from_owned(bytes));
        UI_FONT_ITALIC.to_owned()
    } else {
        medium_key.clone()
    };
    let semibold_italic_key = if let Some((_, bytes)) = semibold_italic {
        fonts.font_data.insert(
            UI_FONT_SEMIBOLD_ITALIC.to_owned(),
            FontData::from_owned(bytes),
        );
        UI_FONT_SEMIBOLD_ITALIC.to_owned()
    } else {
        semibold_key.clone()
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

    let ui_fallback = build_font_fallback_chain(
        std::slice::from_ref(&medium_key),
        &[cjk_key.as_deref(), symbols_key.as_deref()],
        &default_proportional,
    );
    let semibold_primary = semibold_path
        .map(|_| vec![semibold_key.clone()])
        .unwrap_or_default();
    let semibold_fallback = build_font_fallback_chain(
        &semibold_primary,
        &[
            cjk_key.as_deref(),
            symbols_key.as_deref(),
            Some(medium_key.as_str()),
        ],
        &default_proportional,
    );
    let italic_primary = italic_path.map(|_| vec![italic_key]).unwrap_or_default();
    let italic_fallback = build_font_fallback_chain(
        &italic_primary,
        &[
            cjk_key.as_deref(),
            symbols_key.as_deref(),
            Some(medium_key.as_str()),
        ],
        &default_proportional,
    );
    let semibold_italic_primary = semibold_italic_path
        .map(|_| vec![semibold_italic_key])
        .unwrap_or_default();
    let semibold_italic_fallback = build_font_fallback_chain(
        &semibold_italic_primary,
        &[
            cjk_key.as_deref(),
            symbols_key.as_deref(),
            Some(semibold_key.as_str()),
            Some(medium_key.as_str()),
        ],
        &default_proportional,
    );
    let mono_fallback = build_font_fallback_chain(
        &default_monospace,
        &[
            cjk_key.as_deref(),
            symbols_key.as_deref(),
            Some(medium_key.as_str()),
        ],
        &[],
    );
    let symbols_fallback = symbols_key
        .as_ref()
        .map(|key| vec![key.clone()])
        .unwrap_or_default();

    trace_font_fallback_role(
        "ui-normal-and-tree",
        UI_FONT_MEDIUM,
        &medium_key,
        medium_path,
        &ui_fallback,
    );
    trace_font_fallback_role(
        "ui-semibold-and-selected-tree",
        UI_FONT_SEMIBOLD,
        semibold_primary
            .first()
            .map(String::as_str)
            .unwrap_or("<no-dedicated-face>"),
        semibold_path,
        &semibold_fallback,
    );
    trace_font_fallback_role(
        "modelica-text-italic",
        UI_FONT_ITALIC,
        italic_primary
            .first()
            .map(String::as_str)
            .unwrap_or("<none>"),
        italic_path,
        &italic_fallback,
    );
    trace_font_fallback_role(
        "modelica-text-semibold-italic",
        UI_FONT_SEMIBOLD_ITALIC,
        semibold_italic_primary
            .first()
            .map(String::as_str)
            .unwrap_or("<none>"),
        semibold_italic_path,
        &semibold_italic_fallback,
    );
    trace_font_fallback_role(
        "source-body-line-number-and-tree-kind",
        UI_FONT_MONO,
        mono_fallback
            .first()
            .map(String::as_str)
            .unwrap_or("<none>"),
        None,
        &mono_fallback,
    );
    trace_font_fallback_role(
        "cjk-glyph-fallback",
        "glyph-fallback",
        cjk_key.as_deref().unwrap_or("<unavailable>"),
        cjk_path,
        cjk_key.as_slice(),
    );
    trace_font_fallback_role(
        "unicode-symbols-fallback",
        UI_FONT_SYMBOLS,
        symbols_key.as_deref().unwrap_or("<unavailable>"),
        symbols_path,
        &symbols_fallback,
    );

    fonts
        .families
        .insert(FontFamily::Name(UI_FONT_MEDIUM.into()), ui_fallback.clone());
    fonts
        .families
        .insert(FontFamily::Name(UI_FONT_SEMIBOLD.into()), semibold_fallback);
    fonts
        .families
        .insert(FontFamily::Name(UI_FONT_ITALIC.into()), italic_fallback);
    fonts.families.insert(
        FontFamily::Name(UI_FONT_SEMIBOLD_ITALIC.into()),
        semibold_italic_fallback,
    );
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
    if let Some(path) = italic_path {
        eprintln!("modelica-wgpu: installed italic model text font from {path}");
    }
    if let Some(path) = semibold_italic_path {
        eprintln!("modelica-wgpu: installed semibold italic model text font from {path}");
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
    Connection,
    Component,
    Connector,
    Overlay,
}

impl DiagramRenderLayer {
    const COUNT: usize = 5;

    fn index(self) -> usize {
        match self {
            Self::Background => 0,
            Self::Connection => 1,
            Self::Component => 2,
            Self::Connector => 3,
            Self::Overlay => 4,
        }
    }
}

const DIAGRAM_RENDER_LAYERS: [DiagramRenderLayer; 4] = [
    DiagramRenderLayer::Background,
    DiagramRenderLayer::Connection,
    DiagramRenderLayer::Component,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StrokeKind {
    Connection,
    Icon,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaleRouteReason {
    InvalidSourceRoute,
    EndpointDrift,
    ExcessiveDetour,
    ReanchorInvalid,
    ReanchorEndpointMismatch,
}

impl StaleRouteReason {
    fn label(self) -> &'static str {
        match self {
            Self::InvalidSourceRoute => "invalid_source_route",
            Self::EndpointDrift => "endpoint_drift",
            Self::ExcessiveDetour => "excessive_detour",
            Self::ReanchorInvalid => "reanchor_invalid",
            Self::ReanchorEndpointMismatch => "reanchor_endpoint_mismatch",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelTextAlignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
struct ModelTextOverlayItem {
    text: String,
    corners: [CorePoint; 4],
    color: [u8; 3],
    font_size: Option<f32>,
    font_name: Option<String>,
    scale: f32,
    scale_x: f32,
    scale_y: f32,
    extent_width: f32,
    extent_height: f32,
    angle: f32,
    alignment: ModelTextAlignment,
    bold: bool,
    italic: bool,
    underline: bool,
    minimum_screen_px: f32,
}

/// Placement inputs for a component while a diagram edit is still only a
/// preview. Keeping these values together lets the GPU geometry and the text
/// overlay use the same effective component transform during move and resize
/// interactions.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ComponentPreviewPlacement {
    origin: CorePoint,
    rotation: f32,
    extent: modelica_core::scene::Extent,
    delta: CorePoint,
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
    stroke_zoom: f32,
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

    fn commit_translation(
        &mut self,
        queue: &wgpu::Queue,
        edit_key: &str,
        translation: [f32; 2],
        component_transform: Option<Transform2D>,
    ) {
        for geometry in &mut self.geometries {
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
            geometry.base_vertices = vertices;
            if let (Some(component), Some(transform)) =
                (&mut geometry.component, component_transform)
            {
                component.transform = transform;
            }
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
            let Some(updated) = line_geometry(
                &line,
                connection.transform,
                self.stroke_zoom,
                StrokeKind::Connection,
            )
            .into_iter()
            .next() else {
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
    stroke_zoom: f32,
    points: Vec<CorePoint>,
}

impl ConnectionPreviewMesh {
    fn new(
        device: &wgpu::Device,
        style_layout: &wgpu::BindGroupLayout,
        connection_id: String,
        line: &LineGraphic,
        transform: Transform2D,
        stroke_zoom: f32,
    ) -> Self {
        let segment_count = line.points.len().saturating_sub(1);
        let segment_capacity = connection_preview_segment_capacity(segment_count);
        let initial_vertices = preview_connection_vertices(
            &line.points,
            line.origin,
            line.rotation,
            line.thickness,
            transform,
            stroke_zoom,
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
            stroke_zoom,
            points: line.points.clone(),
        }
    }

    fn update(&mut self, queue: &wgpu::Queue, points: &[CorePoint]) -> bool {
        if !self.can_update(points) {
            return false;
        }
        let segment_count = points.len().saturating_sub(1);
        update_preview_connection_vertices(
            &mut self.vertices[..segment_count * 4],
            points,
            self.line_origin,
            self.line_rotation,
            self.line_thickness,
            self.transform,
            self.stroke_zoom,
        );
        self.points = points.to_vec();
        self.active_segment_count = segment_count;
        if segment_count > 0 {
            queue.write_buffer(
                &self.vertex_buffer,
                0,
                bytemuck::cast_slice(&self.vertices[..segment_count * 4]),
            );
        }
        true
    }

    fn set_zoom(&mut self, queue: &wgpu::Queue, stroke_zoom: f32) {
        if (self.stroke_zoom - stroke_zoom).abs() <= f32::EPSILON {
            return;
        }
        self.stroke_zoom = stroke_zoom;
        let segment_count = self.points.len().saturating_sub(1);
        if segment_count == 0 {
            return;
        }
        update_preview_connection_vertices(
            &mut self.vertices[..segment_count * 4],
            &self.points,
            self.line_origin,
            self.line_rotation,
            self.line_thickness,
            self.transform,
            self.stroke_zoom,
        );
        queue.write_buffer(
            &self.vertex_buffer,
            0,
            bytemuck::cast_slice(&self.vertices[..segment_count * 4]),
        );
    }

    fn can_update(&self, points: &[CorePoint]) -> bool {
        valid_interactive_connection_route(points)
            && points.len().saturating_sub(1) <= self.segment_capacity
    }
}

/// Transient connection meshes shown while a diagram component is being
/// dragged. The static diagram scene is left untouched until MouseUp commits
/// the source edit.
struct ComponentConnectionPreviewSet {
    previews: Vec<ConnectionPreviewMesh>,
}

impl ComponentConnectionPreviewSet {
    fn new(
        device: &wgpu::Device,
        style_layout: &wgpu::BindGroupLayout,
        scene: &CoreDiagramScene,
        snapshots: &[ConnectionDragSnapshot],
        stroke_zoom: f32,
    ) -> Self {
        let previews = snapshots
            .iter()
            .filter_map(|snapshot| {
                let raw_line = scene
                    .connections
                    .iter()
                    .find(|connection| connection.key == snapshot.connection_key)
                    .and_then(|connection| connection.line.as_ref())?;
                // The displayed route may contain a semantic reanchor elbow
                // that is not present in the serialized source line. Size
                // the transient mesh for that canonical route, otherwise a
                // valid fallback route can be rejected simply because the
                // old two-point capacity is too small.
                let mut line = raw_line.clone();
                line.points = if valid_interactive_connection_route(&snapshot.base_route_points) {
                    snapshot.base_route_points.clone()
                } else {
                    component_drag_preview_route(snapshot, CorePoint { x: 0.0, y: 0.0 })
                };
                Some(ConnectionPreviewMesh::new(
                    device,
                    style_layout,
                    snapshot.connection_id.clone(),
                    &line,
                    Transform2D {
                        scale_y: -1.0,
                        ..Transform2D::identity()
                    },
                    stroke_zoom,
                ))
            })
            .collect();
        Self { previews }
    }

    fn contains(&self, connection_id: &str) -> bool {
        self.previews
            .iter()
            .any(|preview| preview.connection_id == connection_id)
    }

    fn resource_stats(&self) -> PreviewResourceStats {
        PreviewResourceStats {
            preview_count: self.previews.len(),
            // Each preview owns a vertex buffer, an index buffer and a style
            // uniform buffer. The uniform buffer is retained by its bind
            // group even though the mesh does not store a second handle.
            preview_buffer_count: self.previews.len().saturating_mul(3),
            preview_segment_count: self
                .previews
                .iter()
                .map(|preview| preview.segment_capacity)
                .sum(),
        }
    }

    fn update(&mut self, queue: &wgpu::Queue, connection_id: &str, points: &[CorePoint]) -> bool {
        self.previews
            .iter_mut()
            .find(|preview| preview.connection_id == connection_id)
            .is_some_and(|preview| preview.update(queue, points))
    }

    fn set_zoom(&mut self, queue: &wgpu::Queue, stroke_zoom: f32) {
        for preview in &mut self.previews {
            preview.set_zoom(queue, stroke_zoom);
        }
    }
}

fn geometry_connection_is_in_preview_set(
    geometry: &GpuGeometry,
    previews: Option<&ComponentConnectionPreviewSet>,
) -> bool {
    geometry
        .edit_key
        .as_deref()
        .is_some_and(|connection_id| previews.is_some_and(|set| set.contains(connection_id)))
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
    stroke_zoom: f32,
}

impl ConnectionCreationPreview {
    fn new(device: &wgpu::Device, style_layout: &wgpu::BindGroupLayout, stroke_zoom: f32) -> Self {
        Self::with_capacity(
            device,
            style_layout,
            INITIAL_CONNECTION_CREATION_SEGMENTS,
            stroke_zoom,
        )
    }

    fn with_capacity(
        device: &wgpu::Device,
        style_layout: &wgpu::BindGroupLayout,
        segment_capacity: usize,
        stroke_zoom: f32,
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
            stroke_zoom,
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
        let mut replacement =
            Self::with_capacity(device, style_layout, segment_capacity, self.stroke_zoom);
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
                self.stroke_zoom,
            );
        }
        segment_count
    }

    fn set_zoom(&mut self, queue: &wgpu::Queue, points: &[CorePoint], stroke_zoom: f32) {
        if (self.stroke_zoom - stroke_zoom).abs() <= f32::EPSILON {
            return;
        }
        let segment_count = points.len().saturating_sub(1);
        if segment_count > self.segment_capacity || segment_count != self.active_segment_count {
            return;
        }
        self.stroke_zoom = stroke_zoom;
        if segment_count == 0 {
            return;
        }
        update_preview_connection_vertices(
            &mut self.vertices[..segment_count * 4],
            points,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            0.35,
            Transform2D {
                scale_y: -1.0,
                ..Transform2D::identity()
            },
            self.stroke_zoom,
        );
        queue.write_buffer(
            &self.vertex_buffer,
            0,
            bytemuck::cast_slice(&self.vertices[..segment_count * 4]),
        );
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
    stroke_zoom: f32,
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
        stroke_zoom,
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
    stroke_zoom: f32,
) {
    let half_width =
        model_stroke_width(thickness, transform, stroke_zoom, StrokeKind::Connection) * 0.5;
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
    egui_run: Duration,
    texture_update: Duration,
    update_buffers: Duration,
    scene_encode: Duration,
    queue_submit: Duration,
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
            egui_run: Duration::ZERO,
            texture_update: Duration::ZERO,
            update_buffers: Duration::ZERO,
            scene_encode: Duration::ZERO,
            queue_submit: Duration::ZERO,
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
        total: Duration,
        scene_scan: Duration,
        timings: FrameStageTimings,
    ) {
        if !self.enabled {
            return;
        }
        self.frames += 1;
        self.scene_scan += scene_scan;
        self.egui_tessellation += timings.egui_tessellation;
        self.egui_run += timings.egui_run;
        self.texture_update += timings.texture_update;
        self.update_buffers += timings.update_buffers;
        self.scene_encode += timings.scene_encode;
        self.queue_submit += timings.queue_submit;
        self.ui += ui;
        self.encode += encode;
        self.present += timings.present;
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
            "drag-profile: events={} frames={} p50_ms={:.2} p95_ms={:.2} worst_ms={:.2} preview_update_us={:.1} input_us={:.1} interaction_clone_us={:.1} snap_us={:.1} reanchor_us={:.1} tessellation_us={:.1} gpu_upload_us={:.1} scene_scan_us={:.1} egui_run_us={:.1} egui_tessellation_us={:.1} texture_update_us={:.1} egui_update_buffers_us={:.1} scene_encode_us={:.1} queue_submit_us={:.1} ui_us={:.1} encode_us={:.1} present_us={:.1}",
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
            micros(self.egui_run),
            micros(self.egui_tessellation),
            micros(self.texture_update),
            micros(self.update_buffers),
            micros(self.scene_encode),
            micros(self.queue_submit),
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
        self.egui_run = Duration::ZERO;
        self.egui_tessellation = Duration::ZERO;
        self.texture_update = Duration::ZERO;
        self.update_buffers = Duration::ZERO;
        self.scene_encode = Duration::ZERO;
        self.queue_submit = Duration::ZERO;
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

fn trace_interaction_latency(phase: &str, started: Instant) {
    if std::env::var_os("MODELICA_WGPU_PROFILE_COMPONENT_COMMIT").is_some()
        || std::env::var_os("MODELICA_WGPU_PROFILE_CANCEL").is_some()
        || std::env::var_os("MODELICA_WGPU_PROFILE_DESELECT").is_some()
    {
        eprintln!(
            "[INTERACTION LATENCY] phase={phase} total_us={:.1}",
            started.elapsed().as_secs_f64() * 1_000_000.0
        );
    }
}

struct ComponentCommitProfile {
    enabled: bool,
    started: Instant,
    connections: usize,
    component_edit_build: Duration,
    connection_edit_build: Duration,
    transaction_apply: Duration,
    parse: Duration,
    scene_resolve: Duration,
    validation: Duration,
    gpu_component_commit: Duration,
    gpu_connections_commit: Duration,
    hit_cache: Duration,
    ui_refresh: Duration,
    parse_count: u64,
    resolve_count: u64,
}

impl ComponentCommitProfile {
    fn new(connections: usize) -> Self {
        Self {
            enabled: std::env::var_os("MODELICA_WGPU_PROFILE_COMPONENT_COMMIT").is_some(),
            started: Instant::now(),
            connections,
            component_edit_build: Duration::ZERO,
            connection_edit_build: Duration::ZERO,
            transaction_apply: Duration::ZERO,
            parse: Duration::ZERO,
            scene_resolve: Duration::ZERO,
            validation: Duration::ZERO,
            gpu_component_commit: Duration::ZERO,
            gpu_connections_commit: Duration::ZERO,
            hit_cache: Duration::ZERO,
            ui_refresh: Duration::ZERO,
            parse_count: 0,
            resolve_count: 0,
        }
    }

    fn micros(duration: Duration) -> f64 {
        duration.as_secs_f64() * 1_000_000.0
    }
}

impl Drop for ComponentCommitProfile {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        eprintln!(
            "[COMPONENT COMMIT] connections={} total_us={:.1} component_edit_build_us={:.1} connection_edit_build_us={:.1} transaction_apply_us={:.1} parse_us={:.1} scene_resolve_us={:.1} validation_us={:.1} gpu_component_commit_us={:.1} gpu_connections_commit_us={:.1} hit_cache_us={:.1} ui_refresh_us={:.1} parse_count={} resolve_count={}",
            self.connections,
            Self::micros(self.started.elapsed()),
            Self::micros(self.component_edit_build),
            Self::micros(self.connection_edit_build),
            Self::micros(self.transaction_apply),
            Self::micros(self.parse),
            Self::micros(self.scene_resolve),
            Self::micros(self.validation),
            Self::micros(self.gpu_component_commit),
            Self::micros(self.gpu_connections_commit),
            Self::micros(self.hit_cache),
            Self::micros(self.ui_refresh),
            self.parse_count,
            self.resolve_count,
        );
        trace_interaction_latency("component_commit", self.started);
    }
}

struct ValidatedSourceCandidate {
    source: String,
    parsed: ModelicaFile,
    transaction_apply: Duration,
    parse: Duration,
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
    msaa_samples: u32,
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
    status_message: Option<String>,
    pending_document_action: Option<PendingDocumentAction>,
    exit_requested: bool,
    selected_class: Option<String>,
    ui_document: Option<UiDocument>,
    source_highlight_cache: SourceHighlightCache,
    source_scroll_state: SourceScrollState,
    source_interaction: SourceInteractionState,
    source_scroll_rect: Option<egui::Rect>,
    source_wheel_sample: Option<SourceWheelSample>,
    source_perf_frame: Option<SourcePerfFrame>,
    source_frame_stats: SourceFrameStats,
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
    component_connection_previews: Option<ComponentConnectionPreviewSet>,
    connection_creation_preview: Option<ConnectionCreationPreview>,
    pending_drag_position: Option<(PhysicalPosition<f64>, Instant)>,
    pending_waypoint: bool,
    pending_waypoint_queued_at: Option<Instant>,
    waypoint_frame_pending: bool,
    waypoint_profile_frames_remaining: u8,
    suppress_next_pointer_release_redraw: bool,
    cancel_generation: u64,
    pending_cancel_profile: Option<PendingCancelProfile>,
    cancel_e2e_profile: CancelE2EProfile,
    deselect_generation: u64,
    pending_deselect_profile: Option<PendingDeselectProfile>,
    deselect_profile: DeselectProfile,
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

fn configured_msaa_samples() -> u32 {
    match std::env::var("MODELICA_WGPU_MSAA")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("1") => 1,
        Some("4") | None => DEFAULT_MSAA_SAMPLES,
        Some(value) => {
            eprintln!(
                "modelica-wgpu: unsupported MODELICA_WGPU_MSAA={value:?}; using {DEFAULT_MSAA_SAMPLES}x"
            );
            DEFAULT_MSAA_SAMPLES
        }
    }
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
        let msaa_samples = configured_msaa_samples();
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
        if std::env::var_os("MODELICA_WGPU_PROFILE_SOURCE_SCROLL").is_some()
            || std::env::var_os("MODELICA_WGPU_PROFILE_DRAG").is_some()
        {
            eprintln!(
                "[WGPU FRAME POLICY] selected_present_mode={present_mode:?} supported_present_modes={:?} vsync={} desired_maximum_frame_latency=1 msaa_samples={msaa_samples}",
                capabilities.present_modes,
                !no_vsync,
            );
        }
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
                count: msaa_samples,
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
                    // All Modelica geometry is emitted with alpha=1.0. Keep
                    // the opaque path as a replace operation so sRGB colors
                    // are not routed through an unnecessary alpha blend.
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
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: msaa_samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        let scene = build_scene(
            &device,
            &style_layout,
            document.as_ref(),
            None,
            INITIAL_ZOOM,
        );
        let diagram_scene = build_diagram_scene(
            &device,
            &style_layout,
            document.as_ref(),
            None,
            INITIAL_ZOOM,
        );
        let msaa_view = create_msaa_view(&device, &config, msaa_samples);
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
            msaa_samples,
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
            status_message: None,
            pending_document_action: None,
            exit_requested: false,
            selected_class: None,
            ui_document: None,
            source_highlight_cache: SourceHighlightCache::default(),
            source_scroll_state: SourceScrollState::new(),
            source_interaction: SourceInteractionState::default(),
            source_scroll_rect: None,
            source_wheel_sample: None,
            source_perf_frame: None,
            source_frame_stats: SourceFrameStats::default(),
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
            component_connection_previews: None,
            connection_creation_preview: None,
            pending_drag_position: None,
            pending_waypoint: false,
            pending_waypoint_queued_at: None,
            waypoint_frame_pending: false,
            waypoint_profile_frames_remaining: 0,
            suppress_next_pointer_release_redraw: false,
            cancel_generation: 0,
            pending_cancel_profile: None,
            cancel_e2e_profile: CancelE2EProfile::new(),
            deselect_generation: 0,
            pending_deselect_profile: None,
            deselect_profile: DeselectProfile::new(),
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
        rect.contains(physical_to_logical_position(
            self.cursor,
            self.window.scale_factor() as f32,
        ))
    }

    fn pointer_over_source_scroll(&self) -> bool {
        if self.main_view != MainView::Source {
            return false;
        }
        let Some(rect) = self.source_scroll_rect else {
            return false;
        };
        rect.contains(physical_to_logical_position(
            self.cursor,
            self.window.scale_factor() as f32,
        ))
    }

    fn enqueue_source_line_scroll(&mut self, delta_y: f32) {
        self.source_scroll_state.enqueue_line_delta(delta_y);
        self.source_wheel_sample = Some(SourceWheelSample {
            kind: SourceWheelKind::LineDelta,
            delta_y,
        });
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
            let canonical_points = canonical_connection_points(scene, connection);
            let Some((hit, distance)) = hit_test_connection_segment_with_distance(
                connection,
                segment.segment_index,
                &canonical_points,
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
        self.hit_test_diagram_port_with(
            pointer_model,
            tolerance,
            connector_anchor_active_hit_distance,
        )
    }

    fn hit_test_diagram_hover_port(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
    ) -> Option<PortKey> {
        self.hit_test_diagram_port_with(pointer_model, tolerance, connector_anchor_hit_distance)
    }

    fn hit_test_diagram_port_with(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
        hit_distance: fn(&ConnectorAnchor, CorePoint, f32) -> Option<f32>,
    ) -> Option<PortKey> {
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
                hit_distance(anchor, pointer_model, tolerance).map(|distance| (distance, anchor))
            })
            .min_by(|(left_distance, left), (right_distance, right)| {
                compare_connector_anchor_hits(*left_distance, left, *right_distance, right)
            })
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

    fn hit_test_diagram_port_for_component(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
        component_id: &str,
    ) -> Option<PortKey> {
        let anchors = self.diagram_connector_anchors()?;
        let candidates = self
            .diagram_hit_cache
            .spatial_index
            .query(pointer_model, tolerance);
        candidates
            .port_indices
            .iter()
            .filter_map(|index| anchors.get(*index))
            .filter(|anchor| anchor.owner_component_id == component_id)
            .filter_map(|anchor| {
                connector_anchor_active_hit_distance(anchor, pointer_model, tolerance)
                    .map(|distance| (distance, anchor))
            })
            .min_by(|(left_distance, left), (right_distance, right)| {
                compare_connector_anchor_hits(*left_distance, left, *right_distance, right)
            })
            .map(|(_, anchor)| anchor.key.clone())
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
        let mut placement_fallback: Option<(f32, f32, (String, String, CorePoint))> = None;
        let result = candidates.component_indices.iter().rev().find_map(|index| {
            let item = self.diagram_hit_cache.components.get(*index)?;
            if !item.bounds.contains(pointer_model, tolerance) {
                return None;
            }
            let component = scene.components.get(item.scene_index)?;
            if !component.editable || !component.visible {
                return None;
            }
            let graphic_hit = diagram_component_contains_point(component, pointer_model, tolerance);
            let body_hit = point_in_component_placement_extent(component, pointer_model, tolerance);
            if !graphic_hit {
                if body_hit {
                    let extent = component
                        .placement_extent
                        .unwrap_or_else(default_component_extent);
                    let area =
                        (extent.p2.x - extent.p1.x).abs() * (extent.p2.y - extent.p1.y).abs();
                    let center = apply_transform_point(
                        CorePoint {
                            x: (extent.p1.x + extent.p2.x) * 0.5,
                            y: (extent.p1.y + extent.p2.y) * 0.5,
                        },
                        Transform2D {
                            translation: component.origin,
                            rotation: component.rotation,
                            scale_x: 1.0,
                            scale_y: 1.0,
                        },
                    );
                    let distance = distance_between(center, pointer_model);
                    let candidate = (
                        area,
                        distance,
                        (
                            component.id.clone(),
                            component.name.clone(),
                            component.origin,
                        ),
                    );
                    let replace =
                        placement_fallback
                            .as_ref()
                            .is_none_or(|(best_area, best_distance, _)| {
                                area < *best_area
                                    || ((area - *best_area).abs() <= DIAGRAM_GEOMETRY_EPSILON
                                        && distance < *best_distance)
                            });
                    if replace {
                        placement_fallback = Some(candidate);
                    }
                }
                return None;
            }
            Some((
                component.id.clone(),
                component.name.clone(),
                component.origin,
            ))
        });
        let result = result.or_else(|| placement_fallback.map(|(_, _, component)| component));
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
            self.hit_test_diagram_hover_port(pointer_model, tolerance)
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
        let edit_data = (|| -> Result<
            (
                ConnectionKey,
                LineGraphic,
                Vec<CorePoint>,
                ConnectionEndpointConstraint,
                String,
            ),
            String,
        > {
            let scene = document
                .diagram(&class_name)
                .ok_or_else(|| "Connection edit could not find the selected Diagram".to_owned())?;
            let connection = scene
                .connections
                .iter()
                .find(|connection| connection.id == hit.connection_id)
                .ok_or_else(|| "Connection edit lost its connection identity".to_owned())?;
            let raw_line = connection
                .line
                .as_ref()
                .ok_or_else(|| "Connection has no editable Line annotation".to_owned())?;
            let mut line = raw_line.clone();
            line.points = canonical_connection_points(scene, connection);
            let source_before = document
                .class_text(&class_name)
                .ok_or_else(|| "Connection edit could not load the selected source".to_owned())?;
            connection_source_editable_in_class(connection, &class_name, &source_before)?;
            let lhs = line
                .points
                .first()
                .copied()
                .ok_or_else(|| "Connection Line has no first point".to_owned())?;
            let rhs = line
                .points
                .last()
                .copied()
                .ok_or_else(|| "Connection Line has no last point".to_owned())?;
            if std::env::var_os("MODELICA_WGPU_PROFILE_CONNECTION_ORDER").is_some() {
                let first_world = line_local_to_world(&line, lhs);
                let last_world = line_local_to_world(&line, rhs);
                match resolve_connection_endpoints(scene, connection) {
                    Ok(endpoints) => {
                        let forward_error = distance_between(first_world, endpoints.lhs.world_position)
                            + distance_between(last_world, endpoints.rhs.world_position);
                        let reverse_error = distance_between(first_world, endpoints.rhs.world_position)
                            + distance_between(last_world, endpoints.lhs.world_position);
                        let order = if forward_error <= reverse_error {
                            "LhsToRhs"
                        } else {
                            "RhsToLhs"
                        };
                        eprintln!(
                            "[CONNECTION ORDER] id={} key={:?} lhs={} rhs={} first_world={:?} last_world={:?} lhs_anchor={:?} rhs_anchor={:?} forward_error={forward_error:.3} reverse_error={reverse_error:.3} chosen_order={order}",
                            connection.id,
                            connection.key,
                            connector_ref_text(&connection.lhs),
                            connector_ref_text(&connection.rhs),
                            first_world,
                            last_world,
                            endpoints.lhs.world_position,
                            endpoints.rhs.world_position,
                        );
                    }
                    Err(error) => {
                        eprintln!(
                            "[CONNECTION ORDER] id={} key={:?} lhs={} rhs={} first_world={:?} last_world={:?} unresolved={error:?}",
                            connection.id,
                            connection.key,
                            connector_ref_text(&connection.lhs),
                            connector_ref_text(&connection.rhs),
                            first_world,
                            last_world,
                        );
                    }
                }
            }
            // Connector resolution belongs at drag-start. The endpoint
            // policy is immutable while a connection segment/corner moves.
            let endpoint_constraint = match strict_connection_points(scene, connection) {
                Ok((lhs, rhs)) => ConnectionEndpointConstraint::Semantic { lhs, rhs },
                Err(_) => ConnectionEndpointConstraint::FixedExisting { lhs, rhs },
            };
            Ok((
                connection.key.clone(),
                line.clone(),
                line.points.clone(),
                endpoint_constraint,
                source_before,
            ))
        })();
        self.set_diagram_selection(DiagramSelection::Connection(hit.connection_id.clone()));
        let (connection_key, line, original_points, endpoint_constraint, source_before) =
            match edit_data {
                Ok(data) => data,
                Err(error) => {
                    self.load_error = Some(error);
                    return;
                }
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
            self.zoom,
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
                    endpoint_constraint,
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
                    endpoint_constraint,
                    snap_axes,
                    snapped_axis: None,
                    source_before,
                };
            }
            _ => {
                self.connection_preview = None;
                self.component_connection_previews = None;
            }
        }
    }

    fn begin_connection_creation(&mut self, source: &ConnectorAnchor) {
        self.connection_creation_profile.start();
        self.connection_preview = None;
        self.component_connection_previews = None;
        self.pending_waypoint = false;
        self.pending_waypoint_queued_at = None;
        self.waypoint_frame_pending = false;
        self.waypoint_profile_frames_remaining = 0;
        self.suppress_next_pointer_release_redraw = false;
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
            self.zoom,
        ));
        self.hovered_port = None;
    }

    fn selected_connection_overlay_points(&self) -> Option<Vec<CorePoint>> {
        let connection_id = self.selected_connection_id()?;
        let class_name = self.selected_class_name()?;
        let scene = self.document.as_ref()?.diagram(class_name)?;
        let connection = scene
            .connections
            .iter()
            .find(|connection| connection.id == connection_id)?;
        let raw_line = connection.line.as_ref()?;
        let canonical_points = canonical_connection_points(scene, connection);
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
            _ => &canonical_points,
        };
        Some(connection_world_points(raw_line, points))
    }

    fn component_preview_placement(
        &self,
        component: &CoreComponentInstance,
    ) -> Option<ComponentPreviewPlacement> {
        match &self.pointer_interaction {
            PointerInteraction::MoveDiagramComponent {
                component_id,
                preview_origin,
                ..
            } if component_id == &component.id => Some(ComponentPreviewPlacement {
                origin: *preview_origin,
                rotation: component.rotation,
                extent: component
                    .placement_extent
                    .unwrap_or_else(default_component_extent),
                delta: CorePoint {
                    x: preview_origin.x - component.origin.x,
                    y: preview_origin.y - component.origin.y,
                },
            }),
            PointerInteraction::ResizeDiagramComponent {
                component_id,
                original_component,
                preview_extent,
                ..
            } if component_id == &component.id => Some(ComponentPreviewPlacement {
                origin: original_component.origin,
                rotation: original_component.rotation,
                extent: *preview_extent,
                delta: CorePoint { x: 0.0, y: 0.0 },
            }),
            _ => None,
        }
    }

    fn active_diagram_component_preview(&self) -> Option<(&str, ComponentPreviewPlacement)> {
        let component_id = match &self.pointer_interaction {
            PointerInteraction::MoveDiagramComponent { component_id, .. }
            | PointerInteraction::ResizeDiagramComponent { component_id, .. } => {
                component_id.as_str()
            }
            _ => return None,
        };
        let class_name = self.selected_class_name()?;
        let component = self
            .document
            .as_ref()?
            .diagram(class_name)?
            .components
            .iter()
            .find(|component| component.id == component_id)?;
        self.component_preview_placement(component)
            .map(|preview| (component_id, preview))
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
        let preview = self.component_preview_placement(component);
        Some(ComponentSelectionOverlay {
            origin: preview.map_or(component.origin, |preview| preview.origin),
            extent: preview.map_or_else(
                || {
                    component
                        .placement_extent
                        .unwrap_or_else(default_component_extent)
                },
                |preview| preview.extent,
            ),
            rotation: preview.map_or(component.rotation, |preview| preview.rotation),
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

    fn hit_test_selected_component_body(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
    ) -> Option<(String, String, CorePoint)> {
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
        if !component.editable || !component.visible {
            return None;
        }
        // Placement covers the component's transparent area, including its
        // ports. Only the small active semantic port radius keeps ownership
        // of the actual blue point so selected components remain draggable
        // in the whitespace around dense FluidPort groups.
        if self
            .hit_test_diagram_port_for_component(
                pointer_model,
                component_drag_port_tolerance(self.zoom),
                &component.id,
            )
            .is_some()
        {
            return None;
        }
        point_in_component_placement_extent(component, pointer_model, tolerance).then(|| {
            (
                component.id.clone(),
                component.name.clone(),
                component.origin,
            )
        })
    }

    fn hit_test_selected_component_port(
        &self,
        pointer_model: CorePoint,
        tolerance: f32,
    ) -> Option<PortKey> {
        let component_name = match &self.diagram_selection {
            DiagramSelection::Component(component_name) => component_name,
            _ => return None,
        };
        let class_name = self.selected_class_name()?;
        let component_id = self
            .document
            .as_ref()?
            .diagram(class_name)?
            .components
            .iter()
            .find(|component| component.name == *component_name)
            .map(|component| component.id.as_str())?;
        self.hit_test_diagram_port_for_component(pointer_model, tolerance, component_id)
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
        self.component_connection_previews = None;
        let Some(scene) = document.diagram(&class_name) else {
            return;
        };
        let connected_connections =
            connection_drag_snapshots(scene, &component_name, &class_name, &source_before);
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
        let port_tolerance = component_drag_port_tolerance(self.zoom);
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
                    self.clear_diagram_selection();
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
                if let Some((component_name, handle)) =
                    self.hit_test_selected_component_handle(pointer_model, tolerance)
                {
                    self.begin_component_resize(component_name, handle);
                    return;
                }
                if let Some((component_id, component_name, original_origin)) =
                    self.hit_test_selected_component_body(pointer_model, tolerance)
                {
                    let Some(source_before) = document.class_text(&class_name) else {
                        return;
                    };
                    let Some(scene) = document.diagram(&class_name) else {
                        return;
                    };
                    if let Err(error) = component_connection_drag_preflight(
                        scene,
                        &component_name,
                        &class_name,
                        &source_before,
                    ) {
                        trace_component_edit(
                            "preflight-warning",
                            &component_id,
                            &component_name,
                            &error,
                        );
                    }
                    let connected_connections = connection_drag_snapshots(
                        scene,
                        &component_name,
                        &class_name,
                        &source_before,
                    );
                    let component_connection_previews =
                        document.diagram(&class_name).map(|scene| {
                            ComponentConnectionPreviewSet::new(
                                &self.device,
                                &self.style_layout,
                                scene,
                                &connected_connections,
                                self.zoom,
                            )
                        });
                    self.component_connection_previews = component_connection_previews;
                    self.pointer_interaction = PointerInteraction::MoveDiagramComponent {
                        button: MouseButton::Left,
                        component_id,
                        component_name,
                        start_pointer_model: pointer_model,
                        original_origin,
                        preview_origin: original_origin,
                        preview_delta: CorePoint { x: 0.0, y: 0.0 },
                        connected_connections,
                        source_before,
                    };
                    return;
                }
                if let Some(port) =
                    self.hit_test_selected_component_port(pointer_model, port_tolerance)
                {
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
                if let Some(port) = self.hit_test_diagram_port(pointer_model, port_tolerance) {
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
                let Some((component_id, component_name, original_origin)) =
                    self.hit_test_diagram_component(pointer_model, tolerance)
                else {
                    if let Some(hit) = self.hit_test_diagram_connection(pointer_model, tolerance) {
                        self.begin_connection_edit(hit, pointer_model);
                    } else {
                        self.clear_diagram_selection();
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
                let Some(scene) = document.diagram(&class_name) else {
                    return;
                };
                if let Err(error) = component_connection_drag_preflight(
                    scene,
                    &component_name,
                    &class_name,
                    &source_before,
                ) {
                    trace_component_edit(
                        "preflight-warning",
                        &component_id,
                        &component_name,
                        &error,
                    );
                }
                let connected_connections =
                    connection_drag_snapshots(scene, &component_name, &class_name, &source_before);
                let component_connection_previews = document.diagram(&class_name).map(|scene| {
                    ComponentConnectionPreviewSet::new(
                        &self.device,
                        &self.style_layout,
                        scene,
                        &connected_connections,
                        self.zoom,
                    )
                });
                self.set_diagram_selection(DiagramSelection::Component(component_name.clone()));
                self.component_connection_previews = component_connection_previews;
                self.pointer_interaction = PointerInteraction::MoveDiagramComponent {
                    button: MouseButton::Left,
                    component_id,
                    component_name,
                    start_pointer_model: pointer_model,
                    original_origin,
                    preview_origin: original_origin,
                    preview_delta: CorePoint { x: 0.0, y: 0.0 },
                    connected_connections,
                    source_before,
                };
            }
            MainView::Source => {}
        }
    }

    fn interactive_drag_active(&self) -> bool {
        matches!(
            self.pointer_interaction,
            PointerInteraction::MoveDiagramComponent { .. }
                | PointerInteraction::MoveDiagramConnectionSegment { .. }
                | PointerInteraction::MoveDiagramConnectionCorner { .. }
                | PointerInteraction::CreateDiagramConnection(..)
        )
    }

    fn pointer_interaction_active(&self) -> bool {
        !matches!(self.pointer_interaction, PointerInteraction::None)
    }

    fn request_redraw(&mut self) {
        if let Some(profile) = self.pending_deselect_profile.as_mut() {
            profile.request_count = profile.request_count.saturating_add(1);
        }
        self.window.request_redraw();
    }

    fn deselect_selection_metadata(&self) -> DeselectSelectionMetadata {
        let mut metadata = match &self.diagram_selection {
            DiagramSelection::None => DeselectSelectionMetadata {
                selected_kind: "None",
                selected_component_id: None,
                selected_component_graphic_count: 0,
                selected_component_port_count: 0,
            },
            DiagramSelection::Port(key) => DeselectSelectionMetadata {
                selected_kind: "Port",
                selected_component_id: Some(key.owner_component_id.clone()),
                selected_component_graphic_count: 0,
                selected_component_port_count: self
                    .diagram_hit_cache
                    .ports
                    .iter()
                    .filter(|anchor| anchor.owner_component_id == key.owner_component_id)
                    .count(),
            },
            DiagramSelection::Component(_) => DeselectSelectionMetadata {
                selected_kind: "Component",
                selected_component_id: None,
                selected_component_graphic_count: 0,
                selected_component_port_count: 0,
            },
            DiagramSelection::Connection(_) => DeselectSelectionMetadata {
                selected_kind: "Connection",
                selected_component_id: None,
                selected_component_graphic_count: 0,
                selected_component_port_count: 0,
            },
        };

        let component = match &self.diagram_selection {
            DiagramSelection::Component(component_name) => self
                .document
                .as_ref()
                .and_then(|document| {
                    self.selected_class_name()
                        .and_then(|class_name| document.diagram(class_name))
                })
                .and_then(|scene| {
                    scene
                        .components
                        .iter()
                        .find(|component| component.name == *component_name)
                }),
            DiagramSelection::Port(key) => self
                .document
                .as_ref()
                .and_then(|document| {
                    self.selected_class_name()
                        .and_then(|class_name| document.diagram(class_name))
                })
                .and_then(|scene| {
                    scene
                        .components
                        .iter()
                        .find(|component| component.id == key.owner_component_id)
                }),
            _ => None,
        };
        if let Some(component) = component {
            metadata.selected_component_id = Some(component.id.clone());
            metadata.selected_component_graphic_count = component
                .diagram_layer()
                .map_or(0, |layer| layer.graphics.len());
            metadata.selected_component_port_count = self
                .diagram_hit_cache
                .ports
                .iter()
                .filter(|anchor| anchor.owner_component_id == component.id)
                .count();
        }
        metadata
    }

    fn begin_deselect_profile(
        &mut self,
        metadata: DeselectSelectionMetadata,
        started: Instant,
        selection_completed_at: Instant,
        selection_state: Duration,
        hover_update: Duration,
    ) {
        if !self.deselect_profile.enabled {
            return;
        }
        self.deselect_generation = self.deselect_generation.wrapping_add(1);
        self.pending_deselect_profile = Some(PendingDeselectProfile {
            generation: self.deselect_generation,
            started,
            selection_completed_at,
            selection_state,
            hover_update,
            selected_kind: metadata.selected_kind,
            selected_component_id: metadata.selected_component_id,
            selected_component_graphic_count: metadata.selected_component_graphic_count,
            selected_component_port_count: metadata.selected_component_port_count,
            first_redraw_at: None,
            request_count: 0,
            redraw_count: 0,
        });
    }

    fn begin_deselect_redraw(&mut self) -> Option<DeselectRedrawTiming> {
        let profile = self.pending_deselect_profile.as_mut()?;
        let now = Instant::now();
        profile.redraw_count = profile.redraw_count.saturating_add(1);
        let first_redraw_at = *profile.first_redraw_at.get_or_insert(now);
        Some(DeselectRedrawTiming {
            generation: profile.generation,
            first_redraw_at,
            request_count: profile.request_count,
            redraw_count: profile.redraw_count,
        })
    }

    fn finish_deselect_profile(
        &mut self,
        redraw: DeselectRedrawTiming,
        frame: DeselectFrameTiming,
        finished_at: Instant,
    ) {
        let Some(pending) = self.pending_deselect_profile.take() else {
            return;
        };
        if pending.generation != redraw.generation {
            return;
        }
        let redraw = DeselectRedrawTiming {
            request_count: pending.request_count,
            redraw_count: pending.redraw_count,
            ..redraw
        };
        self.deselect_profile
            .record(pending, redraw, frame, finished_at);
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_cancel_profile(
        &mut self,
        interaction: &'static str,
        connection_count: usize,
        preview_stats: PreviewResourceStats,
        preview_cleanup: Duration,
        started: Instant,
        cancel_completed_at: Instant,
        cancel_cpu: Duration,
    ) {
        if !self.cancel_e2e_profile.enabled {
            return;
        }
        self.cancel_generation = self.cancel_generation.wrapping_add(1);
        self.pending_cancel_profile = Some(PendingCancelProfile {
            generation: self.cancel_generation,
            started,
            cancel_completed_at,
            interaction,
            connection_count,
            preview_stats,
            preview_cleanup,
            cancel_cpu,
            first_redraw_at: None,
            redraw_count: 0,
        });
    }

    fn begin_cancel_redraw(&mut self) -> Option<CancelRedrawTiming> {
        let profile = self.pending_cancel_profile.as_mut()?;
        let now = Instant::now();
        profile.redraw_count = profile.redraw_count.saturating_add(1);
        let first_redraw_at = *profile.first_redraw_at.get_or_insert(now);
        Some(CancelRedrawTiming {
            generation: profile.generation,
            first_redraw_at,
            redraw_count: profile.redraw_count,
            flush_drag_preview: Duration::ZERO,
        })
    }

    fn finish_cancel_profile(
        &mut self,
        redraw: CancelRedrawTiming,
        frame: CancelFrameTiming,
        finished_at: Instant,
    ) {
        let Some(pending) = self.pending_cancel_profile.take() else {
            return;
        };
        if pending.generation != redraw.generation {
            return;
        }
        self.cancel_e2e_profile
            .record(pending, redraw, frame, finished_at);
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
        let component_drag = match &self.pointer_interaction {
            PointerInteraction::MoveDiagramComponent {
                component_id,
                original_origin,
                start_pointer_model,
                ..
            } => Some((component_id.clone(), *original_origin, *start_pointer_model)),
            _ => None,
        };
        if let Some((component_id, original_origin, start_pointer_model)) = component_drag {
            let delta = CorePoint {
                x: current.x - start_pointer_model.x,
                y: current.y - start_pointer_model.y,
            };
            let route_started = Instant::now();
            let preview_routes = match &self.pointer_interaction {
                PointerInteraction::MoveDiagramComponent {
                    connected_connections,
                    ..
                } => connected_connections
                    .iter()
                    .map(|snapshot| {
                        (
                            snapshot.connection_id.clone(),
                            component_drag_preview_route(snapshot, delta),
                        )
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            let route_elapsed = route_started.elapsed();
            // The component is authoritative during a drag. Each connection
            // preview is best effort and must not be allowed to freeze the
            // component when its mesh is missing, full, or otherwise unable
            // to accept this particular route.
            let mut preview_mesh_results = Vec::with_capacity(preview_routes.len());
            if let Some(previews) = self.component_connection_previews.as_mut() {
                for (connection_id, points) in &preview_routes {
                    let updated = valid_interactive_connection_route(points)
                        && previews.update(&self.queue, connection_id, points);
                    preview_mesh_results.push(updated);
                }
            } else {
                preview_mesh_results.resize(preview_routes.len(), false);
            }
            let preview_started = Instant::now();
            self.diagram_scene
                .preview_translation(&self.queue, &component_id, [delta.x, -delta.y]);
            if let PointerInteraction::MoveDiagramComponent {
                preview_origin,
                preview_delta,
                connected_connections,
                ..
            } = &mut self.pointer_interaction
            {
                *preview_origin = CorePoint {
                    x: original_origin.x + delta.x,
                    y: original_origin.y + delta.y,
                };
                *preview_delta = delta;
                for ((snapshot, (_, points)), mesh_updated) in connected_connections
                    .iter_mut()
                    .zip(&preview_routes)
                    .zip(&preview_mesh_results)
                {
                    snapshot.preview_route_valid = valid_interactive_connection_route(points);
                    if !*mesh_updated {
                        let reason = if snapshot.preview_route_valid {
                            "preview mesh rejected route"
                        } else {
                            "invalid route after Manhattan fallback"
                        };
                        trace_component_drag_issue(
                            &component_id,
                            &snapshot.connection_key,
                            reason,
                            snapshot,
                            points,
                        );
                    }
                    snapshot.preview_points = points.clone();
                }
            }
            self.drag_profile.record_preview(
                Duration::ZERO,
                Duration::ZERO,
                route_elapsed,
                Duration::ZERO,
                preview_started.elapsed(),
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
                    endpoint_constraint,
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
                        *endpoint_constraint,
                    );
                    let reanchor_elapsed = reanchor_started.elapsed();
                    let upload_started = Instant::now();
                    let route_accepted = preview
                        .as_mut()
                        .is_some_and(|preview| preview.update(queue, &next_points));
                    profile.record_preview(
                        Duration::ZERO,
                        snap_elapsed,
                        reanchor_elapsed,
                        Duration::ZERO,
                        upload_started.elapsed(),
                    );
                    if route_accepted {
                        *preview_points = next_points;
                    }
                    return true;
                }
                PointerInteraction::MoveDiagramConnectionCorner {
                    corner_index,
                    line_origin,
                    line_rotation,
                    start_pointer_model,
                    original_points,
                    endpoint_constraint,
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
                        *endpoint_constraint,
                    );
                    let reanchor_elapsed = reanchor_started.elapsed();
                    let upload_started = Instant::now();
                    let route_accepted = preview
                        .as_mut()
                        .is_some_and(|preview| preview.update(queue, &next_points));
                    profile.record_preview(
                        Duration::ZERO,
                        Duration::ZERO,
                        reanchor_elapsed,
                        Duration::ZERO,
                        upload_started.elapsed(),
                    );
                    if route_accepted {
                        *preview_points = next_points;
                    }
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
            PointerInteraction::MoveDiagramComponent { .. } => {
                unreachable!("component drags return before the clone fallback")
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
                let placement = effective_component_transform(
                    icon,
                    &original_component,
                    Some(ComponentPreviewPlacement {
                        origin: original_component.origin,
                        rotation: original_component.rotation,
                        extent: preview_extent,
                        delta: CorePoint { x: 0.0, y: 0.0 },
                    }),
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
                                let raw_points = connection
                                    .line
                                    .as_ref()
                                    .map(|line| line.points.clone())
                                    .filter(|points| !points.is_empty())
                                    .unwrap_or_else(|| snapshot.base_route_points.clone());
                                let Ok((preview_points, _)) = resolved_connection_display_route(
                                    &preview_scene,
                                    connection,
                                    &raw_points,
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
            self.zoom,
        );
        self.connection_preview = None;
        self.component_connection_previews = None;
        self.diagram_scene = build_diagram_scene(
            &self.device,
            &self.style_layout,
            self.document.as_ref(),
            self.selected_class.as_deref(),
            self.zoom,
        );
        self.diagram_hit_cache =
            build_diagram_hit_cache(self.document.as_ref(), self.selected_class.as_deref());
        self.refresh_ui_document();
    }

    fn rollback_component_preview(
        &mut self,
        component_id: &str,
        _connected_connections: &[ConnectionDragSnapshot],
    ) -> Duration {
        let started = Instant::now();
        self.diagram_scene
            .preview_translation(&self.queue, component_id, [0.0, 0.0]);
        started.elapsed()
    }

    fn cancel_pointer_interaction(&mut self) -> bool {
        let (interaction_name, connection_count) = match &self.pointer_interaction {
            PointerInteraction::None => return false,
            PointerInteraction::Pan { .. } => ("Pan", 0),
            PointerInteraction::MoveIconGraphic { .. } => ("MoveIconGraphic", 0),
            PointerInteraction::MoveDiagramComponent {
                connected_connections,
                ..
            } => ("MoveDiagramComponent", connected_connections.len()),
            PointerInteraction::MoveDiagramConnectionSegment { .. } => {
                ("MoveDiagramConnectionSegment", 1)
            }
            PointerInteraction::MoveDiagramConnectionCorner { .. } => {
                ("MoveDiagramConnectionCorner", 1)
            }
            PointerInteraction::CreateDiagramConnection(_) => ("CreateDiagramConnection", 0),
            PointerInteraction::ResizeDiagramComponent {
                connected_connections,
                ..
            } => ("ResizeDiagramComponent", connected_connections.len()),
        };
        let total_started = Instant::now();
        let interaction =
            std::mem::replace(&mut self.pointer_interaction, PointerInteraction::None);
        self.pending_drag_position = None;
        let preview_stats = self
            .component_connection_previews
            .as_ref()
            .map(ComponentConnectionPreviewSet::resource_stats)
            .unwrap_or_default();
        let mut rollback_component = Duration::ZERO;
        let mut rollback_connections = Duration::ZERO;
        match interaction {
            PointerInteraction::Pan { start_pan, .. } => {
                let started = Instant::now();
                if self.pan != start_pan {
                    self.pan = start_pan;
                    self.update_view_uniform();
                }
                rollback_component = started.elapsed();
            }
            PointerInteraction::MoveIconGraphic { graphic_id, .. } => {
                let started = Instant::now();
                self.scene
                    .preview_translation(&self.queue, &graphic_id, [0.0, 0.0]);
                rollback_component = started.elapsed();
            }
            PointerInteraction::MoveDiagramComponent {
                component_id,
                connected_connections,
                ..
            } => {
                rollback_component =
                    self.rollback_component_preview(&component_id, &connected_connections);
            }
            PointerInteraction::MoveDiagramConnectionSegment { .. }
            | PointerInteraction::MoveDiagramConnectionCorner { .. } => {}
            PointerInteraction::CreateDiagramConnection(_) => {
                self.connection_creation_profile.finish();
            }
            PointerInteraction::ResizeDiagramComponent {
                component_id,
                original_component,
                connected_connections,
                ..
            } => {
                let started = Instant::now();
                if let Some(icon) = original_component.diagram_layer() {
                    let original_transform = compose_transform(
                        Transform2D {
                            scale_y: -1.0,
                            ..Transform2D::identity()
                        },
                        diagram_placement_transform(icon, &original_component),
                    );
                    self.diagram_scene.preview_component_resize(
                        &self.queue,
                        &component_id,
                        original_transform,
                    );
                }
                rollback_component = started.elapsed();

                let started = Instant::now();
                for snapshot in &connected_connections {
                    let points = if snapshot.base_route_points.len() >= 2 {
                        &snapshot.base_route_points
                    } else {
                        &snapshot.source_line_points
                    };
                    if points.len() >= 2 {
                        self.diagram_scene.preview_connection_points(
                            &self.device,
                            &self.queue,
                            &snapshot.connection_id,
                            points,
                        );
                    }
                }
                rollback_connections = started.elapsed();
            }
            PointerInteraction::None => unreachable!("cancelled interaction was not active"),
        }

        let selection_started = Instant::now();
        if interaction_name == "CreateDiagramConnection" {
            self.hovered_port = None;
        }
        let selection_update = selection_started.elapsed();

        let cleanup_started = Instant::now();
        self.connection_preview = None;
        self.component_connection_previews = None;
        self.connection_creation_preview = None;
        self.pending_waypoint = false;
        self.pending_waypoint_queued_at = None;
        self.waypoint_frame_pending = false;
        self.waypoint_profile_frames_remaining = 0;
        self.suppress_next_pointer_release_redraw = true;
        let preview_cleanup = cleanup_started.elapsed();
        let gpu_upload = rollback_component + rollback_connections;
        let cancel_completed_at = Instant::now();
        let cancel_cpu = cancel_completed_at.saturating_duration_since(total_started);
        trace_interaction_latency("drag_cancel", total_started);
        self.begin_cancel_profile(
            interaction_name,
            connection_count,
            preview_stats,
            preview_cleanup,
            total_started,
            cancel_completed_at,
            cancel_cpu,
        );
        trace_cancel_profile(
            interaction_name,
            connection_count,
            cancel_cpu,
            rollback_component,
            rollback_connections,
            preview_cleanup,
            selection_update,
            Duration::ZERO,
            Duration::ZERO,
            gpu_upload,
        );
        true
    }

    fn cancel_connection_creation(&mut self) -> bool {
        self.connection_creation_active() && self.cancel_pointer_interaction()
    }

    fn clear_diagram_selection(&mut self) -> bool {
        let total_started = Instant::now();
        let metadata = self
            .deselect_profile
            .enabled
            .then(|| self.deselect_selection_metadata());
        let selection_started = Instant::now();
        let selection_changed = self.set_diagram_selection(DiagramSelection::None);
        let selection_state = selection_started.elapsed();
        let hover_started = Instant::now();
        let hover_changed = self.hovered_port.take().is_some();
        let hover_update = hover_started.elapsed();
        let changed = selection_changed || hover_changed;
        if changed {
            trace_interaction_latency("deselect", total_started);
        }
        if changed && self.deselect_profile.enabled {
            let completed_at = Instant::now();
            self.begin_deselect_profile(
                metadata.unwrap_or(DeselectSelectionMetadata {
                    selected_kind: "None",
                    selected_component_id: None,
                    selected_component_graphic_count: 0,
                    selected_component_port_count: 0,
                }),
                total_started,
                completed_at,
                selection_state,
                hover_update,
            );
        }
        changed
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
        self.component_connection_previews = None;
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
                original_origin,
                preview_origin,
                connected_connections,
                source_before,
                ..
            } => {
                self.commit_diagram_component_move(
                    component_id,
                    component_name,
                    original_origin,
                    preview_origin,
                    source_before,
                    connected_connections,
                );
            }
            PointerInteraction::MoveDiagramConnectionSegment {
                connection_id: _connection_id,
                connection_key,
                original_points,
                preview_points,
                endpoint_constraint,
                source_before,
                ..
            } => {
                self.commit_diagram_connection_move(
                    connection_key,
                    original_points,
                    preview_points,
                    endpoint_constraint,
                    source_before,
                );
            }
            PointerInteraction::MoveDiagramConnectionCorner {
                connection_id: _connection_id,
                connection_key,
                endpoint_constraint,
                original_points,
                preview_points,
                source_before,
                ..
            } => {
                self.commit_diagram_connection_move(
                    connection_key,
                    original_points,
                    preview_points,
                    endpoint_constraint,
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
        document.replace_icon(&class_name, resolved_icon);
        document.replace_diagram(&class_name, resolved_diagram);
        record_successful_edit(
            &mut self.history,
            &mut self.redo_history,
            EditCommand::CreateDiagramConnection {
                class_name,
                connection_key,
                before_source: source_before,
                after_source: candidate,
            },
        );
        self.load_error = None;
        self.status_message = None;
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
        document.replace_icon(&class_name, resolved_icon);
        document.replace_diagram(&class_name, resolved_diagram);
        record_successful_edit(
            &mut self.history,
            &mut self.redo_history,
            EditCommand::MoveIconGraphic {
                class_name,
                graphic_id,
                before_geometry,
                after_geometry,
                before_source: source_before,
                after_source: candidate,
            },
        );
        self.load_error = None;
        self.status_message = None;
        self.rebuild_selected_scenes();
    }

    fn commit_diagram_component_move(
        &mut self,
        component_id: String,
        component_name: String,
        before_origin: CorePoint,
        after_origin: CorePoint,
        source_before: String,
        connected_connections: Vec<ConnectionDragSnapshot>,
    ) {
        let mut profile = ComponentCommitProfile::new(connected_connections.len());
        trace_component_edit(
            "commit-begin",
            &component_id,
            &component_name,
            format_args!(
                "before={before_origin:?} after={after_origin:?} connections={}",
                connected_connections.len()
            ),
        );
        let delta = CorePoint {
            x: after_origin.x - before_origin.x,
            y: after_origin.y - before_origin.y,
        };
        if delta_is_zero(delta) {
            return;
        }
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            self.load_error = Some("Diagram edit has no selected class".into());
            let _ = self.rollback_component_preview(&component_id, &connected_connections);
            return;
        };
        let Some(version) = self
            .document
            .as_ref()
            .map(|document| document.source_version(&class_name))
        else {
            self.load_error = Some("Diagram edit has no source version".into());
            let _ = self.rollback_component_preview(&component_id, &connected_connections);
            return;
        };
        let Some(current_scene) = self
            .document
            .as_ref()
            .and_then(|document| document.diagram(&class_name))
            .cloned()
        else {
            self.load_error = Some("Diagram edit has no selected Diagram".into());
            let _ = self.rollback_component_preview(&component_id, &connected_connections);
            return;
        };
        if !current_scene
            .components
            .iter()
            .any(|component| component.id == component_id)
        {
            self.load_error = Some("Diagram edit failed: component origin mismatch".into());
            let _ = self.rollback_component_preview(&component_id, &connected_connections);
            return;
        }

        let component_edit_started = Instant::now();
        let component_edit =
            match component_origin_edit(&source_before, &component_name, after_origin) {
                Ok(edit) => edit,
                Err(error) => {
                    self.load_error = Some(format!("Diagram edit rejected: {error}"));
                    let _ = self.rollback_component_preview(&component_id, &connected_connections);
                    return;
                }
            };
        profile.component_edit_build = component_edit_started.elapsed();

        // All connection ranges and expected text come from the same source and
        // scene snapshot. Do not resolve a candidate while building this list.
        let connection_edit_started = Instant::now();
        let connection_source_edits = match build_component_connection_edits(
            &current_scene,
            &class_name,
            &source_before,
            &connected_connections,
        ) {
            Ok(edits) => edits,
            Err(error) => {
                trace_component_edit(
                    "connection-source-edit-rejected",
                    &component_id,
                    &component_name,
                    &error,
                );
                self.load_error = Some(format!("Diagram edit rejected: {error}"));
                let _ = self.rollback_component_preview(&component_id, &connected_connections);
                return;
            }
        };
        profile.connection_edit_build = connection_edit_started.elapsed();

        let mut source_edits = Vec::with_capacity(1 + connection_source_edits.len());
        source_edits.push(component_edit);
        source_edits.extend(
            connection_source_edits
                .iter()
                .map(|(_, source_edit)| source_edit.clone()),
        );
        let validated_started = Instant::now();
        let validated =
            match apply_validated_source_edits_with_parsed(&source_before, source_edits, version) {
                Ok(validated) => validated,
                Err(error) => {
                    profile.transaction_apply += validated_started.elapsed();
                    self.load_error = Some(format!("Diagram edit rejected: {error}"));
                    let _ = self.rollback_component_preview(&component_id, &connected_connections);
                    return;
                }
            };
        profile.transaction_apply += validated.transaction_apply;
        profile.parse += validated.parse;
        profile.parse_count += 1;
        let candidate = validated.source;
        let resolve_started = Instant::now();
        profile.resolve_count += 1;
        let scenes = self.document.as_ref().and_then(|document| {
            document
                .resolve_candidate_scenes_from_parsed(&class_name, &candidate, &validated.parsed)
                .ok()
        });
        profile.scene_resolve += resolve_started.elapsed();
        let Some((resolved_icon, resolved_diagram)) = scenes else {
            self.load_error = Some("Diagram edit could not resolve candidate source".into());
            let _ = self.rollback_component_preview(&component_id, &connected_connections);
            return;
        };

        let validation_started = Instant::now();
        let Some(resolved_component) = resolved_diagram
            .components
            .iter()
            .find(|component| component.id == component_id)
        else {
            profile.validation += validation_started.elapsed();
            self.load_error = Some("Diagram edit failed: component origin mismatch".into());
            let _ = self.rollback_component_preview(&component_id, &connected_connections);
            return;
        };
        if !point_nearly_equal(resolved_component.origin, after_origin) {
            profile.validation += validation_started.elapsed();
            self.load_error = Some("Diagram edit failed: component origin mismatch".into());
            let _ = self.rollback_component_preview(&component_id, &connected_connections);
            return;
        }
        let invalid_connections = connection_source_edits
            .iter()
            .filter_map(|(edit, _)| {
                let connection = resolved_diagram
                    .connections
                    .iter()
                    .find(|connection| connection.key == edit.connection_key);
                match connection {
                    Some(connection) => connection_invariant_failure(
                        &resolved_diagram,
                        connection,
                        &edit.after_points,
                    )
                    .map(|reason| (edit.connection_key.clone(), reason)),
                    None => Some((
                        edit.connection_key.clone(),
                        "connection identity is no longer present",
                    )),
                }
            })
            .collect::<Vec<_>>();
        profile.validation += validation_started.elapsed();
        if let Some((key, reason)) = invalid_connections.first() {
            trace_component_edit(
                "connection-validation-rejected",
                &component_id,
                &component_name,
                format_args!("key={key:?} reason={reason}"),
            );
            self.load_error = Some(format!("Diagram edit failed: connection {key:?}: {reason}"));
            let _ = self.rollback_component_preview(&component_id, &connected_connections);
            return;
        }
        let connection_edits = connection_source_edits
            .into_iter()
            .map(|(connection_edit, _)| connection_edit)
            .collect::<Vec<_>>();
        let canonical_after_origin = resolved_diagram
            .components
            .iter()
            .find(|component| component.id == component_id)
            .map_or(after_origin, |component| component.origin);
        let mut connection_edits = connection_edits;
        for edit in &mut connection_edits {
            if let Some(line) = resolved_diagram
                .connections
                .iter()
                .find(|connection| connection.key == edit.connection_key)
                .and_then(|connection| connection.line.as_ref())
            {
                // History must contain the points that the batch transaction
                // actually persisted, not a display fallback route.
                edit.after_points = line.points.clone();
                edit.line_origin = line.origin;
            }
        }

        let mut visual_connection_commits = Vec::with_capacity(connected_connections.len());
        for snapshot in &connected_connections {
            let Some(connection) = resolved_diagram
                .connections
                .iter()
                .find(|connection| connection.key == snapshot.connection_key)
            else {
                continue;
            };
            let raw_points = valid_interactive_connection_route(&snapshot.preview_points)
                .then(|| snapshot.preview_points.clone())
                .or_else(|| connection.line.as_ref().map(|line| line.points.clone()))
                .filter(|points| !points.is_empty())
                .unwrap_or_else(|| snapshot.base_route_points.clone());
            if let Ok((route, fallback)) =
                resolved_connection_display_route(&resolved_diagram, connection, &raw_points)
            {
                trace_connection_reanchor(
                    &component_id,
                    snapshot,
                    raw_points
                        .first()
                        .zip(raw_points.last())
                        .map(|(first, last)| (*first, *last)),
                    strict_connection_points(&resolved_diagram, connection).ok(),
                    route
                        .first()
                        .zip(route.last())
                        .map(|(first, last)| (*first, *last)),
                    fallback,
                );
                visual_connection_commits.push((connection.id.clone(), route));
            }
        }

        let component_transform = {
            let Some(document) = self.document.as_mut() else {
                let _ = self.rollback_component_preview(&component_id, &connected_connections);
                return;
            };
            document.set_class_text(&class_name, candidate.clone());
            document.replace_icon(&class_name, resolved_icon);
            document.replace_diagram(&class_name, resolved_diagram);
            document
                .diagram(&class_name)
                .and_then(|scene| {
                    scene
                        .components
                        .iter()
                        .find(|component| component.id == component_id)
                })
                .and_then(|component| {
                    component.diagram_layer().map(|layer| {
                        compose_transform(
                            Transform2D {
                                scale_y: -1.0,
                                ..Transform2D::identity()
                            },
                            diagram_placement_transform(layer, component),
                        )
                    })
                })
        };
        let gpu_component_started = Instant::now();
        self.diagram_scene.commit_translation(
            &self.queue,
            &component_id,
            [
                canonical_after_origin.x - before_origin.x,
                -(canonical_after_origin.y - before_origin.y),
            ],
            component_transform,
        );
        profile.gpu_component_commit = gpu_component_started.elapsed();
        let gpu_connections_started = Instant::now();
        for (connection_id, points) in visual_connection_commits {
            self.diagram_scene.commit_connection_points(
                &self.device,
                &self.queue,
                &connection_id,
                &points,
            );
        }
        profile.gpu_connections_commit = gpu_connections_started.elapsed();
        let source_connection_edit_count = connection_edits.len();
        trace_component_edit(
            "commit-ok",
            &component_id,
            &component_name,
            format_args!("delta={delta:?} source_connection_edits={source_connection_edit_count}"),
        );
        record_successful_edit(
            &mut self.history,
            &mut self.redo_history,
            EditCommand::MoveDiagramComponent {
                class_name,
                component_id,
                before_origin,
                after_origin: canonical_after_origin,
                before_source: source_before,
                after_source: candidate,
                connection_edits,
            },
        );
        self.load_error = None;
        self.status_message = None;
        let hit_cache_started = Instant::now();
        self.diagram_hit_cache =
            build_diagram_hit_cache(self.document.as_ref(), self.selected_class.as_deref());
        profile.hit_cache = hit_cache_started.elapsed();
        let ui_refresh_started = Instant::now();
        self.refresh_ui_document();
        profile.ui_refresh = ui_refresh_started.elapsed();
    }

    fn commit_diagram_connection_move(
        &mut self,
        connection_key: ConnectionKey,
        before_points: Vec<CorePoint>,
        after_points: Vec<CorePoint>,
        endpoint_constraint: ConnectionEndpointConstraint,
        source_before: String,
    ) {
        trace_connection_edit(
            "commit-begin",
            &connection_key,
            endpoint_constraint,
            format_args!(
                "before_len={} after_len={} before={before_points:?} after={after_points:?}",
                before_points.len(),
                after_points.len(),
            ),
        );
        if before_points == after_points {
            trace_connection_edit(
                "noop",
                &connection_key,
                endpoint_constraint,
                "before == after",
            );
            return;
        }
        let mut profile = EditCommitProfile::new();
        let Some(class_name) = self.selected_class_name().map(str::to_owned) else {
            self.load_error = Some("Connection edit has no selected class".into());
            return;
        };
        let Some(version) = self
            .document
            .as_ref()
            .map(|document| document.source_version(&class_name))
        else {
            self.load_error = Some("Connection edit has no source version".into());
            return;
        };
        let Some(current_scene) = self
            .document
            .as_ref()
            .and_then(|document| document.diagram(&class_name))
        else {
            self.load_error = Some("Connection edit has no selected Diagram".into());
            return;
        };
        let Some(cache_connection_index) = current_scene
            .connections
            .iter()
            .position(|connection| connection.key == connection_key)
        else {
            self.load_error = Some("Connection edit lost its connection identity".into());
            return;
        };
        let Some(connection) = current_scene.connections.get(cache_connection_index) else {
            self.load_error = Some("Connection edit lost its connection identity".into());
            return;
        };
        if let Err(error) =
            connection_source_editable_in_class(connection, &class_name, &source_before)
        {
            self.load_error = Some(connection_edit_diagnostic(
                connection,
                &class_name,
                Some(endpoint_constraint),
                "preflight",
                error,
            ));
            return;
        }
        let after_points = match finalize_connection_route_with_constraint(
            current_scene,
            connection,
            &after_points,
            endpoint_constraint,
        ) {
            Ok(points) => points,
            Err(error) => {
                trace_connection_edit(
                    "finalize-rejected",
                    &connection_key,
                    endpoint_constraint,
                    &error,
                );
                self.load_error = Some(connection_edit_diagnostic(
                    connection,
                    &class_name,
                    Some(endpoint_constraint),
                    "finalize",
                    error,
                ));
                return;
            }
        };
        trace_connection_edit(
            "finalize-ok",
            &connection_key,
            endpoint_constraint,
            format_args!("points={after_points:?}"),
        );
        let patch_started = Instant::now();
        let edit = match connection_points_edit_for_key(
            &source_before,
            current_scene,
            &connection_key,
            &after_points,
        ) {
            Ok(edit) => edit,
            Err(error) => {
                trace_connection_edit(
                    "source-edit-rejected",
                    &connection_key,
                    endpoint_constraint,
                    &error,
                );
                self.load_error = Some(connection_edit_diagnostic(
                    connection,
                    &class_name,
                    Some(endpoint_constraint),
                    "source-edit",
                    error,
                ));
                return;
            }
        };
        trace_connection_edit(
            "source-edit-ok",
            &connection_key,
            endpoint_constraint,
            format_args!("range={}..{}", edit.start, edit.end),
        );
        let candidate = match apply_validated_source_edits(&source_before, vec![edit], version) {
            Ok(candidate) => candidate,
            Err(error) => {
                trace_connection_edit(
                    "transaction-rejected",
                    &connection_key,
                    endpoint_constraint,
                    &error,
                );
                self.load_error = Some(connection_edit_diagnostic(
                    connection,
                    &class_name,
                    Some(endpoint_constraint),
                    "transaction",
                    error,
                ));
                return;
            }
        };
        trace_connection_edit(
            "transaction-ok",
            &connection_key,
            endpoint_constraint,
            format_args!("candidate_len={}", candidate.len()),
        );
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
                trace_connection_edit(
                    "candidate-resolve-rejected",
                    &connection_key,
                    endpoint_constraint,
                    "could not resolve candidate source",
                );
                self.load_error = Some(connection_edit_diagnostic(
                    connection,
                    &class_name,
                    Some(endpoint_constraint),
                    "candidate-resolve",
                    "could not resolve candidate source",
                ));
                return;
            }
        };
        trace_connection_edit(
            "candidate-resolve-ok",
            &connection_key,
            endpoint_constraint,
            format_args!("connections={}", resolved_diagram.connections.len()),
        );
        if profile.enabled {
            profile.resolve_candidate = resolve_started.elapsed();
        }
        let validation_started = Instant::now();
        let Some(resolved_connection_index) = resolved_diagram
            .connections
            .iter()
            .position(|connection| connection.key == connection_key)
        else {
            self.load_error = Some(connection_edit_diagnostic(
                connection,
                &class_name,
                Some(endpoint_constraint),
                "identity",
                "connection identity is no longer present",
            ));
            return;
        };
        let Some(connection) = resolved_diagram.connections.get(resolved_connection_index) else {
            self.load_error = Some(connection_edit_diagnostic(
                connection,
                &class_name,
                Some(endpoint_constraint),
                "identity",
                "connection identity is no longer present",
            ));
            return;
        };
        if let Some(reason) = connection_invariant_failure_with_constraint(
            &resolved_diagram,
            connection,
            &after_points,
            Some(endpoint_constraint),
        ) {
            trace_connection_edit(
                "validation-rejected",
                &connection_key,
                endpoint_constraint,
                reason,
            );
            self.load_error = Some(connection_edit_diagnostic(
                connection,
                &class_name,
                Some(endpoint_constraint),
                "validation",
                reason,
            ));
            return;
        }
        let Some(canonical_line) = connection.line.clone() else {
            self.load_error = Some(connection_edit_diagnostic(
                connection,
                &class_name,
                Some(endpoint_constraint),
                "validation",
                "Line annotation is missing",
            ));
            return;
        };
        let Some((semantic_points, display_points)) =
            connection_geometry_points(&resolved_diagram, connection, self.zoom)
        else {
            self.load_error = Some(connection_edit_diagnostic(
                connection,
                &class_name,
                Some(endpoint_constraint),
                "validation",
                "unable to construct a display route from the resolved connection",
            ));
            return;
        };
        trace_connection_edit(
            "validation-ok",
            &connection_key,
            endpoint_constraint,
            format_args!(
                "source_points={:?} semantic_points={semantic_points:?} display_points={display_points:?}",
                canonical_line.points
            ),
        );
        let source_points = canonical_line.points.clone();
        let connection_id = connection.id.clone();
        if profile.enabled {
            profile.semantic_validation = validation_started.elapsed();
        }
        let document_started = Instant::now();
        let Some(document) = self.document.as_mut() else {
            self.load_error = Some("Connection edit lost its document".into());
            return;
        };
        document.set_class_text(&class_name, candidate.clone());
        document.replace_icon(&class_name, resolved_icon);
        document.replace_diagram(&class_name, resolved_diagram);
        if profile.enabled {
            profile.document_update = document_started.elapsed();
        }
        let gpu_started = Instant::now();
        self.diagram_scene.commit_connection_points(
            &self.device,
            &self.queue,
            &connection_id,
            &display_points,
        );
        trace_connection_edit(
            "gpu-commit-ok",
            &connection_key,
            endpoint_constraint,
            format_args!("display_points={display_points:?}"),
        );
        if profile.enabled {
            profile.gpu_update = gpu_started.elapsed();
        }
        let hit_index_started = Instant::now();
        let mut hit_line = canonical_line.clone();
        hit_line.points = semantic_points;
        self.diagram_hit_cache
            .update_connection(cache_connection_index, &hit_line);
        if profile.enabled {
            profile.hit_index_update = hit_index_started.elapsed();
        }
        record_successful_edit(
            &mut self.history,
            &mut self.redo_history,
            EditCommand::MoveDiagramConnection {
                class_name,
                connection_key,
                before_points,
                after_points: source_points,
                endpoint_constraint,
            },
        );
        self.load_error = None;
        self.status_message = None;
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
            if !snapshot.source_editable {
                trace_component_edit(
                    "connection-source-read-only",
                    &component_id,
                    &component_name,
                    format_args!(
                        "key={:?} reason={:?}; resize visual route is canonicalized after resolve",
                        snapshot.connection_key, snapshot.source_edit_error
                    ),
                );
                continue;
            }
            let after_points = match reanchor_connection_points(
                &reanchored_scene,
                connection,
                &snapshot.base_route_points,
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
                before_points: snapshot.source_line_points.clone(),
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
        document.replace_icon(&class_name, resolved_icon);
        document.replace_diagram(&class_name, resolved_diagram);
        record_successful_edit(
            &mut self.history,
            &mut self.redo_history,
            EditCommand::ResizeDiagramComponent {
                class_name,
                component_id,
                before_extent,
                after_extent,
                before_source: source_before,
                after_source: candidate,
                connection_edits,
            },
        );
        self.load_error = None;
        self.status_message = None;
        self.rebuild_selected_scenes();
    }

    fn apply_edit_command(&mut self, command: &EditCommand, after: bool) -> Result<(), String> {
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
                let document = self
                    .document
                    .as_ref()
                    .ok_or_else(|| "icon undo/redo failed: no document is open".to_owned())?;
                let (resolved_icon, resolved_diagram) = document
                    .resolve_candidate_scenes(class_name, source)
                    .map_err(|error| format!("icon undo/redo candidate failed: {error}"))?;
                if !resolved_icon.graphics.iter().any(|graphic| {
                    graphic.id.0 == *graphic_id && graphic.graphic == *expected_geometry
                }) {
                    return Err(format!(
                        "icon undo/redo failed: graphic `{graphic_id}` no longer matches"
                    ));
                }
                let Some(document) = self.document.as_mut() else {
                    return Err("icon undo/redo failed: document disappeared".to_owned());
                };
                document.set_class_text(class_name, source.clone());
                document.replace_icon(class_name, resolved_icon);
                document.replace_diagram(class_name, resolved_diagram);
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
                let document = self
                    .document
                    .as_ref()
                    .ok_or_else(|| "component undo/redo failed: no document is open".to_owned())?;
                let (resolved_icon, resolved_diagram) = document
                    .resolve_candidate_scenes(class_name, source)
                    .map_err(|error| format!("component undo/redo candidate failed: {error}"))?;
                if !resolved_diagram.components.iter().any(|component| {
                    component.id == *component_id
                        && point_nearly_equal(component.origin, expected_origin)
                }) {
                    return Err(format!(
                        "component undo/redo failed: component `{component_id}` no longer matches"
                    ));
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
                        return Err(format!(
                            "component undo/redo failed: connection {:?} is missing",
                            edit.connection_key
                        ));
                    };
                    let Some(line) = connection.line.as_ref() else {
                        return Err(format!(
                            "component undo/redo failed: connection {:?} has no Line",
                            edit.connection_key
                        ));
                    };
                    if !point_nearly_equal(line.origin, edit.line_origin)
                        || !connection_points_match_invariants(
                            &resolved_diagram,
                            connection,
                            expected_points,
                        )
                    {
                        return Err(format!(
                            "component undo/redo failed: connection {:?} geometry no longer matches",
                            edit.connection_key
                        ));
                    }
                }
                let Some(document) = self.document.as_mut() else {
                    return Err("component undo/redo failed: document disappeared".to_owned());
                };
                document.set_class_text(class_name, source.clone());
                document.replace_icon(class_name, resolved_icon);
                document.replace_diagram(class_name, resolved_diagram);
            }
            EditCommand::MoveDiagramConnection {
                class_name,
                connection_key,
                before_points,
                after_points,
                endpoint_constraint,
            } => {
                let expected_points = if after { after_points } else { before_points };
                let Some(document) = self.document.as_ref() else {
                    return Err("connection undo/redo failed: no document is open".to_owned());
                };
                let Some(source) = document.class_text(class_name) else {
                    return Err(format!(
                        "connection undo/redo failed: class `{class_name}` is missing"
                    ));
                };
                let Some(scene) = document.diagram(class_name) else {
                    return Err(format!(
                        "connection undo/redo failed: class `{class_name}` has no Diagram"
                    ));
                };
                let version = document.source_version(class_name);
                let edit =
                    connection_points_edit_for_key(&source, scene, connection_key, expected_points)
                        .map_err(|error| {
                            format!("connection undo/redo source is stale: {error}")
                        })?;
                let candidate = apply_validated_source_edits(&source, vec![edit], version)
                    .map_err(|error| {
                        format!("connection undo/redo source patch failed: {error}")
                    })?;
                let (resolved_icon, resolved_diagram) = document
                    .resolve_candidate_scenes(class_name, &candidate)
                    .map_err(|error| format!("connection undo/redo candidate failed: {error}"))?;
                let Some(connection) = resolved_diagram
                    .connections
                    .iter()
                    .find(|connection| connection.key == *connection_key)
                else {
                    return Err(format!(
                        "connection undo/redo failed: connection {:?} is missing",
                        connection_key
                    ));
                };
                if let Some(reason) = connection_invariant_failure_with_constraint(
                    &resolved_diagram,
                    connection,
                    expected_points,
                    Some(*endpoint_constraint),
                ) {
                    return Err(format!(
                        "connection undo/redo failed: geometry validation failed: {reason}"
                    ));
                }
                let Some(document) = self.document.as_mut() else {
                    return Err("connection undo/redo failed: document disappeared".to_owned());
                };
                document.set_class_text(class_name, candidate);
                document.replace_icon(class_name, resolved_icon);
                document.replace_diagram(class_name, resolved_diagram);
            }
            EditCommand::CreateDiagramConnection {
                class_name,
                connection_key,
                before_source,
                after_source,
            } => {
                let source = if after { after_source } else { before_source };
                let document = self.document.as_ref().ok_or_else(|| {
                    "connection creation undo/redo failed: no document is open".to_owned()
                })?;
                let (resolved_icon, resolved_diagram) = document
                    .resolve_candidate_scenes(class_name, source)
                    .map_err(|error| {
                        format!("connection creation undo/redo candidate failed: {error}")
                    })?;
                let has_connection = resolved_diagram
                    .connections
                    .iter()
                    .any(|connection| connection.key == *connection_key);
                if has_connection != after {
                    return Err(format!(
                        "connection creation undo/redo failed: connection {:?} presence mismatch",
                        connection_key
                    ));
                }
                let Some(document) = self.document.as_mut() else {
                    return Err(
                        "connection creation undo/redo failed: document disappeared".to_owned()
                    );
                };
                document.set_class_text(class_name, source.clone());
                document.replace_icon(class_name, resolved_icon);
                document.replace_diagram(class_name, resolved_diagram);
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
                let document = self
                    .document
                    .as_ref()
                    .ok_or_else(|| "resize undo/redo failed: no document is open".to_owned())?;
                let (resolved_icon, resolved_diagram) = document
                    .resolve_candidate_scenes(class_name, source)
                    .map_err(|error| format!("resize undo/redo candidate failed: {error}"))?;
                if !resolved_diagram.components.iter().any(|component| {
                    component.id == *component_id
                        && component.placement_extent == Some(*expected_extent)
                }) {
                    return Err(format!(
                        "resize undo/redo failed: component `{component_id}` extent no longer matches"
                    ));
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
                        return Err(format!(
                            "resize undo/redo failed: connection {:?} is missing",
                            edit.connection_key
                        ));
                    };
                    let Some(line) = connection.line.as_ref() else {
                        return Err(format!(
                            "resize undo/redo failed: connection {:?} has no Line",
                            edit.connection_key
                        ));
                    };
                    if !point_nearly_equal(line.origin, edit.line_origin)
                        || !connection_points_match_invariants(
                            &resolved_diagram,
                            connection,
                            expected_points,
                        )
                    {
                        return Err(format!(
                            "resize undo/redo failed: connection {:?} geometry no longer matches",
                            edit.connection_key
                        ));
                    }
                }
                let Some(document) = self.document.as_mut() else {
                    return Err("resize undo/redo failed: document disappeared".to_owned());
                };
                document.set_class_text(class_name, source.clone());
                document.replace_icon(class_name, resolved_icon);
                document.replace_diagram(class_name, resolved_diagram);
            }
        }
        self.load_error = None;
        self.status_message = None;
        self.rebuild_selected_scenes();
        Ok(())
    }

    fn undo(&mut self) {
        let Some(command) = self.history.last().cloned() else {
            return;
        };
        match self.apply_edit_command(&command, false) {
            Ok(()) => {
                self.history.pop();
                self.redo_history.push(command);
            }
            Err(error) => {
                self.load_error = Some(format!("Undo failed: {error}"));
            }
        }
    }

    fn redo(&mut self) {
        let Some(command) = self.redo_history.last().cloned() else {
            return;
        };
        match self.apply_edit_command(&command, true) {
            Ok(()) => {
                self.redo_history.pop();
                self.history.push(command);
            }
            Err(error) => {
                self.load_error = Some(format!("Redo failed: {error}"));
            }
        }
    }

    fn persist_edits(&mut self) -> Result<usize, String> {
        let result = match self.document.as_mut() {
            Some(document) => save_edited_classes(document),
            None => Err("no Modelica document is open".to_owned()),
        };
        self.refresh_ui_document();
        self.update_title(None);
        result
    }

    fn discard_unsaved_changes(&mut self) {
        if let Some(document) = self.document.as_mut() {
            document.discard_unsaved_changes();
        }
        self.rebuild_selected_scenes();
        self.update_title(None);
    }

    fn has_unsaved_changes(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(LoadedDocument::has_unsaved_changes)
    }

    fn request_document_open(&mut self, path: PathBuf) {
        if self.loading_document.is_some() || self.pending_document_action.is_some() {
            return;
        }
        if self.has_unsaved_changes() {
            self.pending_document_action = Some(PendingDocumentAction::Open(path));
            self.status_message = Some("当前文档有未保存修改".to_owned());
            self.request_redraw();
        } else {
            self.begin_document_load(path);
        }
    }

    fn request_window_close(&mut self) {
        if self.pending_document_action.is_some() {
            return;
        }
        if self.has_unsaved_changes() {
            self.pending_document_action = Some(PendingDocumentAction::Close);
            self.status_message = Some("关闭前需要处理未保存修改".to_owned());
            self.request_redraw();
        } else {
            self.exit_requested = true;
        }
    }

    fn execute_document_action(&mut self, action: PendingDocumentAction) {
        match action {
            PendingDocumentAction::Open(path) => self.begin_document_load(path),
            PendingDocumentAction::Close => self.exit_requested = true,
        }
    }

    fn handle_leave_decision(&mut self, decision: LeaveDecision) {
        let Some(action) = self.pending_document_action.clone() else {
            return;
        };
        match decision {
            LeaveDecision::Cancel => {
                self.pending_document_action = None;
                self.status_message = Some("已取消离开操作".to_owned());
            }
            LeaveDecision::Discard => {
                self.discard_unsaved_changes();
                self.pending_document_action = None;
                self.status_message = Some("已放弃本次未保存修改".to_owned());
                self.execute_document_action(action);
            }
            LeaveDecision::Save => match self.persist_edits() {
                Ok(saved) if !self.has_unsaved_changes() => {
                    self.pending_document_action = None;
                    self.status_message = Some(if saved == 0 {
                        "没有待保存的修改".to_owned()
                    } else {
                        format!("保存成功：已写入 {saved} 个文件")
                    });
                    self.load_error = None;
                    self.execute_document_action(action);
                }
                Ok(_) => {
                    self.status_message = Some("保存部分成功：仍有修改未写入".to_owned());
                }
                Err(error) => {
                    self.status_message = Some(if error.contains("save partially completed") {
                        "保存部分成功：仍有修改未写入".to_owned()
                    } else {
                        "保存失败：当前修改已保留".to_owned()
                    });
                    self.load_error = Some(error);
                }
            },
        }
        self.request_redraw();
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
        self.msaa_view = create_msaa_view(&self.device, &self.config, self.msaa_samples);
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
            self.rebuild_scene_geometry_for_zoom();
            self.update_view_uniform();
            return;
        };
        let size = bounds.size();
        let target_x = self.config.width as f32 * 0.50;
        let target_y = self.config.height as f32 * 0.52;
        let width = size[0].abs().max(f32::EPSILON);
        let height = size[1].abs().max(f32::EPSILON);
        let width_zoom = self.config.width as f32 * FIT_SCENE_FILL / width;
        let height_zoom = self.config.height as f32 * FIT_SCENE_FILL / height;
        self.zoom = width_zoom.min(height_zoom).clamp(MIN_ZOOM, MAX_ZOOM);
        let center = bounds.center();
        self.pan = [
            target_x - self.config.width as f32 * 0.5 - center[0] * self.zoom,
            target_y - self.config.height as f32 * 0.5 - center[1] * self.zoom,
        ];
        self.rebuild_scene_geometry_for_zoom();
        self.update_view_uniform();
    }

    fn rebuild_scene_geometry_for_zoom(&mut self) {
        let selected_class = self.selected_class.clone();
        let document = self.document.as_ref();
        match self.main_view {
            MainView::Icon => {
                self.scene = build_scene(
                    &self.device,
                    &self.style_layout,
                    document,
                    selected_class.as_deref(),
                    self.zoom,
                );
            }
            MainView::Diagram => {
                self.diagram_scene = build_diagram_scene(
                    &self.device,
                    &self.style_layout,
                    document,
                    selected_class.as_deref(),
                    self.zoom,
                );
            }
            MainView::Source => return,
        }
        self.refresh_preview_geometry_for_zoom();
    }

    fn refresh_preview_geometry_for_zoom(&mut self) {
        if let Some(preview) = self.connection_preview.as_mut() {
            preview.set_zoom(&self.queue, self.zoom);
        }
        if let Some(previews) = self.component_connection_previews.as_mut() {
            previews.set_zoom(&self.queue, self.zoom);
        }
        let creation_points = match &self.pointer_interaction {
            PointerInteraction::CreateDiagramConnection(creation) => {
                let mut points = Vec::with_capacity(creation.committed_points.len() + 2);
                append_connection_creation_route_with_orientation(
                    &creation.committed_points,
                    creation.cursor_point,
                    creation.tail_orientation,
                    &mut points,
                );
                Some(points)
            }
            _ => None,
        };
        if let (Some(preview), Some(points)) = (
            self.connection_creation_preview.as_mut(),
            creation_points.as_deref(),
        ) {
            preview.set_zoom(&self.queue, points, self.zoom);
        }
    }

    fn zoom_at_cursor(&mut self, wheel_delta: f32) {
        if !self.canvas_navigation_enabled() {
            return;
        }
        self.zoom_about_screen_point(
            zoom_after_wheel(self.zoom, wheel_delta),
            [self.cursor.x as f32, self.cursor.y as f32],
        );
    }

    fn zoom_about_screen_point(&mut self, new_zoom: f32, anchor: [f32; 2]) {
        if !self.canvas_navigation_enabled() {
            return;
        }
        let old_zoom = self.zoom;
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (new_zoom - old_zoom).abs() < f32::EPSILON {
            return;
        }
        let viewport_center = [
            self.config.width as f32 * 0.5,
            self.config.height as f32 * 0.5,
        ];
        self.pan = pan_after_zoom_at_anchor(self.pan, old_zoom, new_zoom, viewport_center, anchor);
        self.zoom = new_zoom;
        self.rebuild_scene_geometry_for_zoom();
        self.update_view_uniform();
    }

    /// Install a freshly parsed document into the viewer state and reset all
    /// per-class editing/selection state for the new library.
    fn begin_document_load(&mut self, path: PathBuf) {
        if self.loading_document.is_some() || self.has_unsaved_changes() {
            return;
        }
        eprintln!(
            "modelica-wgpu: loading document in background: {}",
            path.display()
        );
        self.load_error = None;
        self.status_message = Some("正在加载新文档…".to_owned());
        self.loading_document = Some(std::thread::spawn(move || LoadedDocument::load(&path)));
        self.request_redraw();
    }

    fn poll_document_load(&mut self) {
        let Some(handle) = self.loading_document.as_ref() else {
            return;
        };
        if !handle.is_finished() {
            self.request_redraw();
            return;
        }
        let handle = self
            .loading_document
            .take()
            .expect("document load handle still present");
        match handle.join() {
            Ok(Ok(document)) if self.has_unsaved_changes() => {
                self.status_message =
                    Some("新文档加载期间当前文档发生了修改，已保留当前文档".to_owned());
                self.load_error =
                    Some("新文档未打开：当前文档在加载期间产生了未保存修改".to_owned());
                drop(document);
            }
            Ok(Ok(document)) => self.adopt_loaded_document(document),
            Ok(Err(error)) => {
                self.status_message = Some("文档加载失败：当前文档已保留".to_owned());
                self.load_error = Some(error);
            }
            Err(_) => {
                self.status_message = Some("文档加载失败：当前文档已保留".to_owned());
                self.load_error = Some("Modelica document loading thread panicked".into());
            }
        }
        self.request_redraw();
    }

    /// Install a freshly parsed document into the viewer state and reset all
    /// per-class editing/selection state for the new library.
    fn adopt_loaded_document(&mut self, document: LoadedDocument) {
        self.scene = build_scene(
            &self.device,
            &self.style_layout,
            Some(&document),
            None,
            self.zoom,
        );
        self.diagram_scene = build_diagram_scene(
            &self.device,
            &self.style_layout,
            Some(&document),
            None,
            self.zoom,
        );
        self.document = Some(document);
        reset_edit_history(&mut self.history, &mut self.redo_history);
        self.selected_class = None;
        self.refresh_ui_document();
        self.expanded_nodes.clear();
        if let Some(doc) = self.document.as_ref() {
            expand_top_level(&mut self.expanded_nodes, &doc.model_tree);
        }
        self.canvas_rect = None;
        self.pointer_interaction = PointerInteraction::None;
        self.component_connection_previews = None;
        self.connection_creation_preview = None;
        self.pending_waypoint = false;
        self.pending_waypoint_queued_at = None;
        self.waypoint_frame_pending = false;
        self.waypoint_profile_frames_remaining = 0;
        self.suppress_next_pointer_release_redraw = false;
        self.diagram_selection = DiagramSelection::None;
        self.hovered_port = None;
        self.diagram_hit_cache = DiagramHitCache::default();
        self.load_error = None;
        self.status_message = Some("文档加载完成".to_owned());
        self.update_title(None);
    }

    fn render(
        &mut self,
        cancel_redraw: Option<CancelRedrawTiming>,
        deselect_redraw: Option<DeselectRedrawTiming>,
    ) -> Result<(), wgpu::SurfaceError> {
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
        let status_message = self.status_message.clone();
        let mut open_requested = false;
        let mut open_directory_requested = false;
        let mut class_clicked = None;
        let mut fit_requested = false;
        let mut zoom_action = None;
        let mut view_changed = false;
        let mut icon_clip_rect = None;
        let mut expand_all_requested = false;
        let mut collapse_all_requested = false;
        let mut leave_decision = None;
        let mut source_highlight_cache = std::mem::take(&mut self.source_highlight_cache);
        let mut source_scroll_state =
            std::mem::replace(&mut self.source_scroll_state, SourceScrollState::new());
        let mut source_interaction = std::mem::take(&mut self.source_interaction);
        let mut source_scroll_rect = self.source_scroll_rect;
        let source_wheel_sample = self.source_wheel_sample;
        let mut source_perf_frame = None;
        let mut tree_ui = Duration::ZERO;
        let overlay_update_started = Instant::now();
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
        let diagram_component_preview = self.active_diagram_component_preview();
        trace_component_preview(diagram_component_preview);
        let icon_text_items = collect_model_text_overlay_items(
            self.document.as_ref(),
            selected_class.as_deref(),
            MainView::Icon,
            None,
        );
        let diagram_text_items = collect_model_text_overlay_items(
            self.document.as_ref(),
            selected_class.as_deref(),
            MainView::Diagram,
            diagram_component_preview,
        );
        let overlay_update = overlay_update_started.elapsed();
        let pre_ui_prepare = overlay_update_started.duration_since(frame_started);
        let zoom = self.zoom;
        let pan = self.pan;
        let viewport = [self.config.width, self.config.height];
        let window_scale_factor = self.window.scale_factor() as f32;
        let pixels_per_point = window_scale_factor;
        let trace_text_layout = std::env::var_os("MODELICA_WGPU_TRACE_TEXT_LAYOUT").is_some();
        let ui_build_started = Instant::now();
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
                zoom,
                &mut zoom_action,
                &mut fit_requested,
                &mut view_changed,
                &mut icon_clip_rect,
                &mut expand_all_requested,
                &mut collapse_all_requested,
                load_error.as_deref(),
                status_message.as_deref(),
                document_loading,
                &mut source_highlight_cache,
                &mut source_scroll_state,
                &mut source_interaction,
                &mut source_scroll_rect,
                source_wheel_sample,
                &mut source_perf_frame,
                &mut tree_ui,
                self.pending_document_action.as_ref(),
                &mut leave_decision,
                window_scale_factor,
                trace_text_layout,
            );
            if main_view == MainView::Icon {
                draw_model_text_overlay(
                    ctx,
                    icon_clip_rect,
                    &icon_text_items,
                    zoom,
                    pan,
                    viewport,
                    pixels_per_point,
                    false,
                );
            } else if main_view == MainView::Diagram {
                draw_model_text_overlay(
                    ctx,
                    icon_clip_rect,
                    &diagram_text_items,
                    zoom,
                    pan,
                    viewport,
                    pixels_per_point,
                    true,
                );
            }
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
        let egui_run = ui_build_started.elapsed();
        if let Some(profile) = source_perf_frame.as_mut() {
            profile.pre_ui_prepare = pre_ui_prepare;
            profile.overlay_update = overlay_update;
            profile.tree_ui = tree_ui;
        }
        self.source_highlight_cache = source_highlight_cache;
        self.source_scroll_state = source_scroll_state;
        self.source_interaction = source_interaction;
        self.source_scroll_rect = source_scroll_rect;
        self.source_perf_frame = source_perf_frame;
        let ui_build = ui_build_started.elapsed();
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
            self.request_redraw();
        }
        let previous_main_view = self.main_view;
        self.main_view = main_view;
        self.canvas_rect = icon_clip_rect;
        if previous_main_view != self.main_view {
            self.pointer_interaction = PointerInteraction::None;
            self.connection_preview = None;
            self.component_connection_previews = None;
            self.connection_creation_preview = None;
            self.suppress_next_pointer_release_redraw = false;
            self.diagram_selection = DiagramSelection::None;
            self.hovered_port = None;
        }
        if expand_all_requested {
            if let Some(document) = &self.document {
                expanded_nodes.clear();
                collect_expandable_paths(&document.model_tree, &mut expanded_nodes);
            }
        } else if collapse_all_requested {
            expanded_nodes.clear();
        }
        self.expanded_nodes = expanded_nodes;
        if expand_all_requested || collapse_all_requested {
            self.request_redraw();
        }
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);

        if let Some(decision) = leave_decision {
            self.handle_leave_decision(decision);
        }

        if fit_requested {
            self.fit_scene();
            self.request_redraw();
        }

        if view_changed {
            if should_fit_scene_after_view_change(previous_main_view, self.main_view) {
                self.fit_scene();
            }
            self.request_redraw();
        }

        if let Some(action) = zoom_action {
            let anchor = canvas_center_physical_anchor(
                self.canvas_rect,
                self.window.scale_factor() as f32,
                [self.config.width, self.config.height],
            );
            self.zoom_about_screen_point(zoom_after_toolbar_action(self.zoom, action), anchor);
            self.request_redraw();
        }

        if (open_requested || open_directory_requested)
            && !document_loading
            && self.pending_document_action.is_none()
        {
            let picked = if open_directory_requested {
                FileDialog::new().pick_folder()
            } else {
                FileDialog::new()
                    .add_filter("Modelica", &["mo"])
                    .pick_file()
            };
            if let Some(path) = picked {
                self.request_document_open(path);
            }
        }

        if let Some(class_name) = class_clicked {
            let class_open_started = Instant::now();
            let (icon_cache_hit, diagram_cache_hit, before_resolution) = self
                .document
                .as_ref()
                .map(|document| {
                    (
                        document.icon_cache_hit(&class_name),
                        document.diagram_cache_hit(&class_name),
                        document.scene_resolution_stats(),
                    )
                })
                .unwrap_or((false, false, SceneResolutionStats::default()));
            let has_visual = self.document.as_ref().is_some_and(|document| {
                document.icon(&class_name).is_some() || document.diagram(&class_name).is_some()
            });
            let gpu_scene_build_started = Instant::now();
            if has_visual {
                self.scene = build_scene(
                    &self.device,
                    &self.style_layout,
                    self.document.as_ref(),
                    Some(&class_name),
                    self.zoom,
                );
            } else {
                self.scene = build_scene(
                    &self.device,
                    &self.style_layout,
                    self.document.as_ref(),
                    None,
                    self.zoom,
                );
            }
            self.diagram_scene = build_diagram_scene(
                &self.device,
                &self.style_layout,
                self.document.as_ref(),
                Some(&class_name),
                self.zoom,
            );
            let gpu_scene_build_time = gpu_scene_build_started.elapsed();
            let after_resolution = self
                .document
                .as_ref()
                .map_or_else(SceneResolutionStats::default, |document| {
                    document.scene_resolution_stats()
                });
            trace_class_open(
                &class_name,
                icon_cache_hit,
                diagram_cache_hit,
                before_resolution,
                after_resolution,
                gpu_scene_build_time,
                class_open_started.elapsed(),
            );
            self.selected_class = Some(class_name);
            self.refresh_ui_document();
            self.main_view = MainView::Source;
            self.canvas_rect = None;
            self.pointer_interaction = PointerInteraction::None;
            self.connection_preview = None;
            self.component_connection_previews = None;
            self.connection_creation_preview = None;
            self.suppress_next_pointer_release_redraw = false;
            self.diagram_selection = DiagramSelection::None;
            self.hovered_port = None;
            self.diagram_hit_cache =
                build_diagram_hit_cache(self.document.as_ref(), self.selected_class.as_deref());
            self.fit_scene();
            self.update_title(None);
            self.request_redraw();
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
        let profile_enabled = self.drag_profile.enabled
            || self.connection_creation_profile.enabled
            || self.cancel_e2e_profile.enabled
            || self.deselect_profile.enabled
            || std::env::var_os("MODELICA_WGPU_PROFILE_SOURCE_SCROLL").is_some();
        let egui_tessellation_started = profile_enabled.then(Instant::now);
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        if let Some(started) = egui_tessellation_started {
            egui_tessellation = started.elapsed();
        }
        let scene_encode_started = Instant::now();
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
        let texture_update_started = profile_enabled.then(Instant::now);
        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }
        let texture_update = texture_update_started
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        let update_buffers_started = profile_enabled.then(Instant::now);
        let mut command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        let update_buffers = update_buffers_started
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        let source_scene_encode_started = profile_enabled.then(Instant::now);
        let native_render_pass_started = profile_enabled.then(Instant::now);
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
                    let component_connection_previews = self.component_connection_previews.as_ref();
                    let scene_scan_started = profile_enabled.then(Instant::now);
                    for layer in render_layers {
                        for &geometry_index in &active_scene.layer_indices[layer.index()] {
                            let geometry = &active_scene.geometries[geometry_index];
                            if geometry.edit_key.as_deref() == preview_connection_id
                                || geometry_connection_is_in_preview_set(
                                    geometry,
                                    component_connection_previews,
                                )
                            {
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
                    if let Some(previews) = component_connection_previews {
                        for preview in &previews.previews {
                            pass.set_bind_group(1, &preview.style_bind_group, &[]);
                            pass.set_vertex_buffer(0, preview.vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                preview.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            pass.draw_indexed(
                                0..(preview.active_segment_count * 6) as u32,
                                0,
                                0..1,
                            );
                        }
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
        let native_render_pass = native_render_pass_started
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        let egui_render_pass_started = profile_enabled.then(Instant::now);
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
        let egui_render_pass = egui_render_pass_started
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        command_buffers.push(encoder.finish());
        let source_scene_encode = source_scene_encode_started
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        let scene_encode = scene_encode_started.elapsed();
        let queue_submit_started = Instant::now();
        self.queue.submit(command_buffers);
        let queue_submit = queue_submit_started.elapsed();
        let render_encode = render_encode_started.elapsed();
        let gpu_uploaded = frame_started.elapsed();
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        // Queue the next animation frame before a FIFO present can block on
        // the compositor. This keeps the following frame eligible for the
        // next refresh while leaving the idle event loop in ControlFlow::Wait.
        let keep_source_scroll_animating =
            self.main_view == MainView::Source && self.source_scroll_state.active;
        if keep_source_scroll_animating {
            self.request_redraw();
        }
        let present_started = Instant::now();
        frame.present();
        let present = present_started.elapsed();

        let total = frame_started.elapsed();
        let finished_at = Instant::now();
        let total_ms = total.as_secs_f64() * 1000.0;
        if let Some(mut cancel_redraw) = cancel_redraw {
            cancel_redraw.flush_drag_preview = Duration::ZERO;
            self.finish_cancel_profile(
                cancel_redraw,
                CancelFrameTiming {
                    ui: ui_done,
                    egui_tessellation,
                    scene_encode: source_scene_encode,
                    queue_submit,
                    present,
                    frame_total: total,
                },
                finished_at,
            );
        }
        if let Some(deselect_redraw) = deselect_redraw {
            self.finish_deselect_profile(
                deselect_redraw,
                DeselectFrameTiming {
                    overlay_update,
                    ui_build,
                    egui_tessellation,
                    scene_encode,
                    queue_submit,
                    present,
                    frame_total: total,
                },
                finished_at,
            );
        }
        if self.interactive_drag_active() {
            self.drag_profile.record_frame(
                ui_done,
                render_encode,
                total,
                scene_scan,
                FrameStageTimings {
                    egui_run,
                    egui_tessellation,
                    native_render_pass,
                    egui_render_pass,
                    texture_update,
                    update_buffers,
                    scene_encode: source_scene_encode,
                    queue_submit,
                    present,
                    frame_total: total,
                },
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

        let source_frame_seen = if let Some(profile) = self.source_perf_frame.take() {
            let timings = FrameStageTimings {
                egui_run,
                egui_tessellation,
                native_render_pass,
                egui_render_pass,
                texture_update,
                update_buffers,
                scene_encode,
                queue_submit,
                present,
                frame_total: total,
            };
            trace_source_frame(profile, timings);
            self.source_frame_stats.record(profile, timings);
            true
        } else {
            false
        };
        self.source_wheel_sample = None;
        if !keep_source_scroll_animating && source_frame_seen {
            self.source_frame_stats.finish_session();
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
    sample_count: u32,
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
            sample_count,
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
        theme_rgb(31, 35, 40) // --text-primary: #1f2328
    }
}

// Keep UI text roles opaque and separated enough to remain readable on the
// light surface. These colors apply to UI chrome only, not Diagram graphics.
fn theme_text_secondary() -> Color32 {
    if is_dark_theme() {
        theme_rgb(169, 171, 182) // --text-secondary: #a9abb6
    } else {
        theme_rgb(95, 99, 104) // --text-secondary: #5f6368
    }
}

#[allow(dead_code)]
fn theme_text_disabled() -> Color32 {
    if is_dark_theme() {
        theme_rgb(104, 108, 124)
    } else {
        theme_rgb(150, 155, 163)
    }
}

fn theme_text_tertiary() -> Color32 {
    theme_text_tertiary_for(is_dark_theme())
}

fn theme_text_tertiary_for(dark: bool) -> Color32 {
    if dark {
        theme_rgb(150, 153, 165) // --text-tertiary: #9699a5
    } else {
        theme_rgb(105, 114, 125) // --text-tertiary: #69727d
    }
}

fn theme_code_keyword() -> Color32 {
    if is_dark_theme() {
        theme_rgb(179, 164, 255)
    } else {
        theme_rgb(75, 59, 157)
    }
}

fn theme_code_number() -> Color32 {
    if is_dark_theme() {
        theme_rgb(240, 177, 96)
    } else {
        theme_rgb(145, 76, 15)
    }
}

fn theme_code_string() -> Color32 {
    if is_dark_theme() {
        theme_rgb(112, 210, 151)
    } else {
        theme_rgb(22, 115, 72)
    }
}

fn theme_code_comment() -> Color32 {
    if is_dark_theme() {
        theme_rgb(157, 160, 173)
    } else {
        theme_rgb(103, 108, 115)
    }
}

fn theme_code_punctuation() -> Color32 {
    if is_dark_theme() {
        theme_rgb(194, 196, 205)
    } else {
        theme_rgb(83, 89, 97)
    }
}

fn theme_code_type() -> Color32 {
    if is_dark_theme() {
        theme_rgb(100, 198, 229)
    } else {
        theme_rgb(17, 105, 140)
    }
}

fn theme_code_builtin() -> Color32 {
    if is_dark_theme() {
        theme_rgb(104, 214, 166)
    } else {
        theme_rgb(37, 128, 91)
    }
}

fn theme_code_operator() -> Color32 {
    if is_dark_theme() {
        theme_rgb(220, 184, 244)
    } else {
        theme_rgb(119, 79, 151)
    }
}

fn theme_code_function() -> Color32 {
    if is_dark_theme() {
        theme_rgb(232, 153, 92)
    } else {
        theme_rgb(144, 74, 26)
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
    visuals.code_bg_color = if is_dark_theme() {
        theme_surface_soft(210)
    } else {
        theme_rgb(248, 249, 252)
    };
    visuals.warn_fg_color = theme_rgb(154, 116, 31);
    visuals.error_fg_color = theme_rgb(193, 72, 90);
    visuals.selection = egui::style::Selection {
        bg_fill: theme_accent_soft(40),
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
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, theme_text_primary());
    visuals.widgets.hovered.bg_fill = theme_accent_soft(28);
    visuals.widgets.hovered.weak_bg_fill = theme_accent_soft(22);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, theme_accent_soft(110));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, theme_accent());
    visuals.widgets.active.bg_fill = theme_accent();
    visuals.widgets.active.weak_bg_fill = theme_accent_soft(130);
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
    zoom: f32,
    zoom_action: &mut Option<ZoomAction>,
    fit_requested: &mut bool,
    view_changed: &mut bool,
    icon_clip_rect: &mut Option<egui::Rect>,
    expand_all_requested: &mut bool,
    collapse_all_requested: &mut bool,
    load_error: Option<&str>,
    status_message: Option<&str>,
    document_loading: bool,
    source_highlight_cache: &mut SourceHighlightCache,
    source_scroll_state: &mut SourceScrollState,
    source_interaction: &mut SourceInteractionState,
    source_scroll_rect: &mut Option<egui::Rect>,
    source_wheel_sample: Option<SourceWheelSample>,
    source_perf_frame: &mut Option<SourcePerfFrame>,
    tree_ui_duration: &mut Duration,
    pending_document_action: Option<&PendingDocumentAction>,
    leave_decision: &mut Option<LeaveDecision>,
    window_scale_factor: f32,
    trace_text_layout: bool,
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
    visuals.code_bg_color = if is_dark_theme() {
        theme_surface_soft(210)
    } else {
        theme_rgb(248, 249, 252)
    };
    visuals.selection = egui::style::Selection {
        bg_fill: theme_accent_soft(40),
        stroke: Stroke::new(1.0_f32, theme_accent()),
    };
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, theme_text_primary());
    visuals.widgets.hovered.bg_fill = theme_accent_soft(28);
    visuals.widgets.hovered.weak_bg_fill = theme_accent_soft(22);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, theme_accent_soft(110));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, theme_accent());
    visuals.widgets.active.bg_fill = theme_accent();
    visuals.widgets.active.weak_bg_fill = theme_accent_soft(130);
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
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(if document.dirty {
                            "● 未保存"
                        } else {
                            "✓ 已保存"
                        })
                        .size(10.0)
                        .font(ui_semibold_font(10.0))
                        .color(if document.dirty {
                            theme_rgb(190, 116, 31)
                        } else {
                            theme_live()
                        }),
                    );
                }
                if let Some(status) = status_message {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(status)
                            .size(10.0)
                            .color(theme_text_secondary()),
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
                    if ui
                        .add_enabled(
                            !document_loading && pending_document_action.is_none(),
                            open_library,
                        )
                        .clicked()
                    {
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
                    if ui
                        .add_enabled(
                            !document_loading && pending_document_action.is_none(),
                            open_file,
                        )
                        .clicked()
                    {
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
                let tree_ui_started = std::env::var_os("MODELICA_WGPU_PROFILE_SOURCE_SCROLL")
                    .is_some()
                    .then(Instant::now);
                if let Some(clicked) = document_tree(
                    ui,
                    &document.tree,
                    selected_class,
                    expanded_nodes,
                    window_scale_factor,
                    trace_text_layout,
                ) {
                    *class_clicked = Some(clicked);
                }
                if let Some(started) = tree_ui_started {
                    *tree_ui_duration += started.elapsed();
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
                    RichText::new(format!("错误：{error}"))
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
                        if canvas_zoom_controls_visible_for(*main_view) {
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

                            let zoom_in_button = ui.add_sized(
                                [28.0, 28.0],
                                egui::Button::new(
                                    RichText::new("+")
                                        .size(16.0)
                                        .font(ui_font(16.0))
                                        .color(theme_text_secondary()),
                                )
                                .fill(theme_surface_soft(230))
                                .stroke(Stroke::new(1.0_f32, theme_border(34)))
                                .rounding(Rounding::same(4.0)),
                            );
                            if zoom_in_button.clicked() {
                                *zoom_action = Some(ZoomAction::In);
                            }

                            ui.label(
                                RichText::new(format!("{}%", zoom_percent(zoom)))
                                    .size(12.0)
                                    .font(ui_font(12.0))
                                    .color(theme_text_tertiary()),
                            );

                            let zoom_out_button = ui.add_sized(
                                [28.0, 28.0],
                                egui::Button::new(
                                    RichText::new("−")
                                        .size(16.0)
                                        .font(ui_font(16.0))
                                        .color(theme_text_secondary()),
                                )
                                .fill(theme_surface_soft(230))
                                .stroke(Stroke::new(1.0_f32, theme_border(34)))
                                .rounding(Rounding::same(4.0)),
                            );
                            if zoom_out_button.clicked() {
                                *zoom_action = Some(ZoomAction::Out);
                            }
                        }
                    });
                });
                ui.separator();
                let content_size = ui.available_size();
                ui.allocate_ui_with_layout(content_size, Layout::top_down(Align::Min), |ui| {
                    match *main_view {
                        MainView::Source => source_preview(
                            ui,
                            document,
                            source_highlight_cache,
                            source_scroll_state,
                            source_interaction,
                            source_scroll_rect,
                            source_wheel_sample,
                            source_perf_frame,
                            window_scale_factor,
                            trace_text_layout,
                        ),
                        MainView::Icon => icon_preview(ui, document, icon_clip_rect),
                        MainView::Diagram => diagram_preview(ui, document, icon_clip_rect),
                    }
                });
            });
        });

    if let Some(action) = pending_document_action {
        let screen_rect = ctx.screen_rect();
        egui::Area::new(egui::Id::new("leave-prompt-blocker"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.min)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen_rect.size(), Sense::click());
                ui.painter()
                    .rect_filled(rect, Rounding::ZERO, theme_rgba(12, 16, 24, 105));
            });
        egui::Window::new("未保存修改")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(
                    RichText::new(format!(
                        "当前文档有未保存修改。确定要{}吗？",
                        action.description()
                    ))
                    .size(14.0)
                    .color(theme_text_primary()),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("保存并继续").clicked() {
                        *leave_decision = Some(LeaveDecision::Save);
                    }
                    if ui.button("放弃修改").clicked() {
                        *leave_decision = Some(LeaveDecision::Discard);
                    }
                    if ui.button("取消").clicked() {
                        *leave_decision = Some(LeaveDecision::Cancel);
                    }
                });
            });
    }
}

fn glass_frame() -> Frame {
    Frame::none()
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0_f32, theme_border(23)))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::same(10.0))
}

fn tree_row_galley_position(rect: Rect, x: f32, galley_height: f32, pixels_per_point: f32) -> Pos2 {
    snap_point_to_physical_pixel(
        Pos2::new(x, rect.center().y - galley_height * 0.5),
        pixels_per_point,
    )
}

#[allow(clippy::too_many_arguments)]
fn tree_row(
    ui: &mut egui::Ui,
    marker: &str,
    icon: &str,
    label: &str,
    identity: &str,
    kind: &str,
    selected: bool,
    indent: f32,
    trace_text_layout: bool,
    trace_sample: bool,
    window_scale_factor: f32,
) -> (egui::Response, egui::Response) {
    let row_width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(row_width, 29.0), Sense::click());
    let marker_rect = Rect::from_min_size(
        Pos2::new(rect.left() + indent * 12.0, rect.top()),
        Vec2::new(24.0, rect.height()),
    );
    let marker_response = ui.interact(
        marker_rect,
        ui.id().with(("tree-toggle", identity)),
        Sense::click(),
    );
    let fill = if selected {
        theme_accent_soft(42)
    } else if response.hovered() {
        theme_surface_raised(115)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, Rounding::same(4.0), fill);
    if selected {
        ui.painter().rect_stroke(
            rect,
            Rounding::same(4.0),
            Stroke::new(1.0_f32, theme_accent_soft(125)),
        );
    }
    let pixels_per_point = ui.ctx().pixels_per_point();
    let marker_color = theme_text_tertiary();
    let marker_galley = ui
        .painter()
        .layout_no_wrap(marker.to_owned(), ui_font(12.0), marker_color);
    let marker_position = tree_row_galley_position(
        rect,
        rect.left() + indent * 12.0 + 8.0,
        marker_galley.size().y,
        pixels_per_point,
    );
    ui.painter()
        .galley(marker_position, marker_galley, marker_color);
    let text_position = snap_point_to_physical_pixel(
        Pos2::new(rect.left() + indent * 12.0 + 30.0, rect.center().y),
        pixels_per_point,
    );
    let label_font = if selected {
        ui_semibold_font(13.0)
    } else {
        ui_font(13.0)
    };
    let label_color = if selected {
        theme_accent_strong()
    } else {
        theme_text_primary()
    };
    let kind_font = ui_mono_font(9.0);
    let kind_color = if selected {
        theme_accent_strong()
    } else {
        theme_text_tertiary()
    };
    let kind_galley = ui
        .painter()
        .layout_no_wrap(kind.to_owned(), kind_font, kind_color);
    let label_max_width =
        (rect.right() - 8.0 - kind_galley.size().x - 8.0 - text_position.x).max(24.0);
    let label_galley = ui.painter().layout_no_wrap(
        ellipsize_tree_label(
            ui.painter(),
            icon,
            label,
            label_font.clone(),
            label_color,
            label_max_width,
        ),
        label_font.clone(),
        label_color,
    );
    let label_position = tree_row_galley_position(
        rect,
        text_position.x,
        label_galley.size().y,
        pixels_per_point,
    );
    if trace_sample {
        trace_text_layout_sample(
            trace_text_layout,
            "tree-primary",
            Some(label),
            None,
            label_position,
            None,
            &label_font,
            ui.ctx().pixels_per_point(),
            window_scale_factor,
        );
    }
    ui.painter()
        .galley(label_position, label_galley.clone(), label_color);
    let kind_x = (rect.right() - 8.0 - kind_galley.size().x)
        .max(label_position.x + label_galley.size().x + 6.0);
    let kind_position =
        tree_row_galley_position(rect, kind_x, kind_galley.size().y, pixels_per_point);
    ui.painter().galley(kind_position, kind_galley, kind_color);
    (response, marker_response)
}

fn tree_row_label(icon: &str, label: &str) -> String {
    format!("{icon}  {label}")
}

fn theme_code_annotation() -> Color32 {
    if is_dark_theme() {
        theme_rgb(234, 174, 111)
    } else {
        theme_rgb(157, 86, 30)
    }
}

fn ellipsize_tree_label(
    painter: &egui::Painter,
    icon: &str,
    label: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> String {
    let full = tree_row_label(icon, label);
    if painter
        .layout_no_wrap(full.clone(), font.clone(), color)
        .size()
        .x
        <= max_width
    {
        return full;
    }
    let characters = label.chars().collect::<Vec<_>>();
    let ellipsis = '…';
    let fits = |count: usize| {
        let prefix = characters[..count].iter().collect::<String>();
        painter
            .layout_no_wrap(format!("{icon}  {prefix}{ellipsis}"), font.clone(), color)
            .size()
            .x
            <= max_width
    };
    let mut low = 0;
    let mut high = characters.len();
    while low < high {
        let middle = (low + high).div_ceil(2);
        if fits(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    format!(
        "{icon}  {}{ellipsis}",
        characters[..low].iter().collect::<String>()
    )
}

fn tree_node_kind_label(kind: Option<ClassKind>) -> &'static str {
    match kind {
        Some(ClassKind::Package) => "package",
        Some(ClassKind::Model) => "model",
        Some(ClassKind::Block) => "block",
        Some(ClassKind::Connector | ClassKind::ExpandableConnector) => "connector",
        Some(ClassKind::Record | ClassKind::OperatorRecord) => "record",
        Some(ClassKind::Function | ClassKind::OperatorFunction) => "function",
        Some(ClassKind::Type) => "type",
        Some(ClassKind::Class) => "class",
        Some(ClassKind::Operator) => "operator",
        None => "",
    }
}

fn document_tree(
    ui: &mut egui::Ui,
    root: &TreeNode,
    selected_class: Option<&str>,
    expanded_nodes: &mut HashSet<String>,
    window_scale_factor: f32,
    trace_text_layout: bool,
) -> Option<String> {
    let mut clicked = None;
    let mut trace_sample_emitted = false;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_tree_node(
                ui,
                root,
                0,
                selected_class,
                expanded_nodes,
                &mut clicked,
                window_scale_factor,
                trace_text_layout,
                &mut trace_sample_emitted,
            );
        });
    clicked
}

#[allow(clippy::too_many_arguments)]
fn render_tree_node(
    ui: &mut egui::Ui,
    node: &TreeNode,
    depth: usize,
    selected_class: Option<&str>,
    expanded_nodes: &mut HashSet<String>,
    clicked: &mut Option<String>,
    window_scale_factor: f32,
    trace_text_layout: bool,
    trace_sample_emitted: &mut bool,
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
    let icon = tree_node_icon(node.kind);
    let selected = selected_class.is_some() && node.class_name.as_deref() == selected_class;
    let (response, marker_response) = tree_row(
        ui,
        marker,
        icon,
        &node.name,
        &node.qualified_name,
        tree_node_kind_label(node.kind),
        selected,
        depth as f32,
        trace_text_layout,
        trace_text_layout && depth > 0 && !*trace_sample_emitted,
        window_scale_factor,
    );
    if trace_text_layout && depth > 0 && !*trace_sample_emitted {
        *trace_sample_emitted = true;
    }
    if marker_response.clicked() {
        if has_children {
            if expanded {
                expanded_nodes.remove(&node.qualified_name);
            } else {
                expanded_nodes.insert(node.qualified_name.clone());
            }
        }
    } else if response.clicked() {
        if let Some(class_name) = &node.class_name {
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
                window_scale_factor,
                trace_text_layout,
                trace_sample_emitted,
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

fn expand_top_level(expanded: &mut HashSet<String>, root: &TreeNode) {
    if !root.children.is_empty() {
        expanded.insert(root.qualified_name.clone());
    }
    for child in &root.children {
        if !child.children.is_empty() {
            expanded.insert(child.qualified_name.clone());
        }
    }
}

const SOURCE_ROW_HEIGHT: f32 = 20.0;
const SOURCE_HEADER_HEIGHT: f32 = 24.0;
const SOURCE_FOLD_GUTTER_WIDTH: f32 = 18.0;
const SOURCE_LINE_NUMBER_WIDTH: f32 = 40.0;
const SOURCE_LINE_GAP: f32 = 12.0;
const SOURCE_SCROLL_NOTCH_ROWS: f32 = 2.75;
const SOURCE_SCROLL_SMOOTH_SECONDS: f32 = 0.09;
const SOURCE_SCROLL_EPSILON: f32 = 0.25;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceClickOwner {
    Fold,
    Viewport,
    None,
}

fn source_click_owner(
    viewport_rect: Rect,
    pointer: Option<Pos2>,
    fold_gutter_rect: Option<Rect>,
) -> SourceClickOwner {
    let Some(pointer) = pointer.filter(|position| viewport_rect.contains(*position)) else {
        return SourceClickOwner::None;
    };
    if fold_gutter_rect.is_some_and(|rect| rect.contains(pointer)) {
        SourceClickOwner::Fold
    } else {
        SourceClickOwner::Viewport
    }
}

fn source_fold_marker_position(rect: Rect, galley_size: Vec2, pixels_per_point: f32) -> Pos2 {
    snap_point_to_physical_pixel(
        Pos2::new(
            rect.center().x - galley_size.x * 0.5,
            rect.center().y - galley_size.y * 0.5,
        ),
        pixels_per_point,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceWheelKind {
    LineDelta,
    PixelDelta,
}

#[derive(Clone, Copy, Debug)]
struct SourceWheelSample {
    kind: SourceWheelKind,
    delta_y: f32,
}

#[derive(Clone, Copy, Debug)]
struct SourcePerfFrame {
    rows: usize,
    pre_ui_prepare: Duration,
    overlay_update: Duration,
    tree_ui: Duration,
    source_ui: Duration,
    highlight: Duration,
    fold_sync: Duration,
    visible_map_rebuild: Duration,
    visible_row_lookup: Duration,
    fold_layout_rebuild_count: u64,
    wheel: Option<SourceWheelSample>,
    scroll_offset: f32,
    cache_hits: usize,
    cache_misses: usize,
}

#[derive(Default)]
struct SourceFrameStats {
    samples: Vec<SourceFrameSample>,
    last_frame_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
struct SourceFrameSample {
    frame_total: Duration,
    frame_interval: Option<Duration>,
    pre_ui_prepare: Duration,
    overlay_update: Duration,
    tree_ui: Duration,
    egui_run: Duration,
    source_ui: Duration,
    highlight: Duration,
    egui_tessellation: Duration,
    native_render_pass: Duration,
    egui_render_pass: Duration,
    update_buffers: Duration,
    texture_update: Duration,
    scene_encode: Duration,
    queue_submit: Duration,
    present: Duration,
}

impl SourceFrameStats {
    fn record(&mut self, profile: SourcePerfFrame, timings: FrameStageTimings) {
        let now = Instant::now();
        let frame_interval = self
            .last_frame_at
            .replace(now)
            .map(|previous| now - previous);
        self.samples.push(SourceFrameSample {
            frame_total: timings.frame_total,
            frame_interval,
            pre_ui_prepare: profile.pre_ui_prepare,
            overlay_update: profile.overlay_update,
            tree_ui: profile.tree_ui,
            egui_run: timings.egui_run,
            source_ui: profile.source_ui,
            highlight: profile.highlight,
            egui_tessellation: timings.egui_tessellation,
            native_render_pass: timings.native_render_pass,
            egui_render_pass: timings.egui_render_pass,
            update_buffers: timings.update_buffers,
            texture_update: timings.texture_update,
            scene_encode: timings.scene_encode,
            queue_submit: timings.queue_submit,
            present: timings.present,
        });
    }

    fn finish_session(&mut self) {
        if self.samples.is_empty() {
            return;
        }
        let values = |select: fn(&SourceFrameSample) -> Duration| {
            self.samples.iter().map(select).collect::<Vec<_>>()
        };
        let intervals = self
            .samples
            .iter()
            .filter_map(|sample| sample.frame_interval)
            .collect::<Vec<_>>();
        let frame_totals = values(|sample| sample.frame_total);
        let interval_p50 = duration_percentile_ms(&intervals, 0.50);
        let measured_budget_ms = if interval_p50 > 0.0 {
            interval_p50
        } else {
            duration_percentile_ms(&frame_totals, 0.50)
        };
        let refresh_budget_ms = std::env::var("MODELICA_WGPU_REFRESH_HZ")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|hz| *hz > 0.0)
            .map_or(measured_budget_ms, |hz| 1_000.0 / hz);
        let missed_budget_frames = frame_totals
            .iter()
            .filter(|duration| duration.as_secs_f64() * 1_000.0 > refresh_budget_ms)
            .count();
        let missed_refresh_frames = intervals
            .iter()
            .filter(|duration| duration.as_secs_f64() * 1_000.0 > refresh_budget_ms * 1.5)
            .count();
        let interval_values = intervals
            .iter()
            .map(|duration| duration.as_secs_f64() * 1_000.0)
            .collect::<Vec<_>>();
        let interval_mean = if interval_values.is_empty() {
            0.0
        } else {
            interval_values.iter().sum::<f64>() / interval_values.len() as f64
        };
        let interval_stddev = if interval_values.len() < 2 {
            0.0
        } else {
            let variance = interval_values
                .iter()
                .map(|value| (value - interval_mean).powi(2))
                .sum::<f64>()
                / interval_values.len() as f64;
            variance.sqrt()
        };
        let p95 = |select: fn(&SourceFrameSample) -> Duration| {
            duration_percentile_ms(&values(select), 0.95)
        };
        let p99 = |select: fn(&SourceFrameSample) -> Duration| {
            duration_percentile_ms(&values(select), 0.99)
        };
        eprintln!(
            "[SOURCE SESSION] samples={} refresh_budget_ms={refresh_budget_ms:.2} missed_budget_frames={missed_budget_frames} missed_refresh_frames={missed_refresh_frames} frame_ms_p50={:.2} frame_ms_p90={:.2} frame_ms_p95={:.2} frame_ms_p99={:.2} frame_ms_worst={:.2} interval_ms_mean={interval_mean:.2} interval_ms_stddev={interval_stddev:.2} interval_ms_p50={interval_p50:.2} interval_ms_p90={:.2} interval_ms_p95={:.2} interval_ms_p99={:.2} interval_ms_worst={:.2} pre_ui_p95_us={:.1} overlay_collect_p95_us={:.1} tree_ui_p95_us={:.1} source_ui_p95_us={:.1} egui_run_p95_us={:.1} highlight_p95_us={:.1} tessellation_p95_us={:.1} native_pass_p95_us={:.1} egui_pass_p95_us={:.1} update_buffers_p95_us={:.1} texture_update_p95_us={:.1} encode_p95_us={:.1} submit_p95_us={:.1} present_p95_us={:.1}",
            self.samples.len(),
            duration_percentile_ms(&frame_totals, 0.50),
            duration_percentile_ms(&frame_totals, 0.90),
            duration_percentile_ms(&frame_totals, 0.95),
            duration_percentile_ms(&frame_totals, 0.99),
            duration_percentile_ms(&frame_totals, 1.0),
            duration_percentile_ms(&intervals, 0.90),
            duration_percentile_ms(&intervals, 0.95),
            duration_percentile_ms(&intervals, 0.99),
            duration_percentile_ms(&intervals, 1.0),
            p95(|sample| sample.pre_ui_prepare) * 1_000.0,
            p95(|sample| sample.overlay_update) * 1_000.0,
            p95(|sample| sample.tree_ui) * 1_000.0,
            p95(|sample| sample.source_ui) * 1_000.0,
            p95(|sample| sample.egui_run) * 1_000.0,
            p95(|sample| sample.highlight) * 1_000.0,
            p95(|sample| sample.egui_tessellation) * 1_000.0,
            p95(|sample| sample.native_render_pass) * 1_000.0,
            p95(|sample| sample.egui_render_pass) * 1_000.0,
            p95(|sample| sample.update_buffers) * 1_000.0,
            p95(|sample| sample.texture_update) * 1_000.0,
            p95(|sample| sample.scene_encode) * 1_000.0,
            p95(|sample| sample.queue_submit) * 1_000.0,
            p95(|sample| sample.present) * 1_000.0,
        );
        eprintln!(
            "[SOURCE SESSION DETAIL] p99_us egui_run={:.1} source_ui={:.1} highlight={:.1} tessellation={:.1} update_buffers={:.1} texture_update={:.1} encode={:.1} submit={:.1} present={:.1}",
            p99(|sample| sample.egui_run) * 1_000.0,
            p99(|sample| sample.source_ui) * 1_000.0,
            p99(|sample| sample.highlight) * 1_000.0,
            p99(|sample| sample.egui_tessellation) * 1_000.0,
            p99(|sample| sample.update_buffers) * 1_000.0,
            p99(|sample| sample.texture_update) * 1_000.0,
            p99(|sample| sample.scene_encode) * 1_000.0,
            p99(|sample| sample.queue_submit) * 1_000.0,
            p99(|sample| sample.present) * 1_000.0,
        );
        self.samples.clear();
        self.last_frame_at = None;
    }
}

fn duration_percentile_ms(values: &[Duration], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index].as_secs_f64() * 1_000.0
}

#[derive(Debug)]
struct SourceScrollState {
    selected_class: Option<String>,
    current_y: f32,
    target_y: f32,
    max_y: f32,
    last_update: Instant,
    initialized: bool,
    active: bool,
    override_default_wheel: bool,
}

impl SourceScrollState {
    fn new() -> Self {
        Self {
            selected_class: None,
            current_y: 0.0,
            target_y: 0.0,
            max_y: 0.0,
            last_update: Instant::now(),
            initialized: false,
            active: false,
            override_default_wheel: false,
        }
    }

    fn sync_class(&mut self, selected_class: Option<&str>) {
        let selected_class = selected_class.map(str::to_owned);
        if self.selected_class == selected_class {
            return;
        }
        self.selected_class = selected_class;
        self.current_y = 0.0;
        self.target_y = 0.0;
        self.max_y = 0.0;
        self.initialized = false;
        self.active = false;
        self.override_default_wheel = false;
    }

    fn enqueue_line_delta(&mut self, delta_y: f32) {
        if !self.initialized {
            self.current_y = self.target_y;
            self.initialized = true;
        }
        self.target_y = (self.target_y - delta_y * SOURCE_ROW_HEIGHT * SOURCE_SCROLL_NOTCH_ROWS)
            .clamp(0.0, self.max_y);
        self.active = true;
        self.override_default_wheel = true;
        self.last_update = Instant::now();
    }

    fn advance(&mut self, pixels_per_point: f32) -> bool {
        if !self.initialized {
            return false;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_update)
            .as_secs_f32()
            .clamp(0.0, 0.1);
        self.last_update = now;
        self.advance_by(dt, pixels_per_point)
    }

    fn advance_by(&mut self, dt: f32, pixels_per_point: f32) -> bool {
        if !self.active {
            return false;
        }
        let dt = dt.clamp(0.0, 0.1);
        let alpha = 1.0 - (-dt / SOURCE_SCROLL_SMOOTH_SECONDS).exp();
        self.current_y += (self.target_y - self.current_y) * alpha;
        if (self.target_y - self.current_y).abs() <= SOURCE_SCROLL_EPSILON {
            self.current_y = self.target_y;
            self.active = false;
            self.snap_to_stable_pixel(pixels_per_point);
            return true;
        }
        false
    }

    fn snap_to_stable_pixel(&mut self, pixels_per_point: f32) {
        self.current_y =
            snap_scroll_offset_to_physical_pixel(self.current_y, self.max_y, pixels_per_point);
        self.target_y = self.current_y;
    }

    fn set_bounds(&mut self, max_y: f32) {
        self.max_y = max_y.max(0.0);
        self.current_y = self.current_y.clamp(0.0, self.max_y);
        self.target_y = self.target_y.clamp(0.0, self.max_y);
    }
}

#[derive(Default)]
struct SourceInteractionState {
    selected_class: Option<String>,
    focused: bool,
    all_selected: bool,
    fold_state: SourceFoldState,
}

impl SourceInteractionState {
    fn sync_class(&mut self, selected_class: Option<&str>) {
        let selected_class = selected_class.map(str::to_owned);
        if self.selected_class == selected_class {
            return;
        }
        self.selected_class = selected_class;
        self.focused = false;
        self.all_selected = false;
    }

    fn sync_document(&mut self, document: &UiDocument) {
        self.sync_class(document.selected_class.as_deref());
        self.fold_state.sync_document(document);
    }

    fn focus_for_click_owner(&mut self, owner: SourceClickOwner) {
        if owner == SourceClickOwner::Viewport {
            self.focused = true;
            self.all_selected = false;
        }
    }
}

fn source_copy_text(lines: &[String]) -> String {
    lines.join("\n")
}

#[allow(clippy::too_many_arguments)]
fn source_preview(
    ui: &mut egui::Ui,
    document: Option<&UiDocument>,
    source_highlight_cache: &mut SourceHighlightCache,
    source_scroll_state: &mut SourceScrollState,
    source_interaction: &mut SourceInteractionState,
    source_scroll_rect: &mut Option<egui::Rect>,
    source_wheel_sample: Option<SourceWheelSample>,
    source_perf_frame: &mut Option<SourcePerfFrame>,
    window_scale_factor: f32,
    trace_text_layout: bool,
) {
    *source_scroll_rect = None;
    let frame = Frame::none()
        .fill(theme_surface())
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::same(16.0));
    frame.show(ui, |ui| {
        if let Some(document) = document {
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), SOURCE_HEADER_HEIGHT),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(
                        RichText::new(if document.source_name.is_empty() {
                            "No class selected"
                        } else {
                            document.source_name.as_str()
                        })
                        .size(13.0)
                        .font(ui_semibold_font(13.0))
                        .color(theme_text_primary()),
                    );
                    ui.label(
                        RichText::new(if document.source_name.is_empty() {
                            "click a model in the library"
                        } else {
                            "read-only source"
                        })
                        .size(11.0)
                        .color(if document.source_name.is_empty() {
                            theme_text_secondary()
                        } else {
                            theme_live()
                        }),
                    );
                },
            );
        }
        ui.add_space(8.0);
        if let Some(document) = document {
            let source_ui_started = Instant::now();
            let mut highlight_time = Duration::ZERO;
            let mut visible_row_lookup = Duration::ZERO;
            let mut rendered_rows = 0;
            let fold_sync_before = source_interaction.fold_state.fold_sync_time;
            let fold_layout_time_before = source_interaction.fold_state.fold_layout_rebuild_time;
            let fold_layout_count_before = source_interaction.fold_state.fold_layout_rebuild_count;
            source_scroll_state.sync_class(document.selected_class.as_deref());
            source_interaction.sync_document(document);
            if source_interaction.focused
                && ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::A))
            {
                source_interaction.all_selected = true;
            }
            if source_interaction.focused
                && source_interaction.all_selected
                && ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::C))
            {
                ui.output_mut(|output| {
                    output.copied_text = source_copy_text(&document.source_lines);
                });
            }
            let scroll_height = ui.available_height().max(0.0);
            let scroll_width = ui.available_width().max(0.0);
            let scroll_delta = ui.input(|input| input.raw_scroll_delta);
            let pointer_position = ui.ctx().pointer_latest_pos();
            let pixels_per_point = ui.ctx().pixels_per_point();
            let scroll_id_source = ("modelica-source-scroll", document.selected_class.as_deref());
            // Prepare/validate the source cache once; all per-visible-row
            // lookups below reuse this class/version/theme key.
            let source_content_width = source_highlight_cache.content_width(ui, document);
            let (expanded_marker_galley, collapsed_marker_galley) =
                source_highlight_cache.fold_marker_galleys(ui);
            let cache_hits_before = source_highlight_cache.cache_hits;
            let cache_misses_before = source_highlight_cache.cache_misses;
            let scroll_id = ui.make_persistent_id(egui::Id::new(scroll_id_source));
            let mut state_before =
                egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
            if !source_scroll_state.initialized {
                source_scroll_state.current_y = state_before.offset.y;
                source_scroll_state.target_y = state_before.offset.y;
                source_scroll_state.initialized = true;
            } else if !source_scroll_state.active && !source_scroll_state.override_default_wheel {
                source_scroll_state.current_y = state_before.offset.y;
                source_scroll_state.target_y = state_before.offset.y;
            }
            let settled_this_frame = source_scroll_state.advance(pixels_per_point);
            if source_scroll_state.active
                || source_scroll_state.override_default_wheel
                || settled_this_frame
            {
                state_before.offset.y = source_scroll_state.current_y;
                state_before.store(ui.ctx(), scroll_id);
            }
            let offset_before = state_before.offset;
            let mut fold_toggle_line = None;
            let mut fold_gutter_rect_under_pointer = None;
            let primary_clicked =
                ui.input(|input| input.pointer.button_clicked(egui::PointerButton::Primary));
            let source_all_selected = source_interaction.all_selected;
            let scroll_output = {
                let fold_state = &source_interaction.fold_state;
                let visible_source_rows = &fold_state.visible_rows.rows;
                let fold_ranges = &fold_state.ranges;
                let collapsed_folds = &fold_state.collapsed;
                let range_by_start_line = &fold_state.range_by_start_line;
                egui::ScrollArea::both()
                    .id_source(scroll_id_source)
                    .max_height(scroll_height)
                    .max_width(scroll_width)
                    .auto_shrink([false, false])
                    .drag_to_scroll(false)
                    .show_rows(
                        ui,
                        SOURCE_ROW_HEIGHT,
                        visible_source_rows.len(),
                        |ui, row_range| {
                            let trace_sample_row = row_range.start;
                            rendered_rows += row_range.len();
                            for visible_index in row_range {
                                let visible_row = &visible_source_rows[visible_index];
                                let original_line = visible_row.original_line;
                                let lookup_started = Instant::now();
                                let range_index = range_by_start_line.get(&original_line).copied();
                                let marker = range_index.map(|index| {
                                    if collapsed_folds.contains(&fold_ranges[index].id) {
                                        collapsed_marker_galley.clone()
                                    } else {
                                        expanded_marker_galley.clone()
                                    }
                                });
                                let range = visible_row
                                    .fold_range_index
                                    .and_then(|index| fold_ranges.get(index));
                                let lookup_elapsed = lookup_started.elapsed();
                                let (number_galley, code_galley, elapsed) = if let Some(range) =
                                    range
                                {
                                    let (code_galley, elapsed) = source_highlight_cache
                                        .collapsed_galley(ui, document, range);
                                    let (number_galley, _, number_elapsed) =
                                        source_highlight_cache.galleys(ui, document, original_line);
                                    (number_galley, code_galley, elapsed + number_elapsed)
                                } else {
                                    source_highlight_cache.galleys(ui, document, original_line)
                                };
                                highlight_time += elapsed;
                                let fold_response = render_source_line(
                                    ui,
                                    number_galley,
                                    code_galley,
                                    source_content_width,
                                    original_line,
                                    marker,
                                    source_all_selected,
                                    source_scroll_state.current_y,
                                    window_scale_factor,
                                    trace_text_layout && visible_index == trace_sample_row,
                                );
                                if let Some((response, fold_rect)) = fold_response {
                                    if response.clicked() {
                                        fold_toggle_line = Some(original_line);
                                    }
                                    if pointer_position
                                        .is_some_and(|pointer| fold_rect.contains(pointer))
                                    {
                                        fold_gutter_rect_under_pointer = Some(fold_rect);
                                    }
                                }
                                visible_row_lookup += lookup_elapsed;
                            }
                        },
                    )
            };
            let click_owner = source_click_owner(
                scroll_output.inner_rect,
                pointer_position,
                fold_gutter_rect_under_pointer,
            );
            if let Some(line) = fold_toggle_line {
                source_interaction
                    .fold_state
                    .toggle_line(line, document.source_lines.len());
                ui.ctx().request_repaint();
            } else if primary_clicked {
                source_interaction.focus_for_click_owner(click_owner);
            }
            *source_scroll_rect = Some(scroll_output.inner_rect);
            let max_scroll_y =
                (scroll_output.content_size.y - scroll_output.inner_rect.height()).max(0.0);
            source_scroll_state.set_bounds(max_scroll_y);
            if settled_this_frame {
                source_scroll_state.snap_to_stable_pixel(pixels_per_point);
            }
            let mut final_state = scroll_output.state;
            if source_scroll_state.active
                || source_scroll_state.override_default_wheel
                || settled_this_frame
            {
                final_state.offset.y = source_scroll_state.current_y;
                final_state.store(ui.ctx(), scroll_id);
            } else {
                source_scroll_state.current_y = final_state.offset.y;
                source_scroll_state.target_y = final_state.offset.y;
                source_scroll_state.initialized = true;
            }
            source_scroll_state.override_default_wheel = false;
            trace_source_scroll(
                document,
                scroll_delta,
                pointer_position,
                offset_before,
                &scroll_output,
                scroll_height,
            );
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
            if std::env::var_os("MODELICA_WGPU_PROFILE_SOURCE_SCROLL").is_some()
                && (source_wheel_sample.is_some()
                    || source_scroll_state.active
                    || scroll_delta != Vec2::ZERO)
            {
                *source_perf_frame = Some(SourcePerfFrame {
                    rows: rendered_rows,
                    pre_ui_prepare: Duration::ZERO,
                    overlay_update: Duration::ZERO,
                    tree_ui: Duration::ZERO,
                    source_ui: source_ui_started.elapsed(),
                    highlight: highlight_time,
                    fold_sync: source_interaction
                        .fold_state
                        .fold_sync_time
                        .saturating_sub(fold_sync_before),
                    visible_map_rebuild: source_interaction
                        .fold_state
                        .fold_layout_rebuild_time
                        .saturating_sub(fold_layout_time_before),
                    visible_row_lookup,
                    fold_layout_rebuild_count: source_interaction
                        .fold_state
                        .fold_layout_rebuild_count
                        .saturating_sub(fold_layout_count_before),
                    wheel: source_wheel_sample,
                    scroll_offset: source_scroll_state.current_y,
                    cache_hits: source_highlight_cache
                        .cache_hits
                        .saturating_sub(cache_hits_before),
                    cache_misses: source_highlight_cache
                        .cache_misses
                        .saturating_sub(cache_misses_before),
                });
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn render_source_line(
    ui: &mut egui::Ui,
    number_galley: Arc<egui::Galley>,
    code_galley: Arc<egui::Galley>,
    content_width: f32,
    original_line: usize,
    fold_marker: Option<Arc<egui::Galley>>,
    selected: bool,
    scroll_offset: f32,
    window_scale_factor: f32,
    trace_sample: bool,
) -> Option<(egui::Response, Rect)> {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(content_width.max(ui.available_width()), SOURCE_ROW_HEIGHT),
        Sense::hover(),
    );
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, Rounding::ZERO, theme_accent_soft(30));
    }
    let fold_rect =
        Rect::from_min_size(rect.min, Vec2::new(SOURCE_FOLD_GUTTER_WIDTH, rect.height()));
    let fold_response = fold_marker.map(|marker_galley| {
        let response = ui.interact(
            fold_rect,
            ui.id().with(("source-fold", original_line)),
            Sense::click(),
        );
        painter.galley(
            source_fold_marker_position(
                fold_rect,
                marker_galley.size(),
                ui.ctx().pixels_per_point(),
            ),
            marker_galley,
            theme_text_secondary(),
        );
        (response, fold_rect)
    });
    let pixels_per_point = ui.ctx().pixels_per_point();
    let number_position = source_line_galley_position(
        rect,
        rect.left() + SOURCE_FOLD_GUTTER_WIDTH,
        number_galley.size().y,
        pixels_per_point,
    );
    let code_position = source_line_galley_position(
        rect,
        rect.left() + SOURCE_FOLD_GUTTER_WIDTH + SOURCE_LINE_NUMBER_WIDTH + SOURCE_LINE_GAP,
        code_galley.size().y,
        pixels_per_point,
    );
    if trace_sample {
        trace_text_layout_sample(
            true,
            "source-line-number",
            None,
            Some(original_line + 1),
            number_position,
            Some(scroll_offset),
            &ui_mono_font(13.0),
            pixels_per_point,
            window_scale_factor,
        );
        trace_text_layout_sample(
            true,
            "source-body",
            None,
            Some(original_line + 1),
            code_position,
            Some(scroll_offset),
            &ui_mono_font(12.0),
            pixels_per_point,
            window_scale_factor,
        );
    }
    painter.galley(number_position, number_galley, theme_text_tertiary());
    painter.galley(code_position, code_galley, theme_text_primary());
    fold_response
}

fn source_line_galley_position(
    rect: Rect,
    x: f32,
    galley_height: f32,
    pixels_per_point: f32,
) -> Pos2 {
    let y = rect.top() + (SOURCE_ROW_HEIGHT - galley_height) * 0.5;
    snap_y_to_physical_pixel(Pos2::new(x, y), pixels_per_point)
}

#[allow(clippy::too_many_arguments)]
fn trace_text_layout_sample(
    enabled: bool,
    surface: &str,
    sample_text: Option<&str>,
    line_number: Option<usize>,
    logical_position: Pos2,
    scroll_offset: Option<f32>,
    font_id: &FontId,
    pixels_per_point: f32,
    window_scale_factor: f32,
) {
    if !enabled {
        return;
    }
    let physical_position = logical_to_physical_pixels(logical_position, pixels_per_point);
    eprintln!(
        "[TEXT LAYOUT] surface={surface} sample_text={sample_text:?} line={line_number:?} pixels_per_point={pixels_per_point:.3} window_scale_factor={window_scale_factor:.3} logical=({:.3},{:.3}) physical=({:.3},{:.3}) scroll_offset={scroll_offset:?} font_family={:?} font_size={:.2}",
        logical_position.x,
        logical_position.y,
        physical_position.x,
        physical_position.y,
        font_id.family,
        font_id.size,
    );
}

fn trace_source_scroll(
    document: &UiDocument,
    scroll_delta: Vec2,
    pointer_position: Option<Pos2>,
    offset_before: Vec2,
    output: &egui::scroll_area::ScrollAreaOutput<()>,
    viewport_height: f32,
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_SOURCE_SCROLL").is_none() || scroll_delta == Vec2::ZERO
    {
        return;
    }
    let hovered = pointer_position.is_some_and(|position| output.inner_rect.contains(position));
    eprintln!(
        "[SOURCE SCROLL] class={} hovered={hovered} wheel_delta={scroll_delta:?} offset_before={offset_before:?} offset_after={:?} viewport_height={viewport_height:.1} content_height={:.1}",
        document.selected_class.as_deref().unwrap_or("<none>"),
        output.state.offset,
        output.content_size.y,
    );
}

#[derive(Clone, Copy, Debug)]
struct FrameStageTimings {
    egui_run: Duration,
    egui_tessellation: Duration,
    native_render_pass: Duration,
    egui_render_pass: Duration,
    texture_update: Duration,
    update_buffers: Duration,
    scene_encode: Duration,
    queue_submit: Duration,
    present: Duration,
    frame_total: Duration,
}

fn trace_source_frame(profile: SourcePerfFrame, timings: FrameStageTimings) {
    if std::env::var_os("MODELICA_WGPU_PROFILE_SOURCE_SCROLL").is_none() {
        return;
    }
    let (wheel_kind, wheel_delta) = match profile.wheel {
        Some(SourceWheelSample {
            kind: SourceWheelKind::LineDelta,
            delta_y,
        }) => ("LineDelta", delta_y),
        Some(SourceWheelSample {
            kind: SourceWheelKind::PixelDelta,
            delta_y,
        }) => ("PixelDelta", delta_y),
        None => ("None", 0.0),
    };
    let source_ui_us = profile.source_ui.as_secs_f64() * 1_000_000.0;
    let highlight_us = profile.highlight.as_secs_f64() * 1_000_000.0;
    let fold_sync_us = profile.fold_sync.as_secs_f64() * 1_000_000.0;
    let visible_map_rebuild_us = profile.visible_map_rebuild.as_secs_f64() * 1_000_000.0;
    let visible_row_lookup_us = profile.visible_row_lookup.as_secs_f64() * 1_000_000.0;
    let cache_total = profile.cache_hits + profile.cache_misses;
    let cache_hit_rate = if cache_total == 0 {
        100.0
    } else {
        profile.cache_hits as f64 / cache_total as f64 * 100.0
    };
    eprintln!(
        "[SOURCE FRAME] rows={} wheel={} wheel_delta={wheel_delta:.3} scroll_offset={:.1} pre_ui_us={:.1} overlay_collect_us={:.1} tree_ui_us={:.1} egui_run_us={:.1} source_ui_us={source_ui_us:.1} highlight_us={highlight_us:.1} fold_sync_us={fold_sync_us:.1} visible_map_rebuild_us={visible_map_rebuild_us:.1} visible_row_lookup_us={visible_row_lookup_us:.1} fold_layout_rebuild_count={} egui_tessellation_us={:.1} native_pass_us={:.1} egui_pass_us={:.1} egui_update_buffers_us={:.1} texture_update_us={:.1} encode_us={:.1} submit_us={:.1} present_us={:.1} frame_ms={:.2} fps={:.1} galley_hits={} galley_misses={} galley_hit_rate={cache_hit_rate:.1}%",
        profile.rows,
        wheel_kind,
        profile.scroll_offset,
        profile.pre_ui_prepare.as_secs_f64() * 1_000_000.0,
        profile.overlay_update.as_secs_f64() * 1_000_000.0,
        profile.tree_ui.as_secs_f64() * 1_000_000.0,
        timings.egui_run.as_secs_f64() * 1_000_000.0,
        profile.fold_layout_rebuild_count,
        timings.egui_tessellation.as_secs_f64() * 1_000_000.0,
        timings.native_render_pass.as_secs_f64() * 1_000_000.0,
        timings.egui_render_pass.as_secs_f64() * 1_000_000.0,
        timings.update_buffers.as_secs_f64() * 1_000_000.0,
        timings.texture_update.as_secs_f64() * 1_000_000.0,
        timings.scene_encode.as_secs_f64() * 1_000_000.0,
        timings.queue_submit.as_secs_f64() * 1_000_000.0,
        timings.present.as_secs_f64() * 1_000_000.0,
        timings.frame_total.as_secs_f64() * 1000.0,
        1.0 / timings.frame_total.as_secs_f64().max(f64::EPSILON),
        profile.cache_hits,
        profile.cache_misses,
    );
}

fn trace_source_wheel(
    source_view: bool,
    delta_y: f32,
    egui_consumed: bool,
    egui_repaint: bool,
    redraw_requested: bool,
    owner: WheelOwner,
) {
    if !source_view || std::env::var_os("MODELICA_WGPU_TRACE_SOURCE_SCROLL").is_none() {
        return;
    }
    eprintln!(
        "[SOURCE WHEEL] delta_y={delta_y:.3} egui_consumed={egui_consumed} egui_repaint={egui_repaint} redraw_requested={redraw_requested} owner={owner:?}"
    );
}

#[allow(clippy::too_many_arguments)]
fn trace_canvas_wheel(
    main_view: MainView,
    control_pressed: bool,
    cursor_physical: PhysicalPosition<f64>,
    scale_factor: f32,
    canvas_rect: Option<egui::Rect>,
    egui_consumed: &str,
    owner: WheelOwner,
    sample: SourceWheelSample,
    zoom_before: f32,
    zoom_after: f32,
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_CANVAS_WHEEL").is_none() {
        return;
    }
    let cursor_logical = physical_to_logical_position(cursor_physical, scale_factor);
    let pointer_over_canvas = canvas_rect.is_some_and(|rect| rect.contains(cursor_logical));
    eprintln!(
        "[CANVAS WHEEL] view={main_view:?} ctrl={control_pressed} pointer_over_canvas={pointer_over_canvas} egui_consumed={egui_consumed} owner={owner:?} delta_kind={:?} delta={:.3} cursor_physical=({:.1},{:.1}) cursor_logical=({:.1},{:.1}) scale_factor={scale_factor:.2} canvas_rect={canvas_rect:?} zoom_before={zoom_before:.4} zoom_after={zoom_after:.4}",
        sample.kind,
        sample.delta_y,
        cursor_physical.x,
        cursor_physical.y,
        cursor_logical.x,
        cursor_logical.y,
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceTokenRole {
    Keyword,
    Annotation,
    Type,
    Builtin,
    Number,
    String,
    Comment,
    Operator,
    Punctuation,
    Function,
    Identifier,
    Whitespace,
}

const SOURCE_ANNOTATION_CALLS: &[&str] = &[
    "Icon",
    "Diagram",
    "Placement",
    "transformation",
    "iconTransformation",
    "Line",
    "Polygon",
    "Rectangle",
    "Ellipse",
    "Text",
    "Bitmap",
];

fn modelica_layout_job(
    line: &str,
    full_class_names: &HashSet<String>,
    short_class_names: &HashSet<String>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font_id = ui_mono_font(12.0);
    let tokens = tokenize(line);
    for (index, token) in tokens.iter().enumerate() {
        let role = source_token_role(&tokens, index, full_class_names, short_class_names);
        job.append(
            &token.text,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: source_token_color(role),
                ..Default::default()
            },
        );
    }
    job
}

fn source_token_role(
    tokens: &[Token],
    index: usize,
    full_class_names: &HashSet<String>,
    short_class_names: &HashSet<String>,
) -> SourceTokenRole {
    let token = &tokens[index];
    if matches!(token.kind, TokenKind::Whitespace) {
        return SourceTokenRole::Whitespace;
    }
    if matches!(token.kind, TokenKind::Comment) {
        return SourceTokenRole::Comment;
    }
    if matches!(token.kind, TokenKind::String) {
        return SourceTokenRole::String;
    }
    if matches!(token.kind, TokenKind::Number) {
        return SourceTokenRole::Number;
    }
    if token.text == "annotation" {
        return SourceTokenRole::Annotation;
    }
    if is_source_operator(&token.text) {
        return SourceTokenRole::Operator;
    }
    if matches!(token.kind, TokenKind::Punctuation) {
        return SourceTokenRole::Punctuation;
    }
    if matches!(token.kind, TokenKind::Keyword) {
        return SourceTokenRole::Keyword;
    }
    if matches!(token.kind, TokenKind::Identifier) && is_builtin_type(&token.text) {
        return SourceTokenRole::Builtin;
    }
    if matches!(token.kind, TokenKind::Identifier)
        && SOURCE_ANNOTATION_CALLS.contains(&token.text.as_str())
        && next_non_trivia(tokens, index).is_some_and(|next| next.text == "(")
    {
        return SourceTokenRole::Annotation;
    }
    if matches!(token.kind, TokenKind::Identifier)
        && (full_class_names.contains(&token.text)
            || short_class_names.contains(&token.text)
            || token.text.chars().next().is_some_and(char::is_uppercase)
            || adjacent_punctuation(tokens, index, "."))
    {
        return SourceTokenRole::Type;
    }
    if matches!(token.kind, TokenKind::Identifier)
        && next_non_trivia(tokens, index).is_some_and(|next| next.text == "(")
    {
        return SourceTokenRole::Function;
    }
    if matches!(token.kind, TokenKind::Identifier) {
        SourceTokenRole::Identifier
    } else {
        SourceTokenRole::Whitespace
    }
}

fn source_token_color(role: SourceTokenRole) -> Color32 {
    match role {
        SourceTokenRole::Keyword => theme_code_keyword(),
        SourceTokenRole::Annotation => theme_code_annotation(),
        SourceTokenRole::Type => theme_code_type(),
        SourceTokenRole::Builtin => theme_code_builtin(),
        SourceTokenRole::Number => theme_code_number(),
        SourceTokenRole::String => theme_code_string(),
        SourceTokenRole::Comment => theme_code_comment(),
        SourceTokenRole::Operator => theme_code_operator(),
        SourceTokenRole::Punctuation => theme_code_punctuation(),
        SourceTokenRole::Function => theme_code_function(),
        SourceTokenRole::Identifier | SourceTokenRole::Whitespace => theme_text_primary(),
    }
}

fn is_source_operator(value: &str) -> bool {
    matches!(value, "=" | "+" | "-" | "*" | "/" | "^" | ":")
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

fn collect_model_text_overlay_items(
    document: Option<&LoadedDocument>,
    selected_class: Option<&str>,
    main_view: MainView,
    diagram_component_preview: Option<(&str, ComponentPreviewPlacement)>,
) -> Vec<ModelTextOverlayItem> {
    let Some(document) = document else {
        return Vec::new();
    };
    let Some(class_name) = selected_class else {
        return Vec::new();
    };
    let display_class_name = class_name.rsplit('.').next().unwrap_or(class_name);
    let class_text_context = ModelTextContext::new(
        class_name.to_owned(),
        display_class_name.to_owned(),
        display_class_name.to_owned(),
        HashMap::new(),
        HashMap::new(),
    );
    let mut items = Vec::new();

    match main_view {
        MainView::Source => {}
        MainView::Icon => {
            let Some(scene) = document.icon(class_name) else {
                return items;
            };
            for resolved in &scene.graphics {
                if let CoreGraphic::Text(text) = &resolved.graphic {
                    items.push(model_text_overlay_item(
                        text,
                        resolved.transform,
                        &class_text_context,
                    ));
                }
            }
        }
        MainView::Diagram => {
            let Some(scene) = document.diagram(class_name) else {
                return items;
            };
            for graphic in &scene.background_graphics {
                if let CoreGraphic::Text(text) = graphic {
                    items.push(model_text_overlay_item(
                        text,
                        Transform2D::identity(),
                        &class_text_context,
                    ));
                }
            }
            for component in &scene.components {
                if !component.visible {
                    continue;
                }
                let Some(layer) = component.diagram_layer() else {
                    continue;
                };
                let preview = diagram_component_preview
                    .filter(|(component_id, _)| *component_id == component.id.as_str())
                    .map(|(_, preview)| preview);
                let placement = effective_component_transform(layer, component, preview);
                for resolved in &layer.graphics {
                    if let CoreGraphic::Text(text) = &resolved.graphic {
                        items.push(model_text_overlay_item(
                            text,
                            compose_transform(placement, resolved.transform),
                            &component.model_text_context,
                        ));
                    }
                }
            }
        }
    }
    items
}

fn model_text_overlay_item(
    text: &modelica_core::scene::TextGraphic,
    transform: Transform2D,
    context: &ModelTextContext,
) -> ModelTextOverlayItem {
    let extent = text.extent;
    let local_corners = [
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
    ];
    let corners = local_corners.map(|point| {
        let [x, y] = transform_graphic_point(point, text.origin, text.rotation, transform);
        CorePoint { x, y }
    });
    ModelTextOverlayItem {
        text: resolve_modelica_text(&text.text, context),
        corners,
        color: text.color,
        font_size: text.font_size,
        font_name: text.font_name.clone(),
        scale: transform_scale(transform),
        scale_x: transform.scale_x,
        scale_y: transform.scale_y,
        extent_width: (extent.p2.x - extent.p1.x).abs(),
        extent_height: (extent.p2.y - extent.p1.y).abs(),
        angle: text.rotation + transform.rotation,
        alignment: model_text_alignment(text.horizontal_alignment.as_deref()),
        bold: text.text_style.iter().any(|style| style.contains("Bold")),
        italic: text.text_style.iter().any(|style| style.contains("Italic")),
        underline: text
            .text_style
            .iter()
            .any(|style| style.contains("UnderLine") || style.contains("Underline")),
        minimum_screen_px: if text.text.contains("%name") {
            MIN_SCREEN_MODEL_NAME_TEXT_PX
        } else {
            MIN_SCREEN_MODEL_TEXT_PX
        },
    }
}

fn trace_component_preview(preview: Option<(&str, ComponentPreviewPlacement)>) {
    if std::env::var_os("MODELICA_WGPU_TRACE_COMPONENT_PREVIEW").is_none() {
        return;
    }
    let Some((component_id, preview)) = preview else {
        return;
    };
    eprintln!(
        "[COMPONENT PREVIEW] component={component_id} delta=({:.3},{:.3}) geometry_delta=({:.3},{:.3}) text_delta=({:.3},{:.3}) port_delta=({:.3},{:.3})",
        preview.delta.x,
        preview.delta.y,
        preview.delta.x,
        -preview.delta.y,
        preview.delta.x,
        preview.delta.y,
        preview.delta.x,
        preview.delta.y,
    );
}

fn model_text_alignment(value: Option<&str>) -> ModelTextAlignment {
    match value {
        Some(value) if value.contains("Left") => ModelTextAlignment::Left,
        Some(value) if value.contains("Right") => ModelTextAlignment::Right,
        _ => ModelTextAlignment::Center,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_model_text_overlay(
    ctx: &egui::Context,
    canvas_rect: Option<egui::Rect>,
    items: &[ModelTextOverlayItem],
    zoom: f32,
    pan: [f32; 2],
    viewport: [u32; 2],
    pixels_per_point: f32,
    diagram: bool,
) {
    let Some(canvas_rect) = canvas_rect else {
        return;
    };
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("modelica-model-text"),
        ))
        .with_clip_rect(canvas_rect);
    let to_screen = |point: CorePoint| {
        Pos2::new(
            (viewport[0] as f32 * 0.5 + pan[0] + point.x * zoom) / pixels_per_point,
            (viewport[1] as f32 * 0.5
                + pan[1]
                + if diagram {
                    -point.y * zoom
                } else {
                    point.y * zoom
                })
                / pixels_per_point,
        )
    };

    for item in items {
        if item.text.is_empty() {
            continue;
        }
        let screen_corners = item.corners.map(to_screen);
        let x_axis = screen_corners[1] - screen_corners[0];
        let y_axis = screen_corners[3] - screen_corners[0];
        let alignment_fraction = match item.alignment {
            ModelTextAlignment::Left => 0.0,
            ModelTextAlignment::Center => 0.5,
            ModelTextAlignment::Right => 1.0,
        };
        // A mirrored placement changes the box's x direction, but it must
        // not mirror the glyphs themselves. Use the matching visual side as
        // the alignment anchor and keep the font orientation readable.
        let alignment_fraction = if item.scale_x < 0.0 {
            1.0 - alignment_fraction
        } else {
            alignment_fraction
        };
        let anchor = screen_corners[0] + x_axis * alignment_fraction + y_axis * 0.5;
        let angle = if diagram {
            -item.angle.to_radians()
        } else {
            item.angle.to_radians()
        };
        let color = Color32::from_rgb(item.color[0], item.color[1], item.color[2]);
        let available_width =
            (item.extent_width * item.scale_x.abs() * zoom / pixels_per_point).max(1.0);
        let available_height =
            (item.extent_height * item.scale_y.abs() * zoom / pixels_per_point).max(1.0);
        let font_size = model_text_font_size(
            &painter,
            item,
            available_width,
            available_height,
            zoom,
            pixels_per_point,
            color,
        );
        let font = model_text_font(font_size, item.font_name.as_deref(), item.bold, item.italic);
        let galley = painter.layout_no_wrap(item.text.clone(), font, color);
        let aligned_x = match item.alignment {
            ModelTextAlignment::Left => 0.0,
            ModelTextAlignment::Center => galley.size().x * 0.5,
            ModelTextAlignment::Right => galley.size().x,
        };
        let glyph_offset = rotate_text_vector(
            Vec2::new(
                if item.scale_x < 0.0 {
                    galley.size().x - aligned_x
                } else {
                    aligned_x
                },
                galley.size().y * 0.5,
            ),
            angle,
        );
        let mut shape = TextShape::new(anchor - glyph_offset, galley, color).with_angle(angle);
        if item.underline {
            shape = shape.with_underline(Stroke::new((font_size * 0.08).max(1.0), color));
        }
        painter.add(shape);
    }
}

fn rotate_text_vector(vector: Vec2, angle: f32) -> Vec2 {
    let (sin, cos) = angle.sin_cos();
    Vec2::new(
        vector.x * cos - vector.y * sin,
        vector.x * sin + vector.y * cos,
    )
}

fn model_text_font_size(
    painter: &egui::Painter,
    item: &ModelTextOverlayItem,
    available_width: f32,
    available_height: f32,
    zoom: f32,
    pixels_per_point: f32,
    color: Color32,
) -> f32 {
    let scale = item.scale * zoom;
    let explicit = item
        .font_size
        .filter(|font_size| *font_size > 0.0)
        .map(|font_size| font_size * scale);
    let size = explicit.unwrap_or_else(|| {
        let probe_font = model_text_font(1.0, item.font_name.as_deref(), item.bold, item.italic);
        let probe = painter.layout_no_wrap(item.text.clone(), probe_font, color);
        let width_fit = if probe.size().x > 0.0 {
            available_width / probe.size().x
        } else {
            f32::INFINITY
        };
        let height_fit = if probe.size().y > 0.0 {
            available_height / probe.size().y
        } else {
            f32::INFINITY
        };
        width_fit.min(height_fit) * 0.88
    });
    size.max(item.minimum_screen_px / pixels_per_point)
        .min(28.0)
}

fn model_text_font(size: f32, name: Option<&str>, bold: bool, italic: bool) -> FontId {
    let is_monospace = name.is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        name.contains("courier") || name.contains("mono")
    });
    let family = if is_monospace {
        FontFamily::Name(UI_FONT_MONO.into())
    } else if bold && italic {
        FontFamily::Name(UI_FONT_SEMIBOLD_ITALIC.into())
    } else if bold {
        FontFamily::Name(UI_FONT_SEMIBOLD.into())
    } else if italic {
        FontFamily::Name(UI_FONT_ITALIC.into())
    } else {
        FontFamily::Name(UI_FONT_MEDIUM.into())
    };
    FontId::new(size, family)
}

fn build_scene(
    device: &wgpu::Device,
    style_layout: &wgpu::BindGroupLayout,
    document: Option<&LoadedDocument>,
    selected_class: Option<&str>,
    stroke_zoom: f32,
) -> GpuIconScene {
    let geometries = document
        .and_then(|document| selected_class.and_then(|name| document.icon(name)))
        .map(|scene| core_icon_geometry_at_zoom(scene, stroke_zoom))
        .unwrap_or_default();
    gpu_scene_from_geometries(device, style_layout, geometries, "icon", stroke_zoom)
}

fn build_diagram_scene(
    device: &wgpu::Device,
    style_layout: &wgpu::BindGroupLayout,
    document: Option<&LoadedDocument>,
    selected_class: Option<&str>,
    stroke_zoom: f32,
) -> GpuIconScene {
    let diagram =
        document.and_then(|document| selected_class.and_then(|name| document.diagram(name)));
    let geometries = diagram
        .map(|scene| core_diagram_geometry_at_zoom(scene, stroke_zoom))
        .unwrap_or_default();
    if std::env::var_os("MODELICA_WGPU_DEBUG_DIAGRAM").is_some() {
        if let Some(scene) = diagram {
            log_diagram_geometry_diagnostics(scene, &geometries);
        } else {
            eprintln!("diagram diagnostic: no selected DiagramScene");
        }
    }
    gpu_scene_from_geometries(device, style_layout, geometries, "diagram", stroke_zoom)
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
        let canonical_points = canonical_connection_points(scene, connection);
        let points = connection_world_points(line, &canonical_points);
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
    stroke_zoom: f32,
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
        stroke_zoom,
    }
}

fn connection_mesh_buffer_capacity(vertex_count: usize, index_count: usize) -> (usize, usize) {
    (
        vertex_count.saturating_mul(2).saturating_add(32),
        index_count.saturating_mul(2).saturating_add(48),
    )
}

fn core_icon_geometry_at_zoom(scene: &CoreIconScene, stroke_zoom: f32) -> Vec<Geometry> {
    scene
        .graphics
        .iter()
        .flat_map(|resolved| {
            let edit_key = resolved.editable.then(|| resolved.id.0.clone());
            core_graphic_geometry_at_zoom(resolved, stroke_zoom)
                .into_iter()
                .map(move |mut geometry| {
                    geometry.edit_key = edit_key.clone();
                    geometry
                })
        })
        .collect()
}

/// Return the route used by every interactive/rendering path.
///
/// Source annotations are allowed to carry endpoints that are slightly stale
/// relative to the resolved connector instances. Keep the source untouched,
/// but anchor the displayed route to the semantic connector positions so the
/// line, hit cache, overlays, and component-drag previews agree.
fn canonical_connection_points(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
) -> Vec<CorePoint> {
    let Some(line) = connection.line.as_ref() else {
        return Vec::new();
    };
    match resolved_connection_display_route(scene, connection, &line.points) {
        Ok((points, _)) => points,
        Err(_) => {
            // A connection without resolvable semantic endpoints can still be
            // a fixed/external line, so preserve its source route in that
            // case. Once endpoints resolve, never silently display stale raw
            // geometry.
            if let Ok((first, last)) = strict_connection_points(scene, connection) {
                let points = canonical_orthogonal_route(first, last, &line.points);
                if valid_interactive_connection_route(&points) {
                    return points;
                }
                return Vec::new();
            }
            line.points.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionRouteFallback {
    SemanticReanchor,
    SemanticEndpoint,
    Manhattan,
    CanonicalOrthogonal,
}

/// Construct the smallest valid orthogonal route between semantic endpoints.
///
/// This is deliberately independent of the source annotation.  A source
/// `Line.points` array may be stale after a component/connector placement has
/// changed, but a displayed route must still meet the resolved connector
/// anchors.  Keep the source route's first segment orientation when it is
/// usable so the fallback does not needlessly flip an existing corner.
fn semantic_endpoint_route(
    first: CorePoint,
    last: CorePoint,
    base_points: &[CorePoint],
) -> Vec<CorePoint> {
    if (first.x - last.x).abs() <= ORTHOGONAL_EPSILON
        || (first.y - last.y).abs() <= ORTHOGONAL_EPSILON
    {
        return vec![first, last];
    }

    let horizontal_first = match base_points.first().zip(base_points.get(1)) {
        Some((start, next)) if (start.y - next.y).abs() <= ORTHOGONAL_EPSILON => true,
        Some((start, next)) if (start.x - next.x).abs() <= ORTHOGONAL_EPSILON => false,
        _ => (last.x - first.x).abs() >= (last.y - first.y).abs(),
    };
    let elbow = if horizontal_first {
        CorePoint {
            x: last.x,
            y: first.y,
        }
    } else {
        CorePoint {
            x: first.x,
            y: last.y,
        }
    };
    vec![first, elbow, last]
}

fn canonical_orthogonal_route(
    first: CorePoint,
    last: CorePoint,
    route_hint: &[CorePoint],
) -> Vec<CorePoint> {
    if (first.x - last.x).abs() <= ORTHOGONAL_EPSILON
        || (first.y - last.y).abs() <= ORTHOGONAL_EPSILON
    {
        return vec![first, last];
    }

    let source_horizontal = match route_hint.first().zip(route_hint.get(1)) {
        Some((start, next)) if (start.y - next.y).abs() <= ORTHOGONAL_EPSILON => true,
        Some((start, next)) if (start.x - next.x).abs() <= ORTHOGONAL_EPSILON => false,
        _ => (last.x - first.x).abs() >= (last.y - first.y).abs(),
    };
    let first_moved = route_hint
        .first()
        .is_some_and(|point| distance_between(*point, first) > ORTHOGONAL_EPSILON);
    let last_moved = route_hint
        .last()
        .is_some_and(|point| distance_between(*point, last) > ORTHOGONAL_EPSILON);

    if first_moved == last_moved {
        return semantic_endpoint_route(first, last, route_hint);
    }

    // Preserve the side of a two-ended route that did not move. This is the
    // same stable choice used by interactive re-anchoring, and avoids a
    // visible corner flip when a stale source route is repaired on startup.
    let horizontal_first = match (first_moved, last_moved) {
        (false, true) => !source_horizontal,
        (true, false) => source_horizontal,
        _ => source_horizontal,
    };
    let elbow = if horizontal_first {
        CorePoint {
            x: last.x,
            y: first.y,
        }
    } else {
        CorePoint {
            x: first.x,
            y: last.y,
        }
    };
    vec![first, elbow, last]
}

fn stale_route_reason(
    raw_points: &[CorePoint],
    semantic_first: CorePoint,
    semantic_last: CorePoint,
) -> Option<StaleRouteReason> {
    if !valid_interactive_connection_route(raw_points) {
        return Some(StaleRouteReason::InvalidSourceRoute);
    }
    let first_drift = distance_between(raw_points[0], semantic_first);
    let last_drift = distance_between(
        *raw_points.last().expect("valid route has a last point"),
        semantic_last,
    );
    if first_drift > ROUTE_REPAIR_ENDPOINT_DRIFT_UNITS
        || last_drift > ROUTE_REPAIR_ENDPOINT_DRIFT_UNITS
    {
        return Some(StaleRouteReason::EndpointDrift);
    }

    if raw_points.len() > 2 {
        let direct_manhattan =
            (semantic_first.x - semantic_last.x).abs() + (semantic_first.y - semantic_last.y).abs();
        let route_length = raw_points
            .windows(2)
            .map(|pair| distance_between(pair[0], pair[1]))
            .sum::<f32>();
        let allowed_route_length = direct_manhattan * ROUTE_REPAIR_MAX_DETOUR_RATIO;
        if route_length > allowed_route_length.max(ROUTE_REPAIR_ENDPOINT_DRIFT_UNITS) {
            return Some(StaleRouteReason::ExcessiveDetour);
        }
    }

    None
}

fn trace_route_repair(
    connection: &modelica_core::scene::DiagramConnection,
    raw_points: &[CorePoint],
    semantic_first: CorePoint,
    semantic_last: CorePoint,
    reason: StaleRouteReason,
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_ROUTE_REPAIR").is_none() {
        return;
    }
    eprintln!(
        "[ROUTE REPAIR] connection={} raw_points={raw_points:?} semantic_start={semantic_first:?} semantic_end={semantic_last:?} stale_reason={} fallback=canonical_orthogonal",
        connection.id,
        reason.label(),
    );
}

fn resolved_connection_display_route(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    raw_points: &[CorePoint],
) -> Result<(Vec<CorePoint>, ConnectionRouteFallback), String> {
    let (lhs, rhs) = strict_connection_points(scene, connection)
        .map_err(|error| format!("unable to resolve connector anchors: {error:?}"))?;
    let base_points = if valid_interactive_connection_route(raw_points) {
        raw_points.to_vec()
    } else {
        vec![lhs, rhs]
    };

    let mut stale_reason = stale_route_reason(raw_points, lhs, rhs);
    if stale_reason.is_none() {
        match reanchor_connection_points(scene, connection, &base_points) {
            Ok(reanchored) => {
                let canonical = canonicalize_orthogonal_points(&reanchored);
                if valid_interactive_connection_route(&canonical)
                    && displayed_connection_points_match_invariant(scene, connection, &canonical)
                {
                    return Ok((canonical, ConnectionRouteFallback::SemanticReanchor));
                }
                stale_reason = Some(if valid_interactive_connection_route(&canonical) {
                    StaleRouteReason::ReanchorEndpointMismatch
                } else {
                    StaleRouteReason::ReanchorInvalid
                });
            }
            Err(_) => stale_reason = Some(StaleRouteReason::ReanchorInvalid),
        }
    }

    if let Some(reason) = stale_reason {
        let fallback =
            canonicalize_orthogonal_points(&canonical_orthogonal_route(lhs, rhs, raw_points));
        if valid_interactive_connection_route(&fallback)
            && displayed_connection_points_match_invariant(scene, connection, &fallback)
        {
            trace_route_repair(connection, raw_points, lhs, rhs, reason);
            return Ok((fallback, ConnectionRouteFallback::CanonicalOrthogonal));
        }
        return Err("unable to construct a canonical orthogonal display route".to_owned());
    }

    let moved_first_endpoint = raw_points
        .first()
        .is_some_and(|point| distance_between(*point, lhs) > ORTHOGONAL_EPSILON);
    let moved_last_endpoint = raw_points
        .last()
        .is_some_and(|point| distance_between(*point, rhs) > ORTHOGONAL_EPSILON);
    let fallback = canonicalize_orthogonal_points(&manhattan_component_translation_route(
        &base_points,
        lhs,
        rhs,
        moved_first_endpoint,
        moved_last_endpoint,
    ));
    if valid_interactive_connection_route(&fallback)
        && displayed_connection_points_match_invariant(scene, connection, &fallback)
    {
        return Ok((fallback, ConnectionRouteFallback::Manhattan));
    }

    // The route-preserving and translation fallbacks can both reject a
    // malformed source polyline (for example, after a connector placement
    // changed the endpoint axes).  Semantic anchors are still authoritative;
    // never let a display path fall back to the stale source endpoints when
    // they resolved successfully.
    let endpoint_fallback =
        canonicalize_orthogonal_points(&canonical_orthogonal_route(lhs, rhs, raw_points));
    if valid_interactive_connection_route(&endpoint_fallback)
        && displayed_connection_points_match_invariant(scene, connection, &endpoint_fallback)
    {
        return Ok((endpoint_fallback, ConnectionRouteFallback::SemanticEndpoint));
    }

    Err("unable to construct an orthogonal route anchored to both connectors".to_owned())
}

fn trace_connection_reanchor(
    component_id: &str,
    snapshot: &ConnectionDragSnapshot,
    old_endpoint: Option<(CorePoint, CorePoint)>,
    semantic_endpoint: Option<(CorePoint, CorePoint)>,
    committed_endpoint: Option<(CorePoint, CorePoint)>,
    fallback: ConnectionRouteFallback,
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_COMPONENT_DRAG").is_none()
        && std::env::var_os("MODELICA_WGPU_TRACE_COMPONENT_EDIT").is_none()
        && std::env::var_os("MODELICA_WGPU_TRACE_CONNECTION_EDIT").is_none()
    {
        return;
    }
    eprintln!(
        "[CONNECTION REANCHOR] component_id={component_id} connection_key={:?} source_editable={} old_endpoint={old_endpoint:?} new_semantic_endpoint={semantic_endpoint:?} committed_display_endpoint={committed_endpoint:?} route_fallback={fallback:?} source_edit_error={:?}",
        snapshot.connection_key,
        snapshot.source_editable,
        snapshot.source_edit_error,
    );
}

/// Resolve the two route forms needed after a committed connection change.
///
/// The semantic route is used by hit testing and interaction. The display
/// route is the same route used by initial scene construction, including the
/// display-only connector overdraw. Keeping both results together prevents a
/// commit frame from showing different geometry than the next rebuild.
fn connection_geometry_points(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    stroke_zoom: f32,
) -> Option<(Vec<CorePoint>, Vec<CorePoint>)> {
    connection.line.as_ref()?;
    let semantic_points = canonical_connection_points(scene, connection);
    (semantic_points.len() >= 2).then(|| {
        let display_points =
            display_connection_points(scene, connection, &semantic_points, stroke_zoom);
        (semantic_points, display_points)
    })
}

#[cfg(test)]
fn core_diagram_geometry(scene: &CoreDiagramScene) -> Vec<Geometry> {
    core_diagram_geometry_at_zoom(scene, INITIAL_ZOOM)
}

/// Add a small display-only overlap between a connection stroke and each
/// connector graphic. The returned points remain in Line-local coordinates;
/// semantic anchors, hit testing, snapping, and source edits continue to use
/// the unmodified route passed into this function.
fn display_connection_points(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    semantic_points: &[CorePoint],
    stroke_zoom: f32,
) -> Vec<CorePoint> {
    let Some(line) = connection.line.as_ref() else {
        return semantic_points.to_vec();
    };
    if semantic_points.len() < 2 {
        return semantic_points.to_vec();
    }
    let Ok(endpoints) = resolve_connection_endpoints(scene, connection) else {
        return semantic_points.to_vec();
    };
    let (first_anchor, last_anchor) = match endpoints.point_order {
        ConnectionPointOrder::LhsToRhs => (&endpoints.lhs, &endpoints.rhs),
        ConnectionPointOrder::RhsToLhs => (&endpoints.rhs, &endpoints.lhs),
    };
    let max_overdraw = CONNECTION_ENDPOINT_OVERDRAW_PX / stroke_zoom.max(MIN_ZOOM);
    let mut display_points = semantic_points.to_vec();
    if let Some(point) =
        display_connection_endpoint(line, &display_points, 0, first_anchor, max_overdraw)
    {
        display_points[0] = point;
    }
    let last_index = display_points.len() - 1;
    if let Some(point) =
        display_connection_endpoint(line, &display_points, last_index, last_anchor, max_overdraw)
    {
        display_points[last_index] = point;
    }
    if valid_interactive_connection_route(&display_points) {
        display_points
    } else {
        semantic_points.to_vec()
    }
}

fn display_connection_endpoint(
    line: &LineGraphic,
    points: &[CorePoint],
    endpoint_index: usize,
    anchor: &ConnectorAnchor,
    max_overdraw: f32,
) -> Option<CorePoint> {
    let bounds = anchor.visual_bounds?;
    let endpoint = line_local_to_world(line, *points.get(endpoint_index)?);
    let neighbor_index = if endpoint_index == 0 {
        1
    } else {
        points.len().checked_sub(2)?
    };
    let neighbor = line_local_to_world(line, *points.get(neighbor_index)?);
    let target = endpoint_overdraw_target(endpoint, neighbor, bounds, max_overdraw)?;
    Some(world_to_line_local(line, target))
}

/// Return a point a few pixels inside an axis-aligned connector visual bound.
/// The ray pointing away from the route is preferred; the opposite ray is a
/// fallback for glyphs whose semantic origin is at their tip or outer rim.
fn endpoint_overdraw_target(
    endpoint: CorePoint,
    route_neighbor: CorePoint,
    bounds: modelica_render::Bounds,
    max_overdraw: f32,
) -> Option<CorePoint> {
    let route_delta = CorePoint {
        x: route_neighbor.x - endpoint.x,
        y: route_neighbor.y - endpoint.y,
    };
    let route_length = route_delta.x.hypot(route_delta.y);
    if route_length <= ORTHOGONAL_EPSILON {
        return None;
    }
    let route_direction = CorePoint {
        x: route_delta.x / route_length,
        y: route_delta.y / route_length,
    };
    let directions = [
        CorePoint {
            x: -route_direction.x,
            y: -route_direction.y,
        },
        route_direction,
    ];
    let min_x = bounds.x.min(bounds.x + bounds.width);
    let max_x = bounds.x.max(bounds.x + bounds.width);
    let min_y = bounds.y.min(bounds.y + bounds.height);
    let max_y = bounds.y.max(bounds.y + bounds.height);
    for direction in directions {
        let Some((entry, exit)) =
            ray_bounds_interval(endpoint, direction, min_x, max_x, min_y, max_y)
        else {
            continue;
        };
        let entry = entry.max(0.0);
        if exit <= entry + ORTHOGONAL_EPSILON {
            continue;
        }
        let distance = if entry <= ORTHOGONAL_EPSILON {
            max_overdraw.min(exit)
        } else if entry <= max_overdraw {
            entry + max_overdraw.min(exit - entry)
        } else {
            continue;
        };
        if distance > ORTHOGONAL_EPSILON {
            return Some(CorePoint {
                x: endpoint.x + direction.x * distance,
                y: endpoint.y + direction.y * distance,
            });
        }
    }
    None
}

fn ray_bounds_interval(
    origin: CorePoint,
    direction: CorePoint,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
) -> Option<(f32, f32)> {
    let mut entry = f32::NEG_INFINITY;
    let mut exit = f32::INFINITY;
    for (axis_origin, axis_direction, axis_min, axis_max) in [
        (origin.x, direction.x, min_x, max_x),
        (origin.y, direction.y, min_y, max_y),
    ] {
        if axis_direction.abs() <= ORTHOGONAL_EPSILON {
            if axis_origin < axis_min - ORTHOGONAL_EPSILON
                || axis_origin > axis_max + ORTHOGONAL_EPSILON
            {
                return None;
            }
            continue;
        }
        let first = (axis_min - axis_origin) / axis_direction;
        let second = (axis_max - axis_origin) / axis_direction;
        entry = entry.max(first.min(second));
        exit = exit.min(first.max(second));
        if entry > exit + ORTHOGONAL_EPSILON {
            return None;
        }
    }
    Some((entry, exit))
}

fn core_diagram_geometry_at_zoom(scene: &CoreDiagramScene, stroke_zoom: f32) -> Vec<Geometry> {
    let diagram_flip = Transform2D {
        scale_y: -1.0,
        ..Transform2D::identity()
    };
    let mut geometries = scene
        .background_graphics
        .iter()
        .flat_map(|graphic| core_graphic_geometry_from_graphic(graphic, diagram_flip, stroke_zoom))
        .map(|mut geometry| {
            geometry.layer = DiagramRenderLayer::Background;
            geometry
        })
        .collect::<Vec<_>>();
    for connection in &scene.connections {
        if let Some((_, display_points)) =
            connection_geometry_points(scene, connection, stroke_zoom)
        {
            let line = connection
                .line
                .as_ref()
                .expect("connection geometry has a line");
            let mut display_line = line.clone();
            display_line.points = display_points;
            geometries.extend(
                line_geometry(
                    &display_line,
                    diagram_flip,
                    stroke_zoom,
                    StrokeKind::Connection,
                )
                .into_iter()
                .map(|mut geometry| {
                    geometry.layer = DiagramRenderLayer::Connection;
                    geometry.edit_key = Some(connection.id.clone());
                    geometry.connection = Some(ConnectionGeometry {
                        line: display_line.clone(),
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
                core_graphic_geometry_from_graphic(&resolved.graphic, transform, stroke_zoom)
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

fn core_graphic_geometry_at_zoom(resolved: &ResolvedGraphic, stroke_zoom: f32) -> Vec<Geometry> {
    core_graphic_geometry_from_graphic(&resolved.graphic, resolved.transform, stroke_zoom)
}

fn core_graphic_geometry_from_graphic(
    graphic: &CoreGraphic,
    transform: Transform2D,
    stroke_zoom: f32,
) -> Vec<Geometry> {
    match graphic {
        CoreGraphic::Line(line) => line_geometry(line, transform, stroke_zoom, StrokeKind::Icon),
        CoreGraphic::Polygon(polygon) => polygon_geometry(polygon, transform, stroke_zoom),
        CoreGraphic::Rectangle(rectangle) => rectangle_geometry(rectangle, transform, stroke_zoom),
        CoreGraphic::Ellipse(ellipse) => ellipse_geometry(ellipse, transform, stroke_zoom),
        CoreGraphic::Text(_) | CoreGraphic::Bitmap(_) => Vec::new(),
    }
}

fn diagram_placement_transform(
    icon: &CoreIconScene,
    component: &CoreComponentInstance,
) -> Transform2D {
    effective_component_transform(icon, component, None)
}

fn effective_component_transform(
    icon: &CoreIconScene,
    component: &CoreComponentInstance,
    preview: Option<ComponentPreviewPlacement>,
) -> Transform2D {
    let preview = preview.unwrap_or(ComponentPreviewPlacement {
        origin: component.origin,
        rotation: component.rotation,
        extent: component
            .placement_extent
            .unwrap_or_else(default_component_extent),
        delta: CorePoint { x: 0.0, y: 0.0 },
    });
    diagram_placement_transform_for_extent(icon, preview.origin, preview.rotation, preview.extent)
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

fn line_geometry(
    line: &LineGraphic,
    transform: Transform2D,
    stroke_zoom: f32,
    kind: StrokeKind,
) -> Vec<Geometry> {
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
        model_stroke_width(line.thickness, transform, stroke_zoom, kind),
        color_rgba(line.color),
    )]
}

fn polygon_geometry(
    polygon: &PolygonGraphic,
    transform: Transform2D,
    stroke_zoom: f32,
) -> Vec<Geometry> {
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
        stroke_zoom,
    )
}

fn rectangle_geometry(
    rectangle: &RectangleGraphic,
    transform: Transform2D,
    stroke_zoom: f32,
) -> Vec<Geometry> {
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
        stroke_zoom,
    )
}

fn ellipse_geometry(
    ellipse: &EllipseGraphic,
    transform: Transform2D,
    stroke_zoom: f32,
) -> Vec<Geometry> {
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
            model_stroke_width(
                ellipse.line_thickness.unwrap_or(0.25),
                transform,
                stroke_zoom,
                StrokeKind::Icon,
            ),
            color_rgba(ellipse.line_color),
        ));
    }
    geometry
}

#[allow(clippy::too_many_arguments)]
fn closed_shape_geometry(
    points: &[[f32; 2]],
    fill_color: [u8; 3],
    fill_pattern: Option<&str>,
    line_color: [u8; 3],
    line_pattern: Option<&str>,
    line_thickness: Option<f32>,
    transform: Transform2D,
    stroke_zoom: f32,
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
            model_stroke_width(
                line_thickness.unwrap_or(0.25),
                transform,
                stroke_zoom,
                StrokeKind::Icon,
            ),
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

fn model_stroke_width(
    annotation_width: f32,
    transform: Transform2D,
    stroke_zoom: f32,
    kind: StrokeKind,
) -> f32 {
    let transformed_width = annotation_width.max(0.1) * transform_scale(transform);
    let minimum_screen_width = match kind {
        StrokeKind::Connection => MIN_SCREEN_CONNECTION_STROKE_PX,
        StrokeKind::Icon => MIN_SCREEN_ICON_STROKE_PX,
    };
    transformed_width.max(minimum_screen_width / stroke_zoom.max(MIN_ZOOM))
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
    pattern.is_none_or(|value| value.contains("None"))
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

fn canvas_zoom_controls_visible_for(main_view: MainView) -> bool {
    canvas_navigation_enabled_for(main_view)
}

fn should_fit_scene_after_view_change(previous: MainView, current: MainView) -> bool {
    previous != current
        && canvas_navigation_enabled_for(previous)
        && canvas_navigation_enabled_for(current)
}

fn physical_to_logical_position(position: PhysicalPosition<f64>, scale_factor: f32) -> Pos2 {
    let scale_factor = scale_factor.max(f32::EPSILON);
    Pos2::new(
        position.x as f32 / scale_factor,
        position.y as f32 / scale_factor,
    )
}

fn logical_to_physical_pixels(position: Pos2, pixels_per_point: f32) -> Pos2 {
    Pos2::new(position.x * pixels_per_point, position.y * pixels_per_point)
}

fn snap_point_to_physical_pixel(point: Pos2, pixels_per_point: f32) -> Pos2 {
    let pixels_per_point = valid_pixels_per_point(pixels_per_point);
    Pos2::new(
        snap_coordinate_to_physical_pixel(point.x, pixels_per_point),
        snap_coordinate_to_physical_pixel(point.y, pixels_per_point),
    )
}

fn snap_y_to_physical_pixel(point: Pos2, pixels_per_point: f32) -> Pos2 {
    let pixels_per_point = valid_pixels_per_point(pixels_per_point);
    Pos2::new(
        point.x,
        snap_coordinate_to_physical_pixel(point.y, pixels_per_point),
    )
}

fn valid_pixels_per_point(pixels_per_point: f32) -> f32 {
    if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    }
}

fn snap_coordinate_to_physical_pixel(coordinate: f32, pixels_per_point: f32) -> f32 {
    (coordinate * pixels_per_point).round() / pixels_per_point
}

fn snap_scroll_offset_to_physical_pixel(
    offset: f32,
    max_offset: f32,
    pixels_per_point: f32,
) -> f32 {
    let pixels_per_point = valid_pixels_per_point(pixels_per_point);
    let max_aligned_offset = (max_offset.max(0.0) * pixels_per_point).floor() / pixels_per_point;
    snap_coordinate_to_physical_pixel(offset.max(0.0), pixels_per_point)
        .clamp(0.0, max_aligned_offset)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WheelOwner {
    Egui,
    CanvasZoom,
    None,
}

fn wheel_owner(
    main_view: MainView,
    egui_consumed: bool,
    pointer_over_canvas: bool,
    control_pressed: bool,
) -> WheelOwner {
    if should_zoom_canvas(main_view, pointer_over_canvas, control_pressed) {
        WheelOwner::CanvasZoom
    } else if egui_consumed {
        WheelOwner::Egui
    } else {
        WheelOwner::None
    }
}

fn wheel_delta_sample(delta: MouseScrollDelta) -> SourceWheelSample {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => SourceWheelSample {
            kind: SourceWheelKind::LineDelta,
            delta_y: y,
        },
        MouseScrollDelta::PixelDelta(position) => SourceWheelSample {
            kind: SourceWheelKind::PixelDelta,
            delta_y: position.y as f32 / 80.0,
        },
    }
}

fn zoom_after_wheel(current_zoom: f32, wheel_delta: f32) -> f32 {
    (current_zoom * (1.0 + wheel_delta * 0.1)).clamp(MIN_ZOOM, MAX_ZOOM)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ZoomAction {
    In,
    Out,
}

fn zoom_after_toolbar_action(current_zoom: f32, action: ZoomAction) -> f32 {
    let next = match action {
        ZoomAction::In => current_zoom * 1.1,
        ZoomAction::Out => current_zoom / 1.1,
    };
    next.clamp(MIN_ZOOM, MAX_ZOOM)
}

fn zoom_percent(zoom: f32) -> u32 {
    (zoom / INITIAL_ZOOM * 100.0).round() as u32
}

fn pan_after_zoom_at_anchor(
    pan: [f32; 2],
    old_zoom: f32,
    new_zoom: f32,
    viewport_center: [f32; 2],
    anchor: [f32; 2],
) -> [f32; 2] {
    let world_before = [
        (anchor[0] - viewport_center[0] - pan[0]) / old_zoom,
        (anchor[1] - viewport_center[1] - pan[1]) / old_zoom,
    ];
    [
        anchor[0] - viewport_center[0] - world_before[0] * new_zoom,
        anchor[1] - viewport_center[1] - world_before[1] * new_zoom,
    ]
}

fn canvas_center_physical_anchor(
    canvas_rect: Option<Rect>,
    scale_factor: f32,
    viewport_size: [u32; 2],
) -> [f32; 2] {
    canvas_rect
        .map(|rect| logical_to_physical_pixels(rect.center(), scale_factor))
        .map(|center| [center.x, center.y])
        .unwrap_or([viewport_size[0] as f32 * 0.5, viewport_size[1] as f32 * 0.5])
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

fn point_in_component_placement_extent(
    component: &CoreComponentInstance,
    point: CorePoint,
    tolerance: f32,
) -> bool {
    let extent = component
        .placement_extent
        .unwrap_or_else(default_component_extent);
    let local = inverse_transform_point(
        point,
        Transform2D {
            translation: component.origin,
            rotation: component.rotation,
            scale_x: 1.0,
            scale_y: 1.0,
        },
    );
    let tolerance = tolerance.max(0.0);
    local.x >= extent.p1.x.min(extent.p2.x) - tolerance
        && local.x <= extent.p1.x.max(extent.p2.x) + tolerance
        && local.y >= extent.p1.y.min(extent.p2.y) - tolerance
        && local.y <= extent.p1.y.max(extent.p2.y) + tolerance
}

fn connection_world_points(line: &LineGraphic, points: &[CorePoint]) -> Vec<CorePoint> {
    points
        .iter()
        .map(|point| line_local_to_world(line, *point))
        .collect()
}

fn connector_anchor_active_hit_distance(
    anchor: &ConnectorAnchor,
    point: CorePoint,
    tolerance: f32,
) -> Option<f32> {
    let distance = distance_between(anchor.world_position, point);
    (distance <= tolerance.max(0.0)).then_some(distance)
}

fn component_drag_port_tolerance(zoom: f32) -> f32 {
    COMPONENT_DRAG_PORT_HIT_PIXELS / zoom.max(MIN_ZOOM)
}

fn compare_connector_anchors_stably(
    left: &ConnectorAnchor,
    right: &ConnectorAnchor,
) -> std::cmp::Ordering {
    left.owner_component_id
        .cmp(&right.owner_component_id)
        .then_with(|| {
            left.connector_ref
                .connector_path
                .cmp(&right.connector_ref.connector_path)
        })
        .then_with(|| {
            left.connector_ref
                .subscripts
                .cmp(&right.connector_ref.subscripts)
        })
        .then_with(|| {
            left.key
                .owner_component_id
                .cmp(&right.key.owner_component_id)
        })
        .then_with(|| left.key.connector_path.cmp(&right.key.connector_path))
}

fn compare_connector_anchor_hits(
    left_distance: f32,
    left: &ConnectorAnchor,
    right_distance: f32,
    right: &ConnectorAnchor,
) -> std::cmp::Ordering {
    let distance_order = if (left_distance - right_distance).abs() <= PORT_HIT_DISTANCE_TIE_EPSILON
    {
        std::cmp::Ordering::Equal
    } else {
        left_distance.total_cmp(&right_distance)
    };
    distance_order.then_with(|| compare_connector_anchors_stably(left, right))
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
            let Some(line) = line else {
                continue;
            };
            let Some((hit, distance)) = hit_test_connection_segment_with_distance(
                connection,
                segment_index,
                &line.points,
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
    points: &[CorePoint],
    pointer: CorePoint,
    tolerance: f32,
) -> Option<(ConnectionHit, f32)> {
    let line = connection.line.as_ref()?;
    let start = points
        .get(segment_index)
        .map(|point| line_local_to_world(line, *point))?;
    let end = points
        .get(segment_index + 1)
        .map(|point| line_local_to_world(line, *point))?;
    let distance = distance_to_segment(pointer, start, end);
    if distance > tolerance {
        return None;
    }
    let target = match points
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

fn placement_transformation_call(call: &AnnotationCall) -> Option<&AnnotationCall> {
    nested_call(call, "transformation").or_else(|| nested_call(call, "iconTransformation"))
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
        // The lightweight class parser can mistake `redeclare package ...`
        // inside a component modification for a nested class. Only mask
        // children that begin at the class body's delimiter depth; nested
        // declarations inside `(...)`, `{...}`, or `[...]` belong to the
        // enclosing component statement and must remain visible to the
        // source-edit scanner.
        if !source_delimiters_at(source, child.source_range.start)
            .is_some_and(|(parens, braces, brackets)| parens == 0 && braces == 0 && brackets == 0)
        {
            continue;
        }
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

fn source_delimiters_at(source: &str, offset: usize) -> Option<(i32, i32, i32)> {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    let mut parens = 0;
    let mut braces = 0;
    let mut brackets = 0;
    for token in tokenize(&source[..offset]) {
        match token.text.as_str() {
            "(" => parens += 1,
            ")" => parens -= 1,
            "{" => braces += 1,
            "}" => braces -= 1,
            "[" => brackets += 1,
            "]" => brackets -= 1,
            _ => {}
        }
    }
    Some((parens, braces, brackets))
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
        let Some(declaration) = parse_component_declaration(&tokenize(statement)) else {
            continue;
        };
        if declaration.instance_name != component_name {
            continue;
        }
        let Some(placement) = nested_call(&annotation, "Placement") else {
            continue;
        };
        let Some(transformation) = placement_transformation_call(placement) else {
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
        let Some(declaration) = parse_component_declaration(&tokenize(statement)) else {
            continue;
        };
        if declaration.instance_name != component_name {
            continue;
        }
        let Some(placement) = nested_call(&annotation, "Placement") else {
            continue;
        };
        let Some(transformation) = placement_transformation_call(placement) else {
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
    let (first_distance, last_distance) = match endpoints.point_order {
        ConnectionPointOrder::LhsToRhs => (endpoints.lhs_distance, endpoints.rhs_distance),
        ConnectionPointOrder::RhsToLhs => (
            distance_between(endpoints.rhs.world_position, endpoints.lhs_line_position),
            distance_between(endpoints.lhs.world_position, endpoints.rhs_line_position),
        ),
    };
    first_distance <= DIAGRAM_GEOMETRY_EPSILON && last_distance <= DIAGRAM_GEOMETRY_EPSILON
}

/// Anchor, simplify, and validate a route before it is serialized.
///
/// Re-anchoring is intentionally part of the fixed-point loop: replacing a
/// bridge with its semantic endpoint can create a new duplicate or collinear
/// vertex. Canonicalization therefore never gets to remove an endpoint, and
/// the route is only accepted once both operations are stable.
#[cfg(test)]
fn finalize_connection_route(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    raw_points: &[CorePoint],
) -> Result<Vec<CorePoint>, String> {
    let (lhs, rhs) = strict_connection_points(scene, connection)
        .map_err(|error| format!("unable to resolve connector anchors: {error:?}"))?;
    finalize_connection_route_with_constraint(
        scene,
        connection,
        raw_points,
        ConnectionEndpointConstraint::Semantic { lhs, rhs },
    )
}

fn finalize_connection_route_with_constraint(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    raw_points: &[CorePoint],
    endpoint_constraint: ConnectionEndpointConstraint,
) -> Result<Vec<CorePoint>, String> {
    if raw_points.len() < 2 {
        return Err("connection route must contain at least two points".to_owned());
    }

    let (lhs, rhs) = endpoint_constraint.points();
    let mut points = raw_points.to_vec();
    loop {
        let anchored = match endpoint_constraint {
            ConnectionEndpointConstraint::Semantic { .. } => {
                reanchor_connection_points(scene, connection, &points)
                    .map_err(|error| format!("unable to resolve connector anchors: {error:?}"))?
            }
            ConnectionEndpointConstraint::FixedExisting { .. } => {
                let mut anchored = points.clone();
                anchored[0] = lhs;
                *anchored.last_mut().expect("at least two points") = rhs;
                anchored
            }
        };
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

    if distance_between(points[0], lhs) > DIAGRAM_GEOMETRY_EPSILON
        || distance_between(*points.last().expect("at least two points"), rhs)
            > DIAGRAM_GEOMETRY_EPSILON
    {
        return Err(match endpoint_constraint {
            ConnectionEndpointConstraint::Semantic { .. } => {
                "connection route endpoints are not anchored".to_owned()
            }
            ConnectionEndpointConstraint::FixedExisting { .. } => {
                "fixed connection endpoints changed".to_owned()
            }
        });
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

fn valid_interactive_connection_route(points: &[CorePoint]) -> bool {
    if points.len() < 2
        || points
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return false;
    }
    if points
        .windows(2)
        .any(|pair| distance_between(pair[0], pair[1]) <= ORTHOGONAL_EPSILON)
    {
        return false;
    }
    is_orthogonal_polyline(points)
}

fn displayed_connection_points_match_invariant(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    displayed_points: &[CorePoint],
) -> bool {
    let Ok((semantic_first, semantic_last)) = strict_connection_points(scene, connection) else {
        return false;
    };
    displayed_points
        .first()
        .zip(displayed_points.last())
        .is_some_and(|(first, last)| {
            point_nearly_equal(*first, semantic_first) && point_nearly_equal(*last, semantic_last)
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
    connection_invariant_failure_with_constraint(scene, connection, expected_points, None)
}

fn connection_invariant_failure_with_constraint(
    scene: &CoreDiagramScene,
    connection: &modelica_core::scene::DiagramConnection,
    expected_points: &[CorePoint],
    endpoint_constraint: Option<ConnectionEndpointConstraint>,
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
    let endpoints_match = match endpoint_constraint {
        Some(ConnectionEndpointConstraint::FixedExisting { lhs, rhs }) => {
            point_nearly_equal(*first, lhs)
                && point_nearly_equal(*line.points.last().expect("line has a last point"), rhs)
        }
        Some(ConnectionEndpointConstraint::Semantic { lhs, rhs }) => {
            let Ok((resolved_lhs, resolved_rhs)) = strict_connection_points(scene, connection)
            else {
                return Some("endpoint anchor mismatch");
            };
            point_nearly_equal(resolved_lhs, lhs) && point_nearly_equal(resolved_rhs, rhs)
        }
        None => connection_endpoints_match(scene, connection),
    };
    if !endpoints_match {
        return Some("endpoint anchor mismatch");
    }
    None
}

fn connection_drag_snapshots(
    scene: &CoreDiagramScene,
    component_name: &str,
    class_name: &str,
    source: &str,
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
            let source_edit_error =
                connection_source_editable_in_class(connection, class_name, source)
                    .err()
                    .inspect(|error| {
                        trace_component_connection_state(component_name, connection, error)
                    });
            let source_editable = source_edit_error.is_none();
            let line = connection.line.as_ref();
            let source_line_points = line.map_or_else(Vec::new, |line| line.points.clone());
            let original_line_origin =
                line.map_or(CorePoint { x: 0.0, y: 0.0 }, |line| line.origin);
            let original_line_rotation = line.map_or(0.0, |line| line.rotation);
            let resolved_endpoints = resolve_connection_endpoints(scene, connection).ok();
            let original_endpoint_points = strict_connection_points(scene, connection)
                .ok()
                .or_else(|| {
                    line.and_then(|line| {
                        line.points
                            .first()
                            .zip(line.points.last())
                            .map(|(first, last)| (*first, *last))
                    })
                })
                .unwrap_or((CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 0.0, y: 0.0 }));
            let base_route_points = line
                .map(|_| canonical_connection_points(scene, connection))
                .filter(|points| valid_interactive_connection_route(points))
                .or_else(|| {
                    line.map(|line| line.points.clone())
                        .filter(|points| valid_interactive_connection_route(points))
                })
                .or_else(|| {
                    valid_interactive_connection_route(&[
                        original_endpoint_points.0,
                        original_endpoint_points.1,
                    ])
                    .then(|| vec![original_endpoint_points.0, original_endpoint_points.1])
                })
                .unwrap_or_default();
            if base_route_points.is_empty() {
                trace_component_connection_state(
                    component_name,
                    connection,
                    "connection has no initial display route; semantic tracking retained",
                );
            }
            let (moved_first_endpoint, moved_last_endpoint) = match resolved_endpoints
                .as_ref()
                .map(|endpoints| endpoints.point_order)
            {
                Some(ConnectionPointOrder::LhsToRhs) => (
                    connection.lhs.component_name == component_name,
                    connection.rhs.component_name == component_name,
                ),
                Some(ConnectionPointOrder::RhsToLhs) => (
                    connection.rhs.component_name == component_name,
                    connection.lhs.component_name == component_name,
                ),
                None => (
                    connection.lhs.component_name == component_name,
                    connection.rhs.component_name == component_name,
                ),
            };
            Some(ConnectionDragSnapshot {
                connection_id: connection.id.clone(),
                connection_key: connection.key.clone(),
                source_editable,
                source_edit_error,
                source_line_points,
                base_route_points: base_route_points.clone(),
                original_line_origin,
                original_line_rotation,
                original_endpoint_points,
                preview_points: base_route_points.clone(),
                preview_route_valid: valid_interactive_connection_route(&base_route_points),
                moved_first_endpoint,
                moved_last_endpoint,
            })
        })
        .collect()
}

fn trace_component_connection_state(
    component_name: &str,
    connection: &modelica_core::scene::DiagramConnection,
    reason: impl std::fmt::Display,
) {
    if std::env::var_os("MODELICA_WGPU_TRACE_COMPONENT_DRAG").is_some()
        || std::env::var_os("MODELICA_WGPU_TRACE_COMPONENT_EDIT").is_some()
    {
        eprintln!(
            "[COMPONENT DRAG] component={component_name} connection_id={} source_edit_unavailable={reason}",
            connection.id
        );
    }
}

fn component_connection_drag_preflight(
    scene: &CoreDiagramScene,
    component_name: &str,
    class_name: &str,
    source: &str,
) -> Result<(), String> {
    for connection in scene.connections.iter().filter(|connection| {
        connection.lhs.component_name == component_name
            || connection.rhs.component_name == component_name
    }) {
        connection_source_editable_in_class(connection, class_name, source).map_err(|error| {
            format!(
                "connection {} cannot be moved with component: {error}",
                connection.id
            )
        })?;
        let line = connection.line.as_ref().ok_or_else(|| {
            format!(
                "connection {} has no editable Line annotation",
                connection.id
            )
        })?;
        if line.points.len() < 2 {
            return Err(format!(
                "connection {} has fewer than two Line points",
                connection.id
            ));
        }
        resolve_connection_endpoints(scene, connection).map_err(|error| {
            format!(
                "connection {} has unresolved connector endpoints: {error:?}",
                connection.id
            )
        })?;
        if canonical_connection_points(scene, connection).len() < 2 {
            return Err(format!(
                "connection {} has no usable orthogonal route",
                connection.id
            ));
        }
    }
    Ok(())
}

fn build_component_connection_edits(
    scene: &CoreDiagramScene,
    class_name: &str,
    source: &str,
    snapshots: &[ConnectionDragSnapshot],
) -> Result<Vec<(ConnectionLineEdit, SourceEdit)>, String> {
    let mut edits = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        if !snapshot.source_editable {
            // Inherited/read-only connections are not written to the current
            // class. Their display route is rebuilt from semantic endpoints.
            continue;
        }
        if !snapshot.preview_route_valid
            || !valid_interactive_connection_route(&snapshot.preview_points)
        {
            return Err(format!(
                "connection {:?} has an invalid editable preview route",
                snapshot.connection_key
            ));
        }
        let connection = scene
            .connections
            .iter()
            .find(|connection| connection.key == snapshot.connection_key)
            .ok_or_else(|| {
                format!(
                    "connection {:?} is missing from the current scene",
                    snapshot.connection_key
                )
            })?;
        connection_source_editable_in_class(connection, class_name, source).map_err(|error| {
            format!(
                "connection {:?} source is no longer editable: {error}",
                snapshot.connection_key
            )
        })?;
        let line = connection.line.as_ref().ok_or_else(|| {
            format!(
                "connection {:?} has no editable Line annotation",
                snapshot.connection_key
            )
        })?;
        if points_nearly_equal(&line.points, &snapshot.preview_points) {
            continue;
        }
        let source_edit = connection_points_edit_for_key(
            source,
            scene,
            &snapshot.connection_key,
            &snapshot.preview_points,
        )
        .map_err(|error| {
            format!(
                "connection {:?} source edit failed: {error}",
                snapshot.connection_key
            )
        })?;
        edits.push((
            ConnectionLineEdit {
                connection_key: snapshot.connection_key.clone(),
                before_points: snapshot.source_line_points.clone(),
                after_points: snapshot.preview_points.clone(),
                line_origin: line.origin,
            },
            source_edit,
        ));
    }
    Ok(edits)
}

fn connection_route_for_component_translation(
    base_route_points: &[CorePoint],
    line_origin: CorePoint,
    line_rotation: f32,
    original_endpoint_points: (CorePoint, CorePoint),
    moved_first_endpoint: bool,
    moved_last_endpoint: bool,
    world_delta: CorePoint,
) -> Vec<CorePoint> {
    if base_route_points.len() < 2 || (!moved_first_endpoint && !moved_last_endpoint) {
        return base_route_points.to_vec();
    }
    let local_delta = world_delta_to_line_local(line_origin, line_rotation, world_delta);
    let (original_lhs, original_rhs) = original_endpoint_points;
    let lhs = if moved_first_endpoint {
        translated_point(original_lhs, local_delta)
    } else {
        original_lhs
    };
    let rhs = if moved_last_endpoint {
        translated_point(original_rhs, local_delta)
    } else {
        original_rhs
    };

    if base_route_points.len() == 2 {
        return two_point_component_translation_route(
            base_route_points,
            lhs,
            rhs,
            moved_first_endpoint,
            moved_last_endpoint,
        );
    }

    let mut points =
        connection_drag_points_with_semantic_endpoints(base_route_points, original_endpoint_points);
    if moved_first_endpoint {
        points[0] = lhs;
        preserve_orthogonal_neighbor(
            &mut points[1],
            original_lhs,
            base_route_points[1],
            local_delta,
        );
    }
    if moved_last_endpoint {
        let last_index = points.len() - 1;
        points[last_index] = rhs;
        preserve_orthogonal_neighbor(
            &mut points[last_index - 1],
            original_rhs,
            base_route_points[last_index - 1],
            local_delta,
        );
    }
    points
}

fn component_drag_preview_route(
    snapshot: &ConnectionDragSnapshot,
    world_delta: CorePoint,
) -> Vec<CorePoint> {
    let base_route = if snapshot.base_route_points.len() >= 2 {
        &snapshot.base_route_points
    } else {
        &snapshot.source_line_points
    };
    let route = connection_route_for_component_translation(
        base_route,
        snapshot.original_line_origin,
        snapshot.original_line_rotation,
        snapshot.original_endpoint_points,
        snapshot.moved_first_endpoint,
        snapshot.moved_last_endpoint,
        world_delta,
    );
    if valid_interactive_connection_route(&route) {
        return route;
    }

    let local_delta = world_delta_to_line_local(
        snapshot.original_line_origin,
        snapshot.original_line_rotation,
        world_delta,
    );
    let lhs = if snapshot.moved_first_endpoint {
        translated_point(snapshot.original_endpoint_points.0, local_delta)
    } else {
        snapshot.original_endpoint_points.0
    };
    let rhs = if snapshot.moved_last_endpoint {
        translated_point(snapshot.original_endpoint_points.1, local_delta)
    } else {
        snapshot.original_endpoint_points.1
    };
    let fallback = manhattan_component_translation_route(
        base_route,
        lhs,
        rhs,
        snapshot.moved_first_endpoint,
        snapshot.moved_last_endpoint,
    );
    if valid_interactive_connection_route(&fallback) {
        fallback
    } else {
        route
    }
}

fn translated_point(point: CorePoint, delta: CorePoint) -> CorePoint {
    CorePoint {
        x: point.x + delta.x,
        y: point.y + delta.y,
    }
}

fn two_point_component_translation_route(
    base_route_points: &[CorePoint],
    lhs: CorePoint,
    rhs: CorePoint,
    moved_first_endpoint: bool,
    moved_last_endpoint: bool,
) -> Vec<CorePoint> {
    manhattan_component_translation_route(
        base_route_points,
        lhs,
        rhs,
        moved_first_endpoint,
        moved_last_endpoint,
    )
}

fn manhattan_component_translation_route(
    base_route_points: &[CorePoint],
    lhs: CorePoint,
    rhs: CorePoint,
    moved_first_endpoint: bool,
    moved_last_endpoint: bool,
) -> Vec<CorePoint> {
    if (lhs.x - rhs.x).abs() <= ORTHOGONAL_EPSILON || (lhs.y - rhs.y).abs() <= ORTHOGONAL_EPSILON {
        return vec![lhs, rhs];
    }
    let first_horizontal = base_route_points
        .first()
        .zip(base_route_points.get(1))
        .is_some_and(|(first, second)| (first.y - second.y).abs() <= ORTHOGONAL_EPSILON);
    let last_horizontal = base_route_points
        .get(base_route_points.len().saturating_sub(2))
        .zip(base_route_points.last())
        .is_some_and(|(previous, last)| (previous.y - last.y).abs() <= ORTHOGONAL_EPSILON);
    let horizontal_first = if moved_last_endpoint && !moved_first_endpoint {
        last_horizontal
    } else {
        first_horizontal
    };
    let elbow = if horizontal_first {
        if moved_last_endpoint && !moved_first_endpoint {
            CorePoint { x: lhs.x, y: rhs.y }
        } else {
            CorePoint { x: rhs.x, y: lhs.y }
        }
    } else if moved_last_endpoint && !moved_first_endpoint {
        CorePoint { x: rhs.x, y: lhs.y }
    } else {
        CorePoint { x: lhs.x, y: rhs.y }
    };
    vec![lhs, elbow, rhs]
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
    if points.len() < 2 {
        return points;
    }
    let original_lhs = points[0];
    let original_rhs = *points.last().expect("at least two points");
    points[0] = lhs;
    let last_index = points.len() - 1;
    points[last_index] = rhs;

    // Parsed Modelica annotations may place a line endpoint a small distance
    // away from its semantic connector position. After replacing the endpoint,
    // carry that position onto the adjacent point on the route's existing axis
    // so segment/corner editing starts from a truly orthogonal polyline.
    align_endpoint_neighbor_to_route_axis(&mut points[1], original_lhs, original_points[1], lhs);
    align_endpoint_neighbor_to_route_axis(
        &mut points[last_index - 1],
        original_rhs,
        original_points[last_index - 1],
        rhs,
    );
    points
}

fn align_endpoint_neighbor_to_route_axis(
    neighbor: &mut CorePoint,
    original_endpoint: CorePoint,
    original_neighbor: CorePoint,
    endpoint: CorePoint,
) {
    match segment_orientation(original_endpoint, original_neighbor) {
        Some(ConnectionSegmentOrientation::Horizontal) => neighbor.y = endpoint.y,
        Some(ConnectionSegmentOrientation::Vertical) => neighbor.x = endpoint.x,
        None => {}
    }
}

fn build_connection_segment_drag_route(
    original_points: &[CorePoint],
    segment_index: usize,
    orientation: ConnectionSegmentOrientation,
    line_origin: CorePoint,
    line_rotation: f32,
    delta: CorePoint,
    endpoint_constraint: ConnectionEndpointConstraint,
) -> Vec<CorePoint> {
    let anchored_points = connection_drag_points_with_semantic_endpoints(
        original_points,
        endpoint_constraint.points(),
    );
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
    endpoint_constraint: ConnectionEndpointConstraint,
) -> Vec<CorePoint> {
    let anchored_points = connection_drag_points_with_semantic_endpoints(
        original_points,
        endpoint_constraint.points(),
    );
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
    reference.text()
}

fn connection_source_editable_in_class(
    connection: &modelica_core::scene::DiagramConnection,
    class_name: &str,
    source: &str,
) -> Result<(), String> {
    if connection.key.owner_class != class_name {
        return Err(format!(
            "Connection is inherited from {} and is read-only in {}",
            connection.key.owner_class, class_name
        ));
    }
    let line_source_range = connection
        .line_source_range
        .ok_or_else(|| "Connection has no editable Line source range".to_owned())?;
    if line_source_range.start > line_source_range.end
        || line_source_range.end > source.len()
        || source
            .get(line_source_range.start..line_source_range.end)
            .is_none()
    {
        return Err("Connection Line source range is stale".to_owned());
    }
    let line_source = source
        .get(line_source_range.start..line_source_range.end)
        .expect("validated connection Line source range");
    let line = parse_call(line_source)
        .map_err(|error| format!("Connection Line annotation cannot be parsed: {error}"))?;
    if line.name != "Line" {
        return Err("Connection source range does not point to a Line annotation".to_owned());
    }
    if line.named("points").is_none() {
        return Err("Connection Line has no points argument".to_owned());
    }
    Ok(())
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
    Ok(apply_validated_source_edits_with_parsed(source, edits, version)?.source)
}

fn tree_node_icon(kind: Option<ClassKind>) -> &'static str {
    match kind {
        Some(ClassKind::Package) => "▱",
        Some(ClassKind::Model) => "◇",
        Some(ClassKind::Block) => "■",
        Some(ClassKind::Connector | ClassKind::ExpandableConnector) => "●",
        Some(ClassKind::Record | ClassKind::OperatorRecord) => "▤",
        Some(ClassKind::Function | ClassKind::OperatorFunction) => "ƒ",
        Some(ClassKind::Type) => "T",
        Some(ClassKind::Operator) => "◈",
        Some(ClassKind::Class) => "◆",
        None => "·",
    }
}

fn apply_validated_source_edits_with_parsed(
    source: &str,
    edits: Vec<SourceEdit>,
    version: u64,
) -> Result<ValidatedSourceCandidate, String> {
    let transaction = SourceTransaction {
        edits,
        source_version: Some(version),
    };
    let transaction_started = Instant::now();
    let candidate = apply_source_transaction(source, &transaction, Some(version))
        .map_err(|error| error.to_string())?;
    let transaction_apply = transaction_started.elapsed();
    let parse_started = Instant::now();
    let parsed = parse(&candidate, "<candidate>")
        .map_err(|error| format!("candidate source does not parse: {error}"))?;
    let parse = parse_started.elapsed();
    Ok(ValidatedSourceCandidate {
        source: candidate,
        parsed,
        transaction_apply,
        parse,
    })
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
        expand_top_level(&mut app.expanded_nodes, &doc.model_tree);
    }
    app.refresh_ui_document();
    app.update_title(None);
    window.request_redraw();

    event_loop
        .run(move |event, event_loop| {
            event_loop.set_control_flow(ControlFlow::Wait);
            match event {
                Event::WindowEvent { window_id, event } if window_id == app.window.id() => {
                    // Ctrl+wheel over an Icon/Diagram canvas is an explicit
                    // canvas gesture. Do not first feed it to egui, where a
                    // ScrollArea or zoom handler could consume it as well.
                    let canvas_wheel_routed_exclusively =
                        matches!(&event, WindowEvent::MouseWheel { .. })
                            && should_zoom_canvas(
                                app.main_view,
                                app.pointer_over_canvas(),
                                app.modifiers.control_key(),
                            );
                    let (egui_consumed, egui_repaint) = if canvas_wheel_routed_exclusively {
                        (false, false)
                    } else {
                        let response = app.egui_state.on_window_event(&app.window, &event);
                        (response.consumed, response.repaint)
                    };
                    let egui_repaint_requested = egui_repaint
                        && !matches!(&event, WindowEvent::RedrawRequested)
                        // Interactive drag input is coalesced by pending_drag_position;
                        // avoid turning every CursorMoved event into another frame request.
                        && !app.interactive_drag_active();
                    if egui_repaint_requested {
                        app.request_redraw();
                    }
                    match event {
                        WindowEvent::CloseRequested => app.request_window_close(),
                        WindowEvent::Resized(size) => {
                            app.resize(size);
                            app.request_redraw();
                        }
                        WindowEvent::RedrawRequested => {
                            let mut cancel_redraw = app.begin_cancel_redraw();
                            let deselect_redraw = app.begin_deselect_redraw();
                            let flush_started = Instant::now();
                            app.flush_drag_preview();
                            if let Some(redraw) = cancel_redraw.as_mut() {
                                redraw.flush_drag_preview = flush_started.elapsed();
                            }
                            app.process_pending_waypoint();
                            match app.render(cancel_redraw, deselect_redraw) {
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
                            app.request_redraw();
                        }
                        WindowEvent::KeyboardInput { event, .. }
                            if event.state == ElementState::Pressed && !event.repeat =>
                        {
                            if !egui_consumed && app.pending_document_action.is_none() {
                                match event.physical_key {
                                    PhysicalKey::Code(KeyCode::Escape)
                                        if app.pointer_interaction_active() =>
                                    {
                                        app.cancel_pointer_interaction();
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
                                                app.status_message =
                                                    Some("没有待保存的修改".to_owned());
                                                eprintln!("modelica-wgpu: nothing to save");
                                            }
                                            Ok(saved) => {
                                                app.status_message = Some(format!(
                                                    "保存成功：已写入 {saved} 个文件"
                                                ));
                                                eprintln!(
                                                    "modelica-wgpu: saved {saved} edited file(s) to disk"
                                                );
                                                app.load_error = None;
                                            }
                                            Err(error) => {
                                                app.status_message = Some(
                                                    if error.contains("save partially completed") {
                                                        "保存部分成功：仍有修改未写入".to_owned()
                                                    } else {
                                                        "保存失败：当前修改已保留".to_owned()
                                                    },
                                                );
                                                app.load_error = Some(error);
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            app.request_redraw();
                        }
                        WindowEvent::CursorMoved { position, .. } => {
                            app.cursor = position;
                            if app.interactive_drag_active() {
                                // Coalesce high-rate mouse input: one preview update
                                // per redraw, never one GPU update per OS event.
                                app.drag_profile.record_event();
                                if app.connection_creation_active() {
                                    app.connection_creation_profile.record_event();
                                }
                                let request_redraw = app.pending_drag_position.is_none();
                                app.pending_drag_position = Some((position, Instant::now()));
                                if request_redraw {
                                    app.request_redraw();
                                }
                            } else {
                                let hover_changed = app.update_hovered_diagram_port();
                                let drag_changed = app.update_model_drag_preview(position);
                                if egui_consumed || hover_changed || drag_changed {
                                    app.request_redraw();
                                }
                            }
                        }
                        WindowEvent::MouseInput { state, button, .. } => {
                            if app.pending_document_action.is_some() {
                                app.request_redraw();
                                return;
                            }
                            // A connection-creation click must not clone the
                            // current selection before entering its hot path.
                            let selection_before = (!app.connection_creation_active())
                                .then(|| app.diagram_selection.clone());
                            let inert_pointer_release =
                                if state == ElementState::Released {
                                    let suppress = app.suppress_next_pointer_release_redraw;
                                    app.suppress_next_pointer_release_redraw = false;
                                    suppress
                                } else {
                                    false
                                };
                            let mut interaction_changed = false;
                            if state == ElementState::Released {
                                if button == MouseButton::Left
                                    && app.interactive_drag_active()
                                    && !app.connection_creation_active()
                                {
                                    // The last CursorMoved may still be coalesced. Flush it before
                                    // taking the route that the user actually saw on screen.
                                    app.flush_drag_preview();
                                }
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
                            } else if button == MouseButton::Right
                                && app.pointer_interaction_active()
                            {
                                interaction_changed = app.cancel_pointer_interaction();
                            } else if app.connection_creation_active() {
                                if button == MouseButton::Left {
                                    // Keep input handling tiny. The latest
                                    // cursor/snap state and preview metadata are
                                    // consumed together by RedrawRequested.
                                    app.pending_waypoint = true;
                                    app.pending_waypoint_queued_at = Some(Instant::now());
                                    app.request_redraw();
                                }
                            } else if app.canvas_event_allowed() {
                                if button == MouseButton::Right {
                                    interaction_changed = app.clear_diagram_selection();
                                    app.suppress_next_pointer_release_redraw = true;
                                } else if wants_pan(button, app.modifiers.control_key()) {
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
                                app.suppress_next_pointer_release_redraw = true;
                            }
                            let selection_changed = selection_before
                                .is_some_and(|selection_before| app.diagram_selection != selection_before);
                            if state == ElementState::Pressed
                                && button == MouseButton::Left
                                && !app.pointer_interaction_active()
                                && selection_changed
                            {
                                app.suppress_next_pointer_release_redraw = true;
                            }
                            if egui_consumed
                                || (state == ElementState::Released && !inert_pointer_release)
                                || selection_changed
                                || interaction_changed
                            {
                                app.request_redraw();
                            }
                        }
                        WindowEvent::MouseWheel { delta, .. } => {
                            let wheel_sample = wheel_delta_sample(delta);
                            let amount = wheel_sample.delta_y;
                            let source_line_scroll = wheel_sample.kind == SourceWheelKind::LineDelta
                                && !app.modifiers.control_key()
                                && app.pointer_over_source_scroll();
                            if source_line_scroll {
                                app.enqueue_source_line_scroll(amount);
                                app.request_redraw();
                            } else {
                                if app.main_view == MainView::Source
                                    && app.pointer_over_source_scroll()
                                {
                                    app.source_wheel_sample = Some(SourceWheelSample {
                                        kind: wheel_sample.kind,
                                        delta_y: wheel_sample.delta_y,
                                    });
                                }
                                let owner = wheel_owner(
                                    app.main_view,
                                    egui_consumed,
                                    app.pointer_over_canvas(),
                                    app.modifiers.control_key(),
                                );
                                let zoom_before = app.zoom;
                                if owner == WheelOwner::CanvasZoom {
                                    app.zoom_at_cursor(amount);
                                    app.request_redraw();
                                }
                                let egui_consumed_trace = if canvas_wheel_routed_exclusively {
                                    "not-dispatched"
                                } else if egui_consumed {
                                    "true"
                                } else {
                                    "false"
                                };
                                trace_canvas_wheel(
                                    app.main_view,
                                    app.modifiers.control_key(),
                                    app.cursor,
                                    app.window.scale_factor() as f32,
                                    app.canvas_rect,
                                    egui_consumed_trace,
                                    owner,
                                    wheel_sample,
                                    zoom_before,
                                    app.zoom,
                                );
                                trace_source_wheel(
                                    matches!(app.main_view, MainView::Source),
                                    amount,
                                    egui_consumed,
                                    egui_repaint,
                                    egui_repaint_requested,
                                    owner,
                                );
                            }
                        }
                        _ => {}
                    }
                    if app.exit_requested {
                        event_loop.exit();
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

    fn relative_luminance(color: Color32) -> f32 {
        let linearize = |channel: u8| {
            let value = f32::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };

        0.2126 * linearize(color.r())
            + 0.7152 * linearize(color.g())
            + 0.0722 * linearize(color.b())
    }

    fn contrast_ratio(foreground: Color32, background: Color32) -> f32 {
        let foreground_luminance = relative_luminance(foreground);
        let background_luminance = relative_luminance(background);
        (foreground_luminance.max(background_luminance) + 0.05)
            / (foreground_luminance.min(background_luminance) + 0.05)
    }

    #[test]
    fn tertiary_text_contrast_is_readable_but_remains_secondary() {
        let light_surface = theme_rgb(255, 255, 255);
        let dark_surface = theme_rgb(34, 36, 43);
        let light_tertiary = theme_text_tertiary_for(false);
        let dark_tertiary = theme_text_tertiary_for(true);
        let light_secondary = theme_rgb(95, 99, 104);
        let dark_secondary = theme_rgb(169, 171, 182);

        let light_contrast = contrast_ratio(light_tertiary, light_surface);
        let dark_contrast = contrast_ratio(dark_tertiary, dark_surface);
        assert!(
            light_contrast >= 4.5,
            "light tertiary contrast: {light_contrast:.2}:1"
        );
        assert!(
            dark_contrast >= 4.5,
            "dark tertiary contrast: {dark_contrast:.2}:1"
        );
        assert!(contrast_ratio(light_secondary, light_surface) > light_contrast);
        assert!(contrast_ratio(dark_secondary, dark_surface) > dark_contrast);
        assert_eq!(light_tertiary.a(), 255);
        assert_eq!(dark_tertiary.a(), 255);
    }

    fn text_context(instance_name: &str, class_name: &str) -> ModelTextContext {
        ModelTextContext::new(
            format!("Demo.{class_name}"),
            class_name,
            instance_name,
            HashMap::new(),
            HashMap::new(),
        )
    }

    #[test]
    fn tree_row_keeps_descriptions_in_metadata_but_not_visible_text() {
        let nodes = [
            TreeNode {
                name: "Heater".to_owned(),
                qualified_name: "Demo.Heater".to_owned(),
                class_name: Some("Demo.Heater".to_owned()),
                kind: Some(ClassKind::Block),
                description: Some("一等温吸附干燥器（很长的中文说明）".to_owned()),
                children: Vec::new(),
            },
            TreeNode {
                name: "Nested".to_owned(),
                qualified_name: "Demo.Parent.Nested".to_owned(),
                class_name: Some("Demo.Parent.Nested".to_owned()),
                kind: Some(ClassKind::Model),
                description: None,
                children: Vec::new(),
            },
        ];

        for node in nodes {
            let visible_text = tree_row_label(tree_node_icon(node.kind), &node.name);
            assert_eq!(
                visible_text,
                format!("{}  {}", tree_node_icon(node.kind), node.name)
            );
            assert!(!visible_text.contains('—'));
            assert!(!visible_text.contains("干燥器"));
            if node.name == "Heater" {
                assert_eq!(
                    node.description.as_deref(),
                    Some("一等温吸附干燥器（很长的中文说明）")
                );
            }
        }
    }

    #[test]
    fn tree_rows_expose_kind_without_description_text() {
        assert_eq!(tree_node_kind_label(Some(ClassKind::Block)), "block");
        assert_eq!(
            tree_node_kind_label(Some(ClassKind::Connector)),
            "connector"
        );
        assert_eq!(tree_node_kind_label(Some(ClassKind::Package)), "package");
        assert_eq!(tree_node_kind_label(None), "");

        let visible_text = tree_row_label("□", "Heater");
        assert_eq!(visible_text, "□  Heater");
        assert!(!visible_text.contains("等温"));
    }

    #[test]
    fn tree_marker_name_and_kind_positions_snap_at_common_dpi_scales() {
        let rect = Rect::from_min_size(Pos2::new(13.25, 29.4), Vec2::new(260.0, 29.0));
        let original_rect = rect;
        let cases = [(0.0, 13.0, 17.3, 14.1), (3.0, 13.4, 17.6, 14.4)];

        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            for (indent, marker_height, label_height, kind_height) in cases {
                let marker_x = rect.left() + indent * 12.0 + 8.0;
                let marker =
                    tree_row_galley_position(rect, marker_x, marker_height, pixels_per_point);

                let label_x = rect.left() + indent * 12.0 + 30.0;
                let label_anchor = snap_point_to_physical_pixel(
                    Pos2::new(label_x, rect.center().y),
                    pixels_per_point,
                );
                let label =
                    tree_row_galley_position(rect, label_anchor.x, label_height, pixels_per_point);

                let kind_x = rect.right() - 8.0 - 31.0;
                let kind = tree_row_galley_position(rect, kind_x, kind_height, pixels_per_point);

                for (position, expected_x) in [(marker, marker_x), (label, label_x), (kind, kind_x)]
                {
                    for coordinate in [position.x, position.y] {
                        let physical = coordinate * pixels_per_point;
                        assert!((physical - physical.round()).abs() < 0.0001);
                    }
                    assert!((position.x - expected_x).abs() <= 0.5001 / pixels_per_point);
                }
            }
        }

        assert_eq!(
            rect, original_rect,
            "paint snapping must not alter row geometry"
        );
    }

    #[test]
    fn font_roles_select_ui_mono_and_model_text_faces_explicitly() {
        assert_eq!(
            ui_font(12.0).family,
            FontFamily::Name(UI_FONT_MEDIUM.into())
        );
        assert_eq!(
            ui_semibold_font(12.0).family,
            FontFamily::Name(UI_FONT_SEMIBOLD.into())
        );
        assert_eq!(
            ui_mono_font(12.0).family,
            FontFamily::Name(UI_FONT_MONO.into())
        );
        assert_eq!(
            model_text_font(12.0, None, false, false).family,
            FontFamily::Name(UI_FONT_MEDIUM.into())
        );
        assert_eq!(
            model_text_font(12.0, None, true, false).family,
            FontFamily::Name(UI_FONT_SEMIBOLD.into())
        );
        assert_eq!(
            model_text_font(12.0, None, false, true).family,
            FontFamily::Name(UI_FONT_ITALIC.into())
        );
        assert_eq!(
            model_text_font(12.0, None, true, true).family,
            FontFamily::Name(UI_FONT_SEMIBOLD_ITALIC.into())
        );
        assert_eq!(
            model_text_font(12.0, Some("monospace"), true, true).family,
            FontFamily::Name(UI_FONT_MONO.into())
        );
    }

    #[test]
    fn font_fallback_chains_prefer_cjk_and_symbols_before_generic_faces() {
        let chain = build_font_fallback_chain(
            &["seguisb".to_owned()],
            &[Some("modelica-cjk"), Some(UI_FONT_SYMBOLS), Some("segoeui")],
            &["egui-default".to_owned(), "modelica-cjk".to_owned()],
        );

        assert_eq!(
            chain,
            [
                "seguisb",
                "modelica-cjk",
                UI_FONT_SYMBOLS,
                "segoeui",
                "egui-default"
            ]
        );

        let missing_semibold_chain = build_font_fallback_chain(
            &[],
            &[Some("modelica-cjk"), Some(UI_FONT_SYMBOLS), Some("segoeui")],
            &["egui-default".to_owned()],
        );
        assert_eq!(
            missing_semibold_chain,
            ["modelica-cjk", UI_FONT_SYMBOLS, "segoeui", "egui-default"]
        );

        let mono_chain = build_font_fallback_chain(
            &["Hack".to_owned()],
            &[Some("modelica-cjk"), Some(UI_FONT_SYMBOLS), Some("segoeui")],
            &[],
        );
        assert_eq!(
            mono_chain,
            ["Hack", "modelica-cjk", UI_FONT_SYMBOLS, "segoeui"]
        );
    }

    #[test]
    fn source_token_roles_match_electron_highlighting_categories() {
        let tokens = tokenize(
            "model Demo\n  Real x = 1 + f(y);\n  annotation(Icon(graphics={Rectangle()})); // note",
        );
        let full_class_names = HashSet::from(["Demo".to_owned()]);
        let short_class_names = HashSet::from(["Demo".to_owned()]);
        let role_for = |text: &str| {
            let index = tokens
                .iter()
                .position(|token| token.text == text)
                .expect("token should be present");
            source_token_role(&tokens, index, &full_class_names, &short_class_names)
        };

        assert_eq!(role_for("model"), SourceTokenRole::Keyword);
        assert_eq!(role_for("Demo"), SourceTokenRole::Type);
        assert_eq!(role_for("Real"), SourceTokenRole::Builtin);
        assert_eq!(role_for("="), SourceTokenRole::Operator);
        assert_eq!(role_for("1"), SourceTokenRole::Number);
        assert_eq!(role_for("f"), SourceTokenRole::Function);
        assert_eq!(role_for("annotation"), SourceTokenRole::Annotation);
        assert_eq!(role_for("Icon"), SourceTokenRole::Annotation);
        assert_eq!(role_for("// note"), SourceTokenRole::Comment);
    }

    #[test]
    fn source_interaction_resets_selection_when_class_changes() {
        let mut interaction = SourceInteractionState {
            selected_class: Some("Demo.A".to_owned()),
            focused: true,
            all_selected: true,
            fold_state: SourceFoldState::default(),
        };
        interaction.sync_class(Some("Demo.B"));
        assert!(!interaction.focused);
        assert!(!interaction.all_selected);
        assert_eq!(interaction.selected_class.as_deref(), Some("Demo.B"));
    }

    #[test]
    fn source_copy_text_preserves_modelica_line_boundaries() {
        let lines = vec!["model Demo".to_owned(), "end Demo;".to_owned()];
        assert_eq!(source_copy_text(&lines), "model Demo\nend Demo;");
    }

    fn source_folding_lines(source: &str) -> Vec<String> {
        source.lines().map(str::to_owned).collect()
    }

    fn source_folding_document(source: &str, version: u64) -> UiDocument {
        let lines = source_folding_lines(source);
        UiDocument {
            package_name: "Demo".to_owned(),
            class_names: vec!["Demo".to_owned()],
            full_class_names: HashSet::new(),
            short_class_names: HashSet::new(),
            dirty: false,
            tree: TreeNode {
                name: "Demo".to_owned(),
                qualified_name: "Demo".to_owned(),
                class_name: Some("Demo".to_owned()),
                kind: Some(ClassKind::Model),
                description: None,
                children: Vec::new(),
            },
            selected_class: Some("Demo".to_owned()),
            icon_graphics: 0,
            diagram_background: 0,
            diagram_components: 0,
            diagram_own_components: 0,
            diagram_inherited_components: 0,
            diagram_connectors: 0,
            diagram_unresolved_components: 0,
            diagram_unresolved_bases: 0,
            diagram_connections: 0,
            source_name: "Demo".to_owned(),
            source_max_line_chars: lines.iter().map(String::len).max().unwrap_or_default(),
            source_lines: lines,
            source_version: version,
        }
    }

    fn source_folding_state(source: &str, version: u64) -> SourceFoldState {
        let document = source_folding_document(source, version);
        let mut state = SourceFoldState::default();
        state.sync_document(&document);
        state
    }

    #[test]
    fn multi_line_parentheses_fold() {
        let ranges = discover_source_fold_ranges(
            Some("Demo"),
            1,
            &source_folding_lines("annotation(\n  Icon(\n    graphics={}\n  )\n)"),
        );
        assert!(ranges.iter().any(|range| {
            range.kind == Some(SourceFoldKind::Annotation)
                && range.open_token == FoldDelimiter::Parenthesis
                && range.start_line == 0
                && range.end_line == 4
        }));
    }

    #[test]
    fn nested_parentheses_fold() {
        let ranges = discover_source_fold_ranges(
            Some("Demo"),
            1,
            &source_folding_lines(
                "annotation(\n  Icon(\n    Text(\n      textString=\"x\"\n    )\n  )\n)",
            ),
        );
        assert!(ranges
            .iter()
            .any(|range| range.start_line == 0 && range.end_line == 6));
        assert!(ranges
            .iter()
            .any(|range| range.start_line == 1 && range.end_line == 5));
        assert!(ranges
            .iter()
            .any(|range| range.start_line == 2 && range.end_line == 4));
    }

    #[test]
    fn brace_array_fold() {
        let ranges = discover_source_fold_ranges(
            Some("Demo"),
            1,
            &source_folding_lines("graphics={\n  Rectangle(),\n  Ellipse()\n}"),
        );
        assert!(ranges.iter().any(|range| {
            range.open_token == FoldDelimiter::Brace
                && range.kind == Some(SourceFoldKind::Array)
                && range.start_line == 0
                && range.end_line == 3
        }));
    }

    #[test]
    fn bracket_fold() {
        let ranges =
            discover_source_fold_ranges(Some("Demo"), 1, &source_folding_lines("values[\n  1\n]"));
        assert!(ranges.iter().any(|range| {
            range.open_token == FoldDelimiter::Bracket
                && range.start_line == 0
                && range.end_line == 2
        }));
    }

    #[test]
    fn comments_do_not_create_fold_ranges() {
        let lines = source_folding_lines("// fake(annotation( { [ ) } ])\nmodel Demo\nend Demo;");
        assert!(discover_source_fold_ranges(Some("Demo"), 1, &lines).is_empty());
    }

    #[test]
    fn strings_do_not_create_fold_ranges() {
        let lines = source_folding_lines(
            "model Demo\n  String text = \"fake annotation( { [ ) } ]\";\nend Demo;",
        );
        assert!(discover_source_fold_ranges(Some("Demo"), 1, &lines).is_empty());
    }

    #[test]
    fn nested_child_state_survives_parent_toggle() {
        let mut state = source_folding_state(
            "annotation(\n  Icon(\n    Text(\n      textString=\"x\"\n    )\n  )\n)",
            1,
        );
        let parent_id = state.range_starting_at(0).expect("parent fold").id.clone();
        let child_id = state.range_starting_at(1).expect("child fold").id.clone();
        state.collapsed.clear();
        assert!(state.toggle_line(1, 7));
        assert!(state.collapsed.contains(&child_id));
        assert!(state.toggle_line(0, 7));
        assert!(state.toggle_line(0, 7));
        assert!(state.collapsed.contains(&child_id));
        assert!(!state.collapsed.contains(&parent_id));
    }

    #[test]
    fn source_folds_are_collapsed_by_default() {
        let state = source_folding_state("annotation(\n  Icon(\n    graphics={}\n  )\n)", 1);
        assert!(!state.ranges.is_empty());
        assert_eq!(state.collapsed.len(), state.ranges.len());
        assert_eq!(state.visible_rows.rows.len(), 1);
    }

    #[test]
    fn source_fold_marker_can_expand_and_collapse_from_its_line() {
        let mut state = source_folding_state("annotation(\n  Icon()\n)", 1);
        let fold_id = state
            .range_starting_at(0)
            .expect("annotation fold")
            .id
            .clone();
        assert!(state.collapsed.contains(&fold_id));

        assert!(state.toggle_line(0, 3));
        assert!(!state.collapsed.contains(&fold_id));
        assert!(state.toggle_line(0, 3));
        assert!(state.collapsed.contains(&fold_id));
    }

    #[test]
    fn source_fold_marker_stays_aligned_inside_its_click_gutter_at_common_dpi_scales() {
        let row = Rect::from_min_size(Pos2::new(31.25, 47.4), Vec2::new(240.0, SOURCE_ROW_HEIGHT));
        let fold_rect = Rect::from_min_size(
            row.min,
            Vec2::new(SOURCE_FOLD_GUTTER_WIDTH, SOURCE_ROW_HEIGHT),
        );
        assert_eq!(fold_rect.width(), 18.0);
        assert_eq!(fold_rect.height(), SOURCE_ROW_HEIGHT);

        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            let marker_size = Vec2::new(8.0, 14.0);
            let marker = source_fold_marker_position(fold_rect, marker_size, pixels_per_point);
            let marker_center = marker + marker_size * 0.5;
            assert!(fold_rect.contains(marker_center));
            assert!((marker_center.x - fold_rect.center().x).abs() <= 0.5001 / pixels_per_point);
            assert!((marker_center.y - fold_rect.center().y).abs() <= 0.5001 / pixels_per_point);
        }
    }

    #[test]
    fn source_click_policy_prioritizes_fold_then_viewport_and_ignores_outside_clicks() {
        let viewport = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(300.0, 200.0));
        let fold_gutter = Rect::from_min_size(viewport.min, Vec2::new(18.0, SOURCE_ROW_HEIGHT));
        assert_eq!(
            source_click_owner(viewport, Some(fold_gutter.center()), Some(fold_gutter)),
            SourceClickOwner::Fold
        );
        assert_eq!(
            source_click_owner(
                viewport,
                Some(Pos2::new(viewport.left() + 120.0, viewport.top() + 10.0)),
                Some(fold_gutter),
            ),
            SourceClickOwner::Viewport
        );
        assert_eq!(
            source_click_owner(
                viewport,
                Some(Pos2::new(viewport.right() + 1.0, viewport.top() + 10.0)),
                Some(fold_gutter),
            ),
            SourceClickOwner::None
        );
        assert_eq!(
            source_click_owner(viewport, None, Some(fold_gutter)),
            SourceClickOwner::None
        );
    }

    #[test]
    fn fold_click_does_not_focus_source_or_clear_selection_but_body_click_does() {
        let mut interaction = SourceInteractionState {
            focused: false,
            all_selected: true,
            ..Default::default()
        };
        interaction.focus_for_click_owner(SourceClickOwner::Fold);
        assert!(!interaction.focused);
        assert!(interaction.all_selected);

        interaction.focus_for_click_owner(SourceClickOwner::Viewport);
        assert!(interaction.focused);
        assert!(!interaction.all_selected);
    }

    #[test]
    fn source_fold_layout_is_cached_during_scroll_frames() {
        let document = source_folding_document("annotation(\n  Icon(\n    graphics={}\n  )\n)", 1);
        let mut state = SourceFoldState::default();
        state.sync_document(&document);
        let rebuild_count = state.fold_layout_rebuild_count;
        let visible_row_count = state.visible_rows.rows.len();
        for _ in 0..100 {
            state.sync_document(&document);
        }
        assert_eq!(state.fold_layout_rebuild_count, rebuild_count);
        assert_eq!(state.visible_rows.rows.len(), visible_row_count);
        assert!(state.range_by_start_line.contains_key(&0));
        assert!(!state.range_by_id.is_empty());
    }

    #[test]
    fn fold_does_not_change_source() {
        let source = "annotation(\n  Icon(\n    graphics={}\n  )\n)";
        let lines = source_folding_lines(source);
        let before = lines.clone();
        let mut state = source_folding_state(source, 1);
        assert!(state.toggle_line(0, lines.len()));
        let _ = state.visible_rows.rows.len();
        assert_eq!(lines, before);
    }

    #[test]
    fn fold_does_not_set_dirty() {
        let dirty = false;
        let mut state = source_folding_state("annotation(\nx\n)", 1);
        state.toggle_line(0, 3);
        assert!(!dirty);
    }

    #[test]
    fn fold_does_not_touch_undo_redo() {
        let undo = vec!["move".to_owned()];
        let redo = vec!["resize".to_owned()];
        let mut state = source_folding_state("annotation(\nx\n)", 1);
        state.toggle_line(0, 3);
        assert_eq!(undo, vec!["move"]);
        assert_eq!(redo, vec!["resize"]);
    }

    #[test]
    fn source_version_rebuilds_fold_ranges_safely() {
        let mut state = SourceFoldState::default();
        let first_document = source_folding_document("annotation(\nx\n)", 1);
        state.sync_document(&first_document);
        let old_id = state.range_starting_at(0).expect("old fold").id.clone();
        assert!(state.collapsed.contains(&old_id));

        let second_document = source_folding_document("model Demo\nend Demo;", 2);
        state.sync_document(&second_document);
        assert!(state.collapsed.is_empty());
        assert!(state.ranges.iter().all(|range| range.end_line < 2));
    }

    #[test]
    fn visible_rows_keep_original_line_numbers() {
        let source = "model Demo\nannotation(\n  Icon()\n)\nend Demo;";
        let mut state = source_folding_state(source, 1);
        state.collapsed.clear();
        assert!(state.toggle_line(1, 5));
        let rows = &state.visible_rows;
        assert_eq!(
            rows.rows
                .iter()
                .map(|row| row.original_line)
                .collect::<Vec<_>>(),
            vec![0, 1, 4]
        );
        assert!(rows.rows[1].fold_range_index.is_some());
    }

    #[test]
    fn source_folding_5000_lines_keeps_show_rows_virtualization() {
        let mut lines = vec!["annotation(".to_owned()];
        lines.extend((0..4_998).map(|_| "  nested = (1 + 2);".to_owned()));
        lines.push(")".to_owned());
        assert_eq!(lines.len(), 5_000);
        let mut state = SourceFoldState::default();
        let document = UiDocument {
            package_name: String::new(),
            class_names: Vec::new(),
            full_class_names: HashSet::new(),
            short_class_names: HashSet::new(),
            dirty: false,
            tree: TreeNode {
                name: "Demo".to_owned(),
                qualified_name: "Demo".to_owned(),
                class_name: Some("Demo".to_owned()),
                kind: Some(ClassKind::Model),
                description: None,
                children: Vec::new(),
            },
            selected_class: Some("Demo".to_owned()),
            icon_graphics: 0,
            diagram_background: 0,
            diagram_components: 0,
            diagram_own_components: 0,
            diagram_inherited_components: 0,
            diagram_connectors: 0,
            diagram_unresolved_components: 0,
            diagram_unresolved_bases: 0,
            diagram_connections: 0,
            source_name: "Demo".to_owned(),
            source_max_line_chars: 32,
            source_lines: lines,
            source_version: 1,
        };
        state.sync_document(&document);
        let rows = &state.visible_rows;
        assert_eq!(rows.rows.len(), 1);
        assert_eq!(rows.rows[0].original_line, 0);
    }

    fn sample_create_command() -> EditCommand {
        EditCommand::CreateDiagramConnection {
            class_name: "Test".to_owned(),
            connection_key: ConnectionKey {
                owner_class: "Test".to_owned(),
                lhs: ConnectorRef::parse("a.x"),
                rhs: ConnectorRef::parse("b.y"),
                occurrence: 0,
            },
            before_source: "model Test end Test;".to_owned(),
            after_source: "model Test equation connect(a.x, b.y); end Test;".to_owned(),
        }
    }

    #[test]
    fn successful_new_edit_records_and_clears_redo_history() {
        let mut history = vec![sample_create_command()];
        let mut redo_history = vec![sample_create_command()];
        record_successful_edit(&mut history, &mut redo_history, sample_create_command());
        assert_eq!(history.len(), 2);
        assert!(redo_history.is_empty());
    }

    #[test]
    fn switching_documents_resets_both_history_stacks() {
        let mut history = vec![sample_create_command()];
        let mut redo_history = vec![sample_create_command()];
        reset_edit_history(&mut history, &mut redo_history);
        assert!(history.is_empty());
        assert!(redo_history.is_empty());
    }

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
            dimensions: Vec::new(),
            resolved_type_qualified_name: Some("Test.Port".to_owned()),
            model_text_context: ModelTextContext::default(),
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
            subscripts: Vec::new(),
        };
        let rhs = ConnectorRef {
            component_name: "b".to_owned(),
            connector_path: String::new(),
            subscripts: Vec::new(),
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
    fn loaded_document_resolves_scenes_lazily_and_reuses_cached_results() {
        let directory = std::env::temp_dir().join(format!(
            "modelica-wgpu-lazy-load-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("LazyLibrary.mo");
        fs::write(&path, "model A\nend A;\nmodel B\nend B;\n").expect("write test library");

        let mut document = LoadedDocument::load(&path).expect("load test library");
        let initial = document.scene_resolution_stats();
        assert_eq!(initial.icon_resolve_count, 0);
        assert_eq!(initial.diagram_resolve_count, 0);
        assert!(document.class_names.iter().any(|name| name == "A"));
        assert!(document.class_names.iter().any(|name| name == "B"));
        assert!(!document.icon_cache_hit("A"));
        assert!(!document.diagram_cache_hit("A"));

        assert!(document.icon("A").is_some());
        assert!(document.diagram("A").is_some());
        let after_first_open = document.scene_resolution_stats();
        assert_eq!(after_first_open.icon_resolve_count, 1);
        assert_eq!(after_first_open.diagram_resolve_count, 1);
        assert!(document.icon_cache_hit("A"));
        assert!(document.diagram_cache_hit("A"));

        assert!(document.icon("A").is_some());
        assert!(document.diagram("A").is_some());
        assert_eq!(document.scene_resolution_stats(), after_first_open);

        assert!(document.icon("B").is_some());
        assert!(document.diagram("B").is_some());
        let after_second_class = document.scene_resolution_stats();
        assert_eq!(after_second_class.icon_resolve_count, 2);
        assert_eq!(after_second_class.diagram_resolve_count, 2);

        document.set_class_text("A", "model A\nend A;\n".to_owned());
        assert!(!document.icon_cache_hit("A"));
        assert!(!document.diagram_cache_hit("A"));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn route_rebuild_is_stable_after_cache_clear_and_reopen() {
        let directory = std::env::temp_dir().join(format!(
            "modelica-wgpu-route-rebuild-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&directory).expect("create route rebuild directory");
        let path = directory.join("RouteRebuild.mo");
        let source = r#"
connector Port
  annotation(Diagram(graphics={Ellipse(extent={{-4,-4},{4,4}})}));
end Port;
model Top
  Port a annotation(Placement(transformation(origin={0,20}, extent={{-10,-10},{10,10}})));
  Port b annotation(Placement(transformation(origin={100,-30}, extent={{-10,-10},{10,10}})));
equation
  connect(a, b) annotation(Line(points={{-800,400},{800,400}}));
end Top;
"#;
        fs::write(&path, source).expect("write route rebuild fixture");

        let mut document = LoadedDocument::load(&path).expect("load route rebuild fixture");
        let first_scene = document.diagram("Top").expect("initial diagram");
        let first_routes = first_scene
            .connections
            .iter()
            .map(|connection| canonical_connection_points(first_scene, connection))
            .collect::<Vec<_>>();
        let first_geometry = core_diagram_geometry(first_scene)
            .into_iter()
            .filter(|geometry| geometry.layer == DiagramRenderLayer::Connection)
            .map(|geometry| {
                geometry
                    .connection
                    .expect("connection metadata")
                    .line
                    .points
            })
            .collect::<Vec<_>>();

        document.invalidate_scene_caches();
        let rebuilt_scene = document.diagram("Top").expect("rebuilt diagram");
        let rebuilt_routes = rebuilt_scene
            .connections
            .iter()
            .map(|connection| canonical_connection_points(rebuilt_scene, connection))
            .collect::<Vec<_>>();
        let rebuilt_geometry = core_diagram_geometry(rebuilt_scene)
            .into_iter()
            .filter(|geometry| geometry.layer == DiagramRenderLayer::Connection)
            .map(|geometry| {
                geometry
                    .connection
                    .expect("connection metadata")
                    .line
                    .points
            })
            .collect::<Vec<_>>();
        assert_eq!(rebuilt_routes, first_routes);
        assert_eq!(rebuilt_geometry, first_geometry);

        let reopened = LoadedDocument::load(&path).expect("reopen route rebuild fixture");
        let reopened_scene = reopened.diagram("Top").expect("reopened diagram");
        let reopened_routes = reopened_scene
            .connections
            .iter()
            .map(|connection| canonical_connection_points(reopened_scene, connection))
            .collect::<Vec<_>>();
        let reopened_geometry = core_diagram_geometry(reopened_scene)
            .into_iter()
            .filter(|geometry| geometry.layer == DiagramRenderLayer::Connection)
            .map(|geometry| {
                geometry
                    .connection
                    .expect("connection metadata")
                    .line
                    .points
            })
            .collect::<Vec<_>>();
        assert_eq!(reopened_routes, first_routes);
        assert_eq!(reopened_geometry, first_geometry);
        let _ = fs::remove_dir_all(directory);
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
    fn source_has_no_canvas_zoom_controls_and_canvas_views_keep_them() {
        assert!(!canvas_zoom_controls_visible_for(MainView::Source));
        assert!(canvas_zoom_controls_visible_for(MainView::Icon));
        assert!(canvas_zoom_controls_visible_for(MainView::Diagram));
        assert!(!should_fit_scene_after_view_change(
            MainView::Source,
            MainView::Icon
        ));
        assert!(!should_fit_scene_after_view_change(
            MainView::Source,
            MainView::Diagram
        ));
        assert!(!should_fit_scene_after_view_change(
            MainView::Icon,
            MainView::Source
        ));
        assert!(should_fit_scene_after_view_change(
            MainView::Icon,
            MainView::Diagram
        ));
    }

    #[test]
    fn toolbar_zoom_in_and_out_use_a_clamped_ten_percent_step() {
        assert!((zoom_after_toolbar_action(INITIAL_ZOOM, ZoomAction::In) - 3.3).abs() < 0.0001);
        assert!(
            (zoom_after_toolbar_action(INITIAL_ZOOM, ZoomAction::Out) - 3.0 / 1.1).abs() < 0.0001
        );
        assert_eq!(
            zoom_after_toolbar_action(MAX_ZOOM, ZoomAction::In),
            MAX_ZOOM
        );
        assert_eq!(
            zoom_after_toolbar_action(MIN_ZOOM, ZoomAction::Out),
            MIN_ZOOM
        );
    }

    #[test]
    fn zoom_percent_tracks_zoom_relative_to_initial_scale() {
        assert_eq!(zoom_percent(INITIAL_ZOOM), 100);
        assert_eq!(zoom_percent(INITIAL_ZOOM * 0.5), 50);
        assert_eq!(zoom_percent(INITIAL_ZOOM * 0.75), 75);
        assert_eq!(zoom_percent(INITIAL_ZOOM * 1.1), 110);
        assert_eq!(zoom_percent(INITIAL_ZOOM * 2.0), 200);
    }

    #[test]
    fn toolbar_zoom_anchor_uses_physical_canvas_center_and_preserves_world_point() {
        let canvas_rect = Rect::from_min_size(Pos2::new(110.0, 80.0), Vec2::new(500.0, 320.0));
        let anchor = canvas_center_physical_anchor(Some(canvas_rect), 1.5, [1800, 1200]);
        assert_eq!(anchor, [540.0, 360.0]);

        let pan = [37.0, -19.0];
        let viewport_center = [900.0, 600.0];
        let old_zoom = INITIAL_ZOOM;
        let new_zoom = zoom_after_toolbar_action(old_zoom, ZoomAction::In);
        let next_pan = pan_after_zoom_at_anchor(pan, old_zoom, new_zoom, viewport_center, anchor);
        let world_before = [
            (anchor[0] - viewport_center[0] - pan[0]) / old_zoom,
            (anchor[1] - viewport_center[1] - pan[1]) / old_zoom,
        ];
        let world_after = [
            (anchor[0] - viewport_center[0] - next_pan[0]) / new_zoom,
            (anchor[1] - viewport_center[1] - next_pan[1]) / new_zoom,
        ];
        assert!((world_before[0] - world_after[0]).abs() < 0.0001);
        assert!((world_before[1] - world_after[1]).abs() < 0.0001);
    }

    #[test]
    fn canvas_wheel_zoom_preserves_the_cursor_anchor() {
        let cursor = [375.0, 240.0];
        let pan = [-28.0, 14.0];
        let viewport_center = [800.0, 500.0];
        let old_zoom = INITIAL_ZOOM;
        let new_zoom = zoom_after_wheel(old_zoom, 2.0);
        let next_pan = pan_after_zoom_at_anchor(pan, old_zoom, new_zoom, viewport_center, cursor);
        let world_before = [
            (cursor[0] - viewport_center[0] - pan[0]) / old_zoom,
            (cursor[1] - viewport_center[1] - pan[1]) / old_zoom,
        ];
        let world_after = [
            (cursor[0] - viewport_center[0] - next_pan[0]) / new_zoom,
            (cursor[1] - viewport_center[1] - next_pan[1]) / new_zoom,
        ];
        assert!((world_before[0] - world_after[0]).abs() < 0.0001);
        assert!((world_before[1] - world_after[1]).abs() < 0.0001);
    }

    #[test]
    fn canvas_ctrl_wheel_has_priority_inside_icon_and_diagram_canvases() {
        for view in [MainView::Icon, MainView::Diagram] {
            assert_eq!(
                wheel_owner(view, true, true, true),
                WheelOwner::CanvasZoom,
                "{view:?} Ctrl+wheel should override generic egui consumption",
            );
            assert_eq!(
                wheel_owner(view, false, true, true),
                WheelOwner::CanvasZoom,
                "{view:?} Ctrl+wheel should zoom even when egui did not consume it",
            );
        }
    }

    #[test]
    fn wheel_ownership_matrix_keeps_source_and_non_canvas_wheels_out_of_canvas_zoom() {
        assert_eq!(
            wheel_owner(MainView::Source, true, true, true),
            WheelOwner::Egui
        );
        assert_eq!(
            wheel_owner(MainView::Source, false, true, true),
            WheelOwner::None
        );
        for view in [MainView::Icon, MainView::Diagram] {
            assert_eq!(wheel_owner(view, true, true, false), WheelOwner::Egui);
            assert_eq!(wheel_owner(view, true, false, true), WheelOwner::Egui);
            assert_eq!(wheel_owner(view, false, false, true), WheelOwner::None);
            assert_eq!(wheel_owner(view, false, true, false), WheelOwner::None);
        }
        assert_eq!(
            wheel_owner(MainView::Source, true, true, false),
            WheelOwner::Egui
        );
        assert_eq!(
            wheel_owner(MainView::Source, false, true, false),
            WheelOwner::None
        );
    }

    #[test]
    fn line_and_pixel_wheel_deltas_keep_up_and_down_directions() {
        let line_up = wheel_delta_sample(MouseScrollDelta::LineDelta(0.0, 1.0));
        let line_down = wheel_delta_sample(MouseScrollDelta::LineDelta(0.0, -1.0));
        let pixel_up = wheel_delta_sample(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            0.0, 80.0,
        )));
        let pixel_down = wheel_delta_sample(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            0.0, -80.0,
        )));

        assert_eq!(line_up.kind, SourceWheelKind::LineDelta);
        assert_eq!(line_up.delta_y, 1.0);
        assert_eq!(line_down.delta_y, -1.0);
        assert_eq!(pixel_up.kind, SourceWheelKind::PixelDelta);
        assert_eq!(pixel_up.delta_y, 1.0);
        assert_eq!(pixel_down.delta_y, -1.0);
        for sample in [line_up, pixel_up] {
            assert!(zoom_after_wheel(1.0, sample.delta_y) > 1.0);
        }
        for sample in [line_down, pixel_down] {
            assert!(zoom_after_wheel(1.0, sample.delta_y) < 1.0);
        }
    }

    #[test]
    fn physical_cursor_maps_to_same_canvas_point_at_common_dpi_scales() {
        for scale_factor in [1.0, 1.25, 1.5, 2.0] {
            let physical = PhysicalPosition::new(
                f64::from(240.0 * scale_factor),
                f64::from(120.0 * scale_factor),
            );
            let logical = physical_to_logical_position(physical, scale_factor);
            assert!((logical.x - 240.0).abs() < 0.001);
            assert!((logical.y - 120.0).abs() < 0.001);
        }
    }

    #[test]
    fn text_layout_diagnostic_reports_unrounded_physical_coordinates() {
        let logical = Pos2::new(10.25, 11.5);
        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            let physical = logical_to_physical_pixels(logical, pixels_per_point);
            assert!((physical.x - logical.x * pixels_per_point).abs() < f32::EPSILON);
            assert!((physical.y - logical.y * pixels_per_point).abs() < f32::EPSILON);
        }
        let fractional = logical_to_physical_pixels(logical, 1.25);
        assert!((fractional.x - 12.8125).abs() < f32::EPSILON);
        assert!((fractional.y - 14.375).abs() < f32::EPSILON);
    }

    #[test]
    fn text_positions_snap_to_physical_pixels_at_common_dpi_scales() {
        let logical = Pos2::new(10.25, 11.5);
        let expected = [
            (1.0, Pos2::new(10.0, 12.0)),
            (1.25, Pos2::new(10.4, 11.2)),
            (1.5, Pos2::new(10.0, 17.0 / 1.5)),
            (2.0, Pos2::new(10.5, 11.5)),
        ];

        for (pixels_per_point, expected_position) in expected {
            let snapped = snap_point_to_physical_pixel(logical, pixels_per_point);
            assert!((snapped.x - expected_position.x).abs() < 0.0001);
            assert!((snapped.y - expected_position.y).abs() < 0.0001);
            assert!(
                (snapped.x * pixels_per_point - (snapped.x * pixels_per_point).round()).abs()
                    < 0.0001
            );
            assert!(
                (snapped.y * pixels_per_point - (snapped.y * pixels_per_point).round()).abs()
                    < 0.0001
            );

            let snapped_y = snap_y_to_physical_pixel(logical, pixels_per_point);
            assert_eq!(snapped_y.x, logical.x, "X must remain unchanged");
            assert!((snapped_y.y - expected_position.y).abs() < 0.0001);
        }
    }

    #[test]
    fn source_line_number_and_body_positions_snap_y_without_changing_x() {
        let rect = Rect::from_min_size(Pos2::new(17.25, 23.4), Vec2::new(640.0, SOURCE_ROW_HEIGHT));
        let x = 51.125;

        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            for galley_height in [14.5, 15.0] {
                let unsnapped_y = rect.top() + (SOURCE_ROW_HEIGHT - galley_height) * 0.5;
                let position =
                    source_line_galley_position(rect, x, galley_height, pixels_per_point);

                assert_eq!(position.x, x);
                let physical_y = position.y * pixels_per_point;
                assert!((physical_y - physical_y.round()).abs() < 0.0001);
                assert!((position.y - unsnapped_y).abs() <= 0.5001 / pixels_per_point);
            }
        }
    }

    #[test]
    fn source_line_delta_uses_row_sized_target_and_clamps() {
        let mut scroll = SourceScrollState::new();
        scroll.initialized = true;
        scroll.current_y = 100.0;
        scroll.target_y = 100.0;
        scroll.set_bounds(180.0);

        scroll.enqueue_line_delta(1.0);
        assert_eq!(scroll.target_y, 45.0);
        assert!(scroll.active);
        assert!(scroll.override_default_wheel);

        scroll.enqueue_line_delta(-10.0);
        assert_eq!(scroll.target_y, 180.0);
    }

    #[test]
    fn source_scroll_state_does_not_overshoot_bounds() {
        let mut scroll = SourceScrollState::new();
        scroll.initialized = true;
        scroll.current_y = 20.0;
        scroll.target_y = 20.0;
        scroll.set_bounds(40.0);
        scroll.enqueue_line_delta(10.0);
        scroll.advance_by(0.05, 1.25);

        assert!((0.0..=40.0).contains(&scroll.current_y));
        assert!((0.0..=40.0).contains(&scroll.target_y));
    }

    #[test]
    fn source_scroll_keeps_fractional_motion_then_snaps_when_settled() {
        let pixels_per_point = 1.25;
        let mut scroll = SourceScrollState::new();
        scroll.initialized = true;
        scroll.active = true;
        scroll.current_y = 10.13;
        scroll.target_y = 11.4;
        scroll.set_bounds(50.0);
        let target_y = scroll.target_y;

        assert!(!scroll.advance_by(0.01, pixels_per_point));
        assert!(scroll.active);
        assert!(
            (scroll.current_y * pixels_per_point - (scroll.current_y * pixels_per_point).round())
                .abs()
                > 0.01
        );

        let mut settled = false;
        for _ in 0..20 {
            if scroll.advance_by(0.1, pixels_per_point) {
                settled = true;
                break;
            }
        }

        assert!(settled, "scroll animation should reach its target");
        assert!(!scroll.active);
        assert_eq!(scroll.current_y, scroll.target_y);
        let physical_y = scroll.current_y * pixels_per_point;
        assert!((physical_y - physical_y.round()).abs() < 0.0001);
        assert!((scroll.current_y - target_y).abs() <= 0.5001 / pixels_per_point);
    }

    #[test]
    fn settled_source_scroll_snap_stays_within_physical_bounds() {
        let max_offset = 10.3;
        for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
            let offset =
                snap_scroll_offset_to_physical_pixel(max_offset, max_offset, pixels_per_point);
            let physical_offset = offset * pixels_per_point;
            assert!((physical_offset - physical_offset.round()).abs() < 0.0001);
            assert!((0.0..=max_offset).contains(&offset));

            assert_eq!(
                snap_scroll_offset_to_physical_pixel(-1.0, max_offset, pixels_per_point),
                0.0
            );
        }
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
    fn component_edit_supports_icon_transformation_placement() {
        let source = "model Parent\n  Child p annotation(Placement(iconTransformation(origin={20, 30}, extent={{-5, -6}, {5, 6}}, rotation=12)));\nend Parent;";
        let candidate = patch_component_origin(source, "p", CorePoint { x: 40.0, y: 50.0 }, 0)
            .expect("component source patch with iconTransformation");
        assert!(candidate.contains("origin={40, 50}"));
        assert!(candidate.contains("extent={{-5, -6}, {5, 6}}"));
        assert!(candidate.contains("rotation=12"));

        let edit = component_extent_edit(
            &candidate,
            "p",
            modelica_core::scene::Extent {
                p1: CorePoint { x: -8.0, y: -9.0 },
                p2: CorePoint { x: 8.0, y: 9.0 },
            },
        )
        .expect("iconTransformation extent edit");
        let updated = apply_validated_source_edits(&candidate, vec![edit], 0)
            .expect("iconTransformation extent source patch");
        assert!(updated.contains("extent={{-8, -9}, {8, 9}}"));
        assert!(updated.contains("origin={40, 50}"));
        assert!(updated.contains("rotation=12"));
    }

    #[test]
    fn component_edit_uses_declaration_name_before_parameter_modifiers() {
        let source = "model Parent\n  Modelica.Fluid.Machines.Pump pump(\n    redeclare package Medium = Modelica.Media.Interfaces.PartialMedium,\n    m_flow_nominal = 1,\n    dp_nominal = 100000)\n    annotation(Placement(transformation(origin={20, 30}, extent={{-5, -6}, {5, 6}}, rotation=12)));\nend Parent;";
        let candidate = patch_component_origin(source, "pump", CorePoint { x: 40.0, y: 50.0 }, 0)
            .expect("component source patch with modifiers");
        assert!(candidate.contains("origin={40, 50}"));
        assert!(candidate.contains("m_flow_nominal = 1"));
        assert!(candidate.contains("dp_nominal = 100000"));
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
            ConnectionEndpointConstraint::Semantic {
                lhs: points[0],
                rhs: points[1],
            },
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
    fn semantic_endpoint_reanchoring_preserves_route_axes() {
        let original = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 40.0 },
            CorePoint { x: 80.0, y: 40.0 },
            CorePoint { x: 80.0, y: 100.0 },
        ];
        let anchored = connection_drag_points_with_semantic_endpoints(
            &original,
            (
                CorePoint { x: 2.0, y: -3.0 },
                CorePoint { x: 86.0, y: 104.0 },
            ),
        );

        assert_eq!(
            anchored,
            vec![
                CorePoint { x: 2.0, y: -3.0 },
                CorePoint { x: 2.0, y: 40.0 },
                CorePoint { x: 86.0, y: 40.0 },
                CorePoint { x: 86.0, y: 104.0 },
            ]
        );
        assert!(is_orthogonal_polyline(&anchored));
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
                    subscripts: Vec::new(),
                },
                modelica_core::scene::ConnectorRef {
                    component_name: "b".to_owned(),
                    connector_path: "port".to_owned(),
                    subscripts: Vec::new(),
                },
                0,
            ),
            id: "connection:test".to_owned(),
            lhs: modelica_core::scene::ConnectorRef {
                component_name: "a".to_owned(),
                connector_path: "port".to_owned(),
                subscripts: Vec::new(),
            },
            rhs: modelica_core::scene::ConnectorRef {
                component_name: "b".to_owned(),
                connector_path: "port".to_owned(),
                subscripts: Vec::new(),
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
    fn canonical_connection_route_anchors_static_geometry_to_ports() {
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 30.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );

        let points = canonical_connection_points(&scene, &connection);
        assert_eq!(
            points,
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 0.0, y: 30.0 },
                CorePoint { x: 100.0, y: 30.0 },
            ]
        );

        let geometries = core_diagram_geometry(&scene);
        let geometry = geometries
            .iter()
            .find(|geometry| geometry.layer == DiagramRenderLayer::Connection)
            .expect("canonical connection geometry");
        assert_eq!(
            geometry
                .connection
                .as_ref()
                .expect("connection metadata")
                .line
                .points,
            points
        );
    }

    #[test]
    fn initial_render_repairs_stale_route_without_mutating_source() {
        let raw_points = vec![
            CorePoint { x: 0.0, y: 40.0 },
            CorePoint { x: 0.0, y: 400.0 },
            CorePoint { x: 100.0, y: 400.0 },
            CorePoint { x: 100.0, y: 40.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 40.0 },
            CorePoint { x: 100.0, y: 40.0 },
            raw_points.clone(),
        );

        let (route, fallback) = resolved_connection_display_route(&scene, &connection, &raw_points)
            .expect("stale route should be repaired for initial display");
        assert_eq!(fallback, ConnectionRouteFallback::CanonicalOrthogonal);
        assert_eq!(
            route,
            vec![
                CorePoint { x: 0.0, y: 40.0 },
                CorePoint { x: 100.0, y: 40.0 },
            ]
        );
        assert!(valid_interactive_connection_route(&route));
        assert_eq!(
            connection.line.as_ref().expect("source line").points,
            raw_points
        );

        let connection_geometry = core_diagram_geometry(&scene)
            .into_iter()
            .find(|geometry| geometry.layer == DiagramRenderLayer::Connection)
            .expect("initial connection geometry");
        assert_eq!(
            connection_geometry
                .connection
                .expect("connection metadata")
                .line
                .points,
            route
        );
    }

    #[test]
    fn multi_interface_inherited_scene_rebuilds_stale_routes_from_semantics() {
        let source = r#"
connector Port
  annotation(Diagram(graphics={Ellipse(extent={{-4,-4},{4,4}})}));
end Port;

partial model Base
  Port p1 annotation(Placement(transformation(origin={-150,0}, extent={{-10,-10},{10,10}})));
  Port p2 annotation(Placement(transformation(origin={-50,0}, extent={{-10,-10},{10,10}})));
  Port p3 annotation(Placement(transformation(origin={50,0}, extent={{-10,-10},{10,10}})));
  Port p4 annotation(Placement(transformation(origin={150,0}, extent={{-10,-10},{10,10}})));
equation
  connect(p1, p2) annotation(Line(points={{-150,0},{-50,0}}));
  connect(p2, p3) annotation(Line(points={{-50,0},{50,0}}));
  connect(p3, p4) annotation(Line(points={{50,0},{150,0}}));
end Base;

model Child
  extends Base;
  Port external annotation(Placement(transformation(origin={250,0}, extent={{-10,-10},{10,10}})));
equation
  connect(p4, external) annotation(Line(points={{150,0},{250,0}}));
end Child;
"#;
        let file = parse(source, "MultiInterface.mo").expect("parse multi-interface fixture");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("MultiInterface.mo", source)
            .expect("register multi-interface fixture");
        let child = file
            .classes
            .iter()
            .find(|class| class.name == "Child")
            .expect("Child class");
        let child_source = source
            .get(child.source_range.start..child.source_range.end)
            .expect("Child source range");
        let mut scene = resolve_diagram(child, source, &mut registry);

        assert_eq!(scene.components.len(), 5);
        assert_eq!(scene.connections.len(), 4);
        assert_eq!(
            scene
                .connections
                .iter()
                .filter(|connection| connection.key.owner_class == "Base")
                .count(),
            3
        );
        assert_eq!(
            scene
                .connections
                .iter()
                .filter(|connection| connection.key.owner_class == "Child")
                .count(),
            1
        );

        // Keep the source route deliberately far from every current endpoint.
        // This simulates a placement change that left the serialized Line
        // annotation behind while retaining connector identity.
        let stale_points = vec![
            CorePoint {
                x: -900.0,
                y: 700.0,
            },
            CorePoint { x: 900.0, y: 700.0 },
        ];
        for connection in &mut scene.connections {
            connection
                .line
                .as_mut()
                .expect("fixture connection line")
                .points = stale_points.clone();
        }

        for connection in &scene.connections {
            let (semantic_first, semantic_last) =
                strict_connection_points(&scene, connection).expect("semantic endpoints");
            let raw_points = connection
                .line
                .as_ref()
                .expect("fixture connection line")
                .points
                .clone();
            let (route, fallback) =
                resolved_connection_display_route(&scene, connection, &raw_points)
                    .expect("stale route should be repaired");
            assert_eq!(fallback, ConnectionRouteFallback::CanonicalOrthogonal);
            assert_eq!(route.first(), Some(&semantic_first));
            assert_eq!(route.last(), Some(&semantic_last));
            assert!(valid_interactive_connection_route(&route));
            assert!(displayed_connection_points_match_invariant(
                &scene, connection, &route
            ));

            if connection.key.owner_class == "Base" {
                assert!(
                    connection_source_editable_in_class(connection, "Child", child_source).is_err()
                );
            } else {
                connection_source_editable_in_class(connection, "Child", child_source)
                    .expect("Child-owned connection should be editable");
            }
        }

        let geometries = core_diagram_geometry(&scene);
        assert_eq!(
            geometries
                .iter()
                .filter(|geometry| geometry.layer == DiagramRenderLayer::Connection)
                .count(),
            4
        );
        assert!(scene.connections.iter().all(|connection| connection
            .line
            .as_ref()
            .expect("source line")
            .points
            == stale_points));
    }

    #[test]
    fn committed_connection_geometry_matches_initial_rebuild_route() {
        let raw_points = vec![
            CorePoint {
                x: -800.0,
                y: 400.0,
            },
            CorePoint { x: 800.0, y: 400.0 },
        ];
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 20.0 },
            CorePoint { x: 100.0, y: -30.0 },
            raw_points.clone(),
        );
        let (semantic_points, display_points) =
            connection_geometry_points(&scene, &connection, INITIAL_ZOOM)
                .expect("committed route must have display geometry");
        let rebuilt_geometry = core_diagram_geometry(&scene)
            .into_iter()
            .find(|geometry| geometry.layer == DiagramRenderLayer::Connection)
            .expect("rebuilt connection geometry");

        assert_eq!(
            semantic_points,
            canonical_connection_points(&scene, &connection)
        );
        assert_eq!(
            rebuilt_geometry
                .connection
                .expect("connection metadata")
                .line
                .points,
            display_points
        );
        assert_ne!(display_points, raw_points);
        assert!(valid_interactive_connection_route(&semantic_points));
    }

    #[test]
    fn endpoint_overdraw_enters_directional_connector_bounds() {
        let input_target = endpoint_overdraw_target(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            modelica_render::Bounds {
                x: 0.0,
                y: -50.0,
                width: 100.0,
                height: 100.0,
            },
            3.0,
        )
        .expect("RealInput ray should enter its triangle bounds");
        assert!(point_nearly_equal(
            input_target,
            CorePoint { x: 3.0, y: 0.0 }
        ));

        let output_target = endpoint_overdraw_target(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: -100.0, y: 0.0 },
            modelica_render::Bounds {
                x: -100.0,
                y: -50.0,
                width: 100.0,
                height: 100.0,
            },
            3.0,
        )
        .expect("RealOutput ray should enter its mirrored triangle bounds");
        assert!(point_nearly_equal(
            output_target,
            CorePoint { x: -3.0, y: 0.0 }
        ));

        let top_target = endpoint_overdraw_target(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 100.0 },
            modelica_render::Bounds {
                x: -50.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            3.0,
        )
        .expect("rotated connector ray should enter its top bounds");
        assert!(point_nearly_equal(top_target, CorePoint { x: 0.0, y: 3.0 }));

        let fluid_target = endpoint_overdraw_target(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            modelica_render::Bounds {
                x: -20.0,
                y: -20.0,
                width: 40.0,
                height: 40.0,
            },
            3.0,
        )
        .expect("FluidPort ray should stay inside its circular bounds");
        assert!(point_nearly_equal(
            fluid_target,
            CorePoint { x: -3.0, y: 0.0 }
        ));
    }

    #[test]
    fn semantic_endpoint_route_keeps_stale_signal_line_anchored() {
        let source = r#"
connector RealInput = input Real annotation(
  Icon(graphics={Polygon(points={{-100,100},{100,0},{-100,-100}})}),
  Diagram(graphics={Polygon(points={{0,50},{100,0},{0,-50},{0,50}})})
);
connector RealOutput = output Real annotation(
  Icon(graphics={Polygon(points={{-100,100},{100,0},{-100,-100}})}),
  Diagram(graphics={Polygon(points={{-100,50},{0,0},{-100,-50}})})
);
model BoundarySig
  RealInput p_in annotation(Placement(transformation(extent={{-110,-10},{-90,10}})));
  RealOutput p_out annotation(Placement(transformation(extent={{90,-10},{110,10}})));
equation
  connect(p_in, p_out) annotation(Line(points={{-30.2,28.4},{30.2,28.4}}));
end BoundarySig;
"#;
        let file = parse(source, "BoundarySig.mo").expect("parse signal fixture");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("BoundarySig.mo", source)
            .expect("register signal fixture");
        let class = file
            .classes
            .iter()
            .find(|class| class.name == "BoundarySig")
            .expect("BoundarySig class");
        let scene = resolve_diagram(class, source, &mut registry);
        let connection = scene.connections.first().expect("signal connection");

        let (semantic_first, semantic_last) =
            strict_connection_points(&scene, connection).expect("signal connector anchors");
        let points = canonical_connection_points(&scene, connection);
        let display_points = display_connection_points(&scene, connection, &points, INITIAL_ZOOM);
        assert_eq!(
            (semantic_first, semantic_last),
            (
                CorePoint { x: -100.0, y: 0.0 },
                CorePoint { x: 100.0, y: 0.0 }
            )
        );
        assert_eq!(
            points,
            vec![
                CorePoint { x: -100.0, y: 0.0 },
                CorePoint { x: 100.0, y: 0.0 },
            ]
        );

        let geometry = core_diagram_geometry(&scene)
            .into_iter()
            .find(|geometry| geometry.layer == DiagramRenderLayer::Connection)
            .expect("signal connection geometry");
        assert_eq!(
            geometry
                .connection
                .expect("connection metadata")
                .line
                .points,
            display_points
        );
        assert_ne!(display_points, points);
        assert_eq!(
            line_local_to_world(
                connection.line.as_ref().expect("signal line"),
                display_points[0],
            ),
            CorePoint { x: -99.0, y: 0.0 }
        );
        assert_eq!(
            line_local_to_world(
                connection.line.as_ref().expect("signal line"),
                *display_points.last().expect("display endpoint"),
            ),
            CorePoint { x: 99.0, y: 0.0 }
        );
    }

    #[test]
    fn semantic_endpoint_route_repairs_unusable_source_polyline() {
        let first = CorePoint { x: -40.0, y: 12.0 };
        let last = CorePoint { x: 60.0, y: -18.0 };
        let route = semantic_endpoint_route(
            first,
            last,
            &[
                CorePoint { x: -30.0, y: 4.0 },
                CorePoint { x: 10.0, y: 25.0 },
                CorePoint { x: 50.0, y: -2.0 },
            ],
        );

        assert_eq!(route.first(), Some(&first));
        assert_eq!(route.last(), Some(&last));
        assert!(valid_interactive_connection_route(&route));
    }

    #[test]
    fn component_placement_extent_is_a_precise_drag_fallback() {
        let (mut scene, _) = connection_test_scene(
            CorePoint { x: 40.0, y: 25.0 },
            CorePoint { x: 140.0, y: 25.0 },
            vec![
                CorePoint { x: 40.0, y: 25.0 },
                CorePoint { x: 140.0, y: 25.0 },
            ],
        );
        scene.components[0].placement_extent = Some(modelica_core::scene::Extent {
            p1: CorePoint { x: -20.0, y: -10.0 },
            p2: CorePoint { x: 20.0, y: 10.0 },
        });
        scene.components[0].rotation = 90.0;
        let component = &scene.components[0];

        assert!(point_in_component_placement_extent(
            component,
            CorePoint { x: 35.0, y: 25.0 },
            0.0,
        ));
        assert!(!point_in_component_placement_extent(
            component,
            CorePoint { x: 70.0, y: 25.0 },
            0.0,
        ));
        assert!(!diagram_component_contains_point(
            component,
            CorePoint { x: 35.0, y: 25.0 },
            0.0,
        ));
    }

    #[test]
    fn component_drag_port_region_is_small_and_uses_semantic_center() {
        let anchor = ConnectorAnchor {
            key: PortKey::new("component", "port"),
            connector_ref: ConnectorRef {
                component_name: "component".to_owned(),
                connector_path: "port".to_owned(),
                subscripts: Vec::new(),
            },
            world_position: CorePoint { x: 0.0, y: 0.0 },
            visual_bounds: Some(modelica_render::Bounds {
                x: -100.0,
                y: -100.0,
                width: 200.0,
                height: 200.0,
            }),
            qualified_type: None,
            owner_component_id: "component".to_owned(),
            editable: true,
        };

        let tolerance = component_drag_port_tolerance(1.0);
        assert_eq!(tolerance, 4.0);
        assert!(connector_anchor_active_hit_distance(
            &anchor,
            CorePoint { x: 2.0, y: 0.0 },
            tolerance
        )
        .is_some());
        assert!(connector_anchor_active_hit_distance(
            &anchor,
            CorePoint { x: 6.0, y: 0.0 },
            tolerance
        )
        .is_none());
        assert!(
            connector_anchor_active_hit_distance(&anchor, CorePoint { x: 20.0, y: 0.0 }, 8.0,)
                .is_none()
        );
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
        let geometry = ellipse_geometry(&ellipse, Transform2D::identity(), INITIAL_ZOOM);
        assert_eq!(geometry.len(), 2);
        assert!(geometry[1].indices.len() >= 3);
    }

    #[test]
    fn omitted_fill_pattern_is_transparent_for_closed_shapes() {
        let rectangle = RectangleGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: -10.0, y: -8.0 },
                p2: CorePoint { x: 10.0, y: 8.0 },
            },
            line_color: [0, 0, 0],
            fill_color: [255, 255, 255],
            line_pattern: Some("LinePattern.Solid".to_owned()),
            line_thickness: Some(1.0),
            fill_pattern: None,
            radius: None,
        };
        let ellipse = EllipseGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: -10.0, y: -8.0 },
                p2: CorePoint { x: 10.0, y: 8.0 },
            },
            line_color: [0, 0, 0],
            fill_color: [255, 255, 255],
            line_pattern: Some("LinePattern.Solid".to_owned()),
            line_thickness: Some(1.0),
            fill_pattern: None,
            start_angle: None,
            end_angle: None,
        };
        let polygon = PolygonGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            points: vec![
                CorePoint { x: -10.0, y: -8.0 },
                CorePoint { x: 10.0, y: -8.0 },
                CorePoint { x: 0.0, y: 8.0 },
            ],
            line_color: [0, 0, 0],
            fill_color: [255, 255, 255],
            line_pattern: Some("LinePattern.Solid".to_owned()),
            line_thickness: Some(1.0),
            fill_pattern: None,
            smooth: None,
        };

        let rectangle_geometry =
            rectangle_geometry(&rectangle, Transform2D::identity(), INITIAL_ZOOM);
        let ellipse_geometry = ellipse_geometry(&ellipse, Transform2D::identity(), INITIAL_ZOOM);
        let polygon_geometry = polygon_geometry(&polygon, Transform2D::identity(), INITIAL_ZOOM);
        assert_eq!(
            rectangle_geometry.len(),
            1,
            "rectangle should only have a stroke"
        );
        assert_eq!(
            ellipse_geometry.len(),
            1,
            "ellipse should only have a stroke"
        );
        assert_eq!(
            polygon_geometry.len(),
            1,
            "polygon should only have a stroke"
        );
        assert!(rectangle_geometry[0].indices.len() >= 3);
        assert!(ellipse_geometry[0].indices.len() >= 3);
        assert!(polygon_geometry[0].indices.len() >= 3);
    }

    #[test]
    fn explicit_solid_fill_pattern_produces_fill_and_stroke() {
        let rectangle = RectangleGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: -10.0, y: -8.0 },
                p2: CorePoint { x: 10.0, y: 8.0 },
            },
            line_color: [0, 0, 0],
            fill_color: [255, 255, 255],
            line_pattern: Some("LinePattern.Solid".to_owned()),
            line_thickness: Some(1.0),
            fill_pattern: Some("FillPattern.Solid".to_owned()),
            radius: None,
        };

        let geometry = rectangle_geometry(&rectangle, Transform2D::identity(), INITIAL_ZOOM);
        assert_eq!(geometry.len(), 2);
        assert!(geometry.iter().all(|item| item.indices.len() >= 3));
    }

    #[test]
    fn connector_default_fill_does_not_occlude_connection_endpoint() {
        let coordinate_system = modelica_core::scene::CoordinateSystem::default();
        let connector_id = "connector";
        let connector_icon = Box::new(CoreIconScene {
            owner_qualified_name: Some("Port".to_owned()),
            coordinate_system,
            graphics: vec![ResolvedGraphic {
                id: modelica_core::scene::GraphicId("Port::rectangle".to_owned()),
                graphic: CoreGraphic::Rectangle(RectangleGraphic {
                    origin: CorePoint { x: 0.0, y: 0.0 },
                    rotation: 0.0,
                    extent: modelica_core::scene::Extent {
                        p1: CorePoint { x: -20.0, y: -20.0 },
                        p2: CorePoint { x: 20.0, y: 20.0 },
                    },
                    line_color: [0, 0, 0],
                    fill_color: [255, 255, 255],
                    line_pattern: Some("LinePattern.Solid".to_owned()),
                    line_thickness: Some(1.0),
                    // Modelica's default is FillPattern.None.
                    fill_pattern: None,
                    radius: None,
                }),
                owner: modelica_core::scene::GraphicOwner {
                    qualified_name: "Port".to_owned(),
                    kind: GraphicOwnerKind::Own,
                    instance_name: None,
                    dimensions: Vec::new(),
                },
                transform: Transform2D::identity(),
                editable: false,
            }],
            diagnostics: Vec::new(),
        });
        let component = CoreComponentInstance {
            id: connector_id.to_owned(),
            name: "port".to_owned(),
            source_owner: "Synthetic".to_owned(),
            type_name: "Port".to_owned(),
            dimensions: Vec::new(),
            resolved_type_qualified_name: Some("Port".to_owned()),
            model_text_context: ModelTextContext::default(),
            class_kind: Some(ClassKind::Connector),
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            placement_extent: Some(coordinate_system.extent),
            visible: true,
            editable: true,
            resolved_icon: Some(connector_icon),
            resolved_diagram: None,
        };
        let connection = modelica_core::scene::DiagramConnection {
            key: ConnectionKey::new(
                "Synthetic",
                ConnectorRef {
                    component_name: "port".to_owned(),
                    connector_path: String::new(),
                    subscripts: Vec::new(),
                },
                ConnectorRef {
                    component_name: "other".to_owned(),
                    connector_path: String::new(),
                    subscripts: Vec::new(),
                },
                0,
            ),
            id: "connection:port->other".to_owned(),
            lhs: ConnectorRef {
                component_name: "port".to_owned(),
                connector_path: String::new(),
                subscripts: Vec::new(),
            },
            rhs: ConnectorRef {
                component_name: "other".to_owned(),
                connector_path: String::new(),
                subscripts: Vec::new(),
            },
            from: "port".to_owned(),
            to: "other".to_owned(),
            line: Some(LineGraphic {
                origin: CorePoint { x: 0.0, y: 0.0 },
                rotation: 0.0,
                points: vec![CorePoint { x: -80.0, y: 0.0 }, CorePoint { x: 0.0, y: 0.0 }],
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
        let scene = CoreDiagramScene {
            class_qualified_name: Some("Synthetic".to_owned()),
            class_kind: Some(ClassKind::Model),
            coordinate_system,
            background_graphics: Vec::new(),
            components: vec![component],
            connections: vec![connection],
            diagnostics: Vec::new(),
            content_bounds: None,
        };

        let geometries = core_diagram_geometry(&scene);
        let connector_geometries = geometries
            .iter()
            .filter(|geometry| geometry.edit_key.as_deref() == Some(connector_id))
            .collect::<Vec<_>>();
        let connection_geometries = geometries
            .iter()
            .filter(|geometry| geometry.layer == DiagramRenderLayer::Connection)
            .collect::<Vec<_>>();
        assert_eq!(connector_geometries.len(), 1);
        assert_eq!(connector_geometries[0].layer, DiagramRenderLayer::Connector);
        assert_eq!(connection_geometries.len(), 1);
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
    fn modelica_strokes_keep_annotation_ratios_with_a_screen_floor() {
        let identity = Transform2D::identity();
        let icon_width = model_stroke_width(0.25, identity, 1.0, StrokeKind::Icon);
        assert!((icon_width - MIN_SCREEN_ICON_STROKE_PX).abs() < 0.0001);
        assert_eq!(
            model_stroke_width(2.0, identity, 1.0, StrokeKind::Icon),
            2.0
        );
        let zoomed_out_width = model_stroke_width(
            0.5,
            Transform2D {
                scale_x: 0.1,
                scale_y: 0.1,
                ..identity
            },
            0.5,
            StrokeKind::Icon,
        );
        assert!((zoomed_out_width * 0.5 - MIN_SCREEN_ICON_STROKE_PX).abs() < 0.0001);
        let connection_width = model_stroke_width(0.25, identity, 1.0, StrokeKind::Connection);
        assert!((connection_width - MIN_SCREEN_CONNECTION_STROKE_PX).abs() < 0.0001);
    }

    #[test]
    fn model_text_macros_and_annotation_color_are_preserved() {
        assert_eq!(
            resolve_modelica_text(
                "%%name %name (%class)",
                &text_context("sink", "BoundarySig"),
            ),
            "%name sink (BoundarySig)"
        );
        let text = modelica_core::scene::TextGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: -10.0, y: -5.0 },
                p2: CorePoint { x: 10.0, y: 5.0 },
            },
            text: "sink".to_owned(),
            color: [0, 0, 127],
            fill_color: None,
            fill_pattern: None,
            font_size: Some(12.0),
            font_name: None,
            horizontal_alignment: Some("TextAlignment.Left".to_owned()),
            text_style: vec!["TextStyle.Bold".to_owned()],
        };
        let context = text_context("sink", "BoundarySig");
        let item = model_text_overlay_item(&text, Transform2D::identity(), &context);
        assert_eq!(item.text, "sink");
        assert_eq!(item.color, [0, 0, 127]);
        assert_eq!(item.alignment, ModelTextAlignment::Left);
        assert!(item.bold);
        assert_eq!(item.minimum_screen_px, MIN_SCREEN_MODEL_TEXT_PX);

        let name_item = model_text_overlay_item(
            &modelica_core::scene::TextGraphic {
                text: "%name".to_owned(),
                ..text
            },
            Transform2D::identity(),
            &context,
        );
        assert_eq!(name_item.minimum_screen_px, MIN_SCREEN_MODEL_NAME_TEXT_PX);
    }

    #[test]
    fn model_text_style_flags_preserve_italic_underline_font_and_rotation() {
        let text = modelica_core::scene::TextGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 30.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: -20.0, y: -5.0 },
                p2: CorePoint { x: 20.0, y: 5.0 },
            },
            text: "styled".to_owned(),
            color: [0, 0, 0],
            fill_color: None,
            fill_pattern: None,
            font_size: None,
            font_name: Some("Courier New".to_owned()),
            horizontal_alignment: Some("TextAlignment.Right".to_owned()),
            text_style: vec![
                "TextStyle.Bold".to_owned(),
                "TextStyle.Italic".to_owned(),
                "TextStyle.UnderLine".to_owned(),
            ],
        };
        let item = model_text_overlay_item(
            &text,
            Transform2D {
                rotation: 15.0,
                scale_x: -1.0,
                scale_y: 1.0,
                ..Transform2D::identity()
            },
            &text_context("instance", "Component"),
        );
        assert!(item.font_size.is_none());
        assert_eq!(item.font_name.as_deref(), Some("Courier New"));
        assert!(item.bold && item.italic && item.underline);
        assert_eq!(item.alignment, ModelTextAlignment::Right);
        assert_eq!(item.angle, 45.0);
        let rotated = rotate_text_vector(Vec2::new(1.0, 0.0), std::f32::consts::FRAC_PI_2);
        assert!(rotated.x.abs() < 0.0001);
        assert!((rotated.y - 1.0).abs() < 0.0001);
    }

    #[test]
    fn text_parity_fixture_reaches_wgpu_overlay_inputs() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modelica-core/tests/fixtures/TextParity.mo"
        ));
        let _file = parse(source, "TextParity.mo").expect("parse TextParity fixture");
        let mut registry = LibraryRegistry::default();
        registry
            .register_source("TextParity.mo", source)
            .expect("index TextParity fixture");
        let (class, class_source) = registry
            .resolve_class("TextParity")
            .expect("TextParity class");
        let icon = IconResolver::new(&mut registry).resolve(&class, &class_source);
        let styled = icon
            .graphics
            .iter()
            .find_map(|graphic| match &graphic.graphic {
                CoreGraphic::Text(text) if text.text == "Bold Italic" => Some(text),
                _ => None,
            })
            .expect("styled Text graphic");
        let item = model_text_overlay_item(
            styled,
            Transform2D::identity(),
            &text_context("TextParity", "TextParity"),
        );
        assert!(item.bold && item.italic);

        let diagram = resolve_diagram(&class, &class_source, &mut registry);
        let component = diagram
            .components
            .iter()
            .find(|component| component.name == "instance")
            .expect("TextParity component");
        let component_text = component
            .resolved_icon
            .as_ref()
            .expect("component icon")
            .graphics
            .iter()
            .find_map(|graphic| match &graphic.graphic {
                CoreGraphic::Text(text) if text.text == "Instance override" => Some(text),
                _ => None,
            })
            .expect("parameterized component Text graphic");
        let component_item = model_text_overlay_item(
            component_text,
            Transform2D::identity(),
            &component.model_text_context,
        );
        assert_eq!(component_item.text, "Instance override");
    }

    #[test]
    fn diagram_text_preview_uses_the_same_move_transform_as_component_geometry() {
        let coordinate_system = modelica_core::scene::CoordinateSystem::default();
        let icon = CoreIconScene {
            owner_qualified_name: Some("Test.Component".to_owned()),
            coordinate_system,
            graphics: Vec::new(),
            diagnostics: Vec::new(),
        };
        let component = CoreComponentInstance {
            id: "component-id".to_owned(),
            name: "cpa".to_owned(),
            source_owner: "Test".to_owned(),
            type_name: "Component".to_owned(),
            dimensions: Vec::new(),
            resolved_type_qualified_name: Some("Test.Component".to_owned()),
            model_text_context: ModelTextContext::default(),
            class_kind: Some(ClassKind::Model),
            origin: CorePoint { x: 50.0, y: -20.0 },
            rotation: 90.0,
            placement_extent: Some(modelica_core::scene::Extent {
                p1: CorePoint { x: 10.0, y: 10.0 },
                p2: CorePoint { x: -10.0, y: -10.0 },
            }),
            visible: true,
            editable: true,
            resolved_icon: Some(Box::new(icon.clone())),
            resolved_diagram: None,
        };
        let text = modelica_core::scene::TextGraphic {
            origin: CorePoint { x: 4.0, y: -3.0 },
            rotation: 15.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: -30.0, y: -5.0 },
                p2: CorePoint { x: 30.0, y: 5.0 },
            },
            text: "%name".to_owned(),
            color: [0, 0, 0],
            fill_color: None,
            fill_pattern: None,
            font_size: Some(12.0),
            font_name: None,
            horizontal_alignment: None,
            text_style: Vec::new(),
        };
        let moved_preview = ComponentPreviewPlacement {
            origin: CorePoint { x: 70.0, y: -10.0 },
            rotation: component.rotation,
            extent: component
                .placement_extent
                .unwrap_or_else(default_component_extent),
            delta: CorePoint { x: 20.0, y: 10.0 },
        };
        let base_transform = compose_transform(
            effective_component_transform(&icon, &component, None),
            Transform2D::identity(),
        );
        let moved_transform = compose_transform(
            effective_component_transform(&icon, &component, Some(moved_preview)),
            Transform2D::identity(),
        );
        let context = text_context("cpa", "Component");
        let base_item = model_text_overlay_item(&text, base_transform, &context);
        let moved_item = model_text_overlay_item(&text, moved_transform, &context);

        assert_eq!(moved_item.text, base_item.text);
        assert_eq!(moved_item.scale, base_item.scale);
        for (base, moved) in base_item.corners.iter().zip(moved_item.corners) {
            assert!((moved.x - base.x - 20.0).abs() < 0.0001);
            assert!((moved.y - base.y - 10.0).abs() < 0.0001);
        }

        let resized_preview = ComponentPreviewPlacement {
            origin: component.origin,
            rotation: component.rotation,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: 20.0, y: 20.0 },
                p2: CorePoint { x: -20.0, y: -20.0 },
            },
            delta: CorePoint { x: 0.0, y: 0.0 },
        };
        let resized_item = model_text_overlay_item(
            &text,
            effective_component_transform(&icon, &component, Some(resized_preview)),
            &context,
        );
        assert!(resized_item.scale > base_item.scale);
    }

    #[test]
    fn diagram_text_preview_preserves_parameter_text_during_component_move() {
        let text = modelica_core::scene::TextGraphic {
            origin: CorePoint { x: 0.0, y: 0.0 },
            rotation: 0.0,
            extent: modelica_core::scene::Extent {
                p1: CorePoint { x: -20.0, y: -4.0 },
                p2: CorePoint { x: 20.0, y: 4.0 },
            },
            text: "H2,O2,H2O,N2".to_owned(),
            color: [0, 0, 0],
            fill_color: None,
            fill_pattern: None,
            font_size: Some(10.0),
            font_name: None,
            horizontal_alignment: None,
            text_style: Vec::new(),
        };
        let context = text_context("cpa", "Component");
        let base = model_text_overlay_item(&text, Transform2D::identity(), &context);
        let moved = model_text_overlay_item(
            &text,
            Transform2D {
                translation: CorePoint { x: 20.0, y: 10.0 },
                ..Transform2D::identity()
            },
            &context,
        );
        assert_eq!(moved.text, "H2,O2,H2O,N2");
        for (base, moved) in base.corners.iter().zip(moved.corners) {
            assert!((moved.x - base.x - 20.0).abs() < 0.0001);
            assert!((moved.y - base.y - 10.0).abs() < 0.0001);
        }
    }

    #[test]
    fn diagram_layers_draw_connectors_after_opaque_components() {
        let connection_layer = DIAGRAM_RENDER_LAYERS
            .iter()
            .position(|layer| *layer == DiagramRenderLayer::Connection)
            .expect("connection render layer");
        let component_layer = DIAGRAM_RENDER_LAYERS
            .iter()
            .position(|layer| *layer == DiagramRenderLayer::Component)
            .expect("component render layer");
        let connector_layer = DIAGRAM_RENDER_LAYERS
            .iter()
            .position(|layer| *layer == DiagramRenderLayer::Connector)
            .expect("connector render layer");
        assert!(connection_layer < component_layer);
        assert!(component_layer < connector_layer);

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
                        dimensions: Vec::new(),
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
                dimensions: Vec::new(),
                resolved_type_qualified_name: Some(name.to_owned()),
                model_text_context: ModelTextContext::default(),
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
            // the transformed line width, so bounds are checked with a
            // renderer tolerance rather than against the raw placement box.
            // The visible stroke floor intentionally makes tiny fitted icons
            // expand slightly more than their pre-floor geometry.
            let geometry_tolerance = MIN_SCREEN_ICON_STROKE_PX / INITIAL_ZOOM + 0.1;
            assert!((min[0] - expected_min_x).abs() < geometry_tolerance);
            assert!((max[0] - expected_max_x).abs() < geometry_tolerance);
            assert!((min[1] + 4.0).abs() < geometry_tolerance);
            assert!((max[1] - 4.0).abs() < geometry_tolerance);
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
            ConnectionEndpointConstraint::Semantic {
                lhs: CorePoint { x: 0.0, y: 0.0 },
                rhs: CorePoint { x: 60.0, y: 0.0 },
            },
        );
        assert_eq!(anchored.first(), Some(&CorePoint { x: 0.0, y: 0.0 }));
        assert_eq!(anchored.last(), Some(&CorePoint { x: 60.0, y: 0.0 }));
        assert!(is_orthogonal_polyline(&anchored));
    }

    #[test]
    fn component_translation_route_moves_owned_endpoints_without_diagonals() {
        let points = vec![
            CorePoint { x: -40.0, y: 0.0 },
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 20.0 },
            CorePoint { x: 40.0, y: 20.0 },
        ];
        let delta = CorePoint { x: 10.0, y: 5.0 };

        let moved_lhs = connection_route_for_component_translation(
            &points,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            (points[0], points[3]),
            true,
            false,
            delta,
        );
        assert_eq!(
            moved_lhs,
            vec![
                CorePoint { x: -30.0, y: 5.0 },
                CorePoint { x: 0.0, y: 5.0 },
                CorePoint { x: 0.0, y: 20.0 },
                CorePoint { x: 40.0, y: 20.0 },
            ]
        );
        assert!(is_orthogonal_polyline(&moved_lhs));

        let moved_rhs = connection_route_for_component_translation(
            &points,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            (points[0], points[3]),
            false,
            true,
            delta,
        );
        assert_eq!(
            moved_rhs,
            vec![
                CorePoint { x: -40.0, y: 0.0 },
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 0.0, y: 25.0 },
                CorePoint { x: 50.0, y: 25.0 },
            ]
        );
        assert!(is_orthogonal_polyline(&moved_rhs));
    }

    #[test]
    fn resolved_display_route_reanchors_read_only_connection_after_component_move() {
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let mut moved_scene = scene.clone();
        moved_scene.components[0].origin = CorePoint { x: 20.0, y: 30.0 };
        let (route, fallback) = resolved_connection_display_route(
            &moved_scene,
            &connection,
            &[CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        )
        .expect("semantic anchors should produce a display route");
        assert_eq!(fallback, ConnectionRouteFallback::CanonicalOrthogonal);
        assert_eq!(
            route,
            vec![
                CorePoint { x: 20.0, y: 30.0 },
                CorePoint { x: 100.0, y: 30.0 },
                CorePoint { x: 100.0, y: 0.0 },
            ]
        );
        assert!(displayed_connection_points_match_invariant(
            &moved_scene,
            &connection,
            &route,
        ));
    }

    #[test]
    fn component_drag_preview_reanchors_read_only_connection_with_empty_base_route() {
        let snapshot = ConnectionDragSnapshot {
            connection_id: "connection:read-only".to_owned(),
            connection_key: ConnectionKey::new(
                "Base",
                ConnectorRef {
                    component_name: "moved".to_owned(),
                    connector_path: String::new(),
                    subscripts: Vec::new(),
                },
                ConnectorRef {
                    component_name: "other".to_owned(),
                    connector_path: String::new(),
                    subscripts: Vec::new(),
                },
                0,
            ),
            source_editable: false,
            source_edit_error: Some("inherited".to_owned()),
            source_line_points: Vec::new(),
            base_route_points: Vec::new(),
            original_line_origin: CorePoint { x: 0.0, y: 0.0 },
            original_line_rotation: 0.0,
            original_endpoint_points: (
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 100.0, y: 0.0 },
            ),
            preview_points: Vec::new(),
            preview_route_valid: false,
            moved_first_endpoint: true,
            moved_last_endpoint: false,
        };
        let route = component_drag_preview_route(&snapshot, CorePoint { x: 20.0, y: 30.0 });
        assert_eq!(
            route,
            vec![
                CorePoint { x: 20.0, y: 30.0 },
                CorePoint { x: 20.0, y: 0.0 },
                CorePoint { x: 100.0, y: 0.0 },
            ]
        );
        assert!(valid_interactive_connection_route(&route));
    }

    #[test]
    fn component_drag_snapshots_retain_connections_without_source_edits() {
        let (mut scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let mut inherited = connection;
        inherited.id = "connection:inherited".to_owned();
        inherited.key = ConnectionKey::new("Base", inherited.lhs.clone(), inherited.rhs.clone(), 1);
        scene.connections.push(inherited);

        let snapshots = connection_drag_snapshots(&scene, "a", "Test", "");

        assert_eq!(snapshots.len(), 2);
        assert!(snapshots.iter().all(|snapshot| !snapshot.source_editable));
        assert!(snapshots
            .iter()
            .all(|snapshot| snapshot.preview_route_valid));
    }

    #[test]
    fn editable_component_connection_preflight_rejects_invalid_preview() {
        let (scene, _) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let mut snapshot = connection_drag_snapshots(&scene, "a", "Test", "")
            .pop()
            .expect("component connection snapshot");
        snapshot.source_editable = true;
        snapshot.preview_route_valid = false;
        snapshot.preview_points = vec![
            CorePoint { x: 20.0, y: 30.0 },
            CorePoint { x: 100.0, y: 0.0 },
        ];

        let error = build_component_connection_edits(&scene, "Test", "", &[snapshot])
            .expect_err("editable invalid routes must reject the whole component edit");
        assert!(error.contains("invalid editable preview route"));
        assert_eq!(
            scene.connections[0]
                .line
                .as_ref()
                .expect("source line")
                .points,
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }]
        );
    }

    #[test]
    fn read_only_component_connection_preflight_is_display_only() {
        let (scene, _) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let snapshots = connection_drag_snapshots(&scene, "a", "Child", "");
        assert_eq!(snapshots.len(), 1);
        assert!(!snapshots[0].source_editable);

        let edits = build_component_connection_edits(&scene, "Child", "", &snapshots)
            .expect("read-only connections should not create source edits");
        assert!(edits.is_empty());
        assert_eq!(
            scene.connections[0]
                .line
                .as_ref()
                .expect("source line")
                .points,
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }]
        );
    }

    #[test]
    fn component_translation_route_adds_elbow_for_moved_two_point_endpoint() {
        let points = vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }];
        let moved = connection_route_for_component_translation(
            &points,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            (points[0], points[1]),
            true,
            false,
            CorePoint { x: 20.0, y: 30.0 },
        );
        assert_eq!(
            moved,
            vec![
                CorePoint { x: 20.0, y: 30.0 },
                CorePoint { x: 100.0, y: 30.0 },
                CorePoint { x: 100.0, y: 0.0 },
            ]
        );
        assert!(is_orthogonal_polyline(&moved));
    }

    #[test]
    fn component_edit_history_and_scene_commit_move_together() {
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let before_points = connection.line.as_ref().expect("test line").points.clone();
        let after_origin = CorePoint { x: 20.0, y: 30.0 };
        let after_points = connection_route_for_component_translation(
            &before_points,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            (
                before_points[0],
                *before_points.last().expect("line endpoint"),
            ),
            true,
            false,
            after_origin,
        );

        let mut committed_scene = scene.clone();
        committed_scene.components[0].origin = after_origin;
        let mut committed_connection = connection.clone();
        committed_connection
            .line
            .as_mut()
            .expect("test line")
            .points = after_points.clone();
        committed_scene.connections = vec![committed_connection.clone()];

        assert!(connection_points_match_invariants(
            &committed_scene,
            &committed_connection,
            &after_points,
        ));

        let command = EditCommand::MoveDiagramComponent {
            class_name: "Test".to_owned(),
            component_id: "a-id".to_owned(),
            before_origin: scene.components[0].origin,
            after_origin,
            before_source: "before".to_owned(),
            after_source: "after".to_owned(),
            connection_edits: vec![ConnectionLineEdit {
                connection_key: connection.key.clone(),
                before_points: before_points.clone(),
                after_points: after_points.clone(),
                line_origin: CorePoint { x: 0.0, y: 0.0 },
            }],
        };
        let mut history = Vec::new();
        let mut redo_history = vec![sample_create_command()];
        record_successful_edit(&mut history, &mut redo_history, command);
        assert!(redo_history.is_empty());
        let Some(EditCommand::MoveDiagramComponent {
            after_origin: recorded_origin,
            connection_edits,
            ..
        }) = history.last()
        else {
            panic!("component edit was not recorded");
        };
        assert_eq!(*recorded_origin, after_origin);
        assert_eq!(connection_edits[0].after_points, after_points);

        let original_component_origin = scene.components[0].origin;
        let original_connection_points = before_points.clone();
        let invalid_points = vec![after_origin, *before_points.last().expect("line endpoint")];
        assert!(!valid_interactive_connection_route(&invalid_points));

        // A failed preflight must not partially apply either side of the edit.
        assert_eq!(scene.components[0].origin, original_component_origin);
        assert_eq!(
            scene.connections[0]
                .line
                .as_ref()
                .expect("test line")
                .points,
            original_connection_points
        );
        assert_eq!(history.len(), 1);
        assert!(redo_history.is_empty());
    }

    #[test]
    fn failed_stale_source_range_preserves_source_and_history() {
        let source = "model A\nend A;";
        let original_source = source.to_owned();
        let history = [sample_create_command()];
        let redo_history = [sample_create_command()];
        let result = apply_validated_source_edits(
            source,
            vec![SourceEdit {
                start: 6,
                end: 7,
                expected_text: Some("Old".to_owned()),
                replacement: "New".to_owned(),
            }],
            0,
        );

        let error = result.expect_err("stale source ranges must fail before applying");
        assert!(error.contains("stale source range"));
        assert_eq!(source, original_source);
        assert_eq!(history.len(), 1);
        assert_eq!(redo_history.len(), 1);
    }

    #[test]
    fn failed_candidate_parse_preserves_source_and_history() {
        let source = "model A\nend A;";
        let original_source = source.to_owned();
        let history = [sample_create_command()];
        let redo_history = [sample_create_command()];
        let result = apply_validated_source_edits(
            source,
            vec![SourceEdit {
                start: 0,
                end: 5,
                expected_text: Some("model".to_owned()),
                replacement: "model A\n  \"unterminated".to_owned(),
            }],
            0,
        );

        let error = result.expect_err("an unparsable candidate must fail before applying");
        assert!(error.contains("candidate source does not parse"));
        assert_eq!(source, original_source);
        assert_eq!(history.len(), 1);
        assert_eq!(redo_history.len(), 1);
    }

    #[test]
    fn failed_inherited_connection_preflight_preserves_scene_and_history() {
        let (scene, mut connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        connection.key.owner_class = "Base".to_owned();
        let original_origin = scene.components[0].origin;
        let original_points = scene.connections[0]
            .line
            .as_ref()
            .expect("test line")
            .points
            .clone();
        let history = [sample_create_command()];
        let redo_history = [sample_create_command()];
        let error =
            connection_source_editable_in_class(&connection, "Child", "model Child\nend Child;")
                .expect_err("inherited connections must be rejected");

        assert!(error.contains("read-only"));
        assert_eq!(scene.components[0].origin, original_origin);
        assert_eq!(
            scene.connections[0]
                .line
                .as_ref()
                .expect("test line")
                .points,
            original_points
        );
        assert_eq!(history.len(), 1);
        assert_eq!(redo_history.len(), 1);
    }

    #[test]
    fn failed_connection_geometry_validation_preserves_scene_and_history() {
        let (scene, connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let original_origin = scene.components[0].origin;
        let original_points = connection.line.as_ref().expect("test line").points.clone();
        let invalid_points = vec![
            CorePoint { x: 20.0, y: 30.0 },
            CorePoint { x: 100.0, y: 0.0 },
        ];
        let history = [sample_create_command()];
        let redo_history = [sample_create_command()];
        let error = connection_invariant_failure(&scene, &connection, &invalid_points)
            .expect("diagonal geometry must fail validation");

        assert_eq!(error, "connection points mismatch");
        assert_eq!(scene.components[0].origin, original_origin);
        assert_eq!(
            scene.connections[0]
                .line
                .as_ref()
                .expect("test line")
                .points,
            original_points
        );
        assert_eq!(history.len(), 1);
        assert_eq!(redo_history.len(), 1);
    }

    #[test]
    fn component_translation_manhattan_fallback_keeps_invalid_preview_orthogonal() {
        let base_route = vec![
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 40.0, y: 0.0 },
            CorePoint { x: 40.0, y: 30.0 },
        ];
        let fallback = manhattan_component_translation_route(
            &base_route,
            CorePoint { x: 20.0, y: 15.0 },
            CorePoint { x: 40.0, y: 30.0 },
            true,
            false,
        );
        assert_eq!(
            fallback,
            vec![
                CorePoint { x: 20.0, y: 15.0 },
                CorePoint { x: 40.0, y: 15.0 },
                CorePoint { x: 40.0, y: 30.0 },
            ]
        );
        assert!(valid_interactive_connection_route(&fallback));
    }

    #[test]
    fn component_translation_route_preserves_reversed_complex_line_order() {
        let points = vec![
            CorePoint { x: 50.0, y: 0.0 },
            CorePoint { x: 50.0, y: 30.0 },
            CorePoint { x: -20.0, y: 30.0 },
            CorePoint { x: -20.0, y: 0.0 },
            CorePoint { x: -50.0, y: 0.0 },
        ];
        let moved = connection_route_for_component_translation(
            &points,
            CorePoint { x: 0.0, y: 0.0 },
            0.0,
            (points[0], points[4]),
            false,
            true,
            CorePoint { x: 10.0, y: 5.0 },
        );
        assert_eq!(
            moved,
            vec![
                CorePoint { x: 50.0, y: 0.0 },
                CorePoint { x: 50.0, y: 30.0 },
                CorePoint { x: -20.0, y: 30.0 },
                CorePoint { x: -20.0, y: 5.0 },
                CorePoint { x: -40.0, y: 5.0 },
            ]
        );
        assert!(valid_interactive_connection_route(&moved));
    }

    #[test]
    fn invalid_interactive_connection_routes_are_rejected_before_gpu_update() {
        assert!(!valid_interactive_connection_route(&[]));
        assert!(!valid_interactive_connection_route(&[
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 0.0, y: 0.0 },
        ]));
        assert!(!valid_interactive_connection_route(&[
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 10.0, y: 5.0 },
        ]));
        assert!(!valid_interactive_connection_route(&[
            CorePoint {
                x: f32::NAN,
                y: 0.0,
            },
            CorePoint { x: 10.0, y: 0.0 },
        ]));
        assert!(valid_interactive_connection_route(&[
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 10.0, y: 0.0 },
            CorePoint { x: 10.0, y: 20.0 },
        ]));
    }

    #[test]
    fn fixed_existing_connection_route_does_not_require_semantic_anchors() {
        let (scene, mut connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![
                CorePoint { x: 0.0, y: 0.0 },
                CorePoint { x: 0.0, y: 20.0 },
                CorePoint { x: 100.0, y: 20.0 },
                CorePoint { x: 100.0, y: 0.0 },
            ],
        );
        connection.lhs.component_name = "missing".to_owned();
        let expected = connection.line.as_ref().unwrap().points.clone();
        let result = finalize_connection_route_with_constraint(
            &scene,
            &connection,
            &expected,
            ConnectionEndpointConstraint::FixedExisting {
                lhs: expected[0],
                rhs: *expected.last().unwrap(),
            },
        )
        .expect("fixed endpoints should bypass semantic resolution");
        assert_eq!(result, expected);
    }

    #[test]
    fn connection_edit_preflight_rejects_inherited_connection() {
        let source = "model Child\n equation\nend Child;";
        let (_, mut connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        connection.key.owner_class = "Base".to_owned();
        let error = connection_source_editable_in_class(&connection, "Child", source)
            .expect_err("inherited connections must be read-only");
        assert!(error.contains("inherited from Base"));
        assert!(error.contains("read-only in Child"));
    }

    #[test]
    fn connection_edit_preflight_validates_current_class_line_source() {
        let source =
            "model Test\n equation\n  connect(a, b) annotation(Line(points={{0, 0}, {100, 0}}));\nend Test;";
        let line_start = source.find("Line(").expect("Line annotation");
        let line_end = source[line_start..]
            .find(')')
            .map(|offset| line_start + offset + 1)
            .expect("Line annotation end");
        let (_, mut connection) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        connection.line_source_range = Some(SourceRange::new(line_start, line_end));
        connection.key.owner_class = "Test".to_owned();
        connection_source_editable_in_class(&connection, "Test", source)
            .expect("current class Line should be editable");
    }

    #[test]
    fn multiple_connection_edits_resolve_each_current_source_range() {
        let mut source = "model Top\n equation\n  connect(a, b) annotation(Line(points={{10, 0}, {20, 0}}));\n  connect(c, d) annotation(Line(points={{40, 0}, {50, 0}}));\nend Top;".to_owned();
        let mut registry = LibraryRegistry::default();
        for iteration in 0..20 {
            let file = parse(&source, "Multiple.mo").expect("parse iteration");
            registry
                .register_source("Multiple.mo", &source)
                .expect("reindex iteration");
            let scene = resolve_diagram(&file.classes[0], &source, &mut registry);
            assert_eq!(scene.connections.len(), 2);
            for (index, key) in scene
                .connections
                .iter()
                .map(|connection| connection.key.clone())
                .enumerate()
            {
                let base = (10 + iteration + index * 30) as f32;
                let edit = connection_points_edit_for_key(
                    &source,
                    &scene,
                    &key,
                    &[
                        CorePoint { x: base, y: 0.0 },
                        CorePoint {
                            x: base + 10.0,
                            y: 0.0,
                        },
                    ],
                )
                .expect("keyed line edit");
                source =
                    apply_validated_source_edits(&source, vec![edit], 0).expect("candidate source");
            }
        }
        assert!(parse(&source, "Multiple.mo").is_ok());
        assert!(source.contains("connect(a, b)"));
        assert!(source.contains("connect(c, d)"));
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
            INITIAL_ZOOM,
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
            INITIAL_ZOOM,
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
            subscripts: Vec::new(),
        };
        let rhs = ConnectorRef {
            component_name: "b".to_owned(),
            connector_path: "port".to_owned(),
            subscripts: Vec::new(),
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
                    subscripts: Vec::new(),
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
                    subscripts: Vec::new(),
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

    #[test]
    fn nearest_port_tie_breaks_by_stable_anchor_identity() {
        let anchors = vec![
            ConnectorAnchor {
                key: PortKey::new("z-component", "port"),
                connector_ref: ConnectorRef {
                    component_name: "z-component".to_owned(),
                    connector_path: "port".to_owned(),
                    subscripts: Vec::new(),
                },
                world_position: CorePoint { x: 0.0, y: 0.0 },
                visual_bounds: None,
                qualified_type: None,
                owner_component_id: "z-component".to_owned(),
                editable: true,
            },
            ConnectorAnchor {
                key: PortKey::new("a-component", "port"),
                connector_ref: ConnectorRef {
                    component_name: "a-component".to_owned(),
                    connector_path: "port".to_owned(),
                    subscripts: Vec::new(),
                },
                world_position: CorePoint { x: 0.0, y: 0.0 },
                visual_bounds: None,
                qualified_type: None,
                owner_component_id: "a-component".to_owned(),
                editable: true,
            },
        ];
        let mut spatial_index = DiagramSpatialIndex::default();
        for port_index in 0..anchors.len() {
            spatial_index.insert_port(
                port_index,
                HitBounds {
                    min: CorePoint { x: -1.0, y: -1.0 },
                    max: CorePoint { x: 1.0, y: 1.0 },
                },
            );
        }
        assert_eq!(
            spatial_index.nearest_port(CorePoint { x: 0.0, y: 0.0 }, 1.0, None, &anchors),
            Some(1)
        );
    }

    #[test]
    fn component_drag_snapshots_only_track_touching_connections() {
        let (base_scene, template) = connection_test_scene(
            CorePoint { x: 0.0, y: 0.0 },
            CorePoint { x: 100.0, y: 0.0 },
            vec![CorePoint { x: 0.0, y: 0.0 }, CorePoint { x: 100.0, y: 0.0 }],
        );
        let make_connection =
            |lhs_name: &str, lhs_path: String, rhs_name: &str, rhs_path: String, index: usize| {
                let lhs = ConnectorRef {
                    component_name: lhs_name.to_owned(),
                    connector_path: lhs_path,
                    subscripts: Vec::new(),
                };
                let rhs = ConnectorRef {
                    component_name: rhs_name.to_owned(),
                    connector_path: rhs_path,
                    subscripts: Vec::new(),
                };
                let mut connection = template.clone();
                connection.id = format!("connection:{lhs_name}:{index}");
                connection.key = ConnectionKey::new("Test", lhs.clone(), rhs.clone(), index);
                connection.lhs = lhs;
                connection.rhs = rhs;
                connection.from = lhs_name.to_owned();
                connection.to = rhs_name.to_owned();
                connection.line.as_mut().expect("test line").points = vec![
                    CorePoint {
                        x: index as f32 * 10.0,
                        y: 0.0,
                    },
                    CorePoint {
                        x: index as f32 * 10.0 + 5.0,
                        y: 0.0,
                    },
                ];
                connection
            };

        let mut scene = base_scene;
        scene.connections = (0..10)
            .map(|index| {
                make_connection(
                    "a",
                    format!("port[{index}]"),
                    &format!("sink{index}"),
                    String::new(),
                    index,
                )
            })
            .chain((0..40).map(|index| {
                make_connection(
                    "other_a",
                    format!("port[{index}]"),
                    &format!("other_b{index}"),
                    String::new(),
                    index + 10,
                )
            }))
            .collect();

        let snapshots = connection_drag_snapshots(&scene, "a", "Test", "");
        assert_eq!(snapshots.len(), 10);
        assert!(snapshots
            .iter()
            .all(|snapshot| snapshot.connection_key.lhs.component_name == "a"));
    }

    #[test]
    fn model_tree_comes_from_ordered_ast_members_and_retains_kinds() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../modelica-core/tests/fixtures/IEH_CPP.mo");
        let package = PackageLoader.load(fixture).expect("load IEH_CPP fixture");
        let tree = build_model_tree(&package);
        assert_eq!(tree.kind, Some(ClassKind::Package));
        assert_eq!(tree.name, "IEH_CPP");
        assert_eq!(
            tree.description.as_deref(),
            Some("远宽综合能源库 (pure C++ thermopack-cxx backend)")
        );
        assert_eq!(
            tree.children
                .iter()
                .map(|child| child.name.as_str())
                .collect::<Vec<_>>(),
            [
                "ThermoMedium",
                "Interfaces",
                "FluidUnits",
                "Converter",
                "FMU"
            ]
        );
        assert_eq!(tree.children[0].kind, Some(ClassKind::Package));
        assert_eq!(
            tree.children[0]
                .children
                .iter()
                .map(|child| child.name.as_str())
                .collect::<Vec<_>>(),
            ["Types", "Functions", "MediumWorld", "Units", "Examples"]
        );
        assert_eq!(tree.children[0].children[2].kind, Some(ClassKind::Model));
        assert_eq!(
            tree.children[1].children[0].children[0].kind,
            Some(ClassKind::Connector)
        );
        assert_eq!(
            tree.children[3].description.as_deref(),
            Some("能源转换设备库")
        );
    }
}
