import type { DragEvent } from "react";
import type { GraphicToolType } from "../../../shared/modelicaGraphics";

const tools: Array<{ type: GraphicToolType; label: string }> = [
  { type: "Line", label: "Line" },
  { type: "Polygon", label: "Polygon" },
  { type: "Rectangle", label: "Rectangle" },
  { type: "Text", label: "Text" },
  { type: "Ellipse", label: "Ellipse" },
  { type: "Bitmap", label: "Bitmap" },
];

function ToolGlyph({ type }: { type: GraphicToolType }) {
  if (type === "Line") return <svg viewBox="0 0 24 24"><path d="M4 19 20 5" /></svg>;
  if (type === "Polygon") return <svg viewBox="0 0 24 24"><path d="m4 18 4-12 12 3-5 11Z" /></svg>;
  if (type === "Rectangle") return <svg viewBox="0 0 24 24"><rect x="4" y="5" width="16" height="14" /></svg>;
  if (type === "Text") return <svg viewBox="0 0 24 24"><path d="M5 6h14M12 6v13M8 19h8" /></svg>;
  if (type === "Ellipse") return <svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="8" ry="6" /></svg>;
  return <svg viewBox="0 0 24 24"><rect x="4" y="5" width="16" height="14" /><path d="m5 17 5-5 3 3 2-2 4 4M8 9h.01" /></svg>;
}

export const GRAPHIC_DRAG_MIME = "application/x-modelica-graphic";

export function GraphicToolbar({ enabled = true }: { enabled?: boolean }) {
  const handleDragStart = (event: DragEvent<HTMLButtonElement>, type: GraphicToolType) => {
    event.dataTransfer.effectAllowed = "copy";
    event.dataTransfer.setData(
      GRAPHIC_DRAG_MIME,
      JSON.stringify({ type: "create-modelica-graphic", graphicType: type }),
    );
    event.currentTarget.classList.add("is-dragging");
  };

  return (
    <div className="graphic-toolbar" aria-label="Icon graphic tools">
      <span className="graphic-toolbar-label">Graphics</span>
      {tools.map((tool) => (
        <button
          key={tool.type}
          type="button"
          className="graphic-tool"
          draggable={enabled}
          disabled={!enabled}
          title={`拖动 ${tool.label} 到 Icon 画布`}
          onDragStart={(event) => handleDragStart(event, tool.type)}
          onDragEnd={(event) => event.currentTarget.classList.remove("is-dragging")}
        >
          <ToolGlyph type={tool.type} />
          <span>{tool.label}</span>
        </button>
      ))}
    </div>
  );
}
