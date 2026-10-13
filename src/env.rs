//! Everything the app reads from the user's environment, gathered in one place
//! so tests can point it at a temp directory instead of the real home.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }

    pub fn is_unix(self) -> bool {
        self != Os::Windows
    }
}

#[derive(Clone, Debug)]
pub struct Env {
    pub os: Os,
    /// `~` on Unix, `%USERPROFILE%` on Windows.
    pub home: PathBuf,
    /// The process `PATH`.
    pub path: Option<OsString>,
    /// `$SHELL`, falling back to a sensible default on Unix.
    pub shell: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub claude_config_dir: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub localappdata: Option<PathBuf>,
    /// Installer directories outside `home` (e.g. `/opt/homebrew/bin`).
    pub system_bin_dirs: Vec<PathBuf>,
    /// macOS application folders (`/Applications`, `~/Applications`).
    pub app_dirs: Vec<PathBuf>,
}

impl Env {
    pub fn from_system() -> Result<Env, String> {
        let os = Os::current();
        let home = dirs::home_dir().ok_or("could not determine your home directory")?;
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let shell = var("SHELL").map(PathBuf::from).or_else(|| match os {
            Os::MacOs => Some(PathBuf::from("/bin/zsh")),
            Os::Linux => Some(PathBuf::from("/bin/sh")),
            Os::Windows => None,
        });
        let system_bin_dirs = match os {
            Os::Windows => Vec::new(),
            _ => [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/home/linuxbrew/.linuxbrew/bin",
            ]
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        };
        let app_dirs = match os {
            Os::MacOs => vec![PathBuf::from("/Applications"), home.join("Applications")],
            _ => Vec::new(),
        };
        Ok(Env {
            os,
            home,
            path: var("PATH"),
            shell,
            codex_home: var("CODEX_HOME").map(PathBuf::from),
            claude_config_dir: var("CLAUDE_CONFIG_DIR").map(PathBuf::from),
            xdg_config_home: var("XDG_CONFIG_HOME").map(PathBuf::from),
            appdata: var("APPDATA").map(PathBuf::from),
            localappdata: var("LOCALAPPDATA").map(PathBuf::from),
            system_bin_dirs,
            app_dirs,
        })
    }

    /// A hermetic environment rooted at `home`: no PATH, no shell, no system
    /// dirs; the only application folder is `home/Applications`.
    pub fn for_tests(os: Os, home: &Path) -> Env {
        Env {
            os,
            home: home.to_path_buf(),
            path: None,
            shell: None,
            codex_home: None,
            claude_config_dir: None,
            xdg_config_home: None,
            appdata: None,
            localappdata: None,
            system_bin_dirs: Vec::new(),
            app_dirs: vec![home.join("Applications")],
        }
    }

    /// `flute` → `flute.exe` on Windows, unchanged elsewhere.
    pub fn exe_name(&self, name: &str) -> String {
        match self.os {
            Os::Windows => format!("{name}.exe"),
            _ => name.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_name_adds_extension_only_on_windows() {
        let home = Path::new("/h");
        assert_eq!(
            Env::for_tests(Os::Windows, home).exe_name("flute"),
            "flute.exe"
        );
        assert_eq!(Env::for_tests(Os::MacOs, home).exe_name("flute"), "flute");
        assert_eq!(Env::for_tests(Os::Linux, home).exe_name("flute"), "flute");
    }

    #[test]
    fn from_system_finds_a_home_directory() {
        let env = Env::from_system().unwrap();
        assert!(env.home.is_absolute());
        assert_eq!(env.os, Os::current());
    }
}
