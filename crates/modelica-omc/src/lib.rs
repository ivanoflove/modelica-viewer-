//! OpenModelica semantic frontend boundary.
//!
//! This crate deliberately has no dependency on the renderer or the legacy
//! Modelica parser.  It owns the boundary between an OpenModelica subprocess
//! and the semantic IR consumed by a future frontend integration.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticModel {
    pub class_name: String,
    pub components: Vec<SemanticComponent>,
    pub connectors: Vec<SemanticConnector>,
    pub connections: Vec<SemanticConnection>,
    pub diagnostics: Vec<SemanticDiagnostic>,
}

impl SemanticModel {
    pub fn new(class_name: impl Into<String>) -> Self {
        Self {
            class_name: class_name.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticComponent {
    pub instance_path: String,
    pub resolved_qualified_type: Option<String>,
    pub kind: Option<String>,
    pub placement: Option<SemanticPlacement>,
    pub icon_transformation: Option<SemanticPlacement>,
    pub icon_graphics: Vec<SemanticGraphic>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticConnector {
    pub instance_path: String,
    pub resolved_type: Option<String>,
    pub placement: Option<SemanticPlacement>,
    pub icon_transformation: Option<SemanticPlacement>,
    pub icon_graphics: Vec<SemanticGraphic>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticConnection {
    pub lhs: String,
    pub rhs: String,
    pub line_points: Vec<SemanticPoint>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticPlacement {
    pub origin: Option<SemanticPoint>,
    pub extent: Option<SemanticExtent>,
    pub rotation: Option<f64>,
    pub visible: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SemanticPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SemanticExtent {
    pub p1: SemanticPoint,
    pub p2: SemanticPoint,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemanticGraphic {
    pub kind: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticDiagnostic {
    pub code: String,
    pub message: String,
    pub context: Option<String>,
}

impl SemanticDiagnostic {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        context: Option<impl Into<String>>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            context: context.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmcConfig {
    pub executable: PathBuf,
    pub working_directory: Option<PathBuf>,
}

impl Default for OmcConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("omc"),
            working_directory: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmcVersion {
    pub executable: PathBuf,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmcJsonDocument {
    pub class_name: String,
    pub model_path: PathBuf,
    pub raw_output: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OmcError {
    NotInstalled {
        executable: String,
        detail: String,
    },
    ProcessIo {
        operation: String,
        detail: String,
    },
    ProcessFailed {
        operation: String,
        status: Option<i32>,
        output: String,
    },
    InvalidRequest {
        detail: String,
    },
    SemanticUnavailable {
        detail: String,
    },
}

impl fmt::Display for OmcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled { executable, detail } => write!(
                formatter,
                "OpenModelica executable '{executable}' was not found; install OpenModelica or configure an explicit executable ({detail})"
            ),
            Self::ProcessIo { operation, detail } => {
                write!(formatter, "OpenModelica {operation} failed: {detail}")
            }
            Self::ProcessFailed {
                operation,
                status,
                output,
            } => write!(
                formatter,
                "OpenModelica {operation} exited with status {:?}: {}",
                status,
                output.trim()
            ),
            Self::InvalidRequest { detail } => {
                write!(formatter, "invalid OpenModelica request: {detail}")
            }
            Self::SemanticUnavailable { detail } => write!(
                formatter,
                "OpenModelica semantic frontend unavailable: {detail}"
            ),
        }
    }
}

impl std::error::Error for OmcError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmcBackend {
    config: OmcConfig,
}

impl OmcBackend {
    pub fn new(config: OmcConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &OmcConfig {
        &self.config
    }

    pub fn detect() -> Result<(Self, OmcVersion), OmcError> {
        let backend = Self::new(OmcConfig::default());
        let version = backend.version()?;
        Ok((backend, version))
    }

    pub fn version(&self) -> Result<OmcVersion, OmcError> {
        let output = Command::new(&self.config.executable)
            .arg("--version")
            .output()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    OmcError::NotInstalled {
                        executable: self.config.executable.display().to_string(),
                        detail: error.to_string(),
                    }
                } else {
                    OmcError::ProcessIo {
                        operation: "version check".into(),
                        detail: error.to_string(),
                    }
                }
            })?;

        if !output.status.success() {
            return Err(OmcError::ProcessFailed {
                operation: "version check".into(),
                status: output.status.code(),
                output: combined_output(&output.stdout, &output.stderr),
            });
        }

        Ok(OmcVersion {
            executable: self.config.executable.clone(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    pub fn get_model_instance(
        &self,
        model_path: &Path,
        class_name: &str,
    ) -> Result<OmcJsonDocument, OmcError> {
        if class_name.trim().is_empty() {
            return Err(OmcError::InvalidRequest {
                detail: "class name must not be empty".into(),
            });
        }
        if !model_path.is_file() {
            return Err(OmcError::InvalidRequest {
                detail: format!("Modelica file does not exist: {}", model_path.display()),
            });
        }

        let script_path = temporary_script_path()?;
        let script = format!(
            "loadFile({});\ngetErrorString();\ngetModelInstance({}, prettyPrint=true);\ngetErrorString();\n",
            modelica_string_literal(model_path),
            modelica_identifier(class_name),
        );

        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&script_path)
                .map_err(|error| OmcError::ProcessIo {
                    operation: "temporary script creation".into(),
                    detail: error.to_string(),
                })?;
            file.write_all(script.as_bytes())
                .map_err(|error| OmcError::ProcessIo {
                    operation: "temporary script write".into(),
                    detail: error.to_string(),
                })?;

            let mut command = Command::new(&self.config.executable);
            command.arg(&script_path);
            if let Some(directory) = &self.config.working_directory {
                command.current_dir(directory);
            }
            let output = command.output().map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    OmcError::NotInstalled {
                        executable: self.config.executable.display().to_string(),
                        detail: error.to_string(),
                    }
                } else {
                    OmcError::ProcessIo {
                        operation: "semantic script execution".into(),
                        detail: error.to_string(),
                    }
                }
            })?;
            let raw_output = combined_output(&output.stdout, &output.stderr);
            if !output.status.success() {
                return Err(OmcError::ProcessFailed {
                    operation: "semantic script execution".into(),
                    status: output.status.code(),
                    output: raw_output,
                });
            }
            Ok(OmcJsonDocument {
                class_name: class_name.to_owned(),
                model_path: model_path.to_owned(),
                raw_output,
            })
        })();

        let _ = fs::remove_file(&script_path);
        result
    }

    pub fn load_semantic_model(
        &self,
        model_path: &Path,
        class_name: &str,
    ) -> Result<SemanticModel, OmcError> {
        let _document = self.get_model_instance(model_path, class_name)?;
        Err(OmcError::SemanticUnavailable {
            detail: "OMC getModelInstance output decoding is not enabled yet; no heuristic or fake semantic result was produced".into(),
        })
    }
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        (stdout, "") => stdout.to_owned(),
        ("", stderr) => stderr.to_owned(),
        (stdout, stderr) => format!("stdout: {stdout}; stderr: {stderr}"),
    }
}

fn modelica_identifier(value: &str) -> String {
    value
        .split('.')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

fn modelica_string_literal(path: &Path) -> String {
    let value: OsString = path.as_os_str().to_os_string();
    let value = value.to_string_lossy();
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

fn temporary_script_path() -> Result<PathBuf, OmcError> {
    let directory = env::temp_dir();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| OmcError::ProcessIo {
            operation: "temporary script naming".into(),
            detail: error.to_string(),
        })?;
    Ok(directory.join(format!(
        "modelica-omc-{}-{}.mos",
        std::process::id(),
        timestamp.as_nanos()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_executable_is_reported_without_a_fallback() {
        let backend = OmcBackend::new(OmcConfig {
            executable: PathBuf::from("definitely-not-an-openmodelica-executable"),
            working_directory: None,
        });

        let error = backend.version().expect_err("executable must be absent");
        assert!(matches!(error, OmcError::NotInstalled { .. }));
        assert!(error.to_string().contains("OpenModelica executable"));
    }

    #[test]
    fn modelica_paths_are_escaped_for_the_script() {
        let path = PathBuf::from(r#"D:\Models\A "quoted"\file.mo"#);
        assert_eq!(
            modelica_string_literal(&path),
            r#""D:\\Models\\A \"quoted\"\\file.mo""#
        );
    }

    #[test]
    fn semantic_model_starts_empty_and_has_no_fake_components() {
        let model = SemanticModel::new("IEH_CPP");
        assert_eq!(model.class_name, "IEH_CPP");
        assert!(model.components.is_empty());
        assert!(model.connectors.is_empty());
        assert!(model.connections.is_empty());
    }
}
