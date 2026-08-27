use gpui::{
    App, Bounds, Context, PathBuilder, Pixels, Render, Window, WindowBounds, WindowOptions, canvas,
    div, point, prelude::*, px, rgb, size, uniform_list,
};
use gpui_platform::application;
use modelica_core::{
    Class, ClassKind, Graphic, IconResolver, IconScene, Library, LibraryKind, LibraryRegistry,
    PackageLoader, PackageNode,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccentName {
    Violet,
    Blue,
    Cyan,
    Orange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GlassMode {
    On,
    Reduced,
}

#[derive(Clone, Copy)]
struct Palette {
    root: u32,
    panel: u32,
    panel_alt: u32,
    card: u32,
    border: u32,
    text: u32,
    muted: u32,
    subtle: u32,
    accent: u32,
    accent_hover: u32,
    selected_text: u32,
    canvas: u32,
}

impl ThemeMode {
    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

impl AccentName {
    fn label(self) -> &'static str {
        match self {
            Self::Violet => "Violet",
            Self::Blue => "Blue",
            Self::Cyan => "Cyan",
            Self::Orange => "Orange",
        }
    }

    fn color(self) -> u32 {
        match self {
            Self::Violet => 0x6c5ce7,
            Self::Blue => 0x4c8dff,
            Self::Cyan => 0x27aeba,
            Self::Orange => 0xdd7b39,
        }
    }

    fn hover(self) -> u32 {
        match self {
            Self::Violet => 0x5b4bd6,
            Self::Blue => 0x3478ee,
            Self::Cyan => 0x168d99,
            Self::Orange => 0xc46629,
        }
    }
}

impl GlassMode {
    fn label(self) -> &'static str {
        match self {
            Self::On => "On",
            Self::Reduced => "Reduced",
        }
    }
}

fn palette(theme: ThemeMode, accent: AccentName) -> Palette {
    // System currently follows the dark chrome used by the native prototype. The setting is kept
    // separate so native OS appearance tracking can be wired without changing the UI contract.
    let dark = matches!(theme, ThemeMode::System | ThemeMode::Dark);
    if dark {
        Palette {
            root: 0x17181d,
            panel: 0x25272f,
            panel_alt: 0x1f2128,
            card: 0x2b2d36,
            border: 0x3a3d47,
            text: 0xf1f1f4,
            muted: 0xa9abb6,
            subtle: 0x7e808c,
            accent: accent.color(),
            accent_hover: accent.hover(),
            selected_text: 0xffffff,
            canvas: 0xf8f9fc,
        }
    } else {
        Palette {
            root: 0xf3f4f8,
            panel: 0xffffff,
            panel_alt: 0xf8f9fc,
            card: 0xeef0f5,
            border: 0xdfe1e8,
            text: 0x202128,
            muted: 0x70727d,
            subtle: 0x9799a4,
            accent: accent.color(),
            accent_hover: accent.hover(),
            selected_text: 0xffffff,
            canvas: 0xffffff,
        }
    }
}

struct ClassRow {
    class: Class,
    depth: usize,
}

struct ModelicaViewer {
    package_name: String,
    package_path: PathBuf,
    classes: Vec<ClassRow>,
    selected: Option<usize>,
    scene: IconScene,
    registry: LibraryRegistry,
    theme: ThemeMode,
    accent: AccentName,
    glass: GlassMode,
}

impl ModelicaViewer {
    fn load(path: &Path) -> Result<Self, String> {
        let package = PackageLoader
            .load(path)
            .map_err(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))?;
        let mut registry = LibraryRegistry::default();
        registry.index_package(&package);
        add_bundled_msl(&mut registry);
        register_package_sources(&package, &mut registry);

        let mut classes = Vec::new();
        flatten_package(&package, 0, &mut classes);

        let mut viewer = Self {
            package_name: package.qualified_name.clone(),
            package_path: path.to_owned(),
            classes,
            selected: None,
            scene: empty_scene(None),
            registry,
            theme: ThemeMode::System,
            accent: AccentName::Violet,
            glass: GlassMode::On,
        };

        if !viewer.classes.is_empty() {
            viewer.select_class(0);
        }
        Ok(viewer)
    }

    fn select_class(&mut self, index: usize) {
        let Some(row) = self.classes.get(index) else {
            return;
        };
        let class = row.class.clone();
        let source = match std::fs::read_to_string(&class.source_file) {
            Ok(source) => source,
            Err(error) => {
                self.scene = empty_scene(Some(class.qualified_name.clone()));
                self.scene
                    .diagnostics
                    .push(modelica_core::Diagnostic::warning(
                        "SOURCE_READ",
                        format!("{}: {error}", class.source_file.display()),
                    ));
                self.selected = Some(index);
                return;
            }
        };
        let _ = self
            .registry
            .register_source(class.source_file.clone(), source.clone());
        self.scene = IconResolver::new(&mut self.registry).resolve(&class, &source);
        self.selected = Some(index);
    }

    fn selected_name(&self) -> &str {
        self.selected
            .and_then(|index| self.classes.get(index))
            .map(|row| row.class.qualified_name.as_str())
            .unwrap_or("No class selected")
    }
}

impl Render for ModelicaViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(self.theme, self.accent);
        let glass_alpha = if self.glass == GlassMode::On {
            0.80
        } else {
            0.96
        };
        let row_count = self.classes.len();

        let tree = uniform_list(
            "class-tree",
            row_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|index| {
                        let row = this.classes.get(index)?;
                        let selected = this.selected == Some(index);
                        let indent = row.depth as f32 * 14.0;
                        let label =
                            format!("{}  {}", class_kind_symbol(row.class.kind), row.class.name);
                        Some(
                            div()
                                .id(format!("class-{index}"))
                                .mx_1()
                                .mb_1()
                                .pl(px(10.0 + indent))
                                .pr_2()
                                .py_2()
                                .rounded_md()
                                .text_sm()
                                .text_color(rgb(if selected {
                                    palette.selected_text
                                } else {
                                    palette.text
                                }))
                                .bg(rgb(if selected {
                                    palette.accent
                                } else {
                                    palette.panel_alt
                                })
                                .opacity(if selected {
                                    0.96
                                } else {
                                    0.62
                                }))
                                .hover(move |style| {
                                    style.bg(rgb(if selected {
                                        palette.accent_hover
                                    } else {
                                        palette.card
                                    })
                                    .opacity(0.92))
                                })
                                .cursor_pointer()
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_class(index);
                                    cx.notify();
                                })),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .h_full();

        let scene = self.scene.clone();
        let primitive_count = scene.graphics.len();
        let diagnostic_count = scene.diagnostics.len();
        let selected_name = self.selected_name().to_owned();
        let diagnostics = self
            .scene
            .diagnostics
            .iter()
            .take(4)
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
            .join("  ·  ");

        let mut theme_choices = div().flex().gap_1();
        for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
            let active = self.theme == mode;
            theme_choices = theme_choices.child(
                div()
                    .id(format!("theme-{}", mode.label()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(if active {
                        palette.selected_text
                    } else {
                        palette.muted
                    }))
                    .bg(rgb(if active {
                        palette.accent
                    } else {
                        palette.panel_alt
                    })
                    .opacity(if active { 0.96 } else { 0.68 }))
                    .child(mode.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.theme = mode;
                        cx.notify();
                    })),
            );
        }

        let mut accent_choices = div().flex().gap_1();
        for accent in [
            AccentName::Violet,
            AccentName::Blue,
            AccentName::Cyan,
            AccentName::Orange,
        ] {
            let active = self.accent == accent;
            accent_choices = accent_choices.child(
                div()
                    .id(format!("accent-{}", accent.label()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(if active {
                        accent.color()
                    } else {
                        palette.muted
                    }))
                    .bg(rgb(palette.panel_alt).opacity(if active { 0.92 } else { 0.58 }))
                    .child(format!("● {}", accent.label()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.accent = accent;
                        cx.notify();
                    })),
            );
        }

        let mut glass_choices = div().flex().gap_1();
        for mode in [GlassMode::On, GlassMode::Reduced] {
            let active = self.glass == mode;
            glass_choices = glass_choices.child(
                div()
                    .id(format!("glass-{}", mode.label()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(if active {
                        palette.selected_text
                    } else {
                        palette.muted
                    }))
                    .bg(rgb(if active {
                        palette.accent
                    } else {
                        palette.panel_alt
                    })
                    .opacity(if active { 0.94 } else { 0.58 }))
                    .child(mode.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.glass = mode;
                        cx.notify();
                    })),
            );
        }

        let header = div()
            .h(px(54.0))
            .px_4()
            .border_b_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.panel).opacity(glass_alpha))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(260.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded_lg()
                            .bg(rgb(palette.accent).opacity(0.16))
                            .text_color(rgb(palette.accent))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("◇"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(palette.accent))
                                    .child("MODELICA"),
                            )
                            .child(div().text_sm().child("Modelica Viewer")),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(rgb(palette.subtle))
                    .child(selected_name.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(theme_choices)
                    .child(accent_choices)
                    .child(glass_choices),
            );

        let sidebar = div()
            .w(px(278.0))
            .h_full()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.panel).opacity(glass_alpha))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_sm().child(self.package_name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(palette.subtle))
                                    .child(format!("{} classes", self.classes.len())),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(rgb(palette.subtle))
                            .child(self.package_path.display().to_string()),
                    ),
            )
            .child(div().flex_1().min_h_0().py_2().child(tree));

        let detail = div()
            .flex_1()
            .h_full()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.panel).opacity(glass_alpha))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .min_h(px(58.0))
                    .px_4()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_base().child(selected_name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(palette.subtle))
                                    .child(format!(
                                        "{primitive_count} graphics · {diagnostic_count} diagnostics"
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(palette.muted))
                                    .child("Source"),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(palette.accent).opacity(0.13))
                                    .text_xs()
                                    .text_color(rgb(palette.accent))
                                    .child("Icon"),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(palette.muted))
                                    .child("Diagram"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .m_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.canvas))
                    .overflow_hidden()
                    .child(icon_canvas(scene)),
            )
            .child(
                div()
                    .min_h(px(34.0))
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .text_xs()
                    .text_color(if diagnostic_count == 0 {
                        rgb(palette.subtle)
                    } else {
                        rgb(0xf59e0b)
                    })
                    .child(if diagnostics.is_empty() {
                        "Ready · GPUI native paint · Electron legacy appearance port"
                            .to_owned()
                    } else {
                        diagnostics
                    }),
            );

        div()
            .size_full()
            .bg(rgb(palette.root))
            .text_color(rgb(palette.text))
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_3()
                    .flex()
                    .child(sidebar)
                    .child(detail),
            )
    }
}

fn icon_canvas(scene: IconScene) -> impl IntoElement {
    canvas(
        move |_, _, _| {},
        move |bounds, _, window, _| {
            paint_icon_scene(&scene, bounds, window);
        },
    )
    .size_full()
}

fn paint_icon_scene(scene: &IconScene, bounds: Bounds<Pixels>, window: &mut Window) {
    let map = SceneMap::new(scene, bounds);
    for graphic in &scene.graphics {
        match graphic {
            Graphic::Rectangle(graphic) => {
                let points = extent_points(graphic.extent)
                    .map(|point| map.graphic_point(point, graphic.origin, graphic.rotation));
                paint_closed_polygon(
                    window,
                    &points,
                    graphic.fill_color,
                    graphic.line_color,
                    graphic.line_thickness.unwrap_or(0.25) * map.scale,
                );
            }
            Graphic::Ellipse(graphic) => {
                let center_x = (graphic.extent.p1.x + graphic.extent.p2.x) * 0.5;
                let center_y = (graphic.extent.p1.y + graphic.extent.p2.y) * 0.5;
                let radius_x = (graphic.extent.p2.x - graphic.extent.p1.x).abs() * 0.5;
                let radius_y = (graphic.extent.p2.y - graphic.extent.p1.y).abs() * 0.5;
                let mut points = Vec::with_capacity(49);
                for index in 0..48 {
                    let angle = std::f32::consts::TAU * index as f32 / 48.0;
                    points.push(map.graphic_point(
                        modelica_core::scene::Point {
                            x: center_x + radius_x * angle.cos(),
                            y: center_y + radius_y * angle.sin(),
                        },
                        graphic.origin,
                        graphic.rotation,
                    ));
                }
                paint_closed_polygon(
                    window,
                    &points,
                    graphic.fill_color,
                    graphic.line_color,
                    graphic.line_thickness.unwrap_or(0.25) * map.scale,
                );
            }
            Graphic::Line(graphic) => {
                let points = graphic
                    .points
                    .iter()
                    .copied()
                    .map(|point| map.graphic_point(point, graphic.origin, graphic.rotation))
                    .collect::<Vec<_>>();
                paint_polyline(
                    window,
                    &points,
                    graphic.color,
                    graphic.thickness.max(0.25) * map.scale,
                );
            }
            Graphic::Polygon(graphic) => {
                let points = graphic
                    .points
                    .iter()
                    .copied()
                    .map(|point| map.graphic_point(point, graphic.origin, graphic.rotation))
                    .collect::<Vec<_>>();
                paint_closed_polygon(
                    window,
                    &points,
                    graphic.fill_color,
                    graphic.line_color,
                    graphic.line_thickness.unwrap_or(0.25) * map.scale,
                );
            }
            Graphic::Text(_) | Graphic::Bitmap(_) => {}
        }
    }
}

struct SceneMap {
    center_x: f32,
    center_y: f32,
    model_center_x: f32,
    model_center_y: f32,
    scale: f32,
}

impl SceneMap {
    fn new(scene: &IconScene, bounds: Bounds<Pixels>) -> Self {
        let extent = scene.coordinate_system.extent;
        let width = (extent.p2.x - extent.p1.x).abs().max(1.0);
        let height = (extent.p2.y - extent.p1.y).abs().max(1.0);
        let available_width = f32::from(bounds.size.width).max(1.0) * 0.88;
        let available_height = f32::from(bounds.size.height).max(1.0) * 0.88;
        let scale = (available_width / width).min(available_height / height);
        Self {
            center_x: f32::from(bounds.origin.x) + f32::from(bounds.size.width) * 0.5,
            center_y: f32::from(bounds.origin.y) + f32::from(bounds.size.height) * 0.5,
            model_center_x: (extent.p1.x + extent.p2.x) * 0.5,
            model_center_y: (extent.p1.y + extent.p2.y) * 0.5,
            scale,
        }
    }

    fn graphic_point(
        &self,
        local: modelica_core::scene::Point,
        origin: modelica_core::scene::Point,
        rotation: f32,
    ) -> gpui::Point<Pixels> {
        let angle = rotation.to_radians();
        let rotated_x = local.x * angle.cos() - local.y * angle.sin();
        let rotated_y = local.x * angle.sin() + local.y * angle.cos();
        let model_x = origin.x + rotated_x;
        let model_y = origin.y + rotated_y;
        point(
            px(self.center_x + (model_x - self.model_center_x) * self.scale),
            px(self.center_y - (model_y - self.model_center_y) * self.scale),
        )
    }
}

fn extent_points(extent: modelica_core::scene::Extent) -> [modelica_core::scene::Point; 4] {
    [
        extent.p1,
        modelica_core::scene::Point {
            x: extent.p2.x,
            y: extent.p1.y,
        },
        extent.p2,
        modelica_core::scene::Point {
            x: extent.p1.x,
            y: extent.p2.y,
        },
    ]
}

fn paint_closed_polygon(
    window: &mut Window,
    points: &[gpui::Point<Pixels>],
    fill: [u8; 3],
    stroke: [u8; 3],
    stroke_width: f32,
) {
    if points.len() < 3 {
        return;
    }
    let mut fill_builder = PathBuilder::fill();
    fill_builder.move_to(points[0]);
    for point in &points[1..] {
        fill_builder.line_to(*point);
    }
    fill_builder.close();
    if let Ok(path) = fill_builder.build() {
        window.paint_path(path, rgb(rgb24(fill)));
    }

    let mut stroke_builder = PathBuilder::stroke(px(stroke_width.max(0.5)));
    stroke_builder.move_to(points[0]);
    for point in &points[1..] {
        stroke_builder.line_to(*point);
    }
    stroke_builder.line_to(points[0]);
    if let Ok(path) = stroke_builder.build() {
        window.paint_path(path, rgb(rgb24(stroke)));
    }
}

fn paint_polyline(
    window: &mut Window,
    points: &[gpui::Point<Pixels>],
    color: [u8; 3],
    stroke_width: f32,
) {
    if points.len() < 2 {
        return;
    }
    let mut builder = PathBuilder::stroke(px(stroke_width.max(0.5)));
    builder.move_to(points[0]);
    for point in &points[1..] {
        builder.line_to(*point);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(rgb24(color)));
    }
}

fn rgb24(color: [u8; 3]) -> u32 {
    (u32::from(color[0]) << 16) | (u32::from(color[1]) << 8) | u32::from(color[2])
}

fn flatten_package(package: &PackageNode, depth: usize, rows: &mut Vec<ClassRow>) {
    for class in &package.classes {
        flatten_class(class, depth, rows);
    }
    for child in &package.children {
        flatten_package(child, depth + 1, rows);
    }
}

fn flatten_class(class: &Class, depth: usize, rows: &mut Vec<ClassRow>) {
    rows.push(ClassRow {
        class: class.clone(),
        depth,
    });
    for child in &class.children {
        flatten_class(child, depth + 1, rows);
    }
}

fn register_package_sources(package: &PackageNode, registry: &mut LibraryRegistry) {
    if let Ok(source) = std::fs::read_to_string(&package.source_file) {
        let _ = registry.register_source(package.source_file.clone(), source);
    }
    for class in &package.classes {
        register_class_sources(class, registry);
    }
    for child in &package.children {
        register_package_sources(child, registry);
    }
}

fn register_class_sources(class: &Class, registry: &mut LibraryRegistry) {
    if let Ok(source) = std::fs::read_to_string(&class.source_file) {
        let _ = registry.register_source(class.source_file.clone(), source);
    }
    for child in &class.children {
        register_class_sources(child, registry);
    }
}

fn add_bundled_msl(registry: &mut LibraryRegistry) {
    let candidates = [
        PathBuf::from("resources/modelica/msl-4.1.0/Modelica"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/modelica/msl-4.1.0/Modelica"),
    ];
    if let Some(root) = candidates.into_iter().find(|path| path.is_dir()) {
        registry.add(Library {
            root,
            name: Some("Modelica Standard Library".into()),
            version: Some("4.1.0".into()),
            kind: LibraryKind::Builtin,
            read_only: true,
        });
    }
}

fn empty_scene(owner: Option<String>) -> IconScene {
    IconScene {
        owner_qualified_name: owner,
        coordinate_system: Default::default(),
        graphics: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn class_kind_symbol(kind: ClassKind) -> &'static str {
    match kind {
        ClassKind::Package => "◇",
        ClassKind::Model => "M",
        ClassKind::Block => "B",
        ClassKind::Connector | ClassKind::ExpandableConnector => "○",
        ClassKind::Record => "R",
        ClassKind::Function | ClassKind::OperatorFunction => "ƒ",
        ClassKind::Type => "T",
        ClassKind::Operator | ClassKind::OperatorRecord => "O",
        ClassKind::Class => "C",
    }
}

fn main() {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: cargo run -p modelica-gpui -- <file-or-library-directory>");
        std::process::exit(2);
    };
    let viewer = match ModelicaViewer::load(&path) {
        Ok(viewer) => viewer,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| viewer),
        )
        .expect("open Modelica Viewer window");
        cx.activate(true);
    });
}
