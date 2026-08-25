import type { IconDto } from "../../../shared/modelicaGraphics";
import { GraphicItem } from "./GraphicItem";

interface Props {
  icon: IconDto | null;
  modelName: string;
}

export function IconViewer({ icon, modelName }: Props) {
  if (!icon) {
    return <div className="no-icon">No Icon annotation</div>;
  }

  const extent = icon.coordinateSystem.extent;
  const x1 = extent.p1.x;
  const y1 = extent.p1.y;
  const x2 = extent.p2.x;
  const y2 = extent.p2.y;
  const minX = Math.min(x1, x2);
  const minY = Math.min(y1, y2);
  const width = Math.abs(x2 - x1);
  const height = Math.abs(y2 - y1);
  const viewBox = `${minX} ${minY} ${width} ${height}`;

  // Replace %name already done in resolver, but keep fallback for modelName display
  void modelName;

  return (
    <div className="icon-viewer">
      <svg
        viewBox={viewBox}
        className="modelica-icon"
        preserveAspectRatio="xMidYMid meet"
      >
        {/* Flip Y: Modelica Y up, SVG Y down */}
        <g transform="scale(1,-1)">
          {icon.graphics.map((item, idx) => (
            <GraphicItem key={idx} item={item} />
          ))}
        </g>
      </svg>
    </div>
  );
}
