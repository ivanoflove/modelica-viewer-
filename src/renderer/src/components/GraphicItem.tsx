import { memo } from "react";
import type {
    RectangleDto,
    EllipseDto,
    LineDto,
    PolygonDto,
    TextDto,
    BitmapDto,
    GraphicItemDto,
} from "../../../shared/modelicaGraphics";
import { recordViewerPerformance } from "../performance";

function toCssColor(color?: [number, number, number]): string {
    if (!color) return "none";
    return `rgb(${color[0]}, ${color[1]}, ${color[2]})`;
}

function lineColor(item: GraphicItemDto): string {
    const color = item.type === "Line" ? item.color : item.type === "Text" ? item.textColor : item.lineColor;
    return toCssColor(color) === "none" ? "#000" : toCssColor(color);
}

function hasFill(fillColor: [number, number, number] | undefined, pattern?: string) {
    return !!fillColor && pattern !== "FillPattern.None";
}

function fillPaint(item: RectangleDto | EllipseDto | PolygonDto, styleId: string): string {
    if (!hasFill(item.fillColor, item.fillPattern)) return "none";
    const color = toCssColor(item.fillColor);
    switch (item.fillPattern) {
        case "FillPattern.Horizontal":
        case "FillPattern.Vertical":
        case "FillPattern.Cross":
        case "FillPattern.Forward":
        case "FillPattern.Backward":
        case "FillPattern.CrossDiag":
        case "FillPattern.HorizontalCylinder":
        case "FillPattern.VerticalCylinder":
        case "FillPattern.Sphere":
            return `url(#${styleId})`;
        default:
            // Solid and styles not yet known to the renderer use the base color.
            return color;
    }
}

function strokeDasharray(pattern?: string): string | undefined {
    switch (pattern) {
        case "LinePattern.Dash": return "8 4";
        case "LinePattern.Dot": return "2 3";
        case "LinePattern.DashDot": return "8 4 2 4";
        case "LinePattern.DashDotDot": return "8 4 2 4 2 4";
        default: return undefined;
    }
}

function styleDefinitions(item: GraphicItemDto, styleId: string): JSX.Element | null {
    if (item.type === "Line" || item.type === "Text" || !hasFill(item.fillColor, item.fillPattern)) return null;
    const fill = toCssColor(item.fillColor);
    const stroke = lineColor(item);
    const pattern = item.fillPattern;
    if (pattern === "FillPattern.HorizontalCylinder") {
        return <linearGradient id={styleId} x1="0%" y1="0%" x2="0%" y2="100%">
            <stop offset="0%" stopColor={stroke} />
            <stop offset="50%" stopColor={fill} />
            <stop offset="100%" stopColor={stroke} />
        </linearGradient>;
    }
    if (pattern === "FillPattern.VerticalCylinder") {
        return <linearGradient id={styleId} x1="0%" y1="0%" x2="100%" y2="0%">
            <stop offset="0%" stopColor={stroke} />
            <stop offset="50%" stopColor={fill} />
            <stop offset="100%" stopColor={stroke} />
        </linearGradient>;
    }
    if (pattern === "FillPattern.Sphere") {
        return <radialGradient id={styleId} cx="35%" cy="30%" r="70%">
            <stop offset="0%" stopColor="#fff" stopOpacity="0.8" />
            <stop offset="45%" stopColor={fill} />
            <stop offset="100%" stopColor={stroke} />
        </radialGradient>;
    }
    const horizontal = pattern === "FillPattern.Horizontal" || pattern === "FillPattern.Cross";
    const vertical = pattern === "FillPattern.Vertical" || pattern === "FillPattern.Cross";
    const forward = pattern === "FillPattern.Forward" || pattern === "FillPattern.CrossDiag";
    const backward = pattern === "FillPattern.Backward" || pattern === "FillPattern.CrossDiag";
    return <pattern id={styleId} patternUnits="userSpaceOnUse" width="8" height="8">
        <rect width="8" height="8" fill={fill} />
        {horizontal && <line x1="0" y1="4" x2="8" y2="4" stroke={stroke} strokeWidth="1" />}
        {vertical && <line x1="4" y1="0" x2="4" y2="8" stroke={stroke} strokeWidth="1" />}
        {forward && <line x1="0" y1="8" x2="8" y2="0" stroke={stroke} strokeWidth="1" />}
        {backward && <line x1="0" y1="0" x2="8" y2="8" stroke={stroke} strokeWidth="1" />}
    </pattern>;
}

function renderRectangle(item: RectangleDto, styleId: string) {
    const x1 = item.extent.p1.x;
    const y1 = item.extent.p1.y;
    const x2 = item.extent.p2.x;
    const y2 = item.extent.p2.y;
    return <rect x={Math.min(x1, x2)} y={Math.min(y1, y2)} width={Math.abs(x2 - x1)}
        height={Math.abs(y2 - y1)} rx={item.radius} ry={item.radius}
        stroke={item.pattern === "LinePattern.None" ? "none" : lineColor(item)}
        strokeDasharray={strokeDasharray(item.pattern)} fill={fillPaint(item, styleId)}
        strokeWidth={item.lineThickness ?? 1} vectorEffect="non-scaling-stroke" />;
}

function renderEllipse(item: EllipseDto, styleId: string) {
    const x1 = item.extent.p1.x;
    const y1 = item.extent.p1.y;
    const x2 = item.extent.p2.x;
    const y2 = item.extent.p2.y;
    return <ellipse cx={(x1 + x2) / 2} cy={(y1 + y2) / 2}
        rx={Math.abs(x2 - x1) / 2} ry={Math.abs(y2 - y1) / 2}
        stroke={item.pattern === "LinePattern.None" ? "none" : lineColor(item)}
        strokeDasharray={strokeDasharray(item.pattern)} fill={fillPaint(item, styleId)}
        strokeWidth={item.lineThickness ?? 1} vectorEffect="non-scaling-stroke" />;
}

function renderLine(item: LineDto) {
    return <polyline points={item.points.map((p) => `${p.x},${p.y}`).join(" ")} fill="none"
        stroke={item.pattern === "LinePattern.None" ? "none" : lineColor(item)}
        strokeDasharray={strokeDasharray(item.pattern)} strokeWidth={item.thickness ?? 1}
        vectorEffect="non-scaling-stroke" />;
}

function renderPolygon(item: PolygonDto, styleId: string) {
    return <polygon points={item.points.map((p) => `${p.x},${p.y}`).join(" ")}
        stroke={item.pattern === "LinePattern.None" ? "none" : lineColor(item)}
        strokeDasharray={strokeDasharray(item.pattern)} fill={fillPaint(item, styleId)}
        strokeWidth={item.lineThickness ?? 1} vectorEffect="non-scaling-stroke" />;
}

function renderText(item: TextDto) {
    const x1 = item.extent.p1.x;
    const y1 = item.extent.p1.y;
    const x2 = item.extent.p2.x;
    const y2 = item.extent.p2.y;
    const width = Math.abs(x2 - x1);
    const height = Math.abs(y2 - y1);
    const characterCount = Math.max([...item.textString].length, 1);
    const autoFontSize = Math.max(0.1, Math.min(height * 0.82, width / (characterCount * 0.62)));
    const fontSize = item.fontSize && item.fontSize > 0 ? item.fontSize : autoFontSize;
    const textAnchor = item.horizontalAlignment === "TextAlignment.Left" ? "start" :
        item.horizontalAlignment === "TextAlignment.Right" ? "end" : "middle";
    const anchorX = textAnchor === "start" ? -width / 2 : textAnchor === "end" ? width / 2 : 0;
    const fontWeight = item.textStyle?.includes("TextStyle.Bold") ? 700 : 400;
    const fontStyle = item.textStyle?.includes("TextStyle.Italic") ? "italic" : "normal";
    const textDecoration = item.textStyle?.includes("TextStyle.UnderLine") ? "underline" : undefined;
    return <g transform={`translate(${(x1 + x2) / 2},${(y1 + y2) / 2}) rotate(${item.rotation ?? 0}) scale(1,-1)`}>
        <text x={anchorX} y={0} textAnchor={textAnchor} dominantBaseline="middle"
            fontSize={fontSize}
            fill={toCssColor(item.textColor) === "none" ? "#000" : toCssColor(item.textColor)}
            style={{ fontFamily: "sans-serif", fontWeight, fontStyle, textDecoration }}>
            {item.textString}
        </text>
    </g>;
}

function renderBitmap(item: BitmapDto) {
    const x = Math.min(item.extent.p1.x, item.extent.p2.x);
    const y = Math.min(item.extent.p1.y, item.extent.p2.y);
    const width = Math.abs(item.extent.p2.x - item.extent.p1.x);
    const height = Math.abs(item.extent.p2.y - item.extent.p1.y);
    return <g className="bitmap-placeholder">
        <rect x={x} y={y} width={width} height={height} fill="rgb(var(--accent-rgb) / 0.06)" stroke="#7b8799" strokeDasharray="5 3" vectorEffect="non-scaling-stroke" />
        <path d={`M ${x} ${y} L ${x + width} ${y + height} M ${x + width} ${y} L ${x} ${y + height}`} stroke="#7b8799" vectorEffect="non-scaling-stroke" />
    </g>;
}

export const GraphicItem = memo(function GraphicItem({ item, styleId = "graphic-style" }: { item: GraphicItemDto; styleId?: string }) {
    recordViewerPerformance("graphicItemRenders");
    const rendered = (() => {
        switch (item.type) {
            case "Rectangle": return renderRectangle(item, styleId);
            case "Ellipse": return renderEllipse(item, styleId);
            case "Line": return renderLine(item);
            case "Polygon": return renderPolygon(item, styleId);
            case "Text": return renderText(item);
            case "Bitmap": return renderBitmap(item);
        }
    })();
    const content = <>{styleDefinitions(item, styleId)}{rendered}</>;
    if (!item.origin) return content;
    return <g transform={`translate(${item.origin.x},${item.origin.y})`}>{content}</g>;
});

// Diagram and Icon share the same pure graphic renderer. Viewport, selection,
// hit testing, and editor overlays stay outside this component.
export const ResolvedGraphicRenderer = GraphicItem;
