//! Local asset import and validation for the Editor.

use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use quasar_project::assets::{
    ASSET_METADATA_VERSION, AssetId, AssetKind, AssetMetadata, AssetRecord, AssetStatus,
    sidecar_path,
};
use uuid::Uuid;

pub mod jobs;

const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_GLTF_JSON_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXTURE_DIMENSION: u32 = 32_768;
static ASSET_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn import_local_asset_cancellable(
    project_root: &Path,
    source_path: &Path,
    provenance: Option<ImportProvenance>,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<AssetRecord, String> {
    let _mutation_guard = asset_mutation_lock()?;
    ensure_not_cancelled(cancelled)?;
    let project_root = canonical_project_root(project_root)?;
    let source_path = fs::canonicalize(source_path).map_err(|error| {
        format!(
            "cannot resolve import source {}: {error}",
            source_path.display()
        )
    })?;
    let file_name = source_path
        .file_name()
        .ok_or_else(|| "import source must have a file name".to_owned())?;
    let kind = supported_kind(&source_path)?;
    validate_source(&source_path, kind)?;
    ensure_not_cancelled(cancelled)?;

    let assets_directory = ensure_project_directory(
        &project_root,
        Path::new("Assets").join(kind.directory()).as_path(),
    )?;
    let staging_root = ensure_project_directory(&project_root, Path::new(".quasar/staging"))?;

    let asset_id = AssetId::new();
    let staging_directory = staging_root.join(asset_id.0.to_string());
    fs::create_dir(&staging_directory)
        .map_err(|error| format!("cannot create import staging area: {error}"))?;
    let staged_source = staging_directory.join(file_name);
    let result = (|| {
        copy_cancellable(&source_path, &staged_source, cancelled, &mut progress)?;
        validate_source(&staged_source, kind)?;
        ensure_not_cancelled(cancelled)?;
        let (destination, sidecar) = unique_destination(&assets_directory, file_name)?;
        let relative_path = destination
            .strip_prefix(&project_root)
            .map_err(|_| "asset destination escaped the project root".to_owned())?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = AssetMetadata {
            metadata_version: ASSET_METADATA_VERSION,
            asset_id,
            kind,
            source_path: relative_path,
            importer_id: importer_id(kind).to_owned(),
            importer_version: 1,
            import_settings: serde_json::json!({}),
            source_url: provenance.as_ref().map(|source| source.url.clone()),
            author: provenance.as_ref().and_then(|source| source.author.clone()),
            license: provenance
                .as_ref()
                .and_then(|source| source.license.clone()),
            derived_files: Vec::new(),
        };
        metadata.validate()?;
        let staged_metadata = staging_directory.join("asset.meta.json");
        let encoded = serde_json::to_vec_pretty(&metadata)
            .map_err(|error| format!("cannot encode asset metadata: {error}"))?;
        write_new_file(&staged_metadata, &encoded)?;

        ensure_not_cancelled(cancelled)?;
        quasar_project::persistence::replace_file_from_staging(
            &staged_source,
            &destination,
            "asset source",
        )?;
        if let Err(error) = fs::rename(&staged_metadata, &sidecar) {
            let _ = fs::remove_file(&destination);
            return Err(format!("cannot publish asset metadata: {error}"));
        }
        Ok(AssetRecord {
            metadata,
            status: AssetStatus::Ready,
            source_file: destination,
            metadata_file: sidecar,
        })
    })();
    let _ = fs::remove_dir_all(&staging_directory);
    result
}

#[derive(Clone, Debug)]
pub struct ImportProvenance {
    pub url: String,
    pub author: Option<String>,
    pub license: Option<String>,
}

pub fn reimport_asset_cancellable(
    project_root: &Path,
    asset_id: AssetId,
    source_path: Option<PathBuf>,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<AssetRecord, String> {
    ensure_not_cancelled(cancelled)?;
    let project_root = canonical_project_root(project_root)?;
    let catalog = quasar_project::assets::AssetCatalog::scan(&project_root);
    let existing = catalog
        .assets
        .into_iter()
        .find(|asset| asset.metadata.asset_id == asset_id)
        .ok_or_else(|| format!("asset '{}' is missing from the project catalog", asset_id.0))?;
    if existing.status == AssetStatus::Conflict {
        return Err(format!("asset '{}' has a duplicate AssetId", asset_id.0));
    }
    if existing.status == AssetStatus::Missing {
        return Err(format!(
            "asset source '{}' is missing",
            existing.metadata.source_path
        ));
    }
    let staging_root = ensure_project_directory(&project_root, Path::new(".quasar/staging"))?;
    let staging_directory = staging_root.join(format!("reimport-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging_directory)
        .map_err(|error| format!("cannot create reimport staging area: {error}"))?;
    let staged_source = staging_directory.join("source.staged");
    let result = (|| {
        let new_source = if let Some(source_path) = source_path {
            if supported_kind(&source_path)? != existing.metadata.kind {
                return Err("reimport source kind must match the existing asset".to_owned());
            }
            source_path
        } else if let Some(source_url) = &existing.metadata.source_url {
            let url = url::Url::parse(source_url)
                .map_err(|error| format!("stored asset URL is invalid: {error}"))?;
            let extension = Path::new(&existing.metadata.source_path)
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("bin");
            let downloaded = staging_directory.join(format!("source.{extension}"));
            download_direct_url(&url, &downloaded, cancelled, &mut progress)?;
            downloaded
        } else {
            return Err("reimport requires a new source file path".to_owned());
        };
        validate_source(&new_source, existing.metadata.kind)?;
        copy_cancellable(&new_source, &staged_source, cancelled, &mut progress)?;
        validate_source(&staged_source, existing.metadata.kind)?;
        ensure_not_cancelled(cancelled)?;
        let _mutation_guard = asset_mutation_lock()?;
        quasar_project::persistence::replace_file_from_staging(
            &staged_source,
            &existing.source_file,
            "reimported asset",
        )?;
        Ok(AssetRecord {
            status: AssetStatus::Ready,
            ..existing
        })
    })();
    let _ = fs::remove_dir_all(staging_directory);
    result
}

pub fn import_asset_from_url_cancellable(
    project_root: &Path,
    raw_url: &str,
    author: Option<String>,
    license: Option<String>,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<AssetRecord, String> {
    let project_root = canonical_project_root(project_root)?;
    let mut url =
        url::Url::parse(raw_url).map_err(|error| format!("invalid asset URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("asset URL must use HTTP or HTTPS".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("asset URL must not contain embedded credentials".to_owned());
    }
    url.set_fragment(None);
    let source_name = url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .ok_or_else(|| "asset URL must end with a supported file name".to_owned())?;
    let extension = Path::new(source_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| "asset URL must end with .glb, .png, .jpg, .jpeg or .wav".to_owned())?;
    let temp_name = format!("download-{}.{}", Uuid::new_v4(), extension);
    let staging_root = ensure_project_directory(&project_root, Path::new(".quasar/staging"))?;
    let staging_directory = staging_root.join(format!("url-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging_directory)
        .map_err(|error| format!("cannot create URL import staging area: {error}"))?;
    let download_path = staging_directory.join(temp_name);
    let result = (|| {
        download_direct_url(&url, &download_path, cancelled, &mut progress)?;
        ensure_not_cancelled(cancelled)?;
        import_local_asset_cancellable(
            &project_root,
            &download_path,
            Some(ImportProvenance {
                url: url.to_string(),
                author,
                license,
            }),
            cancelled,
            &mut progress,
        )
    })();
    let _ = fs::remove_dir_all(staging_directory);
    result
}

fn copy_cancellable(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<(), String> {
    let mut input =
        fs::File::open(source).map_err(|error| format!("cannot read source: {error}"))?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("cannot stage source: {error}"))?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut copied = 0u64;
    loop {
        ensure_not_cancelled(cancelled)?;
        let count = input
            .read(&mut buffer)
            .map_err(|error| format!("cannot read source: {error}"))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("cannot write staged source: {error}"))?;
        copied += count as u64;
        progress(copied);
    }
    output
        .sync_all()
        .map_err(|error| format!("cannot sync staged source: {error}"))
}

fn download_direct_url(
    url: &url::Url,
    destination: &Path,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<(), String> {
    let host = url
        .host_str()
        .ok_or_else(|| "asset URL does not contain a host".to_owned())?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "asset URL uses an unsupported port".to_owned())?;
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("cannot resolve asset URL host '{host}': {error}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !is_public_address(address.ip()))
    {
        return Err("asset URL host resolves to a local or private network address".to_owned());
    }
    let mut command = Command::new(curl_executable());
    command
        .arg("--silent")
        .arg("--show-error")
        .arg("--fail")
        .arg("--noproxy")
        .arg("*")
        .arg("--max-time")
        .arg("90")
        .arg("--max-filesize")
        .arg(MAX_IMPORT_BYTES.to_string())
        .arg("--proto")
        .arg("=http,https")
        .arg("--output")
        .arg(destination);
    for address in &addresses {
        let ip = match address.ip() {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        command.arg("--resolve").arg(format!("{host}:{port}:{ip}"));
    }
    let mut child = command
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start curl for direct asset download: {error}"))?;
    let start = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(destination);
            return Err("asset download was cancelled".to_owned());
        }
        if let Ok(metadata) = fs::metadata(destination) {
            let received = metadata.len();
            progress(received);
            if received > MAX_IMPORT_BYTES {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(destination);
                return Err("download exceeded the 512 MiB asset limit".to_owned());
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot monitor asset download: {error}"))?
        {
            break status;
        }
        if start.elapsed() > Duration::from_secs(90) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(destination);
            return Err("asset download exceeded the 90 second time limit".to_owned());
        }
        thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        let mut error = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut error);
        }
        let _ = fs::remove_file(destination);
        return Err(if error.trim().is_empty() {
            format!("curl failed with status {status}")
        } else {
            format!("asset download failed: {}", error.trim())
        });
    }
    if !fs::metadata(destination)
        .map(|metadata| metadata.len() > 0 && metadata.len() <= MAX_IMPORT_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::remove_file(destination);
        return Err("downloaded file is empty or exceeds the 512 MiB limit".to_owned());
    }
    Ok(())
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b == 18 || b == 19))
                || a >= 224)
        }
        IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4_mapped() {
                return is_public_address(IpAddr::V4(ipv4));
            }
            !(ip.is_unspecified()
                || ip.is_loopback()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || ip.is_multicast())
        }
    }
}

#[cfg(windows)]
fn curl_executable() -> &'static str {
    "curl.exe"
}

#[cfg(not(windows))]
fn curl_executable() -> &'static str {
    "curl"
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("asset operation was cancelled".to_owned())
    } else {
        Ok(())
    }
}

fn asset_mutation_lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    ASSET_MUTATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "asset mutation lock is unavailable".to_owned())
}

fn ensure_project_directory(project_root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(project_root).map_err(|error| {
        format!(
            "cannot resolve project root {}: {error}",
            project_root.display()
        )
    })?;
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("asset directory must stay inside the project root".to_owned());
    }
    let mut current = root.clone();
    for component in relative.components() {
        if let std::path::Component::Normal(name) = component {
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(format!(
                        "project asset directory contains a non-directory or symlink: {}",
                        current.display()
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current).map_err(|error| {
                        format!(
                            "cannot create project asset directory {}: {error}",
                            current.display()
                        )
                    })?;
                }
                Err(error) => {
                    return Err(format!(
                        "cannot inspect project asset directory {}: {error}",
                        current.display()
                    ));
                }
            }
        }
    }
    let canonical = fs::canonicalize(&current).map_err(|error| {
        format!(
            "cannot resolve project asset directory {}: {error}",
            current.display()
        )
    })?;
    if !canonical.starts_with(&root) {
        return Err("project asset directory resolves outside the project root".to_owned());
    }
    Ok(canonical)
}

fn canonical_project_root(project_root: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(project_root).map_err(|error| {
        format!(
            "cannot resolve project root {}: {error}",
            project_root.display()
        )
    })
}

fn supported_kind(path: &Path) -> Result<AssetKind, String> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "glb" => Ok(AssetKind::Model),
        "png" | "jpg" | "jpeg" => Ok(AssetKind::Texture),
        "wav" => Ok(AssetKind::Audio),
        extension => Err(format!("unsupported asset extension '.{extension}'")),
    }
}

fn validate_source(path: &Path, kind: AssetKind) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect import source {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err("import source must be a regular file".to_owned());
    }
    if metadata.len() == 0 || metadata.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "import file size must be between 1 byte and {} MiB",
            MAX_IMPORT_BYTES / (1024 * 1024)
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("cannot read import source: {error}"))?;
    match kind {
        AssetKind::Model => validate_glb(&bytes),
        AssetKind::Texture => validate_texture(path, &bytes),
        AssetKind::Audio => validate_wav(&bytes),
    }
}

fn validate_glb(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return Err("GLB file has an invalid header".to_owned());
    }
    if u32_le(bytes, 4)? != 2 {
        return Err("only binary glTF 2.0 files are supported".to_owned());
    }
    if u32_le(bytes, 8)? as usize != bytes.len() {
        return Err("GLB declared length does not match the file size".to_owned());
    }
    let mut offset = 12usize;
    let mut json_chunk = None;
    let mut chunk_count = 0usize;
    while offset < bytes.len() {
        let chunk_length = u32_le(bytes, offset)? as usize;
        let chunk_type = u32_le(bytes, offset + 4)?;
        offset = offset
            .checked_add(8)
            .ok_or_else(|| "GLB chunk offset overflow".to_owned())?;
        let end = offset
            .checked_add(chunk_length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "GLB chunk extends beyond the file".to_owned())?;
        if chunk_count == 0 {
            if chunk_type != 0x4E4F_534A {
                return Err("GLB first chunk must contain glTF JSON".to_owned());
            }
            if chunk_length > MAX_GLTF_JSON_BYTES {
                return Err("GLB JSON chunk exceeds the 64 MiB limit".to_owned());
            }
            json_chunk = Some(&bytes[offset..end]);
        } else if chunk_type != 0x004E_4942 {
            return Err("GLB contains an unsupported chunk type".to_owned());
        }
        chunk_count += 1;
        offset = end;
    }
    if offset != bytes.len() || chunk_count == 0 {
        return Err("GLB chunk table is incomplete".to_owned());
    }
    let json = json_chunk.ok_or_else(|| "GLB is missing its JSON chunk".to_owned())?;
    let document: serde_json::Value =
        serde_json::from_slice(json).map_err(|error| format!("GLB JSON is invalid: {error}"))?;
    if document
        .pointer("/asset/version")
        .and_then(|value| value.as_str())
        != Some("2.0")
    {
        return Err("GLB asset version must be 2.0".to_owned());
    }
    if contains_external_uri(&document) {
        return Err(
            "GLB refers to an external file; embed all model dependencies first".to_owned(),
        );
    }
    Ok(())
}

fn contains_external_uri(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            (key == "uri" && value.as_str().is_some_and(|uri| !uri.starts_with("data:")))
                || contains_external_uri(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(contains_external_uri),
        _ => false,
    }
}

fn validate_texture(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (width, height) = if extension == "png" {
        png_dimensions(bytes)?
    } else {
        if bytes.len() < 4
            || bytes.get(..2) != Some(&[0xFF, 0xD8])
            || bytes.get(bytes.len() - 2..) != Some(&[0xFF, 0xD9])
        {
            return Err("JPEG file has an invalid header or end marker".to_owned());
        }
        jpeg_dimensions(bytes)?
    };
    if width == 0 || height == 0 || width > MAX_TEXTURE_DIMENSION || height > MAX_TEXTURE_DIMENSION
    {
        return Err("texture dimensions must be between 1 and 32768 pixels".to_owned());
    }
    Ok(())
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        return Err("PNG file has an invalid header".to_owned());
    }
    let mut offset = 8usize;
    let mut dimensions = None;
    let mut has_image_data = false;
    let mut has_end = false;
    while offset < bytes.len() {
        let length = u32_be(bytes, offset)? as usize;
        let chunk_type = bytes
            .get(offset + 4..offset + 8)
            .ok_or_else(|| "PNG chunk header is truncated".to_owned())?;
        let data_start = offset
            .checked_add(8)
            .ok_or_else(|| "PNG chunk offset overflow".to_owned())?;
        let data_end = data_start
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "PNG chunk exceeds the file length".to_owned())?;
        let chunk_end = data_end
            .checked_add(4)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "PNG chunk CRC is truncated".to_owned())?;
        match chunk_type {
            b"IHDR" => {
                if offset != 8 || length != 13 || dimensions.is_some() {
                    return Err("PNG must start with one 13-byte IHDR chunk".to_owned());
                }
                dimensions = Some((u32_be(bytes, data_start)?, u32_be(bytes, data_start + 4)?));
            }
            b"IDAT" => {
                if dimensions.is_none() {
                    return Err("PNG image data appears before its IHDR chunk".to_owned());
                }
                has_image_data = true;
            }
            b"IEND" => {
                if length != 0 || chunk_end != bytes.len() {
                    return Err("PNG IEND chunk is malformed or not the last chunk".to_owned());
                }
                has_end = true;
                break;
            }
            _ if dimensions.is_none() => {
                return Err("PNG is missing its initial IHDR chunk".to_owned());
            }
            _ => {}
        }
        offset = chunk_end;
    }
    if !has_image_data || !has_end {
        return Err("PNG must contain image data followed by an IEND chunk".to_owned());
    }
    dimensions.ok_or_else(|| "PNG is missing its IHDR chunk".to_owned())
}

fn jpeg_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    let mut offset = 2usize;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xFF {
            offset += 1;
            continue;
        }
        while offset < bytes.len() && bytes[offset] == 0xFF {
            offset += 1;
        }
        let marker = *bytes
            .get(offset)
            .ok_or_else(|| "JPEG marker table is truncated".to_owned())?;
        offset += 1;
        if matches!(marker, 0xD8 | 0xD9 | 0x01 | 0xD0..=0xD7) {
            continue;
        }
        let segment_length = u16::from_be_bytes([
            *bytes
                .get(offset)
                .ok_or_else(|| "JPEG segment is truncated".to_owned())?,
            *bytes
                .get(offset + 1)
                .ok_or_else(|| "JPEG segment is truncated".to_owned())?,
        ]) as usize;
        if segment_length < 2 || offset + segment_length > bytes.len() {
            return Err("JPEG segment extends beyond the file".to_owned());
        }
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            if segment_length < 7 {
                return Err("JPEG frame header is incomplete".to_owned());
            }
            let height = u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]) as u32;
            let width = u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]) as u32;
            return Ok((width, height));
        }
        offset += segment_length;
    }
    Err("JPEG dimensions could not be read".to_owned())
}

fn validate_wav(bytes: &[u8]) -> Result<(), String> {
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

fn unique_destination(
    directory: &Path,
    original_name: &std::ffi::OsStr,
) -> Result<(PathBuf, PathBuf), String> {
    let original = Path::new(original_name);
    let stem = original
        .file_stem()
        .ok_or_else(|| "import source must have a file name".to_owned())?
        .to_string_lossy();
    let extension = original.extension().unwrap_or_default();
    for suffix in 0..10_000u32 {
        let name = if suffix == 0 {
            original_name.to_os_string()
        } else {
            let mut name = std::ffi::OsString::from(format!("{stem} ({suffix})"));
            name.push(".");
            name.push(extension);
            name
        };
        let destination = directory.join(name);
        let sidecar = sidecar_path(&destination);
        if !destination.exists() && !sidecar.exists() {
            return Ok((destination, sidecar));
        }
    }
    Err("could not find a free asset file name".to_owned())
}

fn importer_id(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Model => "quasar.gltf.glb",
        AssetKind::Texture => "quasar.image",
        AssetKind::Audio => "quasar.audio.wav",
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create staged metadata: {error}"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write staged metadata: {error}"))
}

fn u32_le(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "GLB header or chunk table is truncated".to_owned())?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}

fn u32_be(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "PNG dimension header is truncated".to_owned())?;
    Ok(u32::from_be_bytes(value.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestProject(PathBuf);

    impl TestProject {
        fn new(label: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("quasar-asset-{label}-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).expect("temporary project directory can be created");
            Self(root)
        }

        fn root(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    fn not_cancelled() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn imports_glb_and_reimport_preserves_last_good_source_on_failure() {
        let project = TestProject::new("reimport");
        let first = fixture("stage0-player/assets/Quasar/viewport-prop.glb");
        let replacement = fixture("stage0-animation/assets/Quasar/RobotExpressive.glb");
        let cancelled = not_cancelled();

        let imported =
            import_local_asset_cancellable(project.root(), &first, None, &cancelled, |_| {})
                .expect("valid GLB imports");
        assert_eq!(imported.status, AssetStatus::Ready);
        assert_eq!(imported.metadata.kind, AssetKind::Model);
        assert!(imported.metadata_file.is_file());
        assert_eq!(
            fs::read(&imported.source_file).unwrap(),
            fs::read(&first).unwrap()
        );

        let reimported = reimport_asset_cancellable(
            project.root(),
            imported.metadata.asset_id,
            Some(replacement.clone()),
            &cancelled,
            |_| {},
        )
        .expect("valid replacement GLB reimports");
        assert_eq!(reimported.metadata.asset_id, imported.metadata.asset_id);
        let last_good_bytes = fs::read(&reimported.source_file).unwrap();
        assert_eq!(last_good_bytes, fs::read(&replacement).unwrap());

        let invalid = project.root().join("invalid.glb");
        fs::write(&invalid, b"not a glTF file").unwrap();
        let error = reimport_asset_cancellable(
            project.root(),
            imported.metadata.asset_id,
            Some(invalid),
            &cancelled,
            |_| {},
        )
        .expect_err("invalid reimport is rejected");
        assert!(
            error.contains("invalid header"),
            "unexpected error: {error}"
        );
        assert_eq!(fs::read(&reimported.source_file).unwrap(), last_good_bytes);
        let catalog = quasar_project::assets::AssetCatalog::scan(project.root());
        assert_eq!(catalog.assets.len(), 1);
        assert_eq!(catalog.assets[0].status, AssetStatus::Ready);
    }

    #[test]
    fn invalid_and_cancelled_imports_do_not_publish_assets() {
        let project = TestProject::new("invalid-import");
        let invalid = project.root().join("invalid.glb");
        fs::write(&invalid, b"not a glTF file").unwrap();
        let cancelled = not_cancelled();
        assert!(
            import_local_asset_cancellable(project.root(), &invalid, None, &cancelled, |_| {},)
                .is_err()
        );
        assert!(
            quasar_project::assets::AssetCatalog::scan(project.root())
                .assets
                .is_empty()
        );

        let unsupported = project.root().join("notes.txt");
        fs::write(&unsupported, b"not an asset format").unwrap();
        let error =
            import_local_asset_cancellable(project.root(), &unsupported, None, &cancelled, |_| {})
                .expect_err("unsupported file format is rejected");
        assert!(error.contains("unsupported asset extension"));

        let truncated_png = project.root().join("truncated.png");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        png.extend_from_slice(&[0; 4]);
        fs::write(&truncated_png, png).unwrap();
        let error = import_local_asset_cancellable(
            project.root(),
            &truncated_png,
            None,
            &cancelled,
            |_| {},
        )
        .expect_err("truncated PNG chunk table is rejected");
        assert!(error.contains("PNG"), "unexpected error: {error}");

        let already_cancelled = AtomicBool::new(true);
        let valid = fixture("stage0-player/assets/Quasar/viewport-prop.glb");
        let error = import_local_asset_cancellable(
            project.root(),
            &valid,
            None,
            &already_cancelled,
            |_| {},
        )
        .expect_err("cancelled import does not proceed");
        assert!(error.contains("cancelled"));
        assert!(
            quasar_project::assets::AssetCatalog::scan(project.root())
                .assets
                .is_empty()
        );
    }

    #[test]
    fn imports_valid_png_jpeg_and_wav_files() {
        let project = TestProject::new("other-formats");
        let cancelled = not_cancelled();
        let png =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/images/stage0-viewport.png");
        let png_asset =
            import_local_asset_cancellable(project.root(), &png, None, &cancelled, |_| {})
                .expect("valid PNG imports");
        assert_eq!(png_asset.metadata.kind, AssetKind::Texture);

        let jpeg = project.root().join("tiny.jpg");
        fs::write(
            &jpeg,
            [
                0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11,
                0x00, 0xFF, 0xD9,
            ],
        )
        .unwrap();
        let jpeg_asset =
            import_local_asset_cancellable(project.root(), &jpeg, None, &cancelled, |_| {})
                .expect("valid JPEG structure imports");
        assert_eq!(jpeg_asset.metadata.kind, AssetKind::Texture);

        let wav = fixture("stage0-player/assets/Quasar/audio/room-ambience.wav");
        let wav_asset =
            import_local_asset_cancellable(project.root(), &wav, None, &cancelled, |_| {})
                .expect("valid WAV imports");
        assert_eq!(wav_asset.metadata.kind, AssetKind::Audio);
        assert_eq!(
            quasar_project::assets::AssetCatalog::scan(project.root())
                .assets
                .len(),
            3
        );
    }

    #[test]
    fn direct_url_import_rejects_local_addresses_before_network_access() {
        let project = TestProject::new("url-guard");
        let cancelled = not_cancelled();
        let error = import_asset_from_url_cancellable(
            project.root(),
            "http://127.0.0.1/viewport-prop.glb",
            None,
            None,
            &cancelled,
            |_| {},
        )
        .expect_err("loopback URL is rejected without invoking curl");
        assert!(
            error.contains("local or private"),
            "unexpected error: {error}"
        );
        assert!(
            quasar_project::assets::AssetCatalog::scan(project.root())
                .assets
                .is_empty()
        );
    }
}
