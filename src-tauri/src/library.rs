// RUST DEEP MODULE B: UNIVERSAL MULTI-PACK & LOCAL MUSIC LIBRARY ENGINE
// Supports importing & managing multiple standard .zip song packs, local folders, and loose files:
//   - Standard .zip archives (UTF-8 / GB18030 / Shift-JIS) — scanned in-place & streamed directly via komapack:// without duplicating disk space
//   - Local album / artist / flat music folders — linked in-place without duplicating disk space
//   - Per-source enable/disable toggle, relocate/rescan, reveal in Explorer/Finder, and remove
//   - ID3v2 / FLAC Vorbis / MP4 tags, exact duration, embedded cover art, and bilingual .lrc

use encoding_rs::{GB18030, SHIFT_JIS, UTF_16BE, UTF_16LE};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;
use tauri::http::{self, header, StatusCode};
use tauri::{AppHandle, Emitter, Manager};

// ============================================================================
// PUBLIC WIRE DTOs (Frontend <-> Backend Contract)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricLineDto {
    pub t: Option<f64>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtext: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDto {
    pub no: usize,
    pub title: String,
    pub artist: String,
    pub audio: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    pub duration: f64,
    pub full_dur: String,
    pub env: Vec<u8>,
    pub lyrics: Vec<LyricLineDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDto {
    pub id: String,
    pub source_id: String,
    pub code: String,
    pub title: String,
    pub artist: String,
    pub subtitle: String,
    pub year: String,
    pub freq: String,
    pub cover: String,
    pub phosphor: String,
    pub glow: String,
    pub tracks: Vec<TrackDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySourceDto {
    pub id: String,
    pub kind: String, // "zip" | "folder" | "files"
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_path: Option<String>,
    pub cover: String,
    pub enabled: bool,
    pub album_count: usize,
    pub track_count: usize,
    pub missing: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStateDto {
    pub albums: Vec<AlbumDto>,
    pub sources: Vec<LibrarySourceDto>,
    pub select_album_id: Option<String>,
    pub imported_tracks: usize,
}

// ============================================================================
// PERSISTED LIBRARY REGISTRY (Backwards-Compatible)
// ============================================================================

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryRegistry {
    #[serde(default, alias = "installed_packs")]
    installed_archives: Vec<InstalledArchiveEntry>,
    #[serde(default, deserialize_with = "deserialize_linked_folders")]
    linked_folders: Vec<LinkedFolderEntry>,
    #[serde(default)]
    loose_tracks: Vec<LooseTrackEntry>,
    #[serde(default = "default_true")]
    loose_enabled: bool,
}

impl Default for LibraryRegistry {
    fn default() -> Self {
        Self {
            installed_archives: Vec::new(),
            linked_folders: Vec::new(),
            loose_tracks: Vec::new(),
            loose_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstalledArchiveEntry {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    path: String,
    #[serde(default, skip_serializing)]
    dir: String,
    #[serde(default, skip_serializing)]
    origin_path: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn migrate_legacy_registry(reg: &mut LibraryRegistry, legacy_archives_dir: Option<&Path>) -> bool {
    let mut changed = false;
    for entry in &mut reg.installed_archives {
        if entry.path.trim().is_empty() {
            if let Some(origin) = entry.origin_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                entry.path = origin.to_string();
                changed = true;
            }
        }
        if !entry.dir.is_empty() {
            entry.dir.clear();
            changed = true;
        }
        if entry.origin_path.is_some() {
            entry.origin_path = None;
            changed = true;
        }
    }
    if let Some(legacy_dir) = legacy_archives_dir {
        if legacy_dir.exists() {
            let _ = fs::remove_dir_all(legacy_dir);
        }
    }
    changed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LinkedFolderEntry {
    path: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawLinkedFolder {
    PathOnly(String),
    Full(LinkedFolderEntry),
}

fn deserialize_linked_folders<'de, D>(deserializer: D) -> Result<Vec<LinkedFolderEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Vec::<RawLinkedFolder>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|item| match item {
            RawLinkedFolder::PathOnly(path) => LinkedFolderEntry {
                path,
                enabled: true,
            },
            RawLinkedFolder::Full(entry) => entry,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LooseTrackEntry {
    audio: String,
    #[serde(default)]
    lrc: Option<String>,
    #[serde(default)]
    cover: Option<String>,
}

#[derive(Debug, Default)]
struct ExtractedAudioMeta {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    track_no: Option<usize>,
    year: Option<String>,
    duration_secs: Option<f64>,
    embedded_cover: Option<String>,
    embedded_lyrics: Option<String>,
}

#[derive(Debug, Default)]
pub struct ParsedLrc {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub lines: Vec<LyricLineDto>,
}

const PHOSPHOR_PALETTES: [(&str, &str); 5] = [
    ("#dff2ff", "rgba(130, 202, 255, 0.44)"),
    ("#ffe9cf", "rgba(255, 174, 96, 0.42)"),
    ("#d8ffec", "rgba(106, 236, 174, 0.40)"),
    ("#f2e3ff", "rgba(198, 146, 255, 0.40)"),
    ("#fff4d4", "rgba(255, 212, 108, 0.40)"),
];

// ============================================================================
// TAURI COMMANDS & LIBRARY MANAGEMENT ACTIONS
// ============================================================================

#[tauri::command]
pub fn load_library(app: AppHandle) -> LibraryStateDto {
    let mut reg = read_registry(&app);
    let legacy_dir = app_storage_dir(&app).join("archives");
    if migrate_legacy_registry(&mut reg, Some(&legacy_dir)) {
        let _ = write_registry(&app, &reg);
    }
    let (albums, sources) = build_catalog(&app, &reg);
    let total = albums.iter().map(|a| a.tracks.len()).sum();
    LibraryStateDto {
        albums,
        sources,
        select_album_id: None,
        imported_tracks: total,
    }
}

#[tauri::command]
pub fn rescan_library(app: AppHandle) -> LibraryStateDto {
    let state = load_library(app.clone());
    let _ = app.emit("library://updated", state.clone());
    state
}

#[tauri::command]
pub fn import_paths(app: AppHandle, paths: Vec<String>) -> Result<LibraryStateDto, String> {
    let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    import_paths_internal(&app, &path_bufs)
}

#[tauri::command]
pub async fn pick_and_import(
    app: AppHandle,
    kind: String,
) -> Result<Option<LibraryStateDto>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || -> Option<Vec<PathBuf>> {
        if kind == "folder" {
            rfd::FileDialog::new()
                .set_title("选择音乐文件夹（可多选）")
                .pick_folders()
        } else {
            rfd::FileDialog::new()
                .set_title("选择音乐文件或压缩包（可多选）")
                .add_filter(
                    "音频与歌曲压缩包",
                    &[
                        "zip", "flac", "mp3", "m4a", "wav", "ogg", "opus", "aac", "webm", "lrc",
                        "jpg", "jpeg", "png", "webp",
                    ],
                )
                .pick_files()
        }
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(paths) = picked else {
        return Ok(None);
    };
    if paths.is_empty() {
        return Ok(None);
    }

    let outcome = import_paths_internal(&app, &paths)?;
    Ok(Some(outcome))
}

#[tauri::command]
pub fn toggle_library_source(app: AppHandle, source_id: String) -> LibraryStateDto {
    let mut reg = read_registry(&app);

    if let Some(archive_id) = source_id
        .strip_prefix("zip:")
        .or_else(|| source_id.strip_prefix("pack:"))
    {
        if let Some(entry) = reg.installed_archives.iter_mut().find(|a| a.id == archive_id) {
            entry.enabled = !entry.enabled;
        }
    } else if let Some(folder) = source_id.strip_prefix("folder:") {
        let target_key = normalize_cmp_key(Path::new(folder));
        if let Some(entry) = reg
            .linked_folders
            .iter_mut()
            .find(|f| normalize_cmp_key(Path::new(&f.path)) == target_key)
        {
            entry.enabled = !entry.enabled;
        }
    } else if source_id.starts_with("loose:") {
        reg.loose_enabled = !reg.loose_enabled;
    }

    let _ = write_registry(&app, &reg);
    emit_current_state(&app, &reg, None)
}

#[tauri::command]
pub async fn reimport_library_source(
    app: AppHandle,
    source_id: String,
) -> Result<Option<LibraryStateDto>, String> {
    let mut reg = read_registry(&app);

    if let Some(archive_id) = source_id
        .strip_prefix("zip:")
        .or_else(|| source_id.strip_prefix("pack:"))
    {
        let Some(idx) = reg.installed_archives.iter().position(|a| a.id == archive_id) else {
            return Ok(None);
        };

        let existing_path = reg.installed_archives[idx].path.clone();
        if existing_path.is_empty() || !Path::new(&existing_path).is_file() {
            let picked = tauri::async_runtime::spawn_blocking(move || {
                rfd::FileDialog::new()
                    .set_title("重新选择音乐包")
                    .add_filter("歌曲压缩包", &["zip"])
                    .pick_file()
            })
            .await
            .map_err(|e| e.to_string())?;

            let Some(zip_path) = picked else {
                return Ok(None);
            };
            validate_zip_archive(&zip_path)?;
            let stem = zip_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("pack");
            reg.installed_archives[idx].name = normalize_nfc(stem.trim());
            reg.installed_archives[idx].path = clean_path_string(&zip_path);
            reg.installed_archives[idx].dir.clear();
            reg.installed_archives[idx].origin_path = None;
        }
        reg.installed_archives[idx].enabled = true;
        invalidate_zip_caches(&app, archive_id);
        write_registry(&app, &reg)?;

        let target_source = format!("zip:{archive_id}");
        return Ok(Some(emit_state_selecting_source(&app, &reg, &target_source)));
    }

    if let Some(folder) = source_id.strip_prefix("folder:") {
        let target_key = normalize_cmp_key(Path::new(folder));
        let Some(idx) = reg
            .linked_folders
            .iter()
            .position(|f| normalize_cmp_key(Path::new(&f.path)) == target_key)
        else {
            return Ok(None);
        };

        if !Path::new(&reg.linked_folders[idx].path).is_dir() {
            let relocated = tauri::async_runtime::spawn_blocking(move || {
                rfd::FileDialog::new()
                    .set_title("重新定位音乐文件夹")
                    .pick_folder()
            })
            .await
            .map_err(|e| e.to_string())?;

            let Some(new_dir) = relocated else {
                return Ok(None);
            };
            reg.linked_folders[idx].path = clean_path_string(&new_dir);
        }
        reg.linked_folders[idx].enabled = true;
        write_registry(&app, &reg)?;

        let target_source = format!("folder:{}", reg.linked_folders[idx].path);
        return Ok(Some(emit_state_selecting_source(&app, &reg, &target_source)));
    }

    if source_id.starts_with("loose:") {
        let any_missing = reg
            .loose_tracks
            .iter()
            .any(|t| !Path::new(&t.audio).is_file());
        if any_missing {
            let picked = tauri::async_runtime::spawn_blocking(move || {
                rfd::FileDialog::new()
                    .set_title("重新选择音乐文件")
                    .add_filter(
                        "音频与歌词文件",
                        &[
                            "flac", "mp3", "m4a", "wav", "ogg", "opus", "aac", "webm", "lrc",
                            "jpg", "jpeg", "png", "webp",
                        ],
                    )
                    .pick_files()
            })
            .await
            .map_err(|e| e.to_string())?;

            if let Some(paths) = picked.filter(|p| !p.is_empty()) {
                reg.loose_tracks.retain(|t| Path::new(&t.audio).is_file());
                write_registry(&app, &reg)?;
                return Ok(Some(import_paths_internal(&app, &paths)?));
            }
            return Ok(None);
        }
    }

    Ok(Some(emit_current_state(&app, &reg, None)))
}

#[tauri::command]
pub fn reveal_library_source(app: AppHandle, source_id: String) -> Result<(), String> {
    let reg = read_registry(&app);
    let target_dir: Option<PathBuf> = if let Some(archive_id) = source_id
        .strip_prefix("zip:")
        .or_else(|| source_id.strip_prefix("pack:"))
    {
        reg.installed_archives
            .iter()
            .find(|a| a.id == archive_id)
            .filter(|a| !a.path.is_empty())
            .and_then(|a| Path::new(&a.path).parent().map(Path::to_path_buf))
    } else if let Some(folder) = source_id.strip_prefix("folder:") {
        Some(PathBuf::from(folder))
    } else if source_id.starts_with("loose:") {
        reg.loose_tracks
            .first()
            .and_then(|t| Path::new(&t.audio).parent().map(Path::to_path_buf))
    } else {
        None
    };

    let Some(dir) = target_dir.filter(|p| p.exists()) else {
        return Err("目录不存在".to_string());
    };

    crate::platform::reveal_in_file_manager(&dir)?;

    Ok(())
}

#[tauri::command]
pub fn remove_library_source(app: AppHandle, source_id: String) -> LibraryStateDto {
    let mut reg = read_registry(&app);

    if let Some(archive_id) = source_id
        .strip_prefix("zip:")
        .or_else(|| source_id.strip_prefix("pack:"))
    {
        reg.installed_archives.retain(|a| a.id != archive_id);
        invalidate_zip_caches(&app, archive_id);
    } else if let Some(folder) = source_id.strip_prefix("folder:") {
        let target_key = normalize_cmp_key(Path::new(folder));
        reg.linked_folders
            .retain(|f| normalize_cmp_key(Path::new(&f.path)) != target_key);
    } else if let Some(target_slug) = source_id.strip_prefix("loose:") {
        if target_slug == "all" {
            reg.loose_tracks.clear();
        } else {
            let cover_cache = cover_cache_dir(&app);
            reg.loose_tracks.retain(|entry| {
                let meta = extract_audio_meta(Path::new(&entry.audio), &cover_cache);
                let alb_name = meta
                    .album
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("单曲");
                slugify(alb_name, 0) != target_slug
            });
        }
    }

    let _ = write_registry(&app, &reg);
    emit_current_state(&app, &reg, None)
}

#[tauri::command]
pub fn clear_library(app: AppHandle) -> LibraryStateDto {
    let _ = fs::remove_dir_all(pack_meta_cache_dir(&app));
    let _ = fs::remove_dir_all(app_storage_dir(&app).join("archives"));
    clear_all_ram_media_cache();

    let reg = LibraryRegistry::default();
    let _ = write_registry(&app, &reg);
    emit_current_state(&app, &reg, None)
}

pub fn trigger_dialog_import_from_menu(app: AppHandle, kind: &'static str) {
    tauri::async_runtime::spawn(async move {
        let _ = pick_and_import(app, kind.to_string()).await;
    });
}

fn emit_current_state(
    app: &AppHandle,
    reg: &LibraryRegistry,
    select_album_id: Option<String>,
) -> LibraryStateDto {
    let (albums, sources) = build_catalog(app, reg);
    let imported_tracks = albums.iter().map(|a| a.tracks.len()).sum();
    let state = LibraryStateDto {
        albums,
        sources,
        select_album_id,
        imported_tracks,
    };
    let _ = app.emit("library://updated", state.clone());
    state
}

fn emit_state_selecting_source(
    app: &AppHandle,
    reg: &LibraryRegistry,
    source_id: &str,
) -> LibraryStateDto {
    let (albums, sources) = build_catalog(app, reg);
    let select_album_id = albums
        .iter()
        .find(|a| a.source_id == source_id)
        .map(|a| a.id.clone());
    let imported_tracks = albums.iter().map(|a| a.tracks.len()).sum();
    let state = LibraryStateDto {
        albums,
        sources,
        select_album_id,
        imported_tracks,
    };
    let _ = app.emit("library://updated", state.clone());
    state
}

// ============================================================================
// CORE INGESTION & MULTI-PACK ZIP EXTRACTION
// ============================================================================

pub fn import_paths_internal(
    app: &AppHandle,
    paths: &[PathBuf],
) -> Result<LibraryStateDto, String> {
    let mut reg = read_registry(app);
    let prev_ids: HashSet<String> = build_catalog(app, &reg)
        .0
        .into_iter()
        .map(|a| a.id)
        .collect();

    let mut loose_audios: Vec<PathBuf> = Vec::new();
    let mut loose_lrcs: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut loose_covers: Vec<PathBuf> = Vec::new();
    let mut last_touched_source: Option<String> = None;

    for raw_path in paths {
        if !raw_path.exists() {
            continue;
        }
        if raw_path.is_dir() {
            let clean = clean_path_string(raw_path);
            let key = normalize_cmp_key(raw_path);
            if let Some(existing) = reg
                .linked_folders
                .iter_mut()
                .find(|f| normalize_cmp_key(Path::new(&f.path)) == key)
            {
                existing.enabled = true;
            } else {
                reg.linked_folders.push(LinkedFolderEntry {
                    path: clean.clone(),
                    enabled: true,
                });
            }
            last_touched_source = Some(format!("folder:{clean}"));
            continue;
        }

        let ext = ext_lower(raw_path);
        match ext.as_str() {
            "zip" => {
                validate_zip_archive(raw_path)?;
                let raw_origin_key = normalize_cmp_key(raw_path);
                let stem = raw_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("pack");
                let base_id = slugify(stem, reg.installed_archives.len());

                let matching_idx = reg.installed_archives.iter().position(|a| {
                    (!a.path.is_empty() && normalize_cmp_key(Path::new(&a.path)) == raw_origin_key)
                        || (a.path.is_empty() && a.id == base_id)
                });

                let assigned_id = if let Some(idx) = matching_idx {
                    reg.installed_archives[idx].id.clone()
                } else {
                    allocate_unique_archive_id(&base_id, &reg.installed_archives)
                };

                let entry = InstalledArchiveEntry {
                    id: assigned_id.clone(),
                    name: normalize_nfc(stem.trim()),
                    path: clean_path_string(raw_path),
                    dir: String::new(),
                    origin_path: None,
                    enabled: true,
                };
                if let Some(idx) = matching_idx {
                    reg.installed_archives[idx] = entry;
                } else {
                    reg.installed_archives.push(entry);
                }
                invalidate_zip_caches(app, &assigned_id);
                last_touched_source = Some(format!("zip:{assigned_id}"));
            }
            ext if is_audio_ext(ext) => {
                loose_audios.push(raw_path.clone());
            }
            "lrc" | "txt" => {
                if let Some(stem) = raw_path.file_stem().and_then(|s| s.to_str()) {
                    loose_lrcs.insert(normalize_stem_key(stem), raw_path.clone());
                }
            }
            ext if is_image_ext(ext) => {
                loose_covers.push(raw_path.clone());
            }
            _ => {}
        }
    }

    if !loose_audios.is_empty() {
        reg.loose_enabled = true;
        let batch_cover = if loose_covers.len() == 1 {
            Some(clean_path_string(&loose_covers[0]))
        } else {
            None
        };

        for audio_path in loose_audios {
            let clean_audio = clean_path_string(&audio_path);
            let stem_key = audio_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(normalize_stem_key)
                .unwrap_or_default();

            let paired_lrc = loose_lrcs
                .get(&stem_key)
                .cloned()
                .or_else(|| find_sidecar_lrc(&audio_path))
                .map(|p| clean_path_string(&p));

            let paired_cover = loose_covers
                .iter()
                .find(|img| {
                    img.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| normalize_stem_key(s) == stem_key)
                        .unwrap_or(false)
                })
                .map(|p| clean_path_string(p))
                .or_else(|| batch_cover.clone());

            let key = normalize_cmp_key(&audio_path);
            reg.loose_tracks
                .retain(|t| normalize_cmp_key(Path::new(&t.audio)) != key);
            reg.loose_tracks.push(LooseTrackEntry {
                audio: clean_audio,
                lrc: paired_lrc,
                cover: paired_cover,
            });
            last_touched_source = Some("loose:".to_string());
        }
    } else if !loose_lrcs.is_empty() && reg.loose_tracks.len() == 1 {
        if let Some((_, lrc_path)) = loose_lrcs.into_iter().next() {
            reg.loose_tracks[0].lrc = Some(clean_path_string(&lrc_path));
        }
    }

    write_registry(app, &reg)?;
    let (albums, sources) = build_catalog(app, &reg);

    let select_album_id = albums
        .iter()
        .find(|a| !prev_ids.contains(&a.id))
        .or_else(|| {
            last_touched_source.as_deref().and_then(|src| {
                albums
                    .iter()
                    .find(|a| a.source_id == src || a.source_id.starts_with(src))
            })
        })
        .map(|a| a.id.clone());

    let imported_tracks: usize = albums.iter().map(|a| a.tracks.len()).sum();

    let state = LibraryStateDto {
        albums,
        sources,
        select_album_id,
        imported_tracks,
    };

    let _ = app.emit("library://updated", state.clone());
    Ok(state)
}

fn allocate_unique_archive_id(base_id: &str, existing: &[InstalledArchiveEntry]) -> String {
    if !existing.iter().any(|a| a.id == base_id) {
        return base_id.to_string();
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{base_id}-{n}");
        if !existing.iter().any(|a| a.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn validate_zip_archive(archive_path: &Path) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|e| format!("无法打开压缩包 {}: {e}", archive_path.display()))?;
    zip::ZipArchive::new(file).map_err(|e| format!("无效的 ZIP 压缩文件: {e}"))?;
    Ok(())
}

fn file_signature(path: &Path) -> Option<(u64, u128)> {
    let meta = fs::metadata(path).ok()?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Some((meta.len(), modified_ms))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedZipCatalog {
    path: String,
    file_len: u64,
    modified_unix_ms: u128,
    albums: Vec<AlbumDto>,
}

struct CachedMediaBlob {
    cache_key: String,
    archive_id: String,
    file_len: u64,
    modified_unix_ms: u128,
    mime: String,
    bytes: Arc<Vec<u8>>,
    is_audio: bool,
}

static MEDIA_RAM_CACHE: OnceLock<Mutex<Vec<CachedMediaBlob>>> = OnceLock::new();

fn media_ram_cache() -> &'static Mutex<Vec<CachedMediaBlob>> {
    MEDIA_RAM_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn invalidate_zip_caches(app: &AppHandle, archive_id: &str) {
    let _ = fs::remove_file(pack_meta_file_path(app, archive_id));
    if let Ok(mut guard) = media_ram_cache().lock() {
        guard.retain(|item| item.archive_id != archive_id);
    }
}

fn clear_all_ram_media_cache() {
    if let Ok(mut guard) = media_ram_cache().lock() {
        guard.clear();
    }
}

fn get_cached_ram_blob(
    cache_key: &str,
    sig: (u64, u128),
) -> Option<(String, Arc<Vec<u8>>)> {
    let mut guard = media_ram_cache().lock().ok()?;
    let pos = guard.iter().position(|it| {
        it.cache_key == cache_key && it.file_len == sig.0 && it.modified_unix_ms == sig.1
    })?;
    let item = guard.remove(pos);
    let result = (item.mime.clone(), Arc::clone(&item.bytes));
    guard.push(item);
    Some(result)
}

fn put_cached_ram_blob(blob: CachedMediaBlob) {
    let Ok(mut guard) = media_ram_cache().lock() else {
        return;
    };
    guard.retain(|it| it.cache_key != blob.cache_key);
    let max_items = if blob.is_audio { 2 } else { 24 };
    let current_count = guard.iter().filter(|it| it.is_audio == blob.is_audio).count();
    if current_count >= max_items {
        if let Some(evict_idx) = guard.iter().position(|it| it.is_audio == blob.is_audio) {
            guard.remove(evict_idx);
        }
    }
    guard.push(blob);
}

pub fn handle_komapack_protocol(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let resp = serve_komapack_request(&app, &request);
        responder.respond(resp);
    });
}

fn build_empty_status_response(status: StatusCode) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCEPT_RANGES, "bytes")
        .body(Vec::new())
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}

fn parse_single_http_range(header_val: &str, total_len: usize) -> Result<(usize, usize), ()> {
    if total_len == 0 {
        return Err(());
    }
    let spec = header_val
        .trim()
        .strip_prefix("bytes=")
        .ok_or(())?
        .split(',')
        .next()
        .ok_or(())?
        .trim();
    let (start_str, end_str) = spec.split_once('-').ok_or(())?;
    let start_trimmed = start_str.trim();
    let end_trimmed = end_str.trim();

    if start_trimmed.is_empty() {
        let suffix: usize = end_trimmed.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = total_len.saturating_sub(suffix);
        return Ok((start, total_len - 1));
    }

    let start: usize = start_trimmed.parse().map_err(|_| ())?;
    if start >= total_len {
        return Err(());
    }
    let end = if end_trimmed.is_empty() {
        total_len - 1
    } else {
        end_trimmed.parse::<usize>().map_err(|_| ())?.min(total_len - 1)
    };
    if end < start {
        return Err(());
    }
    Ok((start, end))
}

fn build_media_http_response(
    request: &http::Request<Vec<u8>>,
    bytes: &[u8],
    mime: &str,
) -> http::Response<Vec<u8>> {
    if request.method() == http::Method::OPTIONS {
        return http::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "range")
            .body(Vec::new())
            .unwrap_or_else(|_| http::Response::new(Vec::new()));
    }

    let total_len = bytes.len();
    if request.method() == http::Method::HEAD {
        return http::Response::builder()
            .status(StatusCode::OK)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_TYPE, mime)
            .header(header::CONTENT_LENGTH, total_len)
            .body(Vec::new())
            .unwrap_or_else(|_| http::Response::new(Vec::new()));
    }

    if let Some(range_header) = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
    {
        match parse_single_http_range(range_header, total_len) {
            Ok((start, raw_end)) => {
                const MAX_RANGE_CHUNK: usize = 2 * 1024 * 1024;
                let end = raw_end.min(start.saturating_add(MAX_RANGE_CHUNK - 1));
                let slice = &bytes[start..=end];
                return http::Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(
                        header::ACCESS_CONTROL_EXPOSE_HEADERS,
                        "content-range, accept-ranges, content-length",
                    )
                    .header(header::CONTENT_TYPE, mime)
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{total_len}"),
                    )
                    .header(header::CONTENT_LENGTH, slice.len())
                    .body(slice.to_vec())
                    .unwrap_or_else(|_| http::Response::new(Vec::new()));
            }
            Err(()) => {
                return http::Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(header::CONTENT_RANGE, format!("bytes */{total_len}"))
                    .body(Vec::new())
                    .unwrap_or_else(|_| http::Response::new(Vec::new()));
            }
        }
    }

    http::Response::builder()
        .status(StatusCode::OK)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, total_len)
        .body(bytes.to_vec())
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}

fn serve_komapack_request(
    app: &AppHandle,
    request: &http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    if request.method() == http::Method::OPTIONS {
        return build_media_http_response(request, &[], "application/octet-stream");
    }

    let raw_path = request.uri().path().trim_start_matches('/');
    let decoded = percent_decode_str(raw_path);
    let mut segs = decoded.split('/');
    let Some(archive_id) = segs.next() else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };
    let Some(mode) = segs.next() else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };
    let Some(file_part) = segs.next() else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };
    if segs.next().is_some() || !matches!(mode, "entry" | "embedded") {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    }

    let Some((idx_str, req_ext)) = file_part.split_once('.') else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };
    let Ok(entry_idx) = idx_str.parse::<usize>() else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };

    let mut reg = read_registry(app);
    migrate_legacy_registry(&mut reg, None);
    let Some(archive) = reg.installed_archives.into_iter().find(|a| a.id == archive_id) else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };
    let zip_path = Path::new(&archive.path);
    let Some(sig) = file_signature(zip_path) else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };

    let cache_key = format!("{archive_id}:{mode}:{entry_idx}");
    if let Some((mime, bytes)) = get_cached_ram_blob(&cache_key, sig) {
        return build_media_http_response(request, &bytes, &mime);
    }

    let Ok(file) = File::open(zip_path) else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    };
    if entry_idx >= zip.len() {
        return build_empty_status_response(StatusCode::NOT_FOUND);
    }

    let (entry_name, raw_bytes) = {
        let Ok(mut entry) = zip.by_index(entry_idx) else {
            return build_empty_status_response(StatusCode::NOT_FOUND);
        };
        let name = decode_text_smart(entry.name_raw());
        let mut buf = Vec::with_capacity(entry.size().min(16 * 1024 * 1024) as usize);
        if entry.read_to_end(&mut buf).is_err() {
            return build_empty_status_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
        (name, buf)
    };

    let (mime, final_bytes, is_audio) = if mode == "entry" {
        let lower_ext = req_ext.to_ascii_lowercase();
        if is_image_ext(&lower_ext) {
            let (img_mime, _) = detect_image_mime_and_ext(&raw_bytes);
            (img_mime.to_string(), Arc::new(raw_bytes), false)
        } else {
            (
                audio_mime_for_ext(&lower_ext).to_string(),
                Arc::new(raw_bytes),
                true,
            )
        }
    } else {
        let entry_ext = Path::new(&entry_name)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let Some(pic_bytes) = extract_embedded_picture_bytes(&raw_bytes, &entry_ext) else {
            return build_empty_status_response(StatusCode::NOT_FOUND);
        };
        let (img_mime, _) = detect_image_mime_and_ext(&pic_bytes);
        (img_mime.to_string(), Arc::new(pic_bytes), false)
    };

    put_cached_ram_blob(CachedMediaBlob {
        cache_key,
        archive_id: archive_id.to_string(),
        file_len: sig.0,
        modified_unix_ms: sig.1,
        mime: mime.clone(),
        bytes: Arc::clone(&final_bytes),
        is_audio,
    });

    build_media_http_response(request, &final_bytes, &mime)
}

fn sanitize_zip_rel_path(raw_name: &str) -> Option<PathBuf> {
    let normalized = raw_name.replace('\\', "/");
    let mut clean = PathBuf::new();
    for seg in normalized.split('/') {
        let s = seg.trim();
        if s.is_empty() || s == "." {
            continue;
        }
        if s == ".."
            || s == "__MACOSX"
            || s == ".DS_Store"
            || s == "Thumbs.db"
            || s.contains(':')
        {
            return None;
        }
        clean.push(sanitize_fs_component(s));
    }
    (!clean.as_os_str().is_empty()).then_some(clean)
}

fn sanitize_fs_component(s: &str) -> String {
    let nfc = normalize_nfc(s);
    let cleaned: String = nfc
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "album".to_string()
    } else {
        trimmed.to_string()
    }
}

fn load_or_scan_zip_albums(
    app: &AppHandle,
    archive: &InstalledArchiveEntry,
    source_id: &str,
    base_offset: usize,
) -> Vec<AlbumDto> {
    let zip_path = Path::new(&archive.path);
    let Some(sig) = file_signature(zip_path) else {
        return Vec::new();
    };
    let clean_path = clean_path_string(zip_path);
    let meta_path = pack_meta_file_path(app, &archive.id);

    if let Ok(raw) = fs::read_to_string(&meta_path) {
        if let Ok(cached) = serde_json::from_str::<CachedZipCatalog>(&raw) {
            if cached.path == clean_path
                && cached.file_len == sig.0
                && cached.modified_unix_ms == sig.1
            {
                return cached.albums;
            }
        }
    }

    let albums = scan_zip_archive_in_place(zip_path, &archive.id, source_id, base_offset)
        .unwrap_or_default();
    let payload = CachedZipCatalog {
        path: clean_path,
        file_len: sig.0,
        modified_unix_ms: sig.1,
        albums: albums.clone(),
    };
    if let Some(parent) = meta_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&payload) {
        let _ = fs::write(meta_path, json);
    }
    albums
}

pub(crate) fn scan_zip_archive_in_place(
    zip_path: &Path,
    archive_id: &str,
    source_id: &str,
    base_offset: usize,
) -> Result<Vec<AlbumDto>, String> {
    let file = File::open(zip_path)
        .map_err(|e| format!("无法打开压缩包 {}: {e}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("无效的 ZIP 压缩文件: {e}"))?;

    let zip_stem = zip_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("music");
    let default_title = strip_leading_year_prefix(&normalize_nfc(zip_stem));

    let mut dir_groups: BTreeMap<PathBuf, Vec<(usize, PathBuf)>> = BTreeMap::new();
    for idx in 0..zip.len() {
        let Ok(entry) = zip.by_index(idx) else {
            continue;
        };
        let decoded_name = decode_text_smart(entry.name_raw());
        if entry.is_dir() || decoded_name.ends_with('/') || decoded_name.ends_with('\\') {
            continue;
        }
        let Some(rel_path) = sanitize_zip_rel_path(&decoded_name) else {
            continue;
        };
        let vdir = rel_path.parent().unwrap_or(Path::new("")).to_path_buf();
        if vdir.components().count() > 5 {
            continue;
        }
        dir_groups.entry(vdir).or_default().push((idx, rel_path));
    }

    let mut out = Vec::new();
    let mut scratch = Vec::new();

    for (vdir, entries) in dir_groups {
        let mut audio_entries: Vec<(usize, PathBuf, String)> = Vec::new();
        let mut image_entries: Vec<(usize, PathBuf, String)> = Vec::new();
        let mut lrc_entries: Vec<(usize, PathBuf, String)> = Vec::new();

        for (idx, rel_path) in entries {
            let ext = ext_lower(&rel_path);
            if is_audio_ext(&ext) {
                audio_entries.push((idx, rel_path, ext));
            } else if is_image_ext(&ext) {
                image_entries.push((idx, rel_path, ext));
            } else if matches!(ext.as_str(), "lrc" | "txt") {
                lrc_entries.push((idx, rel_path, ext));
            }
        }

        if audio_entries.is_empty() {
            continue;
        }

        let mut folder_cover_uri: Option<String> = None;
        let mut image_stems: BTreeMap<String, String> = BTreeMap::new();
        for (idx, rel_path, ext) in &image_entries {
            let uri = format!("komapack://{archive_id}/entry/{idx}.{ext}");
            let stem = rel_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let lower_stem = stem.to_ascii_lowercase();
            if matches!(
                lower_stem.as_str(),
                "cover" | "folder" | "front" | "album" | "artwork" | "jacket"
            ) {
                folder_cover_uri = Some(uri.clone());
            }
            if !stem.is_empty() {
                image_stems.insert(normalize_stem_key(stem), uri);
            }
        }
        if folder_cover_uri.is_none() && image_entries.len() == 1 && audio_entries.len() > 1 {
            let (idx, _, ref ext) = image_entries[0];
            folder_cover_uri = Some(format!("komapack://{archive_id}/entry/{idx}.{ext}"));
        }

        // Insert .txt first so .lrc takes precedence when both exist for the same stem
        lrc_entries.sort_by_key(|(_, _, ext)| ext == "lrc");
        let mut lrc_map: BTreeMap<String, String> = BTreeMap::new();
        for (idx, rel_path, _) in lrc_entries {
            let Some(stem) = rel_path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(mut entry) = zip.by_index(idx) else {
                continue;
            };
            scratch.clear();
            if entry.read_to_end(&mut scratch).is_ok() {
                lrc_map.insert(normalize_stem_key(stem), decode_text_smart(&scratch));
            }
        }

        audio_entries.sort_by(|a, b| a.1.cmp(&b.1));
        let mut scanned = Vec::with_capacity(audio_entries.len());
        for (idx, rel_path, ext) in audio_entries {
            scratch.clear();
            let meta = if let Ok(mut entry) = zip.by_index(idx) {
                if entry.read_to_end(&mut scratch).is_ok() {
                    extract_zip_audio_meta(&scratch, &ext, archive_id, idx)
                } else {
                    ExtractedAudioMeta::default()
                }
            } else {
                ExtractedAudioMeta::default()
            };
            let raw_stem = rel_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("未命名曲目")
                .to_string();
            let stem_key = normalize_stem_key(&raw_stem);
            scanned.push(ScannedTrackInput {
                sort_path: rel_path,
                raw_stem,
                audio_uri: format!("komapack://{archive_id}/entry/{idx}.{ext}"),
                meta,
                lrc_text: lrc_map.get(&stem_key).cloned(),
                stem_cover_uri: image_stems.get(&stem_key).cloned(),
            });
        }

        let folder_title = vdir
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| strip_leading_year_prefix(&normalize_nfc(s)))
            .unwrap_or_else(|| default_title.clone());

        out.extend(assemble_albums_from_scanned_items(
            scanned,
            folder_cover_uri,
            &folder_title,
            source_id,
            base_offset + out.len(),
        ));
    }

    Ok(out)
}

// ============================================================================
// CATALOG CONSTRUCTION & SOURCE SUMMARY METRICS
// ============================================================================

fn build_catalog(app: &AppHandle, reg: &LibraryRegistry) -> (Vec<AlbumDto>, Vec<LibrarySourceDto>) {
    let cover_cache = cover_cache_dir(app);
    let _ = fs::create_dir_all(&cover_cache);
    allow_asset_dir(app, &app_storage_dir(app));

    let mut active_albums = Vec::new();
    let mut sources = Vec::new();

    for archive in &reg.installed_archives {
        let zip_path = Path::new(&archive.path);
        let exists = !archive.path.trim().is_empty() && zip_path.is_file();
        let source_id = format!("zip:{}", archive.id);
        let pack_albums = if exists {
            load_or_scan_zip_albums(app, archive, &source_id, active_albums.len())
        } else {
            Vec::new()
        };
        let album_count = pack_albums.len();
        let track_count: usize = pack_albums.iter().map(|a| a.tracks.len()).sum();
        let display_name = if !archive.name.trim().is_empty() {
            normalize_nfc(&archive.name)
        } else if album_count == 1 {
            pack_albums[0].title.clone()
        } else {
            normalize_nfc(
                zip_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&archive.id),
            )
        };

        let src_cover = pack_albums
            .iter()
            .find_map(|a| (!a.cover.is_empty()).then(|| a.cover.clone()))
            .unwrap_or_default();

        sources.push(LibrarySourceDto {
            id: source_id,
            kind: "zip".to_string(),
            name: display_name,
            path: archive.path.clone(),
            origin_path: None,
            cover: src_cover,
            enabled: archive.enabled,
            album_count,
            track_count,
            missing: !exists,
        });

        if archive.enabled {
            active_albums.extend(pack_albums);
        }
    }

    for folder_entry in &reg.linked_folders {
        let dir = Path::new(&folder_entry.path);
        let exists = dir.is_dir();
        let source_id = format!("folder:{}", folder_entry.path);
        let mut folder_albums = Vec::new();
        if exists {
            allow_asset_dir(app, dir);
            scan_root_into_albums(
                app,
                dir,
                &source_id,
                &cover_cache,
                active_albums.len(),
                &mut folder_albums,
            );
        }
        let album_count = folder_albums.len();
        let track_count: usize = folder_albums.iter().map(|a| a.tracks.len()).sum();
        let folder_name = normalize_nfc(
            dir.file_name()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&folder_entry.path),
        );
        let src_cover = folder_albums
            .iter()
            .find_map(|a| (!a.cover.is_empty()).then(|| a.cover.clone()))
            .unwrap_or_default();

        sources.push(LibrarySourceDto {
            id: source_id,
            kind: "folder".to_string(),
            name: folder_name,
            path: folder_entry.path.clone(),
            origin_path: None,
            cover: src_cover,
            enabled: folder_entry.enabled,
            album_count,
            track_count,
            missing: !exists,
        });

        if folder_entry.enabled {
            active_albums.extend(folder_albums);
        }
    }

    if !reg.loose_tracks.is_empty() {
        let mut loose_albums = Vec::new();
        build_loose_track_albums(
            app,
            &reg.loose_tracks,
            &cover_cache,
            active_albums.len(),
            &mut loose_albums,
        );
        let album_count = loose_albums.len();
        let track_count: usize = loose_albums.iter().map(|a| a.tracks.len()).sum();
        let any_missing = reg
            .loose_tracks
            .iter()
            .any(|t| !Path::new(&t.audio).is_file());
        let src_cover = loose_albums
            .iter()
            .find_map(|a| (!a.cover.is_empty()).then(|| a.cover.clone()))
            .unwrap_or_default();

        sources.push(LibrarySourceDto {
            id: "loose:all".to_string(),
            kind: "files".to_string(),
            name: "零散单曲".to_string(),
            path: String::new(),
            origin_path: None,
            cover: src_cover,
            enabled: reg.loose_enabled,
            album_count,
            track_count,
            missing: any_missing,
        });

        if reg.loose_enabled {
            active_albums.extend(loose_albums);
        }
    }

    for (idx, alb) in active_albums.iter_mut().enumerate() {
        let palette = PHOSPHOR_PALETTES[idx % PHOSPHOR_PALETTES.len()];
        alb.phosphor = palette.0.to_string();
        alb.glow = palette.1.to_string();
        alb.code = format!("CH-{:02}", idx + 1);
        alb.freq = format!("{:.1}", 19.4 + (idx as f64) * 0.8);
    }

    (active_albums, sources)
}

fn scan_root_into_albums(
    app: &AppHandle,
    root: &Path,
    source_id: &str,
    cover_cache: &Path,
    base_offset: usize,
    out: &mut Vec<AlbumDto>,
) {
    let mut candidate_dirs: Vec<PathBuf> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    collect_album_dirs_recursive(root, 0, &mut visited, &mut candidate_dirs);

    for alb_dir in candidate_dirs {
        allow_asset_dir(app, &alb_dir);
        let next_idx = base_offset + out.len();
        let parsed = parse_album_directory(&alb_dir, source_id, cover_cache, next_idx);
        out.extend(parsed);
    }
}

fn collect_album_dirs_recursive(
    dir: &Path,
    depth: usize,
    visited: &mut HashSet<String>,
    out: &mut Vec<PathBuf>,
) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut has_audio = false;
    let mut subdirs: Vec<PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            subdirs.push(p);
        } else if is_audio_ext(&ext_lower(&p)) {
            has_audio = true;
        }
    }

    subdirs.sort();

    if has_audio {
        let key = normalize_cmp_key(dir);
        if visited.insert(key) {
            out.push(dir.to_path_buf());
        }
    }

    for sub in subdirs {
        collect_album_dirs_recursive(&sub, depth + 1, visited, out);
    }
}

struct ScannedTrackInput {
    sort_path: PathBuf,
    raw_stem: String,
    audio_uri: String,
    meta: ExtractedAudioMeta,
    lrc_text: Option<String>,
    stem_cover_uri: Option<String>,
}

fn parse_album_directory(
    dir: &Path,
    source_id: &str,
    cover_cache: &Path,
    base_index: usize,
) -> Vec<AlbumDto> {
    let folder_title = dir
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| strip_leading_year_prefix(&normalize_nfc(s)))
        .unwrap_or_else(|| "未命名专辑".to_string());

    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut audio_files: Vec<PathBuf> = Vec::new();
    let mut image_files: Vec<PathBuf> = Vec::new();
    let mut folder_cover: Option<PathBuf> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = ext_lower(&path);
        if is_audio_ext(&ext) {
            audio_files.push(path);
        } else if is_image_ext(&ext) {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(
                stem.as_str(),
                "cover" | "folder" | "front" | "album" | "artwork" | "jacket"
            ) {
                folder_cover = Some(path.clone());
            }
            image_files.push(path);
        }
    }

    if audio_files.is_empty() {
        return Vec::new();
    }

    if folder_cover.is_none() && image_files.len() == 1 && audio_files.len() > 1 {
        folder_cover = Some(image_files[0].clone());
    }

    audio_files.sort();

    let scanned: Vec<ScannedTrackInput> = audio_files
        .into_iter()
        .map(|p| {
            let meta = extract_audio_meta(&p, cover_cache);
            let raw_stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("未命名曲目")
                .to_string();
            let lrc_text = find_sidecar_lrc(&p)
                .and_then(|lp| fs::read(lp).ok())
                .map(|bytes| decode_text_smart(&bytes));
            let stem_cover_uri =
                find_matching_stem_image(&image_files, &raw_stem).map(|cp| clean_path_string(&cp));
            ScannedTrackInput {
                audio_uri: clean_path_string(&p),
                sort_path: p,
                raw_stem,
                meta,
                lrc_text,
                stem_cover_uri,
            }
        })
        .collect();

    let folder_cover_uri = folder_cover.map(|p| clean_path_string(&p));
    assemble_albums_from_scanned_items(scanned, folder_cover_uri, &folder_title, source_id, base_index)
}

fn assemble_albums_from_scanned_items(
    items: Vec<ScannedTrackInput>,
    folder_cover_uri: Option<String>,
    folder_title: &str,
    source_id: &str,
    base_index: usize,
) -> Vec<AlbumDto> {
    let distinct_embedded_albums: HashSet<String> = items
        .iter()
        .filter_map(|it| it.meta.album.as_ref().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();

    if folder_cover_uri.is_none() && distinct_embedded_albums.len() >= 2 {
        let mut groups: BTreeMap<String, Vec<ScannedTrackInput>> = BTreeMap::new();
        for item in items {
            let group_name = item
                .meta
                .album
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(folder_title)
                .to_string();
            groups.entry(group_name).or_default().push(item);
        }

        let mut out = Vec::new();
        for (g_idx, (alb_title, group_items)) in groups.into_iter().enumerate() {
            if let Some(alb) = build_album_from_scanned_items(
                group_items,
                None,
                &alb_title,
                source_id,
                base_index + g_idx,
            ) {
                out.push(alb);
            }
        }
        return out;
    }

    let single_title = if distinct_embedded_albums.len() == 1 {
        distinct_embedded_albums.into_iter().next().unwrap()
    } else {
        folder_title.to_string()
    };

    build_album_from_scanned_items(
        items,
        folder_cover_uri,
        &single_title,
        source_id,
        base_index,
    )
    .into_iter()
    .collect()
}

fn build_album_from_scanned_items(
    mut items: Vec<ScannedTrackInput>,
    folder_cover_uri: Option<String>,
    alb_title: &str,
    source_id: &str,
    palette_idx: usize,
) -> Option<AlbumDto> {
    items.sort_by(|a, b| {
        let na = a.meta.track_no.or_else(|| parse_leading_track_no(&a.sort_path));
        let nb = b.meta.track_no.or_else(|| parse_leading_track_no(&b.sort_path));
        match (na, nb) {
            (Some(ta), Some(tb)) if ta != tb => ta.cmp(&tb),
            _ => a.sort_path.cmp(&b.sort_path),
        }
    });

    let mut tracks = Vec::new();
    let mut first_embedded_cover: Option<String> = None;
    let mut inferred_artist = String::new();
    let mut inferred_year = String::new();

    for (i, item) in items.into_iter().enumerate() {
        let ScannedTrackInput {
            sort_path,
            raw_stem,
            audio_uri,
            meta,
            lrc_text,
            stem_cover_uri,
        } = item;

        if first_embedded_cover.is_none() {
            first_embedded_cover = meta.embedded_cover.clone();
        }
        if inferred_year.is_empty() {
            if let Some(y) = &meta.year {
                inferred_year = y.clone();
            }
        }

        let parsed_lrc = parse_lrc_sources(lrc_text.as_deref(), meta.embedded_lyrics.as_deref());

        let title = meta
            .title
            .filter(|s| !s.trim().is_empty())
            .or_else(|| parsed_lrc.title.clone())
            .unwrap_or_else(|| strip_leading_track_num(&raw_stem));

        let artist = meta
            .artist
            .filter(|s| !s.trim().is_empty())
            .or_else(|| meta.album_artist.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| parsed_lrc.artist.clone())
            .unwrap_or_default();

        if inferred_artist.is_empty() && !artist.is_empty() {
            inferred_artist = artist.clone();
        }

        let track_cover = stem_cover_uri.or(meta.embedded_cover);

        let lyrics = if parsed_lrc.lines.is_empty() {
            vec![LyricLineDto {
                t: None,
                text: title.clone(),
                subtext: None,
            }]
        } else {
            parsed_lrc.lines
        };

        let dur = meta.duration_secs.unwrap_or(0.0);
        let track_no = meta
            .track_no
            .or_else(|| parse_leading_track_no(&sort_path))
            .unwrap_or(i + 1);

        tracks.push(TrackDto {
            no: track_no,
            title: title.clone(),
            artist,
            audio: audio_uri,
            cover: track_cover,
            duration: dur,
            full_dur: if dur > 0.2 {
                format_mm_ss(dur)
            } else {
                "--:--".to_string()
            },
            env: synthesize_smooth_envelope(&title),
            lyrics,
        });
    }

    if tracks.is_empty() {
        return None;
    }

    let alb_cover = folder_cover_uri
        .or(first_embedded_cover)
        .or_else(|| tracks.iter().find_map(|t| t.cover.clone()))
        .unwrap_or_default();

    let palette = PHOSPHOR_PALETTES[palette_idx % PHOSPHOR_PALETTES.len()];

    Some(AlbumDto {
        id: format!(
            "{}-{}",
            slugify(source_id, palette_idx),
            slugify(alb_title, palette_idx)
        ),
        source_id: source_id.to_string(),
        code: String::new(),
        title: alb_title.to_string(),
        artist: inferred_artist.clone(),
        subtitle: inferred_artist,
        year: inferred_year,
        freq: String::new(),
        cover: alb_cover,
        phosphor: palette.0.to_string(),
        glow: palette.1.to_string(),
        tracks,
    })
}

fn build_loose_track_albums(
    app: &AppHandle,
    loose: &[LooseTrackEntry],
    cover_cache: &Path,
    base_offset: usize,
    out: &mut Vec<AlbumDto>,
) {
    let mut groups: BTreeMap<String, Vec<(&LooseTrackEntry, ExtractedAudioMeta, ParsedLrc)>> =
        BTreeMap::new();

    for item in loose {
        let path = Path::new(&item.audio);
        if !path.is_file() {
            continue;
        }
        if let Some(parent) = path.parent() {
            allow_asset_dir(app, parent);
        }
        if let Some(cov) = &item.cover {
            if let Some(parent) = Path::new(cov).parent() {
                allow_asset_dir(app, parent);
            }
        }
        let meta = extract_audio_meta(path, cover_cache);
        let lrc_path = item
            .lrc
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| find_sidecar_lrc(path));
        let parsed_lrc = load_lrc(lrc_path.as_deref(), meta.embedded_lyrics.as_deref());

        let group_title = meta
            .album
            .as_deref()
            .or(parsed_lrc.album.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("单曲")
            .to_string();
        groups
            .entry(group_title)
            .or_default()
            .push((item, meta, parsed_lrc));
    }

    for (alb_title, entries) in groups {
        let palette_idx = base_offset + out.len();
        let palette = PHOSPHOR_PALETTES[palette_idx % PHOSPHOR_PALETTES.len()];
        let mut tracks = Vec::new();
        let mut alb_artist = String::new();

        for (i, (entry, meta, parsed_lrc)) in entries.into_iter().enumerate() {
            let audio_path = Path::new(&entry.audio);
            let raw_stem = audio_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled");
            let title = meta
                .title
                .filter(|s| !s.trim().is_empty())
                .or(parsed_lrc.title)
                .unwrap_or_else(|| strip_leading_track_num(raw_stem));
            let artist = meta.artist.or(parsed_lrc.artist).unwrap_or_default();
            if alb_artist.is_empty() && !artist.is_empty() {
                alb_artist = artist.clone();
            }

            let track_cover = entry
                .cover
                .clone()
                .filter(|p| Path::new(p).is_file())
                .or(meta.embedded_cover);

            let lyrics = if parsed_lrc.lines.is_empty() {
                vec![LyricLineDto {
                    t: None,
                    text: title.clone(),
                    subtext: None,
                }]
            } else {
                parsed_lrc.lines
            };
            let dur = meta.duration_secs.unwrap_or(0.0);

            tracks.push(TrackDto {
                no: meta.track_no.unwrap_or(i + 1),
                title: title.clone(),
                artist,
                audio: entry.audio.clone(),
                cover: track_cover,
                duration: dur,
                full_dur: if dur > 0.2 {
                    format_mm_ss(dur)
                } else {
                    "--:--".to_string()
                },
                env: synthesize_smooth_envelope(&title),
                lyrics,
            });
        }

        if tracks.is_empty() {
            continue;
        }

        let slug = slugify(&alb_title, palette_idx);
        let alb_cover = tracks
            .iter()
            .find_map(|t| t.cover.clone())
            .unwrap_or_default();

        out.push(AlbumDto {
            id: format!("loose-{slug}"),
            source_id: format!("loose:{slug}"),
            code: String::new(),
            title: alb_title,
            artist: alb_artist.clone(),
            subtitle: alb_artist,
            year: String::new(),
            freq: String::new(),
            cover: alb_cover,
            phosphor: palette.0.to_string(),
            glow: palette.1.to_string(),
            tracks,
        });
    }
}

// ============================================================================
// EMBEDDED AUDIO TAGS, ARTWORK CACHE & MULTI-ENCODING LRC PARSER
// ============================================================================

fn extract_audio_meta(path: &Path, cover_cache: &Path) -> ExtractedAudioMeta {
    let Ok(tagged) = Probe::open(path).and_then(|p| p.read()) else {
        return ExtractedAudioMeta::default();
    };
    extract_meta_from_tagged_file(&tagged, |pic_bytes| {
        cache_embedded_picture(pic_bytes, cover_cache).map(|p| clean_path_string(&p))
    })
}

fn probe_tagged_file_from_bytes(bytes: &[u8], ext: &str) -> Option<lofty::file::TaggedFile> {
    if let Ok(guessed) = Probe::new(Cursor::new(bytes)).guess_file_type() {
        if guessed.file_type().is_some() {
            if let Ok(tf) = guessed.read() {
                return Some(tf);
            }
        }
    }
    let ft = FileType::from_ext(ext)?;
    Probe::new(Cursor::new(bytes)).set_file_type(ft).read().ok()
}

fn extract_zip_audio_meta(
    bytes: &[u8],
    ext: &str,
    archive_id: &str,
    entry_idx: usize,
) -> ExtractedAudioMeta {
    let Some(tagged) = probe_tagged_file_from_bytes(bytes, ext) else {
        return ExtractedAudioMeta::default();
    };
    extract_meta_from_tagged_file(&tagged, |pic_bytes| {
        if pic_bytes.len() < 16 {
            return None;
        }
        let (_, pic_ext) = detect_image_mime_and_ext(pic_bytes);
        Some(format!("komapack://{archive_id}/embedded/{entry_idx}.{pic_ext}"))
    })
}

fn extract_embedded_picture_bytes(bytes: &[u8], ext: &str) -> Option<Vec<u8>> {
    let tagged = probe_tagged_file_from_bytes(bytes, ext)?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pic = tag
        .pictures()
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())?;
    if pic.data().len() < 16 {
        return None;
    }
    Some(pic.data().to_vec())
}

fn extract_meta_from_tagged_file<F>(
    tagged: &lofty::file::TaggedFile,
    resolve_cover: F,
) -> ExtractedAudioMeta
where
    F: FnOnce(&[u8]) -> Option<String>,
{
    let dur = tagged.properties().duration().as_secs_f64();
    let duration_secs = (dur > 0.2).then_some(dur);

    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return ExtractedAudioMeta {
            duration_secs,
            ..Default::default()
        };
    };

    let embedded_cover = tag
        .pictures()
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())
        .and_then(|pic| resolve_cover(pic.data()));

    let embedded_lyrics = tag
        .get_string(&ItemKey::Lyrics)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    ExtractedAudioMeta {
        title: tag
            .title()
            .map(|s| normalize_nfc(s.trim()))
            .filter(|s| !s.is_empty()),
        artist: tag
            .artist()
            .map(|s| normalize_nfc(s.trim()))
            .filter(|s| !s.is_empty()),
        album: tag
            .album()
            .map(|s| normalize_nfc(s.trim()))
            .filter(|s| !s.is_empty()),
        album_artist: tag
            .get_string(&ItemKey::AlbumArtist)
            .map(|s| normalize_nfc(s.trim()))
            .filter(|s| !s.is_empty()),
        track_no: tag.track().map(|n| n as usize),
        year: tag.year().map(|y| y.to_string()),
        duration_secs,
        embedded_cover,
        embedded_lyrics,
    }
}

fn detect_image_mime_and_ext(bytes: &[u8]) -> (&'static str, &'static str) {
    if bytes.starts_with(b"\x89PNG") {
        ("image/png", "png")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ("image/webp", "webp")
    } else if bytes.starts_with(b"GIF8") {
        ("image/gif", "gif")
    } else if bytes.starts_with(b"BM") {
        ("image/bmp", "bmp")
    } else {
        ("image/jpeg", "jpg")
    }
}

fn audio_mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "aac" => "audio/aac",
        "webm" => "audio/webm",
        _ => "application/octet-stream",
    }
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_str(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn cache_embedded_picture(bytes: &[u8], cache_dir: &Path) -> Option<PathBuf> {
    if bytes.len() < 16 {
        return None;
    }
    let (_, ext) = detect_image_mime_and_ext(bytes);

    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |acc, &b| {
        (acc ^ (b as u64)).wrapping_mul(0x100000001b3)
    });
    let file_path = cache_dir.join(format!("{hash:016x}.{ext}"));
    if !file_path.exists() {
        fs::write(&file_path, bytes).ok()?;
    }
    Some(file_path)
}

fn parse_lrc_sources(sidecar_raw: Option<&str>, embedded_raw: Option<&str>) -> ParsedLrc {
    if let Some(raw) = sidecar_raw {
        let parsed = parse_lrc_full(raw);
        if !parsed.lines.is_empty() {
            return parsed;
        }
    }
    if let Some(raw) = embedded_raw {
        let parsed = parse_lrc_full(raw);
        if !parsed.lines.is_empty() {
            return parsed;
        }
    }
    ParsedLrc::default()
}

fn load_lrc(lrc_file: Option<&Path>, embedded_raw: Option<&str>) -> ParsedLrc {
    let sidecar_raw = lrc_file
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| decode_text_smart(&bytes));
    parse_lrc_sources(sidecar_raw.as_deref(), embedded_raw)
}

fn decode_text_smart(bytes: &[u8]) -> String {
    if let Some(stripped) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if let Ok(s) = std::str::from_utf8(stripped) {
            return s.to_string();
        }
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (cow, _, _) = UTF_16LE.decode(&bytes[2..]);
        return cow.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (cow, _, _) = UTF_16BE.decode(&bytes[2..]);
        return cow.into_owned();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let (gbk_cow, _, gbk_err) = GB18030.decode(bytes);
    if !gbk_err {
        return gbk_cow.into_owned();
    }
    let (sjis_cow, _, _) = SHIFT_JIS.decode(bytes);
    sjis_cow.into_owned()
}

pub fn parse_lrc_full(raw: &str) -> ParsedLrc {
    let mut out = ParsedLrc::default();
    let mut offset_sec = 0.0f64;
    let mut timed: Vec<(f64, String)> = Vec::new();
    let mut untimed: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim().trim_start_matches('\u{feff}');
        if trimmed.is_empty() {
            continue;
        }

        let mut rest = trimmed;
        let mut line_times: Vec<f64> = Vec::new();
        let mut had_bracket_tag = false;

        while let Some(after_open) = rest.strip_prefix('[') {
            let Some((tag, after_close)) = after_open.split_once(']') else {
                break;
            };
            had_bracket_tag = true;
            if let Some(sec) = parse_time_tag(tag) {
                line_times.push(sec);
                rest = after_close;
            } else {
                if let Some((k, v)) = tag.split_once(':') {
                    let val = v.trim();
                    if !val.is_empty() {
                        match k.trim().to_ascii_lowercase().as_str() {
                            "ti" => out.title = Some(normalize_nfc(val)),
                            "ar" => out.artist = Some(normalize_nfc(val)),
                            "al" => out.album = Some(normalize_nfc(val)),
                            "offset" => {
                                if let Ok(ms) = val.parse::<f64>() {
                                    offset_sec = ms / 1000.0;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                rest = "";
                break;
            }
        }

        let body = normalize_nfc(rest.trim());
        if !line_times.is_empty() {
            if !body.is_empty() && body != "//" {
                for t in line_times {
                    timed.push(((t - offset_sec).max(0.0), body.clone()));
                }
            }
        } else if !had_bracket_tag && !body.is_empty() {
            untimed.push(body);
        }
    }

    if !timed.is_empty() {
        timed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut merged: Vec<LyricLineDto> = Vec::with_capacity(timed.len());
        for (t, text) in timed {
            if let Some(prev) = merged.last_mut() {
                if let Some(pt) = prev.t {
                    if (t - pt).abs() <= 0.08 && prev.text != text && prev.subtext.is_none() {
                        prev.subtext = Some(text);
                        continue;
                    }
                }
            }
            merged.push(LyricLineDto {
                t: Some(t),
                text,
                subtext: None,
            });
        }
        out.lines = merged;
        return out;
    }

    out.lines = untimed
        .into_iter()
        .map(|text| LyricLineDto {
            t: None,
            text,
            subtext: None,
        })
        .collect();
    out
}

fn parse_time_tag(tag: &str) -> Option<f64> {
    let (mm, sec_part) = tag.trim().split_once(':')?;
    let m: f64 = mm.trim().parse().ok()?;
    let normalized_sec = sec_part.trim().replace(':', ".");
    let s: f64 = normalized_sec.parse().ok()?;
    Some(m * 60.0 + s)
}

// ============================================================================
// FILESYSTEM & STRING HELPERS
// ============================================================================

fn app_storage_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_local_data_dir()
        .or_else(|_| app.path().app_data_dir())
        .unwrap_or_else(|_| std::env::temp_dir().join("koma"))
}

fn pack_meta_cache_dir(app: &AppHandle) -> PathBuf {
    app_storage_dir(app).join("cache").join("pack_meta")
}

fn pack_meta_file_path(app: &AppHandle, archive_id: &str) -> PathBuf {
    pack_meta_cache_dir(app).join(format!("{}.json", sanitize_fs_component(archive_id)))
}

fn cover_cache_dir(app: &AppHandle) -> PathBuf {
    app_storage_dir(app).join("cache").join("covers")
}

fn registry_file_path(app: &AppHandle) -> PathBuf {
    app_storage_dir(app).join("library.json")
}

fn read_registry(app: &AppHandle) -> LibraryRegistry {
    let p = registry_file_path(app);
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_registry(app: &AppHandle, reg: &LibraryRegistry) -> Result<(), String> {
    let p = registry_file_path(app);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(reg).map_err(|e| e.to_string())?;
    fs::write(&p, json).map_err(|e| e.to_string())
}

fn allow_asset_dir(app: &AppHandle, dir: &Path) {
    let _ = app.asset_protocol_scope().allow_directory(dir, true);
}

pub fn normalize_nfc(s: &str) -> String {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc()
        .normalize(s)
        .into_owned()
}

fn normalize_stem_key(s: &str) -> String {
    normalize_nfc(s.trim()).to_lowercase()
}

fn find_sidecar_lrc(audio_path: &Path) -> Option<PathBuf> {
    for ext in ["lrc", "LRC", "txt"] {
        let candidate = audio_path.with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let parent = audio_path.parent()?;
    let target_stem = normalize_stem_key(audio_path.file_stem()?.to_str()?);
    let entries = fs::read_dir(parent).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = ext_lower(&path);
        if matches!(ext.as_str(), "lrc" | "txt") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if normalize_stem_key(stem) == target_stem {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn find_matching_stem_image(images: &[PathBuf], audio_stem: &str) -> Option<PathBuf> {
    let target = normalize_stem_key(audio_stem);
    images
        .iter()
        .find(|img| {
            img.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| normalize_stem_key(s) == target)
                .unwrap_or(false)
        })
        .cloned()
}

fn clean_path_string(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

fn normalize_cmp_key(p: &Path) -> String {
    let cleaned = normalize_nfc(&clean_path_string(p).replace('\\', "/"));
    if crate::platform::CASE_INSENSITIVE_FS {
        cleaned.to_lowercase()
    } else {
        cleaned
    }
}

fn ext_lower(p: &Path) -> String {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_audio_ext(ext: &str) -> bool {
    matches!(
        ext,
        "flac" | "m4a" | "mp3" | "wav" | "ogg" | "opus" | "aac" | "webm"
    )
}

fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp")
}

fn format_mm_ss(sec: f64) -> String {
    let total = sec.round().max(0.0) as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn parse_leading_track_no(path: &Path) -> Option<usize> {
    let stem = path.file_stem()?.to_str()?.trim();
    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 3 {
        return None;
    }
    digits.parse::<usize>().ok()
}

fn strip_leading_track_num(stem: &str) -> String {
    let nfc = normalize_nfc(stem);
    let trimmed = nfc.trim();
    let stripped = trimmed
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches(['.', '-', '_', ' ']);
    if stripped.is_empty() {
        trimmed.to_string()
    } else {
        stripped.to_string()
    }
}

fn strip_leading_year_prefix(folder_name: &str) -> String {
    let trimmed = folder_name.trim();
    if trimmed.len() > 6 && trimmed.chars().take(4).all(|c| c.is_ascii_digit()) {
        let rest = trimmed[4..].trim_start_matches([' ', '-', '_', '·', '.']);
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    trimmed.to_string()
}

fn slugify(name: &str, fallback_idx: usize) -> String {
    let normalized = normalize_nfc(name);
    let mut s = String::new();
    for c in normalized.chars() {
        if c.is_alphanumeric() {
            for lc in c.to_lowercase() {
                s.push(lc);
            }
        } else if matches!(c, '-' | '_' | ' ') && !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        format!("item-{}", fallback_idx + 1)
    } else {
        s
    }
}

fn synthesize_smooth_envelope(seed_str: &str) -> Vec<u8> {
    let seed: u64 = seed_str
        .bytes()
        .fold(1469598103934665603u64, |acc, b| {
            acc.wrapping_mul(1099511628211).wrapping_add(b as u64)
        });
    (0..120)
        .map(|i| {
            let phase = (i as f64) * 0.21 + ((seed % 97) as f64) * 0.07;
            let v = 52.0 + 26.0 * phase.sin() + 14.0 * (phase * 2.3).cos();
            v.clamp(16.0, 94.0) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bilingual_and_header_lrc() {
        let raw = "\u{feff}[ti:藍二乗]\n[ar:ヨルシカ]\n[al:だから僕は音楽を辞めた]\n[offset:500]\n[00:10.50][00:30.50]変わらない風景\n[00:10.52]一成不变的风景\n";
        let parsed = parse_lrc_full(raw);
        assert_eq!(parsed.title.as_deref(), Some("藍二乗"));
        assert_eq!(parsed.artist.as_deref(), Some("ヨルシカ"));
        assert_eq!(parsed.album.as_deref(), Some("だから僕は音楽を辞めた"));
        assert_eq!(parsed.lines.len(), 2);
        assert_eq!(parsed.lines[0].t, Some(10.0));
        assert_eq!(parsed.lines[0].subtext.as_deref(), Some("一成不变的风景"));
    }

    #[test]
    fn allocates_distinct_ids_for_multiple_packs() {
        let existing = vec![InstalledArchiveEntry {
            id: "album-a".to_string(),
            name: "album-a".to_string(),
            path: "D:/a/album-a.zip".to_string(),
            dir: String::new(),
            origin_path: None,
            enabled: true,
        }];
        assert_eq!(allocate_unique_archive_id("album-a", &existing), "album-a-2");
        assert_eq!(allocate_unique_archive_id("album-b", &existing), "album-b");
    }

    #[test]
    fn decodes_gb18030_and_utf16le() {
        let (gbk_bytes, _, _) = GB18030.encode("[00:05.00]夏日与花火");
        assert_eq!(decode_text_smart(&gbk_bytes), "[00:05.00]夏日与花火");

        let mut utf16: Vec<u8> = vec![0xFF, 0xFE];
        for u in "[00:08.00]夜行".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text_smart(&utf16), "[00:08.00]夜行");
    }

    #[test]
    fn scans_local_untracked_dakaraboku_pack_if_present() {
        let zip_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("packs")
            .join("だから僕は音楽を辞めた.zip");
        if !zip_path.exists() {
            return;
        }
        let albums = scan_zip_archive_in_place(&zip_path, "dakaraboku", "zip:dakaraboku", 0)
            .expect("scan zip in place");

        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].tracks.len(), 14);
        assert!(albums[0].cover.starts_with("komapack://dakaraboku/"));
        for tr in &albums[0].tracks {
            assert!(tr.audio.starts_with("komapack://dakaraboku/entry/"));
            assert!(tr.duration > 90.0, "track {} duration too short: {}", tr.title, tr.duration);
            assert!(!tr.lyrics.is_empty(), "track {} missing lyrics", tr.title);
        }
        let aijijou = &albums[0].tracks[1];
        assert!(aijijou.lyrics.len() >= 30);
        assert!(aijijou.lyrics.iter().any(|l| l.subtext.is_some()));
    }

    #[test]
    fn scans_zip_in_place_without_extraction_and_serves_http_ranges() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let tmp_dir = std::env::temp_dir().join(format!("koma_zip_inplace_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();
        let zip_path = tmp_dir.join("2024. Demo Pack.zip");

        // Build minimal 44-byte WAV header + 16000 zero PCM bytes (0.5s @ 16kHz 16-bit mono)
        let mut wav = Vec::with_capacity(44 + 16000);
        let data_len: u32 = 16000;
        let riff_len: u32 = 36 + data_len;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&riff_len.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&32000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.resize(44 + data_len as usize, 0);

        let png_stub: [u8; 16] = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R',
        ];
        let lrc_text = "[00:00.10]Hello In-Place\n[00:00.12]原地流式播放\n";

        {
            let file = File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let opts = SimpleFileOptions::default();
            writer.start_file("Demo Album/01. Intro.wav", opts).unwrap();
            writer.write_all(&wav).unwrap();
            writer.start_file("Demo Album/01. Intro.lrc", opts).unwrap();
            writer.write_all(lrc_text.as_bytes()).unwrap();
            writer.start_file("Demo Album/cover.png", opts).unwrap();
            writer.write_all(&png_stub).unwrap();
            writer.finish().unwrap();
        }

        validate_zip_archive(&zip_path).expect("valid zip pack");

        let albums = scan_zip_archive_in_place(&zip_path, "demo-pack", "zip:demo-pack", 0)
            .expect("scan zip in place");
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].tracks.len(), 1);
        assert!(albums[0].cover.starts_with("komapack://demo-pack/entry/"));
        assert!(albums[0].tracks[0].audio.starts_with("komapack://demo-pack/entry/"));
        assert!(albums[0].tracks[0].audio.ends_with(".wav"));
        assert_eq!(albums[0].tracks[0].lyrics.len(), 1);
        assert_eq!(
            albums[0].tracks[0].lyrics[0].subtext.as_deref(),
            Some("原地流式播放")
        );

        let get_req = http::Request::builder()
            .method(http::Method::GET)
            .uri("komapack://demo-pack/entry/0.wav")
            .body(Vec::new())
            .unwrap();
        let full_resp = build_media_http_response(&get_req, &wav, "audio/wav");
        assert_eq!(full_resp.status(), StatusCode::OK);
        assert_eq!(full_resp.body().len(), wav.len());

        let range_req = http::Request::builder()
            .method(http::Method::GET)
            .uri("komapack://demo-pack/entry/0.wav")
            .header(header::RANGE, "bytes=0-15")
            .body(Vec::new())
            .unwrap();
        let range_resp = build_media_http_response(&range_req, &wav, "audio/wav");
        assert_eq!(range_resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(range_resp.body().as_slice(), &wav[0..16]);
        assert_eq!(
            range_resp
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok()),
            Some(format!("bytes 0-15/{}", wav.len()).as_str())
        );

        // Legacy registry migration removes AppData/archives and promotes origin_path -> path
        let legacy_archives = tmp_dir.join("legacy_archives");
        fs::create_dir_all(legacy_archives.join("old-pack")).unwrap();
        let mut reg = LibraryRegistry {
            installed_archives: vec![InstalledArchiveEntry {
                id: "old-pack".to_string(),
                name: "Old Pack".to_string(),
                path: String::new(),
                dir: legacy_archives.join("old-pack").to_string_lossy().to_string(),
                origin_path: Some(zip_path.to_string_lossy().to_string()),
                enabled: true,
            }],
            ..Default::default()
        };
        assert!(migrate_legacy_registry(&mut reg, Some(&legacy_archives)));
        assert!(!legacy_archives.exists());
        assert_eq!(
            reg.installed_archives[0].path,
            zip_path.to_string_lossy().to_string()
        );
        assert!(reg.installed_archives[0].dir.is_empty());
        assert!(reg.installed_archives[0].origin_path.is_none());

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn matches_nfd_and_nfc_japanese_dakuten_stems_and_normalizes_zip_paths() {
        let nfd_stem = "01. た\u{3099}から僕は音楽を辞めた";
        let nfc_stem = "01. だから僕は音楽を辞めた";
        assert_eq!(normalize_stem_key(nfd_stem), normalize_stem_key(nfc_stem));
        assert_eq!(slugify(nfd_stem, 0), slugify(nfc_stem, 0));

        let nfd_zip_rel = "た\u{3099}から僕は音楽を辞めた/01. ハ\u{309A}レート\u{3099}.m4a";
        let clean_rel = sanitize_zip_rel_path(nfd_zip_rel).expect("sanitized rel path");
        assert_eq!(
            clean_rel,
            PathBuf::from("だから僕は音楽を辞めた").join("01. パレード.m4a")
        );

        let nfc_img = PathBuf::from(format!("/tmp/{nfc_stem}.jpg"));
        assert_eq!(
            find_matching_stem_image(&[nfc_img.clone()], nfd_stem),
            Some(nfc_img)
        );

        let tmp_dir = std::env::temp_dir().join(format!("koma_nfd_lrc_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();
        let nfd_audio = tmp_dir.join(format!("{nfd_stem}.m4a"));
        let nfc_lrc = tmp_dir.join(format!("{nfc_stem}.lrc"));
        fs::write(&nfd_audio, b"").unwrap();
        fs::write(&nfc_lrc, b"[00:01.00]test\n").unwrap();

        let matched = find_sidecar_lrc(&nfd_audio).expect("sidecar lrc should match across NFD/NFC");
        assert_eq!(normalize_nfc(&matched.file_name().unwrap().to_string_lossy()), format!("{nfc_stem}.lrc"));
        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
