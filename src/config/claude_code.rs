//! Claude Code user-scope servers, written through the `claude` CLI so we never
//! race Claude Code's own writes to `~/.claude.json`.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{
    EntryReport, EntrySpec, EntryStatus, WriteReport, backup, find_duplicates, json_entry,
    prod_writes_warning, read_optional,
};
use crate::catalog::PROD_WRITES_VAR;
use crate::process::CommandRunner;

const TIMEOUT: Duration = Duration::from_secs(30);

/// `claude` is the resolved CLI; `config_file` is only read (for existing
/// entries) and backed up.
pub fn upsert<'a>(
    claude: &Path,
    config_file: &Path,
    specs: &'a [EntrySpec],
    stamp: &str,
    runner: &dyn CommandRunner,
) -> WriteReport<'a> {
    let mut report = WriteReport::default();
    let existing = read_user_servers(config_file, &mut report.warnings);

    let mut planned = Vec::new();
    for spec in specs {
        let old = existing.get(&spec.name);
        // New entries start as {"type","command","args"} so keys read in a natural order.
        let base = json!({"type": "stdio", "command": "", "args": []});
        let (mut map, removed_args) = match json_entry::merge(old.or(Some(&base)), spec) {
            Ok(merged) => merged,
            Err(e) => return WriteReport::failed(format!("{e} in {}", config_file.display())),
        };
        map.insert("type".to_string(), json!("stdio"));
        map.entry("args").or_insert_with(|| json!([]));
        let entry = Value::Object(map);
        let status = match old {
            None => EntryStatus::Added,
            Some(o) if *o == entry => EntryStatus::Unchanged,
            Some(_) => EntryStatus::Updated,
        };
        let prod = entry["env"].get(PROD_WRITES_VAR).and_then(Value::as_str);
        report.warnings.extend(prod_writes_warning(spec, prod));
        report.entries.push(EntryReport {
            spec,
            status,
            removed_args,
        });
        if status != EntryStatus::Unchanged {
            planned.push((spec.name.clone(), entry, old.cloned()));
        }
    }
    report.duplicates = find_duplicates(
        existing
            .iter()
            .map(|(n, v)| (n.as_str(), v.get("command").and_then(Value::as_str))),
        specs,
    );
    if planned.is_empty() {
        return report;
    }

    if is_batch_shim(claude) {
        mark_not_applied(&mut report, &planned);
        report.error = Some(manual_instructions(claude, &planned));
        return report;
    }
    match backup::backup(config_file, stamp) {
        Ok(path) => report.backup = path,
        Err(e) => {
            report.error = Some(format!("could not back up {}: {e}", config_file.display()));
            return report;
        }
    }

    for (index, (name, entry, old)) in planned.iter().enumerate() {
        let remove = remove_args(name);
        report.commands.push(display_command(claude, &remove));
        let _ = runner.run(claude, &remove, TIMEOUT); // usually fails with "not found"; that's fine

        let add = add_args(name, entry);
        report.commands.push(display_command(claude, &add));
        if let Err(message) = run_checked(runner, claude, &add) {
            let restore = match old {
                None => String::new(),
                Some(old) => match run_checked(runner, claude, &add_args(name, old)) {
                    Ok(()) => format!("; the previous \"{name}\" entry was restored"),
                    Err(_) => format!(
                        "; the previous \"{name}\" entry could not be restored — see the backup"
                    ),
                },
            };
            mark_not_applied(&mut report, &planned[index..]);
            report.error = Some(format!(
                "`claude mcp add-json` for \"{name}\" failed: {message}{restore}"
            ));
            return report;
        }
    }
    report
}

/// Planned changes that were never made must not be reported as added/updated.
fn mark_not_applied(report: &mut WriteReport, planned: &[(String, Value, Option<Value>)]) {
    for entry in &mut report.entries {
        if planned.iter().any(|(name, _, _)| *name == entry.spec.name) {
            entry.status = EntryStatus::NotApplied;
        }
    }
}

fn read_user_servers(config_file: &Path, warnings: &mut Vec<String>) -> Map<String, Value> {
    let parsed = read_optional(config_file).and_then(|text| match text {
        None => Ok(Value::Null),
        Some(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
    });
    match parsed {
        Ok(root) => root
            .get("mcpServers")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        Err(e) => {
            warnings.push(format!(
                "could not read existing Claude Code servers from {} ({e}); treating them as absent",
                config_file.display()
            ));
            Map::new()
        }
    }
}

fn remove_args(name: &str) -> Vec<String> {
    ["mcp", "remove", "--scope", "user", name]
        .map(str::to_string)
        .to_vec()
}

fn add_args(name: &str, entry: &Value) -> Vec<String> {
    let json = serde_json::to_string(entry).expect("a JSON value always serializes");
    vec![
        "mcp".into(),
        "add-json".into(),
        "--scope".into(),
        "user".into(),
        name.into(),
        json,
    ]
}

fn run_checked(runner: &dyn CommandRunner, claude: &Path, args: &[String]) -> Result<(), String> {
    match runner.run(claude, args, TIMEOUT) {
        Ok(out) if out.success => Ok(()),
        Ok(out) => {
            let code = out.code.map_or("signal".to_string(), |c| c.to_string());
            let detail = [out.stderr.trim(), out.stdout.trim()]
                .into_iter()
                .find(|s| !s.is_empty());
            Err(format!(
                "exit {code}{}",
                detail.map(|d| format!(": {d}")).unwrap_or_default()
            ))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// npm installs `claude` on Windows as a `.cmd` shim; Rust refuses to pass our
/// JSON argument to batch files safely, so the user runs those commands instead.
pub fn is_batch_shim(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// Shell-style rendering for the log: arguments containing spaces or quotes are single-quoted.
fn display_command(program: &Path, args: &[String]) -> String {
    let quote = |a: &str| {
        if a.is_empty() || a.contains([' ', '"', '\'', '{']) {
            format!("'{}'", a.replace('\'', r"'\''"))
        } else {
            a.to_string()
        }
    };
    std::iter::once(program.display().to_string())
        .chain(args.iter().map(|a| quote(a)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn manual_instructions(claude: &Path, planned: &[(String, Value, Option<Value>)]) -> String {
    let mut text = format!(
        "{} is a batch-file shim that this app cannot run safely. Run these in Command Prompt (cmd.exe):",
        claude.display()
    );
    for (name, entry, _) in planned {
        let json = serde_json::to_string(entry).expect("a JSON value always serializes");
        // Writing to a String cannot fail.
        let _ = write!(text, "\n      claude mcp remove --scope user {name}");
        let _ = write!(
            text,
            "\n      claude mcp add-json --scope user {name} \"{}\"",
            json.replace('"', "\\\"")
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_specs;
    use crate::process::RunError;
    use crate::testutil::{FakeRunner, fail, ok};

    const CLAUDE: &str = "/u/.local/bin/claude";

    fn run_with(
        config: Option<&str>,
        runner: &FakeRunner,
    ) -> (WriteReport<'static>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".claude.json");
        if let Some(text) = config {
            std::fs::write(&file, text).unwrap();
        }
        let report = upsert(
            Path::new(CLAUDE),
            &file,
            test_specs::flute_cli("/b"),
            "s",
            runner,
        );
        (report, tmp)
    }

    #[test]
    fn adds_new_entries_via_cli() {
        let runner = FakeRunner::new(|_, _| Ok(ok("")));
        let (report, _tmp) = run_with(None, &runner);
        assert_eq!(report.error, None);
        assert_eq!(report.backup, None);
        let calls = runner.calls();
        assert_eq!(calls.len(), 4);
        assert_eq!(
            calls[0],
            [CLAUDE, "mcp", "remove", "--scope", "user", "flute-sandbox"]
        );
        assert_eq!(
            calls[1],
            [
                CLAUDE,
                "mcp",
                "add-json",
                "--scope",
                "user",
                "flute-sandbox",
                r#"{"type":"stdio","command":"/b/flute-cli-mcp","args":[],"env":{"FLUTE_PROFILE":"sandbox","FLUTE_BIN":"/b/flute"}}"#,
            ]
        );
        assert!(
            report.commands[1]
                .starts_with("/u/.local/bin/claude mcp add-json --scope user flute-sandbox '{")
        );
    }

    /// `claude mcp remove` exits 1 when the entry doesn't exist yet — the
    /// normal first run — and that must not stop the add.
    #[test]
    fn remove_reporting_not_found_does_not_block_the_add() {
        let runner = FakeRunner::new(|_, args| {
            Ok(if args[1] == "remove" {
                fail(1, "No MCP server named flute-sandbox found in user scope")
            } else {
                ok("")
            })
        });
        let (report, _tmp) = run_with(None, &runner);
        assert_eq!(report.error, None);
        let adds = runner.calls().iter().filter(|c| c[2] == "add-json").count();
        assert_eq!(adds, 2);
    }

    #[test]
    fn entries_not_applied_are_reported_as_such() {
        // The second add fails: the first entry was applied, the second was not.
        let runner = FakeRunner::new(|_, args| {
            let second_add = args[1] == "add-json" && args[4] == "flute-prod-readonly";
            Ok(if second_add { fail(1, "boom") } else { ok("") })
        });
        let (report, _tmp) = run_with(None, &runner);
        assert!(report.error.is_some());
        let statuses: Vec<_> = report.entries.iter().map(|e| e.status).collect();
        assert_eq!(statuses, [EntryStatus::Added, EntryStatus::NotApplied]);
    }

    #[test]
    fn unchanged_entries_run_nothing() {
        let config = r#"{"mcpServers": {
            "flute-sandbox": {"type": "stdio", "command": "/b/flute-cli-mcp", "args": [], "env": {"FLUTE_PROFILE": "sandbox", "FLUTE_BIN": "/b/flute"}},
            "flute-prod-readonly": {"type": "stdio", "command": "/b/flute-cli-mcp", "args": [], "env": {"FLUTE_PROFILE": "production", "FLUTE_BIN": "/b/flute"}}
        }}"#;
        let runner = FakeRunner::new(|_, _| Ok(ok("")));
        let (report, tmp) = run_with(Some(config), &runner);
        assert!(!report.changed());
        assert_eq!(runner.calls(), Vec::<Vec<String>>::new());
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            1,
            "no backup"
        );
    }

    #[test]
    fn updates_broken_entry_keeping_unowned_keys_and_backs_up() {
        let config = r#"{"mcpServers": {"flute-sandbox": {"type": "stdio", "command": "/gone/flute-cli-mcp", "args": [], "env": {"FLUTE_PROFILE": "sandbox", "FLUTE_BIN": "/gone/flute", "FLUTE_MCP_DEBUG": "1"}}}}"#;
        let runner = FakeRunner::new(|_, _| Ok(ok("")));
        let (report, tmp) = run_with(Some(config), &runner);
        assert_eq!(report.entries[0].status, EntryStatus::Updated);
        assert_eq!(report.entries[1].status, EntryStatus::Added);
        assert!(report.backup.unwrap().starts_with(tmp.path()));
        let add = &runner.calls()[1];
        assert!(add[6].contains(r#""FLUTE_MCP_DEBUG":"1""#), "{}", add[6]);
        assert!(add[6].contains(r#""command":"/b/flute-cli-mcp""#));
    }

    #[test]
    fn add_failure_restores_previous_entry_and_reports() {
        let config = r#"{"mcpServers": {"flute-sandbox": {"type": "stdio", "command": "/gone/flute-cli-mcp", "args": [], "env": {}}}}"#;
        let runner = FakeRunner::new(|_, args| {
            let is_new_add = args[1] == "add-json" && args[5].contains("/b/flute-cli-mcp");
            Ok(if is_new_add { fail(1, "boom") } else { ok("") })
        });
        let (report, _tmp) = run_with(Some(config), &runner);
        let error = report.error.unwrap();
        assert!(error.contains("exit 1: boom"), "{error}");
        assert!(
            error.contains("previous \"flute-sandbox\" entry was restored"),
            "{error}"
        );
        let restore = &runner.calls()[2];
        assert!(restore[6].contains("/gone/flute-cli-mcp"));
    }

    #[test]
    fn spawn_failure_is_an_error() {
        let runner = FakeRunner::new(|_, args| {
            if args[1] == "add-json" {
                Err(RunError::Timeout)
            } else {
                Ok(ok(""))
            }
        });
        let (report, _tmp) = run_with(None, &runner);
        assert!(report.error.unwrap().contains("timed out"));
    }

    #[test]
    fn unreadable_config_is_a_warning_not_an_error() {
        let runner = FakeRunner::new(|_, _| Ok(ok("")));
        let (report, _tmp) = run_with(Some("{ nope"), &runner);
        assert_eq!(report.error, None);
        assert!(report.warnings[0].contains("treating them as absent"));
        assert!(
            report
                .entries
                .iter()
                .all(|e| e.status == EntryStatus::Added)
        );
    }

    #[test]
    fn windows_cmd_shim_gets_manual_instructions() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new(|_, _| Ok(ok("")));
        let report = upsert(
            Path::new(r"C:\Users\me\AppData\Roaming\npm\claude.CMD"),
            &tmp.path().join(".claude.json"),
            test_specs::flute_cli("/b"),
            "s",
            &runner,
        );
        assert_eq!(runner.calls(), Vec::<Vec<String>>::new());
        assert!(
            report
                .entries
                .iter()
                .all(|e| e.status == EntryStatus::NotApplied)
        );
        let error = report.error.unwrap();
        assert!(error.contains("Command Prompt"));
        assert!(
            error
                .contains(r#"claude mcp add-json --scope user flute-sandbox "{\"type\":\"stdio\""#),
            "{error}"
        );
    }

    #[test]
    fn reports_duplicates_from_existing_servers() {
        let config = r#"{"mcpServers": {"flute_cli_mcp": {"command": "/x/flute-cli-mcp"}}}"#;
        let runner = FakeRunner::new(|_, _| Ok(ok("")));
        let (report, _tmp) = run_with(Some(config), &runner);
        assert_eq!(
            report.duplicates,
            [("flute_cli_mcp".to_string(), "flute-cli-mcp".to_string())]
        );
    }
}
