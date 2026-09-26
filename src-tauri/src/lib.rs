use chrono::{DateTime, Local, NaiveDate};
use exif::{In, Reader, Tag};
use image::{
    imageops::FilterType, metadata::Orientation, DynamicImage, GenericImageView, ImageDecoder,
    ImageReader,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, FileTimes},
    io::BufReader,
    path::{Path, PathBuf},
    time::SystemTime,
};
use tauri::Manager;
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
    skipped_count: usize,
}

struct Candidate {
    item: ImageItem,
    full_hash: Option<u64>,
    crop_hash: Option<u64>,
}

#[tauri::command]
async fn scan_folder(app: tauri::AppHandle, options: ScanOptions) -> Result<ScanResponse, String> {
    let root = Path::new(&options.root_path);
    if !root.is_dir() {
        return Err("请选择一个有效的图片文件夹".to_string());
    }
    app.asset_protocol_scope()
        .allow_directory(root, true)
        .map_err(|error| format!("无法读取图片文件夹：{error}"))?;
    tauri::async_runtime::spawn_blocking(move || scan_folder_sync(options))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn move_to_trash(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let file = Path::new(&path);
    if !file.is_file() {
        return Err("文件不存在或不是普通文件".to_string());
    }
    if !app.asset_protocol_scope().is_allowed(file) {
        return Err("文件不在已选择的图片文件夹中".to_string());
    }

    let backup = create_undo_backup(file)?;
    if let Err(error) = trash::delete(file) {
        let _ = fs::remove_file(&backup);
        return Err(format!("无法移入回收站：{error}"));
    }
    Ok(backup.to_string_lossy().into_owned())
}

#[tauri::command]
fn restore_from_undo(
    app: tauri::AppHandle,
    backup_path: String,
    original_path: String,
) -> Result<(), String> {
    restore_from_undo_in_scope(backup_path, original_path, |parent| {
        app.asset_protocol_scope().is_allowed(parent)
    })
}

fn restore_from_undo_in_scope(
    backup_path: String,
    original_path: String,
    is_allowed: impl Fn(&Path) -> bool,
) -> Result<(), String> {
    let backup = fs::canonicalize(&backup_path).map_err(|_| "撤销备份不存在".to_string())?;
    let original = Path::new(&original_path);
    let undo_dir =
        fs::canonicalize(undo_session_dir()).map_err(|_| "撤销备份路径无效".to_string())?;
    if !backup.starts_with(&undo_dir) || !backup.is_file() {
        return Err("撤销备份路径无效".to_string());
    }
    let parent = original
        .parent()
        .ok_or_else(|| "原文件路径无效".to_string())?;
    if !parent.is_dir() || !is_allowed(parent) {
        return Err("原文件不在已选择的图片文件夹中".to_string());
    }
    match fs::symlink_metadata(original) {
        Ok(_) => return Err("原位置已有同名文件，无法回滚".to_string()),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("无法检查原文件位置：{error}"));
        }
        Err(_) => {}
    }

    copy_with_file_times(&backup, original).map_err(|error| format!("无法恢复文件：{error}"))?;
    let _ = fs::remove_file(backup);
    Ok(())
}

fn undo_session_dir() -> PathBuf {
    std::env::temp_dir()
        .join("picture-cleaner-undo")
        .join(std::process::id().to_string())
}

fn create_undo_backup(file: &Path) -> Result<PathBuf, String> {
    let undo_dir = undo_session_dir();
    fs::create_dir_all(&undo_dir).map_err(|error| format!("无法创建撤销备份：{error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("无法生成撤销备份名称：{error}"))?
        .as_nanos();
    let backup = undo_dir.join(format!("{}-{timestamp}", std::process::id()));
    copy_with_file_times(file, &backup).map_err(|error| format!("无法创建撤销备份：{error}"))?;
    Ok(backup)
}

fn copy_with_file_times(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::metadata(source).map_err(|error| error.to_string())?;
    if let Err(error) = fs::copy(source, destination) {
        let _ = fs::remove_file(destination);
        return Err(error.to_string());
    }

    let mut times = FileTimes::new();
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::darwin::fs::FileTimesExt;
        if let Ok(created) = metadata.created() {
            times = times.set_created(created);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTimesExt;
        if let Ok(created) = metadata.created() {
            times = times.set_created(created);
        }
    }

    if let Err(error) = File::open(destination).and_then(|file| file.set_times(times)) {
        let _ = fs::remove_file(destination);
        return Err(error.to_string());
    }
    Ok(())
}

fn scan_folder_sync(options: ScanOptions) -> Result<ScanResponse, String> {
    let root = PathBuf::from(&options.root_path);
    if !root.is_dir() {
        return Err("请选择一个有效的图片文件夹".to_string());
    }
    validate_date_range(options.start_date.as_deref(), options.end_date.as_deref())?;
    let needs_hashes = matches!(&options.mode, ScanMode::Similar);

    let mut image_paths = Vec::new();
    for entry in WalkDir::new(&root).follow_links(false) {
        let entry = entry.map_err(|error| format!("无法遍历图片文件夹：{error}"))?;
        if entry.file_type().is_file() && is_supported_image(entry.path()) {
            image_paths.push(entry.into_path());
        }
    }
    image_paths.sort_unstable();
    let total_scanned = image_paths.len();
    let screenshots_only = matches!(&options.mode, ScanMode::Screenshots);
    let analyzed = image_paths
        .par_iter()
        .map(|path| analyze_image(path, &options.time_basis, needs_hashes))
        .collect::<Vec<_>>();
    let skipped_count = analyzed
        .iter()
        .filter(|candidate| candidate.is_err())
        .count();
    let mut candidates = analyzed
        .into_iter()
        .filter_map(Result::ok)
        .filter(|candidate| {
            let item = &candidate.item;
            in_date_range(
                &item.date,
                options.start_date.as_deref(),
                options.end_date.as_deref(),
            ) && (!screenshots_only
                || is_screenshot_candidate(Path::new(&item.path), item.width, item.height))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by(|left, right| left.item.path.cmp(&right.item.path));

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
        skipped_count,
    })
}

fn analyze_image(
    path: &Path,
    time_basis: &TimeBasis,
    needs_hashes: bool,
) -> Result<Candidate, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let mut decoder = ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .into_decoder()
        .map_err(|error| error.to_string())?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);
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
        full_hash: needs_hashes.then(|| perceptual_hash(&image, false)),
        crop_hash: needs_hashes.then(|| perceptual_hash(&image, true)),
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
    left.full_hash
        .zip(right.full_hash)
        .is_some_and(|(left, right)| hamming(left, right) <= 14)
        || left
            .crop_hash
            .zip(right.crop_hash)
            .is_some_and(|(left, right)| hamming(left, right) <= 14)
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
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
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
    let mobile_shape =
        height > width && width >= 500 && (1.6..=2.5).contains(&(height as f32 / width as f32));
    let png_mobile_shape = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        && mobile_shape;

    // ponytail: filename/PNG is a small heuristic; inspect image metadata only if converted screenshots cause false positives.
    name_match || png_mobile_shape
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
    let date = NaiveDate::parse_from_str(date, "%Y:%m:%d").ok()?;
    Some(date.format("%Y-%m-%d").to_string())
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

fn validate_date_range(start: Option<&str>, end: Option<&str>) -> Result<(), String> {
    let parse = |value: &str, label: &str| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| format!("{label}日期无效"))
    };
    let start_date = start.map(|value| parse(value, "开始")).transpose()?;
    let end_date = end.map(|value| parse(value, "结束")).transpose()?;
    if start_date
        .zip(end_date)
        .is_some_and(|(start, end)| start > end)
    {
        return Err("开始日期不能晚于结束日期".to_string());
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            scan_folder,
            move_to_trash,
            restore_from_undo
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|_, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            if let Err(error) = fs::remove_dir_all(undo_session_dir()) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("无法清理本次会话的撤销备份：{error}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        analyze_image, create_undo_backup, in_date_range, is_screenshot_candidate,
        normalize_exif_date, restore_from_undo_in_scope, scan_folder_sync, similar_items,
        validate_date_range, Candidate, ImageItem, ScanMode, ScanOptions, TimeBasis,
    };
    use image::{codecs::jpeg::JpegEncoder, ExtendedColorType, ImageEncoder};
    use std::{
        fs::{self, File},
        path::Path,
        time::SystemTime,
    };

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
        assert!(!is_screenshot_candidate(
            Path::new("IMG_1234.jpg"),
            1170,
            2532
        ));
        assert!(!is_screenshot_candidate(
            Path::new("IMG_1234.png"),
            2532,
            1170
        ));
        assert!(is_screenshot_candidate(
            Path::new("IMG_screenshot.jpg"),
            2532,
            1170
        ));
        assert!(!is_screenshot_candidate(
            Path::new("Screenshots/holiday.png"),
            1000,
            1000
        ));
        assert_eq!(
            normalize_exif_date("2026:09:07 12:30:00"),
            Some("2026-09-07".to_string())
        );
        assert_eq!(normalize_exif_date("2026:02:30 12:30:00"), None);
        assert!(validate_date_range(Some("2026-09-08"), Some("2026-09-07")).is_err());
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
        let original = root.join(format!("{}.jpg", "a".repeat(240)));
        fs::write(&original, b"test image").unwrap();
        let original_modified =
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_650_000_000);
        File::open(&original)
            .unwrap()
            .set_modified(original_modified)
            .unwrap();
        let backup = create_undo_backup(&original).unwrap();
        fs::remove_file(&original).unwrap();

        let denied = restore_from_undo_in_scope(
            backup.to_string_lossy().into_owned(),
            original.to_string_lossy().into_owned(),
            |_| false,
        );
        assert!(denied.is_err());
        assert!(!original.exists());

        restore_from_undo_in_scope(
            backup.to_string_lossy().into_owned(),
            original.to_string_lossy().into_owned(),
            |_| true,
        )
        .unwrap();

        assert_eq!(fs::read(&original).unwrap(), b"test image");
        assert_eq!(
            fs::metadata(&original).unwrap().modified().unwrap(),
            original_modified
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn undo_rejects_backup_outside_session_directory() {
        let root = std::env::temp_dir().join(format!(
            "picture-cleaner-outside-undo-test-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let backup = root.join("untrusted-backup");
        let original = root.join("restored.jpg");
        fs::write(&backup, b"not an undo backup").unwrap();

        let result = restore_from_undo_in_scope(
            backup.to_string_lossy().into_owned(),
            original.to_string_lossy().into_owned(),
            |_| true,
        );

        assert!(result.is_err());
        assert!(!original.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_analysis_applies_exif_orientation() {
        let root = std::env::temp_dir().join(format!(
            "picture-cleaner-orientation-test-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("portrait.jpg");
        let exif = [
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0,
            0,
        ];
        let mut encoder = JpegEncoder::new(File::create(&path).unwrap());
        encoder.set_exif_metadata(exif.to_vec()).unwrap();
        encoder
            .write_image(&[0; 18], 2, 3, ExtendedColorType::Rgb8)
            .unwrap();

        let image = analyze_image(&path, &TimeBasis::Modified, false).unwrap();
        assert_eq!((image.item.width, image.item.height), (3, 2));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_reports_unreadable_images() {
        let root = std::env::temp_dir().join(format!(
            "picture-cleaner-unreadable-test-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("broken.jpg"), b"not an image").unwrap();

        let response = scan_folder_sync(ScanOptions {
            root_path: root.to_string_lossy().into_owned(),
            mode: ScanMode::Time,
            time_basis: TimeBasis::Modified,
            start_date: None,
            end_date: None,
        })
        .unwrap();
        assert_eq!(response.total_scanned, 1);
        assert_eq!(response.skipped_count, 1);
        assert!(response.items.is_empty());

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
            full_hash: Some(full_hash),
            crop_hash: Some(crop_hash),
        }
    }
}
