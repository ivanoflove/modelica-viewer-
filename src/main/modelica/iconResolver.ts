import type { AnnotationCall, AnnotationValue } from "./annotation.js";
import {
  getArg,
  getArgWithRange,
  parseAnnotationSlice,
  findIconCall,
} from "./annotation.js";
import { tokenize } from "./lexer.js";
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
  BitmapDto,
  EditableGraphic,
  EditableIconDto,
  GraphicProvenance,
  SourceRangeRef,
} from "../../shared/modelicaGraphics.js";
import { identityTransform } from "../../shared/modelicaGraphics.js";
import { resolveModelicaTextString } from "../../shared/modelicaText.js";
import type { ClassNode } from "./types.js";

function asNumber(v: AnnotationValue | undefined): number | undefined {
  if (v && v.type === "number") return v.value;
  return undefined;
}

function asString(v: AnnotationValue | undefined): string | undefined {
  if (v && v.type === "string") return v.value;
  return undefined;
}

function asName(v: AnnotationValue | undefined): string | undefined {
  if (v && (v.type === "identifier" || v.type === "qualifiedName"))
    return v.name;
  return undefined;
}

function asNames(v: AnnotationValue | undefined): string[] {
  if (!v || v.type !== "array") return [];
  return v.items.map(asName).filter((name): name is string => !!name);
}

function asArray(
  v: AnnotationValue | undefined,
): AnnotationValue[] | undefined {
  if (v && v.type === "array") return v.items;
  return undefined;
}

function parsePoint(value: AnnotationValue | undefined): Point | null {
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

function parseOrigin(call: AnnotationCall): Point | undefined {
  return parsePoint(getArg(call, "origin")) ?? undefined;
}

function parseFillPattern(call: AnnotationCall): string | undefined {
  return asName(getArg(call, "fillPattern"));
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
    origin: parseOrigin(call),
    fillPattern: parseFillPattern(call),
    pattern: asName(getArg(call, "pattern")),
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
    lineThickness: asNumber(getArg(call, "lineThickness")),
    startAngle: asNumber(getArg(call, "startAngle")),
    endAngle: asNumber(getArg(call, "endAngle")),
    origin: parseOrigin(call),
    fillPattern: parseFillPattern(call),
    pattern: asName(getArg(call, "pattern")),
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
    origin: parseOrigin(call),
    smooth: asName(getArg(call, "smooth")),
    pattern: asName(getArg(call, "pattern")),
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
    lineThickness: asNumber(getArg(call, "lineThickness")),
    origin: parseOrigin(call),
    fillPattern: parseFillPattern(call),
    pattern: asName(getArg(call, "pattern")),
  };
}

function resolveText(call: AnnotationCall, modelName: string): TextDto | null {
  const extent = parseExtent(getArg(call, "extent"));
  const textStringRaw = asString(getArg(call, "textString"));
  if (!extent || textStringRaw === undefined) return null;
  const textString = resolveModelicaTextString(textStringRaw, {
    className: modelName,
    instanceName: modelName,
  });
  return {
    type: "Text",
    extent,
    textString,
    textTemplate: textStringRaw,
    textColor:
      parseColor(getArg(call, "textColor")) ??
      parseColor(getArg(call, "lineColor")),
    fontSize: asNumber(getArg(call, "fontSize")),
    rotation: asNumber(getArg(call, "rotation")),
    horizontalAlignment:
      asName(getArg(call, "horizontalAlignment")) ??
      asName(getArg(call, "textAlignment")),
    origin: parseOrigin(call),
    textStyle: asNames(getArg(call, "textStyle")),
  };
}

function resolveBitmap(call: AnnotationCall): BitmapDto | null {
  const extent = parseExtent(getArg(call, "extent"));
  if (!extent) return null;
  return {
    type: "Bitmap",
    extent,
    origin: parseOrigin(call),
    fileName: asString(getArg(call, "fileName")),
    imageSource: asString(getArg(call, "imageSource")),
  };
}

function scalarValueText(value: AnnotationValue): string | undefined {
  switch (value.type) {
    case "string": return value.value;
    case "number": return String(value.value);
    case "boolean": return String(value.value);
    case "identifier":
    case "qualifiedName": return value.name;
    default: return undefined;
  }
}

/** Read scalar parameter defaults from the owning class for Text macros. */
function parseParameterDefaults(classSlice: string): Record<string, string> {
  const tokens = tokenize(classSlice);
  const defaults: Record<string, string> = {};
  for (let index = 0; index < tokens.length; index++) {
    if (tokens[index]!.value !== "parameter") continue;
    let end = index;
    while (end < tokens.length && tokens[end]!.type !== "SEMICOLON") end++;
    const relativeEquals = tokens.slice(index, end).findIndex((token) => token.type === "EQUALS");
    if (relativeEquals < 0) {
      index = end;
      continue;
    }
    const equalsIndex = index + relativeEquals;
    let nameIndex = equalsIndex - 1;
    while (nameIndex > index && tokens[nameIndex]!.type !== "IDENT") nameIndex--;
    const name = tokens[nameIndex]?.value;
    if (!name) {
      index = end;
      continue;
    }
    const valueStart = tokens[equalsIndex]!.end;
    const valueEnd = tokens[end]?.start ?? classSlice.length;
    const valueCall = parseAnnotationSlice(`__value(${classSlice.slice(valueStart, valueEnd)})`);
    const value = valueCall?.arguments[0]?.value;
    const text = value ? scalarValueText(value) : undefined;
    if (text !== undefined) defaults[name] = text;
    index = end;
  }
  return defaults;
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
    case "Bitmap":
      return resolveBitmap(call);
    default:
      return null;
  }
}

/** Resolve one graphic call after its ownership has already been established. */
export function resolveGraphicCall(
  call: AnnotationCall,
  modelName: string,
): GraphicItemDto | null {
  return resolveGraphic(call, modelName);
}

/** Resolve graphics from a specific Icon/Diagram call, never from descendants. */
export function resolveGraphicsFromCall(
  containerCall: AnnotationCall,
  modelName: string,
): GraphicItemDto[] {
  const graphicsArg = getArg(containerCall, "graphics");
  if (!graphicsArg || graphicsArg.type !== "array") return [];
  return graphicsArg.items.flatMap((item) => {
    if (item.type !== "call") return [];
    const graphic = resolveGraphic(item.call, modelName);
    return graphic ? [graphic] : [];
  });
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
  if (preserveArg) {
    const preserveName = asName(preserveArg);
    if (preserveName === "true") preserveAspectRatio = true;
    if (preserveName === "false") preserveAspectRatio = false;
  }
  return {
    extent,
    preserveAspectRatio,
    grid: parsePoint(getArg(call, "grid")) ?? undefined,
    initialScale: asNumber(getArg(call, "initialScale")),
  };
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

export interface IconResolution {
  icon: IconDto | null;
  warnings: string[];
}

export interface IconClassLocation {
  target: ClassNode;
  allClasses: ClassNode[];
  source: string;
}

export type ExternalClassResolver = (
  target: ClassNode,
  baseName: string,
) => IconClassLocation | null;

function fallbackBaseIcon(baseName: string): IconDto | null {
  if (!baseName.startsWith("Modelica.Icons.")) return null;
  return {
    coordinateSystem: {
      extent: { p1: { x: -100, y: -100 }, p2: { x: 100, y: 100 } },
    },
    graphics: [
      {
        type: "Rectangle",
        extent: { p1: { x: -80, y: -60 }, p2: { x: 80, y: 60 } },
        lineColor: [0, 0, 127],
        fillColor: [255, 255, 255],
        fillPattern: "FillPattern.Solid",
        graphicId: `${baseName}:Icon.graphics:0`,
        ownerQualifiedName: baseName,
        inherited: true,
        inheritancePath: [baseName],
      },
    ],
  };
}

function annotateGraphics(
  icon: IconDto,
  owner: ClassNode,
  inherited: boolean,
  inheritancePath: string[],
  modelName = owner.name,
): IconDto {
  return {
    coordinateSystem: icon.coordinateSystem,
    parameterDefaults: icon.parameterDefaults,
    graphics: icon.graphics.map((graphic, index) => {
      const provenance: GraphicProvenance = {
        graphicId: `${owner.qualifiedName}:Icon.graphics:${index}`,
        ownerQualifiedName: owner.qualifiedName,
        ownerSourceFile: owner.sourceFile,
        inherited,
        inheritancePath,
      };
      if (graphic.type !== "Text") return { ...graphic, ...provenance } as typeof graphic;
      const textString = resolveModelicaTextString(
        graphic.textTemplate ?? graphic.textString,
        {
          classQualifiedName: owner.qualifiedName,
          className: owner.name,
          instanceName: modelName,
          parameterDefaults: icon.parameterDefaults,
        },
      );
      return { ...graphic, ...provenance, textString } as typeof graphic;
    }),
  };
}

function markInherited(icon: IconDto, through: string): IconDto {
  return {
    coordinateSystem: icon.coordinateSystem,
    parameterDefaults: icon.parameterDefaults,
    graphics: icon.graphics.map((graphic) => ({
      ...graphic,
      inherited: true,
      inheritancePath: [
        through,
        ...(graphic.inheritancePath ?? [
          graphic.ownerQualifiedName ?? "<base>",
        ]),
      ],
    })),
  };
}

function findInheritedClass(
  target: ClassNode,
  baseName: string,
  allClasses: ClassNode[],
): ClassNode | null {
  const direct = findClassByQualifiedName(allClasses, baseName);
  if (direct) return direct;
  if (baseName.includes(".")) return null;

  const namespace = target.qualifiedName.split(".").slice(0, -1);
  for (let length = namespace.length; length >= 0; length--) {
    const prefix = namespace.slice(0, length).join(".");
    const candidate = prefix ? `${prefix}.${baseName}` : baseName;
    const found = findClassByQualifiedName(allClasses, candidate);
    if (found) return found;
  }
  return null;
}

export function findClassBySourceRange(
  classes: ClassNode[],
  sourceRange: { start: number; end: number } | null,
): ClassNode | null {
  if (!sourceRange) return null;
  for (const cls of classes) {
    if (
      cls.sourceRange.start === sourceRange.start &&
      cls.sourceRange.end === sourceRange.end
    )
      return cls;
    const found = findClassBySourceRange(cls.children, sourceRange);
    if (found) return found;
  }
  return null;
}

/** Resolve a class's own Icon and any resolvable `extends` Icon graphics. */
export function resolveIconForClass(
  target: ClassNode,
  allClasses: ClassNode[],
  source: string,
  modelName: string,
  resolving = new Set<string>(),
  externalResolver?: ExternalClassResolver,
): IconResolution {
  if (resolving.has(target.qualifiedName)) {
    return {
      icon: null,
      warnings: [`Icon inheritance cycle at ${target.qualifiedName}`],
    };
  }
  const nextResolving = new Set(resolving).add(target.qualifiedName);
  const ownSlice = source.slice(
    target.sourceRange.start,
    target.sourceRange.end,
  );
  const ownMatch = findIconAnnotation(ownSlice);
  const ownRawIcon = ownMatch
    ? (() => {
        const icon = resolveIcon(ownMatch.icon, modelName);
        return icon
          ? { ...icon, parameterDefaults: parseParameterDefaults(ownSlice) }
          : null;
      })()
    : null;
  const ownDefinesCoordinateSystem = ownMatch
    ? iconDefinesCoordinateSystem(ownMatch.icon)
    : false;
  const ownIcon = ownRawIcon
    ? annotateGraphics(ownRawIcon, target, false, [target.qualifiedName], modelName)
    : null;
  let inherited: IconDto | null = null;
  const warnings: string[] = [];

  for (const baseName of target.extendsClauses) {
    const baseClass = findInheritedClass(target, baseName, allClasses);
    const externalLocation = externalResolver?.(target, baseName) ?? null;
    const baseResult = externalLocation
      ? resolveIconForClass(
          externalLocation.target,
          externalLocation.allClasses,
          externalLocation.source,
          externalLocation.target.name,
          nextResolving,
          externalResolver,
        )
      : baseClass
        ? resolveIconForClass(
            baseClass,
            allClasses,
            source,
            baseClass.name,
            nextResolving,
            externalResolver,
          )
        : {
            icon: fallbackBaseIcon(baseName),
            warnings: [`Base icon not resolved: ${baseName}`],
          };
    warnings.push(...baseResult.warnings);
    if (baseResult.icon) {
      const inheritedBase = markInherited(
        baseResult.icon,
        target.qualifiedName,
      );
      const currentInherited = inherited as IconDto | null;
      if (currentInherited === null) {
        inherited = inheritedBase;
      } else {
        inherited = {
          graphics: [...currentInherited.graphics, ...inheritedBase.graphics],
          coordinateSystem: currentInherited.coordinateSystem,
          parameterDefaults: {
            ...(currentInherited.parameterDefaults ?? {}),
            ...(inheritedBase.parameterDefaults ?? {}),
          },
        };
      }
    }
  }

  if (!ownIcon && !inherited) return { icon: null, warnings };
  if (!ownIcon) return { icon: inherited, warnings };
  if (!inherited) return { icon: ownIcon, warnings };
  return {
    icon: {
      coordinateSystem: ownDefinesCoordinateSystem
        ? ownIcon.coordinateSystem
        : inherited.coordinateSystem,
      graphics: [...inherited.graphics, ...ownIcon.graphics],
      parameterDefaults: {
        ...(inherited.parameterDefaults ?? {}),
        ...(ownIcon.parameterDefaults ?? {}),
      },
    },
    warnings,
  };
}

function iconDefinesCoordinateSystem(icon: AnnotationCall): boolean {
  const named = getArg(icon, "coordinateSystem");
  if (named?.type === "call" && named.call.name === "coordinateSystem") return true;
  if (getArg(icon, "extent")) return true;
  return icon.arguments.some(
    (argument) =>
      !argument.name &&
      argument.value.type === "call" &&
      argument.value.call.name === "coordinateSystem",
  );
}

interface IconAnnotationMatch {
  annotation: AnnotationCall;
  icon: AnnotationCall;
  offset: number;
}

/**
 * A class may contain many annotations (Placement, Documentation, Icon, ...).
 * Do not use String#indexOf here: the first annotation is often a connector
 * Placement annotation, and strings/comments may also contain that word.
 */
function findIconAnnotation(slice: string): IconAnnotationMatch | null {
  const tokens = tokenize(slice);
  for (let i = 0; i < tokens.length - 1; i++) {
    const token = tokens[i]!;
    const next = tokens[i + 1]!;
    if (token.type !== "KEYWORD" || token.value !== "annotation") continue;
    if (next.type !== "LPAREN") continue;
    const annotation = parseAnnotationSlice(slice.slice(token.start));
    if (!annotation) continue;
    const icon = findIconCall(annotation);
    if (icon) return { annotation, icon, offset: token.start };
  }
  return null;
}

// Top-level helper: extract annotation string slice -> IconDto
export function extractIconFromSlice(
  slice: string,
  modelName: string,
): IconDto | null {
  try {
    const match = findIconAnnotation(slice);
    return match ? resolveIcon(match.icon, modelName) : null;
  } catch {
    return null;
  }
}

/** Return the offset of the actual Icon annotation within a class slice. */
export function findIconAnnotationOffset(slice: string): number {
  return findIconAnnotation(slice)?.offset ?? -1;
}

/** Return the Icon(...) range relative to the supplied source slice. */
export function findIconSourceRange(
  slice: string,
): { start: number; end: number } | null {
  const match = findIconAnnotation(slice);
  if (match) {
    return {
      start: match.offset + match.icon.sourceRange.start,
      end: match.offset + match.icon.sourceRange.end,
    };
  }

  // Keep the structural range available even when the annotation value has
  // a syntax error. This lets callers distinguish a bad Icon body from a
  // missing/mis-bound Icon range. Tokens already ignore strings/comments.
  const tokens = tokenize(slice);
  for (let i = 0; i < tokens.length - 1; i++) {
    if (tokens[i]!.value !== "Icon" || tokens[i + 1]!.type !== "LPAREN")
      continue;
    let depth = 0;
    for (let j = i + 1; j < tokens.length; j++) {
      const token = tokens[j]!;
      if (token.type === "LPAREN") depth++;
      if (token.type === "RPAREN") {
        depth--;
        if (depth === 0) {
          return { start: tokens[i]!.start, end: token.end };
        }
      }
    }
  }
  return null;
}

export interface GraphicInsertionEdit {
  start: number;
  end: number;
  expectedText: string;
  replacement: string;
  graphicIndex: number;
}

/**
 * Build an insertion from token/annotation ranges. This deliberately does
 * not search for parentheses with a regular expression: strings, comments,
 * nested classes and nested annotation calls are all handled by the lexer.
 */
export function buildGraphicInsertionEdit(
  classSlice: string,
  graphicText: string,
): GraphicInsertionEdit | null {
  const tokens = tokenize(classSlice);
  let annotationMatch: { annotation: AnnotationCall; offset: number } | null = null;
  let iconMatch: IconAnnotationMatch | null = null;
  for (let i = 0; i < tokens.length - 1; i++) {
    const token = tokens[i]!;
    if (token.type !== "KEYWORD" || token.value !== "annotation") continue;
    if (tokens[i + 1]!.type !== "LPAREN") continue;
    const annotation = parseAnnotationSlice(classSlice.slice(token.start));
    if (!annotation) continue;
    annotationMatch ??= { annotation, offset: token.start };
    const icon = findIconCall(annotation);
    if (icon) {
      iconMatch = { annotation, icon, offset: token.start };
      break;
    }
  }

  if (iconMatch) {
    const icon = iconMatch.icon;
    const graphicsArg = getArgWithRange(icon, "graphics");
    if (graphicsArg?.value.type === "array") {
      const range = {
        start: iconMatch.offset + graphicsArg.value.range.start,
        end: iconMatch.offset + graphicsArg.value.range.end,
      };
      const body = classSlice.slice(range.start + 1, range.end - 1);
      const whitespace = body.match(/\s*$/)?.[0] ?? "";
      const start = range.end - 1 - whitespace.length;
      return {
        start,
        end: start,
        expectedText: "",
        replacement: body.trim() ? `, ${graphicText}` : graphicText,
        graphicIndex: graphicsArg.value.items.length,
      };
    }
    const close = iconMatch.offset + icon.sourceRange.end - 1;
    return {
      start: close,
      end: close,
      expectedText: "",
      replacement: icon.arguments.length
        ? `, graphics={${graphicText}}`
        : `graphics={${graphicText}}`,
      graphicIndex: 0,
    };
  }

  if (annotationMatch) {
    const annotationStart = annotationMatch.offset + annotationMatch.annotation.sourceRange.start;
    const annotationText = classSlice.slice(annotationStart, annotationMatch.offset + annotationMatch.annotation.sourceRange.end);
    const annotationTokens = tokenize(annotationText);
    const open = annotationTokens.find((token) => token.type === "LPAREN");
    if (!open) return null;
    const start = annotationStart + open.end;
    return {
      start,
      end: start,
      expectedText: "",
      replacement: annotationMatch.annotation.arguments.length
        ? `Icon(graphics={${graphicText}}), `
        : `Icon(graphics={${graphicText}})`,
      graphicIndex: 0,
    };
  }

  const endToken = [...tokens].reverse().find((token) => token.type === "KEYWORD" && token.value === "end");
  if (!endToken) return null;
  return {
    start: endToken.start,
    end: endToken.start,
    expectedText: "",
    replacement: `annotation(Icon(graphics={${graphicText}}));\n  `,
    graphicIndex: 0,
  };
}

// Editable helpers
export interface EditableOwnerMetadata {
  qualifiedName: string;
  sourceFile?: string;
  inheritancePath?: string[];
}

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
  owner?: EditableOwnerMetadata,
): EditableIconDto | null {
  const base = resolveIcon(iconCall, modelName);
  if (!base) return null;
  const editables: EditableGraphic[] = [];
  // need to map graphics items to their source ranges
  const graphicsArg = getArgWithRange(iconCall, "graphics");
  let graphicsItems: AnnotationValue[] | undefined;
  let graphicsRange: { start: number; end: number } | undefined;
  const graphicsRangeMap = new Map<number, { start: number; end: number }>();
  if (graphicsArg && graphicsArg.value.type === "array") {
    graphicsItems = graphicsArg.value.items;
    graphicsRange = (graphicsArg.value as { range?: { start: number; end: number } }).range;
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
          graphicsRange = (arg.value as { range?: { start: number; end: number } }).range;
          arg.value.items.forEach((item, idx) => {
            if (item.type === "call") graphicsRangeMap.set(idx, item.range);
          });
          break;
        }
      }
    }
  }
  if (!graphicsItems) return { icon: base, editables };
  let graphicIndex = 0;
  for (const item of graphicsItems) {
    if (item.type !== "call") continue;
    const graphic = resolveGraphic(item.call, modelName);
    if (!graphic) {
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
    const rangeOf = (name: string) =>
      getArgWithRange(item.call, name)?.value.range as
        | { start: number; end: number }
        | undefined;
    const ownerQualifiedName = owner?.qualifiedName ?? modelName;
    const id = `${ownerQualifiedName}:Icon.graphics:${graphicIndex++}`;
    const graphicWithProvenance = {
      ...graphic,
      graphicId: id,
      ownerQualifiedName,
      ownerSourceFile: owner?.sourceFile,
      inherited: false,
      inheritancePath: owner?.inheritancePath ?? [ownerQualifiedName],
    } as GraphicItemDto;
    editables.push({
      id,
      graphic: graphicWithProvenance,
      ownerQualifiedName,
      ownerSourceFile: owner?.sourceFile,
      inherited: false,
      inheritancePath: owner?.inheritancePath ?? [
        owner?.qualifiedName ?? modelName,
      ],
      selected: false,
      transform: identityTransform,
      source: {
        itemRange,
        extentRange,
        pointsRange,
        originRange,
        lineColorRange: rangeOf("lineColor"),
        colorRange: rangeOf("color"),
          textColorRange: rangeOf("textColor"),
          fillColorRange: rangeOf("fillColor"),
          textStringRange: rangeOf("textString"),
          fontSizeRange: rangeOf("fontSize"),
          textStyleRange: rangeOf("textStyle"),
          lineThicknessRange: rangeOf("lineThickness"),
        thicknessRange: rangeOf("thickness"),
        patternRange: rangeOf("pattern"),
        fillPatternRange: rangeOf("fillPattern"),
      },
    });
  }
  return { icon: base, editables, graphicsRange };
}

export function extractEditableIconFromSlice(
  slice: string,
  modelName: string,
  owner?: EditableOwnerMetadata,
): EditableIconDto | null {
  try {
    const match = findIconAnnotation(slice);
    const editable = match
      ? resolveEditableIcon(match.icon, modelName, owner)
      : null;
    if (!editable || !match) return editable;
    const withExpectedText = (range?: { start: number; end: number }) =>
      range
        ? {
            ...range,
            expectedText: slice.slice(
              match.offset + range.start,
              match.offset + range.end,
            ),
          }
        : undefined;
    return {
      ...editable,
      graphicsRange: withExpectedText(editable.graphicsRange),
      editables: editable.editables.map((item) => ({
        ...item,
        source: {
          ...item.source,
          itemRange: withExpectedText(item.source.itemRange)!,
          extentRange: withExpectedText(item.source.extentRange),
          pointsRange: withExpectedText(item.source.pointsRange),
          originRange: withExpectedText(item.source.originRange),
          lineColorRange: withExpectedText(item.source.lineColorRange),
          colorRange: withExpectedText(item.source.colorRange),
          textColorRange: withExpectedText(item.source.textColorRange),
          fillColorRange: withExpectedText(item.source.fillColorRange),
          textStringRange: withExpectedText(item.source.textStringRange),
          fontSizeRange: withExpectedText(item.source.fontSizeRange),
          textStyleRange: withExpectedText(item.source.textStyleRange),
          lineThicknessRange: withExpectedText(item.source.lineThicknessRange),
          thicknessRange: withExpectedText(item.source.thicknessRange),
          patternRange: withExpectedText(item.source.patternRange),
          fillPatternRange: withExpectedText(item.source.fillPatternRange),
        },
      })),
    };
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
  const shiftRange = (r?: SourceRangeRef): SourceRangeRef | undefined =>
    r ? { ...r, start: r.start + shift, end: r.end + shift } : undefined;
  const editables = editable.editables.map((e) => {
    return {
      ...e,
      source: {
        itemRange: shiftRange(e.source.itemRange)!,
        extentRange: shiftRange(e.source.extentRange),
        pointsRange: shiftRange(e.source.pointsRange),
        originRange: shiftRange(e.source.originRange),
        lineColorRange: shiftRange(e.source.lineColorRange),
        colorRange: shiftRange(e.source.colorRange),
          textColorRange: shiftRange(e.source.textColorRange),
          fillColorRange: shiftRange(e.source.fillColorRange),
          textStringRange: shiftRange(e.source.textStringRange),
          fontSizeRange: shiftRange(e.source.fontSizeRange),
          textStyleRange: shiftRange(e.source.textStyleRange),
          lineThicknessRange: shiftRange(e.source.lineThicknessRange),
        thicknessRange: shiftRange(e.source.thicknessRange),
        patternRange: shiftRange(e.source.patternRange),
        fillPatternRange: shiftRange(e.source.fillPatternRange),
      },
    };
  });
  return {
    icon: editable.icon,
    graphicsRange: shiftRange(editable.graphicsRange)!,
    editables,
  };
}

// Recursively locate a class by its qualified name inside parsed classes.
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

/** Build a fresh index for a freshly parsed source tree. */
export function buildClassIndex(classes: ClassNode[]): Map<string, ClassNode> {
  const index = new Map<string, ClassNode>();
  const visit = (nodes: ClassNode[]) => {
    for (const node of nodes) {
      index.set(node.qualifiedName, node);
      visit(node.children);
    }
  };
  visit(classes);
  return index;
}

/**
 * Resolve a class after reparsing a source file. Normally the qualified name
 * is sufficient; the unique-leaf fallback covers legacy Modelica files that
 * omit `within` and are qualified by the directory loader instead.
 */
export function findClassByQualifiedNameOrUniqueLeaf(
  classes: ClassNode[],
  qualifiedName: string,
): ClassNode | null {
  const exact = findClassByQualifiedName(classes, qualifiedName);
  if (exact) return exact;
  const leaf = qualifiedName.split(".").at(-1);
  if (!leaf) return null;
  const matches: ClassNode[] = [];
  const visit = (nodes: ClassNode[]) => {
    for (const node of nodes) {
      if (node.name === leaf) matches.push(node);
      visit(node.children);
    }
  };
  visit(classes);
  return matches.length === 1 ? matches[0]! : null;
}
