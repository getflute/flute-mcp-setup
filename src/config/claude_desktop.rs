//! `claude_desktop_config.json` → `mcpServers`.

use std::path::Path;

use serde_json::{Value, json};

use super::{
    EntryReport, EntrySpec, EntryStatus, WriteReport, backup, find_duplicates, json_entry,
    prod_writes_warning, read_optional,
};
use crate::catalog::PROD_WRITES_VAR;

pub fn upsert<'a>(path: &Path, specs: &'a [EntrySpec], stamp: &str) -> WriteReport<'a> {
    try_upsert(path, specs, stamp).unwrap_or_else(WriteReport::failed)
}

fn try_upsert<'a>(
    path: &Path,
    specs: &'a [EntrySpec],
    stamp: &str,
) -> Result<WriteReport<'a>, String> {
    let shown = path.display();
    let mut root: Value = match read_optional(path)? {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .map_err(|e| format!("{shown} is not valid JSON ({e}); it was left unchanged"))?,
        _ => json!({}),
    };
    let servers = root
        .as_object_mut()
        .ok_or_else(|| format!("{shown} does not contain a JSON object; it was left unchanged"))?
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            format!("`mcpServers` in {shown} is not an object; it was left unchanged")
        })?;

    let mut report = WriteReport::default();
    for spec in specs {
        let existing = servers.get(&spec.name);
        let (entry, removed_args) = json_entry::merge(existing, spec)
            .map_err(|e| format!("{e} in {shown}; it was left unchanged"))?;
        let status = match existing {
            None => EntryStatus::Added,
            Some(old) if old.as_object() == Some(&entry) => EntryStatus::Unchanged,
            Some(_) => EntryStatus::Updated,
        };
        let prod = entry["env"].get(PROD_WRITES_VAR).and_then(Value::as_str);
        report.warnings.extend(prod_writes_warning(spec, prod));
        servers.insert(spec.name.clone(), Value::Object(entry));
        report.entries.push(EntryReport {
            spec,
            status,
            removed_args,
        });
    }
    report.duplicates = find_duplicates(
        servers
            .iter()
            .map(|(name, v)| (name.as_str(), v.get("command").and_then(Value::as_str))),
        specs,
    );

    if report.changed() {
        report.backup =
            backup::backup(path, stamp).map_err(|e| format!("could not back up {shown}: {e}"))?;
        let mut text = serde_json::to_string_pretty(&root).expect("a JSON value always serializes");
        text.push('\n');
        backup::write_atomic(path, &text).map_err(|e| format!("could not write {shown}: {e}"))?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_specs;

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn creates_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Claude/claude_desktop_config.json");
        let report = upsert(&path, test_specs::flute_cli("/b"), "s");
        assert_eq!(report.error, None);
        assert_eq!(report.backup, None);
        assert!(
            report
                .entries
                .iter()
                .all(|e| e.status == EntryStatus::Added)
        );
        let specs = test_specs::flute_cli("/b");
        assert_eq!(report.entries[0].spec, &specs[0]);
        assert_eq!(report.entries[1].spec, &specs[1]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("}\n"));
        assert_eq!(
            read(&path),
            json!({"mcpServers": {
                "flute-sandbox": {"command": "/b/flute-cli-mcp", "env": {"FLUTE_PROFILE": "sandbox", "FLUTE_BIN": "/b/flute"}},
                "flute-prod-readonly": {"command": "/b/flute-cli-mcp", "env": {"FLUTE_PROFILE": "production", "FLUTE_BIN": "/b/flute"}}
            }})
        );
    }

    #[test]
    fn empty_file_is_treated_as_empty_object() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        std::fs::write(&path, "  \n").unwrap();
        assert_eq!(upsert(&path, test_specs::flute_cli("/b"), "s").error, None);
        assert!(read(&path)["mcpServers"]["flute-sandbox"].is_object());
    }

    #[test]
    fn keeps_other_servers_and_top_level_keys_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        std::fs::write(
            &path,
            r#"{"preferences": {"x": 1}, "mcpServers": {"zeta": {"command": "/z"}, "alpha": {"command": "/a"}}}"#,
        )
        .unwrap();
        let report = upsert(&path, test_specs::flute_cli("/b"), "s");
        assert_eq!(report.error, None);
        let root = read(&path);
        let top: Vec<_> = root.as_object().unwrap().keys().cloned().collect();
        assert_eq!(top, ["preferences", "mcpServers"]);
        let names: Vec<_> = root["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            names,
            ["zeta", "alpha", "flute-sandbox", "flute-prod-readonly"]
        );
        assert_eq!(root["mcpServers"]["zeta"], json!({"command": "/z"}));
        assert!(report.backup.unwrap().ends_with("c.json.bak-s"));
    }

    #[test]
    fn updates_same_name_entry_and_reports_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        std::fs::write(
            &path,
            r#"{"mcpServers": {"flute-sandbox": {"command": "/gone/flute-cli-mcp", "env": {"FLUTE_PROFILE": "sandbox", "FLUTE_BIN": "/gone/flute", "FLUTE_MERCHANT_ID": "m1"}}}}"#,
        )
        .unwrap();
        let report = upsert(&path, &test_specs::flute_cli("/b")[..1], "s");
        assert_eq!(report.entries[0].status, EntryStatus::Updated);
        let entry = &read(&path)["mcpServers"]["flute-sandbox"];
        assert_eq!(entry["command"], "/b/flute-cli-mcp");
        assert_eq!(entry["env"]["FLUTE_MERCHANT_ID"], "m1");
    }

    #[test]
    fn unchanged_run_writes_nothing_and_makes_no_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        let specs = test_specs::flute_cli("/b");
        assert_eq!(upsert(&path, specs, "first").error, None);
        let before = std::fs::read_to_string(&path).unwrap();
        let report = upsert(&path, specs, "second");
        assert!(!report.changed());
        assert_eq!(report.backup, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(!tmp.path().join("c.json.bak-second").exists());
    }

    #[test]
    fn invalid_json_is_left_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        std::fs::write(&path, "{ not json").unwrap();
        let report = upsert(&path, test_specs::flute_cli("/b"), "s");
        assert!(report.error.unwrap().contains("is not valid JSON"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn wrong_shapes_are_left_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        for bad in [
            "[]",
            r#"{"mcpServers": []}"#,
            r#"{"mcpServers": {"flute-sandbox": {"env": "x"}}}"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            let report = upsert(&path, test_specs::flute_cli("/b"), "s");
            assert!(report.error.is_some(), "{bad}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        }
    }

    #[test]
    fn reports_duplicates_and_prod_write_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        std::fs::write(
            &path,
            r#"{"mcpServers": {
                "flute_cli_mcp": {"command": "/x/flute-cli-mcp"},
                "flute-prod-readonly": {"command": "/b/flute-cli-mcp", "env": {"FLUTE_MCP_ALLOW_PROD_WRITES": "1"}}
            }}"#,
        )
        .unwrap();
        let report = upsert(&path, test_specs::flute_cli("/b"), "s");
        assert_eq!(
            report.duplicates,
            [("flute_cli_mcp".to_string(), "flute-cli-mcp".to_string())]
        );
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(
            read(&path)["mcpServers"]["flute-prod-readonly"]["env"]["FLUTE_MCP_ALLOW_PROD_WRITES"],
            "1"
        );
    }
}
