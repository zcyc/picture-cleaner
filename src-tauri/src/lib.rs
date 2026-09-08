use chrono::{DateTime, Local};
use exif::{In, Reader, Tag};
use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageReader};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
    time::SystemTime,
};
use walkdir::WalkDir;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ScanMode {
    Similar,
    Screenshots,
    Time,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum TimeBasis {
    Captured,
    Created,
    Modified,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanOptions {
    root_path: String,
    mode: ScanMode,
    time_basis: TimeBasis,
    start_date: Option<String>,
    end_date: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageItem {
    id: String,
    path: String,
    name: String,
    bytes: u64,
    width: u32,
    height: u32,
    date: String,
    date_source: String,
    group_id: Option<usize>,
    group_size: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanResponse {
    items: Vec<ImageItem>,
    total_scanned: usize,
}

struct Candidate {
    item: ImageItem,
    full_hash: u64,
    crop_hash: u64,
}

#[tauri::command]
async fn scan_folder(options: ScanOptions) -> Result<ScanResponse, String> {
    tauri::async_runtime::spawn_blocking(move || scan_folder_sync(options))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn move_to_trash(path: String) -> Result<String, String> {
    let file = Path::new(&path);
    if !file.is_file() {
        return Err("文件不存在或不是普通文件".to_string());
    }

    let backup = create_undo_backup(file)?;
    if let Err(error) = trash::delete(file) {
        let _ = fs::remove_file(&backup);
        return Err(format!("无法移入回收站：{error}"));
    }
    Ok(backup.to_string_lossy().into_owned())
}

#[tauri::command]
fn restore_from_undo(backup_path: String, original_path: String) -> Result<(), String> {
    let backup = Path::new(&backup_path);
    let original = Path::new(&original_path);
    if !backup.starts_with(std::env::temp_dir().join("picture-cleaner-undo")) {
        return Err("撤销备份路径无效".to_string());
    }
    if !backup.is_file() {
        return Err("撤销备份不存在，无法回滚".to_string());
    }
    if original.exists() {
        return Err("原位置已有同名文件，无法回滚".to_string());
    }

    let parent = original
        .parent()
        .ok_or_else(|| "原文件路径无效".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建原文件夹：{error}"))?;
    fs::copy(backup, original).map_err(|error| format!("无法恢复文件：{error}"))?;
    let _ = fs::remove_file(backup);
    Ok(())
}

fn create_undo_backup(file: &Path) -> Result<PathBuf, String> {
    let undo_dir = std::env::temp_dir().join("picture-cleaner-undo");
    fs::create_dir_all(&undo_dir).map_err(|error| format!("无法创建撤销备份：{error}"))?;
    let filename = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image");
    let timestamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("无法生成撤销备份名称：{error}"))?
        .as_nanos();
    let backup = undo_dir.join(format!("{}-{}-{filename}", std::process::id(), timestamp));
    fs::copy(file, &backup).map_err(|error| format!("无法创建撤销备份：{error}"))?;
    Ok(backup)
}

fn scan_folder_sync(options: ScanOptions) -> Result<ScanResponse, String> {
    let root = PathBuf::from(&options.root_path);
    if !root.is_dir() {
        return Err("请选择一个有效的图片文件夹".to_string());
    }

    let image_paths = WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && is_supported_image(entry.path()))
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    let total_scanned = image_paths.len();
    let candidates = image_paths
        .par_iter()
        .filter_map(|path| {
            let Ok(candidate) = analyze_image(path, &options.time_basis) else {
                return None;
            };

            if !in_date_range(
                &candidate.item.date,
                options.start_date.as_deref(),
                options.end_date.as_deref(),
            ) {
                return None;
            }

            if matches!(options.mode, ScanMode::Screenshots)
                && !is_screenshot_candidate(path, candidate.item.width, candidate.item.height)
            {
                return None;
            }

            Some(candidate)
        })
        .collect::<Vec<_>>();

    let items = match options.mode {
        ScanMode::Similar => similar_items(candidates),
        ScanMode::Screenshots | ScanMode::Time => candidates
            .into_iter()
            .map(|candidate| candidate.item)
            .collect(),
    };

    Ok(ScanResponse {
        items,
        total_scanned,
    })
}

fn analyze_image(path: &Path, time_basis: &TimeBasis) -> Result<Candidate, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let image = ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?;
    let (width, height) = image.dimensions();
    let captured = read_capture_date(path);
    let created = format_system_time(metadata.created().ok());
    let modified = format_system_time(metadata.modified().ok());
    let (date, date_source) = choose_date(time_basis, captured, created, modified);
    let path_string = path.to_string_lossy().into_owned();

    Ok(Candidate {
        item: ImageItem {
            id: path_string.clone(),
            path: path_string,
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("未命名图片")
                .to_string(),
            bytes: metadata.len(),
            width,
            height,
            date,
            date_source,
            group_id: None,
            group_size: 1,
        },
        full_hash: perceptual_hash(&image, false),
        crop_hash: perceptual_hash(&image, true),
    })
}

fn similar_items(candidates: Vec<Candidate>) -> Vec<ImageItem> {
    let candidate_count = candidates.len();
    let candidate_refs: &[Candidate] = &candidates;
    let mut parent: Vec<usize> = (0..candidate_count).collect();

    // ponytail: O(n²) remains the exact matcher; bucket hashes only after real libraries show this ceiling.
    const LEFT_CHUNK_SIZE: usize = 64;
    for start in (0..candidate_count).step_by(LEFT_CHUNK_SIZE) {
        let end = (start + LEFT_CHUNK_SIZE).min(candidate_count);
        let matches = (start..end)
            .into_par_iter()
            .flat_map_iter(|left| {
                ((left + 1)..candidate_count).filter_map(move |right| {
                    are_similar(&candidate_refs[left], &candidate_refs[right])
                        .then_some((left, right))
                })
            })
            .collect::<Vec<_>>();

        for (left, right) in matches {
            union(&mut parent, left, right);
        }
    }

    let mut grouped: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..candidate_count {
        let root = find(&mut parent, index);
        grouped.entry(root).or_default().push(index);
    }

    let mut groups = grouped
        .into_values()
        .filter(|group| group.len() > 1)
        .collect::<Vec<_>>();
    groups.sort_by_key(|group| group[0]);

    let mut group_by_index = vec![None; candidate_count];
    for (group_id, group) in groups.into_iter().enumerate() {
        for &index in &group {
            group_by_index[index] = Some((group_id, group.len()));
        }
    }

    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut candidate)| {
            let (group_id, group_size) = group_by_index[index]?;
            candidate.item.group_id = Some(group_id);
            candidate.item.group_size = group_size;
            Some(candidate.item)
        })
        .collect()
}

fn are_similar(left: &Candidate, right: &Candidate) -> bool {
    hamming(left.full_hash, right.full_hash) <= 14 || hamming(left.crop_hash, right.crop_hash) <= 14
}

fn perceptual_hash(image: &DynamicImage, crop_center: bool) -> u64 {
    let source = if crop_center {
        let (width, height) = image.dimensions();
        let side = width.min(height);
        image.crop_imm((width - side) / 2, (height - side) / 2, side, side)
    } else {
        image.clone()
    };
    let grayscale = source
        .grayscale()
        .resize_exact(9, 8, FilterType::Triangle)
        .to_luma8();
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if grayscale.get_pixel(x, y)[0] > grayscale.get_pixel(x + 1, y)[0] {
                hash |= 1;
            }
        }
    }
    hash
}

fn hamming(left: u64, right: u64) -> u32 {
    (left ^ right).count_ones()
}

fn find(parent: &mut [usize], index: usize) -> usize {
    if parent[index] == index {
        return index;
    }
    let root = find(parent, parent[index]);
    parent[index] = root;
    root
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let left_root = find(parent, left);
    let right_root = find(parent, right);
    if left_root != right_root {
        parent[right_root] = left_root;
    }
}

fn is_supported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "avif")
    )
}

fn is_screenshot_candidate(path: &Path, width: u32, height: u32) -> bool {
    let name = path.to_string_lossy().to_ascii_lowercase();
    let name_match = [
        "screenshot",
        "screen_shot",
        "screen shot",
        "截图",
        "截屏",
        "屏幕快照",
    ]
    .iter()
    .any(|marker| name.contains(marker));
    let short_side = width.min(height);
    let long_side = width.max(height);
    let mobile_shape =
        short_side >= 500 && (1.6..=2.5).contains(&(long_side as f32 / short_side as f32));
    name_match || mobile_shape
}

fn read_capture_date(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = Reader::new().read_from_container(&mut reader).ok()?;

    for tag in [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime] {
        if let Some(field) = exif.get_field(tag, In::PRIMARY) {
            if let Some(date) =
                normalize_exif_date(&field.display_value().with_unit(&exif).to_string())
            {
                return Some(date);
            }
        }
    }
    None
}

fn normalize_exif_date(value: &str) -> Option<String> {
    let date = value.trim().trim_matches('"').split_whitespace().next()?;
    let mut parts = date.split(':');
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    Some(format!("{year}-{month}-{day}"))
}

fn format_system_time(time: Option<SystemTime>) -> Option<String> {
    time.map(|value| {
        DateTime::<Local>::from(value)
            .format("%Y-%m-%d")
            .to_string()
    })
}

fn choose_date(
    basis: &TimeBasis,
    captured: Option<String>,
    created: Option<String>,
    modified: Option<String>,
) -> (String, String) {
    match basis {
        TimeBasis::Captured => captured
            .map(|date| (date, "captured".to_string()))
            .or_else(|| modified.map(|date| (date, "modified".to_string())))
            .or_else(|| created.map(|date| (date, "created".to_string())))
            .unwrap_or_else(|| ("未知日期".to_string(), "unknown".to_string())),
        TimeBasis::Created => created
            .map(|date| (date, "created".to_string()))
            .or_else(|| modified.map(|date| (date, "modified".to_string())))
            .or_else(|| captured.map(|date| (date, "captured".to_string())))
            .unwrap_or_else(|| ("未知日期".to_string(), "unknown".to_string())),
        TimeBasis::Modified => modified
            .map(|date| (date, "modified".to_string()))
            .or_else(|| created.map(|date| (date, "created".to_string())))
            .or_else(|| captured.map(|date| (date, "captured".to_string())))
            .unwrap_or_else(|| ("未知日期".to_string(), "unknown".to_string())),
    }
}

fn in_date_range(date: &str, start: Option<&str>, end: Option<&str>) -> bool {
    if date == "未知日期" {
        return start.is_none() && end.is_none();
    }
    start.is_none_or(|value| date >= value) && end.is_none_or(|value| date <= value)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            scan_folder,
            move_to_trash,
            restore_from_undo
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{
        create_undo_backup, in_date_range, is_screenshot_candidate, normalize_exif_date,
        restore_from_undo, similar_items, Candidate, ImageItem,
    };
    use std::{fs, path::Path, time::SystemTime};

    #[test]
    fn date_filter_and_screenshot_detection_work() {
        assert!(in_date_range(
            "2026-09-07",
            Some("2026-09-01"),
            Some("2026-09-30")
        ));
        assert!(!in_date_range("2026-08-31", Some("2026-09-01"), None));
        assert!(is_screenshot_candidate(
            Path::new("IMG_screenshot.png"),
            1170,
            2532
        ));
        assert_eq!(
            normalize_exif_date("2026:09:07 12:30:00"),
            Some("2026-09-07".to_string())
        );
    }

    #[test]
    fn undo_backup_restores_file() {
        let root = std::env::temp_dir().join(format!(
            "picture-cleaner-test-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let original = root.join("photo.jpg");
        fs::write(&original, b"test image").unwrap();
        let backup = create_undo_backup(&original).unwrap();
        fs::remove_file(&original).unwrap();

        restore_from_undo(
            backup.to_string_lossy().into_owned(),
            original.to_string_lossy().into_owned(),
        )
        .unwrap();

        assert_eq!(fs::read(&original).unwrap(), b"test image");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn similar_items_keeps_connected_groups() {
        let items = similar_items(vec![
            candidate("a", 0, 0),
            candidate("b", 1, 0),
            candidate("c", u64::MAX, u64::MAX),
            candidate("d", u64::MAX - 1, u64::MAX),
        ]);

        assert_eq!(items.len(), 4);
        assert_eq!(
            items
                .iter()
                .map(|item| (item.group_id, item.group_size))
                .collect::<Vec<_>>(),
            vec![(Some(0), 2), (Some(0), 2), (Some(1), 2), (Some(1), 2)]
        );
    }

    fn candidate(id: &str, full_hash: u64, crop_hash: u64) -> Candidate {
        Candidate {
            item: ImageItem {
                id: id.to_string(),
                path: id.to_string(),
                name: id.to_string(),
                bytes: 0,
                width: 1,
                height: 1,
                date: "2026-09-07".to_string(),
                date_source: "test".to_string(),
                group_id: None,
                group_size: 1,
            },
            full_hash,
            crop_hash,
        }
    }
}
