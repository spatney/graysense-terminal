//! Opening files and folders with the system, safely.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Open a file for viewing with its default app (folders open in the native file manager). Files that would
/// *run* when opened (scripts, programs) open in an editor instead: an agent may have written them.
pub fn open_path(p: &Path) -> Result<(), String> {
    let p = std::path::absolute(p).map_err(|e| format!("Couldn't resolve {}: {e}", p.display()))?;
    if needs_editor(&p)? {
        return match open_in_vscode(&p) {
            Ok(()) => Ok(()),
            Err(editor_error) => {
                let mut cmd = text_editor();
                cmd.arg(&p);
                run_opener(&mut cmd).map_err(|e| format!("{editor_error} {e}"))
            }
        };
    }
    let mut cmd = system_opener();
    cmd.arg(&p);
    run_opener(&mut cmd)
}

fn needs_editor(p: &Path) -> Result<bool, String> {
    const RUNS: &[&str] = &["exe", "bat", "cmd", "com", "msi", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "scr", "lnk", "reg", "hta", "cpl", "pif", "appref-ms",
        "app", "pkg", "mpkg", "command", "sh", "bash", "zsh", "fish", "py", "rb", "pl", "jar", "scpt", "scptd", "applescript", "workflow", "action", "webloc", "url", "desktop"];
    let meta = std::fs::metadata(p).map_err(|e| format!("Couldn't open {}: {e}", p.display()))?;
    let target = std::fs::canonicalize(p).map_err(|e| format!("Couldn't resolve {}: {e}", p.display()))?;
    if [p, target.as_path()].into_iter().any(|path| {
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        RUNS.contains(&ext.as_str()) && (!meta.is_dir() || ["app", "pkg", "mpkg", "scptd", "workflow", "action"].contains(&ext.as_str()))
    }) {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
            return Ok(true);
        }
    }
    let _ = meta;
    Ok(false)
}

fn system_opener() -> std::process::Command {
    #[cfg(windows)]
    let cmd = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("/usr/bin/open");
    #[cfg(not(any(windows, target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open");
    cmd
}

fn text_editor() -> std::process::Command {
    #[cfg(windows)]
    let cmd = std::process::Command::new("notepad");
    #[cfg(target_os = "macos")]
    let cmd = {
        let mut cmd = system_opener();
        cmd.arg("-t");
        cmd
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let cmd = std::process::Command::new("gedit");
    cmd
}

fn run_opener(cmd: &mut std::process::Command) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.spawn().map(|_| ()).map_err(|e| format!("Couldn't open with {}: {e}", cmd.get_program().to_string_lossy()))
    }
    #[cfg(not(windows))]
    {
        let output = cmd.output().map_err(|e| format!("Couldn't open with {}: {e}", cmd.get_program().to_string_lossy()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!("Couldn't open with {} ({}): {}", cmd.get_program().to_string_lossy(), output.status, String::from_utf8_lossy(&output.stderr).trim()))
        }
    }
}

/// Show a file selected in Finder or File Explorer (a normal folder opens).
pub fn reveal_path(p: &Path) -> Result<(), String> {
    run_opener(&mut reveal_command(p)?)
}

fn reveal_command(p: &Path) -> Result<std::process::Command, String> {
    let p = std::path::absolute(p).map_err(|e| format!("Couldn't resolve {}: {e}", p.display()))?;
    let meta = std::fs::metadata(&p).map_err(|e| format!("Couldn't reveal {}: {e}", p.display()))?;
    let mut cmd = system_opener();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if meta.is_dir() {
            cmd.arg(&p);
        } else {
            cmd.raw_arg(format!("/select,\"{}\"", p.display()));
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Always reveal bundles; opening an .app folder would launch it.
        if !meta.is_dir() || needs_editor(&p)? {
            cmd.arg("-R");
        }
        cmd.arg(&p);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    cmd.arg(if meta.is_dir() { p.as_path() } else { p.parent().ok_or("That file has no parent folder.")? });
    Ok(cmd)
}

/// Open a file or folder in VS Code (a per-user or machine-wide install, or `code` on PATH).
pub fn open_in_vscode(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let installs = [
        Some(PathBuf::from("/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code")),
        dirs::home_dir().map(|h| h.join("Applications/Visual Studio Code.app/Contents/Resources/app/bin/code")),
    ];
    #[cfg(not(target_os = "macos"))]
    let installs = [
        dirs::data_local_dir().map(|d| d.join("Programs").join("Microsoft VS Code").join("Code.exe")),
        std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Microsoft VS Code").join("Code.exe")),
    ];
    let exe = installs.into_iter().flatten().find(|p| p.is_file()).or_else(|| which::which("code").ok());
    let Some(exe) = exe else { return Err("VS Code isn't installed, or `code` isn't on PATH.".into()) };
    let mut cmd = std::process::Command::new(exe);
    let path = std::path::absolute(path).map_err(|e| format!("Couldn't resolve {}: {e}", path.display()))?;
    cmd.arg("--").arg(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    run_opener(&mut cmd)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: i64,
}

/// What's in a folder: folders first, then files, by name; hidden and build folders left out.
pub fn list_dir(dir: &Path) -> Result<Vec<DirEntry>, String> {
    directory_entries(dir, None)
}

/// Path completion includes dotfiles and build folders when explicitly browsing for a file.
pub fn complete_path(dir: &Path, prefix: &str) -> Result<Vec<DirEntry>, String> {
    directory_entries(dir, Some(prefix))
}

fn directory_entries(dir: &Path, prefix: Option<&str>) -> Result<Vec<DirEntry>, String> {
    const SKIP: &[&str] = &["node_modules", "target", ".git", "__pycache__", ".venv", "dist", ".next"];
    let read = std::fs::read_dir(dir).map_err(|e| format!("Couldn't read {}: {e}", dir.display()))?;
    let prefix = prefix.map(str::to_lowercase);
    let mut out: Vec<DirEntry> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(prefix) = &prefix {
                if !name.to_lowercase().starts_with(prefix) {
                    return None;
                }
            } else if name.starts_with('.') && name != ".github" || SKIP.contains(&name.as_str()) {
                return None;
            }
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            Some(DirEntry { name, path: e.path().display().to_string(), is_dir: meta.is_dir(), size: meta.len(), modified })
        })
        .collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out.truncate(if prefix.is_some() { 50 } else { 2_000 });
    Ok(out)
}

/// The start of a text file, for previews (`None` for binaries).
pub fn read_text_head(path: &Path, max: usize) -> Result<Option<String>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("Couldn't open {}: {e}", path.display()))?;
    let mut buf = vec![0u8; max];
    let n = f.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);
    if buf.contains(&0) {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Image files Graysense previews, by extension.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "avif", "svg"];
const PREVIEW_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// An image file's bytes, for a preview. Only image files (by extension), up to 50 MB.
pub fn read_image(path: &Path) -> Result<Vec<u8>, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("{} isn't an image.", path.display()));
    }
    let failed = |e: std::io::Error| format!("Couldn't open {}: {e}", path.display());
    let meta = std::fs::metadata(path).map_err(failed)?;
    if !meta.is_file() {
        return Err(format!("{} isn't an image.", path.display()));
    }
    if meta.len() > PREVIEW_MAX_BYTES {
        return Err("That image is too big to preview.".into());
    }
    std::fs::read(path).map_err(failed)
}

/// `path` with `.` and `..` worked out without touching the disk, so it reads the way it was typed.
pub fn tidy_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // Nothing is above a drive's root.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other),
        }
    }
    out
}

const PASTED_PREFIX: &str = "pasted-image-";
const PASTED_MAX_BYTES: usize = 50 * 1024 * 1024;
/// Long enough to come back to a session, short enough that screenshots don't pile up.
const PASTED_KEEP_DAYS: u64 = 30;

/// The extension for image bytes a model can be shown (PNG, JPEG, GIF or WebP), by signature.
fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("jpg"),
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Some("gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("webp"),
        _ => None,
    }
}

/// Save an image pasted into a message box in `dir` as a new `pasted-image-<time>` file, first
/// clearing out images pasted there more than [`PASTED_KEEP_DAYS`] ago.
pub fn save_pasted_image(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    if bytes.len() > PASTED_MAX_BYTES {
        return Err("That image is too big to paste (the limit is 50 MB).".into());
    }
    let ext = image_extension(bytes).ok_or("Graysense can paste PNG, JPEG, GIF and WebP images.")?;
    let failed = |e: std::io::Error| format!("Couldn't save the pasted image: {e}");
    std::fs::create_dir_all(dir).map_err(failed)?;
    prune_pasted_images(dir, std::time::Duration::from_secs(PASTED_KEEP_DAYS * 86_400));
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut n = 1;
    loop {
        let suffix = if n == 1 { String::new() } else { format!("-{n}") };
        let path = dir.join(format!("{PASTED_PREFIX}{stamp}{suffix}.{ext}"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(failed(e));
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(failed(e)),
        }
    }
}

/// Delete pasted images (only those) last written longer than `keep` ago.
fn prune_pasted_images(dir: &Path, keep: std::time::Duration) {
    let Some(cutoff) = std::time::SystemTime::now().checked_sub(keep) else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().is_ok_and(|m| m.is_file() && m.modified().is_ok_and(|t| t < cutoff));
        if old && entry.file_name().to_string_lossy().starts_with(PASTED_PREFIX) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_report_errors_instead_of_launching_an_opener() {
        let missing = std::env::temp_dir().join(format!("gs-missing-{}", uuid::Uuid::new_v4()));
        assert!(open_path(&missing).unwrap_err().contains("Couldn't open"));
        assert!(reveal_path(&missing).unwrap_err().contains("Couldn't reveal"));
        assert!(run_opener(&mut std::process::Command::new("graysense-no-such-opener")).is_err());
    }

    #[test]
    fn scripts_installers_and_app_bundles_are_never_opened_as_programs() {
        let dir = std::env::temp_dir().join(format!("gs-safe-open-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Example.app")).unwrap();
        for name in ["script.command", "script.SH", "script.py", "installer.pkg", "script.scpt", "script.exe"] {
            let path = dir.join(name);
            std::fs::write(&path, "do not execute").unwrap();
            assert!(needs_editor(&path).unwrap(), "{name}");
        }
        assert!(needs_editor(&dir.join("Example.app")).unwrap());
        assert!(!needs_editor(&dir).unwrap());
        let text = dir.join("readme.txt");
        std::fs::write(&text, "text").unwrap();
        assert!(!needs_editor(&text).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = dir.join("no-extension");
            std::fs::write(&executable, "#!/bin/sh\nexit 1").unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(needs_editor(&executable).unwrap());
            let link = dir.join("looks-like-text.txt");
            std::os::unix::fs::symlink(&executable, &link).unwrap();
            assert!(needs_editor(&link).unwrap());
            let bundle_link = dir.join("looks-like-folder");
            std::os::unix::fs::symlink(dir.join("Example.app"), &bundle_link).unwrap();
            assert!(needs_editor(&bundle_link).unwrap());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finder_reveals_files_and_bundles_but_opens_regular_folders() {
        let dir = std::env::temp_dir().join(format!("gs-finder-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Example.app")).unwrap();
        let file = dir.join("a file.txt");
        std::fs::write(&file, "text").unwrap();
        for path in [&file, &dir.join("Example.app")] {
            let cmd = reveal_command(path).unwrap();
            assert_eq!(cmd.get_program(), "/usr/bin/open");
            assert_eq!(cmd.get_args().collect::<Vec<_>>(), [std::ffi::OsStr::new("-R"), path.as_os_str()]);
        }
        assert_eq!(reveal_command(&dir).unwrap().get_args().collect::<Vec<_>>(), [dir.as_os_str()]);
        assert_eq!(text_editor().get_args().collect::<Vec<_>>(), [std::ffi::OsStr::new("-t")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn path_completion_filters_before_limiting_and_includes_hidden_files() {
        let dir = std::env::temp_dir().join(format!("gs-complete-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Source Files")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join(".env"), "").unwrap();
        std::fs::write(dir.join("SOURCE.rs"), "").unwrap();
        for i in 0..55 {
            std::fs::write(dir.join(format!("a{i}.txt")), "").unwrap();
        }
        let entries = complete_path(&dir, "sou").unwrap();
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Source Files", "SOURCE.rs"]);
        assert!(entries[0].is_dir);
        assert!(Path::new(&entries[1].path).is_absolute());
        assert_eq!(complete_path(&dir, ".e").unwrap()[0].name, ".env");
        assert_eq!(complete_path(&dir, "node").unwrap()[0].name, "node_modules");
        assert_eq!(complete_path(&dir, "").unwrap().len(), 50);
        assert!(complete_path(&dir, "missing").unwrap().is_empty());
        assert!(complete_path(&dir.join("missing"), "").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folders_list_first_and_build_output_is_skipped() {
        let dir = std::env::temp_dir().join(format!("gs-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::write(dir.join("A.md"), "a").unwrap();
        let names: Vec<String> = list_dir(&dir).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["src", "A.md", "b.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_image_files_are_read_for_previews() {
        let dir = std::env::temp_dir().join(format!("gs-image-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("folder.png")).unwrap();
        let png = dir.join("Shot.PNG");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert_eq!(read_image(&png).unwrap(), b"\x89PNG\r\n\x1a\nrest");
        let text = dir.join("secrets.txt");
        std::fs::write(&text, "token").unwrap();
        assert!(read_image(&text).is_err());
        assert!(read_image(&dir.join("folder.png")).is_err());
        assert!(read_image(&dir.join("missing.png")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn paths_are_tidied_as_typed() {
        assert_eq!(tidy_path(Path::new(r"C:\code\app\..\web\.\src\")), PathBuf::from(r"C:\code\web\src"));
        assert_eq!(tidy_path(Path::new(r"C:\..\code")), PathBuf::from(r"C:\code"));
        assert_eq!(tidy_path(Path::new(r"..\..\x")), PathBuf::from(r"..\..\x"));
    }

    #[test]
    fn pasted_images_are_saved_by_type_without_replacing_and_expire() {
        let dir = std::env::temp_dir().join(format!("gs-pasted-{}", uuid::Uuid::new_v4()));
        let png = b"\x89PNG\r\n\x1a\nrest";
        let first = save_pasted_image(&dir, png).unwrap();
        let second = save_pasted_image(&dir, png).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.extension().unwrap(), "png");
        assert_eq!(std::fs::read(&second).unwrap(), png);
        let jpg = save_pasted_image(&dir, b"\xFF\xD8\xFF\xE0").unwrap();
        assert_eq!(jpg.extension().unwrap(), "jpg");
        assert_eq!(save_pasted_image(&dir, b"RIFF\0\0\0\0WEBPVP8 ").unwrap().extension().unwrap(), "webp");
        assert!(save_pasted_image(&dir, b"<svg></svg>").is_err());
        assert!(save_pasted_image(&dir, &vec![0; PASTED_MAX_BYTES + 1]).is_err());

        let month_ago = std::time::SystemTime::now() - std::time::Duration::from_secs((PASTED_KEEP_DAYS + 1) * 86_400);
        let other = dir.join("other.png");
        std::fs::write(&other, png).unwrap();
        for old in [&jpg, &other] {
            std::fs::File::options().write(true).open(old).unwrap().set_modified(month_ago).unwrap();
        }
        let third = save_pasted_image(&dir, png).unwrap();
        assert!(!jpg.exists());
        assert!([&first, &second, &third, &other].iter().all(|p| p.exists()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
