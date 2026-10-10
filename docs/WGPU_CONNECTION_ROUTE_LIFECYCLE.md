# WGPU connection route lifecycle

This document records which route representation is authoritative at each
stage of diagram editing. It is intentionally limited to connection editing;
it does not describe GPU buffer allocation or rendering performance.

## Route representations

1. **Source route** — `DiagramConnection.line.points` from the Modelica
   `Line(points=...)` annotation. It is the persisted edit input and is never
   changed merely because a display route is repaired.
2. **Semantic endpoints** — `strict_connection_points(scene, connection)`.
   These are resolved from the connector identity and current component
   placement. When they resolve, they are authoritative over source endpoint
   coordinates.
3. **Canonical/display route** —
   `resolved_connection_display_route(scene, connection, raw_points)` and
   `canonical_connection_points`. The source route is a hint; stale or
   invalid routes are reanchored or replaced with a Manhattan route whose
   first and last points are the semantic endpoints.
4. **Raster display points** — `display_connection_points` applies the
   connector-bounds endpoint overdraw to a copy of the canonical route. These
   points exist only in `Geometry::connection.line`; they are not used for
   source edits, snapping, or semantic hit testing.
5. **Interaction preview route** — a temporary route owned by
   `PointerInteraction` or `ConnectionDragSnapshot`. It is uploaded to the
   preview mesh during a drag and becomes source data only after the commit
   transaction validates it.

## Entry points

| Stage | Route source | Validation/rebuild path |
| --- | --- | --- |
| Initial diagram load | `scene.connections[*].line.points` | `build_diagram_scene` → `core_diagram_geometry_at_zoom` → `canonical_connection_points` → `display_connection_points` |
| Hit-test cache | source route as a hint | `build_diagram_hit_cache` uses `canonical_connection_points`; it never indexes raw stale geometry |
| Component drag preview | `ConnectionDragSnapshot.base_route_points` / source fallback | `component_drag_preview_route` updates temporary GPU meshes; source is untouched |
| Connection segment/corner preview | current canonical interaction points | `build_connection_segment_drag_route` or `build_connection_corner_drag_route` |
| Component commit | preview routes plus current source snapshot | source transaction → candidate parse/resolve → component/connection invariants → document/GPU/history update |
| Connection commit | interaction route | finalize/reanchor → source transaction → candidate parse/resolve → endpoint/orthogonality invariants → document/GPU/history update |
| Undo/redo | command's before/after source | candidate parse/resolve and geometry checks occur before document mutation; successful application rebuilds selected scenes |
| Zoom/rebuild | current resolved scene | `rebuild_scene_geometry_for_zoom` calls `build_diagram_scene` again, so it cannot depend on preview mesh residue |
| Save/reopen | saved source and refreshed class ranges | reload reconstructs the scene from source; the same canonical/display route pipeline is used |

## Invariants

- If semantic endpoints resolve, the displayed route must be orthogonal and
  anchored to both semantic endpoints.
- If the source route is stale, only the displayed route is repaired; the
  source `Line(points=...)` remains unchanged until an explicit edit commit.
- A display-only endpoint overdraw may change raster points, but cannot change
  connector identity, semantic snap points, hit-test points, or source text.
- If semantic endpoints cannot be resolved, a fixed existing route may remain
  as source geometry. Such a route is not treated as an editable semantic
  connection.
- After a successful commit, preview state is disposable. Any later rebuild,
  zoom, undo/redo, or reload must reproduce the route from the committed
  source and semantic scene.
