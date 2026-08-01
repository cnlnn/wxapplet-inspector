use crate::cache::{self, CacheLayout};
use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};
use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateSource {
    RunningProcess,
    KnownPath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheCandidate {
    pub path: PathBuf,
    pub layout: CacheLayout,
    pub source: CandidateSource,
    pub app_count: usize,
    pub modified_at: u64,
}

fn push_unique(
    paths: &mut Vec<(PathBuf, CandidateSource)>,
    path: PathBuf,
    source: CandidateSource,
) {
    if !paths.iter().any(|(existing, _)| existing == &path) {
        paths.push((path, source));
    }
}

fn child_directories(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn add_radium_candidates(
    paths: &mut Vec<(PathBuf, CandidateSource)>,
    radium: &Path,
    source: CandidateSource,
) {
    for applet in [radium.join("Applet"), radium.join("applet")] {
        push_unique(paths, applet.clone(), source);
        for isolated in child_directories(&applet) {
            if isolated.join("packages").is_dir() {
                push_unique(paths, isolated, source);
            }
        }
    }
    for user in child_directories(&radium.join("users")) {
        push_unique(paths, user.join("applet"), source);
        push_unique(paths, user.join("Applet"), source);
    }
}

fn parse_wmpf_roots(arguments: &[OsString]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let marker = OsStr::new("--wmpf_root_dir");
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == marker {
            if let Some(value) = arguments.get(index + 1) {
                roots.push(PathBuf::from(value));
                index += 2;
                continue;
            }
        }
        let value = argument.to_string_lossy();
        if let Some(path) = value.strip_prefix("--wmpf_root_dir=") {
            if !path.is_empty() {
                roots.push(PathBuf::from(path));
            }
        }
        index += 1;
    }
    roots
}

fn is_wechat_process(name: &OsStr, cmd: &[OsString]) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    name.contains("wechat")
        || name.contains("weixin")
        || name == "weapp"
        || cmd.first().is_some_and(|value| {
            let value = value.to_string_lossy().to_ascii_lowercase();
            value.contains("wechat") || value.contains("weixin") || value.contains("weapp")
        })
}

fn process_candidates() -> Vec<(PathBuf, CandidateSource)> {
    let refresh = RefreshKind::nothing().with_processes(
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    let system = System::new_with_specifics(refresh);
    let mut paths = Vec::new();
    for process in system.processes().values() {
        for root in parse_wmpf_roots(process.cmd()) {
            add_radium_candidates(&mut paths, &root, CandidateSource::RunningProcess);
        }
        if !is_wechat_process(process.name(), process.cmd()) {
            continue;
        }
        for variable in process.environ() {
            let value = variable.to_string_lossy();
            if let Some(home) = value.strip_prefix("HOME=") {
                add_radium_candidates(
                    &mut paths,
                    &Path::new(home).join(".xwechat/radium"),
                    CandidateSource::RunningProcess,
                );
            }
        }
    }
    paths
}

fn known_candidates() -> Vec<(PathBuf, CandidateSource)> {
    let mut paths = Vec::new();
    let source = CandidateSource::KnownPath;
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from);

    #[cfg(target_os = "linux")]
    if let Some(home) = &home {
        add_radium_candidates(&mut paths, &home.join(".xwechat/radium"), source);
        for id in ["com.tencent.WeChat", "com.tencent.WeChatLinux"] {
            add_radium_candidates(
                &mut paths,
                &home.join(".var/app").join(id).join("data/.xwechat/radium"),
                source,
            );
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(config) = env::var_os("APPDATA").map(PathBuf::from) {
            add_radium_candidates(&mut paths, &config.join("Tencent/xwechat/radium"), source);
            add_radium_candidates(&mut paths, &config.join("Tencent/WeChat/radium"), source);
        }
        if let Some(home) = &home {
            push_unique(
                &mut paths,
                home.join("Documents/WeChat Files/Applet"),
                source,
            );
        }
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = &home {
        let container = home.join("Library/Containers/com.tencent.xinWeChat/Data");
        add_radium_candidates(
            &mut paths,
            &container.join("Documents/app_data/radium"),
            source,
        );
        push_unique(&mut paths, container.join(".wxapplet"), source);
        push_unique(
            &mut paths,
            home.join("Library/Application Support/WeChat/Applet"),
            source,
        );
    }
    paths
}

pub fn discover_cache_roots() -> Vec<CacheCandidate> {
    let mut raw = process_candidates();
    raw.extend(known_candidates());
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for (path, source) in raw {
        let Some(info) = cache::inspect_cache_root(&path) else {
            continue;
        };
        if !seen.insert(info.root.clone()) {
            if source == CandidateSource::RunningProcess {
                if let Some(existing) = candidates
                    .iter_mut()
                    .find(|candidate: &&mut CacheCandidate| candidate.path == info.root)
                {
                    existing.source = source;
                }
            }
            continue;
        }
        candidates.push(CacheCandidate {
            path: info.root,
            layout: info.layout,
            source,
            app_count: info.app_count,
            modified_at: info.modified_at,
        });
    }
    candidates.sort_by(|left, right| {
        right
            .modified_at
            .cmp(&left.modified_at)
            .then_with(|| {
                (right.source == CandidateSource::RunningProcess)
                    .cmp(&(left.source == CandidateSource::RunningProcess))
            })
            .then_with(|| right.app_count.cmp(&left.app_count))
            .then_with(|| left.path.cmp(&right.path))
    });
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_joined_and_split_wmpf_arguments() {
        let args = vec![
            OsString::from("WeChatAppEx"),
            OsString::from("--wmpf_root_dir=/tmp/微信/radium"),
            OsString::from("--wmpf_root_dir"),
            OsString::from("/tmp/second/radium"),
        ];
        assert_eq!(
            parse_wmpf_roots(&args),
            vec![
                PathBuf::from("/tmp/微信/radium"),
                PathBuf::from("/tmp/second/radium")
            ]
        );
    }

    #[test]
    fn ignores_empty_or_incomplete_wmpf_arguments() {
        let args = vec![
            OsString::from("--wmpf_root_dir="),
            OsString::from("--wmpf_root_dir"),
        ];
        assert!(parse_wmpf_roots(&args).is_empty());
    }

    #[test]
    fn discovers_expected_running_wechat_root_when_configured() {
        let Some(expected) = env::var_os("WXAPPLET_EXPECT_DISCOVERY").map(PathBuf::from) else {
            return;
        };
        let expected = expected.canonicalize().unwrap();
        let candidates = discover_cache_roots();
        assert_eq!(
            candidates.first().map(|candidate| &candidate.path),
            Some(&expected),
            "最佳候选不是当前微信正在使用的缓存目录：{candidates:?}"
        );
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.path == expected),
            "未从微信进程定位到 {}，实际候选：{:?}",
            expected.display(),
            candidates
        );
        let rows = cache::scan(&expected).unwrap();
        assert!(!rows.is_empty(), "自动定位后的目录未扫描出小程序");
    }
}
