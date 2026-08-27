use gpui::{
    Anchor, App, Bounds, Context, PathBuilder, Pixels, Render, Window, WindowBounds, WindowOptions,
    anchored, canvas, deferred, div, hsla, linear_color_stop, linear_gradient, point, prelude::*, px,
    rgb, size, uniform_list,
};
use gpui_platform::application;
use modelica_core::{
    Class, ClassKind, Graphic, IconResolver, IconScene, Library, LibraryKind, LibraryRegistry,
    PackageLoader, PackageNode,
};
use std::collections::HashSet;
use std::ops::Range;
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
    root_alt: u32,
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
    let dark = matches!(theme, ThemeMode::System | ThemeMode::Dark);
    if dark {
        Palette {
            root: 0x17181d,
            root_alt: 0x20222a,
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
            root_alt: 0xe9ebf3,
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

#[derive(Clone)]
struct TreeNode {
    key: String,
    label: String,
    kind: ClassKind,
    class_index: Option<usize>,
    children: Vec<TreeNode>,
}

#[derive(Clone)]
struct VisibleTreeRow {
    key: String,
    label: String,
    kind: ClassKind,
    class_index: Option<usize>,
    depth: usize,
    has_children: bool,
    expanded: bool,
}

struct ModelicaViewer {
    classes: Vec<Class>,
    tree_root: TreeNode,
    expanded: HashSet<String>,
    selected: Option<usize>,
    scene: IconScene,
    registry: LibraryRegistry,
    theme: ThemeMode,
    accent: AccentName,
    glass: GlassMode,
    appearance_open: bool,
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
        let tree_root = build_package_tree(&package, &mut classes);
        let mut expanded = HashSet::new();
        expanded.insert(tree_root.key.clone());

        Ok(Self {
            classes,
            tree_root,
            expanded,
            selected: None,
            scene: empty_scene(None),
            registry,
            theme: ThemeMode::System,
            accent: AccentName::Violet,
            glass: GlassMode::On,
            appearance_open: false,
        })
    }

    fn select_class(&mut self, index: usize) {
        let Some(class) = self.classes.get(index).cloned() else {
            return;
        };
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

    fn toggle_tree(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_owned());
        }
    }

    fn visible_tree_rows(&self) -> Vec<VisibleTreeRow> {
        let mut rows = Vec::new();
        collect_visible_rows(&self.tree_root, 0, &self.expanded, &mut rows);
        rows
    }
}

impl Render for ModelicaViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(self.theme, self.accent);
        let glass_alpha = if self.glass == GlassMode::On { 0.78 } else { 0.96 };
        let visible_rows = self.visible_tree_rows();
        let visible_count = visible_rows.len();

        let tree = uniform_list(
            "class-tree",
            visible_count,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                range
                    .filter_map(|row_index| {
                        let row = visible_rows.get(row_index)?.clone();
                        let selected = row.class_index == this.selected;
                        let key = row.key.clone();
                        let class_index = row.class_index;
                        let has_children = row.has_children;
                        let toggle = if has_children {
                            if row.expanded { "▾" } else { "▸" }
                        } else {
                            ""
                        };
                        let indent = 5.0 + row.depth as f32 * 12.0;

                        Some(
                            div()
                                .id(format!("tree-row-{row_index}"))
                                .mx_1()
                                .h(px(29.0))
                                .pl(px(indent))
                                .pr_2()
                                .rounded_md()
                                .flex()
                                .items_center()
                                .gap_1()
                                .overflow_hidden()
                                .text_xs()
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
                                .opacity(if selected { 0.95 } else { 0.18 }))
                                .hover(move |style| {
                                    style.bg(rgb(if selected {
                                        palette.accent_hover
                                    } else {
                                        palette.card
                                    })
                                    .opacity(0.78))
                                })
                                .cursor_pointer()
                                .child(
                                    div()
                                        .w(px(14.0))
                                        .text_color(rgb(palette.subtle))
                                        .child(toggle),
                                )
                                .child(tree_icon(row.kind, selected, palette))
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(row.label),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if has_children && class_index.is_none() {
                                        this.toggle_tree(&key);
                                    } else if let Some(index) = class_index {
                                        this.select_class(index);
                                        if has_children {
                                            this.toggle_tree(&key);
                                        }
                                    }
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
        let diagnostics = self
            .scene
            .diagnostics
            .iter()
            .take(3)
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
            .join("  ·  ");

        let appearance = appearance_control(self, palette, cx);

        let header = div()
            .h(px(50.0))
            .px_4()
            .border_b_1()
            .border_color(rgb(palette.border).opacity(0.72))
            .bg(rgb(palette.panel).opacity(glass_alpha))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded_lg()
                            .bg(rgb(palette.accent).opacity(0.13))
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
                            .child(div().text_xs().child("Modelica Viewer")),
                    ),
            )
            .child(appearance);

        let sidebar = div()
            .w(px(278.0))
            .h_full()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border).opacity(0.72))
            .bg(rgb(palette.panel).opacity(glass_alpha))
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .h(px(38.0))
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(palette.border).opacity(0.68))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.subtle))
                            .child("MODEL LIBRARY"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.subtle))
                            .child(format!("{}", self.classes.len())),
                    ),
            )
            .child(div().flex_1().min_h_0().p_2().child(tree));

        let tabs = div()
            .h(px(38.0))
            .px_3()
            .border_b_1()
            .border_color(rgb(palette.border).opacity(0.68))
            .flex()
            .items_end()
            .gap_1()
            .child(tab_chip("Source", false, palette))
            .child(tab_chip("Icon", true, palette))
            .child(tab_chip("Diagram", false, palette));

        let detail = div()
            .flex_1()
            .h_full()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border).opacity(0.72))
            .bg(rgb(palette.panel).opacity(glass_alpha))
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(tabs)
            .child(
                div()
                    .h(px(32.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.subtle))
                            .child(format!(
                                "{primitive_count} graphics  ·  {diagnostic_count} diagnostics"
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.subtle))
                            .child("GPUI native paint"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .mx_3()
                    .mb_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(palette.border).opacity(0.66))
                    .bg(rgb(palette.canvas))
                    .shadow_sm()
                    .overflow_hidden()
                    .child(icon_canvas(scene)),
            )
            .child(
                div()
                    .min_h(px(30.0))
                    .px_4()
                    .border_t_1()
                    .border_color(rgb(palette.border).opacity(0.68))
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(if diagnostic_count == 0 {
                        rgb(palette.subtle)
                    } else {
                        rgb(0xf59e0b)
                    })
                    .child(if diagnostics.is_empty() {
                        "Ready".to_owned()
                    } else {
                        diagnostics
                    }),
            );

        div()
            .size_full()
            .bg(linear_gradient(
                145.0,
                linear_color_stop(rgb(palette.root), 0.0),
                linear_color_stop(rgb(palette.root_alt), 1.0),
            ))
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

fn appearance_control(
    viewer: &ModelicaViewer,
    palette: Palette,
    cx: &mut Context<ModelicaViewer>,
) -> impl IntoElement {
    let mut button = div()
        .id("appearance-toggle")
        .px_3()
        .h(px(30.0))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border).opacity(0.72))
        .bg(rgb(palette.panel_alt).opacity(0.60))
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(rgb(if viewer.appearance_open {
            palette.accent
        } else {
            palette.muted
        }))
        .cursor_pointer()
        .child("Appearance")
        .child(if viewer.appearance_open { "▴" } else { "▾" })
        .on_click(cx.listener(|this, _, _, cx| {
            this.appearance_open = !this.appearance_open;
            cx.notify();
        }));

    if viewer.appearance_open {
        let mut theme_choices = div().flex().gap_1();
        for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
            theme_choices = theme_choices.child(
                choice_chip(
                    format!("theme-{}", mode.label()),
                    mode.label(),
                    viewer.theme == mode,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.theme = mode;
                    cx.notify();
                })),
            );
        }

        let mut accent_choices = div().flex().flex_wrap().gap_1();
        for accent in [
            AccentName::Violet,
            AccentName::Blue,
            AccentName::Cyan,
            AccentName::Orange,
        ] {
            accent_choices = accent_choices.child(
                choice_chip(
                    format!("accent-{}", accent.label()),
                    &format!("● {}", accent.label()),
                    viewer.accent == accent,
                    palette,
                )
                .text_color(rgb(if viewer.accent == accent {
                    accent.color()
                } else {
                    palette.muted
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.accent = accent;
                    cx.notify();
                })),
            );
        }

        let mut glass_choices = div().flex().gap_1();
        for mode in [GlassMode::On, GlassMode::Reduced] {
            glass_choices = glass_choices.child(
                choice_chip(
                    format!("glass-{}", mode.label()),
                    mode.label(),
                    viewer.glass == mode,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.glass = mode;
                    cx.notify();
                })),
            );
        }

        let popover = div()
            .w(px(258.0))
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border).opacity(0.78))
            .bg(rgb(palette.panel).opacity(if viewer.glass == GlassMode::On {
                0.92
            } else {
                0.99
            }))
            .shadow_xl()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Appearance"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.subtle))
                            .child("UI"),
                    ),
            )
            .child(setting_group("Theme", theme_choices, palette))
            .child(setting_group("Accent", accent_choices, palette))
            .child(setting_group("Glass", glass_choices, palette))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.appearance_open = false;
                cx.notify();
            }));

        button = button.child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(10.0))
                    .child(popover),
            )
            .priority(10),
        );
    }

    button
}

fn tree_icon(kind: ClassKind, selected: bool, palette: Palette) -> gpui::Div {
    let (glyph, tint) = match kind {
        ClassKind::Package => ("▰", palette.accent),
        ClassKind::Model => ("◫", 0x4c8dff),
        ClassKind::Block => ("▦", 0x27aeba),
        ClassKind::Connector | ClassKind::ExpandableConnector => ("◎", 0xdd7b39),
        ClassKind::Record => ("▤", 0xb07adf),
        ClassKind::Function | ClassKind::OperatorFunction => ("ƒ", 0x36ae7c),
        ClassKind::Type => ("T", 0x8b93a7),
        ClassKind::Operator | ClassKind::OperatorRecord => ("◇", 0xd86c9b),
        ClassKind::Class => ("C", 0x8b93a7),
    };
    div()
        .w(px(18.0))
        .h(px(18.0))
        .rounded_md()
        .bg(rgb(tint).opacity(if selected { 0.22 } else { 0.10 }))
        .text_color(rgb(if selected { palette.selected_text } else { tint }))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph)
}

fn choice_chip(
    id: String,
    label: &str,
    active: bool,
    palette: Palette,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .h(px(25.0))
        .rounded_md()
        .border_1()
        .border_color(rgb(if active {
            palette.accent
        } else {
            palette.border
        })
        .opacity(0.78))
        .bg(rgb(if active {
            palette.accent
        } else {
            palette.panel_alt
        })
        .opacity(if active { 0.12 } else { 0.46 }))
        .text_xs()
        .text_color(rgb(if active {
            palette.accent
        } else {
            palette.muted
        }))
        .flex()
        .items_center()
        .cursor_pointer()
        .child(label.to_owned())
}

fn setting_group(label: &str, choices: gpui::Div, palette: Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .pt_1()
        .border_t_1()
        .border_color(rgb(palette.border).opacity(0.55))
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.subtle))
                .child(label.to_owned()),
        )
        .child(choices)
}

fn tab_chip(label: &str, active: bool, palette: Palette) -> gpui::Div {
    div()
        .h(px(37.0))
        .px_3()
        .border_b_2()
        .border_color(if active {
            rgb(palette.accent)
        } else {
            rgb(palette.panel).opacity(0.0)
        })
        .text_xs()
        .text_color(rgb(if active {
            palette.text
        } else {
            palette.muted
        }))
        .flex()
        .items_center()
        .child(label.to_owned())
}

fn build_package_tree(package: &PackageNode, classes: &mut Vec<Class>) -> TreeNode {
    let mut children = package
        .children
        .iter()
        .map(|child| build_package_tree(child, classes))
        .collect::<Vec<_>>();
    children.extend(
        package
            .classes
            .iter()
            .map(|class| build_class_tree(class, classes)),
    );
    children.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    TreeNode {
        key: format!("package:{}", package.qualified_name),
        label: package.name.clone(),
        kind: ClassKind::Package,
        class_index: None,
        children,
    }
}

fn build_class_tree(class: &Class, classes: &mut Vec<Class>) -> TreeNode {
    let class_index = classes.len();
    classes.push(class.clone());
    let mut children = class
        .children
        .iter()
        .map(|child| build_class_tree(child, classes))
        .collect::<Vec<_>>();
    children.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    TreeNode {
        key: format!("class:{}", class.qualified_name),
        label: class.name.clone(),
        kind: class.kind,
        class_index: Some(class_index),
        children,
    }
}

fn collect_visible_rows(
    node: &TreeNode,
    depth: usize,
    expanded: &HashSet<String>,
    rows: &mut Vec<VisibleTreeRow>,
) {
    let is_expanded = expanded.contains(&node.key);
    rows.push(VisibleTreeRow {
        key: node.key.clone(),
        label: node.label.clone(),
        kind: node.kind,
        class_index: node.class_index,
        depth,
        has_children: !node.children.is_empty(),
        expanded: is_expanded,
    });
    if is_expanded {
        for child in &node.children {
            collect_visible_rows(child, depth + 1, expanded, rows);
        }
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
                let mut points = Vec::with_capacity(48);
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
