# ReQurv Bridge

[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-00DC82?logo=gnu)](./LICENSE)
[![CI](https://img.shields.io/badge/CI-lint%20%2B%20typecheck%20%2B%20tests-00DC82?logo=githubactions)](https://github.com/ReQurv/requrv-bridge/actions)
[![Release](https://img.shields.io/github/v/release/ReQurv/requrv-bridge?label=release)](https://github.com/ReQurv/requrv-bridge/releases)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-555)](https://github.com/ReQurv/requrv-bridge/releases)

![ReQurv Bridge](public/logo_requrv_bridge.png)

ReQurv Bridge is a desktop application that launches your AI coding agents pre-configured for [AI Hive](https://hive.requrv.ai), the ReQurv AI gateway.

[![Download for macOS (Apple Silicon)](https://img.shields.io/badge/Download%20for%20macOS%20(Apple%20Silicon)-dmg-00DC82?logo=apple&logoColor=white)](https://github.com/ReQurv/requrv-bridge/releases/latest/download/ReQurv.Bridge_aarch64.dmg)
[![Download for Windows](https://img.shields.io/badge/Download%20for%20Windows-msi%20%C2%B7%20exe-00DC82?logo=windows&logoColor=white)](https://github.com/ReQurv/requrv-bridge/releases/latest/download/ReQurv.Bridge_x64-setup.exe)
[![Download for Linux](https://img.shields.io/badge/Download%20for%20Linux-AppImage%20%C2%B7%20deb%20%C2%B7%20rpm-00DC82?logo=linux&logoColor=white)](https://github.com/ReQurv/requrv-bridge/releases/latest/download/ReQurv.Bridge_amd64.AppImage)

![Screenshot](docs/screenshot.png)

## Supported agents

- [OpenCode](https://opencode.ai) — coding IDE (on macOS both the desktop app and the CLI are supported, elsewhere the CLI)
- [Codex](https://chatgpt.com/codex) — OpenAI's coding CLI (on macOS the ChatGPT app is also supported)
- [Claude Code](https://claude.com/product/claude-code) — Anthropic's coding agent (CLI)
- [Hermes](https://hermes-ide.com) — coding IDE whose Agent mode runs on the Claude Agent SDK (desktop app)

## Features

- Save and verify your AI Hive API key (validated against the gateway when saved)
- List the models available on AI Hive and pick a default one
- Automatic detection of installed agents (with support for nvm, Volta, Homebrew, global npm installs and stale Windows registry PATH entries)
- One-click launch:
  - **Target choice**: the app and CLI appear as separate actions, so you launch the installed surface directly
  - **CLI installation**: when a CLI is not detected, its card shows the official platform-specific installer command and documentation. Run the copied command in your terminal, then return and click "Rileva"
  - **CLI project folder**: the first CLI launch opens a folder picker; its selection is reused for the app session and can be changed with the "Cambia cartella" button in the project-folder panel. Every CLI launch uses that path as its working directory (Terminal on macOS; process working directory on Linux/Windows)
  - **OpenCode**: writes the `requrv-hive` provider into the global config file (`~/.config/opencode/opencode.jsonc` or `.json`), preserving other settings, and can restore it with one click from the card's restore button (the original file is backed up as `.bak`, and the backup is never overwritten by later launches)
  - **Codex (terminal)**: points `~/.codex/config.toml` at AI Hive exactly like ChatGPT.app does (same `requrv-hive` provider, `wire_api = "responses"`, model catalog, no WebSocket) and launches the CLI with the `HIVE_API_KEY` variable; a `codex` typed by hand in a terminal also uses AI Hive. The configuration is shared between the two surfaces, so launching one re-points the other (the app re-reads it on every request, no restart needed)
  - **ChatGPT.app (app, macOS only)**: points `~/.codex/config.toml` at AI Hive through a dedicated `requrv-hive` provider (`base_url`, `wire_api = "responses"`, key included; no WebSocket, which the gateway does not support) plus a model catalog in `~/.codex/hive-models.json`, and saves the key in `~/.codex/auth.json` in apikey mode; the original files are backed up as `.hive.bak` and can be restored with one click from the "Restore ChatGPT" button. The model catalog is read at startup, so if the app is already open a restart is requested
  - **Claude Code (terminal)**: writes the AI Hive routing into the `env` block of `~/.claude/settings.json` (`ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_MODEL`, plus the per-model context window), preserving every other setting of yours, and also passes those variables to the launched process. Because the client applies that file, the configuration survives Bridge: a `claude` started by hand, in a new terminal with no Hive variables, goes to AI Hive. The original file is backed up (`settings.json.hive.bak`) and can be restored with one click. First it verifies that the gateway exposes the `/messages` endpoint (Anthropic Messages API). (The Code tab of Claude Desktop is not supported: in consumer versions it is bound to the claude.ai account and cannot use an external gateway)
  - **Hermes (app)**: launches the app with the AI Hive configuration passed as environment variables only (`ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_MODEL`), without writing any configuration file; first verifies that the gateway exposes the `/messages` endpoint (Anthropic Messages API). Since the environment only applies to the launched process, if an instance is already open a restart is requested

On macOS the agents are TUI applications: the launch opens a script in the system terminal, providing a real TTY.

## Installation

Download the latest build for your platform from the [Releases page](https://github.com/ReQurv/requrv-bridge/releases):

| Platform | Packages                              |
| -------- | ------------------------------------- |
| macOS    | `.dmg` (and `.app.tar.gz`)            |
| Linux    | `.AppImage` and `.deb`                |
| Windows  | `.msi` (and NSIS `.exe` installer)    |

You need an AI Hive API key, which you paste into the app once (it is validated against the gateway and stored locally).

### Code signing

- **macOS**: release builds are code-signed (Developer ID) and notarized, so the app opens without any Gatekeeper warning
- **Windows**: the build is not code-signed. When SmartScreen shows "Windows protected your PC", click **More info** → **Run anyway**
- **Linux**: unsigned packages. If the AppImage won't start, grant it execute permission with `chmod +x ReQurv-Bridge_*.AppImage`

## Development

Requirements:

- [Bun](https://bun.sh)
- [Rust](https://rustup.rs) (for the Tauri backend)
- Tauri system dependencies: see the [prerequisites](https://tauri.app/start/prerequisites/)

Install the dependencies:

```bash
bun install
```

Start the app in development mode (Nuxt on `http://localhost:3001` + Tauri window):

```bash
bun run tauri dev
```

To develop the web part only (without Tauri; Tauri-bound features degrade automatically):

```bash
bun run dev
```

### Scripts

| Script              | Description                                       |
| ------------------- | ------------------------------------------------- |
| `bun run dev`       | Nuxt dev server (web only)                        |
| `bun run build`     | Production frontend build (SSG)                   |
| `bun run preview`   | Local preview of the production build             |
| `bun run tauri`     | Tauri CLI (`dev`, `build`, etc.)                  |
| `bun run build:mac` | Signed + notarized macOS release build (reads Apple credentials from the gitignored `.env`) |
| `bun run lint`      | ESLint                                            |
| `bun run typecheck` | Typecheck with `nuxt typecheck` / `vue-tsc`       |

Rust tests:

```bash
cd src-tauri
cargo test
```

## Project structure

```
app/                  # Nuxt frontend (UI, useHive composable)
src-tauri/            # Tauri backend (commands, service detection, launch)
public/               # Static assets (logo, favicon)
docs/                 # Documentation assets
```

Tauri commands exposed to the frontend (`src-tauri/src/commands.rs`):

- `get_hive_key` / `set_hive_key` / `delete_hive_key` — key management (stored in `hive.json` in the app config folder, with a `.bak` backup)
- `list_hive_models` — lists models from the gateway `GET /models`
- `check_services` — detects installed agents (and whether ChatGPT.app is configured for Hive)
- `launch_service` — configures and launches the selected service
- `configure_chatgpt_app` / `open_chatgpt_app` / `restart_chatgpt_app` / `restore_chatgpt_app` — configure, open, restart and restore ChatGPT.app on AI Hive

## Releases

Local build of the binaries and bundles:

```bash
bun run tauri build
```

Releases are automated: CI (`.github/workflows/build.yml`) builds for macOS, Linux and Windows and publishes a GitHub Release whenever a `v*` tag is pushed. The macOS bundle is code-signed (Developer ID) and notarized using repository secrets.

## CI

CI (`.github/workflows/ci.yml`) runs lint, typecheck and the Rust tests on every push.

## Contributing

Contributions are welcome — especially new agent connectors. See [CONTRIBUTING.md](./CONTRIBUTING.md) for development setup, code conventions and a step-by-step guide for adding a new connector.

- [Code of Conduct](./CODE_OF_CONDUCT.md)
- [Security policy](./SECURITY.md)

## License

[GPL-3.0-or-later](./LICENSE) © 2025-2026 ReQurv
