use gpui::{
    App, Bounds, Context, PathBuilder, Pixels, Render, Window, WindowBounds, WindowOptions, canvas,
    div, point, prelude::*, px, rgb, size,
};
use gpui_platform::application;
use modelica_core::{
    Class, ClassKind, Graphic, IconResolver, IconScene, Library, LibraryKind, LibraryRegistry,
    PackageLoader, PackageNode,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThemeMode {
    Glass,
    Midnight,
    Light,
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
            Self::Glass => "Glass",
            Self::Midnight => "Midnight",
            Self::Light => "Light",
        }
    }

    fn palette(self) -> Palette {
        match self {
            Self::Glass => Palette {
                root: 0x090b12,
                panel: 0x121722,
                panel_alt: 0x181f2d,
                card: 0x111722,
                border: 0x2a3446,
                text: 0xe7edf7,
                muted: 0x9aa8bc,
                subtle: 0x68778d,
                accent: 0x5b8cff,
                accent_hover: 0x6f9aff,
                selected_text: 0xffffff,
                canvas: 0xf8fafc,
            },
            Self::Midnight => Palette {
                root: 0x07090d,
                panel: 0x0f1117,
                panel_alt: 0x171a22,
                card: 0x10131a,
                border: 0x272b35,
                text: 0xe5e7eb,
                muted: 0xa1a1aa,
                subtle: 0x71717a,
                accent: 0x2563eb,
                accent_hover: 0x3b82f6,
                selected_text: 0xffffff,
                canvas: 0xfafafa,
            },
            Self::Light => Palette {
                root: 0xeef2f7,
                panel: 0xf8fafc,
                panel_alt: 0xffffff,
                card: 0xffffff,
                border: 0xd7dee8,
                text: 0x172033,
                muted: 0x5f6b7a,
                subtle: 0x8a96a8,
                accent: 0x2563eb,
                accent_hover: 0x1d4ed8,
                selected_text: 0xffffff,
                canvas: 0xffffff,
            },
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
            scene: IconScene {
                owner_qualified_name: None,
                coordinate_system: Default::default(),
                graphics: Vec::new(),
                diagnostics: Vec::new(),
            },
            registry,
            theme: ThemeMode::Glass,
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
                self.scene.diagnostics.push(modelica_core::Diagnostic::warning(
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
        let palette = self.theme.palette();
        let mut tree = div()
            .id("class-tree")
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .flex_1()
            .overflow_scroll();

        for (index, row) in self.classes.iter().enumerate() {
            let selected = self.selected == Some(index);
            let indent = row.depth as f32 * 14.0;
            let label = format!("{}  {}", class_kind_symbol(row.class.kind), row.class.name);
            tree = tree.child(
                div()
                    .id(format!("class-{index}"))
                    .pl(px(8.0 + indent))
                    .pr_2()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .text_color(rgb(if selected { palette.selected_text } else { palette.text }))
                    .bg(rgb(if selected { palette.accent } else { palette.panel_alt }).opacity(if selected { 0.92 } else { 0.72 }))
                    .hover(move |style| style.bg(rgb(if selected { palette.accent_hover } else { palette.card }).opacity(0.94)))
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_class(index);
                        cx.notify();
                    })),
            );
        }

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

        let mut themes = div().flex().gap_1();
        for mode in [ThemeMode::Glass, ThemeMode::Midnight, ThemeMode::Light] {
            let active = self.theme == mode;
            themes = themes.child(
                div()
                    .id(format!("theme-{}", mode.label()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(if active { palette.selected_text } else { palette.muted }))
                    .bg(rgb(if active { palette.accent } else { palette.panel_alt }).opacity(0.82))
                    .hover(move |style| style.bg(rgb(if active { palette.accent_hover } else { palette.card }).opacity(0.94)))
                    .child(mode.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.theme = mode;
                        cx.notify();
                    })),
            );
        }

        div()
            .size_full()
            .bg(rgb(palette.root))
            .text_color(rgb(palette.text))
            .flex()
            .p_2()
            .gap_2()
            .child(
                div()
                    .w(px(300.0))
                    .h_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.panel).opacity(if self.theme == ThemeMode::Glass { 0.86 } else { 1.0 }))
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
                                    .text_xs()
                                    .text_color(rgb(palette.muted))
                                    .child("MODEL LIBRARY"),
                            )
                            .child(div().mt_1().text_base().child(self.package_name.clone()))
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(rgb(palette.subtle))
                                    .child(self.package_path.display().to_string()),
                            ),
                    )
                    .child(tree),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.panel).opacity(if self.theme == ThemeMode::Glass { 0.88 } else { 1.0 }))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .min_h(px(64.0))
                            .px_4()
                            .py_2()
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
                                    .gap_2()
                                    .child(themes)
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(rgb(palette.panel_alt).opacity(0.82))
                                            .text_xs()
                                            .text_color(rgb(palette.muted))
                                            .child("GPUI native paint"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
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
                                "Ready · theme affects UI chrome only; Modelica scene colors stay source-driven"
                                    .to_owned()
                            } else {
                                diagnostics
                            }),
                    ),
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
