use crate::{recognition, wxapkg::Archive};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheLayout {
    Radium,
    Legacy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheRootInfo {
    pub root: PathBuf,
    pub packages_root: PathBuf,
    pub icon_root: PathBuf,
    pub layout: CacheLayout,
    pub app_count: usize,
    pub modified_at: u64,
}

#[derive(Clone, Debug)]
pub struct Applet {
    pub appid: String,
    pub name: String,
    pub name_source: String,
    pub version: String,
    pub package_count: usize,
    pub package_bytes: u64,
    pub package_size: String,
    pub icon_path: String,
    pub package_dir: String,
    pub main_package: String,
    pub used_by: String,
    pub depends_on: String,
    pub created_at: u64,
    pub modified_at: u64,
    pub accessed_at: u64,
    pub name_confidence: u8,
    pub name_candidates: Vec<String>,
}

pub fn is_appid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 18 && bytes.starts_with(b"wx") && bytes[2..].iter().all(u8::is_ascii_hexdigit)
}

fn format_size(mut bytes: f64) -> String {
    for unit in ["B", "KiB", "MiB", "GiB"] {
        if bytes < 1024.0 || unit == "GiB" {
            return if unit == "B" {
                format!("{} B", bytes as u64)
            } else {
                format!("{bytes:.1} {unit}")
            };
        }
        bytes /= 1024.0;
    }
    unreachable!()
}

fn seconds(time: Option<SystemTime>) -> u64 {
    time.and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn directory_activity(path: &Path, depth: usize) -> u64 {
    let mut latest = fs::metadata(path)
        .ok()
        .map(|metadata| seconds(metadata.modified().ok()))
        .unwrap_or(0);
    if depth == 0 {
        return latest;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return latest;
    };
    for entry in entries.filter_map(Result::ok) {
        let child = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&child) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        latest = latest.max(seconds(metadata.modified().ok()));
        if metadata.is_dir() {
            latest = latest.max(directory_activity(&child, depth - 1));
        }
    }
    latest
}

pub fn inspect_cache_root(root: &Path) -> Option<CacheRootInfo> {
    let root = root.canonicalize().ok()?;
    let modern_packages = root.join("packages");
    let (layout, packages_root) = if modern_packages.is_dir() {
        (CacheLayout::Radium, modern_packages)
    } else {
        let has_app_directories = fs::read_dir(&root)
            .ok()?
            .filter_map(Result::ok)
            .any(|entry| entry.path().is_dir() && entry.file_name().to_str().is_some_and(is_appid));
        if !has_app_directories {
            return None;
        }
        (CacheLayout::Legacy, root.clone())
    };
    let app_directories = fs::read_dir(&packages_root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir() && entry.file_name().to_str().is_some_and(is_appid))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    let modified_at = app_directories
        .iter()
        .map(|path| directory_activity(path, 3))
        .max()
        .unwrap_or_else(|| directory_activity(&packages_root, 1));
    Some(CacheRootInfo {
        icon_root: root.join("icon"),
        root,
        packages_root,
        layout,
        app_count: app_directories.len(),
        modified_at,
    })
}

fn collect_packages(directory: &Path, packages: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_packages(&path, packages);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("wxapkg") {
            packages.push(path);
        }
    }
}

fn main_package(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("__APP__.wxapkg"))
        .max_by_key(|path| {
            path.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse::<u64>().ok())
                .unwrap_or(0)
        })
        .cloned()
}

fn provider_ids(path: &Path) -> Vec<String> {
    let Ok(archive) = Archive::open(path) else {
        return Vec::new();
    };
    let mut providers = HashSet::new();
    for (name, bytes) in archive.files() {
        if !matches!(
            name.rsplit('/').next(),
            Some("app-config.json") | Some("app-service.js")
        ) {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        let mut start = 0;
        while let Some(found) = text[start..].find("provider") {
            let after_marker = start + found + "provider".len();
            let Some(offset) = text[after_marker..].find("wx") else {
                break;
            };
            let candidate_start = after_marker + offset;
            let candidate_end = candidate_start.saturating_add(18);
            if let Some(candidate) = text.get(candidate_start..candidate_end) {
                if is_appid(candidate) {
                    providers.insert(candidate.to_owned());
                }
            }
            start = candidate_start.saturating_add(2);
            if start >= text.len() {
                break;
            }
        }
    }
    let mut result: Vec<_> = providers.into_iter().collect();
    result.sort();
    result
}

fn icon_paths(root: &Path) -> HashMap<String, String> {
    let Ok(entries) = fs::read_dir(root.join("icon")) else {
        return HashMap::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let appid = name.get(..18)?;
            if !is_appid(appid) || !name.as_bytes().get(18).is_some_and(|byte| *byte == b'_') {
                return None;
            }
            path.is_file()
                .then(|| (appid.to_owned(), path.to_string_lossy().into_owned()))
        })
        .collect()
}

#[cfg(test)]
pub fn scan(root: &Path) -> Result<Vec<Applet>, String> {
    scan_with_progress(root, |_, _| Ok(()))
}

pub fn scan_with_progress(
    root: &Path,
    mut progress: impl FnMut(usize, usize) -> Result<(), String>,
) -> Result<Vec<Applet>, String> {
    let info = inspect_cache_root(root)
        .ok_or_else(|| format!("不是有效的微信小程序缓存目录：{}", root.display()))?;
    let packages_root = info.packages_root;
    let icons = icon_paths(&info.root);
    let mut directories: Vec<_> = fs::read_dir(&packages_root)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_appid)
        })
        .collect();
    directories.sort();
    let total = directories.len();
    progress(0, total)?;
    if total == 0 {
        return Ok(Vec::new());
    }

    let directories = Arc::new(directories);
    let icons = Arc::new(icons);
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let workers = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(8)
        .min(total);
    let (tx, rx) = mpsc::channel();
    let mut scanned = Vec::with_capacity(total);
    let mut progress_error = None;

    thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let directories = Arc::clone(&directories);
            let icons = Arc::clone(&icons);
            let next = &next;
            let stop = &stop;
            scope.spawn(move || loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= directories.len() {
                    break;
                }
                let result = scan_directory(index, &directories[index], &icons);
                if tx.send(result).is_err() {
                    break;
                }
            });
        }
        drop(tx);

        for result in rx {
            scanned.push(result);
            if progress_error.is_none() {
                if let Err(error) = progress(scanned.len(), total) {
                    stop.store(true, Ordering::Relaxed);
                    progress_error = Some(error);
                }
            }
        }
    });
    if let Some(error) = progress_error {
        return Err(error);
    }

    scanned.sort_by_key(|result| result.index);
    let mut rows = Vec::new();
    let mut parents: HashMap<String, Vec<String>> = HashMap::new();
    let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
    for result in scanned {
        let Some(row) = result.row else {
            continue;
        };
        for provider in &result.providers {
            parents
                .entry(provider.clone())
                .or_default()
                .push(row.appid.clone());
        }
        dependencies.insert(row.appid.clone(), result.providers);
        rows.push(row);
    }
    let names: HashMap<_, _> = rows
        .iter()
        .map(|row| (row.appid.clone(), row.name.clone()))
        .collect();
    for row in &mut rows {
        row.used_by = parents
            .get(&row.appid)
            .into_iter()
            .flatten()
            .filter_map(|id| names.get(id))
            .cloned()
            .collect::<Vec<_>>()
            .join("、");
        row.depends_on = dependencies
            .get(&row.appid)
            .cloned()
            .unwrap_or_default()
            .join("、");
    }
    Ok(rows)
}

struct ScannedDirectory {
    index: usize,
    row: Option<Applet>,
    providers: Vec<String>,
}

fn scan_directory(
    index: usize,
    directory: &Path,
    icons: &HashMap<String, String>,
) -> ScannedDirectory {
    let Some(id) = directory
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|id| is_appid(id))
        .map(str::to_owned)
    else {
        return ScannedDirectory {
            index,
            row: None,
            providers: Vec::new(),
        };
    };
    let mut packages = Vec::new();
    collect_packages(directory, &mut packages);
    let Some(main) = main_package(&packages) else {
        return ScannedDirectory {
            index,
            row: None,
            providers: Vec::new(),
        };
    };
    let providers = provider_ids(&main);
    let recognition = recognition::recognize_main_package(&main);
    let metadata = fs::metadata(&main).ok();
    let package_bytes = packages
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum();
    let row = Applet {
        appid: id.clone(),
        name: recognition
            .as_ref()
            .map(|value| value.name.clone())
            .unwrap_or_else(|| "未识别".into()),
        name_source: recognition
            .as_ref()
            .map(|value| value.source.clone())
            .unwrap_or_else(|| "主包未提供可信名称".into()),
        version: packages
            .iter()
            .filter_map(|path| path.parent()?.file_name()?.to_str()?.parse::<u64>().ok())
            .max()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".into()),
        package_count: packages.len(),
        package_bytes,
        package_size: format_size(package_bytes as f64),
        icon_path: icons.get(&id).cloned().unwrap_or_default(),
        package_dir: directory.to_string_lossy().into_owned(),
        main_package: main.to_string_lossy().into_owned(),
        used_by: String::new(),
        depends_on: String::new(),
        created_at: seconds(metadata.as_ref().and_then(|value| value.created().ok())),
        modified_at: seconds(metadata.as_ref().and_then(|value| value.modified().ok())),
        accessed_at: seconds(metadata.as_ref().and_then(|value| value.accessed().ok())),
        name_confidence: recognition
            .as_ref()
            .map(|value| value.confidence)
            .unwrap_or(0),
        name_candidates: recognition
            .map(|value| value.candidates)
            .unwrap_or_default(),
    };
    ScannedDirectory {
        index,
        row: Some(row),
        providers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::known_samples::{SampleClass, KNOWN_SAMPLES};
    #[test]
    fn appid_requires_ascii_hex() {
        assert!(is_appid("wx26a31270d9ab25e0"));
        assert!(!is_appid("wx26a31270d9ab25预"));
        assert!(!is_appid("wx26a31270d9ab25e"));
    }

    #[test]
    fn detects_modern_and_legacy_cache_layouts() {
        let base = std::env::temp_dir().join(format!(
            "wxapplet-layout-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let modern = base.join("modern");
        let legacy = base.join("legacy");
        fs::create_dir_all(modern.join("packages/wx0123456789abcdef")).unwrap();
        fs::create_dir_all(legacy.join("wxfedcba9876543210")).unwrap();

        let modern_info = inspect_cache_root(&modern).unwrap();
        assert_eq!(modern_info.layout, CacheLayout::Radium);
        assert_eq!(modern_info.app_count, 1);
        assert_eq!(
            modern_info.packages_root,
            modern.canonicalize().unwrap().join("packages")
        );

        let legacy_info = inspect_cache_root(&legacy).unwrap();
        assert_eq!(legacy_info.layout, CacheLayout::Legacy);
        assert_eq!(legacy_info.app_count, 1);
        assert_eq!(legacy_info.packages_root, legacy.canonicalize().unwrap());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn parallel_scan_reports_each_directory_and_propagates_cancellation() {
        let root = std::env::temp_dir().join(format!(
            "wxapplet-scan-progress-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let packages = root.join("packages");
        fs::create_dir_all(&packages).unwrap();
        for index in 0..16 {
            fs::create_dir(packages.join(format!("wx{index:016x}"))).unwrap();
        }

        let mut updates = Vec::new();
        let rows = scan_with_progress(&root, |completed, total| {
            updates.push((completed, total));
            Ok(())
        })
        .unwrap();
        assert!(rows.is_empty());
        assert_eq!(updates.first(), Some(&(0, 16)));
        assert_eq!(updates.last(), Some(&(16, 16)));
        assert_eq!(updates.len(), 17);

        let error = scan_with_progress(&root, |completed, _| {
            if completed >= 3 {
                Err("操作已取消".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error, "操作已取消");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn all_known_names_regress_together_when_configured() {
        let Ok(root) = std::env::var("WXAPPLET_ROOT") else {
            return;
        };
        let rows = scan(Path::new(&root)).unwrap();
        let mut display_exact = 0usize;
        let mut inferable_total = 0usize;
        let mut inferable_exact = 0usize;
        let mut package_exact = 0usize;
        let mut package_total = 0usize;
        let mut recognized = 0usize;
        let mut regressions = Vec::new();
        let mut package_regressions = Vec::new();
        let mut false_positives = Vec::new();
        for sample in KNOWN_SAMPLES {
            let row = rows
                .iter()
                .find(|row| row.appid == sample.appid)
                .unwrap_or_else(|| panic!("测试缓存缺少 {}", sample.appid));
            let recognition =
                crate::recognition::recognize_main_package(Path::new(&row.main_package));
            let diagnostics = recognition
                .as_ref()
                .map(|value| value.diagnostics.join("; "))
                .unwrap_or_else(|| {
                    let ranked =
                        crate::recognition::recognition_diagnostics(Path::new(&row.main_package));
                    if ranked.is_empty() {
                        "no candidate".into()
                    } else {
                        format!("undecided: {}", ranked.join("; "))
                    }
                });
            println!(
                "{} class={:?} display={} package={:?} actual={} source={} evidence=[{}]",
                sample.appid,
                sample.class,
                sample.display_name,
                sample.package_name,
                row.name,
                row.name_source,
                diagnostics
            );
            display_exact += usize::from(row.name == sample.display_name);
            recognized += usize::from(row.name != "未识别");
            if let Some(package_name) = sample.package_name {
                package_total += 1;
                if row.name == package_name {
                    package_exact += 1;
                } else {
                    package_regressions.push(format!(
                        "{}: package_expected={}, actual={}",
                        sample.appid, package_name, row.name
                    ));
                }
            } else if row.name != "未识别" {
                false_positives.push(format!("{}: actual={}", sample.appid, row.name));
            }
            if sample.class == SampleClass::Inferable {
                inferable_total += 1;
                if row.name == sample.display_name {
                    inferable_exact += 1;
                } else {
                    regressions.push(format!(
                        "{}: expected={}, actual={}, evidence=[{}]",
                        sample.appid, sample.display_name, row.name, diagnostics
                    ));
                }
            }
        }
        println!(
            "识别统计：可推导={inferable_exact}/{inferable_total} ({:.1}%), 覆盖={recognized}/{} ({:.1}%), 全量展示名={display_exact}/{} ({:.1}%), 主包名称={package_exact}/{package_total} ({:.1}%)",
            inferable_exact as f64 / inferable_total as f64 * 100.0,
            KNOWN_SAMPLES.len(),
            recognized as f64 / KNOWN_SAMPLES.len() as f64 * 100.0,
            KNOWN_SAMPLES.len(),
            display_exact as f64 / KNOWN_SAMPLES.len() as f64 * 100.0,
            package_exact as f64 / package_total as f64 * 100.0,
        );
        assert!(
            inferable_exact * 100 >= inferable_total * 90,
            "可推导样本正确率低于 90%：{inferable_exact}/{inferable_total}\n{}",
            regressions.join("\n")
        );
        assert!(
            regressions.is_empty(),
            "既有正确样本发生回归\n{}",
            regressions.join("\n")
        );
        assert!(
            package_regressions.is_empty(),
            "主包原生名称发生回归\n{}",
            package_regressions.join("\n")
        );
        assert!(
            false_positives.is_empty(),
            "无名称证据的样本被误识别\n{}",
            false_positives.join("\n")
        );
    }

    #[test]
    fn runtime_recognizer_contains_no_appid_mapping() {
        let source = include_str!("recognition.rs");
        assert!(
            !source
                .as_bytes()
                .windows(18)
                .filter_map(|window| std::str::from_utf8(window).ok())
                .any(is_appid),
            "recognition.rs 不得包含 AppID 匹配"
        );
    }
}
