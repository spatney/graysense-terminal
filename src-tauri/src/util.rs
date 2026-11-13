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
    const SKIP: &[&str] = &["node_modules", "target", ".git", "__pycache__", ".venv", "dist", ".next"];
    let read = std::fs::read_dir(dir).map_err(|e| format!("Couldn't read {}: {e}", dir.display()))?;
    let mut out: Vec<DirEntry> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && name != ".github" || SKIP.contains(&name.as_str()) {
                return None;
            }
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            Some(DirEntry { name, path: e.path().display().to_string(), is_dir: meta.is_dir(), size: meta.len(), modified })
        })
        .collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out.truncate(2_000);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
