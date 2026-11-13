//! The shells a terminal bay can run, found on this computer, the default first: PowerShell 7,
//! Windows PowerShell, Command Prompt and Git Bash (plus WSL when it's installed).

use std::path::PathBuf;

use super::TerminalShell;

#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub id: String,
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Set for this shell only, such as Git Bash's `CHERE_INVOKING`.
    pub env: Vec<(String, String)>,
}

impl Shell {
    pub fn new(id: &str, name: &str, program: PathBuf, args: &[&str]) -> Self {
        Self { id: id.into(), name: name.into(), program, args: args.iter().map(|a| a.to_string()).collect(), env: Vec::new() }
    }

    pub fn describe(&self, default: bool) -> TerminalShell {
        TerminalShell { id: self.id.clone(), name: self.name.clone(), path: self.program.display().to_string(), default }
    }

    /// A shell from the user's setting: a program, optionally with arguments ("pwsh -NoProfile").
    pub fn custom(command: &str) -> Option<Shell> {
        let command = command.trim();
        if command.is_empty() {
            return None;
        }
        let (program, rest) = match command.strip_prefix('"') {
            Some(quoted) => {
                let (p, rest) = quoted.split_once('"')?;
                (p.to_owned(), rest.trim().to_owned())
            }
            None => match command.split_once(' ') {
                Some((p, rest)) => (p.to_owned(), rest.trim().to_owned()),
                None => (command.to_owned(), String::new()),
            },
        };
        let path = PathBuf::from(&program);
        let path = if path.is_file() { path } else { which::which(&program).ok()? };
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(program);
        Some(Shell { id: "custom".into(), name, program: path, args: rest.split_whitespace().map(str::to_owned).collect(), env: Vec::new() })
    }
}

/// The shells on this computer, the default first. A shell chosen in Settings leads: one of the
/// found shells by its id (it keeps its own arguments and environment), or a custom command line.
pub fn available(custom: Option<&str>) -> Vec<Shell> {
    let mut shells = detect();
    if let Some(choice) = custom.map(str::trim).filter(|c| !c.is_empty()) {
        if let Some(i) = shells.iter().position(|s| s.id.eq_ignore_ascii_case(choice)) {
            let chosen = shells.remove(i);
            shells.insert(0, chosen);
        } else if let Some(shell) = Shell::custom(choice) {
            shells.insert(0, shell);
        }
    }
    shells
}

/// The shell `id` names, or the default when it's not given or no longer installed.
pub fn resolve(id: Option<&str>, custom: Option<&str>) -> Option<Shell> {
    let shells = available(custom);
    id.and_then(|id| shells.iter().find(|s| s.id == id).cloned()).or_else(|| shells.into_iter().next())
}

/// A tab title from the one a shell set. Shells often title themselves with their own program's
/// path (`C:\Program Files\PowerShell\7\pwsh.exe`), which says less than its name.
pub fn friendly_title(title: &str) -> String {
    let t = title.trim();
    let file = t.rsplit(['\\', '/']).next().unwrap_or(t);
    let Some(program) = file.len().checked_sub(4).filter(|&n| file.is_char_boundary(n) && file[n..].eq_ignore_ascii_case(".exe")).map(|n| &file[..n]) else {
        return t.chars().take(120).collect();
    };
    match program.to_ascii_lowercase().as_str() {
        "pwsh" => "PowerShell".into(),
        "powershell" => "Windows PowerShell".into(),
        "cmd" => "Command Prompt".into(),
        "bash" => "Bash".into(),
        "wsl" => "WSL".into(),
        _ => program.chars().take(120).collect(),
    }
}

#[cfg(windows)]
fn detect() -> Vec<Shell> {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32");
    let mut shells = Vec::new();
    if let Some(pwsh) = find_pwsh() {
        shells.push(Shell::new("pwsh", "PowerShell", pwsh, &["-NoLogo"]));
    }
    let powershell = system.join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    if powershell.is_file() {
        shells.push(Shell::new("powershell", "Windows PowerShell", powershell, &["-NoLogo"]));
    }
    let cmd = std::env::var_os("ComSpec").map(PathBuf::from).filter(|p| p.is_file()).unwrap_or_else(|| system.join("cmd.exe"));
    if cmd.is_file() {
        shells.push(Shell::new("cmd", "Command Prompt", cmd, &[]));
    }
    if let Some(bash) = find_git_bash() {
        let mut shell = Shell::new("git-bash", "Git Bash", bash, &["--login", "-i"]);
        // Git Bash's profile changes to the home folder unless this is set.
        shell.env.push(("CHERE_INVOKING".into(), "1".into()));
        shells.push(shell);
    }
    let wsl = system.join("wsl.exe");
    if wsl.is_file() && std::env::var_os("LOCALAPPDATA").map(PathBuf::from).is_some_and(|d| d.join("Packages").is_dir()) && wsl_installed() {
        shells.push(Shell::new("wsl", "WSL", wsl, &[]));
    }
    shells
}

/// WSL has a distribution (wsl.exe exists on every Windows 11, installed or not).
#[cfg(windows)]
fn wsl_installed() -> bool {
    let key = dirs::home_dir().map(|h| h.join("AppData").join("Local").join("wsl"));
    key.is_some_and(|k| k.is_dir())
}

#[cfg(windows)]
fn find_pwsh() -> Option<PathBuf> {
    if let Ok(path) = which::which("pwsh") {
        return Some(path);
    }
    ["ProgramFiles", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| PathBuf::from(dir).join("PowerShell").join("7").join("pwsh.exe"))
        .find(|path| path.is_file())
}

/// Git for Windows' `bin\bash.exe`, never `System32\bash.exe` (which starts WSL).
#[cfg(windows)]
fn find_git_bash() -> Option<PathBuf> {
    let git = which::which("git").ok();
    let roots: Vec<PathBuf> = git.as_deref().map(|g| g.ancestors().skip(1).take(3).map(PathBuf::from).collect()).unwrap_or_default();
    let installs = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| PathBuf::from(dir).join("Git"))
        .chain(std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Programs").join("Git")));
    roots.into_iter().chain(installs).map(|root| root.join("bin").join("bash.exe")).find(|path| path.is_file())
}

#[cfg(not(windows))]
fn detect() -> Vec<Shell> {
    let login = std::env::var_os("SHELL").map(PathBuf::from).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("/bin/sh"));
    let name = login.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "sh".into());
    vec![Shell::new(&name, &name, login, &["-l"])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_shell_reads_its_program_and_arguments() {
        let Some(shell) = Shell::custom("cmd /K echo hi") else { return };
        assert_eq!(shell.id, "custom");
        assert_eq!(shell.args, ["/K", "echo", "hi"]);
        assert!(Shell::custom("   ").is_none());
        assert!(Shell::custom("no-such-shell-xyz").is_none());
    }

    #[test]
    fn an_unknown_shell_falls_back_to_the_default() {
        let shells = available(None);
        let Some(first) = shells.first() else { return };
        assert_eq!(resolve(Some("no-such-shell"), None).as_ref(), Some(first));
        assert_eq!(resolve(None, None).as_ref(), Some(first));
    }

    #[test]
    fn a_shell_titled_with_its_program_path_gets_its_name() {
        assert_eq!(friendly_title(r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7.5.4.0_x64__8wekyb3d8bbwe\pwsh.exe"), "PowerShell");
        assert_eq!(friendly_title(r"C:\WINDOWS\system32\cmd.EXE"), "Command Prompt");
        assert_eq!(friendly_title("/usr/bin/vim.exe"), "vim");
        assert_eq!(friendly_title("  npm run dev  "), "npm run dev");
        assert_eq!(friendly_title("~/code/graysense"), "~/code/graysense");
    }
}
