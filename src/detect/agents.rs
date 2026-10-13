//! Detect Claude Desktop, Claude Code and ChatGPT (Codex) and locate their
//! MCP configuration.

use std::path::{Path, PathBuf};

use crate::detect::binaries::Resolver;
use crate::env::{Env, Os};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    ClaudeDesktop,
    ClaudeCode,
    ChatGpt,
}

impl AgentKind {
    pub const ALL: [AgentKind; 3] = [
        AgentKind::ClaudeDesktop,
        AgentKind::ClaudeCode,
        AgentKind::ChatGpt,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AgentKind::ClaudeDesktop => "Claude Desktop",
            AgentKind::ClaudeCode => "Claude Code",
            AgentKind::ChatGpt => "ChatGPT",
        }
    }

    pub fn download_url(self) -> &'static str {
        match self {
            AgentKind::ClaudeDesktop => "https://claude.com/download",
            AgentKind::ClaudeCode => "https://code.claude.com/docs/en/overview",
            AgentKind::ChatGpt => "https://chatgpt.com/download",
        }
    }
}

/// Where a found agent's MCP config lives and how it is written. Each variant
/// carries exactly what its writer needs, so a found Claude Code always has
/// its `claude` executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// `claude_desktop_config.json`, edited directly.
    ClaudeDesktop { config: PathBuf },
    /// Written through `cli` (`claude mcp …`); `config` is only read and backed up.
    ClaudeCode { cli: PathBuf, config: PathBuf },
    /// `config.toml` in the Codex home, edited directly.
    ChatGpt { config: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStatus {
    pub kind: AgentKind,
    /// `Some` exactly when the agent was found.
    pub target: Option<Target>,
    /// Shown in the log and as the status-row tooltip.
    pub detail: String,
}

impl AgentStatus {
    pub fn found(&self) -> bool {
        self.target.is_some()
    }
}

/// "A", "A and B", "A, B and C".
pub fn join_labels(kinds: &[AgentKind]) -> String {
    let labels: Vec<_> = kinds.iter().map(|k| k.label()).collect();
    match labels.split_last() {
        None => String::new(),
        Some((last, [])) => last.to_string(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// All three agents, in display/write order.
pub fn detect(env: &Env, resolver: &Resolver) -> Vec<AgentStatus> {
    vec![
        claude_desktop(env),
        claude_code(env, resolver),
        chatgpt(env, resolver),
    ]
}

fn appdata(env: &Env) -> PathBuf {
    env.appdata
        .clone()
        .unwrap_or_else(|| env.home.join("AppData").join("Roaming"))
}

fn localappdata(env: &Env) -> PathBuf {
    env.localappdata
        .clone()
        .unwrap_or_else(|| env.home.join("AppData").join("Local"))
}

/// `%LOCALAPPDATA%\Packages\Claude_*\LocalCache\Roaming\Claude` if it exists (MSIX install).
fn msix_claude_dir(env: &Env) -> Option<PathBuf> {
    let packages = std::fs::read_dir(localappdata(env).join("Packages")).ok()?;
    packages
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
        .map(|e| e.path().join("LocalCache").join("Roaming").join("Claude"))
        .find(|p| p.is_dir())
}

fn app_bundle(env: &Env, name: &str) -> Option<PathBuf> {
    env.app_dirs
        .iter()
        .map(|d| d.join(name))
        .find(|p| p.is_dir())
}

fn found(kind: AgentKind, target: Target, detail: String) -> AgentStatus {
    AgentStatus {
        kind,
        target: Some(target),
        detail,
    }
}

fn missing(kind: AgentKind, detail: String) -> AgentStatus {
    AgentStatus {
        kind,
        target: None,
        detail,
    }
}

fn claude_desktop(env: &Env) -> AgentStatus {
    const FILE: &str = "claude_desktop_config.json";
    let kind = AgentKind::ClaudeDesktop;
    let (config_dir, install_signal) = match env.os {
        Os::MacOs => (
            env.home
                .join("Library")
                .join("Application Support")
                .join("Claude"),
            app_bundle(env, "Claude.app"),
        ),
        Os::Windows => {
            let install = localappdata(env).join("AnthropicClaude");
            let dir = msix_claude_dir(env).unwrap_or_else(|| appdata(env).join("Claude"));
            (dir, install.is_dir().then_some(install))
        }
        Os::Linux => {
            let xdg = env
                .xdg_config_home
                .clone()
                .unwrap_or_else(|| env.home.join(".config"));
            (xdg.join("Claude"), None)
        }
    };
    if install_signal.is_some() || config_dir.is_dir() {
        let config = config_dir.join(FILE);
        let detail = config.display().to_string();
        found(kind, Target::ClaudeDesktop { config }, detail)
    } else {
        let detail = format!("not found (no Claude app and no {})", config_dir.display());
        missing(kind, detail)
    }
}

pub fn claude_code_config(env: &Env) -> PathBuf {
    env.claude_config_dir
        .clone()
        .unwrap_or_else(|| env.home.clone())
        .join(".claude.json")
}

fn claude_code(env: &Env, resolver: &Resolver) -> AgentStatus {
    let kind = AgentKind::ClaudeCode;
    match resolver.resolve("claude") {
        Some(cli) => {
            let detail = cli.display().to_string();
            let config = claude_code_config(env);
            found(kind, Target::ClaudeCode { cli, config }, detail)
        }
        None => missing(kind, "not found (no `claude` command)".to_string()),
    }
}

pub fn codex_home(env: &Env) -> PathBuf {
    env.codex_home
        .clone()
        .unwrap_or_else(|| env.home.join(".codex"))
}

fn chatgpt(env: &Env, resolver: &Resolver) -> AgentStatus {
    let kind = AgentKind::ChatGpt;
    let home = codex_home(env);
    let chatgpt_app = app_bundle(env, "ChatGPT.app");
    let bundles_codex = |app: &Path| {
        app.join("Contents")
            .join("Resources")
            .join("codex-cli")
            .exists()
    };
    let has_codex = home.is_dir()
        || resolver.resolve("codex").is_some()
        || app_bundle(env, "Codex.app").is_some()
        || chatgpt_app.as_deref().is_some_and(bundles_codex);
    if has_codex {
        let config = home.join("config.toml");
        let detail = config.display().to_string();
        found(kind, Target::ChatGpt { config }, detail)
    } else if chatgpt_app.is_some() {
        let detail =
            "ChatGPT app found but it does not include Codex — update ChatGPT to use MCP servers"
                .to_string();
        missing(kind, detail)
    } else {
        let detail = format!(
            "not found (no ChatGPT/Codex app, no `codex` command, no {})",
            home.display()
        );
        missing(kind, detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::touch_exe;

    fn run(env: &Env) -> Vec<AgentStatus> {
        detect(env, &Resolver::with_login_path(env, None))
    }

    fn mkdir(p: &Path) {
        std::fs::create_dir_all(p).unwrap();
    }

    #[test]
    fn join_labels_reads_naturally() {
        assert_eq!(join_labels(&[]), "");
        assert_eq!(join_labels(&[AgentKind::ChatGpt]), "ChatGPT");
        assert_eq!(
            join_labels(&[AgentKind::ClaudeCode, AgentKind::ChatGpt]),
            "Claude Code and ChatGPT"
        );
        assert_eq!(
            join_labels(&AgentKind::ALL),
            "Claude Desktop, Claude Code and ChatGPT"
        );
    }

    #[test]
    fn nothing_installed_finds_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let found: Vec<bool> = run(&Env::for_tests(Os::MacOs, tmp.path()))
            .iter()
            .map(AgentStatus::found)
            .collect();
        assert_eq!(found, [false, false, false]);
    }

    #[test]
    fn order_is_desktop_code_chatgpt() {
        let tmp = tempfile::tempdir().unwrap();
        let kinds: Vec<_> = run(&Env::for_tests(Os::MacOs, tmp.path()))
            .iter()
            .map(|a| a.kind)
            .collect();
        assert_eq!(kinds, AgentKind::ALL);
    }

    #[test]
    fn mac_claude_app_bundle_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        mkdir(&tmp.path().join("Applications/Claude.app"));
        let desktop = &run(&Env::for_tests(Os::MacOs, tmp.path()))[0];
        assert_eq!(
            desktop.target,
            Some(Target::ClaudeDesktop {
                config: tmp
                    .path()
                    .join("Library/Application Support/Claude/claude_desktop_config.json")
            })
        );
    }

    #[test]
    fn mac_claude_config_dir_alone_is_enough() {
        let tmp = tempfile::tempdir().unwrap();
        mkdir(&tmp.path().join("Library/Application Support/Claude"));
        assert!(run(&Env::for_tests(Os::MacOs, tmp.path()))[0].found());
    }

    #[test]
    fn windows_prefers_msix_config_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let msix = tmp
            .path()
            .join("AppData/Local/Packages/Claude_pzs8sxrjxfjjc/LocalCache/Roaming/Claude");
        mkdir(&msix);
        mkdir(&tmp.path().join("AppData/Roaming/Claude"));
        let desktop = &run(&Env::for_tests(Os::Windows, tmp.path()))[0];
        assert_eq!(
            desktop.target,
            Some(Target::ClaudeDesktop {
                config: msix.join("claude_desktop_config.json")
            })
        );
    }

    #[test]
    fn windows_classic_install_uses_roaming_appdata() {
        let tmp = tempfile::tempdir().unwrap();
        mkdir(&tmp.path().join("AppData/Local/AnthropicClaude"));
        let desktop = &run(&Env::for_tests(Os::Windows, tmp.path()))[0];
        assert_eq!(
            desktop.target,
            Some(Target::ClaudeDesktop {
                config: tmp
                    .path()
                    .join("AppData/Roaming/Claude/claude_desktop_config.json")
            })
        );
    }

    #[test]
    fn linux_honours_xdg_config_home() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = Env::for_tests(Os::Linux, tmp.path());
        env.xdg_config_home = Some(tmp.path().join("xdg"));
        mkdir(&tmp.path().join("xdg/Claude"));
        let desktop = &run(&env)[0];
        assert_eq!(
            desktop.target,
            Some(Target::ClaudeDesktop {
                config: tmp.path().join("xdg/Claude/claude_desktop_config.json")
            })
        );
    }

    #[test]
    fn claude_code_is_found_by_its_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let claude = touch_exe(&tmp.path().join(".local/bin/claude"));
        let mut env = Env::for_tests(Os::MacOs, tmp.path());
        env.claude_config_dir = Some(tmp.path().join("cc"));
        let code = &run(&env)[1];
        assert_eq!(
            code.target,
            Some(Target::ClaudeCode {
                cli: claude,
                config: tmp.path().join("cc/.claude.json")
            })
        );
    }

    #[test]
    fn claude_code_config_defaults_to_home() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            claude_code_config(&Env::for_tests(Os::Linux, tmp.path())),
            tmp.path().join(".claude.json")
        );
    }

    #[test]
    fn chatgpt_found_via_codex_home_and_honours_override() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = Env::for_tests(Os::Linux, tmp.path());
        env.codex_home = Some(tmp.path().join("my-codex"));
        mkdir(&tmp.path().join("my-codex"));
        let chatgpt = &run(&env)[2];
        assert_eq!(
            chatgpt.target,
            Some(Target::ChatGpt {
                config: tmp.path().join("my-codex/config.toml")
            })
        );
    }

    #[test]
    fn chatgpt_found_via_codex_binary() {
        let tmp = tempfile::tempdir().unwrap();
        touch_exe(&tmp.path().join(".local/bin/codex"));
        let chatgpt = &run(&Env::for_tests(Os::Linux, tmp.path()))[2];
        assert_eq!(
            chatgpt.target,
            Some(Target::ChatGpt {
                config: tmp.path().join(".codex/config.toml")
            })
        );
    }

    #[test]
    fn chatgpt_app_with_bundled_codex_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        mkdir(
            &tmp.path()
                .join("Applications/ChatGPT.app/Contents/Resources/codex-cli"),
        );
        assert!(run(&Env::for_tests(Os::MacOs, tmp.path()))[2].found());
    }

    #[test]
    fn chatgpt_app_without_codex_explains_why() {
        let tmp = tempfile::tempdir().unwrap();
        mkdir(
            &tmp.path()
                .join("Applications/ChatGPT.app/Contents/Resources"),
        );
        let chatgpt = &run(&Env::for_tests(Os::MacOs, tmp.path()))[2];
        assert!(!chatgpt.found());
        assert!(chatgpt.detail.contains("does not include Codex"));
    }
}
