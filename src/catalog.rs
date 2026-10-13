//! Static facts from the upstream READMEs: the two MCPs, their binaries,
//! the server entries to write, and how to install each binary.

use crate::env::Os;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum McpKind {
    Webhooks,
    FluteCli,
}

#[derive(Debug, PartialEq, Eq)]
pub struct BinaryInfo {
    pub name: &'static str,
    pub repo: &'static str,
    pub brew: &'static str,
    pub script_asset: &'static str,
    pub powershell_asset: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct EntryTemplate {
    pub name: &'static str,
    pub profile: &'static str,
    /// Production writes are expected to stay off for this entry.
    pub readonly: bool,
}

pub const FLUTE: BinaryInfo = BinaryInfo {
    name: "flute",
    repo: "flute-cli",
    brew: "brew install getflute/flute-cli/flute",
    script_asset: "flute-installer.sh",
    powershell_asset: "flute-installer.ps1",
};

pub const FLUTE_WEBHOOKS: BinaryInfo = BinaryInfo {
    name: "flute-webhooks",
    repo: "flute-webhooks-cli",
    brew: "brew install getflute/flute-webhooks-cli/flute-webhooks-cli",
    script_asset: "flute-webhooks-cli-installer.sh",
    powershell_asset: "flute-webhooks-cli-installer.ps1",
};

pub const FLUTE_CLI_MCP: BinaryInfo = BinaryInfo {
    name: "flute-cli-mcp",
    repo: "flute-cli-mcp",
    brew: "brew install getflute/flute-cli-mcp/flute-cli-mcp",
    script_asset: "flute-cli-mcp-installer.sh",
    powershell_asset: "flute-cli-mcp-installer.ps1",
};

pub const FLUTE_WEBHOOKS_MCP: BinaryInfo = BinaryInfo {
    name: "flute-webhooks-mcp",
    repo: "flute-webhooks-mcp",
    brew: "brew install getflute/flute-webhooks-mcp/flute-webhooks-mcp",
    script_asset: "flute-webhooks-mcp-installer.sh",
    powershell_asset: "flute-webhooks-mcp-installer.ps1",
};

const FLUTE_CLI_ENTRIES: [EntryTemplate; 2] = [
    EntryTemplate {
        name: "flute-sandbox",
        profile: "sandbox",
        readonly: false,
    },
    EntryTemplate {
        name: "flute-prod-readonly",
        profile: "production",
        readonly: true,
    },
];

const WEBHOOKS_ENTRIES: [EntryTemplate; 2] = [
    EntryTemplate {
        name: "flute-webhooks-sandbox",
        profile: "sandbox",
        readonly: false,
    },
    EntryTemplate {
        name: "flute-webhooks-prod",
        profile: "production",
        readonly: false,
    },
];

/// Env var that pins the Flute environment for every server entry.
pub const PROFILE_VAR: &str = "FLUTE_PROFILE";
/// Env var that lifts `flute-cli-mcp`'s production write guard.
pub const PROD_WRITES_VAR: &str = "FLUTE_MCP_ALLOW_PROD_WRITES";

impl McpKind {
    pub const ALL: [McpKind; 2] = [McpKind::Webhooks, McpKind::FluteCli];

    pub fn label(self) -> &'static str {
        match self {
            McpKind::Webhooks => "Flute Webhooks MCP",
            McpKind::FluteCli => "Flute CLI MCP",
        }
    }

    pub fn cli(self) -> &'static BinaryInfo {
        match self {
            McpKind::Webhooks => &FLUTE_WEBHOOKS,
            McpKind::FluteCli => &FLUTE,
        }
    }

    pub fn server(self) -> &'static BinaryInfo {
        match self {
            McpKind::Webhooks => &FLUTE_WEBHOOKS_MCP,
            McpKind::FluteCli => &FLUTE_CLI_MCP,
        }
    }

    pub fn cli_env_var(self) -> &'static str {
        match self {
            McpKind::Webhooks => "FLUTE_WEBHOOKS_BIN",
            McpKind::FluteCli => "FLUTE_BIN",
        }
    }

    pub fn entries(self) -> &'static [EntryTemplate] {
        match self {
            McpKind::Webhooks => &WEBHOOKS_ENTRIES,
            McpKind::FluteCli => &FLUTE_CLI_ENTRIES,
        }
    }

    /// Next-step commands to authenticate the CLI for each profile.
    pub fn auth_commands(self) -> [String; 2] {
        let cli = self.cli().name;
        [
            format!("{cli} auth login"),
            format!("{cli} --profile production auth login"),
        ]
    }
}

impl BinaryInfo {
    pub fn repo_url(&self) -> String {
        format!("https://github.com/getflute/{}", self.repo)
    }

    fn asset_url(&self, asset: &str) -> String {
        format!(
            "https://github.com/getflute/{}/releases/latest/download/{asset}",
            self.repo
        )
    }

    pub fn script_install(&self) -> String {
        format!(
            "curl --proto '=https' --tlsv1.2 -LsSf {} | sh",
            self.asset_url(self.script_asset)
        )
    }

    pub fn powershell_install(&self) -> String {
        format!(
            "powershell -ExecutionPolicy Bypass -c \"irm {} | iex\"",
            self.asset_url(self.powershell_asset)
        )
    }

    /// `(label, command)` pairs to show for this OS.
    pub fn install_instructions(&self, os: Os) -> Vec<(&'static str, String)> {
        match os {
            Os::Windows => vec![("PowerShell", self.powershell_install())],
            _ => vec![
                ("Homebrew", self.brew.to_string()),
                ("script", self.script_install()),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_match_the_readmes() {
        let names: Vec<_> = McpKind::FluteCli
            .entries()
            .iter()
            .map(|e| (e.name, e.profile))
            .collect();
        assert_eq!(
            names,
            [
                ("flute-sandbox", "sandbox"),
                ("flute-prod-readonly", "production")
            ]
        );
        let names: Vec<_> = McpKind::Webhooks
            .entries()
            .iter()
            .map(|e| (e.name, e.profile))
            .collect();
        assert_eq!(
            names,
            [
                ("flute-webhooks-sandbox", "sandbox"),
                ("flute-webhooks-prod", "production")
            ]
        );
        assert!(McpKind::FluteCli.entries()[1].readonly);
        assert!(!McpKind::Webhooks.entries()[1].readonly);
    }

    #[test]
    fn binaries_and_env_vars_match_the_readmes() {
        assert_eq!(McpKind::FluteCli.cli().name, "flute");
        assert_eq!(McpKind::FluteCli.server().name, "flute-cli-mcp");
        assert_eq!(McpKind::FluteCli.cli_env_var(), "FLUTE_BIN");
        assert_eq!(McpKind::Webhooks.cli().name, "flute-webhooks");
        assert_eq!(McpKind::Webhooks.server().name, "flute-webhooks-mcp");
        assert_eq!(McpKind::Webhooks.cli_env_var(), "FLUTE_WEBHOOKS_BIN");
    }

    #[test]
    fn install_commands_match_the_readmes() {
        assert_eq!(
            FLUTE_WEBHOOKS_MCP.script_install(),
            "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/getflute/flute-webhooks-mcp/releases/latest/download/flute-webhooks-mcp-installer.sh | sh"
        );
        assert_eq!(
            FLUTE.powershell_install(),
            "powershell -ExecutionPolicy Bypass -c \"irm https://github.com/getflute/flute-cli/releases/latest/download/flute-installer.ps1 | iex\""
        );
        assert_eq!(
            FLUTE_WEBHOOKS.repo_url(),
            "https://github.com/getflute/flute-webhooks-cli"
        );
    }

    #[test]
    fn install_instructions_depend_on_os() {
        let mac = FLUTE.install_instructions(Os::MacOs);
        assert_eq!(
            mac[0],
            (
                "Homebrew",
                "brew install getflute/flute-cli/flute".to_string()
            )
        );
        assert_eq!(mac[1].0, "script");
        let win = FLUTE.install_instructions(Os::Windows);
        assert_eq!(win.len(), 1);
        assert_eq!(win[0].0, "PowerShell");
    }
}
