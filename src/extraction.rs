use crate::{cache::is_appid, wxapkg::Archive};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

fn safe_components(name: &str) -> Result<Vec<&std::ffi::OsStr>, String> {
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

fn write_file(base: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let components = safe_components(name.trim_start_matches('/'))?;
    let mut target = base.to_path_buf();
    for component in &components {
        target.push(component);
    }
    let parent = target.parent().ok_or("无效输出路径")?;
    let mut current = base.to_path_buf();
    for component in parent
        .strip_prefix(base)
        .map_err(|_| "无效输出路径")?
        .components()
    {
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
    if fs::symlink_metadata(&target)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(format!("拒绝覆盖符号链接：{}", target.display()));
    }
    fs::write(target, bytes).map_err(|error| error.to_string())
}

fn extract_to(package: &Path, output: &Path) -> Result<usize, String> {
    let archive = Archive::open(package)?;
    let mut count = 0;
    for (name, bytes) in archive.files() {
        write_file(output, name, bytes)?;
        count += 1;
    }
    Ok(count)
}

pub fn extract_one(package: &Path, output: &Path) -> Result<usize, String> {
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let output = output.canonicalize().map_err(|error| error.to_string())?;
    extract_to(package, &output)
}

#[derive(Clone, Debug)]
pub struct ExtractionFailure {
    pub appid: String,
    pub error: String,
}

#[derive(Clone, Debug)]
pub struct ExtractionSummary {
    pub package_count: usize,
    pub file_count: usize,
    pub output: String,
    pub failures: Vec<ExtractionFailure>,
}

pub fn extract_many_with_progress(
    packages: &[PathBuf],
    output: &Path,
    mut progress: impl FnMut(usize, usize, usize) -> Result<(), String>,
) -> Result<ExtractionSummary, String> {
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let output = output.canonicalize().map_err(|error| error.to_string())?;
    let total = packages.len();
    let mut package_count = 0;
    let mut file_count = 0;
    let mut failures = Vec::new();
    progress(0, total, 0)?;
    for (index, package) in packages.iter().enumerate() {
        let appid = package
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .filter(|value| is_appid(value))
            .map(str::to_owned);
        let Some(appid) = appid else {
            failures.push(ExtractionFailure {
                appid: package.to_string_lossy().into_owned(),
                error: format!("主包路径不符合缓存目录结构：{}", package.display()),
            });
            progress(index + 1, total, file_count)?;
            continue;
        };
        let target = output.join(&appid);
        let result = fs::create_dir_all(&target)
            .map_err(|error| error.to_string())
            .and_then(|_| extract_to(package, &target));
        match result {
            Ok(count) => {
                package_count += 1;
                file_count += count;
            }
            Err(error) => failures.push(ExtractionFailure { appid, error }),
        }
        progress(index + 1, total, file_count)?;
    }
    Ok(ExtractionSummary {
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
    #[test]
    fn rejects_parent_paths() {
        assert!(safe_components("../outside.js").is_err());
    }
    #[test]
    fn accepts_normal_paths() {
        assert_eq!(safe_components("pages/home.js").unwrap().len(), 2);
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
    fn batch_extraction_reports_progress_and_partial_failures() {
        let root = std::env::temp_dir().join(format!(
            "wxapkg-batch-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let package_dir = root.join("packages/wx0123456789abcdef/1");
        let valid = package_dir.join("__APP__.wxapkg");
        let missing = root.join("packages/wxfedcba9876543210/1/__APP__.wxapkg");
        let output = root.join("output");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(&valid, fixture(&[("app.js", b"ok")])).unwrap();

        let mut events = Vec::new();
        let summary =
            extract_many_with_progress(&[valid, missing], &output, |done, total, files| {
                events.push((done, total, files));
                Ok(())
            })
            .unwrap();

        assert_eq!(summary.package_count, 1);
        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.failures.len(), 1);
        assert_eq!(summary.failures[0].appid, "wxfedcba9876543210");
        assert_eq!(events, vec![(0, 2, 0), (1, 2, 1), (2, 2, 1)]);
        assert_eq!(
            fs::read(output.join("wx0123456789abcdef/app.js")).unwrap(),
            b"ok"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn batch_extraction_can_be_cancelled_before_writing() {
        let root = std::env::temp_dir().join(format!(
            "wxapkg-cancel-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let error =
            extract_many_with_progress(&[], &root, |_, _, _| Err("操作已取消".into())).unwrap_err();
        assert_eq!(error, "操作已取消");
        let _ = fs::remove_dir_all(root);
    }
}
