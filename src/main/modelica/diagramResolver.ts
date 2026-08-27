import { parseAnnotationSlice, getArg, getPositionalArg, type AnnotationCall, type AnnotationValue } from "./annotation.js";
import { tokenize, type Token } from "./lexer.js";
import { resolveIconForClass, type ExternalClassResolver } from "./iconResolver.js";
import type { ClassNode } from "./types.js";
import type {
  ComponentInstanceDto,
  DiagramSceneDto,
  PlacementDto,
  TransformationDto,
} from "../../shared/modelica.js";
import type { CoordinateSystemDto, Extent, Point } from "../../shared/modelicaGraphics.js";

const defaultCoordinateSystem: CoordinateSystemDto = {
  extent: { p1: { x: -100, y: -100 }, p2: { x: 100, y: 100 } },
};

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
  const origin = parsePoint(getArg(call, "origin")) ?? { x: 0, y: 0 };
  const extent = parseExtent(getArg(call, "extent")) ?? {
    p1: { x: -10, y: -10 },
    p2: { x: 10, y: 10 },
  };
  return {
    origin,
    extent,
    rotation: asNumber(getArg(call, "rotation")) ?? 0,
  };
}

function parsePlacement(annotation: AnnotationCall): PlacementDto | undefined {
  const placement = findCall(annotation, "Placement");
  if (!placement) return undefined;
  const transformation = parseTransformation(
    callValue(getArg(placement, "transformation"), "transformation") ??
      callValue(getPositionalArg(placement, 0), "transformation"),
  );
  return {
    visible: asBoolean(getArg(placement, "visible")) ?? true,
    transformation,
  };
}

function parseDiagramCoordinateSystem(classSlice: string): CoordinateSystemDto {
  const tokens = tokenize(classSlice);
  for (let index = 0; index < tokens.length - 1; index++) {
    const token = tokens[index]!;
    if (token.value !== "annotation" || tokens[index + 1]!.type !== "LPAREN") continue;
    const annotation = parseAnnotationSlice(classSlice.slice(token.start));
    const diagram = annotation && findCall(annotation, "Diagram");
    if (!diagram) continue;
    const coordinateSystem =
      callValue(getArg(diagram, "coordinateSystem"), "coordinateSystem") ??
      callValue(getPositionalArg(diagram, 0), "coordinateSystem");
    if (!coordinateSystem) return defaultCoordinateSystem;
    return {
      extent: parseExtent(getArg(coordinateSystem, "extent")) ?? defaultCoordinateSystem.extent,
      preserveAspectRatio: asBoolean(getArg(coordinateSystem, "preserveAspectRatio")),
      grid: parsePoint(getArg(coordinateSystem, "grid")),
      initialScale: asNumber(getArg(coordinateSystem, "initialScale")),
    };
  }
  return defaultCoordinateSystem;
}

const declarationPrefixes = new Set([
  "input", "output", "parameter", "constant", "discrete", "flow", "stream",
  "inner", "outer", "replaceable", "final", "each", "constrainedby",
]);

function qualifiedNameAt(tokens: Token[], start: number): { name: string; end: number } | undefined {
  const first = tokens[start];
  if (!first || first.type !== "IDENT" || declarationPrefixes.has(first.value)) return undefined;
  const parts = [first.value];
  let end = start;
  while (tokens[end + 1]?.type === "DOT" && tokens[end + 2]?.type === "IDENT") {
    parts.push(tokens[end + 2]!.value);
    end += 2;
  }
  return { name: parts.join("."), end };
}

function parseDeclaration(statement: Token[]): { typeName: string; name: string; start: number } | undefined {
  let nameIndex = -1;
  for (let index = statement.length - 1; index >= 0; index--) {
    if (statement[index]!.type === "IDENT") {
      nameIndex = index;
      break;
    }
  }
  const nameToken = nameIndex >= 0 ? statement[nameIndex] : undefined;
  if (!nameToken || nameIndex <= 0) return undefined;
  let typeEnd = nameIndex - 1;
  while (typeEnd >= 2 && statement[typeEnd - 1]?.type === "DOT" && statement[typeEnd]?.type === "IDENT") {
    typeEnd -= 2;
  }
  const type = qualifiedNameAt(statement, typeEnd);
  if (!type || type.end !== nameIndex - 1 || declarationPrefixes.has(type.name)) return undefined;
  return { typeName: type.name, name: nameToken.value, start: statement[typeEnd]!.start };
}

function componentFromAnnotation(
  tokens: Token[],
  annotationIndex: number,
  annotation: AnnotationCall,
  classNode: ClassNode,
  componentIndex: number,
  resolver: ExternalClassResolver,
): { component?: ComponentInstanceDto; diagnostic?: string } {
  let statementStart = 0;
  for (let index = annotationIndex - 1; index >= 0; index--) {
    if (tokens[index]!.type === "SEMICOLON") {
      statementStart = index + 1;
      break;
    }
  }
  const declaration = parseDeclaration(tokens.slice(statementStart, annotationIndex));
  const placement = parsePlacement(annotation);
  if (!declaration || !placement) return {};
  let end = tokens[annotationIndex]!.end;
  for (let index = annotationIndex; index < tokens.length; index++) {
    if (tokens[index]!.type === "SEMICOLON") {
      end = tokens[index]!.end;
      break;
    }
  }
  const location = resolver(classNode, declaration.typeName);
  const component: ComponentInstanceDto = {
    id: `${classNode.qualifiedName}:component:${declaration.name}:${componentIndex}`,
    name: declaration.name,
    typeName: declaration.typeName,
    sourceRange: {
      start: classNode.sourceRange.start + declaration.start,
      end: classNode.sourceRange.start + end,
    },
    placement,
  };
  if (!location) {
    return {
      component,
      diagnostic: `${declaration.name}: type ${declaration.typeName} could not be resolved`,
    };
  }
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

export function resolveDiagramForClass(
  classNode: ClassNode,
  source: string,
  resolver: ExternalClassResolver,
): DiagramSceneDto {
  const classSlice = source.slice(classNode.sourceRange.start, classNode.sourceRange.end);
  const tokens = tokenize(classSlice);
  const components: ComponentInstanceDto[] = [];
  const diagnostics: string[] = [];
  let componentIndex = 0;
  for (let index = 0; index < tokens.length - 1; index++) {
    const token = tokens[index]!;
    if (token.value !== "annotation" || tokens[index + 1]!.type !== "LPAREN") continue;
    const annotation = parseAnnotationSlice(classSlice.slice(token.start));
    if (!annotation) continue;
    const parsed = componentFromAnnotation(tokens, index, annotation, classNode, componentIndex, resolver);
    if (!parsed.component) continue;
    componentIndex++;
    components.push(parsed.component);
    if (parsed.diagnostic) diagnostics.push(parsed.diagnostic);
  }
  const placedNames = new Set(components.map((component) => component.name));
  let statementStart = 0;
  for (let index = 0; index < tokens.length; index++) {
    if (tokens[index]!.type !== "SEMICOLON") continue;
    const statement = tokens.slice(statementStart, index);
    statementStart = index + 1;
    if (statement.some((token) => token.value === "annotation")) continue;
    const declaration = parseDeclaration(statement);
    if (!declaration || placedNames.has(declaration.name)) continue;
    const location = resolver(classNode, declaration.typeName);
    if (!location) continue;
    const component: ComponentInstanceDto = {
      id: `${classNode.qualifiedName}:component:${declaration.name}:${componentIndex}`,
      name: declaration.name,
      typeName: declaration.typeName,
      sourceRange: {
        start: classNode.sourceRange.start + declaration.start,
        end: classNode.sourceRange.start + tokens[index]!.end,
      },
    };
    const resolved = resolveIconForClass(
      location.target,
      location.allClasses,
      location.source,
      declaration.name,
      new Set<string>(),
      resolver,
    );
    if (resolved.icon) component.resolvedIcon = resolved.icon;
    components.push(component);
    diagnostics.push(`${declaration.name}: Component has no Placement annotation`);
    componentIndex++;
  }
  return {
    coordinateSystem: parseDiagramCoordinateSystem(classSlice),
    components,
    diagnostics,
  };
}
