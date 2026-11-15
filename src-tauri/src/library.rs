// RUST DEEP MODULE B: LOCAL YORUSHIKA DISCOGRAPHY SCANNER
// Automatically scans ~/Music/Yorushika (or ~/Music/ヨルシカ) for lossless/standard album directories,
// pairing audio tracks (.flac/.m4a/.mp3/.wav) with cover art and .lrc sidecar lyrics.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct LyricLineDto {
    pub t: Option<f64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDto {
    pub no: usize,
    pub title: String,
    pub audio: String,
    pub clip_dur: f64,
    pub full_dur: String,
    pub hook_idx: usize,
    pub env: Vec<u8>,
    pub lyrics: Vec<LyricLineDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AlbumDto {
    pub id: String,
    pub code: String,
    pub title: String,
    pub subtitle: String,
    pub year: String,
    pub freq: String,
    pub cover: String,
    pub phosphor: String,
    pub glow: String,
    pub tracks: Vec<TrackDto>,
}

#[tauri::command]
pub fn scan_local_yorushika() -> Vec<AlbumDto> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };

    let candidates = [
        home.join("Music").join("Yorushika"),
        home.join("Music").join("ヨルシカ"),
    ];

    let mut discovered = Vec::new();
    for root in candidates {
        if !root.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|x| x.path()))
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();

        for (idx, album_dir) in dirs.into_iter().enumerate() {
            if let Some(album) = parse_album_directory(&album_dir, discovered.len() + idx) {
                discovered.push(album);
            }
        }
    }

    discovered
}

fn parse_album_directory(dir: &Path, index: usize) -> Option<AlbumDto> {
    let album_title = dir.file_name()?.to_string_lossy().to_string();
    let entries = fs::read_dir(dir).ok()?;

    let mut audio_files = Vec::new();
    let mut cover_path: Option<PathBuf> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()) else {
            continue;
        };
        match ext.as_str() {
            "flac" | "m4a" | "mp3" | "wav" | "ogg" => audio_files.push(path),
            "jpg" | "jpeg" | "png" | "webp" => {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if cover_path.is_none() || stem == "cover" || stem == "folder" || stem == "front" {
                    cover_path = Some(path);
                }
            }
            _ => {}
        }
    }

    if audio_files.is_empty() {
        return None;
    }
    audio_files.sort();

    let mut tracks = Vec::new();
    for (i, audio_path) in audio_files.into_iter().enumerate() {
        let raw_stem = audio_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled");
        let clean_title = strip_leading_track_num(raw_stem);

        let lrc_path = audio_path.with_extension("lrc");
        let lyrics = fs::read_to_string(&lrc_path)
            .ok()
            .map(|raw| parse_lrc(&raw))
            .unwrap_or_else(|| {
                vec![LyricLineDto {
                    t: Some(0.0),
                    text: clean_title.clone(),
                }]
            });

        // Organic pseudo-envelope seeded by filename so oscilloscope breathes smoothly
        let env = synthesize_smooth_envelope(&clean_title);

        tracks.push(TrackDto {
            no: i + 1,
            title: clean_title,
            audio: audio_path.to_string_lossy().to_string(),
            clip_dur: 210.0,
            full_dur: "FLAC".to_string(),
            hook_idx: 0,
            env,
            lyrics,
        });
    }

    let palette = if index % 2 == 0 {
        ("#dff2ff", "rgba(130, 202, 255, 0.44)")
    } else {
        ("#ffe9cf", "rgba(255, 174, 96, 0.42)")
    };

    Some(AlbumDto {
        id: format!("local-{}", slugify(&album_title, index)),
        code: format!("LC-{:02}", index + 1),
        title: album_title,
        subtitle: "Local Lossless Library".to_string(),
        year: "LOCAL".to_string(),
        freq: format!("{:.1}", 21.2 + (index as f64) * 0.8),
        cover: cover_path
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/library/covers/dakaraboku.jpg".to_string()),
        phosphor: palette.0.to_string(),
        glow: palette.1.to_string(),
        tracks,
    })
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

fn slugify(name: &str, fallback_idx: usize) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if s.is_empty() {
        format!("album{}", fallback_idx + 1)
    } else {
        s
    }
}

fn parse_lrc(raw: &str) -> Vec<LyricLineDto> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        if let Some(rest) = s.strip_prefix('[') {
            if let Some((tag, body)) = rest.split_once(']') {
                if let Some((mm, ss)) = tag.split_once(':') {
                    if let (Ok(m), Ok(sec)) = (mm.trim().parse::<f64>(), ss.trim().parse::<f64>()) {
                        let txt = body.trim();
                        if !txt.is_empty() {
                            out.push(LyricLineDto {
                                t: Some(m * 60.0 + sec),
                                text: txt.to_string(),
                            });
                        }
                    }
                }
            }
        } else {
            out.push(LyricLineDto {
                t: None,
                text: s.to_string(),
            });
        }
    }
    out
}

fn synthesize_smooth_envelope(seed_str: &str) -> Vec<u8> {
    let seed: u64 = seed_str
        .bytes()
        .fold(1469598103934665603u64, |acc, b| acc.wrapping_mul(1099511628211).wrapping_add(b as u64));
    (0..120)
        .map(|i| {
            let phase = (i as f64) * 0.21 + ((seed % 97) as f64) * 0.07;
            let v = 52.0 + 26.0 * phase.sin() + 14.0 * (phase * 2.3).cos();
            v.clamp(16.0, 94.0) as u8
        })
        .collect()
}
