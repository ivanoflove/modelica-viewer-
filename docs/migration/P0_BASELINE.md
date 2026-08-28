# Modelica Viewer native migration baseline

This branch is the native `winit + wgpu + lyon` migration track. The baseline
is intentionally recorded before the existing Electron UI is replaced.

## P0 behavior contract

The first full native milestone must retain these Electron behaviors:

1. Open a standalone `.mo` file, a `package.mo` directory, or a Modelica
   library directory. A CLI path may be supplied at startup.
2. Build a stable package/class tree and select a class without accidentally
   aggregating descendant classes into the selected class.
3. Show the selected class source, resolved Icon, and resolved Diagram.
4. Render Modelica primitives: Line, Polygon, Rectangle, Ellipse, Text, and
   Bitmap, including origin, rotation, line/fill colors, line/fill patterns,
   and coordinate-system fit.
5. Preserve Icon inheritance and connector graphics provenance. Inherited
   graphics are visible but read-only.
6. Report parse, source, inheritance, and diagram diagnostics without taking
   down the window.
7. Edit supported Icon source ranges, write them back safely, and provide
   undo/redo for the existing editor operations.
8. Manage bundled/user libraries: list, add, remove, rescan, and reveal a
   source file in the platform file manager.

## Baseline fixtures

- Tracked parser fixture:
  `src/main/modelica/__tests__/fixtures/IEH_CPP.mo`
- Native smoke model used during development:
  `D:\Documents\Dymola\Model\GH\IEH_CPP\IEH_CPP.mo`
- Representative icon classes: `HeatX`, `Heater`, `Boundary`, plus a simple
  class containing each primitive graphic.

The Dymola path is machine-local and is not required for CI. CI must use
tracked fixtures and generated temporary package trees.

## Performance contract

The renderer must prepare/tessellate scene geometry outside the frame render
callback. Viewport interaction may update uniforms and selection state, but it
must not reparse Modelica or rebuild the complete scene for every mouse event.

Acceptance runs on native Windows and native Linux at 100%, 125%, 150%, and
200% DPI where available. On a 60 Hz display, 30 seconds of continuous zoom
and pan must not show sustained frame times above 16.7 ms or a window hang.

The current prototype's no-vsync Windows comparison is only a renderer
throughput check. It is not a substitute for a 60 Hz native display test.

## P1 exit gates

- `modelica-core` parses files and builds package/class indexes.
- `modelica-core::IconResolver` produces ownership-aware `IconScene` data.
- `modelica-render` provides fill semantics, geometry, hit testing, line
  paths, and viewport transforms without UI/backend dependencies.
- A real-file smoke check is available with:
  `cargo run -p modelica-core --example inspect -- <file-or-directory> <qualified-class>`.
- Core and render tests pass on Windows, and `cargo check` passes for the
  Linux target.
- No GPUI dependency is present in the new workspace.
