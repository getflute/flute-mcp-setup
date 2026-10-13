# flute-mcp-setup

A small desktop app that configures the Flute MCP servers for your AI apps, so you
don't have to hand-edit JSON or TOML files.

Pick **Flute Webhooks MCP**, **Flute CLI MCP** or both, press **Configure**, and the app:

1. Finds your AI apps — **Claude Desktop**, **Claude Code** and **ChatGPT** (Codex) — and
   shows a green ✔ next to each one it found.
2. Checks that the Flute CLI and MCP binaries you selected are installed. If anything is
   missing, it tells you what, where it looked and how to install it, and changes nothing.
3. Writes the sandbox and production server entries from the MCP READMEs into every AI
   app it found, using absolute paths so the servers start even when your AI app was
   launched from the Dock, Start menu or an IDE.

The output panel shows every check and every setting it wrote.

## Before you start

You need at least one AI app:

- [Claude Desktop](https://claude.com/download)
- [Claude Code](https://code.claude.com/docs/en/overview)
- [ChatGPT](https://chatgpt.com/download) with Codex (the app reads `~/.codex/config.toml`)

And, for each MCP you want to set up, both its CLI and its MCP server:

| MCP | CLI | MCP server |
|---|---|---|
| Flute CLI MCP | [`flute`](https://github.com/getflute/flute-cli) | [`flute-cli-mcp`](https://github.com/getflute/flute-cli-mcp) |
| Flute Webhooks MCP | [`flute-webhooks`](https://github.com/getflute/flute-webhooks-cli) | [`flute-webhooks-mcp`](https://github.com/getflute/flute-webhooks-mcp) |

Each repository's README has Homebrew, shell-script and PowerShell install commands. If you
skip this step, the app lists the missing pieces with the exact install command for your OS.

## Download

Get the binary for your computer from the
[latest release](https://github.com/getflute/flute-mcp-setup/releases/latest). There is no
installer: you download one file and run it.

| Computer | File |
|---|---|
| Mac with Apple silicon (M1 or later) | `flute-mcp-setup-macos-arm64` |
| Windows, 64-bit Intel or AMD | `flute-mcp-setup-windows-x86_64.exe` |
| Linux, 64-bit Intel or AMD | `flute-mcp-setup-linux-x86_64` |

Intel Macs and ARM-based Windows or Linux machines are not supported.

### Verify the download (optional)

Each release includes a `SHA256SUMS` file. Download it next to the binary, then:

```sh
# macOS
shasum -a 256 -c SHA256SUMS --ignore-missing
# Linux
sha256sum -c SHA256SUMS --ignore-missing
```

```powershell
# Windows (PowerShell): compare the output with the line for this file in SHA256SUMS
Get-FileHash .\flute-mcp-setup-windows-x86_64.exe -Algorithm SHA256
```

## Running the app

The binaries are **not code-signed**, so each operating system warns you the first time
you run one. The steps below are the expected, safe way through those warnings for a file
you downloaded from this repository's releases.

### macOS (Apple silicon)

Browsers save the file without permission to run, and macOS blocks apps from
unidentified developers the first time. The quickest way is Terminal:

```sh
cd ~/Downloads
chmod +x flute-mcp-setup-macos-arm64
xattr -d com.apple.quarantine flute-mcp-setup-macos-arm64
./flute-mcp-setup-macos-arm64
```

`chmod +x` makes the file runnable, and `xattr -d com.apple.quarantine` removes the
"downloaded from the internet" flag that triggers the block. (If `xattr` reports
"No such xattr", the flag was never set — carry on.)

You can download with `curl` instead of a browser, which skips the quarantine flag:

```sh
curl -LO https://github.com/getflute/flute-mcp-setup/releases/latest/download/flute-mcp-setup-macos-arm64
chmod +x flute-mcp-setup-macos-arm64
./flute-mcp-setup-macos-arm64
```

**Prefer Finder?** Run the `chmod +x` command above once, then double-click the file.
macOS shows a message that Apple could not verify it is free of malware — click **Done**.
Open **System Settings → Privacy & Security**, scroll to **Security**, click **Open
Anyway** next to `flute-mcp-setup-macos-arm64`, and confirm when macOS asks (with your
password or Touch ID). You only have to do this once. (On macOS 14 and earlier you can
instead Control-click the file, choose **Open**, then **Open** again.)

Because this is a plain binary rather than an `.app` bundle, double-clicking it also opens
a Terminal window. That is normal; leave it open while you use the app — closing it quits
the app.

### Windows (64-bit Intel or AMD)

1. Your browser may warn that the file **isn't commonly downloaded**. Choose the option
   to keep it (in Edge: **…** → **Keep**, then **Show more** → **Keep anyway**).
2. Double-click `flute-mcp-setup-windows-x86_64.exe`. Microsoft Defender SmartScreen
   shows **Windows protected your PC** — click **More info**, then **Run anyway**. You only
   have to do this once.

Alternatively, right-click the file → **Properties** → tick **Unblock** → **OK** before
running it, or in PowerShell:

```powershell
Unblock-File .\flute-mcp-setup-windows-x86_64.exe
.\flute-mcp-setup-windows-x86_64.exe
```

### Linux (64-bit Intel or AMD)

Linux doesn't check signatures, but the file must be made executable:

```sh
cd ~/Downloads
chmod +x flute-mcp-setup-linux-x86_64
./flute-mcp-setup-linux-x86_64
```

Requirements: glibc 2.35 or newer (Ubuntu 22.04+, Debian 12+, Fedora 36+ or similar), and a
graphical desktop session (X11 or Wayland) with OpenGL drivers — the Mesa drivers most
distributions install by default are enough. Claude Desktop has no official Linux build, so
on Linux the app configures Claude Code and ChatGPT/Codex.

## Using the app

1. Check the **AI apps** row: green ✔ means found, grey ✖ means not found. Hover over an
   app to see its config file or why it wasn't found.
2. Tick **Flute Webhooks MCP**, **Flute CLI MCP** or both. **Configure** becomes available.
3. Press **Configure** and read the **Output** panel:
   - **green** — every AI app it found is configured,
   - **amber** — some apps were configured and others weren't; the output says why,
   - **red** — something required is missing (nothing was changed; the output says what
     to install) or configuration failed.
4. Restart the AI apps listed in the banner (start a new session in Claude Code).
5. Sign in to each CLI once per environment, if you haven't already:

   ```sh
   flute auth login
   flute --profile production auth login
   flute-webhooks auth login
   flute-webhooks --profile production auth login
   ```

Running the app again is safe: it updates its own entries and reports **Already up to date**
when nothing needs to change.

### What it changes

| Server entry | MCP | `FLUTE_PROFILE` |
|---|---|---|
| `flute-sandbox` | Flute CLI MCP | `sandbox` |
| `flute-prod-readonly` | Flute CLI MCP | `production` (writes stay blocked) |
| `flute-webhooks-sandbox` | Flute Webhooks MCP | `sandbox` |
| `flute-webhooks-prod` | Flute Webhooks MCP | `production` |

Each entry gets the absolute path of the MCP server as its command, plus `FLUTE_PROFILE`
and the absolute path of the CLI (`FLUTE_BIN` or `FLUTE_WEBHOOKS_BIN`). They are written to:

| AI app | Where |
|---|---|
| Claude Desktop | `claude_desktop_config.json` — macOS: `~/Library/Application Support/Claude/`, Windows: `%APPDATA%\Claude\` (Microsoft Store installs: the package's `LocalCache\Roaming\Claude\`), Linux: `~/.config/Claude/` |
| Claude Code | user scope, via `claude mcp add-json --scope user` |
| ChatGPT (Codex) | `~/.codex/config.toml` (or `$CODEX_HOME/config.toml`) |

Only those four entries are touched. Your other servers, settings and TOML comments are
kept, and any other keys you added to a Flute entry (such as `FLUTE_MERCHANT_ID`) are
preserved. If another entry already launches the same Flute binary under a different name,
the output warns you instead of changing it.

Before changing a file, the app saves a copy next to it named
`<file>.bak-YYYYMMDD-HHMMSS`. To undo, copy the backup back over the file.

## Building from source

Requires Rust 1.99.0, which is pinned in `rust-toolchain.toml`; with
[rustup](https://rustup.rs) it is installed automatically the first time you build.

```sh
cargo build --release
./target/release/flute-mcp-setup
```

On Debian/Ubuntu, install the windowing headers first:

```sh
sudo apt-get install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev
```

Run the checks the CI runs with `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
and `cargo test`.

## Releasing

1. Update `version` in `Cargo.toml` and commit.
2. Tag and push: `git tag v0.1.0 && git push origin v0.1.0` (the tag must be `v` followed
   by the `Cargo.toml` version).

The **Release** workflow tests and builds the three binaries and publishes them with
`SHA256SUMS` as a GitHub Release.

To move to a newer Rust, change the version in `rust-toolchain.toml`, fix anything new that
`cargo clippy --all-targets -- -D warnings` reports, and commit.

## License

MIT — see [LICENSE](LICENSE).
