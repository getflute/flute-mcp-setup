//! One Configure run: detect agents, check binaries, write configs, report.
//! Everything is reported through `emit`, so the UI only renders events.

use std::path::PathBuf;

use chrono::NaiveDateTime;

use crate::catalog::{BinaryInfo, McpKind};
use crate::config::{EntrySpec, EntryStatus, WriteReport, claude_code, claude_desktop, codex};
use crate::detect::agents::{self, AgentKind, AgentStatus, Target, join_labels};
use crate::detect::binaries::{self, Resolver};
use crate::env::{Env, Os};
use crate::marks::{MISSING, OK, WARN};
use crate::process::{CommandRunner, WithPath};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Agents(Vec<AgentStatus>),
    Log(String),
    Done(Outcome),
}

#[must_use = "the outcome decides the banner and whether the run succeeded"]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every found agent is configured. `changed` is false when all were already up to date.
    Success {
        agents: Vec<AgentKind>,
        servers: usize,
        changed: bool,
    },
    Partial,
    Failed,
    /// A required binary or agent was missing; no file was touched.
    NothingChanged,
}

/// Startup detection: just the agent statuses.
pub fn detect_agents(env: &Env, runner: &dyn CommandRunner) -> Vec<AgentStatus> {
    agents::detect(env, &Resolver::new(env, runner))
}

struct Out<'a> {
    emit: &'a mut dyn FnMut(Event),
}

impl Out<'_> {
    fn line(&mut self, line: impl Into<String>) {
        (self.emit)(Event::Log(line.into()));
    }

    fn send(&mut self, event: Event) {
        (self.emit)(event);
    }
}

pub fn run(
    selected: &[McpKind],
    env: &Env,
    runner: &dyn CommandRunner,
    now: NaiveDateTime,
    emit: &mut dyn FnMut(Event),
) -> Outcome {
    let mut out = Out { emit };
    log_header(&mut out, selected, env.os, now);

    let resolver = Resolver::new(env, runner);
    // Children (version probes, `claude`) get the PATH we discovered, not the
    // app's own, which is minimal when it was launched from Finder.
    let with_path = WithPath {
        inner: runner,
        path: resolver.child_path(),
    };
    let runner: &dyn CommandRunner = &with_path;
    let agents = agents::detect(env, &resolver);
    out.send(Event::Agents(agents.clone()));
    log_agents(&mut out, &agents);

    let installed = check_mcps(&mut out, selected, env.os, &resolver, runner);
    let found: Vec<(AgentKind, &Target)> = agents
        .iter()
        .filter_map(|a| a.target.as_ref().map(|t| (a.kind, t)))
        .collect();
    if found.is_empty() {
        log_no_agents(&mut out);
    }
    let Some(installed) = installed.filter(|_| !found.is_empty()) else {
        out.line("");
        out.line("Nothing was changed.");
        return Outcome::NothingChanged;
    };

    let specs = entry_specs(&installed);
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let mut succeeded = Vec::new();
    let mut changed_agents = Vec::new();
    let mut failed = 0;
    for (kind, target) in found {
        out.line("");
        out.line(kind.label());
        let report = write_agent(target, &specs, &stamp, runner);
        log_report(&mut out, &report);
        if report.error.is_some() {
            failed += 1;
        } else {
            succeeded.push(kind);
            if report.changed() {
                changed_agents.push(kind);
            }
        }
    }

    out.line("");
    if failed > 0 {
        return if succeeded.is_empty() {
            Outcome::Failed
        } else {
            Outcome::Partial
        };
    }
    log_next_steps(&mut out, selected, &changed_agents);
    Outcome::Success {
        agents: succeeded,
        servers: specs.len(),
        changed: !changed_agents.is_empty(),
    }
}

fn log_header(out: &mut Out, selected: &[McpKind], os: Os, now: NaiveDateTime) {
    let os_name = match os {
        Os::MacOs => "macOS",
        Os::Windows => "Windows",
        Os::Linux => "Linux",
    };
    out.line(format!(
        "Flute MCP Setup — {} — {os_name} {}",
        now.format("%Y-%m-%d %H:%M:%S"),
        std::env::consts::ARCH
    ));
    let labels: Vec<_> = selected.iter().map(|m| m.label()).collect();
    out.line(format!("Selected: {}", labels.join(", ")));
}

fn log_agents(out: &mut Out, agents: &[AgentStatus]) {
    out.line("");
    out.line("AI apps");
    for agent in agents {
        let mark = if agent.found() { OK } else { MISSING };
        out.line(format!(
            "  {mark} {:<15} {}",
            agent.kind.label(),
            agent.detail
        ));
    }
}

fn log_no_agents(out: &mut Out) {
    out.line("");
    out.line(format!(
        "{MISSING} No supported AI app was found. Install one of:"
    ));
    for kind in AgentKind::ALL {
        out.line(format!("    {}: {}", kind.label(), kind.download_url()));
    }
}

/// Check each selected MCP's CLI and MCP binary, logging every result.
/// `None` when any binary is missing.
fn check_mcps(
    out: &mut Out,
    selected: &[McpKind],
    os: Os,
    resolver: &Resolver,
    runner: &dyn CommandRunner,
) -> Option<Vec<Resolved>> {
    out.line("");
    out.line("Binaries");
    if let Some(note) = resolver.login_note() {
        out.line(format!("  (login shell PATH unavailable: {note})"));
    }
    let mut installed = Vec::new();
    let mut missing = false;
    for &mcp in selected {
        let cli = check_binary(out, mcp.cli(), os, resolver, runner);
        let server = check_binary(out, mcp.server(), os, resolver, runner);
        match (cli, server) {
            (Some(cli), Some(server)) => installed.push(Resolved { mcp, cli, server }),
            _ => missing = true,
        }
    }
    (!missing).then_some(installed)
}

fn write_agent<'a>(
    target: &Target,
    specs: &'a [EntrySpec],
    stamp: &str,
    runner: &dyn CommandRunner,
) -> WriteReport<'a> {
    match target {
        Target::ClaudeDesktop { config } => claude_desktop::upsert(config, specs, stamp),
        Target::ChatGpt { config } => codex::upsert(config, specs, stamp),
        Target::ClaudeCode { cli, config } => {
            claude_code::upsert(cli, config, specs, stamp, runner)
        }
    }
}

fn log_next_steps(out: &mut Out, selected: &[McpKind], changed_agents: &[AgentKind]) {
    if changed_agents.is_empty() {
        out.line("Already up to date — no files were changed.");
        return;
    }
    out.line("Next steps");
    out.line(format!(
        "  1. Restart {} to load the new servers.",
        join_labels(changed_agents)
    ));
    out.line("  2. Sign in to each CLI once per profile:");
    for mcp in selected {
        for cmd in mcp.auth_commands() {
            out.line(format!("       {cmd}"));
        }
    }
}

fn log_missing(out: &mut Out, bin: &BinaryInfo, os: Os, resolver: &Resolver) {
    out.line(format!("  {MISSING} {:<18} not found", bin.name));
    out.line(format!(
        "      Searched: {}",
        resolver.searched().join(", ")
    ));
    for (label, cmd) in bin.install_instructions(os) {
        out.line(format!("      Install ({label}): {cmd}"));
    }
    out.line(format!("      More info: {}", bin.repo_url()));
}

/// A selected MCP whose CLI and MCP binary were both found.
struct Resolved {
    mcp: McpKind,
    cli: PathBuf,
    server: PathBuf,
}

/// Resolve `bin`, logging where it is (with its version) or how to install it.
fn check_binary(
    out: &mut Out,
    bin: &BinaryInfo,
    os: Os,
    resolver: &Resolver,
    runner: &dyn CommandRunner,
) -> Option<PathBuf> {
    let Some(path) = resolver.resolve(bin.name) else {
        log_missing(out, bin, os, resolver);
        return None;
    };
    let version = binaries::probe_version(runner, &path);
    out.line(format!(
        "  {OK} {:<18} {} ({version})",
        bin.name,
        path.display()
    ));
    Some(path)
}

fn entry_specs(resolved: &[Resolved]) -> Vec<EntrySpec> {
    resolved
        .iter()
        .flat_map(|r| {
            r.mcp.entries().iter().map(move |t| EntrySpec {
                name: t.name.to_string(),
                command: r.server.to_string_lossy().into_owned(),
                profile: t.profile.to_string(),
                cli_env_var: r.mcp.cli_env_var().to_string(),
                cli_path: r.cli.to_string_lossy().into_owned(),
                readonly: t.readonly,
            })
        })
        .collect()
}

fn log_report(out: &mut Out, report: &WriteReport) {
    if let Some(backup) = &report.backup {
        out.line(format!("  Backup: {}", backup.display()));
    }
    for cmd in &report.commands {
        out.line(format!("  Ran: {cmd}"));
    }
    for entry in &report.entries {
        let spec = entry.spec;
        if entry.status == EntryStatus::NotApplied {
            out.line(format!(
                "  {MISSING} {} {}",
                spec.name,
                entry.status.label()
            ));
            continue;
        }
        out.line(format!("  {OK} {} {}", spec.name, entry.status.label()));
        out.line(format!("      command = {}", spec.command));
        let env: Vec<_> = spec
            .owned_env()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        out.line(format!("      env: {}", env.join(", ")));
        if !entry.removed_args.is_empty() {
            out.line(format!(
                "      removed args that override env: {}",
                entry.removed_args.join(" ")
            ));
        }
    }
    for warning in &report.warnings {
        out.line(format!("  {WARN} {warning}"));
    }
    for (name, stem) in &report.duplicates {
        out.line(format!(
            "  {WARN} \"{name}\" also launches {stem}; it was left unchanged — remove it if it is a duplicate"
        ));
    }
    if let Some(error) = &report.error {
        out.line(format!("  {MISSING} {error}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeRunner, ok, touch_exe};
    use std::path::Path;

    fn now() -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 13)
            .unwrap()
            .and_hms_opt(10, 15, 0)
            .unwrap()
    }

    /// Linux env rooted at a temp home, no login shell, binaries in ~/.cargo/bin.
    fn env(home: &Path, bins: &[&str]) -> Env {
        for bin in bins {
            touch_exe(&home.join(".cargo/bin").join(bin));
        }
        Env::for_tests(Os::Linux, home)
    }

    fn version_runner() -> FakeRunner {
        FakeRunner::new(|_, args| {
            Ok(ok(if args == ["--version"] {
                "tool 1.0.0\n"
            } else {
                ""
            }))
        })
    }

    fn run_capture(selected: &[McpKind], env: &Env, runner: &FakeRunner) -> (Outcome, Vec<Event>) {
        let mut events = Vec::new();
        let outcome = run(selected, env, runner, now(), &mut |e| events.push(e));
        (outcome, events)
    }

    fn log_text(events: &[Event]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Log(line) => Some(line.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    const ALL_BINS: [&str; 4] = [
        "flute",
        "flute-cli-mcp",
        "flute-webhooks",
        "flute-webhooks-mcp",
    ];

    #[test]
    fn entry_specs_use_each_mcps_own_binaries() {
        let resolved = [
            Resolved {
                mcp: McpKind::Webhooks,
                cli: PathBuf::from("/w/flute-webhooks"),
                server: PathBuf::from("/w/flute-webhooks-mcp"),
            },
            Resolved {
                mcp: McpKind::FluteCli,
                cli: PathBuf::from("/f/flute"),
                server: PathBuf::from("/f/flute-cli-mcp"),
            },
        ];
        let specs = entry_specs(&resolved);
        let summary: Vec<_> = specs
            .iter()
            .map(|s| (s.name.as_str(), s.command.as_str(), s.cli_path.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "flute-webhooks-sandbox",
                    "/w/flute-webhooks-mcp",
                    "/w/flute-webhooks"
                ),
                (
                    "flute-webhooks-prod",
                    "/w/flute-webhooks-mcp",
                    "/w/flute-webhooks"
                ),
                ("flute-sandbox", "/f/flute-cli-mcp", "/f/flute"),
                ("flute-prod-readonly", "/f/flute-cli-mcp", "/f/flute"),
            ]
        );
    }

    #[test]
    fn missing_binary_changes_nothing_and_explains_how_to_install() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &["flute-webhooks"]);
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        let (outcome, events) = run_capture(&[McpKind::Webhooks], &env, &version_runner());
        assert_eq!(outcome, Outcome::NothingChanged);
        let log = log_text(&events);
        assert!(log.contains("✖ flute-webhooks-mcp"), "{log}");
        assert!(log.contains(
            "Install (Homebrew): brew install getflute/flute-webhooks-mcp/flute-webhooks-mcp"
        ));
        assert!(log.contains("More info: https://github.com/getflute/flute-webhooks-mcp"));
        assert!(log.contains("✔ flute-webhooks"));
        assert!(log.ends_with("Nothing was changed."));
        assert!(!tmp.path().join(".codex/config.toml").exists());
    }

    #[test]
    fn no_agent_changes_nothing_and_lists_downloads() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &ALL_BINS);
        let (outcome, events) = run_capture(&[McpKind::FluteCli], &env, &version_runner());
        assert_eq!(outcome, Outcome::NothingChanged);
        let log = log_text(&events);
        assert!(log.contains("No supported AI app was found"));
        assert!(log.contains("https://claude.com/download"));
    }

    #[test]
    fn agents_event_is_sent_before_any_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &ALL_BINS);
        let (_, events) = run_capture(&[McpKind::FluteCli], &env, &version_runner());
        assert!(matches!(&events[2], Event::Agents(a) if a.len() == 3));
    }

    #[test]
    fn success_writes_every_found_agent_then_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &ALL_BINS);
        std::fs::create_dir_all(tmp.path().join(".config/Claude")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        let selected = [McpKind::Webhooks, McpKind::FluteCli];

        let (outcome, events) = run_capture(&selected, &env, &version_runner());
        assert_eq!(
            outcome,
            Outcome::Success {
                agents: vec![AgentKind::ClaudeDesktop, AgentKind::ChatGpt],
                servers: 4,
                changed: true
            }
        );
        let log = log_text(&events);
        assert!(log.contains("(tool 1.0.0)"), "{log}");
        assert!(log.contains("✔ flute-webhooks-sandbox added"));
        assert!(log.contains("flute-webhooks auth login"));
        assert!(log.contains("flute --profile production auth login"));
        let codex: toml_edit::DocumentMut =
            std::fs::read_to_string(tmp.path().join(".codex/config.toml"))
                .unwrap()
                .parse()
                .unwrap();
        // Compare as paths: TOML escapes Windows backslashes in the file text.
        let webhooks_bin = codex["mcp_servers"]["flute-webhooks-prod"]["env"]["FLUTE_WEBHOOKS_BIN"]
            .as_str()
            .map(PathBuf::from);
        assert_eq!(
            webhooks_bin,
            Some(tmp.path().join(".cargo").join("bin").join("flute-webhooks"))
        );
        let desktop =
            std::fs::read_to_string(tmp.path().join(".config/Claude/claude_desktop_config.json"))
                .unwrap();
        assert!(desktop.contains("flute-prod-readonly"));

        let (outcome, events) = run_capture(&selected, &env, &version_runner());
        assert_eq!(
            outcome,
            Outcome::Success {
                agents: vec![AgentKind::ClaudeDesktop, AgentKind::ChatGpt],
                servers: 4,
                changed: false
            }
        );
        assert!(log_text(&events).contains("Already up to date"));
    }

    #[test]
    fn next_steps_name_the_changed_agents() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &ALL_BINS);
        std::fs::create_dir_all(tmp.path().join(".config/Claude")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        let (_, events) = run_capture(&[McpKind::FluteCli], &env, &version_runner());
        assert!(
            log_text(&events)
                .contains("  1. Restart Claude Desktop and ChatGPT to load the new servers.")
        );
    }

    #[test]
    fn one_failing_agent_is_partial_and_others_still_write() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &ALL_BINS);
        let desktop = tmp.path().join(".config/Claude/claude_desktop_config.json");
        std::fs::create_dir_all(desktop.parent().unwrap()).unwrap();
        std::fs::write(&desktop, "{ broken").unwrap();
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        let (outcome, events) = run_capture(&[McpKind::FluteCli], &env, &version_runner());
        assert_eq!(outcome, Outcome::Partial);
        assert_eq!(std::fs::read_to_string(&desktop).unwrap(), "{ broken");
        assert!(tmp.path().join(".codex/config.toml").exists());
        assert!(log_text(&events).contains("is not valid JSON"));
    }

    #[test]
    fn every_agent_failing_is_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &ALL_BINS);
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        std::fs::write(tmp.path().join(".codex/config.toml"), "= nope").unwrap();
        let (outcome, _) = run_capture(&[McpKind::FluteCli], &env, &version_runner());
        assert_eq!(outcome, Outcome::Failed);
    }

    #[test]
    fn claude_code_is_configured_through_the_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &["flute", "flute-cli-mcp", "claude"]);
        let runner = version_runner();
        let (outcome, events) = run_capture(&[McpKind::FluteCli], &env, &runner);
        assert_eq!(
            outcome,
            Outcome::Success {
                agents: vec![AgentKind::ClaudeCode],
                servers: 2,
                changed: true
            }
        );
        let adds = runner
            .calls()
            .into_iter()
            .filter(|c| c.get(2).is_some_and(|a| a == "add-json"))
            .count();
        assert_eq!(adds, 2);
        assert!(log_text(&events).contains("Ran: "));
    }

    #[test]
    fn entries_that_failed_to_apply_are_not_logged_as_success() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &["flute", "flute-cli-mcp", "claude"]);
        let runner = FakeRunner::new(|_, args| {
            Ok(match args.get(1).map(String::as_str) {
                Some("add-json") => crate::testutil::fail(1, "boom"),
                _ => ok("tool 1.0.0\n"),
            })
        });
        let (outcome, events) = run_capture(&[McpKind::FluteCli], &env, &runner);
        assert_eq!(outcome, Outcome::Failed);
        let log = log_text(&events);
        assert!(log.contains("✖ flute-sandbox not applied"), "{log}");
        assert!(!log.contains("✔ flute-sandbox"), "{log}");
    }

    /// Launched from Finder the app's PATH is minimal; `claude` (often a
    /// `#!/usr/bin/env node` script) must run with the PATH we discovered.
    #[test]
    fn commands_run_with_the_discovered_path() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &["flute", "flute-cli-mcp", "claude"]);
        let runner = version_runner();
        let (outcome, _) = run_capture(&[McpKind::FluteCli], &env, &runner);
        assert!(matches!(outcome, Outcome::Success { .. }), "{outcome:?}");
        let cargo_bin = tmp.path().join(".cargo").join("bin");
        let paths = runner.paths();
        // Two --version probes, then `claude mcp remove` + `add-json` per entry.
        assert_eq!(paths.len(), 6, "{paths:?}");
        let has_cargo_bin = |p: &str| std::env::split_paths(p).any(|dir| dir == cargo_bin);
        assert!(
            paths
                .iter()
                .all(|p| p.as_deref().is_some_and(has_cargo_bin)),
            "{paths:?}"
        );
    }

    #[test]
    fn duplicate_and_prod_write_warnings_reach_the_log() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(tmp.path(), &ALL_BINS);
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        std::fs::write(
            tmp.path().join(".codex/config.toml"),
            "[mcp_servers.flute_cli_mcp]\ncommand = \"/x/flute-cli-mcp\"\n\n[mcp_servers.flute-prod-readonly]\ncommand = \"/x\"\nenv = { FLUTE_MCP_ALLOW_PROD_WRITES = \"1\" }\n",
        )
        .unwrap();
        let (_, events) = run_capture(&[McpKind::FluteCli], &env, &version_runner());
        let log = log_text(&events);
        assert!(
            log.contains("⚠ \"flute_cli_mcp\" also launches flute-cli-mcp"),
            "{log}"
        );
        assert!(log.contains("⚠ flute-prod-readonly has FLUTE_MCP_ALLOW_PROD_WRITES=1"));
    }
}
