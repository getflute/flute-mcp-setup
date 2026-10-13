//! Applying owned fields to a JSON server entry (Claude Desktop and Claude Code).

use serde_json::{Map, Value, json};

use super::{EntrySpec, strip_override_args};

/// Returns the merged entry (always a JSON object) and any `args` removed.
/// `existing` keeps every key this app does not own, in its original order.
pub fn merge(
    existing: Option<&Value>,
    spec: &EntrySpec,
) -> Result<(Map<String, Value>, Vec<String>), String> {
    let name = &spec.name;
    let mut entry = match existing {
        None => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Err(format!("server \"{name}\" is not a JSON object")),
    };
    entry.insert("command".to_string(), json!(spec.command));

    let mut removed = Vec::new();
    if let Some(args) = entry.get("args") {
        let args: Vec<String> = args
            .as_array()
            .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect())
            .ok_or_else(|| format!("server \"{name}\" has `args` that is not a list of strings"))?;
        let (kept, stripped) = strip_override_args(&args);
        if !stripped.is_empty() {
            entry.insert("args".to_string(), json!(kept));
            removed = stripped;
        }
    }

    let env = entry
        .entry("env")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("server \"{name}\" has `env` that is not a JSON object"))?;
    for (key, value) in spec.owned_env() {
        env.insert(key.to_string(), json!(value));
    }
    Ok((entry, removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_specs;

    #[test]
    fn new_entry_has_command_then_env() {
        let spec = &test_specs::flute_cli("/b")[0];
        let (entry, removed): (Map<String, Value>, _) = merge(None, spec).unwrap();
        assert_eq!(
            serde_json::to_string(&entry).unwrap(),
            r#"{"command":"/b/flute-cli-mcp","env":{"FLUTE_PROFILE":"sandbox","FLUTE_BIN":"/b/flute"}}"#
        );
        assert_eq!(removed, Vec::<String>::new());
    }

    #[test]
    fn existing_entry_keeps_unowned_keys_and_order() {
        let spec = &test_specs::flute_cli("/b")[0];
        let existing = json!({
            "env": {"FLUTE_MERCHANT_ID": "m1", "FLUTE_PROFILE": "production"},
            "command": "/old/flute-cli-mcp",
            "disabled": false
        });
        let (entry, _) = merge(Some(&existing), spec).unwrap();
        assert_eq!(
            serde_json::to_string(&entry).unwrap(),
            r#"{"env":{"FLUTE_MERCHANT_ID":"m1","FLUTE_PROFILE":"sandbox","FLUTE_BIN":"/b/flute"},"command":"/b/flute-cli-mcp","disabled":false}"#
        );
    }

    #[test]
    fn override_args_are_removed_and_reported() {
        let spec = &test_specs::flute_cli("/b")[0];
        let existing = json!({"command": "x", "args": ["--profile", "production", "--debug"]});
        let (entry, removed) = merge(Some(&existing), spec).unwrap();
        assert_eq!(entry["args"], json!(["--debug"]));
        assert_eq!(removed, ["--profile", "production"]);
    }

    #[test]
    fn bad_shapes_are_errors() {
        let spec = &test_specs::flute_cli("/b")[0];
        assert!(
            merge(Some(&json!("nope")), spec)
                .unwrap_err()
                .contains("not a JSON object")
        );
        assert!(
            merge(Some(&json!({"env": []})), spec)
                .unwrap_err()
                .contains("`env`")
        );
        assert!(
            merge(Some(&json!({"args": [1]})), spec)
                .unwrap_err()
                .contains("`args`")
        );
    }
}
