import {
  parseAnnotationSlice,
  getArg,
  getPositionalArg,
  type AnnotationCall,
  type AnnotationValue,
} from "./annotation.js";
import { tokenize, type Token } from "./lexer.js";
import {
  resolveGraphicCall,
  resolveGraphicsFromCall,
  resolveIconForClass,
  resolveDiagramLayerForClass,
  classOwnedSlice,
  type ExternalClassResolver,
} from "./iconResolver.js";
import { typeNameCandidates } from "./registry.js";
import type { ClassNode } from "./types.js";
import type {
  ComponentInstanceDto,
  DiagramBoundsDto,
  DiagramConnectionDto,
  DiagramSceneDto,
  PlacementDto,
  TransformationDto,
} from "../../shared/modelica.js";
import type {
  CoordinateSystemDto,
  Extent,
  GraphicItemDto,
  LineDto,
  Point,
} from "../../shared/modelicaGraphics.js";
import { transformPlacementPoint } from "../../shared/diagram.js";

const defaultCoordinateSystem: CoordinateSystemDto = {
  extent: { p1: { x: -100, y: -100 }, p2: { x: 100, y: 100 } },
};

type AnnotationOwner = "class" | "component" | "connection" | "extends" | "other";

interface ScopedAnnotation {
  annotation: AnnotationCall;
  tokenIndex: number;
  owner: AnnotationOwner;
}

function asNumber(value: AnnotationValue | undefined): number | undefined {
  return value?.type === "number" ? value.value : undefined;
}

function asBoolean(value: AnnotationValue | undefined): boolean | undefined {
  if (value?.type === "boolean") return value.value;
  if (value?.type === "identifier") {
    if (value.name === "true") return true;
    if (value.name === "false") return false;
  }
  return undefined;
}

function parsePoint(value: AnnotationValue | undefined): Point | undefined {
  if (!value || value.type !== "array" || value.items.length !== 2) return undefined;
  const x = asNumber(value.items[0]);
  const y = asNumber(value.items[1]);
  return x !== undefined && y !== undefined ? { x, y } : undefined;
}

function parseExtent(value: AnnotationValue | undefined): Extent | undefined {
  if (!value || value.type !== "array" || value.items.length !== 2) return undefined;
  const p1 = parsePoint(value.items[0]);
  const p2 = parsePoint(value.items[1]);
  return p1 && p2 ? { p1, p2 } : undefined;
}

function callValue(value: AnnotationValue | undefined, name: string): AnnotationCall | undefined {
  return value?.type === "call" && value.call.name === name ? value.call : undefined;
}

function findCall(call: AnnotationCall, name: string): AnnotationCall | undefined {
  for (const argument of call.arguments) {
    const found = callValue(argument.value, name);
    if (found) return found;
  }
  return undefined;
}

function parseTransformation(call: AnnotationCall | undefined): TransformationDto | undefined {
  if (!call) return undefined;
  return {
    origin: parsePoint(getArg(call, "origin")) ?? { x: 0, y: 0 },
    extent: parseExtent(getArg(call, "extent")) ?? {
      p1: { x: -10, y: -10 },
      p2: { x: 10, y: 10 },
    },
    rotation: asNumber(getArg(call, "rotation")) ?? 0,
  };
}

function parsePlacement(annotation: AnnotationCall): PlacementDto | undefined {
  const placement = findCall(annotation, "Placement");
  if (!placement) return undefined;
  return {
    visible: asBoolean(getArg(placement, "visible")) ?? true,
    transformation: parseTransformation(
      callValue(getArg(placement, "transformation"), "transformation") ??
        callValue(getPositionalArg(placement, 0), "transformation"),
    ),
    iconTransformation: parseTransformation(
      callValue(getArg(placement, "iconTransformation"), "transformation") ??
        callValue(getPositionalArg(placement, 1), "iconTransformation"),
    ),
    iconVisible: asBoolean(getArg(placement, "iconVisible")),
  };
}

const declarationPrefixes = new Set([
  "input", "output", "parameter", "constant", "discrete", "flow", "stream",
  "inner", "outer", "replaceable", "final", "each", "constrainedby",
]);

function qualifiedNameAt(tokens: Token[], start: number): { name: string; end: number } | undefined {
  const first = tokens[start];
  const classHeader = new Set(["package", "model", "block", "connector", "record", "function", "class", "type"]);
  if (!first || first.type !== "IDENT" || declarationPrefixes.has(first.value) || classHeader.has(tokens[start - 1]?.value ?? "")) return undefined;
  const parts = [first.value];
  let end = start;
  while (tokens[end + 1]?.type === "DOT" && tokens[end + 2]?.type === "IDENT") {
    parts.push(tokens[end + 2]!.value);
    end += 2;
  }
  return { name: parts.join("."), end };
}

/** Find the declaration prefix, tolerating nested component modifiers. */
function parseDeclaration(statement: Token[]): { typeName: string; name: string; start: number } | undefined {
  for (let index = 0; index < statement.length; index++) {
    const type = qualifiedNameAt(statement, index);
    if (!type) continue;
    const nameToken = statement[type.end + 1];
    if (!nameToken || nameToken.type !== "IDENT") continue;
    // `model System Pump pump` contains a tempting System/Pump pair. A real
    // declaration is followed by a modifier, annotation, equals, or end of
    // statement rather than another bare identifier.
    const afterName = statement[type.end + 2];
    if (afterName?.type === "IDENT") continue;
    return { typeName: type.name, name: nameToken.value, start: statement[index]!.start };
  }
  return undefined;
}

function statementStartIndex(tokens: Token[], annotationIndex: number): number {
  for (let index = annotationIndex - 1; index >= 0; index--) {
    if (tokens[index]!.type === "SEMICOLON") return index + 1;
  }
  return 0;
}

function classifyAnnotation(tokens: Token[], annotationIndex: number): AnnotationOwner {
  const statement = tokens.slice(statementStartIndex(tokens, annotationIndex), annotationIndex);
  if (statement.some((token) => token.value === "connect")) return "connection";
  if (statement.some((token) => token.value === "extends")) return "extends";
  if (parseDeclaration(statement)) return "component";
  if (statement.length === 0 || statement.some((token) => token.value === "annotation")) {
    return "class";
  }
  return "other";
}

function collectAnnotations(classSlice: string, tokens: Token[]): ScopedAnnotation[] {
  const result: ScopedAnnotation[] = [];
  for (let index = 0; index < tokens.length - 1; index++) {
    const token = tokens[index]!;
    if (token.value !== "annotation" || tokens[index + 1]!.type !== "LPAREN") continue;
    const annotation = parseAnnotationSlice(classSlice.slice(token.start));
    if (!annotation) continue;
    result.push({ annotation, tokenIndex: index, owner: classifyAnnotation(tokens, index) });
  }
  return result;
}

function parseDiagramAnnotation(
  annotations: ScopedAnnotation[],
): { coordinateSystem: CoordinateSystemDto; graphics: GraphicItemDto[] } {
  for (const record of annotations) {
    if (record.owner !== "class") continue;
    const diagram = findCall(record.annotation, "Diagram");
    if (!diagram) continue;
    const coordinateSystemCall =
      callValue(getArg(diagram, "coordinateSystem"), "coordinateSystem") ??
      callValue(getPositionalArg(diagram, 0), "coordinateSystem");
    const coordinateSystem = coordinateSystemCall
      ? {
          extent: parseExtent(getArg(coordinateSystemCall, "extent")) ?? defaultCoordinateSystem.extent,
          preserveAspectRatio: asBoolean(getArg(coordinateSystemCall, "preserveAspectRatio")),
          grid: parsePoint(getArg(coordinateSystemCall, "grid")),
          initialScale: asNumber(getArg(coordinateSystemCall, "initialScale")),
        }
      : defaultCoordinateSystem;
    return { coordinateSystem, graphics: resolveGraphicsFromCall(diagram, "Diagram") };
  }
  return { coordinateSystem: defaultCoordinateSystem, graphics: [] };
}

function componentFromAnnotation(
  classSlice: string,
  tokens: Token[],
  annotationRecord: ScopedAnnotation,
  classNode: ClassNode,
  componentIndex: number,
  resolver: ExternalClassResolver,
): { component?: ComponentInstanceDto; diagnostic?: string } {
  const annotationIndex = annotationRecord.tokenIndex;
  const declaration = parseDeclaration(
    tokens.slice(statementStartIndex(tokens, annotationIndex), annotationIndex),
  );
  const placement = parsePlacement(annotationRecord.annotation);
  if (!declaration || !placement) return {};
  const statementStart = statementStartIndex(tokens, annotationIndex);

  let end = tokens[annotationIndex]!.end;
  for (let index = annotationIndex; index < tokens.length; index++) {
    if (tokens[index]!.type === "SEMICOLON") {
      end = tokens[index]!.end;
      break;
    }
  }
  const component: ComponentInstanceDto = {
    id: `${classNode.qualifiedName}:component:${declaration.name}:${componentIndex}`,
    name: declaration.name,
    typeName: declaration.typeName,
    declaredTypeName: declaration.typeName,
    sourceRange: {
      start: classNode.sourceRange.start + declaration.start,
      end: classNode.sourceRange.start + end,
    },
    placement,
    parameterBindings: parseModifierBindings(
      classSlice,
      tokens,
      statementStart,
      annotationIndex,
      declaration.name,
    ),
  };
  const location = resolver(classNode, declaration.typeName);
  if (!location) {
    return {
      component,
      diagnostic: `COMPONENT_TYPE_UNRESOLVED: ${declaration.name}: declaredTypeName=${JSON.stringify(declaration.typeName)}, lexicalScope=${JSON.stringify(classNode.qualifiedName)}, triedCandidates=${JSON.stringify(typeNameCandidates(classNode, declaration.typeName))}`,
    };
  }
  component.classKind = location.target.kind;
  if (location.target.kind === "connector") {
    const resolved = resolveIconForClass(
      location.target,
      location.allClasses,
      location.source,
      declaration.name,
      new Set<string>(),
      resolver,
    );
    component.resolvedTypeQualifiedName = location.target.qualifiedName;
    if (resolved.icon) component.resolvedIcon = resolved.icon;
    const diagram = resolveDiagramLayerForClass(
      location.target,
      location.source,
      declaration.name,
    );
    if (diagram) component.resolvedDiagram = diagram;
    return {
      component,
      diagnostic: diagram
        ? undefined
        : `${declaration.name}: connector has no Diagram layer; using Icon compatibility fallback`,
    };
  }
  component.resolvedTypeQualifiedName = location.target.qualifiedName;
  const resolved = resolveIconForClass(
    location.target,
    location.allClasses,
    location.source,
    declaration.name,
    new Set<string>(),
    resolver,
  );
  if (resolved.icon) component.resolvedIcon = resolved.icon;
  return {
    component,
    diagnostic: resolved.icon ? undefined : `${declaration.name}: Icon not resolved for ${declaration.typeName}`,
  };
}

function stripComments(value: string): string {
  return value.replace(/\/\/[^\n\r]*|\/\*[\s\S]*?\*\//g, "");
}

function splitTopLevel(value: string): string[] {
  const parts: string[] = [];
  let start = 0;
  let parens = 0;
  let braces = 0;
  let brackets = 0;
  let quoted = false;
  for (let index = 0; index < value.length; index++) {
    const char = value[index]!;
    if (char === '"') {
      if (quoted && value[index + 1] === '"') index++;
      else quoted = !quoted;
      continue;
    }
    if (quoted) continue;
    if (char === "(") parens++;
    else if (char === ")") parens--;
    else if (char === "{") braces++;
    else if (char === "}") braces--;
    else if (char === "[") brackets++;
    else if (char === "]") brackets--;
    else if (char === "," && parens === 0 && braces === 0 && brackets === 0) {
      parts.push(value.slice(start, index));
      start = index + 1;
    }
  }
  parts.push(value.slice(start));
  return parts;
}

function modifierValueText(value: AnnotationValue): string | undefined {
  switch (value.type) {
    case "string": return value.value;
    case "number": return String(value.value);
    case "boolean": return String(value.value);
    case "identifier":
    case "qualifiedName": return value.name;
    default: return undefined;
  }
}

function findClosingParen(value: string, openIndex: number): number | undefined {
  let depth = 0;
  let quoted = false;
  for (let index = openIndex; index < value.length; index++) {
    const char = value[index]!;
    if (char === '"') {
      if (quoted && value[index + 1] === '"') index++;
      else quoted = !quoted;
      continue;
    }
    if (quoted) continue;
    if (char === "(") depth++;
    if (char === ")") {
      depth--;
      if (depth === 0) return index;
    }
  }
  return undefined;
}

function parseModifierBindings(
  classSlice: string,
  tokens: Token[],
  statementStart: number,
  annotationIndex: number,
  instanceName: string,
): Record<string, string> | undefined {
  const startToken = tokens[statementStart];
  const annotationToken = tokens[annotationIndex];
  if (!startToken || !annotationToken) return undefined;
  const statement = classSlice.slice(startToken.start, annotationToken.start);
  const nameOffset = statement.indexOf(instanceName);
  if (nameOffset < 0) return undefined;
  const openIndex = statement.indexOf("(", nameOffset + instanceName.length);
  if (openIndex < 0) return undefined;
  const closeIndex = findClosingParen(statement, openIndex);
  if (closeIndex === undefined) return undefined;
  const modifier = parseAnnotationSlice(`__modifier(${statement.slice(openIndex + 1, closeIndex)})`);
  if (!modifier) return undefined;
  const bindings: Record<string, string> = {};
  for (const argument of modifier.arguments) {
    if (!argument.name) continue;
    const value = modifierValueText(argument.value);
    if (value !== undefined) bindings[argument.name] = value;
  }
  return Object.keys(bindings).length > 0 ? bindings : undefined;
}

function findMatchingCallEnd(tokens: Token[], openIndex: number): number | undefined {
  let depth = 0;
  for (let index = openIndex; index < tokens.length; index++) {
    if (tokens[index]!.type === "LPAREN") depth++;
    if (tokens[index]!.type === "RPAREN") {
      depth--;
      if (depth === 0) return index;
    }
  }
  return undefined;
}

function parseConnection(
  classSlice: string,
  tokens: Token[],
  connectIndex: number,
): DiagramConnectionDto | undefined {
  const openIndex = connectIndex + 1;
  if (tokens[openIndex]?.type !== "LPAREN") return undefined;
  const closeIndex = findMatchingCallEnd(tokens, openIndex);
  if (closeIndex === undefined) return undefined;
  const args = splitTopLevel(
    classSlice.slice(tokens[openIndex]!.end, tokens[closeIndex]!.start),
  ).map((part) => stripComments(part).trim());
  if (args.length !== 2 || !args[0] || !args[1]) return undefined;

  let line: LineDto | undefined;
  let end = tokens[closeIndex]!.end;
  for (let index = closeIndex + 1; index < tokens.length; index++) {
    const token = tokens[index]!;
    if (token.type === "SEMICOLON") {
      end = token.end;
      break;
    }
    if (token.value !== "annotation" || tokens[index + 1]?.type !== "LPAREN") continue;
    const annotation = parseAnnotationSlice(classSlice.slice(token.start));
    const lineCall = annotation && findCall(annotation, "Line");
    const resolved = lineCall ? resolveGraphicCall(lineCall, "Diagram") : null;
    if (resolved?.type === "Line") line = resolved;
    if (annotation) end = token.start + annotation.sourceRange.end;
    break;
  }
  return {
    id: `connection:${connectIndex}`,
    from: args[0],
    to: args[1],
    line,
    sourceRange: { start: tokens[connectIndex]!.start, end },
  };
}

interface MutableBounds { minX: number; minY: number; maxX: number; maxY: number }

function includePoint(bounds: MutableBounds, point: Point): void {
  bounds.minX = Math.min(bounds.minX, point.x);
  bounds.minY = Math.min(bounds.minY, point.y);
  bounds.maxX = Math.max(bounds.maxX, point.x);
  bounds.maxY = Math.max(bounds.maxY, point.y);
}

function includeGraphic(bounds: MutableBounds, graphic: GraphicItemDto): void {
  const origin = "origin" in graphic ? graphic.origin ?? { x: 0, y: 0 } : { x: 0, y: 0 };
  if ("extent" in graphic) {
    includePoint(bounds, { x: graphic.extent.p1.x + origin.x, y: graphic.extent.p1.y + origin.y });
    includePoint(bounds, { x: graphic.extent.p2.x + origin.x, y: graphic.extent.p2.y + origin.y });
  }
  if ("points" in graphic) {
    for (const point of graphic.points) includePoint(bounds, { x: point.x + origin.x, y: point.y + origin.y });
  }
}

function includeComponent(bounds: MutableBounds, component: ComponentInstanceDto): void {
  const transformation = component.placement?.transformation;
  if (!component.placement?.visible || !transformation) return;
  const sourceExtent = component.resolvedIcon?.coordinateSystem.extent ?? transformation.extent;
  for (const point of [
    sourceExtent.p1,
    { x: sourceExtent.p1.x, y: sourceExtent.p2.y },
    { x: sourceExtent.p2.x, y: sourceExtent.p1.y },
    sourceExtent.p2,
  ]) {
    includePoint(bounds, transformPlacementPoint(point, { extent: sourceExtent }, transformation));
  }
}

function finishBounds(bounds: MutableBounds): DiagramBoundsDto | undefined {
  if (!Number.isFinite(bounds.minX)) return undefined;
  return {
    x: bounds.minX,
    y: bounds.minY,
    width: Math.max(bounds.maxX - bounds.minX, 1),
    height: Math.max(bounds.maxY - bounds.minY, 1),
  };
}

export function resolveDiagramForClass(
  classNode: ClassNode,
  source: string,
  resolver: ExternalClassResolver,
): DiagramSceneDto {
  // A class source range includes all nested class declarations. Mask their
  // bodies before collecting annotations/equations so a package (or any class
  // containing nested classes) cannot inherit their Diagram contents merely
  // through a text search over its range.
  const classSlice = classOwnedSlice(classNode, source);
  const tokens = tokenize(classSlice);
  const annotations = collectAnnotations(classSlice, tokens);
  const components: ComponentInstanceDto[] = [];
  const diagnostics: string[] = [];
  let componentIndex = 0;

  for (const record of annotations) {
    if (record.owner !== "component") continue;
    const parsed = componentFromAnnotation(classSlice, tokens, record, classNode, componentIndex, resolver);
    if (!parsed.component) continue;
    componentIndex++;
    components.push(parsed.component);
    if (parsed.diagnostic) diagnostics.push(parsed.diagnostic);
  }

  const diagram = parseDiagramAnnotation(annotations);
  const connections: DiagramConnectionDto[] = [];
  for (let index = 0; index < tokens.length - 1; index++) {
    if (tokens[index]!.value !== "connect") continue;
    const connection = parseConnection(classSlice, tokens, index);
    if (connection) connections.push(connection);
  }

  const mutableBounds: MutableBounds = {
    minX: Number.POSITIVE_INFINITY,
    minY: Number.POSITIVE_INFINITY,
    maxX: Number.NEGATIVE_INFINITY,
    maxY: Number.NEGATIVE_INFINITY,
  };
  for (const graphic of diagram.graphics) includeGraphic(mutableBounds, graphic);
  for (const component of components) includeComponent(mutableBounds, component);
  for (const connection of connections) {
    if (!connection.line) continue;
    const origin = connection.line.origin ?? { x: 0, y: 0 };
    for (const point of connection.line.points) {
      includePoint(mutableBounds, { x: origin.x + point.x, y: origin.y + point.y });
    }
  }

  return {
    classQualifiedName: classNode.qualifiedName,
    classKind: classNode.kind,
    coordinateSystem: diagram.coordinateSystem,
    backgroundGraphics: diagram.graphics,
    components,
    connections,
    diagnostics,
    contentBounds: finishBounds(mutableBounds),
  };
}
