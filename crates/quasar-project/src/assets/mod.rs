//! Stable project asset identities and the on-disk asset catalogue.

use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const ASSET_METADATA_VERSION: u32 = 1;
pub const MODEL_COMPONENT_TYPE_ID: &str = "quasar.model";
pub const MODEL_COMPONENT_SCHEMA_VERSION: u32 = 1;
pub const SCRIPT_COMPONENT_TYPE_ID: &str = "quasar.script";
pub const SCRIPT_COMPONENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(pub Uuid);

impl AssetId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for AssetId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Model,
    Texture,
    Audio,
    Script,
}

impl AssetKind {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Model => "Models",
            Self::Texture => "Textures",
            Self::Audio => "Audio",
            Self::Script => "Scripts",
        }
    }
}

/// Reference from an authored scene object to a validated project Lua script asset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptComponent {
    pub asset_id: AssetId,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}

impl ScriptComponent {
    pub fn into_component(self) -> crate::document::ComponentDocument {
        crate::document::ComponentDocument {
            type_id: SCRIPT_COMPONENT_TYPE_ID.to_owned(),
            schema_version: SCRIPT_COMPONENT_SCHEMA_VERSION,
            data: serde_json::to_value(self).expect("script component is serializable"),
        }
    }

    pub fn from_components(
        components: &[crate::document::ComponentDocument],
    ) -> Result<Option<Self>, String> {
        let mut found = None;
        for component in components
            .iter()
            .filter(|c| c.type_id == SCRIPT_COMPONENT_TYPE_ID)
        {
            if component.schema_version != SCRIPT_COMPONENT_SCHEMA_VERSION {
                return Err(format!(
                    "unsupported script component version {}; supported version is {}",
                    component.schema_version, SCRIPT_COMPONENT_SCHEMA_VERSION
                ));
            }
            if found.is_some() {
                return Err("object contains more than one script component".to_owned());
            }
            found = Some(
                serde_json::from_value(component.data.clone())
                    .map_err(|e| format!("invalid script component: {e}"))?,
            );
        }
        Ok(found)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetMetadata {
    pub metadata_version: u32,
    pub asset_id: AssetId,
    pub kind: AssetKind,
    /// Project-relative source path, always written with forward slashes.
    pub source_path: String,
    pub importer_id: String,
    pub importer_version: u32,
    #[serde(default)]
    pub import_settings: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// `None` means that the license is unknown, not that the asset is unrestricted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub derived_files: Vec<String>,
}

impl AssetMetadata {
    pub fn validate(&self) -> Result<(), String> {
        if self.metadata_version != ASSET_METADATA_VERSION {
            return Err(format!(
                "unsupported asset metadata version {}; supported version is {}",
                self.metadata_version, ASSET_METADATA_VERSION
            ));
        }
        if self.importer_id.trim().is_empty() || self.importer_version == 0 {
            return Err("asset importer identity and version must be set".to_owned());
        }
        validate_project_relative_path("asset source", &self.source_path)?;
        for path in &self.derived_files {
            validate_project_relative_path("derived asset", path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetStatus {
    Ready,
    Missing,
    Conflict,
}

#[derive(Clone, Debug)]
pub struct AssetRecord {
    pub metadata: AssetMetadata,
    pub status: AssetStatus,
    pub source_file: PathBuf,
    pub metadata_file: PathBuf,
}

#[derive(Clone, Debug)]
pub struct AssetDiagnostic {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct AssetCatalog {
    pub assets: Vec<AssetRecord>,
    pub diagnostics: Vec<AssetDiagnostic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAssetComponent {
    pub asset_id: AssetId,
}

impl ModelAssetComponent {
    pub fn into_component(self) -> crate::document::ComponentDocument {
        crate::document::ComponentDocument {
            type_id: MODEL_COMPONENT_TYPE_ID.to_owned(),
            schema_version: MODEL_COMPONENT_SCHEMA_VERSION,
            data: serde_json::to_value(self).expect("model asset component is serializable"),
        }
    }

    pub fn from_components(
        components: &[crate::document::ComponentDocument],
    ) -> Result<Option<Self>, String> {
        let mut found = None;
        for component in components
            .iter()
            .filter(|component| component.type_id == MODEL_COMPONENT_TYPE_ID)
        {
            if component.schema_version != MODEL_COMPONENT_SCHEMA_VERSION {
                return Err(format!(
                    "unsupported model component version {}; supported version is {}",
                    component.schema_version, MODEL_COMPONENT_SCHEMA_VERSION
                ));
            }
            if found.is_some() {
                return Err("object contains more than one model asset component".to_owned());
            }
            found = Some(
                serde_json::from_value(component.data.clone())
                    .map_err(|error| format!("invalid model asset component: {error}"))?,
            );
        }
        Ok(found)
    }
}

impl AssetCatalog {
    /// Rebuilds the project catalogue from `Assets/**/*.meta.json` sidecars.
    /// Symlinks are ignored so a project cannot make discovery escape its root.
    pub fn scan(project_root: &Path) -> Self {
        let mut catalog = Self::default();
        let assets_root = project_root.join("Assets");
        let mut sidecars = Vec::new();
        if let Err(error) = collect_sidecars(&assets_root, &mut sidecars) {
            if error.kind() != std::io::ErrorKind::NotFound {
                catalog.diagnostics.push(AssetDiagnostic {
                    path: assets_root,
                    message: format!("cannot scan asset directory: {error}"),
                });
            }
            return catalog;
        }
        sidecars.sort();

        let mut ids: HashMap<AssetId, Vec<usize>> = HashMap::new();
        for sidecar in sidecars {
            let metadata = match fs::read(&sidecar)
                .map_err(|error| format!("cannot read metadata: {error}"))
                .and_then(|bytes| {
                    serde_json::from_slice::<AssetMetadata>(&bytes)
                        .map_err(|error| format!("invalid metadata JSON: {error}"))
                }) {
                Ok(metadata) => metadata,
                Err(message) => {
                    catalog.diagnostics.push(AssetDiagnostic {
                        path: sidecar,
                        message,
                    });
                    continue;
                }
            };
            if let Err(message) = metadata.validate() {
                catalog.diagnostics.push(AssetDiagnostic {
                    path: sidecar,
                    message,
                });
                continue;
            }
            let source_file = match resolve_project_path(project_root, &metadata.source_path) {
                Ok(path) => path,
                Err(message) => {
                    catalog.diagnostics.push(AssetDiagnostic {
                        path: sidecar,
                        message,
                    });
                    continue;
                }
            };
            if sidecar_path(&source_file) != sidecar {
                catalog.diagnostics.push(AssetDiagnostic {
                    path: sidecar,
                    message: "metadata sidecar must stay next to the asset it describes".into(),
                });
                continue;
            }
            let status = if source_file.is_file() {
                let root = fs::canonicalize(project_root);
                let source = fs::canonicalize(&source_file);
                let source_is_inside_project = match (root, source) {
                    (Ok(root), Ok(source)) => source.starts_with(&root),
                    _ => false,
                };
                if !source_is_inside_project {
                    catalog.diagnostics.push(AssetDiagnostic {
                        path: source_file,
                        message: "asset source resolves outside the project root".into(),
                    });
                    continue;
                }
                AssetStatus::Ready
            } else {
                AssetStatus::Missing
            };
            let record_index = catalog.assets.len();
            ids.entry(metadata.asset_id).or_default().push(record_index);
            catalog.assets.push(AssetRecord {
                metadata,
                status,
                source_file,
                metadata_file: sidecar,
            });
        }
        for indexes in ids.values().filter(|indexes| indexes.len() > 1) {
            for index in indexes {
                catalog.assets[*index].status = AssetStatus::Conflict;
            }
        }
        catalog
    }
}

pub fn sidecar_path(source_path: &Path) -> PathBuf {
    let mut name = source_path.file_name().unwrap_or_default().to_os_string();
    name.push(".meta.json");
    source_path.with_file_name(name)
}

/// Checks the RIFF/WAVE container and required format/data chunks without decoding audio.
pub fn validate_wav(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("WAV file has an invalid RIFF/WAVE header".to_owned());
    }
    let declared_length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8;
    if declared_length > bytes.len() || declared_length < 12 {
        return Err("WAV declared length exceeds the file size".to_owned());
    }
    let mut offset = 12usize;
    let mut has_format = false;
    let mut has_data = false;
    while offset + 8 <= declared_length {
        let chunk_length =
            u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start
            .checked_add(chunk_length)
            .filter(|end| *end <= declared_length)
            .ok_or_else(|| "WAV chunk extends beyond the declared file length".to_owned())?;
        match &bytes[offset..offset + 4] {
            b"fmt " if chunk_length >= 16 => has_format = true,
            b"data" if chunk_length > 0 => has_data = true,
            _ => {}
        }
        offset = chunk_end + (chunk_length & 1);
    }
    if offset != declared_length || !has_format || !has_data {
        return Err("WAV must contain complete format and audio data chunks".to_owned());
    }
    Ok(())
}

pub fn validate_project_relative_path(label: &str, value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("{label} must be a safe, non-empty relative path"));
    }
    Ok(())
}

pub fn resolve_project_path(project_root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_project_relative_path("asset path", relative)?;
    Ok(project_root.join(Path::new(relative)))
}

fn collect_sidecars(directory: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_sidecars(&entry.path(), output)?;
        } else if file_type.is_file() && entry.file_name().to_string_lossy().ends_with(".meta.json")
        {
            output.push(entry.path());
        }
    }
    Ok(())
}
