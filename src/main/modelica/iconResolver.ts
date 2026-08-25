import type { AnnotationCall, AnnotationValue } from "./annotation.js";
import { getArg, parseAnnotationSlice, findIconCall } from "./annotation.js";
import type {
  IconDto,
  CoordinateSystemDto,
  Extent,
  Point,
  GraphicItemDto,
  RectangleDto,
  EllipseDto,
  LineDto,
  PolygonDto,
  TextDto,
} from "../../shared/modelicaGraphics.js";

function asNumber(v: AnnotationValue | undefined): number | undefined {
  if (v && v.type === "number") return v.value;
  return undefined;
}

function asString(v: AnnotationValue | undefined): string | undefined {
  if (v && v.type === "string") return v.value;
  return undefined;
}

function asArray(
  v: AnnotationValue | undefined,
): AnnotationValue[] | undefined {
  if (v && v.type === "array") return v.items;
  return undefined;
}

function parsePoint(value: AnnotationValue): Point | null {
  // points can be {x, y} as array of two numbers, or possibly as array with identifier?
  const arr = asArray(value);
  if (!arr || arr.length !== 2) return null;
  const x = asNumber(arr[0]);
  const y = asNumber(arr[1]);
  if (x === undefined || y === undefined) return null;
  return { x, y };
}

function parseExtent(value: AnnotationValue | undefined): Extent | null {
  if (!value) return null;
  const outer = asArray(value);
  if (!outer || outer.length !== 2) return null;
  const p1Val = outer[0]!;
  const p2Val = outer[1]!;
  const p1 = parsePoint(p1Val);
  const p2 = parsePoint(p2Val);
  if (!p1 || !p2) return null;
  return { p1, p2 };
}

function parsePoints(value: AnnotationValue | undefined): Point[] | null {
  if (!value) return null;
  const outer = asArray(value);
  if (!outer) return null;
  const points: Point[] = [];
  for (const item of outer) {
    const pt = parsePoint(item);
    if (pt) points.push(pt);
  }
  return points.length > 0 ? points : null;
}

function parseColor(
  value: AnnotationValue | undefined,
): [number, number, number] | undefined {
  if (!value) return undefined;
  const arr = asArray(value);
  if (!arr || arr.length !== 3) return undefined;
  const r = asNumber(arr[0]);
  const g = asNumber(arr[1]);
  const b = asNumber(arr[2]);
  if (r === undefined || g === undefined || b === undefined) return undefined;
  return [Math.round(r), Math.round(g), Math.round(b)];
}

function resolveRectangle(call: AnnotationCall): RectangleDto | null {
  const extent = parseExtent(getArg(call, "extent"));
  if (!extent) return null;
  return {
    type: "Rectangle",
    extent,
    lineColor: parseColor(getArg(call, "lineColor")),
    fillColor: parseColor(getArg(call, "fillColor")),
    lineThickness: asNumber(getArg(call, "lineThickness")),
    radius: asNumber(getArg(call, "radius")),
  };
}

function resolveEllipse(call: AnnotationCall): EllipseDto | null {
  const extent = parseExtent(getArg(call, "extent"));
  if (!extent) return null;
  return {
    type: "Ellipse",
    extent,
    lineColor: parseColor(getArg(call, "lineColor")),
    fillColor: parseColor(getArg(call, "fillColor")),
    startAngle: asNumber(getArg(call, "startAngle")),
    endAngle: asNumber(getArg(call, "endAngle")),
  };
}

function resolveLine(call: AnnotationCall): LineDto | null {
  const points = parsePoints(getArg(call, "points"));
  if (!points) return null;
  return {
    type: "Line",
    points,
    color:
      parseColor(getArg(call, "color")) ??
      parseColor(getArg(call, "lineColor")),
    thickness:
      asNumber(getArg(call, "thickness")) ??
      asNumber(getArg(call, "lineThickness")),
  };
}

function resolvePolygon(call: AnnotationCall): PolygonDto | null {
  const points = parsePoints(getArg(call, "points"));
  if (!points) return null;
  return {
    type: "Polygon",
    points,
    lineColor: parseColor(getArg(call, "lineColor")),
    fillColor: parseColor(getArg(call, "fillColor")),
  };
}

function resolveText(call: AnnotationCall, modelName: string): TextDto | null {
  const extent = parseExtent(getArg(call, "extent"));
  const textStringRaw = asString(getArg(call, "textString"));
  if (!extent || textStringRaw === undefined) return null;
  const textString = textStringRaw.split("%name").join(modelName);
  return {
    type: "Text",
    extent,
    textString,
    textColor:
      parseColor(getArg(call, "textColor")) ??
      parseColor(getArg(call, "lineColor")),
    fontSize: asNumber(getArg(call, "fontSize")),
  };
}

function resolveGraphic(
  call: AnnotationCall,
  modelName: string,
): GraphicItemDto | null {
  switch (call.name) {
    case "Rectangle":
      return resolveRectangle(call);
    case "Ellipse":
      return resolveEllipse(call);
    case "Line":
      return resolveLine(call);
    case "Polygon":
      return resolvePolygon(call);
    case "Text":
      return resolveText(call, modelName);
    default:
      return null;
  }
}

function parseCoordinateSystem(
  call: AnnotationCall | undefined,
): CoordinateSystemDto {
  const defaultExtent: Extent = {
    p1: { x: -100, y: -100 },
    p2: { x: 100, y: 100 },
  };
  if (!call) return { extent: defaultExtent };
  const extent = parseExtent(getArg(call, "extent")) ?? defaultExtent;
  const preserveArg = getArg(call, "preserveAspectRatio");
  let preserveAspectRatio: boolean | undefined;
  if (preserveArg && preserveArg.type === "boolean")
    preserveAspectRatio = preserveArg.value;
  // Modelica Boolean may also be identifier true/false
  if (preserveArg && preserveArg.type === "identifier") {
    if (preserveArg.name === "true") preserveAspectRatio = true;
    if (preserveArg.name === "false") preserveAspectRatio = false;
  }
  return { extent, preserveAspectRatio };
}

export function resolveIcon(
  iconCall: AnnotationCall,
  modelName: string,
): IconDto | null {
  // iconCall is Icon( coordinateSystem(...), graphics={...} )
  // coordinateSystem may be named arg or positional? Usually Icon(coordinateSystem(...), graphics={...})
  // Also Icon(extent=..., graphics=...) variant? Handle both.
  let coordinateSystem: CoordinateSystemDto | undefined;
  const graphics: GraphicItemDto[] = [];

  // Find coordinateSystem arg: either named "coordinateSystem" as call, or extent directly
  const coordArg = getArg(iconCall, "coordinateSystem");
  if (coordArg && coordArg.type === "call") {
    coordinateSystem = parseCoordinateSystem(coordArg.call);
  } else {
    // Check positional first arg that is coordinateSystem call
    for (const arg of iconCall.arguments) {
      if (
        !arg.name &&
        arg.value.type === "call" &&
        arg.value.call.name === "coordinateSystem"
      ) {
        coordinateSystem = parseCoordinateSystem(arg.value.call);
        break;
      }
    }
  }

  // graphics array: named "graphics" or positional second
  const graphicsArg = getArg(iconCall, "graphics");
  let graphicsItems: AnnotationValue[] | undefined;
  if (graphicsArg && graphicsArg.type === "array") {
    graphicsItems = graphicsArg.items;
  } else {
    // positional: find array that contains calls?
    for (const arg of iconCall.arguments) {
      if (!arg.name && arg.value.type === "array") {
        // check if array contains calls like Rectangle
        const hasGraphics = arg.value.items.some((i) => i.type === "call");
        if (hasGraphics) {
          graphicsItems = arg.value.items;
          break;
        }
      }
    }
    // also check if Icon has extent shorthand: Icon(extent={{...}}, graphics={...}) then extent is direct array
    if (!graphicsItems) {
      // try to find any array arg named graphics alternative
      const alt = iconCall.arguments.find((a) => a.name === "graphics");
      if (alt && alt.value.type === "array") graphicsItems = alt.value.items;
    }
  }

  if (graphicsItems) {
    for (const item of graphicsItems) {
      if (item.type === "call") {
        const g = resolveGraphic(item.call, modelName);
        if (g) graphics.push(g);
      }
    }
  }

  if (!coordinateSystem) {
    // fallback to extent directly on Icon: Icon(extent={{...}})
    const extentDirect = parseExtent(getArg(iconCall, "extent"));
    if (extentDirect) coordinateSystem = { extent: extentDirect };
    else
      coordinateSystem = {
        extent: { p1: { x: -100, y: -100 }, p2: { x: 100, y: 100 } },
      };
  }

  return { coordinateSystem, graphics };
}

// Top-level helper: extract annotation string slice -> IconDto
export function extractIconFromSlice(
  slice: string,
  modelName: string,
): IconDto | null {
  // We need to find annotation( ... Icon(...) ... )
  // Simple approach: tokenize slice, find annotation call, then Icon inside
  // But reuse annotation parser: parse whole slice as Modelica class? Simpler: find annotation substring via slice search
  // For now, try to parse first annotation occurrence in slice
  // We'll scan for "annotation" keyword in tokens and parse from there
  // To keep simple, try to find "annotation(" in slice and parse that call
  const idx = slice.indexOf("annotation");
  if (idx === -1) return null;
  const sub = slice.slice(idx);
  try {
    const anno = parseAnnotationSlice(sub);
    if (!anno) return null;
    const iconCall = findIconCall(anno);
    if (!iconCall) return null;
    return resolveIcon(iconCall, modelName);
  } catch {
    return null;
  }
}
