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

function renderRectangle(item: RectangleDto) {
    const x1 = item.extent.p1.x;
    const y1 = item.extent.p1.y;
    const x2 = item.extent.p2.x;
    const y2 = item.extent.p2.y;
    const x = Math.min(x1, x2);
    const y = Math.min(y1, y2);
    const width = Math.abs(x2 - x1);
    const height = Math.abs(y2 - y1);
    const hasFill = !!item.fillColor;
    return (
        <rect
            x={x}
            y={y}
            width={width}
            height={height}
            rx={item.radius}
            ry={item.radius}
            stroke={
                toCssColor(item.lineColor) === "none"
                    ? "#000"
                    : toCssColor(item.lineColor)
            }
            fill={hasFill ? toCssColor(item.fillColor) : "none"}
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
    const hasFill = !!item.fillColor;
    return (
        <ellipse
            cx={cx}
            cy={cy}
            rx={rx}
            ry={ry}
            stroke={
                toCssColor(item.lineColor) === "none"
                    ? "#000"
                    : toCssColor(item.lineColor)
            }
            fill={hasFill ? toCssColor(item.fillColor) : "none"}
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
            stroke={
                toCssColor(item.color) === "none"
                    ? "#000"
                    : toCssColor(item.color)
            }
            strokeWidth={item.thickness ?? 1}
            vectorEffect="non-scaling-stroke"
        />
    );
}

function renderPolygon(item: PolygonDto) {
    const points = item.points.map((p) => `${p.x},${p.y}`).join(" ");
    const hasFill = !!item.fillColor;
    return (
        <polygon
            points={points}
            stroke={
                toCssColor(item.lineColor) === "none"
                    ? "#000"
                    : toCssColor(item.lineColor)
            }
            fill={hasFill ? toCssColor(item.fillColor) : "none"}
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
                style={{ fontFamily: "Inter, sans-serif" }}
            >
                {item.textString}
            </text>
        </g>
    );
}

export function GraphicItem({ item }: { item: GraphicItemDto }) {
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
}
