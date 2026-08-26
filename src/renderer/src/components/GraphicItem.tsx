import type {
    RectangleDto,
    EllipseDto,
    LineDto,
    PolygonDto,
    TextDto,
    GraphicItemDto,
} from "../../../shared/modelicaGraphics";

function toCssColor(color?: [number, number, number]): string {
    if (!color) return "none";
    return `rgb(${color[0]}, ${color[1]}, ${color[2]})`;
}

function hasFill(fillColor: [number, number, number] | undefined, pattern?: string) {
    return !!fillColor && pattern !== "FillPattern.None";
}

function toCssStroke(
    color: [number, number, number] | undefined,
    pattern?: string,
): string {
    if (pattern === "LinePattern.None") return "none";
    return toCssColor(color) === "none" ? "#000" : toCssColor(color);
}

function renderRectangle(item: RectangleDto) {
    const x1 = item.extent.p1.x;
    const y1 = item.extent.p1.y;
    const x2 = item.extent.p2.x;
    const y2 = item.extent.p2.y;
    const x = Math.min(x1, x2);
    const y = Math.min(y1, y2);
    const width = Math.abs(x2 - x1);
    const height = Math.abs(y2 - y1);
    const filled = hasFill(item.fillColor, item.fillPattern);
    return (
        <rect
            x={x}
            y={y}
            width={width}
            height={height}
            rx={item.radius}
            ry={item.radius}
            stroke={toCssStroke(item.lineColor, item.pattern)}
            fill={filled ? toCssColor(item.fillColor) : "none"}
            strokeWidth={item.lineThickness ?? 1}
            vectorEffect="non-scaling-stroke"
        />
    );
}

function renderEllipse(item: EllipseDto) {
    const x1 = item.extent.p1.x;
    const y1 = item.extent.p1.y;
    const x2 = item.extent.p2.x;
    const y2 = item.extent.p2.y;
    const cx = (x1 + x2) / 2;
    const cy = (y1 + y2) / 2;
    const rx = Math.abs(x2 - x1) / 2;
    const ry = Math.abs(y2 - y1) / 2;
    const filled = hasFill(item.fillColor, item.fillPattern);
    return (
        <ellipse
            cx={cx}
            cy={cy}
            rx={rx}
            ry={ry}
            stroke={toCssStroke(item.lineColor, item.pattern)}
            fill={filled ? toCssColor(item.fillColor) : "none"}
            strokeWidth={1}
            vectorEffect="non-scaling-stroke"
        />
    );
}

function renderLine(item: LineDto) {
    const points = item.points.map((p) => `${p.x},${p.y}`).join(" ");
    return (
        <polyline
            points={points}
            fill="none"
            stroke={toCssStroke(item.color, item.pattern)}
            strokeWidth={item.thickness ?? 1}
            vectorEffect="non-scaling-stroke"
        />
    );
}

function renderPolygon(item: PolygonDto) {
    const points = item.points.map((p) => `${p.x},${p.y}`).join(" ");
    const filled = hasFill(item.fillColor, item.fillPattern);
    return (
        <polygon
            points={points}
            stroke={toCssStroke(item.lineColor, item.pattern)}
            fill={filled ? toCssColor(item.fillColor) : "none"}
            strokeWidth={1}
            vectorEffect="non-scaling-stroke"
        />
    );
}

function renderText(item: TextDto) {
    const x1 = item.extent.p1.x;
    const y1 = item.extent.p1.y;
    const x2 = item.extent.p2.x;
    const y2 = item.extent.p2.y;
    const cx = (x1 + x2) / 2;
    const cy = (y1 + y2) / 2;
    // Text needs to be flipped back because outer g is scale(1,-1)
    const fontSize = item.fontSize ?? 12;
    return (
        <g transform={`translate(${cx},${cy}) scale(1,-1)`}>
            <text
                x={0}
                y={0}
                textAnchor="middle"
                dominantBaseline="middle"
                fontSize={fontSize}
                fill={
                    toCssColor(item.textColor) === "none"
                        ? "#000"
                        : toCssColor(item.textColor)
                }
                style={{
                    fontFamily: "Inter, sans-serif",
                    fontWeight: item.textStyle?.includes("TextStyle.Bold") ? 700 : 400,
                }}
            >
                {item.textString}
            </text>
        </g>
    );
}

export function GraphicItem({ item }: { item: GraphicItemDto }) {
    const rendered = (() => {
      switch (item.type) {
        case "Rectangle":
            return renderRectangle(item);
        case "Ellipse":
            return renderEllipse(item);
        case "Line":
            return renderLine(item);
        case "Polygon":
            return renderPolygon(item);
        case "Text":
            return renderText(item);
        default:
            return null;
      }
    })();
    if (!rendered || !item.origin) return rendered;
    return (
      <g transform={`translate(${item.origin.x},${item.origin.y})`}>
        {rendered}
      </g>
    );
}
