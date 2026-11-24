//! "Install the `blaze` command": the CLI ships inside the app (a Tauri
//! sidecar next to the main executable). This puts it on the user's PATH,
//! like VS Code's `code` command.
//!
//! macOS: a symlink /usr/local/bin/blaze -> the binary in the .app (asks for an
//! administrator password when /usr/local/bin is not writable).
//! Windows: the install folder is added to the user's PATH.

use std::path::PathBuf;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    /// This build carries the CLI (release builds do, dev builds do not).
    pub available: bool,
    pub installed: bool,
    pub path: String,
}

fn bundled() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let bin = dir.join(if cfg!(windows) { "blaze.exe" } else { "blaze" });
    bin.is_file().then_some(bin)
}

#[cfg(target_os = "macos")]
const LINK: &str = "/usr/local/bin/blaze";

pub fn status() -> CliStatus {
    let Some(bin) = bundled() else { return CliStatus { available: false, installed: false, path: String::new() } };
    #[cfg(target_os = "macos")]
    let installed = std::fs::read_link(LINK).ok().is_some_and(|t| t == bin);
    #[cfg(windows)]
    let installed = {
        let dir = bin.parent().map(|d| d.to_string_lossy().to_lowercase()).unwrap_or_default();
        std::env::var("PATH").unwrap_or_default().to_lowercase().split(';').any(|p| p.trim_end_matches('\\') == dir.trim_end_matches('\\'))
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let installed = false;
    CliStatus { available: true, installed, path: bin.display().to_string() }
}

pub fn install() -> Result<String, String> {
    let bin = bundled().ok_or("This build of Blaze IDE does not include the blaze command. Download it from blazeapi.org/ide.")?;
    #[cfg(target_os = "macos")]
    {
        use std::path::Path;
        let link = Path::new(LINK);
        if std::fs::read_link(link).ok().is_some_and(|t| t == bin) {
            return Ok("The blaze command is already installed. Open a terminal and run: blaze".into());
        }
        let plain = (|| -> std::io::Result<()> {
            std::fs::create_dir_all("/usr/local/bin")?;
            if link.symlink_metadata().is_ok() {
                std::fs::remove_file(link)?;
            }
            std::os::unix::fs::symlink(&bin, link)
        })();
        if plain.is_err() {
            // /usr/local/bin is root-owned on a fresh Mac: ask for the password.
            // The path goes in as an argument, never into the script text.
            let out = std::process::Command::new("/usr/bin/osascript")
                .args([
                    "-e",
                    "on run argv",
                    "-e",
                    "do shell script \"mkdir -p /usr/local/bin && ln -sf \" & quoted form of (item 1 of argv) & \" /usr/local/bin/blaze\" with administrator privileges",
                    "-e",
                    "end run",
                ])
                .arg(&bin)
                .output()
                .map_err(|e| format!("Could not ask for permission: {e}"))?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                return Err(if err.contains("-128") { "Cancelled.".into() } else { format!("Could not install the blaze command: {}", err.trim()) });
            }
        }
        Ok("Installed. Open a new terminal and run: blaze".into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let dir = bin.parent().ok_or("No install folder")?.to_string_lossy().into_owned();
        // The folder is passed in an environment variable, not spliced into the script.
        let script = "$d=$env:BLAZE_DIR; $p=[Environment]::GetEnvironmentVariable('Path','User'); if (-not $p) { $p='' }; \
                      if (($p -split ';' | ForEach-Object { $_.TrimEnd('\\') }) -notcontains $d.TrimEnd('\\')) { \
                      [Environment]::SetEnvironmentVariable('Path', (($p.TrimEnd(';') + ';' + $d).TrimStart(';')), 'User') }";
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("BLAZE_DIR", &dir)
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| format!("Could not update PATH: {e}"))?;
        if !out.status.success() {
            return Err(format!("Could not update PATH: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok("Installed. Open a NEW terminal (PowerShell or cmd) and run: blaze".into())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Ok(format!("Add this to your PATH: {}", bin.display()))
    }
}
