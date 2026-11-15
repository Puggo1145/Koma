// RUST DEEP MODULE B: UNIVERSAL MULTI-PACK & LOCAL MUSIC LIBRARY ENGINE
// Supports importing & managing multiple standard .zip song packs, local folders, and loose files:
//   - Standard .zip archives (UTF-8 / GB18030 / Shift-JIS) — extracted & managed per-pack in AppData/archives/
//   - Local album / artist / flat music folders — linked in-place without duplicating disk space
//   - Per-source enable/disable toggle, re-import/relocate, reveal in Explorer/Finder, remove, and rescan
//   - ID3v2 / FLAC Vorbis / MP4 tags, exact duration, hash-cached embedded cover art, and bilingual .lrc

use encoding_rs::{GB18030, SHIFT_JIS, UTF_16BE, UTF_16LE};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
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
    dir: String,
    #[serde(default)]
    origin_path: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
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
    embedded_cover: Option<PathBuf>,
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
    reg.installed_archives
        .retain(|a| Path::new(&a.dir).is_dir());
    let _ = write_registry(&app, &reg);
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
        let existing_idx = reg
            .installed_archives
            .iter()
            .position(|a| a.id == archive_id);
        let Some(idx) = existing_idx else {
            return Ok(None);
        };

        let existing_origin = reg.installed_archives[idx]
            .origin_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_file());

        let chosen_zip = if let Some(valid_origin) = existing_origin {
            Some(valid_origin)
        } else {
            tauri::async_runtime::spawn_blocking(move || {
                rfd::FileDialog::new()
                    .set_title("选择要重新导入的歌曲压缩包")
                    .add_filter("歌曲压缩包", &["zip"])
                    .pick_file()
            })
            .await
            .map_err(|e| e.to_string())?
        };

        let Some(zip_path) = chosen_zip else {
            return Ok(None);
        };

        let was_enabled = reg.installed_archives[idx].enabled;
        let old_dir = reg.installed_archives[idx].dir.clone();
        let updated = extract_zip_archive_with_id(&app, &zip_path, Some(archive_id))?;
        if normalize_cmp_key(Path::new(&old_dir)) != normalize_cmp_key(Path::new(&updated.dir)) {
            let _ = fs::remove_dir_all(&old_dir);
        }
        reg.installed_archives[idx] = InstalledArchiveEntry {
            enabled: was_enabled,
            ..updated
        };
        write_registry(&app, &reg)?;
        return Ok(Some(emit_current_state(&app, &reg, None)));
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
            write_registry(&app, &reg)?;
        }

        return Ok(Some(emit_current_state(&app, &reg, None)));
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
            .map(|a| PathBuf::from(&a.dir))
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

    #[cfg(target_os = "windows")]
    {
        let win_path = clean_path_string(&dir).replace('/', "\\");
        Command::new("explorer.exe")
            .arg(win_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub fn remove_library_source(app: AppHandle, source_id: String) -> LibraryStateDto {
    let mut reg = read_registry(&app);

    if let Some(archive_id) = source_id
        .strip_prefix("zip:")
        .or_else(|| source_id.strip_prefix("pack:"))
    {
        reg.installed_archives.retain(|a| {
            if a.id == archive_id {
                let _ = fs::remove_dir_all(&a.dir);
                false
            } else {
                true
            }
        });
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
    let archives_root = archives_storage_dir(&app);
    let _ = fs::remove_dir_all(&archives_root);
    let _ = fs::create_dir_all(&archives_root);

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
                let raw_origin_key = normalize_cmp_key(raw_path);
                let stem = raw_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("pack");
                let base_id = slugify(stem, reg.installed_archives.len());

                // Check if re-importing the exact same origin zip (or upgrading a legacy entry with no origin_path)
                let matching_idx = reg.installed_archives.iter().position(|a| {
                    a.origin_path
                        .as_deref()
                        .map(|op| normalize_cmp_key(Path::new(op)) == raw_origin_key)
                        .unwrap_or(a.id == base_id)
                });

                let assigned_id = if let Some(idx) = matching_idx {
                    reg.installed_archives[idx].id.clone()
                } else {
                    allocate_unique_archive_id(&base_id, &reg.installed_archives)
                };

                let installed = extract_zip_archive_with_id(app, raw_path, Some(&assigned_id))?;
                if let Some(idx) = matching_idx {
                    let old_dir = reg.installed_archives[idx].dir.clone();
                    if normalize_cmp_key(Path::new(&old_dir))
                        != normalize_cmp_key(Path::new(&installed.dir))
                    {
                        let _ = fs::remove_dir_all(&old_dir);
                    }
                    reg.installed_archives[idx] = installed;
                } else {
                    reg.installed_archives.push(installed);
                }
                last_touched_source = Some(format!("zip:{assigned_id}"));
            }
            ext if is_audio_ext(ext) => {
                loose_audios.push(raw_path.clone());
            }
            "lrc" | "txt" => {
                if let Some(stem) = raw_path.file_stem().and_then(|s| s.to_str()) {
                    loose_lrcs.insert(stem.to_lowercase(), raw_path.clone());
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
            let stem_lower = audio_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();

            let paired_lrc = loose_lrcs
                .get(&stem_lower)
                .cloned()
                .or_else(|| find_sidecar_lrc(&audio_path))
                .map(|p| clean_path_string(&p));

            let paired_cover = loose_covers
                .iter()
                .find(|img| {
                    img.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.eq_ignore_ascii_case(&stem_lower))
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

fn extract_zip_archive_with_id(
    app: &AppHandle,
    archive_path: &Path,
    forced_id: Option<&str>,
) -> Result<InstalledArchiveEntry, String> {
    let archives_root = archives_storage_dir(app);
    extract_zip_to_root(archive_path, &archives_root, forced_id)
}

fn extract_zip_to_root(
    archive_path: &Path,
    archives_root: &Path,
    forced_id: Option<&str>,
) -> Result<InstalledArchiveEntry, String> {
    let file = File::open(archive_path)
        .map_err(|e| format!("无法打开压缩包 {}: {e}", archive_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("无效的 ZIP 压缩文件: {e}"))?;

    let stem = archive_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("music");
    let archive_id = forced_id
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| slugify(stem, 0));

    fs::create_dir_all(archives_root).map_err(|e| e.to_string())?;

    let staging_dir = archives_root.join(format!(".extract-{archive_id}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging_dir);
    fs::create_dir_all(&staging_dir).map_err(|e| e.to_string())?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let decoded_name = decode_text_smart(entry.name_raw());
        let Some(rel_path) = sanitize_zip_rel_path(&decoded_name) else {
            continue;
        };

        let out_path = staging_dir.join(&rel_path);
        if entry.is_dir() || decoded_name.ends_with('/') || decoded_name.ends_with('\\') {
            let _ = fs::create_dir_all(&out_path);
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out_file = File::create(&out_path).map_err(|e| e.to_string())?;
            io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
        }
    }

    let dir_folder_name = if archive_id == slugify(stem, 0) {
        sanitize_fs_component(stem)
    } else {
        sanitize_fs_component(&archive_id)
    };
    let final_dir = archives_root.join(dir_folder_name);
    let _ = fs::remove_dir_all(&final_dir);
    let effective_root = unwrap_single_child_dir(&staging_dir);

    if effective_root == staging_dir {
        fs::rename(&staging_dir, &final_dir).map_err(|e| e.to_string())?;
    } else {
        fs::rename(&effective_root, &final_dir).map_err(|e| e.to_string())?;
        let _ = fs::remove_dir_all(&staging_dir);
    }

    Ok(InstalledArchiveEntry {
        id: archive_id,
        name: stem.trim().to_string(),
        dir: clean_path_string(&final_dir),
        origin_path: Some(clean_path_string(archive_path)),
        enabled: true,
    })
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
    let cleaned: String = s
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

fn unwrap_single_child_dir(dir: &Path) -> PathBuf {
    let mut current = dir.to_path_buf();
    for _ in 0..2 {
        let Ok(entries) = fs::read_dir(&current) else {
            break;
        };
        let valid: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !name.starts_with('.') && name != "Thumbs.db"
            })
            .collect();
        if valid.len() == 1 && valid[0].is_dir() && has_child_subdirs(&valid[0]) {
            current = valid[0].clone();
        } else {
            break;
        }
    }
    current
}

fn has_child_subdirs(dir: &Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .map(|entries| entries.flatten().any(|e| e.path().is_dir()))
        .unwrap_or(false)
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
        let dir = Path::new(&archive.dir);
        let exists = dir.is_dir();
        let source_id = format!("zip:{}", archive.id);
        let mut pack_albums = Vec::new();
        if exists {
            allow_asset_dir(app, dir);
            scan_root_into_albums(
                app,
                dir,
                &source_id,
                &cover_cache,
                active_albums.len(),
                &mut pack_albums,
            );
        }
        let album_count = pack_albums.len();
        let track_count: usize = pack_albums.iter().map(|a| a.tracks.len()).sum();
        let display_name = if !archive.name.trim().is_empty() {
            archive.name.clone()
        } else if album_count == 1 {
            pack_albums[0].title.clone()
        } else {
            dir.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&archive.id)
                .to_string()
        };

        let src_cover = pack_albums
            .iter()
            .find_map(|a| (!a.cover.is_empty()).then(|| a.cover.clone()))
            .unwrap_or_default();

        sources.push(LibrarySourceDto {
            id: source_id,
            kind: "zip".to_string(),
            name: display_name,
            path: archive.dir.clone(),
            origin_path: archive.origin_path.clone(),
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
        let folder_name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&folder_entry.path)
            .to_string();
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
            missing: track_count == 0 && any_missing,
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

fn parse_album_directory(
    dir: &Path,
    source_id: &str,
    cover_cache: &Path,
    base_index: usize,
) -> Vec<AlbumDto> {
    let folder_title = dir
        .file_name()
        .and_then(|s| s.to_str())
        .map(strip_leading_year_prefix)
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

    let metas: Vec<(PathBuf, ExtractedAudioMeta)> = audio_files
        .into_iter()
        .map(|p| {
            let m = extract_audio_meta(&p, cover_cache);
            (p, m)
        })
        .collect();

    let distinct_embedded_albums: HashSet<String> = metas
        .iter()
        .filter_map(|(_, m)| m.album.as_ref().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();

    if folder_cover.is_none() && distinct_embedded_albums.len() >= 2 {
        let mut groups: BTreeMap<String, Vec<(PathBuf, ExtractedAudioMeta)>> = BTreeMap::new();
        for (path, meta) in metas {
            let group_name = meta
                .album
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&folder_title)
                .to_string();
            groups.entry(group_name).or_default().push((path, meta));
        }

        let mut out = Vec::new();
        for (g_idx, (alb_title, items)) in groups.into_iter().enumerate() {
            if let Some(alb) = build_album_from_items(
                items,
                &image_files,
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
        folder_title
    };

    build_album_from_items(
        metas,
        &image_files,
        folder_cover,
        &single_title,
        source_id,
        base_index,
    )
    .into_iter()
    .collect()
}

fn build_album_from_items(
    mut items: Vec<(PathBuf, ExtractedAudioMeta)>,
    image_files: &[PathBuf],
    folder_cover: Option<PathBuf>,
    alb_title: &str,
    source_id: &str,
    palette_idx: usize,
) -> Option<AlbumDto> {
    items.sort_by(|(pa, ma), (pb, mb)| {
        let na = ma.track_no.or_else(|| parse_leading_track_no(pa));
        let nb = mb.track_no.or_else(|| parse_leading_track_no(pb));
        match (na, nb) {
            (Some(ta), Some(tb)) if ta != tb => ta.cmp(&tb),
            _ => pa.cmp(pb),
        }
    });

    let mut tracks = Vec::new();
    let mut first_embedded_cover: Option<PathBuf> = None;
    let mut inferred_artist = String::new();
    let mut inferred_year = String::new();

    for (i, (audio_path, meta)) in items.into_iter().enumerate() {
        if first_embedded_cover.is_none() {
            first_embedded_cover = meta.embedded_cover.clone();
        }
        if inferred_year.is_empty() {
            if let Some(y) = &meta.year {
                inferred_year = y.clone();
            }
        }

        let raw_stem = audio_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("未命名曲目");
        let sidecar_lrc = find_sidecar_lrc(&audio_path);
        let parsed_lrc = load_lrc(sidecar_lrc.as_deref(), meta.embedded_lyrics.as_deref());

        let title = meta
            .title
            .filter(|s| !s.trim().is_empty())
            .or_else(|| parsed_lrc.title.clone())
            .unwrap_or_else(|| strip_leading_track_num(raw_stem));

        let artist = meta
            .artist
            .filter(|s| !s.trim().is_empty())
            .or_else(|| meta.album_artist.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| parsed_lrc.artist.clone())
            .unwrap_or_default();

        if inferred_artist.is_empty() && !artist.is_empty() {
            inferred_artist = artist.clone();
        }

        let track_cover = find_matching_stem_image(image_files, raw_stem)
            .or(meta.embedded_cover)
            .map(|p| clean_path_string(&p));

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
            .or_else(|| parse_leading_track_no(&audio_path))
            .unwrap_or(i + 1);

        tracks.push(TrackDto {
            no: track_no,
            title: title.clone(),
            artist,
            audio: clean_path_string(&audio_path),
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

    let alb_cover = folder_cover
        .or(first_embedded_cover)
        .map(|p| clean_path_string(&p))
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
                .or_else(|| meta.embedded_cover.map(|p| clean_path_string(&p)));

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
        .and_then(|pic| cache_embedded_picture(pic.data(), cover_cache));

    let embedded_lyrics = tag
        .get_string(&ItemKey::Lyrics)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    ExtractedAudioMeta {
        title: tag
            .title()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        artist: tag
            .artist()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        album: tag
            .album()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        album_artist: tag
            .get_string(&ItemKey::AlbumArtist)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        track_no: tag.track().map(|n| n as usize),
        year: tag.year().map(|y| y.to_string()),
        duration_secs,
        embedded_cover,
        embedded_lyrics,
    }
}

fn cache_embedded_picture(bytes: &[u8], cache_dir: &Path) -> Option<PathBuf> {
    if bytes.len() < 16 {
        return None;
    }
    let ext = if bytes.starts_with(b"\x89PNG") {
        "png"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else {
        "jpg"
    };

    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |acc, &b| {
        (acc ^ (b as u64)).wrapping_mul(0x100000001b3)
    });
    let file_path = cache_dir.join(format!("{hash:016x}.{ext}"));
    if !file_path.exists() {
        fs::write(&file_path, bytes).ok()?;
    }
    Some(file_path)
}

fn load_lrc(lrc_file: Option<&Path>, embedded_raw: Option<&str>) -> ParsedLrc {
    if let Some(path) = lrc_file {
        if let Ok(bytes) = fs::read(path) {
            let decoded = decode_text_smart(&bytes);
            let parsed = parse_lrc_full(&decoded);
            if !parsed.lines.is_empty() {
                return parsed;
            }
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
                            "ti" => out.title = Some(val.to_string()),
                            "ar" => out.artist = Some(val.to_string()),
                            "al" => out.album = Some(val.to_string()),
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

        let body = rest.trim();
        if !line_times.is_empty() {
            if !body.is_empty() && body != "//" {
                for t in line_times {
                    timed.push(((t - offset_sec).max(0.0), body.to_string()));
                }
            }
        } else if !had_bracket_tag && !body.is_empty() {
            untimed.push(body.to_string());
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

fn archives_storage_dir(app: &AppHandle) -> PathBuf {
    app_storage_dir(app).join("archives")
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

fn find_sidecar_lrc(audio_path: &Path) -> Option<PathBuf> {
    for ext in ["lrc", "LRC", "txt"] {
        let candidate = audio_path.with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_matching_stem_image(images: &[PathBuf], audio_stem: &str) -> Option<PathBuf> {
    images
        .iter()
        .find(|img| {
            img.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case(audio_stem))
                .unwrap_or(false)
        })
        .cloned()
}

fn clean_path_string(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

fn normalize_cmp_key(p: &Path) -> String {
    let cleaned = clean_path_string(p).replace('\\', "/");
    if cfg!(target_os = "windows") {
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
    let trimmed = stem.trim();
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
    let mut s = String::new();
    for c in name.chars() {
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
            dir: "C:/archives/album-a".to_string(),
            origin_path: Some("D:/a/album-a.zip".to_string()),
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
        let tmp_extract = std::env::temp_dir().join("koma_test_zip_extract");
        let _ = fs::remove_dir_all(&tmp_extract);
        let entry = extract_zip_to_root(&zip_path, &tmp_extract, Some("dakaraboku")).expect("unpack zip");

        let mut candidate_dirs = Vec::new();
        let mut visited = HashSet::new();
        collect_album_dirs_recursive(Path::new(&entry.dir), 0, &mut visited, &mut candidate_dirs);
        assert_eq!(candidate_dirs.len(), 1);

        let cache_dir = std::env::temp_dir().join("koma_test_covers");
        let albums = parse_album_directory(&candidate_dirs[0], "zip:dakaraboku", &cache_dir, 0);
        let _ = fs::remove_dir_all(&tmp_extract);

        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].tracks.len(), 14);
        assert!(!albums[0].cover.is_empty());
        for tr in &albums[0].tracks {
            assert!(tr.duration > 90.0, "track {} duration too short: {}", tr.title, tr.duration);
            assert!(!tr.lyrics.is_empty(), "track {} missing lyrics", tr.title);
        }
        let aijijou = &albums[0].tracks[1];
        assert!(aijijou.lyrics.len() >= 30);
        assert!(aijijou.lyrics.iter().any(|l| l.subtext.is_some()));
    }
}
