import type { AnnotationCall, AnnotationValue } from "./annotation.js";
import {
  getArg,
  getArgWithRange,
  parseAnnotationSlice,
  findIconCall,
} from "./annotation.js";
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
  EditableGraphic,
  EditableIconDto,
} from "../../shared/modelicaGraphics.js";
import { identityTransform } from "../../shared/modelicaGraphics.js";

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

// Editable helpers
function formatNumber(n: number): string {
  // avoid 19.9999999, normalize to 6 decimals and trim
  if (Number.isInteger(n)) return String(n);
  const fixed = Number(n.toFixed(6));
  return String(fixed);
}

export function serializeExtent(extent: Extent): string {
  return `{{${formatNumber(extent.p1.x)},${formatNumber(extent.p1.y)}},{${formatNumber(extent.p2.x)},${formatNumber(extent.p2.y)}}}`;
}

export function serializePoints(points: Point[]): string {
  return `{${points.map((p) => `{${formatNumber(p.x)},${formatNumber(p.y)}}`).join(",")}}`;
}

export function serializeOrigin(origin: Point): string {
  return `{${formatNumber(origin.x)},${formatNumber(origin.y)}}`;
}

export function resolveEditableIcon(
  iconCall: AnnotationCall,
  modelName: string,
): EditableIconDto | null {
  const base = resolveIcon(iconCall, modelName);
  if (!base) return null;
  const editables: EditableGraphic[] = [];
  // need to map graphics items to their source ranges
  const graphicsArg = getArgWithRange(iconCall, "graphics");
  let graphicsItems: AnnotationValue[] | undefined;
  const graphicsRangeMap = new Map<number, { start: number; end: number }>();
  if (graphicsArg && graphicsArg.value.type === "array") {
    graphicsItems = graphicsArg.value.items;
    // map each call's range
    graphicsArg.value.items.forEach((item, idx) => {
      if (item.type === "call") {
        graphicsRangeMap.set(idx, item.range);
      }
    });
  } else {
    // positional fallback
    for (const arg of iconCall.arguments) {
      if (!arg.name && arg.value.type === "array") {
        const hasGraphics = arg.value.items.some((i) => i.type === "call");
        if (hasGraphics) {
          graphicsItems = arg.value.items;
          arg.value.items.forEach((item, idx) => {
            if (item.type === "call") graphicsRangeMap.set(idx, item.range);
          });
          break;
        }
      }
    }
  }
  if (!graphicsItems) return { icon: base, editables };
  let callIdx = 0;
  for (const item of graphicsItems) {
    if (item.type !== "call") continue;
    const graphic = resolveGraphic(item.call, modelName);
    if (!graphic) {
      callIdx++;
      continue;
    }
    const itemRange = (item as { range: { start: number; end: number } }).range;
    // find extent/points/origin ranges inside call
    let extentRange: { start: number; end: number } | undefined;
    let pointsRange: { start: number; end: number } | undefined;
    let originRange: { start: number; end: number } | undefined;
    const extentArg = getArgWithRange(item.call, "extent");
    if (extentArg) extentRange = (extentArg.value as any).range;
    const pointsArg = getArgWithRange(item.call, "points");
    if (pointsArg) pointsRange = (pointsArg.value as any).range;
    const originArg = getArgWithRange(item.call, "origin");
    if (originArg) originRange = (originArg.value as any).range;
    const id = `${modelName}:${itemRange.start}`;
    editables.push({
      id,
      graphic,
      selected: false,
      transform: identityTransform,
      source: {
        itemRange,
        extentRange,
        pointsRange,
        originRange,
      },
    });
    callIdx++;
  }
  return { icon: base, editables };
}

export function extractEditableIconFromSlice(
  slice: string,
  modelName: string,
): EditableIconDto | null {
  const idx = slice.indexOf("annotation");
  if (idx === -1) return null;
  const sub = slice.slice(idx);
  try {
    const anno = parseAnnotationSlice(sub);
    if (!anno) return null;
    const iconCall = findIconCall(anno);
    if (!iconCall) return null;
    return resolveEditableIcon(iconCall, modelName);
  } catch {
    return null;
  }
}

// Convert editable source ranges (relative to the annotation slice) into
// absolute file offsets. sliceBase = start of the class slice in the file,
// annotationIdx = index of "annotation" within that slice.
export function toAbsoluteEditableRanges(
  editable: EditableIconDto,
  sliceBase: number,
  annotationIdx: number,
): EditableIconDto {
  const shift = sliceBase + annotationIdx;
  const editables = editable.editables.map((e) => {
    const shiftRange = (r?: { start: number; end: number }) =>
      r ? { start: r.start + shift, end: r.end + shift } : undefined;
    return {
      ...e,
      source: {
        itemRange: shiftRange(e.source.itemRange)!,
        extentRange: shiftRange(e.source.extentRange),
        pointsRange: shiftRange(e.source.pointsRange),
        originRange: shiftRange(e.source.originRange),
      },
    };
  });
  return { icon: editable.icon, editables };
}

// Recursively locate a class by its qualified name inside parsed classes.
import type { ClassNode } from "./types.js";

export function findClassByQualifiedName(
  classes: ClassNode[],
  qualifiedName: string,
): ClassNode | null {
  for (const c of classes) {
    if (c.qualifiedName === qualifiedName) return c;
    const found = findClassByQualifiedName(c.children, qualifiedName);
    if (found) return found;
  }
  return null;
}
