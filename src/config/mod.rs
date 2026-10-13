//! Writing server entries into each agent's MCP configuration.

pub mod backup;
pub mod claude_code;
pub mod claude_desktop;
pub mod codex;
pub mod json_entry;

use std::path::{Path, PathBuf};

use crate::catalog::{PROD_WRITES_VAR, PROFILE_VAR};

/// One server entry to write, with every path already resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntrySpec {
    pub name: String,
    /// Absolute path of the MCP binary.
    pub command: String,
    pub profile: String,
    /// `FLUTE_BIN` or `FLUTE_WEBHOOKS_BIN`.
    pub cli_env_var: String,
    /// Absolute path of the CLI.
    pub cli_path: String,
    pub readonly: bool,
}

impl EntrySpec {
    /// The env vars this app owns on the entry, in write order.
    pub fn owned_env(&self) -> [(&str, &str); 2] {
        [
            (PROFILE_VAR, &self.profile),
            (&self.cli_env_var, &self.cli_path),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryStatus {
    Added,
    Updated,
    Unchanged,
    /// A change was planned but never made (the agent failed first).
    NotApplied,
}

impl EntryStatus {
    pub fn label(self) -> &'static str {
        match self {
            EntryStatus::Added => "added",
            EntryStatus::Updated => "updated",
            EntryStatus::Unchanged => "unchanged",
            EntryStatus::NotApplied => "not applied",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryReport<'a> {
    /// The entry this report is about.
    pub spec: &'a EntrySpec,
    pub status: EntryStatus,
    /// `--binary`/`--profile` args removed because they override the env vars.
    pub removed_args: Vec<String>,
}

/// What a writer did. `error` set means this agent failed; files it had not
/// yet written are untouched. Failures live in a field rather than a
/// `Result`, so dropping a report would silently ignore one.
#[must_use = "a WriteReport carries the writer's error; check `error`"]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteReport<'a> {
    pub entries: Vec<EntryReport<'a>>,
    pub backup: Option<PathBuf>,
    /// `(entry name, binary stem)` of other entries launching a selected MCP binary.
    pub duplicates: Vec<(String, String)>,
    pub warnings: Vec<String>,
    /// Commands run (Claude Code only), as shown in the log.
    pub commands: Vec<String>,
    pub error: Option<String>,
}

impl WriteReport<'_> {
    pub fn failed(message: String) -> Self {
        WriteReport {
            error: Some(message),
            ..WriteReport::default()
        }
    }

    pub fn changed(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.status != EntryStatus::Unchanged)
    }
}

/// Read a file that may not exist yet.
pub fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

/// Lower-cased file name of `command` without a trailing `.exe`.
pub fn command_stem(command: &str) -> String {
    let name = command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .to_ascii_lowercase();
    match name.strip_suffix(".exe") {
        Some(stem) => stem.to_string(),
        None => name,
    }
}

/// Split `args` into `(kept, removed)`, removing `--binary`/`--profile` and their values.
pub fn strip_override_args(args: &[String]) -> (Vec<String>, Vec<String>) {
    let (mut kept, mut removed) = (Vec::new(), Vec::new());
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--binary" || arg == "--profile" {
            removed.push(arg.clone());
            removed.extend(iter.next().cloned());
        } else if arg.starts_with("--binary=") || arg.starts_with("--profile=") {
            removed.push(arg.clone());
        } else {
            kept.push(arg.clone());
        }
    }
    (kept, removed)
}

pub fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Warning when a read-only entry has production writes switched on.
pub fn prod_writes_warning(spec: &EntrySpec, value: Option<&str>) -> Option<String> {
    let value = value.filter(|v| spec.readonly && is_truthy(v))?;
    Some(format!(
        "{} has {PROD_WRITES_VAR}={value} — production writes are enabled on this entry",
        spec.name
    ))
}

/// Entries not in `specs` whose command is one of the specs' MCP binaries.
pub fn find_duplicates<'a>(
    servers: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    specs: &[EntrySpec],
) -> Vec<(String, String)> {
    let stems: Vec<String> = specs.iter().map(|s| command_stem(&s.command)).collect();
    servers
        .filter(|(name, _)| !specs.iter().any(|s| s.name == *name))
        .filter_map(|(name, command)| {
            let stem = command_stem(command?);
            stems.contains(&stem).then(|| (name.to_string(), stem))
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod test_specs {
    use super::EntrySpec;

    /// The two Flute CLI entries. Leaked so reports, which borrow their specs,
    /// can outlive the call in tests (test-only; the process exits soon after).
    pub fn flute_cli(bin_dir: &str) -> &'static [EntrySpec] {
        let spec = |name: &str, profile: &str, readonly| EntrySpec {
            name: name.to_string(),
            command: format!("{bin_dir}/flute-cli-mcp"),
            profile: profile.to_string(),
            cli_env_var: "FLUTE_BIN".to_string(),
            cli_path: format!("{bin_dir}/flute"),
            readonly,
        };
        let specs = vec![
            spec("flute-sandbox", "sandbox", false),
            spec("flute-prod-readonly", "production", true),
        ];
        Box::leak(specs.into_boxed_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn command_stem_strips_dirs_and_exe() {
        assert_eq!(
            command_stem("/opt/homebrew/bin/flute-cli-mcp"),
            "flute-cli-mcp"
        );
        assert_eq!(
            command_stem(r"C:\Users\me\.cargo\bin\Flute-CLI-MCP.EXE"),
            "flute-cli-mcp"
        );
        assert_eq!(command_stem("flute-webhooks-mcp"), "flute-webhooks-mcp");
    }

    #[test]
    fn strip_override_args_handles_both_forms() {
        let (kept, removed) = strip_override_args(&strings(&[
            "--verbose",
            "--profile",
            "production",
            "--binary=/x/flute",
            "--keep",
        ]));
        assert_eq!(kept, strings(&["--verbose", "--keep"]));
        assert_eq!(
            removed,
            strings(&["--profile", "production", "--binary=/x/flute"])
        );
    }

    #[test]
    fn strip_override_args_tolerates_trailing_flag() {
        let (kept, removed) = strip_override_args(&strings(&["--binary"]));
        assert_eq!(kept, Vec::<String>::new());
        assert_eq!(removed, strings(&["--binary"]));
    }

    #[test]
    fn truthy_values_match_the_readme() {
        for v in ["1", "true", "YES", " on "] {
            assert!(is_truthy(v), "{v}");
        }
        for v in ["0", "false", "", "off", "2"] {
            assert!(!is_truthy(v), "{v}");
        }
    }

    #[test]
    fn prod_writes_warning_only_for_readonly_entries() {
        let specs = test_specs::flute_cli("/b");
        assert_eq!(prod_writes_warning(&specs[0], Some("1")), None);
        assert_eq!(prod_writes_warning(&specs[1], Some("0")), None);
        assert_eq!(prod_writes_warning(&specs[1], None), None);
        let warning = prod_writes_warning(&specs[1], Some("1")).unwrap();
        assert!(warning.starts_with("flute-prod-readonly has FLUTE_MCP_ALLOW_PROD_WRITES=1"));
    }

    #[test]
    fn duplicates_are_other_names_launching_selected_binaries() {
        let specs = test_specs::flute_cli("/b");
        let servers = [
            ("flute-sandbox", Some("/old/flute-cli-mcp")),
            ("flute_cli_mcp", Some("/x/flute-cli-mcp")),
            ("webhooks_cli_mcp", Some("/x/flute-webhooks-mcp")),
            ("node_repl", Some("/x/node")),
            ("broken", None),
        ];
        assert_eq!(
            find_duplicates(servers.into_iter(), specs),
            vec![("flute_cli_mcp".to_string(), "flute-cli-mcp".to_string())]
        );
    }
}
