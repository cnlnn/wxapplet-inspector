use std::{path::Path, process::Command};

pub fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        value.into_owned()
    }
}

pub fn open_path(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("目录不存在：{}", path.display()));
    }

    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    return Err("当前平台不支持打开目录".into());

    command
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开目录：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_windows_extended_path_prefix_for_display() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\Users\root\Applet")),
            r"C:\Users\root\Applet"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\Applet")),
            r"\\server\share\Applet"
        );
        assert_eq!(display_path(Path::new("/tmp/Applet")), "/tmp/Applet");
    }
}
