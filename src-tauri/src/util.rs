//! Opening files and folders with the system, safely.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Open a file for viewing with its default app (folders open in File Explorer). Files that would
/// *run* when opened (scripts, programs) open in an editor instead: an agent may have written them.
pub fn open_path(p: &Path) {
    const RUNS: &[&str] = &["exe", "bat", "cmd", "com", "msi", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "scr", "lnk", "reg", "hta", "cpl", "pif", "appref-ms"];
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !p.is_dir() && RUNS.contains(&ext.as_str()) {
        if open_in_vscode(p).is_err() {
            let _ = std::process::Command::new("notepad").arg(p).spawn();
        }
        return;
    }
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

/// Show a file selected in File Explorer (a folder opens).
pub fn reveal_path(p: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if p.is_dir() {
            let _ = std::process::Command::new("explorer").arg(p).spawn();
        } else {
            let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", p.display())).spawn();
        }
    }
    #[cfg(not(windows))]
    let _ = p;
}

/// Open a file or folder in VS Code (a per-user or machine-wide install, or `code` on PATH).
pub fn open_in_vscode(path: &Path) -> Result<(), String> {
    let installs = [
        dirs::data_local_dir().map(|d| d.join("Programs").join("Microsoft VS Code").join("Code.exe")),
        std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Microsoft VS Code").join("Code.exe")),
    ];
    let exe = installs.into_iter().flatten().find(|p| p.is_file()).or_else(|| which::which("code").ok());
    let Some(exe) = exe else { return Err("VS Code isn't installed, or `code` isn't on PATH.".into()) };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Couldn't start VS Code: {e}"))
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
