//! Resolve absolute paths for executables the way an MCP client can't:
//! process PATH, then the login shell's PATH, then known installer dirs.
//! Paths are returned as found, never canonicalized, so Homebrew and
//! `~/.local/bin` symlinks survive upgrades.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::env::{Env, Os};
use crate::process::CommandRunner;

pub const LOGIN_PATH_MARKER: &str = "__FLUTE_SETUP_PATH__";

#[derive(Clone, Debug)]
pub struct Resolver {
    os: Os,
    process_path: Option<OsString>,
    login_path: Option<OsString>,
    known_dirs: Vec<PathBuf>,
    login_note: Option<String>,
}

impl Resolver {
    /// Builds a resolver, asking the login shell for its PATH on macOS/Linux.
    pub fn new(env: &Env, runner: &dyn CommandRunner) -> Resolver {
        let (login_path, login_note) = if env.os.is_unix() {
            probe_login_path(env, runner)
        } else {
            (None, None)
        };
        Resolver {
            login_note,
            ..Resolver::with_login_path(env, login_path)
        }
    }

    /// Builds a resolver with a given login-shell PATH (no process spawned).
    pub fn with_login_path(env: &Env, login_path: Option<OsString>) -> Resolver {
        Resolver {
            os: env.os,
            process_path: env.path.clone(),
            login_path,
            known_dirs: known_dirs(env),
            login_note: None,
        }
    }

    /// Absolute path to `name` (no extension; `.exe`/`.cmd`/`.bat` are tried on Windows).
    pub fn resolve(&self, name: &str) -> Option<PathBuf> {
        let from_path = |path: &Option<OsString>| {
            path.as_deref()
                .and_then(|p| find_in(&absolute_dirs(p), name, self.os))
        };
        from_path(&self.process_path)
            .or_else(|| from_path(&self.login_path))
            .or_else(|| find_in(&self.known_dirs, name, self.os))
    }

    /// The `PATH` to give child processes: the login-shell PATH, then the
    /// process PATH, then the known installer dirs — absolute entries only,
    /// without repeats.
    pub fn child_path(&self) -> OsString {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let from_paths = [&self.login_path, &self.process_path]
            .into_iter()
            .flatten()
            .flat_map(|p| absolute_dirs(p));
        for dir in from_paths.chain(self.known_dirs.iter().cloned()) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        std::env::join_paths(&dirs)
            .unwrap_or_else(|_| self.process_path.clone().unwrap_or_default())
    }

    /// Human-readable list of where `resolve` looks, in order.
    pub fn searched(&self) -> Vec<String> {
        let mut places = vec!["PATH".to_string()];
        if self.os.is_unix() {
            places.push("login shell PATH".to_string());
        }
        places.extend(self.known_dirs.iter().map(|d| d.display().to_string()));
        places
    }

    /// Why the login-shell PATH is unavailable, if it is.
    pub fn login_note(&self) -> Option<&str> {
        self.login_note.as_deref()
    }
}

pub fn known_dirs(env: &Env) -> Vec<PathBuf> {
    let mut dirs = vec![
        env.home.join(".cargo").join("bin"),
        env.home.join(".local").join("bin"),
    ];
    dirs.extend(env.system_bin_dirs.iter().cloned());
    dirs
}

/// The absolute entries of a PATH-style string; relative entries are dropped.
pub fn absolute_dirs(path: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(path)
        .filter(|d| d.is_absolute())
        .collect()
}

fn candidates(name: &str, os: Os) -> Vec<String> {
    match os {
        Os::Windows => vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
        ],
        _ => vec![name.to_string()],
    }
}

fn find_in(dirs: &[PathBuf], name: &str, os: Os) -> Option<PathBuf> {
    let names = candidates(name, os);
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| is_executable_file(p))
}

/// A regular file (following symlinks) that, on Unix, has an execute bit.
pub fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Extract the PATH printed after `LOGIN_PATH_MARKER`, ignoring any rc-file noise.
pub fn parse_login_path(stdout: &str) -> Option<OsString> {
    stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(LOGIN_PATH_MARKER))
        .map(|p| p.trim_end_matches('\r'))
        .filter(|p| !p.is_empty())
        .map(OsString::from)
}

fn probe_login_path(env: &Env, runner: &dyn CommandRunner) -> (Option<OsString>, Option<String>) {
    let Some(shell) = &env.shell else {
        return (
            None,
            Some("no login shell available to ask for PATH".to_string()),
        );
    };
    let script = format!("printf '\\n{LOGIN_PATH_MARKER}%s\\n' \"$PATH\"");
    match runner.run(shell, &["-ilc".to_string(), script], Duration::from_secs(5)) {
        Ok(out) => match parse_login_path(&out.stdout) {
            Some(path) => (Some(path), None),
            None => (
                None,
                Some(format!(
                    "login shell {} did not report a PATH",
                    shell.display()
                )),
            ),
        },
        Err(e) => (None, Some(format!("login shell {}: {e}", shell.display()))),
    }
}

/// First non-empty line of `<path> --version`, or "version unknown".
pub fn probe_version(runner: &dyn CommandRunner, path: &Path) -> String {
    match runner.run(path, &["--version".to_string()], Duration::from_secs(5)) {
        Ok(out) if out.success => out
            .stdout
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map_or_else(|| "version unknown".to_string(), str::to_string),
        _ => "version unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::RunError;
    use crate::testutil::{FakeRunner, exe, ok, touch_exe};

    fn env_with_path(os: Os, home: &Path, dirs: &[&Path]) -> Env {
        let mut env = Env::for_tests(os, home);
        env.path = Some(std::env::join_paths(dirs).unwrap());
        env
    }

    #[test]
    fn finds_binary_on_process_path() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let flute = touch_exe(&bin.join(exe("flute")));
        let env = env_with_path(Os::current(), tmp.path(), &[&bin]);
        assert_eq!(
            Resolver::with_login_path(&env, None).resolve("flute"),
            Some(flute)
        );
    }

    #[test]
    fn falls_back_to_login_shell_path() {
        let tmp = tempfile::tempdir().unwrap();
        let shell_bin = tmp.path().join("shell-bin");
        let flute = touch_exe(&shell_bin.join(exe("flute")));
        let env = Env::for_tests(Os::current(), tmp.path());
        let resolver = Resolver::with_login_path(&env, Some(shell_bin.into_os_string()));
        assert_eq!(resolver.resolve("flute"), Some(flute));
    }

    #[test]
    fn falls_back_to_known_installer_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let flute = touch_exe(&tmp.path().join(".cargo/bin").join(exe("flute")));
        let env = Env::for_tests(Os::current(), tmp.path());
        assert_eq!(
            Resolver::with_login_path(&env, None).resolve("flute"),
            Some(flute)
        );
    }

    #[test]
    fn process_path_wins_over_known_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        touch_exe(&tmp.path().join(".cargo/bin").join(exe("flute")));
        let bin = tmp.path().join("bin");
        let first = touch_exe(&bin.join(exe("flute")));
        let env = env_with_path(Os::current(), tmp.path(), &[&bin]);
        assert_eq!(
            Resolver::with_login_path(&env, None).resolve("flute"),
            Some(first)
        );
    }

    #[test]
    fn missing_binary_resolves_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let env = Env::for_tests(Os::current(), tmp.path());
        assert_eq!(Resolver::with_login_path(&env, None).resolve("flute"), None);
    }

    #[test]
    fn directories_are_not_executables() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".cargo/bin/flute")).unwrap();
        let env = Env::for_tests(Os::current(), tmp.path());
        assert_eq!(Resolver::with_login_path(&env, None).resolve("flute"), None);
    }

    #[cfg(unix)]
    #[test]
    fn files_without_execute_bit_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".cargo/bin/flute");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        let env = Env::for_tests(Os::current(), tmp.path());
        assert_eq!(Resolver::with_login_path(&env, None).resolve("flute"), None);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_returned_without_canonicalizing() {
        let tmp = tempfile::tempdir().unwrap();
        let real = touch_exe(&tmp.path().join("Cellar/flute/1.4.2/bin/flute"));
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&real, bin.join("flute")).unwrap();
        let env = env_with_path(Os::current(), tmp.path(), &[&bin]);
        assert_eq!(
            Resolver::with_login_path(&env, None).resolve("flute"),
            Some(bin.join("flute"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn relative_path_entries_are_dropped() {
        let path =
            std::env::join_paths([Path::new("relative/bin"), Path::new("/abs/bin")]).unwrap();
        assert_eq!(absolute_dirs(&path), vec![PathBuf::from("/abs/bin")]);
    }

    #[test]
    fn windows_prefers_exe_over_cmd_shim() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join(".local/bin");
        let shim = touch_exe(&bin.join("claude.cmd"));
        let env = Env::for_tests(Os::Windows, tmp.path());
        assert_eq!(
            Resolver::with_login_path(&env, None).resolve("claude"),
            Some(shim)
        );
        let exe = touch_exe(&bin.join("claude.exe"));
        assert_eq!(
            Resolver::with_login_path(&env, None).resolve("claude"),
            Some(exe)
        );
    }

    #[cfg(unix)]
    #[test]
    fn child_path_is_login_then_process_then_known_dirs_without_repeats() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = Env::for_tests(Os::Linux, tmp.path());
        env.path = Some(OsString::from("/usr/bin:/bin:relative"));
        let resolver =
            Resolver::with_login_path(&env, Some(OsString::from("/opt/homebrew/bin:/usr/bin")));
        let dirs: Vec<PathBuf> = std::env::split_paths(&resolver.child_path()).collect();
        assert_eq!(
            dirs,
            [
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                tmp.path().join(".cargo").join("bin"),
                tmp.path().join(".local").join("bin"),
            ]
        );
    }

    #[test]
    fn parses_login_path_after_rc_noise() {
        let out = "Welcome to zsh!\nnvm: using node 22\n\n__FLUTE_SETUP_PATH__/opt/homebrew/bin:/usr/bin\n";
        assert_eq!(
            parse_login_path(out),
            Some(OsString::from("/opt/homebrew/bin:/usr/bin"))
        );
        assert_eq!(parse_login_path("no marker here"), None);
        assert_eq!(parse_login_path("__FLUTE_SETUP_PATH__\n"), None);
    }

    #[test]
    fn new_asks_the_login_shell_on_unix() {
        let tmp = tempfile::tempdir().unwrap();
        let shell_bin = tmp.path().join("shell-bin");
        let flute = touch_exe(&shell_bin.join("flute"));
        let mut env = Env::for_tests(Os::Linux, tmp.path());
        env.shell = Some(PathBuf::from("/bin/zsh"));
        let reported = format!("{LOGIN_PATH_MARKER}{}\n", shell_bin.display());
        let runner = FakeRunner::new(move |_, _| Ok(ok(&reported)));
        let resolver = Resolver::new(&env, &runner);
        assert_eq!(resolver.resolve("flute"), Some(flute));
        assert_eq!(resolver.login_note(), None);
        let calls = runner.calls();
        assert_eq!(calls[0][0], "/bin/zsh");
        assert_eq!(calls[0][1], "-ilc");
    }

    #[test]
    fn login_shell_failure_is_noted_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = Env::for_tests(Os::MacOs, tmp.path());
        env.shell = Some(PathBuf::from("/bin/zsh"));
        let runner = FakeRunner::new(|_, _| Err(RunError::Timeout));
        let resolver = Resolver::new(&env, &runner);
        assert_eq!(
            resolver.login_note(),
            Some("login shell /bin/zsh: timed out")
        );
        assert_eq!(resolver.resolve("flute"), None);
    }

    #[test]
    fn windows_does_not_spawn_a_login_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let env = Env::for_tests(Os::Windows, tmp.path());
        let runner = FakeRunner::new(|_, _| Ok(ok("")));
        Resolver::new(&env, &runner);
        assert_eq!(runner.calls(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn searched_lists_every_location_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = Env::for_tests(Os::MacOs, tmp.path());
        env.system_bin_dirs = vec![PathBuf::from("/opt/homebrew/bin")];
        let searched = Resolver::with_login_path(&env, None).searched();
        assert_eq!(searched[0], "PATH");
        assert_eq!(searched[1], "login shell PATH");
        assert_eq!(
            searched[2],
            tmp.path().join(".cargo").join("bin").display().to_string()
        );
        assert_eq!(
            searched[3],
            tmp.path().join(".local").join("bin").display().to_string()
        );
        assert_eq!(searched[4], "/opt/homebrew/bin");
    }

    #[test]
    fn version_probe_reports_first_line_or_unknown() {
        let runner = FakeRunner::new(|_, _| Ok(ok("\nflute 1.4.2\nprofile: sandbox\n")));
        assert_eq!(probe_version(&runner, Path::new("/x/flute")), "flute 1.4.2");
        assert_eq!(runner.calls()[0], ["/x/flute", "--version"]);
        let runner = FakeRunner::new(|_, _| Err(RunError::Timeout));
        assert_eq!(
            probe_version(&runner, Path::new("/x/flute")),
            "version unknown"
        );
    }
}
