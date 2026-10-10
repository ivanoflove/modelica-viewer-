//! Pure validation and planning for creating Modelica classes.
//!
//! This module deliberately has no UI dependency. A caller supplies the
//! visible class/file context, receives a complete plan, then explicitly
//! applies that plan with [`apply_new_class_plan`].

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ast::{Class, ClassKind};
use crate::lexer::{TokenKind, tokenize};
use crate::parser::parse;
use crate::source::{SourceEdit, SourceTransaction, apply_source_transaction};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NewClassStorageMode {
    /// Create a single `.mo` file, optionally as a member of a known package.
    SingleFile {
        directory: PathBuf,
        within: Option<String>,
        package_order_file: Option<PathBuf>,
        package_order_before: Option<String>,
    },
    /// Create `Name/package.mo` under `parent_directory`.
    DirectoryPackage {
        parent_directory: PathBuf,
        within: Option<String>,
        package_order_file: Option<PathBuf>,
        package_order_before: Option<String>,
    },
    /// Add a nested class to an existing long-form class in a `.mo` file.
    InsertIntoExistingFile {
        file: PathBuf,
        parent_class: String,
        package_order_file: Option<PathBuf>,
        package_order_before: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewClassRequest {
    /// Modelica identifier lexeme, including quotes for a quoted identifier.
    pub name: String,
    pub kind: ClassKind,
    pub description: Option<String>,
    pub partial: bool,
    pub base_class: Option<String>,
    pub storage: NewClassStorageMode,
}

#[derive(Clone, Debug, Default)]
pub struct NewClassContext {
    /// Known classes keyed by their exact Modelica qualified-name lexeme.
    pub class_kinds: HashMap<String, ClassKind>,
    /// Existing names in the destination scope. When `scope_is_explicit` is
    /// true, this separates same-scope collision checks from the broader
    /// class index used to resolve base classes.
    pub scope_class_names: HashSet<String>,
    pub scope_is_explicit: bool,
    /// Current source contents, keyed by absolute or caller-consistent path.
    pub source_files: HashMap<PathBuf, String>,
    /// Existing package.order contents. Only explicitly selected files are
    /// changed; a missing package.order is never created implicitly.
    pub package_order_files: HashMap<PathBuf, String>,
    /// Existing files/directories used for an early collision preview.
    /// Disk state is checked again immediately before writing.
    pub occupied_paths: HashSet<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewClassField {
    Name,
    Kind,
    Description,
    BaseClass,
    Storage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewClassValidationError {
    pub field: NewClassField,
    pub message: String,
}

impl NewClassValidationError {
    fn new(field: NewClassField, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for NewClassValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NewClassValidationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewClassFileChange {
    pub path: PathBuf,
    /// `None` means create-only; `Some` means compare-and-replace.
    pub expected_contents: Option<String>,
    pub replacement_contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewClassPlan {
    pub qualified_name: String,
    pub primary_file: PathBuf,
    /// Path accepted by `PackageLoader::load` to display the new class.
    pub reload_path: PathBuf,
    /// All planned files must remain below this selected/trusted directory.
    pub authorized_root: PathBuf,
    /// Generated declaration as shown in the preview.
    pub source_preview: String,
    /// New class file first, package.order update last where applicable.
    pub changes: Vec<NewClassFileChange>,
}

/// Validate a Modelica identifier lexeme, including a supported printable
/// ASCII quoted-identifier subset. The quote delimiters are retained as part
/// of the identifier, as required by Modelica lexical identity.
pub fn validate_modelica_identifier(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("名称不能为空".to_owned());
    }
    if value.starts_with('\'') {
        return validate_quoted_identifier(value);
    }
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err("名称不能为空".to_owned());
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err("名称必须符合 Modelica 标识符规则；特殊名称请使用单引号括起".to_owned());
    }
    if is_reserved_identifier(value) {
        return Err(format!("`{value}` 是 Modelica 保留字或预定义类型名"));
    }
    Ok(())
}

fn validate_quoted_identifier(value: &str) -> Result<(), String> {
    let Some(body) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) else {
        return Err("带引号标识符必须有成对的单引号".to_owned());
    };
    if body.is_empty() {
        return Err("带引号标识符不能为空".to_owned());
    }
    let bytes = body.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(0x20..=0x7e).contains(&byte) {
            return Err("当前仅支持可打印 ASCII 字符的带引号标识符".to_owned());
        }
        if byte == b'\'' {
            return Err("带引号标识符中的单引号必须写成 \\'".to_owned());
        }
        if byte == b'\\' && bytes.get(index + 1) == Some(&b'\'') {
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn is_reserved_identifier(value: &str) -> bool {
    matches!(
        value,
        "algorithm"
            | "and"
            | "annotation"
            | "block"
            | "break"
            | "class"
            | "connect"
            | "connector"
            | "constant"
            | "der"
            | "discrete"
            | "each"
            | "else"
            | "elseif"
            | "elsewhen"
            | "encapsulated"
            | "end"
            | "enumeration"
            | "equation"
            | "expandable"
            | "extends"
            | "external"
            | "false"
            | "final"
            | "flow"
            | "for"
            | "function"
            | "if"
            | "impure"
            | "import"
            | "in"
            | "initial"
            | "inner"
            | "input"
            | "loop"
            | "model"
            | "not"
            | "operator"
            | "or"
            | "outer"
            | "output"
            | "package"
            | "parameter"
            | "partial"
            | "protected"
            | "public"
            | "pure"
            | "record"
            | "redeclare"
            | "replaceable"
            | "return"
            | "stream"
            | "then"
            | "time"
            | "true"
            | "type"
            | "when"
            | "while"
            | "within"
            | "Real"
            | "Integer"
            | "Boolean"
            | "String"
    )
}

fn validate_qualified_name(
    value: &str,
    field: NewClassField,
) -> Result<(), NewClassValidationError> {
    if value.is_empty() {
        return Err(NewClassValidationError::new(field, "名称不能为空"));
    }
    let mut start = 0;
    let bytes = value.as_bytes();
    while start < bytes.len() {
        let end = if bytes[start] == b'\'' {
            let mut cursor = start + 1;
            let mut found_end = None;
            while cursor < bytes.len() {
                if bytes[cursor] == b'\\' && bytes.get(cursor + 1) == Some(&b'\'') {
                    cursor += 2;
                } else if bytes[cursor] == b'\'' {
                    found_end = Some(cursor + 1);
                    break;
                } else {
                    cursor += 1;
                }
            }
            found_end.ok_or_else(|| {
                NewClassValidationError::new(field, "带引号标识符必须有成对的单引号")
            })?
        } else {
            value[start..]
                .find('.')
                .map_or(value.len(), |offset| start + offset)
        };
        validate_modelica_identifier(&value[start..end])
            .map_err(|error| NewClassValidationError::new(field, error))?;
        if end == bytes.len() {
            return Ok(());
        }
        if bytes[end] != b'.' || end + 1 == bytes.len() {
            return Err(NewClassValidationError::new(
                field,
                "限定名称必须由合法标识符以点号分隔",
            ));
        }
        start = end + 1;
    }
    Err(NewClassValidationError::new(
        field,
        "限定名称必须由合法标识符以点号分隔",
    ))
}

fn supported_kind(kind: ClassKind) -> bool {
    matches!(
        kind,
        ClassKind::Package
            | ClassKind::Model
            | ClassKind::Block
            | ClassKind::Connector
            | ClassKind::ExpandableConnector
            | ClassKind::Record
            | ClassKind::Function
            | ClassKind::Type
            | ClassKind::Class
            | ClassKind::OperatorRecord
            | ClassKind::OperatorFunction
    )
}

fn class_kind_label(kind: ClassKind) -> &'static str {
    match kind {
        ClassKind::Package => "package",
        ClassKind::Model => "model",
        ClassKind::Block => "block",
        ClassKind::Connector => "connector",
        ClassKind::ExpandableConnector => "expandable connector",
        ClassKind::Record => "record",
        ClassKind::Function => "function",
        ClassKind::Type => "type",
        ClassKind::Class => "class",
        ClassKind::OperatorRecord => "operator record",
        ClassKind::OperatorFunction => "operator function",
        ClassKind::Operator => "operator",
    }
}

fn file_component(identifier: &str) -> Result<String, NewClassValidationError> {
    let name = if identifier.starts_with('\'') {
        identifier
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
            .expect("identifier was validated")
            .replace("\\'", "'")
    } else {
        identifier.to_owned()
    };
    let invalid = name.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
    });
    let stem = name.split('.').next().unwrap_or_default();
    let windows_device = matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if name.ends_with([' ', '.']) || windows_device || invalid {
        return Err(NewClassValidationError::new(
            NewClassField::Storage,
            "该合法 Modelica 名称不能安全映射为跨平台文件名；请改用插入现有文件方式",
        ));
    }
    Ok(name)
}

fn qualified_name_for(request: &NewClassRequest) -> Result<String, NewClassValidationError> {
    match &request.storage {
        NewClassStorageMode::SingleFile { within, .. }
        | NewClassStorageMode::DirectoryPackage { within, .. } => {
            if let Some(within) = within {
                validate_qualified_name(within, NewClassField::Storage)?;
                Ok(format!("{within}.{}", request.name))
            } else {
                Ok(request.name.clone())
            }
        }
        NewClassStorageMode::InsertIntoExistingFile { parent_class, .. } => {
            validate_qualified_name(parent_class, NewClassField::Storage)?;
            Ok(format!("{parent_class}.{}", request.name))
        }
    }
}

fn resolved_base_kind(
    request: &NewClassRequest,
    context: &NewClassContext,
    base: &str,
) -> Option<ClassKind> {
    if matches!(base, "Real" | "Integer" | "Boolean" | "String") {
        return Some(ClassKind::Type);
    }
    if let Some(kind) = context.class_kinds.get(base) {
        return Some(*kind);
    }
    let scope = match &request.storage {
        NewClassStorageMode::SingleFile { within, .. }
        | NewClassStorageMode::DirectoryPackage { within, .. } => within.as_deref(),
        NewClassStorageMode::InsertIntoExistingFile { parent_class, .. } => {
            Some(parent_class.as_str())
        }
    };
    scope
        .and_then(|scope| context.class_kinds.get(&format!("{scope}.{base}")))
        .copied()
}

fn base_kind_compatible(target: ClassKind, base: ClassKind) -> bool {
    match target {
        ClassKind::Class => base != ClassKind::Package && base != ClassKind::Operator,
        ClassKind::Package => base == ClassKind::Package,
        ClassKind::Model => base == ClassKind::Model,
        ClassKind::Block => base == ClassKind::Block,
        ClassKind::Connector => {
            matches!(base, ClassKind::Connector | ClassKind::ExpandableConnector)
        }
        ClassKind::ExpandableConnector => {
            matches!(base, ClassKind::Connector | ClassKind::ExpandableConnector)
        }
        ClassKind::Record => base == ClassKind::Record,
        ClassKind::Function => base == ClassKind::Function,
        ClassKind::Type => base == ClassKind::Type,
        ClassKind::OperatorRecord => base == ClassKind::OperatorRecord,
        ClassKind::OperatorFunction => false,
        ClassKind::Operator => false,
    }
}

/// Validate the request against Modelica lexical rules and the caller's
/// current class index. Returns its exact qualified-name lexeme.
pub fn validate_new_class(
    request: &NewClassRequest,
    context: &NewClassContext,
) -> Result<String, NewClassValidationError> {
    validate_modelica_identifier(&request.name)
        .map_err(|error| NewClassValidationError::new(NewClassField::Name, error))?;
    if !supported_kind(request.kind) {
        return Err(NewClassValidationError::new(
            NewClassField::Kind,
            "该类别当前不受新建功能支持",
        ));
    }
    if request.kind == ClassKind::Type && request.base_class.as_deref().is_none_or(str::is_empty) {
        return Err(NewClassValidationError::new(
            NewClassField::BaseClass,
            "Type 必须指定 Real、Integer、Boolean、String 或已知类型作为基类",
        ));
    }
    if let Some(description) = &request.description
        && description.chars().any(|character| {
            character.is_control()
                && !matches!(
                    character,
                    '\n' | '\r' | '\t' | '\u{0007}' | '\u{0008}' | '\u{000b}' | '\u{000c}'
                )
        })
    {
        return Err(NewClassValidationError::new(
            NewClassField::Description,
            "描述包含 Modelica 字符串不支持的控制字符",
        ));
    }

    if let Some(base) = request
        .base_class
        .as_deref()
        .filter(|base| !base.trim().is_empty())
    {
        if !matches!(base, "Real" | "Integer" | "Boolean" | "String") {
            validate_qualified_name(base, NewClassField::BaseClass)?;
        }
        let Some(base_kind) = resolved_base_kind(request, context, base) else {
            return Err(NewClassValidationError::new(
                NewClassField::BaseClass,
                format!("无法在当前模型库中解析基类 `{base}`，请先加载其所在库"),
            ));
        };
        if !base_kind_compatible(request.kind, base_kind) {
            return Err(NewClassValidationError::new(
                NewClassField::BaseClass,
                format!(
                    "类别 `{}` 不能继承类别 `{}`",
                    class_kind_label(request.kind),
                    class_kind_label(base_kind)
                ),
            ));
        }
    }

    if request.kind == ClassKind::OperatorFunction {
        let NewClassStorageMode::InsertIntoExistingFile { parent_class, .. } = &request.storage
        else {
            return Err(NewClassValidationError::new(
                NewClassField::Storage,
                "Operator Function 必须直接插入现有 Operator Record 中",
            ));
        };
        if context.class_kinds.get(parent_class) != Some(&ClassKind::OperatorRecord) {
            return Err(NewClassValidationError::new(
                NewClassField::Storage,
                "Operator Function 的父类必须是已解析的 Operator Record",
            ));
        }
    }
    if matches!(
        request.kind,
        ClassKind::Operator | ClassKind::OperatorFunction
    ) && request
        .base_class
        .as_deref()
        .is_some_and(|base| !base.trim().is_empty())
    {
        return Err(NewClassValidationError::new(
            NewClassField::BaseClass,
            "Operator 类别不支持此处的基类字段",
        ));
    }
    if let NewClassStorageMode::DirectoryPackage { .. } = request.storage
        && request.kind != ClassKind::Package
    {
        return Err(NewClassValidationError::new(
            NewClassField::Storage,
            "目录式 Package 只能用于 Package 类别",
        ));
    }

    let within = match &request.storage {
        NewClassStorageMode::SingleFile { within, .. }
        | NewClassStorageMode::DirectoryPackage { within, .. } => within.as_deref(),
        NewClassStorageMode::InsertIntoExistingFile { .. } => None,
    };
    if let Some(within) = within
        && context.class_kinds.get(within) != Some(&ClassKind::Package)
    {
        return Err(NewClassValidationError::new(
            NewClassField::Storage,
            format!("within 目标 `{within}` 不是当前已加载的 Package"),
        ));
    }

    if !matches!(
        request.storage,
        NewClassStorageMode::InsertIntoExistingFile { .. }
    ) {
        file_component(&request.name)?;
    }
    let qualified_name = qualified_name_for(request)?;
    let name_exists = if context.scope_is_explicit {
        context.scope_class_names.contains(&qualified_name)
    } else {
        context.class_kinds.contains_key(&qualified_name)
    };
    if name_exists {
        return Err(NewClassValidationError::new(
            NewClassField::Name,
            format!("同一作用域中已经存在 `{qualified_name}`"),
        ));
    }
    Ok(qualified_name)
}

/// Generate a minimal long class or a short `type` definition. `within` is
/// emitted only for file-based package members, never for nested classes.
pub fn generate_modelica_source(
    request: &NewClassRequest,
    within: Option<&str>,
) -> Result<String, NewClassValidationError> {
    if let Some(within) = within {
        validate_qualified_name(within, NewClassField::Storage)?;
    }
    let description = request
        .description
        .as_deref()
        .filter(|description| !description.is_empty())
        .map(modelica_string_literal)
        .transpose()?
        .map(|literal| format!(" {literal}"))
        .unwrap_or_default();
    let partial = if request.partial { "partial " } else { "" };
    let class_keyword = class_kind_label(request.kind);
    let mut source = String::new();
    if let Some(within) = within {
        source.push_str(&format!("within {within};\n"));
    }
    if request.kind == ClassKind::Type {
        let base = request
            .base_class
            .as_deref()
            .filter(|base| !base.trim().is_empty())
            .ok_or_else(|| {
                NewClassValidationError::new(NewClassField::BaseClass, "Type 必须指定基类")
            })?;
        source.push_str(&format!(
            "{partial}type {} = {base}{description};\n",
            request.name
        ));
        return Ok(source);
    }
    source.push_str(&format!(
        "{partial}{class_keyword} {}{description}\n",
        request.name
    ));
    if let Some(base) = request
        .base_class
        .as_deref()
        .filter(|base| !base.trim().is_empty())
    {
        source.push_str(&format!("  extends {base};\n"));
    }
    source.push_str(&format!("end {};\n", request.name));
    Ok(source)
}

fn modelica_string_literal(value: &str) -> Result<String, NewClassValidationError> {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        escaped.push_str(match character {
            '\'' => "\\'",
            '"' => "\\\"",
            '?' => "\\?",
            '\\' => "\\\\",
            '\u{0007}' => "\\a",
            '\u{0008}' => "\\b",
            '\u{000c}' => "\\f",
            '\n' => "\\n",
            '\r' => "\\r",
            '\t' => "\\t",
            '\u{000b}' => "\\v",
            value if value.is_control() => {
                return Err(NewClassValidationError::new(
                    NewClassField::Description,
                    "描述包含无法写成 Modelica 字符串的控制字符",
                ));
            }
            value => {
                escaped.push(value);
                continue;
            }
        });
    }
    escaped.push('"');
    Ok(escaped)
}

fn package_order_contents(
    original: &str,
    member: &str,
    before: Option<&str>,
) -> Result<String, NewClassValidationError> {
    let line_ending = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines = original
        .split_inclusive('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if !original.is_empty() && !original.ends_with('\n') && lines.is_empty() {
        lines.push(original.to_owned());
    }
    let entry_name = |line: &str| {
        line.split("//")
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    if lines.iter().any(|line| entry_name(line) == member) {
        return Err(NewClassValidationError::new(
            NewClassField::Storage,
            format!("package.order 已包含 `{member}`"),
        ));
    }
    let insert_at = if let Some(before) = before {
        lines
            .iter()
            .position(|line| entry_name(line) == before)
            .ok_or_else(|| {
                NewClassValidationError::new(
                    NewClassField::Storage,
                    format!("package.order 中找不到插入位置 `{before}`"),
                )
            })?
    } else {
        lines
            .iter()
            .rposition(|line| !entry_name(line).is_empty())
            .map_or(lines.len(), |index| index + 1)
    };
    if insert_at > 0 && !lines[insert_at - 1].ends_with('\n') {
        lines[insert_at - 1].push_str(line_ending);
    }
    lines.insert(insert_at, format!("{member}{line_ending}"));
    Ok(lines.concat())
}

/// Build a complete, side-effect-free file plan. Every existing source and
/// package-order file is snapshotted in `context` and revalidated on apply.
pub fn plan_new_class(
    request: &NewClassRequest,
    context: &NewClassContext,
) -> Result<NewClassPlan, NewClassValidationError> {
    let qualified_name = validate_new_class(request, context)?;
    match &request.storage {
        NewClassStorageMode::SingleFile {
            directory,
            within,
            package_order_file,
            package_order_before,
        } => {
            let component = file_component(&request.name)?;
            let primary_file = directory.join(format!("{component}.mo"));
            reject_occupied_path(&primary_file, context)?;
            let source_preview = generate_modelica_source(request, within.as_deref())?;
            let mut changes = vec![NewClassFileChange {
                path: primary_file.clone(),
                expected_contents: None,
                replacement_contents: source_preview.clone(),
            }];
            add_package_order_change(
                &mut changes,
                package_order_file.as_deref(),
                package_order_before.as_deref(),
                &request.name,
                context,
            )?;
            Ok(NewClassPlan {
                qualified_name,
                primary_file: primary_file.clone(),
                reload_path: primary_file,
                authorized_root: directory.clone(),
                source_preview,
                changes,
            })
        }
        NewClassStorageMode::DirectoryPackage {
            parent_directory,
            within,
            package_order_file,
            package_order_before,
        } => {
            let component = file_component(&request.name)?;
            let package_directory = parent_directory.join(component);
            reject_occupied_path(&package_directory, context)?;
            let primary_file = package_directory.join("package.mo");
            let source_preview = generate_modelica_source(request, within.as_deref())?;
            let mut changes = vec![NewClassFileChange {
                path: primary_file.clone(),
                expected_contents: None,
                replacement_contents: source_preview.clone(),
            }];
            add_package_order_change(
                &mut changes,
                package_order_file.as_deref(),
                package_order_before.as_deref(),
                &request.name,
                context,
            )?;
            Ok(NewClassPlan {
                qualified_name,
                primary_file,
                reload_path: package_directory,
                authorized_root: parent_directory.clone(),
                source_preview,
                changes,
            })
        }
        NewClassStorageMode::InsertIntoExistingFile {
            file,
            parent_class,
            package_order_file,
            package_order_before,
        } => {
            let original = context.source_files.get(file).ok_or_else(|| {
                NewClassValidationError::new(
                    NewClassField::Storage,
                    format!("未能读取父类源文件：{}", file.display()),
                )
            })?;
            let parsed = parse(original, file).map_err(|error| {
                NewClassValidationError::new(
                    NewClassField::Storage,
                    format!("父类文件无法解析：{error}"),
                )
            })?;
            let class = find_class(&parsed.classes, parent_class).ok_or_else(|| {
                NewClassValidationError::new(
                    NewClassField::Storage,
                    format!("父类 `{parent_class}` 不在所选文件中"),
                )
            })?;
            if class.is_short {
                return Err(NewClassValidationError::new(
                    NewClassField::Storage,
                    "不能向 short class definition 插入嵌套类",
                ));
            }
            if class
                .children
                .iter()
                .any(|child| child.name == request.name)
            {
                return Err(NewClassValidationError::new(
                    NewClassField::Name,
                    format!("父类中已经存在成员 `{}`", request.name),
                ));
            }
            let insertion = class_end_line_start(original, class)?;
            let declaration = generate_modelica_source(request, None)?;
            let source_preview = indent_declaration(&declaration, &insertion.indent);
            let replacement = if insertion.at_line_start {
                format!("{source_preview}\n")
            } else {
                format!("\n{source_preview}\n{}", insertion.indent)
            };
            let updated = apply_source_transaction(
                original,
                &SourceTransaction {
                    edits: vec![SourceEdit {
                        start: insertion.position,
                        end: insertion.position,
                        expected_text: Some(String::new()),
                        replacement,
                    }],
                    source_version: None,
                },
                None,
            )
            .map_err(|error| {
                NewClassValidationError::new(
                    NewClassField::Storage,
                    format!("无法安全插入源代码：{error}"),
                )
            })?;
            let mut changes = vec![NewClassFileChange {
                path: file.clone(),
                expected_contents: Some(original.clone()),
                replacement_contents: updated,
            }];
            add_package_order_change(
                &mut changes,
                package_order_file.as_deref(),
                package_order_before.as_deref(),
                &request.name,
                context,
            )?;
            Ok(NewClassPlan {
                qualified_name,
                primary_file: file.clone(),
                reload_path: file.clone(),
                authorized_root: file.parent().unwrap_or_else(|| Path::new(".")).to_owned(),
                source_preview,
                changes,
            })
        }
    }
}

fn add_package_order_change(
    changes: &mut Vec<NewClassFileChange>,
    order_path: Option<&Path>,
    before: Option<&str>,
    member: &str,
    context: &NewClassContext,
) -> Result<(), NewClassValidationError> {
    let Some(order_path) = order_path else {
        return Ok(());
    };
    let original = context.package_order_files.get(order_path).ok_or_else(|| {
        NewClassValidationError::new(
            NewClassField::Storage,
            format!("package.order 不存在或未读取：{}", order_path.display()),
        )
    })?;
    let replacement = package_order_contents(original, member, before)?;
    changes.push(NewClassFileChange {
        path: order_path.to_owned(),
        expected_contents: Some(original.clone()),
        replacement_contents: replacement,
    });
    Ok(())
}

fn reject_occupied_path(
    path: &Path,
    context: &NewClassContext,
) -> Result<(), NewClassValidationError> {
    if context.occupied_paths.contains(path) {
        return Err(NewClassValidationError::new(
            NewClassField::Storage,
            format!("目标路径已存在：{}", path.display()),
        ));
    }
    Ok(())
}

fn find_class<'a>(classes: &'a [Class], qualified_name: &str) -> Option<&'a Class> {
    for class in classes {
        if class.qualified_name == qualified_name {
            return Some(class);
        }
        if let Some(found) = find_class(&class.children, qualified_name) {
            return Some(found);
        }
    }
    None
}

struct InsertionPoint {
    position: usize,
    indent: String,
    at_line_start: bool,
}

fn class_end_line_start(
    source: &str,
    class: &Class,
) -> Result<InsertionPoint, NewClassValidationError> {
    let tokens = tokenize(source)
        .into_iter()
        .filter(|token| {
            token.start >= class.source_range.start
                && token.end <= class.source_range.end
                && !matches!(token.kind, TokenKind::Whitespace | TokenKind::Comment)
        })
        .collect::<Vec<_>>();
    let suffix = tokens
        .get(tokens.len().saturating_sub(3)..)
        .unwrap_or_default();
    if suffix.len() != 3
        || suffix[0].text != "end"
        || suffix[1].text != class.name
        || suffix[2].text != ";"
    {
        return Err(NewClassValidationError::new(
            NewClassField::Storage,
            format!("无法确认父类 `{}` 的 end 位置", class.qualified_name),
        ));
    }
    let end_start = suffix[0].start;
    let line_start = source[..end_start].rfind('\n').map_or(0, |index| index + 1);
    let prefix = source
        .get(line_start..end_start)
        .ok_or_else(|| NewClassValidationError::new(NewClassField::Storage, "父类源码范围无效"))?;
    let at_line_start = prefix.chars().all(char::is_whitespace);
    let (position, indent) = if at_line_start {
        (line_start, prefix.to_owned())
    } else {
        let parent_indent = prefix
            .chars()
            .take_while(|character| character.is_whitespace())
            .collect::<String>();
        (end_start, parent_indent)
    };
    Ok(InsertionPoint {
        position,
        indent,
        at_line_start,
    })
}

fn indent_declaration(source: &str, parent_indent: &str) -> String {
    let child_indent = format!("{parent_indent}  ");
    source
        .lines()
        .map(|line| format!("{child_indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Apply a validated plan. Existing files are compare-and-replaced; new files
/// are created without clobbering a concurrently-created destination. If a
/// later package.order update fails, newly-created files and empty directories
/// are compensated where safe, and the error reports any leftovers.
pub fn apply_new_class_plan(plan: &NewClassPlan) -> Result<PathBuf, String> {
    apply_new_class_plan_with(plan, |change| {
        if let Some(expected) = &change.expected_contents {
            atomic_replace_if_unchanged(&change.path, expected, &change.replacement_contents)
        } else {
            create_new_file_atomic(&change.path, &change.replacement_contents)
        }
    })
}

fn apply_new_class_plan_with<F>(plan: &NewClassPlan, mut write_change: F) -> Result<PathBuf, String>
where
    F: FnMut(&NewClassFileChange) -> Result<(), String>,
{
    let root = &plan.authorized_root;
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("目标目录不可访问：{}: {error}", root.display()))?;
    if root_metadata.file_type().is_symlink() {
        return Err(format!(
            "拒绝使用符号链接作为授权目标目录：{}",
            root.display()
        ));
    }
    if !root_metadata.is_dir() {
        return Err(format!("目标不是目录：{}", root.display()));
    }
    let mut validated = Vec::with_capacity(plan.changes.len());
    for change in &plan.changes {
        validate_path_under_root(root, &change.path)?;
        match &change.expected_contents {
            None if change.path.exists() => {
                return Err(format!("拒绝覆盖已有文件：{}", change.path.display()));
            }
            Some(expected) => {
                let metadata = fs::metadata(&change.path).map_err(|error| {
                    format!("读取目标文件失败 {}: {error}", change.path.display())
                })?;
                if metadata.permissions().readonly() {
                    return Err(format!("目标文件为只读：{}", change.path.display()));
                }
                let current = fs::read_to_string(&change.path).map_err(|error| {
                    format!("读取目标文件失败 {}: {error}", change.path.display())
                })?;
                if &current != expected {
                    return Err(format!(
                        "目标文件已被外部修改，拒绝覆盖：{}",
                        change.path.display()
                    ));
                }
            }
            None => {}
        }
        validated.push(change);
    }

    let mut created_directories = Vec::new();
    let mut created_files: Vec<(PathBuf, String)> = Vec::new();
    let mut replaced_files: Vec<(PathBuf, String, String)> = Vec::new();
    for change in &validated {
        if let Some(parent) = change.path.parent()
            && let Err(error) =
                create_directories_under_root(root, parent, &mut created_directories)
        {
            return rollback_new_files(
                error,
                &created_files,
                &replaced_files,
                &created_directories,
                &plan.primary_file,
            );
        }
        if let Err(error) = write_change(change) {
            if change.expected_contents.is_none()
                && fs::read_to_string(&change.path)
                    .is_ok_and(|contents| contents == change.replacement_contents)
            {
                created_files.push((change.path.clone(), change.replacement_contents.clone()));
            } else if let Some(expected) = &change.expected_contents
                && fs::read_to_string(&change.path)
                    .is_ok_and(|contents| contents == change.replacement_contents)
            {
                replaced_files.push((
                    change.path.clone(),
                    expected.clone(),
                    change.replacement_contents.clone(),
                ));
            }
            return rollback_new_files(
                format!("{}: {error}", change.path.display()),
                &created_files,
                &replaced_files,
                &created_directories,
                &plan.primary_file,
            );
        }
        if change.expected_contents.is_none() {
            created_files.push((change.path.clone(), change.replacement_contents.clone()));
        } else if let Some(expected) = &change.expected_contents {
            replaced_files.push((
                change.path.clone(),
                expected.clone(),
                change.replacement_contents.clone(),
            ));
        }
    }
    Ok(plan.primary_file.clone())
}

fn validate_path_under_root(root: &Path, path: &Path) -> Result<(), String> {
    if !path.is_absolute() || !path.starts_with(root) {
        return Err(format!("拒绝授权目录之外的路径：{}", path.display()));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("路径不能包含 `..`：{}", path.display()));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("拒绝授权目录之外的路径：{}", path.display()))?;
    let mut current = root.to_owned();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("拒绝经过符号链接的目标路径：{}", current.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(format!("检查目标路径失败 {}: {error}", current.display()));
            }
        }
    }
    Ok(())
}

fn create_directories_under_root(
    root: &Path,
    directory: &Path,
    created: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if directory.exists() {
        return Ok(());
    }
    let parent = directory
        .parent()
        .ok_or_else(|| format!("目标目录无父目录：{}", directory.display()))?;
    if !parent.exists() {
        create_directories_under_root(root, parent, created)?;
    }
    if !directory.starts_with(root) {
        return Err(format!(
            "拒绝创建授权目录之外的目录：{}",
            directory.display()
        ));
    }
    fs::create_dir(directory)
        .map_err(|error| format!("创建目录失败 {}: {error}", directory.display()))?;
    created.push(directory.to_owned());
    Ok(())
}

fn temporary_file_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("目标文件无父目录：{}", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("目标路径不是文件：{}", path.display()))?
        .to_string_lossy();
    for _ in 0..32 {
        let serial = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{file_name}.modelica-viewer-{serial}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!("无法为 {} 分配临时文件名", path.display()))
}

fn write_temp(path: &Path, contents: &str) -> Result<PathBuf, String> {
    let temporary = temporary_file_path(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("创建临时文件失败 {}: {error}", temporary.display()))?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(format!("写临时文件失败 {}: {error}", temporary.display()));
    }
    Ok(temporary)
}

fn create_new_file_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let temporary = write_temp(path, contents)?;
    match fs::hard_link(&temporary, path) {
        Ok(()) => {
            fs::remove_file(&temporary).map_err(|error| {
                format!(
                    "新文件已建立，但临时文件清理失败 {}: {error}",
                    temporary.display()
                )
            })?;
            Ok(())
        }
        Err(link_error) => {
            let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(file) => file,
                Err(error) => {
                    let _ = fs::remove_file(&temporary);
                    return Err(format!(
                        "无法以原子方式创建文件 ({link_error})，且独占创建失败：{error}"
                    ));
                }
            };
            let copy_result = fs::read(&temporary)
                .and_then(|bytes| file.write_all(&bytes))
                .and_then(|()| file.sync_all());
            let _ = fs::remove_file(&temporary);
            if let Err(error) = copy_result {
                let _ = fs::remove_file(path);
                return Err(format!("写入新文件失败：{error}"));
            }
            Ok(())
        }
    }
}

fn atomic_replace_if_unchanged(
    path: &Path,
    expected: &str,
    replacement: &str,
) -> Result<(), String> {
    let current = fs::read_to_string(path).map_err(|error| format!("读取现有文件失败：{error}"))?;
    if current != expected {
        return Err("目标文件内容已变化，拒绝覆盖".to_owned());
    }
    let temporary = write_temp(path, replacement)?;
    let latest =
        fs::read_to_string(path).map_err(|error| format!("再次检查现有文件失败：{error}"))?;
    if latest != expected {
        let _ = fs::remove_file(&temporary);
        return Err("目标文件在写入期间被外部修改，拒绝覆盖".to_owned());
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("原子替换失败：{error}"));
    }
    Ok(())
}

fn rollback_new_files(
    original_error: String,
    created_files: &[(PathBuf, String)],
    replaced_files: &[(PathBuf, String, String)],
    created_directories: &[PathBuf],
    primary_file: &Path,
) -> Result<PathBuf, String> {
    let mut recovery = Vec::new();
    for (path, expected, replacement) in replaced_files.iter().rev() {
        match fs::read_to_string(path) {
            Ok(contents) if &contents == replacement => {
                if let Err(error) = atomic_replace_if_unchanged(path, replacement, expected) {
                    recovery.push(format!("无法恢复现有文件 {}: {error}", path.display()));
                }
            }
            Ok(contents) if &contents == expected => {}
            Ok(_) => recovery.push(format!(
                "现有文件在回滚前已被外部修改，保留并需人工检查：{}",
                path.display()
            )),
            Err(error) => recovery.push(format!("检查回滚文件失败 {}: {error}", path.display())),
        }
    }
    for (path, expected) in created_files.iter().rev() {
        match fs::read_to_string(path) {
            Ok(contents) if &contents == expected => {
                if let Err(error) = fs::remove_file(path) {
                    recovery.push(format!("无法回滚已创建文件 {}: {error}", path.display()));
                }
            }
            Ok(_) => recovery.push(format!(
                "文件在回滚前已被外部修改，保留：{}",
                path.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => recovery.push(format!("检查回滚文件失败 {}: {error}", path.display())),
        }
    }
    for directory in created_directories.iter().rev() {
        if let Err(error) = fs::remove_dir(directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            recovery.push(format!("无法移除空目录 {}: {error}", directory.display()));
        }
    }
    if recovery.is_empty() {
        Err(format!("新建失败，已回滚本次文件修改：{original_error}"))
    } else {
        Err(format!(
            "新建部分失败，回滚未完全完成；主目标 {}。原因：{original_error}。恢复信息：{}",
            primary_file.display(),
            recovery.join("；")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{PackageLoader, PackageMember};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn request(name: &str, kind: ClassKind, storage: NewClassStorageMode) -> NewClassRequest {
        NewClassRequest {
            name: name.to_owned(),
            kind,
            description: None,
            partial: false,
            base_class: None,
            storage,
        }
    }

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let crate_directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = crate_directory
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let path = workspace_root
            .join("target")
            .join(format!("modelica-new-class-{label}-{nonce}"));
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    #[test]
    fn validates_unquoted_and_quoted_modelica_identifiers() {
        for name in ["HeatX", "_private", "M2", "'12H'", "'a b'", "'a\\'b'"] {
            assert!(validate_modelica_identifier(name).is_ok(), "{name}");
        }
        for name in ["", "2Model", "A-B", "model", "Real", "'unterminated", "'é'"] {
            assert!(validate_modelica_identifier(name).is_err(), "{name}");
        }
        assert!(validate_modelica_identifier("'model'").is_ok());
    }

    #[test]
    fn explicit_scope_collision_check_ignores_unrelated_library_classes() {
        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Model".to_owned(), ClassKind::Package);
        context
            .class_kinds
            .insert("Other.Model".to_owned(), ClassKind::Model);
        context.scope_is_explicit = true;
        let request = request(
            "Model",
            ClassKind::Model,
            NewClassStorageMode::SingleFile {
                directory: PathBuf::from("/tmp/independent"),
                within: None,
                package_order_file: None,
                package_order_before: None,
            },
        );

        assert!(validate_new_class(&request, &context).is_ok());
        context.scope_class_names.insert("Model".to_owned());
        assert!(validate_new_class(&request, &context).is_err());
    }

    #[test]
    fn generates_minimal_classes_and_roundtrips_supported_kinds() {
        let cases = [
            (ClassKind::Model, "model"),
            (ClassKind::Class, "class"),
            (ClassKind::Block, "block"),
            (ClassKind::Connector, "connector"),
            (ClassKind::ExpandableConnector, "expandable connector"),
            (ClassKind::Record, "record"),
            (ClassKind::Function, "function"),
            (ClassKind::Package, "package"),
            (ClassKind::OperatorRecord, "operator record"),
        ];
        for (kind, keyword) in cases {
            let class = request(
                "Generated",
                kind,
                NewClassStorageMode::SingleFile {
                    directory: PathBuf::from("/tmp"),
                    within: None,
                    package_order_file: None,
                    package_order_before: None,
                },
            );
            let source = generate_modelica_source(&class, None).expect("generate source");
            assert!(source.starts_with(keyword), "{source}");
            let parsed = parse(&source, "Generated.mo").expect("generated source parses");
            assert_eq!(parsed.classes[0].kind, kind);
            assert_eq!(parsed.classes[0].name, "Generated");
            assert_eq!(parsed.classes[0].qualified_name, "Generated");
        }
    }

    #[test]
    fn operator_function_roundtrips_only_as_operator_record_member() {
        let directory = temporary_directory("operator-function-roundtrip");
        let file = directory.join("Operations.mo");
        let original = "operator record Operations\nend Operations;\n";
        fs::write(&file, original).expect("write operator record fixture");
        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Operations".to_owned(), ClassKind::OperatorRecord);
        context
            .source_files
            .insert(file.clone(), original.to_owned());
        let function = request(
            "compare",
            ClassKind::OperatorFunction,
            NewClassStorageMode::InsertIntoExistingFile {
                file: file.clone(),
                parent_class: "Operations".to_owned(),
                package_order_file: None,
                package_order_before: None,
            },
        );

        let plan = plan_new_class(&function, &context).expect("plan nested operator function");
        let candidate = &plan.changes[0].replacement_contents;
        let parsed = parse(candidate, &file).expect("nested operator function parses");
        let parent = &parsed.classes[0];
        assert_eq!(parent.kind, ClassKind::OperatorRecord);
        assert_eq!(parent.children.len(), 1);
        assert_eq!(parent.children[0].kind, ClassKind::OperatorFunction);
        assert_eq!(parent.children[0].name, "compare");
        assert_eq!(parent.children[0].qualified_name, "Operations.compare");

        let mut invalid = function;
        invalid.storage = NewClassStorageMode::SingleFile {
            directory: directory.clone(),
            within: None,
            package_order_file: None,
            package_order_before: None,
        };
        assert!(validate_new_class(&invalid, &context).is_err());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn generates_type_as_short_definition_and_escapes_description() {
        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Real".to_owned(), ClassKind::Type);
        let mut class = request(
            "Temperature",
            ClassKind::Type,
            NewClassStorageMode::SingleFile {
                directory: PathBuf::from("/tmp"),
                within: None,
                package_order_file: None,
                package_order_before: None,
            },
        );
        class.base_class = Some("Real".to_owned());
        class.partial = true;
        class.description = Some("hot \"water\"\\line\n中文".to_owned());
        let source = generate_modelica_source(&class, None).expect("generate type");
        assert!(
            source.starts_with(
                "partial type Temperature = Real \"hot \\\"water\\\"\\\\line\\n中文\";"
            )
        );
        let parsed = parse(&source, "Temperature.mo").expect("type roundtrip");
        assert_eq!(parsed.classes[0].kind, ClassKind::Type);
        assert!(parsed.classes[0].is_short);
        assert_eq!(
            parsed.classes[0].description.as_deref(),
            class.description.as_deref()
        );
        assert!(validate_new_class(&class, &context).is_ok());
    }

    #[test]
    fn planner_builds_within_and_directory_package_files() {
        let root = PathBuf::from("/tmp/Library");
        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Library".into(), ClassKind::Package);
        context
            .class_kinds
            .insert("Library.FluidUnits".into(), ClassKind::Package);
        let single = request(
            "HeatX",
            ClassKind::Model,
            NewClassStorageMode::SingleFile {
                directory: root.join("FluidUnits"),
                within: Some("Library.FluidUnits".into()),
                package_order_file: None,
                package_order_before: None,
            },
        );
        let single_plan = plan_new_class(&single, &context).expect("single-file plan");
        assert_eq!(single_plan.primary_file, root.join("FluidUnits/HeatX.mo"));
        assert!(
            single_plan
                .source_preview
                .starts_with("within Library.FluidUnits;\n")
        );

        let directory_package = request(
            "NewPackage",
            ClassKind::Package,
            NewClassStorageMode::DirectoryPackage {
                parent_directory: root.join("FluidUnits"),
                within: Some("Library.FluidUnits".into()),
                package_order_file: None,
                package_order_before: None,
            },
        );
        let package_plan = plan_new_class(&directory_package, &context).expect("package plan");
        assert_eq!(
            package_plan.primary_file,
            root.join("FluidUnits/NewPackage/package.mo")
        );
        assert!(
            package_plan
                .source_preview
                .starts_with("within Library.FluidUnits;\n")
        );
    }

    #[test]
    fn inserts_nested_class_before_parent_end_using_source_transaction() {
        let file = PathBuf::from("/tmp/Monolithic.mo");
        let original = "package P\n  // keep this comment\nend P;\n".to_owned();
        let mut context = NewClassContext::default();
        context.class_kinds.insert("P".into(), ClassKind::Package);
        context.source_files.insert(file.clone(), original.clone());
        let class = request(
            "Child",
            ClassKind::Model,
            NewClassStorageMode::InsertIntoExistingFile {
                file: file.clone(),
                parent_class: "P".into(),
                package_order_file: None,
                package_order_before: None,
            },
        );
        let plan = plan_new_class(&class, &context).expect("insertion plan");
        assert_eq!(plan.qualified_name, "P.Child");
        let updated = &plan.changes[0].replacement_contents;
        assert!(updated.contains("  // keep this comment\n  model Child\n  end Child;\nend P;"));
        let parsed = parse(updated, &file).expect("inserted class parses");
        assert_eq!(parsed.classes[0].children[0].qualified_name, "P.Child");
    }

    #[test]
    fn package_order_update_preserves_comments_and_selected_position() {
        let path = PathBuf::from("/tmp/Library/package.order");
        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Library".to_owned(), ClassKind::Package);
        context.package_order_files.insert(
            path.clone(),
            "// library order\r\nFirst // keep\r\nLast\r\n// trailing note\r\n".to_owned(),
        );
        let class = request(
            "Middle",
            ClassKind::Model,
            NewClassStorageMode::SingleFile {
                directory: path.parent().expect("parent").to_owned(),
                within: Some("Library".into()),
                package_order_file: Some(path),
                package_order_before: Some("Last".into()),
            },
        );
        let plan = plan_new_class(&class, &context).expect("plan");
        let order = &plan.changes[1].replacement_contents;
        assert_eq!(
            order,
            "// library order\r\nFirst // keep\r\nMiddle\r\nLast\r\n// trailing note\r\n"
        );
    }

    #[test]
    fn apply_creates_without_overwrite_and_reload_roundtrips() {
        let directory = temporary_directory("apply");
        let class = request(
            "Created",
            ClassKind::Model,
            NewClassStorageMode::SingleFile {
                directory: directory.clone(),
                within: None,
                package_order_file: None,
                package_order_before: None,
            },
        );
        let plan = plan_new_class(&class, &NewClassContext::default()).expect("plan");
        let file = apply_new_class_plan(&plan).expect("write files");
        assert_eq!(file, directory.join("Created.mo"));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "model Created\nend Created;\n"
        );
        let error = apply_new_class_plan(&plan).expect_err("second apply must not overwrite");
        assert!(error.contains("拒绝覆盖已有文件"));
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn directory_package_creation_updates_order_and_reloads_members() {
        let directory = temporary_directory("directory-package");
        fs::write(
            directory.join("package.mo"),
            "package Library end Library;\n",
        )
        .expect("write root package");
        fs::write(directory.join("package.order"), "Old\n").expect("write package order");
        fs::write(
            directory.join("Old.mo"),
            "within Library; model Old end Old;\n",
        )
        .expect("write existing class");
        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Library".to_owned(), ClassKind::Package);
        context
            .class_kinds
            .insert("Library.Old".to_owned(), ClassKind::Model);
        let order_path = directory.join("package.order");
        context
            .package_order_files
            .insert(order_path.clone(), "Old\n".to_owned());
        let class = request(
            "FluidUnits",
            ClassKind::Package,
            NewClassStorageMode::DirectoryPackage {
                parent_directory: directory.clone(),
                within: Some("Library".to_owned()),
                package_order_file: Some(order_path),
                package_order_before: None,
            },
        );
        let plan = plan_new_class(&class, &context).expect("directory package plan");
        apply_new_class_plan(&plan).expect("write package and order");

        let loaded = PackageLoader.load(&directory).expect("reload package tree");
        assert_eq!(loaded.qualified_name, "Library");
        let member = loaded
            .ordered_members
            .iter()
            .find(|member| member.name() == "FluidUnits")
            .expect("created package appears in tree");
        assert!(matches!(member, PackageMember::Package(_)));
        assert_eq!(member.qualified_name(), "Library.FluidUnits");
        assert_eq!(
            fs::read_to_string(directory.join("package.order")).unwrap(),
            "Old\nFluidUnits\n"
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn inserting_into_existing_package_file_preserves_and_indexes_classes() {
        let directory = temporary_directory("insert-existing");
        let file = directory.join("Library.mo");
        let original = "package Library\n  model Existing end Existing;\nend Library;\n";
        fs::write(&file, original).expect("write package source");
        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Library".to_owned(), ClassKind::Package);
        context
            .class_kinds
            .insert("Library.Existing".to_owned(), ClassKind::Model);
        context
            .source_files
            .insert(file.clone(), original.to_owned());
        let class = request(
            "Created",
            ClassKind::Block,
            NewClassStorageMode::InsertIntoExistingFile {
                file: file.clone(),
                parent_class: "Library".to_owned(),
                package_order_file: None,
                package_order_before: None,
            },
        );
        let plan = plan_new_class(&class, &context).expect("insertion plan");
        apply_new_class_plan(&plan).expect("insert nested class");

        let loaded = PackageLoader
            .load(&file)
            .expect("reload monolithic package");
        let created = loaded
            .ordered_members
            .iter()
            .find(|member| member.name() == "Created")
            .expect("created class appears in tree")
            .as_class();
        assert_eq!(created.kind, ClassKind::Block);
        assert_eq!(created.qualified_name, "Library.Created");
        assert_eq!(created.source_file, file);
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn inserting_into_directory_package_updates_order_and_reload_range() {
        let directory = temporary_directory("insert-package-roundtrip");
        let package_file = directory.join("package.mo");
        let order_file = directory.join("package.order");
        let original = "package Library\n  model Existing end Existing;\nend Library;\n";
        fs::write(&package_file, original).expect("write package source");
        fs::write(&order_file, "Existing\n").expect("write package order");

        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Library".to_owned(), ClassKind::Package);
        context
            .class_kinds
            .insert("Library.Existing".to_owned(), ClassKind::Model);
        context
            .source_files
            .insert(package_file.clone(), original.to_owned());
        context
            .package_order_files
            .insert(order_file.clone(), "Existing\n".to_owned());
        let class = request(
            "Created",
            ClassKind::Block,
            NewClassStorageMode::InsertIntoExistingFile {
                file: package_file.clone(),
                parent_class: "Library".to_owned(),
                package_order_file: Some(order_file.clone()),
                package_order_before: Some("Existing".to_owned()),
            },
        );
        let plan = plan_new_class(&class, &context).expect("plan nested package member");
        apply_new_class_plan(&plan).expect("write nested member and package order");

        let loaded = PackageLoader
            .load(&directory)
            .expect("reload directory package");
        let created = loaded
            .ordered_members
            .iter()
            .find(|member| member.name() == "Created")
            .expect("created member appears after reload")
            .as_class();
        assert_eq!(created.qualified_name, "Library.Created");
        assert_eq!(created.source_file, package_file);
        assert!(created.source_range.end > created.source_range.start);
        assert_eq!(
            fs::read_to_string(order_file).unwrap(),
            "Created\nExisting\n"
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn package_order_failure_compensates_the_new_class_file() {
        let directory = temporary_directory("rollback");
        fs::write(directory.join("package.order"), "Old\n").expect("write order");
        let order_path = directory.join("package.order");
        let mut context = NewClassContext::default();
        context
            .package_order_files
            .insert(order_path.clone(), "Old\n".to_owned());
        let class = request(
            "NewModel",
            ClassKind::Model,
            NewClassStorageMode::SingleFile {
                directory: directory.clone(),
                within: None,
                package_order_file: Some(order_path.clone()),
                package_order_before: None,
            },
        );
        let plan = plan_new_class(&class, &context).expect("plan");
        let mut writes = 0;
        let error = apply_new_class_plan_with(&plan, |change| {
            writes += 1;
            if writes == 2 {
                Err("injected package.order write failure".to_owned())
            } else {
                create_new_file_atomic(&change.path, &change.replacement_contents)
            }
        })
        .expect_err("second-file failure must fail the transaction");
        assert!(error.contains("已回滚"));
        assert!(!directory.join("NewModel.mo").exists());
        assert_eq!(fs::read_to_string(order_path).unwrap(), "Old\n");
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn package_order_failure_restores_inserted_package_source() {
        let directory = temporary_directory("rollback-insert");
        let package_file = directory.join("package.mo");
        let order_file = directory.join("package.order");
        let original_package = "package Library\nend Library;\n";
        let original_order = "Existing\n";
        fs::write(&package_file, original_package).expect("write package source");
        fs::write(&order_file, original_order).expect("write package order");

        let mut context = NewClassContext::default();
        context
            .class_kinds
            .insert("Library".to_owned(), ClassKind::Package);
        context
            .source_files
            .insert(package_file.clone(), original_package.to_owned());
        context
            .package_order_files
            .insert(order_file.clone(), original_order.to_owned());
        let class = request(
            "Created",
            ClassKind::Model,
            NewClassStorageMode::InsertIntoExistingFile {
                file: package_file.clone(),
                parent_class: "Library".to_owned(),
                package_order_file: Some(order_file.clone()),
                package_order_before: None,
            },
        );
        let plan = plan_new_class(&class, &context).expect("plan insertion and order update");
        assert_eq!(plan.changes.len(), 2);

        let mut writes = 0;
        let error = apply_new_class_plan_with(&plan, |change| {
            writes += 1;
            let expected = change
                .expected_contents
                .as_deref()
                .expect("both inserted source and package.order are replacements");
            atomic_replace_if_unchanged(&change.path, expected, &change.replacement_contents)?;
            if writes == 2 {
                Err("injected failure after package.order replacement".to_owned())
            } else {
                Ok(())
            }
        })
        .expect_err("second replacement failure must abort");

        assert!(error.contains("已回滚"), "{error}");
        assert_eq!(fs::read_to_string(package_file).unwrap(), original_package);
        assert_eq!(fs::read_to_string(order_file).unwrap(), original_order);
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn refuses_external_modification_before_creating_any_file() {
        let directory = temporary_directory("external-conflict");
        let order_path = directory.join("package.order");
        fs::write(&order_path, "Old\n").expect("write order");
        let mut context = NewClassContext::default();
        context
            .package_order_files
            .insert(order_path.clone(), "Old\n".to_owned());
        let class = request(
            "NewModel",
            ClassKind::Model,
            NewClassStorageMode::SingleFile {
                directory: directory.clone(),
                within: None,
                package_order_file: Some(order_path.clone()),
                package_order_before: None,
            },
        );
        let plan = plan_new_class(&class, &context).expect("plan");
        fs::write(&order_path, "Externally changed\n").expect("simulate external writer");
        let error = apply_new_class_plan(&plan).expect_err("stale order must be rejected");
        assert!(error.contains("已被外部修改"));
        assert!(!directory.join("NewModel.mo").exists());
        fs::remove_dir_all(directory).expect("cleanup");
    }
}
