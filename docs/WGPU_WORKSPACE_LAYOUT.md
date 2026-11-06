# Compact WGPU workspace

Reference projects reviewed:

- [egui_tiles](https://github.com/rerun-io/egui_tiles): tiled work areas and tabs.
- [egui_dock](https://github.com/anhosh/egui_dock): resizable tabbed workspaces.
- [Rerun](https://github.com/rerun-io/rerun): an egui-based visualization application.

This change uses existing egui 0.28 panels; it does not copy their code, add a
docking dependency, upgrade egui/wgpu, or introduce simultaneous canvas views.

## Layout

- 40-point top toolbar: open file/library, library visibility, appearance.
- Resizable left library: preferred width 22% of the window; maximum 40%,
  capped at 480 points. Normal minimum is 180 points. Existing panel width is
  retained by egui during the session. The toolbar can hide/show the library.
- Center: one truncated model-path row, Source/Icon/Diagram tabs and existing
  canvas zoom controls, then the active view. The large duplicate title and
  excess card margins are removed. Canvas backgrounds stay transparent so
  egui does not cover the native WGPU scene.
- 26-point bottom status bar: load/error/save feedback, unsaved marker, active
  view and Source's read-only designation. Long messages have hover tooltips.

Long package, class, source and status labels truncate within their own region.
Tree virtualization, Source folding/cache/scrolling, canvas hit-test coordinates,
document state and editing transactions are unchanged.

## Acceptance

Headless tests run the real UI layout for Source/Icon/Diagram at 800x600 and
1360x860 logical points, at 1.0/1.25/1.5/2.0 pixels per point, including long
names/status messages. They verify panel bounds, content-space allocation and
space returned when the library is hidden. These checks are not screenshot QA.

Manual checks: drag the sidebar separator, toggle the library, switch all three
tabs, zoom/pan the canvas, scroll/fold Source, and verify save/load errors remain
visible in the bottom bar in both light and dark themes.
