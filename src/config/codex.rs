//! `~/.codex/config.toml` → `[mcp_servers.<name>]`, preserving comments and layout.

use std::path::Path;

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, TableLike, value};

use super::{
    EntryReport, EntrySpec, EntryStatus, WriteReport, backup, find_duplicates, prod_writes_warning,
    read_optional, strip_override_args,
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
    let original = read_optional(path)?.unwrap_or_default();
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("{shown} is not valid TOML ({e}); it was left unchanged"))?;
    let servers = doc
        .entry("mcp_servers")
        .or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        })
        .as_table_mut()
        .ok_or_else(|| {
            format!("`mcp_servers` in {shown} is not a [mcp_servers] table; it was left unchanged")
        })?;

    let mut report = WriteReport::default();
    for spec in specs {
        let (status, removed_args, prod) = match servers.get_mut(&spec.name) {
            None => {
                servers.insert(&spec.name, Item::Table(new_entry(spec)));
                (EntryStatus::Added, Vec::new(), None)
            }
            Some(item) => {
                let entry = item.as_table_like_mut().ok_or_else(|| {
                    format!(
                        "mcp_servers.{} in {shown} is not a table; it was left unchanged",
                        spec.name
                    )
                })?;
                let (changed, removed) = apply_owned(entry, spec)
                    .map_err(|e| format!("{e} in {shown}; it was left unchanged"))?;
                let prod = entry
                    .get("env")
                    .and_then(Item::as_table_like)
                    .and_then(|env| env.get(PROD_WRITES_VAR))
                    .and_then(Item::as_str)
                    .map(str::to_string);
                let status = if changed {
                    EntryStatus::Updated
                } else {
                    EntryStatus::Unchanged
                };
                (status, removed, prod)
            }
        };
        report
            .warnings
            .extend(prod_writes_warning(spec, prod.as_deref()));
        report.entries.push(EntryReport {
            spec,
            status,
            removed_args,
        });
    }
    report.duplicates = find_duplicates(
        servers.iter().map(|(name, item)| {
            (
                name,
                item.as_table_like()
                    .and_then(|t| t.get("command"))
                    .and_then(Item::as_str),
            )
        }),
        specs,
    );

    if report.changed() {
        report.backup =
            backup::backup(path, stamp).map_err(|e| format!("could not back up {shown}: {e}"))?;
        backup::write_atomic(path, &doc.to_string())
            .map_err(|e| format!("could not write {shown}: {e}"))?;
    }
    Ok(report)
}

/// `command = "..."` plus an inline `env = { ... }`, matching the README.
fn new_entry(spec: &EntrySpec) -> Table {
    let mut env = InlineTable::new();
    for (key, val) in spec.owned_env() {
        env.insert(key, val.into());
    }
    let mut table = Table::new();
    table.insert("command", value(spec.command.as_str()));
    table.insert("env", value(env));
    table
}

/// Set owned fields in place. Returns whether anything changed and the removed args.
fn apply_owned(entry: &mut dyn TableLike, spec: &EntrySpec) -> Result<(bool, Vec<String>), String> {
    let name = &spec.name;
    let mut changed = false;
    if entry.get("command").and_then(Item::as_str) != Some(spec.command.as_str()) {
        entry.insert("command", value(spec.command.as_str()));
        changed = true;
    }

    let mut removed = Vec::new();
    if let Some(args) = entry.get_mut("args") {
        let array = args
            .as_array_mut()
            .ok_or_else(|| format!("mcp_servers.{name} has `args` that is not a list"))?;
        let current: Vec<String> = array
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or_else(|| format!("mcp_servers.{name} has `args` that are not all strings"))?;
        let (kept, stripped) = strip_override_args(&current);
        if !stripped.is_empty() {
            *array = kept.iter().map(String::as_str).collect::<Array>();
            removed = stripped;
            changed = true;
        }
    }

    if entry.get("env").is_none() {
        entry.insert("env", value(InlineTable::new()));
    }
    let env = entry
        .get_mut("env")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| format!("mcp_servers.{name} has `env` that is not a table"))?;
    for (key, val) in spec.owned_env() {
        if env.get(key).and_then(Item::as_str) != Some(val) {
            env.insert(key, value(val));
            changed = true;
        }
    }
    Ok((changed, removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_specs;

    fn write(dir: &Path, text: &str) -> std::path::PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn creates_missing_file_in_readme_style() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".codex/config.toml");
        let report = upsert(&path, test_specs::flute_cli("/b"), "s");
        assert_eq!(report.error, None);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[mcp_servers.flute-sandbox]\n\
             command = \"/b/flute-cli-mcp\"\n\
             env = { FLUTE_PROFILE = \"sandbox\", FLUTE_BIN = \"/b/flute\" }\n\
             \n\
             [mcp_servers.flute-prod-readonly]\n\
             command = \"/b/flute-cli-mcp\"\n\
             env = { FLUTE_PROFILE = \"production\", FLUTE_BIN = \"/b/flute\" }\n"
        );
    }

    #[test]
    fn preserves_comments_and_other_sections_byte_for_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let original = "# my codex settings\nmodel = \"gpt-5\"  # favourite\n\n[mcp_servers.node_repl]\n# keep me\ncommand = \"/x/node\"\n";
        let path = write(tmp.path(), original);
        let report = upsert(&path, test_specs::flute_cli("/b"), "s");
        assert_eq!(report.error, None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(original),
            "existing content changed:\n{text}"
        );
        assert!(text.contains("[mcp_servers.flute-sandbox]"));
    }

    #[test]
    fn updates_env_sub_table_in_place_and_keeps_tools_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "[mcp_servers.flute-sandbox]\n\
             command = \"/gone/flute-cli-mcp\"\n\
             \n\
             [mcp_servers.flute-sandbox.env]\n\
             FLUTE_PROFILE = \"sandbox\"\n\
             FLUTE_BIN = \"/gone/flute\"\n\
             FLUTE_MERCHANT_ID = \"m1\"\n\
             \n\
             [mcp_servers.flute-sandbox.tools.ping]\n\
             approval_mode = \"approve\"\n",
        );
        let report = upsert(&path, &test_specs::flute_cli("/b")[..1], "s");
        assert_eq!(report.entries[0].status, EntryStatus::Updated);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[mcp_servers.flute-sandbox]\n\
             command = \"/b/flute-cli-mcp\"\n\
             \n\
             [mcp_servers.flute-sandbox.env]\n\
             FLUTE_PROFILE = \"sandbox\"\n\
             FLUTE_BIN = \"/b/flute\"\n\
             FLUTE_MERCHANT_ID = \"m1\"\n\
             \n\
             [mcp_servers.flute-sandbox.tools.ping]\n\
             approval_mode = \"approve\"\n"
        );
    }

    #[test]
    fn updates_inline_env_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "[mcp_servers.flute-sandbox]\ncommand = \"/b/flute-cli-mcp\"\nenv = { FLUTE_PROFILE = \"production\", FLUTE_BIN = \"/b/flute\" }\n",
        );
        assert_eq!(
            upsert(&path, &test_specs::flute_cli("/b")[..1], "s").error,
            None
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[mcp_servers.flute-sandbox]\ncommand = \"/b/flute-cli-mcp\"\nenv = { FLUTE_PROFILE = \"sandbox\", FLUTE_BIN = \"/b/flute\" }\n"
        );
    }

    #[test]
    fn strips_override_args() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "[mcp_servers.flute-sandbox]\ncommand = \"/b/flute-cli-mcp\"\nargs = [\"--profile\", \"production\", \"--debug\"]\nenv = { FLUTE_PROFILE = \"sandbox\", FLUTE_BIN = \"/b/flute\" }\n",
        );
        let report = upsert(&path, &test_specs::flute_cli("/b")[..1], "s");
        assert_eq!(report.entries[0].removed_args, ["--profile", "production"]);
        let doc: DocumentMut = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        let args: Vec<_> = doc["mcp_servers"]["flute-sandbox"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(args, ["--debug"]);
    }

    #[test]
    fn unchanged_run_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let specs = test_specs::flute_cli("/b");
        assert_eq!(upsert(&path, specs, "first").error, None);
        let before = std::fs::read_to_string(&path).unwrap();
        let report = upsert(&path, specs, "second");
        assert!(!report.changed());
        assert_eq!(report.backup, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn invalid_toml_and_wrong_shapes_are_left_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in [
            "this is = = not toml",
            "mcp_servers = 3\n",
            "[mcp_servers]\nflute-sandbox = 1\n",
            "[mcp_servers.flute-sandbox]\nenv = \"x\"\n",
            "[mcp_servers.flute-sandbox]\nargs = \"--debug\"\n",
        ] {
            let path = write(tmp.path(), bad);
            let report = upsert(&path, test_specs::flute_cli("/b"), "s");
            assert!(report.error.is_some(), "{bad}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        }
    }

    #[test]
    fn reports_duplicates_by_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "[mcp_servers.flute_cli_mcp]\ncommand = \"/x/flute-cli-mcp\"\n\n[mcp_servers.webhooks_cli_mcp]\ncommand = \"/x/flute-webhooks-mcp\"\n",
        );
        let report = upsert(&path, test_specs::flute_cli("/b"), "s");
        assert_eq!(
            report.duplicates,
            [("flute_cli_mcp".to_string(), "flute-cli-mcp".to_string())]
        );
    }

    #[test]
    fn windows_paths_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let specs = test_specs::flute_cli(r"C:\Users\me\.cargo\bin");
        assert_eq!(upsert(&path, specs, "s").error, None);
        let doc: DocumentMut = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["flute-sandbox"]["command"].as_str(),
            Some(r"C:\Users\me\.cargo\bin/flute-cli-mcp")
        );
    }
}
