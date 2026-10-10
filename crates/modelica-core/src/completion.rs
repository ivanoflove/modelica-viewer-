//! Lightweight Modelica completion primitives.
//!
//! This module deliberately performs prefix lookup over an already-built
//! catalog. It does not parse the whole document or load libraries while the
//! user types; callers provide the classes currently indexed by a registry.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompletionKind {
    Keyword,
    BuiltinType,
    BuiltinFunction,
    Class,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
    pub detail: String,
    pub kind: CompletionKind,
    pub cursor_offset: Option<usize>,
    qualified_name: Option<String>,
    search_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionContext {
    pub prefix: String,
    /// Byte offsets in the current line, suitable for a UTF-8 string range.
    pub replace_start: usize,
    pub replace_end: usize,
    /// Present after a qualified-name dot, for example `Modelica.Blocks`.
    pub qualifier: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ModelicaCompletionIndex {
    items: Vec<CompletionItem>,
    namespace_members: HashMap<String, Vec<CompletionItem>>,
}

impl ModelicaCompletionIndex {
    pub fn new(class_names: impl IntoIterator<Item = String>) -> Self {
        let mut items = builtin_items();
        let mut namespace_members = HashMap::<String, Vec<CompletionItem>>::new();
        for qualified_name in class_names {
            let Some(label) = qualified_name.rsplit('.').next() else {
                continue;
            };
            if label.is_empty() {
                continue;
            }
            let item = CompletionItem {
                label: label.to_owned(),
                insert_text: label.to_owned(),
                detail: qualified_name.clone(),
                kind: CompletionKind::Class,
                cursor_offset: None,
                search_key: label.to_lowercase(),
                qualified_name: Some(qualified_name.clone()),
            };
            if let Some((namespace, _)) = qualified_name.rsplit_once('.') {
                namespace_members
                    .entry(namespace.to_owned())
                    .or_default()
                    .push(item.clone());
            }
            items.push(item);
        }
        items.sort_by(|left, right| {
            left.search_key
                .cmp(&right.search_key)
                .then_with(|| {
                    completion_kind_rank(left.kind).cmp(&completion_kind_rank(right.kind))
                })
                .then_with(|| left.detail.cmp(&right.detail))
        });
        items.dedup_by(|right, left| {
            right.kind == left.kind && right.label == left.label && right.detail == left.detail
        });
        for members in namespace_members.values_mut() {
            members.sort_by(|left, right| {
                left.search_key
                    .cmp(&right.search_key)
                    .then_with(|| left.detail.cmp(&right.detail))
            });
            members.dedup_by(|right, left| right.detail == left.detail);
        }
        Self {
            items,
            namespace_members,
        }
    }

    pub fn complete(
        &self,
        context: &CompletionContext,
        forced: bool,
        limit: usize,
    ) -> Vec<CompletionItem> {
        if context.prefix.is_empty() && !forced && context.qualifier.is_none() {
            return Vec::new();
        }
        let prefix_key = context.prefix.to_lowercase();
        let source = if let Some(qualifier) = context.qualifier.as_deref() {
            self.namespace_members
                .get(qualifier)
                .map(Vec::as_slice)
                .unwrap_or_default()
        } else {
            self.items.as_slice()
        };
        let start = source.partition_point(|item| item.search_key < prefix_key);
        let mut candidates = source[start..]
            .iter()
            .take_while(|item| item.search_key.starts_with(&prefix_key))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            let left_exact_case = !left.label.starts_with(&context.prefix);
            let right_exact_case = !right.label.starts_with(&context.prefix);
            left_exact_case
                .cmp(&right_exact_case)
                .then_with(|| {
                    completion_kind_rank(left.kind).cmp(&completion_kind_rank(right.kind))
                })
                .then_with(|| left.search_key.cmp(&right.search_key))
                .then_with(|| left.detail.cmp(&right.detail))
        });
        candidates.truncate(limit);
        candidates
    }
}

/// Finds the identifier at the caret, unless it is inside a Modelica comment
/// or string. `block_comment_open` is the lexical state at the beginning of
/// this line and lets editor callers cache comment state incrementally.
pub fn analyze_completion_context(
    line: &str,
    cursor_byte: usize,
    block_comment_open: bool,
) -> Option<CompletionContext> {
    if cursor_byte > line.len() || !line.is_char_boundary(cursor_byte) {
        return None;
    }
    if is_suppressed_context(&line[..cursor_byte], block_comment_open) {
        return None;
    }

    let bytes = line.as_bytes();
    let mut prefix_start = cursor_byte;
    while prefix_start > 0 && is_identifier_byte(bytes[prefix_start - 1]) {
        prefix_start -= 1;
    }
    let prefix = line[prefix_start..cursor_byte].to_owned();

    let mut qualifier = None;
    if prefix_start > 0 && bytes[prefix_start - 1] == b'.' {
        let mut chain_start = prefix_start - 1;
        while chain_start > 0 {
            let byte = bytes[chain_start - 1];
            if is_identifier_byte(byte) || byte == b'.' {
                chain_start -= 1;
            } else {
                break;
            }
        }
        let chain = &line[chain_start..prefix_start - 1];
        if !chain.is_empty()
            && chain
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(is_identifier_byte))
        {
            qualifier = Some(chain.to_owned());
        }
    }

    Some(CompletionContext {
        prefix,
        replace_start: prefix_start,
        replace_end: cursor_byte,
        qualifier,
    })
}

fn is_suppressed_context(prefix: &str, mut block_comment_open: bool) -> bool {
    let bytes = prefix.as_bytes();
    let mut index = 0;
    let mut in_string = false;
    while index < bytes.len() {
        if block_comment_open {
            if bytes.get(index..index + 2) == Some(b"*/") {
                block_comment_open = false;
                index += 2;
            } else {
                index += 1;
            }
        } else if in_string {
            if bytes[index] == b'\\' {
                index = (index + 2).min(bytes.len());
            } else if bytes[index] == b'"' {
                if bytes.get(index + 1) == Some(&b'"') {
                    index += 2;
                } else {
                    in_string = false;
                    index += 1;
                }
            } else {
                index += 1;
            }
        } else if bytes.get(index..index + 2) == Some(b"//") {
            return true;
        } else if bytes.get(index..index + 2) == Some(b"/*") {
            block_comment_open = true;
            index += 2;
        } else if bytes[index] == b'"' {
            in_string = true;
            index += 1;
        } else {
            index += prefix[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    block_comment_open || in_string
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn completion_kind_rank(kind: CompletionKind) -> u8 {
    match kind {
        CompletionKind::Keyword => 0,
        CompletionKind::BuiltinType => 1,
        CompletionKind::BuiltinFunction => 2,
        CompletionKind::Class => 3,
    }
}

fn builtin_items() -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for name in [
        "algorithm",
        "and",
        "annotation",
        "assert",
        "block",
        "break",
        "class",
        "connect",
        "connector",
        "constant",
        "discrete",
        "else",
        "elseif",
        "elsewhen",
        "end",
        "equation",
        "expandable connector",
        "extends",
        "external",
        "final",
        "for",
        "function",
        "if",
        "import",
        "impure",
        "in",
        "initial equation",
        "inner",
        "input",
        "loop",
        "model",
        "not",
        "operator function",
        "operator record",
        "or",
        "outer",
        "output",
        "package",
        "parameter",
        "partial",
        "protected",
        "public",
        "pure",
        "record",
        "redeclare",
        "reinit",
        "replaceable",
        "return",
        "stream",
        "then",
        "type",
        "when",
        "while",
        "within",
    ] {
        items.push(item(name, CompletionKind::Keyword, "Modelica 关键字"));
    }
    for name in ["Boolean", "Integer", "Real", "String"] {
        items.push(item(name, CompletionKind::BuiltinType, "内建类型"));
    }
    for name in [
        "abs",
        "acos",
        "asin",
        "atan",
        "atan2",
        "ceil",
        "cos",
        "cosh",
        "der",
        "exp",
        "floor",
        "integer",
        "log",
        "log10",
        "max",
        "min",
        "mod",
        "rem",
        "sign",
        "sin",
        "sinh",
        "size",
        "sqrt",
        "sum",
        "tan",
        "tanh",
        "transpose",
        " noEvent",
    ] {
        let name = name.trim();
        let mut candidate = item(name, CompletionKind::BuiltinFunction, "内建函数");
        if name == "der" {
            candidate.insert_text = "der()".to_owned();
            candidate.cursor_offset = Some(4);
            candidate.detail = "内建函数 · der(变量)".to_owned();
        }
        items.push(candidate);
    }
    items
}

fn item(name: &str, kind: CompletionKind, detail: &str) -> CompletionItem {
    CompletionItem {
        label: name.to_owned(),
        insert_text: name.to_owned(),
        detail: detail.to_owned(),
        kind,
        cursor_offset: None,
        qualified_name: None,
        search_key: name.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::{CompletionKind, ModelicaCompletionIndex, analyze_completion_context};

    #[test]
    fn prefix_completion_returns_modelica_keywords_and_types() {
        let index = ModelicaCompletionIndex::new(Vec::<String>::new());
        for (prefix, expected) in [
            ("mod", "model"),
            ("par", "parameter"),
            ("con", "connect"),
            ("Re", "Real"),
            ("der", "der"),
        ] {
            let context = analyze_completion_context(prefix, prefix.len(), false).unwrap();
            let candidates = index.complete(&context, false, 64);
            assert!(
                candidates
                    .iter()
                    .any(|candidate| candidate.label == expected)
            );
        }
    }

    #[test]
    fn namespace_completion_lists_only_loaded_direct_members() {
        let index = ModelicaCompletionIndex::new([
            "Modelica.Blocks.Sources.Constant".to_owned(),
            "Modelica.Blocks.Sources.Step".to_owned(),
            "Modelica.Blocks.Math.Gain".to_owned(),
            "Modelica.Unloaded.Fake".to_owned(),
        ]);
        let source = "Modelica.Blocks.Sources.St";
        let context = analyze_completion_context(source, source.len(), false).unwrap();
        let candidates = index.complete(&context, false, 64);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].label, "Step");
        assert_eq!(candidates[0].detail, "Modelica.Blocks.Sources.Step");
        assert_eq!(candidates[0].kind, CompletionKind::Class);
    }

    #[test]
    fn completion_is_suppressed_inside_comments_and_strings() {
        assert!(analyze_completion_context("// mod", 6, false).is_none());
        assert!(analyze_completion_context("model A \"mod", 12, false).is_none());
        assert!(analyze_completion_context("mod", 3, true).is_none());
        assert!(analyze_completion_context("/* mod */ con", 13, false).is_some());
    }

    #[test]
    fn context_keeps_utf8_safe_byte_ranges_and_qualified_names() {
        let source = "  名称 = Modelica.Blocks.Sou";
        let context = analyze_completion_context(source, source.len(), false).unwrap();
        assert_eq!(context.prefix, "Sou");
        assert_eq!(context.qualifier.as_deref(), Some("Modelica.Blocks"));
        assert_eq!(&source[context.replace_start..context.replace_end], "Sou");
        assert!(analyze_completion_context("x", 2, false).is_none());
    }
}
