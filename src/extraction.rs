use crate::{
    cache::{is_appid, Applet, CachedPackage, PackageRole},
    wxapkg::Archive,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionMode {
    Complete,
    MainOnly,
}

impl ExtractionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::MainOnly => "main_only",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionTargetKind {
    Applet,
    Plugin,
}

impl ExtractionTargetKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Applet => "applet",
            Self::Plugin => "plugin",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExtractionTarget {
    pub appid: String,
    pub name: String,
    pub version: String,
    pub kind: ExtractionTargetKind,
    pub packages: Vec<CachedPackage>,
}

impl ExtractionTarget {
    pub fn applet(applet: &Applet) -> Self {
        Self {
            appid: applet.appid.clone(),
            name: applet.name.clone(),
            version: applet.version.clone(),
            kind: ExtractionTargetKind::Applet,
            packages: applet.active_packages.clone(),
        }
    }

    pub fn plugin(appid: String, package: CachedPackage) -> Self {
        let version = Path::new(&package.path)
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u64>().ok())
            .map(|version| version.to_string())
            .unwrap_or_else(|| "-".into());
        Self {
            appid,
            name: String::new(),
            version,
            kind: ExtractionTargetKind::Plugin,
            packages: vec![package],
        }
    }

    pub fn package_count(&self, mode: ExtractionMode) -> usize {
        selected_packages(self, mode).count()
    }
}

fn safe_directory_name(name: &str, appid: &str) -> String {
    let cleaned = name
        .chars()
        .take(60)
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']);
    let cleaned = if cleaned.is_empty() {
        "未识别"
    } else {
        cleaned
    };
    format!("{cleaned}_{appid}")
}

fn safe_components(name: &str) -> Result<Vec<&std::ffi::OsStr>, String> {
    if name.contains('\\') {
        return Err(format!("包内路径不安全：{name}"));
    }
    let path = Path::new(name);
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => components.push(part),
            _ => return Err(format!("包内路径不安全：{name}")),
        }
    }
    (!components.is_empty())
        .then_some(components)
        .ok_or_else(|| "包内文件名为空".into())
}

fn ensure_directory(base: &Path, parts: &[&str]) -> Result<PathBuf, String> {
    let mut current = base.to_path_buf();
    for part in parts {
        current.push(part);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() {
                return Err(format!("输出路径包含符号链接：{}", current.display()));
            }
            if !metadata.is_dir() {
                return Err(format!("输出路径不是目录：{}", current.display()));
            }
        } else {
            fs::create_dir(&current).map_err(|error| error.to_string())?;
        }
    }
    Ok(current)
}

fn target_path(base: &Path, name: &str) -> Result<PathBuf, String> {
    let components = safe_components(name.trim_start_matches('/'))?;
    let mut target = base.to_path_buf();
    for component in &components {
        target.push(component);
    }
    let parent = target.parent().ok_or("无效输出路径")?;
    let relative = parent.strip_prefix(base).map_err(|_| "无效输出路径")?;
    let mut current = base.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err("无效输出路径".into());
        };
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() {
                return Err(format!("输出路径包含符号链接：{}", current.display()));
            }
        } else {
            fs::create_dir(&current).map_err(|error| error.to_string())?;
        }
    }
    Ok(target)
}

fn write_merged_file(base: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let target = target_path(base, name)?;
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() {
            return Err(format!("拒绝覆盖符号链接：{}", target.display()));
        }
        if metadata.is_file() {
            let existing = fs::read(&target).map_err(|error| error.to_string())?;
            if existing == bytes {
                return Ok(());
            }
            return Err(format!("文件内容冲突：{}", name.trim_start_matches('/')));
        }
        return Err(format!("输出目标不是文件：{}", target.display()));
    }
    fs::write(target, bytes).map_err(|error| error.to_string())
}

pub fn extract_one(package: &Path, output: &Path) -> Result<usize, String> {
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let output = output.canonicalize().map_err(|error| error.to_string())?;
    let archive = Archive::open(package)?;
    let mut count = 0;
    for (name, bytes) in archive.files() {
        write_merged_file(&output, name, bytes)?;
        count += 1;
    }
    Ok(count)
}

#[derive(Clone, Debug)]
pub struct ExtractionFailure {
    pub appid: String,
    pub error: String,
}

#[derive(Clone, Debug)]
pub struct ExtractionSummary {
    pub applet_count: usize,
    pub plugin_count: usize,
    pub package_count: usize,
    pub file_count: usize,
    pub output: String,
    pub failures: Vec<ExtractionFailure>,
}

fn selected_packages(
    target: &ExtractionTarget,
    mode: ExtractionMode,
) -> impl Iterator<Item = &CachedPackage> {
    target
        .packages
        .iter()
        .filter(move |package| match target.kind {
            ExtractionTargetKind::Plugin => package.role == PackageRole::Plugin,
            ExtractionTargetKind::Applet => match mode {
                ExtractionMode::Complete => package.role != PackageRole::Plugin,
                ExtractionMode::MainOnly => package.role == PackageRole::Main,
            },
        })
}

fn extraction_directories(
    output: &Path,
    target: &ExtractionTarget,
    mode: ExtractionMode,
) -> Result<(PathBuf, PathBuf, &'static str), String> {
    if !is_appid(&target.appid) {
        return Err("无效 AppID".into());
    }
    match target.kind {
        ExtractionTargetKind::Applet => {
            let directory_name = safe_directory_name(&target.name, &target.appid);
            let root = ensure_directory(output, &[&directory_name])?;
            let (content_name, manifest_name) = match mode {
                ExtractionMode::Complete => ("app", "package-manifest.json"),
                ExtractionMode::MainOnly => ("main", "main-package-manifest.json"),
            };
            let content = ensure_directory(&root, &[content_name])?;
            Ok((root, content, manifest_name))
        }
        ExtractionTargetKind::Plugin => {
            let root = ensure_directory(output, &["plugins", &target.appid])?;
            let content = ensure_directory(&root, &["plugin"])?;
            Ok((root, content, "package-manifest.json"))
        }
    }
}

fn write_manifest(
    root: &Path,
    target: &ExtractionTarget,
    mode: ExtractionMode,
    manifest_name: &str,
    packages: &[CachedPackage],
    sources: &BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    let package_values = packages
        .iter()
        .map(|package| {
            json!({
                "file_name": package.file_name,
                "role": package.role.as_str(),
                "original_path": package.path,
            })
        })
        .collect::<Vec<_>>();
    let file_values = sources
        .iter()
        .map(|(path, package_sources)| {
            json!({
                "path": path,
                "sources": package_sources,
            })
        })
        .collect::<Vec<_>>();
    let manifest = json!({
        "schema_version": 1,
            "appid": target.appid,
            "name": target.name,
        "version": target.version,
        "kind": target.kind.as_str(),
        "mode": mode.as_str(),
        "packages": package_values,
        "files": file_values,
    });
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    write_merged_file(root, manifest_name, &bytes)
}

pub fn extract_many_with_progress(
    targets: &[ExtractionTarget],
    output: &Path,
    mode: ExtractionMode,
    mut progress: impl FnMut(usize, usize, usize) -> Result<(), String>,
) -> Result<ExtractionSummary, String> {
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let output = output.canonicalize().map_err(|error| error.to_string())?;
    let total = targets
        .iter()
        .map(|target| target.package_count(mode))
        .sum();
    let mut completed = 0;
    let mut applet_count = 0;
    let mut plugin_count = 0;
    let mut package_count = 0;
    let mut file_count = 0;
    let mut observed_file_count = 0;
    let mut failures = Vec::new();
    progress(0, total, 0)?;

    for target in targets {
        let packages = selected_packages(target, mode).cloned().collect::<Vec<_>>();
        if packages.is_empty() {
            failures.push(ExtractionFailure {
                appid: target.appid.clone(),
                error: "没有可解压的缓存包".into(),
            });
            continue;
        }
        let directories = extraction_directories(&output, target, mode);
        let mut sources: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut target_error = directories.as_ref().err().cloned();
        let content = directories.ok().map(|(_, content, _)| content);

        for package in &packages {
            if target_error.is_none() {
                let result = Archive::open(Path::new(&package.path)).and_then(|archive| {
                    for (name, bytes) in archive.files() {
                        let normalized = name.trim_start_matches('/').to_owned();
                        write_merged_file(
                            content.as_ref().expect("content directory"),
                            name,
                            bytes,
                        )?;
                        let entry = sources.entry(normalized).or_default();
                        if entry.is_empty() {
                            observed_file_count += 1;
                        }
                        entry.push(package.file_name.clone());
                    }
                    Ok(())
                });
                if let Err(error) = result {
                    target_error = Some(format!("{}：{error}", package.file_name));
                }
            }
            completed += 1;
            progress(completed, total, observed_file_count)?;
        }

        if target_error.is_none() {
            let (root, _, manifest_name) = extraction_directories(&output, target, mode)?;
            if let Err(error) =
                write_manifest(&root, target, mode, manifest_name, &packages, &sources)
            {
                target_error = Some(error);
            }
        }
        if let Some(error) = target_error {
            failures.push(ExtractionFailure {
                appid: target.appid.clone(),
                error,
            });
        } else {
            match target.kind {
                ExtractionTargetKind::Applet => applet_count += 1,
                ExtractionTargetKind::Plugin => plugin_count += 1,
            }
            package_count += packages.len();
            file_count += sources.len();
        }
    }

    Ok(ExtractionSummary {
        applet_count,
        plugin_count,
        package_count,
        file_count,
        output: output.to_string_lossy().into_owned(),
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wxapkg::fixture;

    fn package(path: &Path, role: PackageRole) -> CachedPackage {
        CachedPackage {
            path: path.to_string_lossy().into_owned(),
            file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
            role,
        }
    }

    fn applet_target(appid: &str, packages: Vec<CachedPackage>) -> ExtractionTarget {
        ExtractionTarget {
            appid: appid.into(),
            name: "测试小程序".into(),
            version: "1".into(),
            kind: ExtractionTargetKind::Applet,
            packages,
        }
    }

    #[test]
    fn rejects_parent_paths() {
        assert!(safe_components("../outside.js").is_err());
        assert!(safe_components("pages\\outside.js").is_err());
    }

    #[test]
    fn output_directory_combines_sanitized_name_and_appid() {
        assert_eq!(
            safe_directory_name("测试/小程序. ", "wx0123456789abcdef"),
            "测试_小程序_wx0123456789abcdef"
        );
    }

    #[test]
    fn fixture_is_extractable() {
        let root = std::env::temp_dir().join(format!("wxapkg-extract-{}", std::process::id()));
        let package = root.with_extension("wxapkg");
        fs::write(&package, fixture(&[("pages/home.js", b"ok")])).unwrap();
        assert_eq!(extract_one(&package, &root).unwrap(), 1);
        assert_eq!(fs::read(root.join("pages/home.js")).unwrap(), b"ok");
        let _ = fs::remove_file(package);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn complete_mode_merges_main_and_subpackages_with_manifest() {
        let root = std::env::temp_dir().join(format!("wxapkg-complete-{}", std::process::id()));
        let package_dir = root.join("cache");
        let main = package_dir.join("__APP__.wxapkg");
        let sub = package_dir.join("_pages_order_.wxapkg");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(&main, fixture(&[("app.js", b"main")])).unwrap();
        fs::write(&sub, fixture(&[("pages/order.js", b"sub")])).unwrap();
        let target = applet_target(
            "wx0123456789abcdef",
            vec![
                package(&main, PackageRole::Main),
                package(&sub, PackageRole::Subpackage),
            ],
        );
        let summary = extract_many_with_progress(
            &[target],
            &root.join("output"),
            ExtractionMode::Complete,
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(summary.applet_count, 1);
        assert_eq!(summary.package_count, 2);
        assert_eq!(summary.file_count, 2);
        let output = root.join("output/测试小程序_wx0123456789abcdef");
        assert_eq!(fs::read(output.join("app/app.js")).unwrap(), b"main");
        assert_eq!(fs::read(output.join("app/pages/order.js")).unwrap(), b"sub");
        let manifest = fs::read_to_string(output.join("package-manifest.json")).unwrap();
        assert!(manifest.contains("subpackage"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn main_only_mode_excludes_subpackages() {
        let root = std::env::temp_dir().join(format!("wxapkg-main-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let main = root.join("__APP__.wxapkg");
        let sub = root.join("_pages_order_.wxapkg");
        fs::write(&main, fixture(&[("app.js", b"main")])).unwrap();
        fs::write(&sub, fixture(&[("pages/order.js", b"sub")])).unwrap();
        let target = applet_target(
            "wx0123456789abcdef",
            vec![
                package(&main, PackageRole::Main),
                package(&sub, PackageRole::Subpackage),
            ],
        );
        let summary = extract_many_with_progress(
            &[target],
            &root.join("output"),
            ExtractionMode::MainOnly,
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(summary.package_count, 1);
        assert!(!root
            .join("output/测试小程序_wx0123456789abcdef/main/pages/order.js")
            .exists());
        assert_eq!(
            fs::read(root.join("output/测试小程序_wx0123456789abcdef/main/app.js")).unwrap(),
            b"main"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn differing_duplicate_files_are_reported_without_overwrite() {
        let root = std::env::temp_dir().join(format!("wxapkg-conflict-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let main = root.join("__APP__.wxapkg");
        let sub = root.join("_sub_.wxapkg");
        fs::write(&main, fixture(&[("shared.js", b"main")])).unwrap();
        fs::write(&sub, fixture(&[("shared.js", b"sub")])).unwrap();
        let target = applet_target(
            "wx0123456789abcdef",
            vec![
                package(&main, PackageRole::Main),
                package(&sub, PackageRole::Subpackage),
            ],
        );
        let summary = extract_many_with_progress(
            &[target],
            &root.join("output"),
            ExtractionMode::Complete,
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(summary.failures.len(), 1);
        assert!(summary.failures[0].error.contains("文件内容冲突"));
        assert_eq!(
            fs::read(root.join("output/测试小程序_wx0123456789abcdef/app/shared.js")).unwrap(),
            b"main"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plugin_uses_separate_output_tree() {
        let root = std::env::temp_dir().join(format!("wxapkg-plugin-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let plugin = root.join("__PLUGINCODE__.wxapkg");
        fs::write(&plugin, fixture(&[("plugin.js", b"plugin")])).unwrap();
        let target = ExtractionTarget::plugin(
            "wxfedcba9876543210".into(),
            package(&plugin, PackageRole::Plugin),
        );
        let summary = extract_many_with_progress(
            &[target],
            &root.join("output"),
            ExtractionMode::Complete,
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(summary.plugin_count, 1);
        assert_eq!(
            fs::read(root.join("output/plugins/wxfedcba9876543210/plugin/plugin.js")).unwrap(),
            b"plugin"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_is_checked_before_writing() {
        let root = std::env::temp_dir().join(format!("wxapkg-cancel-{}", std::process::id()));
        let error = extract_many_with_progress(&[], &root, ExtractionMode::Complete, |_, _, _| {
            Err("操作已取消".into())
        })
        .unwrap_err();
        assert_eq!(error, "操作已取消");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn configured_real_cache_extracts_a_complete_applet() {
        let Ok(cache_root) = std::env::var("WXAPPLET_ROOT") else {
            return;
        };
        let rows = crate::cache::scan(Path::new(&cache_root)).unwrap();
        let applet = rows
            .iter()
            .find(|row| row.active_packages.len() > 1)
            .expect("测试缓存中至少需要一个包含分包的小程序");
        let output = std::env::temp_dir().join(format!(
            "wxapkg-real-extract-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let summary = extract_many_with_progress(
            &[ExtractionTarget::applet(applet)],
            &output,
            ExtractionMode::Complete,
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert!(summary.failures.is_empty(), "{:?}", summary.failures);
        assert_eq!(summary.package_count, applet.active_packages.len());
        let directory = output.join(safe_directory_name(&applet.name, &applet.appid));
        assert!(directory.join("package-manifest.json").is_file());
        let _ = fs::remove_dir_all(output);
    }
}
