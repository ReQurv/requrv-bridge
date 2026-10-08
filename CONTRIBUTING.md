# Contributing to ReQurv Bridge

Thanks for your interest in ReQurv Bridge! The app is a thin, open-source layer between you and [AI Hive](https://hive.requrv.ai): its only job is to detect the AI coding agents installed on your machine and launch them pre-configured for the Hive gateway.

The best contributions are **new connectors**: support for another agent, IDE or CLI that can be pointed at AI Hive (e.g. Aider, Gemini CLI, Goose, Cline...). This document explains the architecture and walks you through adding one end-to-end.

## Getting started

### Prerequisites

- [Bun](https://bun.sh)
- [Rust](https://rustup.rs) (stable)
- Tauri system dependencies, per platform:
  - **macOS**: Xcode Command Line Tools (`xcode-select --install`)
  - **Linux (Debian/Ubuntu)**: `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`
  - **Windows**: Microsoft C++ Build Tools and WebView2 (preinstalled on Windows 10/11)
  - Full list: [Tauri prerequisites](https://tauri.app/start/prerequisites/)

### Setup

```bash
git clone https://github.com/ReQurv/requrv-bridge.git
cd requrv-bridge
bun install          # postinstall runs `nuxt prepare`
bun run tauri dev    # Nuxt on :3001 + Tauri window
```

> `bun install` must run before `bun run lint` / `bun run typecheck`: ESLint loads its base config from the generated `.nuxt/` directory.

### Verify your environment

```bash
bun run lint
bun run typecheck
cd src-tauri && cargo test && cargo check
```

All four must pass before opening a pull request (they also run in CI).

## Code conventions

- **User-facing strings are in Italian** — Rust error messages, UI copy and toasts. Keep new user-facing text in Italian; code identifiers, comments and docs stay in English.
- **Rust is indented with 2 spaces** (matches `.editorconfig`). Do **not** run `cargo fmt` — it would reformat everything to 4 spaces.
- **Frontend style** is enforced by ESLint (`@nuxt/eslint` stylistic): no semicolons, single quotes, 2 spaces, no trailing commas. `bun run lint` is the source of truth (there is a `.prettierrc` but no format script).
- **The version is duplicated** in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`. Contributors do **not** bump versions; maintainers do it at release time.
- **Web mode must keep working**: the frontend detects Tauri at runtime via `__TAURI_INTERNALS__` and degrades gracefully in a plain browser. Any new `invoke()` call must be guarded the same way (see `app/composables/useHive.ts`).

## Architecture

The app is a single Nuxt page plus one Rust backend module:

```
app/pages/index.vue          # the only page: renders one card per service
app/composables/useHive.ts   # all invoke() wrappers, types, UI state
app/components/ServiceCard.vue  # one card (icon, status, description, launch button)
src-tauri/src/lib.rs         # plugin setup + command registration
src-tauri/src/commands.rs    # ALL backend logic: key, models, detection, launch
```

Data flow: `useHive.ts` → `invoke('check_services' | 'launch_service' | ...)` → `commands.rs` → gateway (`https://hive.requrv.ai/api/v1`).

Gateway endpoints used by the app:

| Endpoint    | Used for                                          |
| ----------- | ------------------------------------------------- |
| `GET /models` | listing models, key validation                  |
| `GET /responses` | probing OpenAI-compatible support (Codex)      |
| `POST /messages` | probing Anthropic Messages support (Claude)   |

## Adding a new connector

A connector = "detect agent X, configure it for AI Hive, launch it". You will touch, in order: the Rust backend (detection + launch), the frontend (card + state), tests, and docs.

### 1. Pick a configuration strategy

Each existing connector shows a proven pattern — pick the one that fits the new agent:

| Pattern | Example | When to use |
| ------- | ------- | ----------- |
| **Persistent config, merged** | OpenCode | The agent has a global JSON/JSONC config; add a provider entry while preserving existing fields (strip JSONC comments, keep a backup) |
| **Dedicated profile** | Codex CLI | The agent supports profiles: write a separate `hive.config.toml` + launch flag (`--profile hive`), never touch the user's main config |
| **Env vars only** | Claude Code, Hermes | The agent is fully configurable via environment variables: launch with `ANTHROPIC_*`/`*_BASE_URL` + key + model, no files written. Desktop-app variant (Hermes): the env lives only in the launched process, so an already-running instance needs a confirmed restart (frontend-driven commands, like ChatGPT.app) |
| **Full app config rewrite** | ChatGPT.app | A desktop app whose config you rewrite (root `model`, `model_provider`, bearer token), with one-shot `.bak` backups, a restore command and a restart flow (config read at startup) |

Rules that apply to every strategy:

- **Preserve the user's data**: read-modify-write, never clobber; keep a `.bak`/`.hive.bak` backup before overwriting.
- **Probe before launching**: verify the gateway exposes the endpoint the agent needs (`/responses`, `/messages`) and fail with a clear Italian error if it does not.
- **Never store the key in the agent's config in a shared location** unless the pattern requires it (see ChatGPT.app: the key goes in `experimental_bearer_token` / `auth.json`, which is how that app consumes it).
- **macOS TTY**: TUI agents must be launched through the system terminal (a generated shell script opened in Terminal.app) so they get a real TTY. Reuse the existing launch helpers in `commands.rs`.

### 2. Rust backend (`src-tauri/src/commands.rs`)

1. **Detection** — extend `check_services()` (and the `ServiceStatus` struct) with a `<agent>_cli` / `<agent>_app` field. Reuse the existing resolvers: `find_npm()`, the nvm/Volta/Homebrew/registry PATH handling, and the per-OS path helpers.
2. **Launch** — extend `launch_service()` with a branch for the new `service` id, implementing the strategy from step 1. Errors are `String` and **in Italian** (they surface directly in the UI). Exception: if the flow needs a restart confirmation, add dedicated `#[tauri::command]`s (e.g. `launch_<agent>_app` / `restart_<agent>_app`) and drive them from the frontend — `launch_service` must NOT route that `service`/mode pair (see the ChatGPT.app and Hermes flows).
3. **Register** — if you add new `#[tauri::command]`s (e.g. a restore command), register them in the `invoke_handler` list in `src-tauri/src/lib.rs`.

### 3. Frontend

1. **`app/composables/useHive.ts`**:
   - add the id to `ServiceId`
   - add the `<agent>_cli` / `<agent>_app` fields to `ServiceStatus`
   - add an entry to `SERVICE_META` (title, **Italian** description, Iconify icon — `@iconify-json/simple-icons` is already installed — and download URL)
   - if the launch flow needs extra steps (modals, restore, restart), add them mirroring how the ChatGPT.app flow is wired (`requestLaunch` / `launch` / `confirmRestart` / `restoreApp`)
2. **`app/pages/index.vue`**: render the new `ServiceCard` (status flags, launch button, any extra buttons like "Restore").
3. Keep the `isTauri` guard: in web mode the card renders but launching is a no-op.

### 4. Tests

Unit tests live in `#[cfg(test)] mod tests` inside `src-tauri/src/commands.rs` (TOML/JSONC config writing, script generation, probe logic). Add tests for whatever pure logic your connector introduces, and make `cargo test` pass.

### 5. Docs

Update, in the same PR:

- `README.md` — supported agents list + features bullet for the new connector
- `AGENTS.md` — one paragraph describing the new launch flow (AI coding agents read this file to understand the codebase)

## Pull requests

- Fork and branch from `main` (e.g. `connector/aider`)
- Small, focused PRs; a connector is one PR
- All CI checks must pass: `bun run lint`, `bun run typecheck`, `cargo test`
- No version bumps, no `Cargo.toml`/`package.json` metadata changes
- Fill in the PR template; for new connectors, link the proposal issue if you have one

## Releasing (maintainers)

1. Bump the version in **both** `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`
2. Commit and push
3. Tag and push `v<version>` — `.github/workflows/build.yml` builds macOS, Linux and Windows with `tauri-action` and publishes the GitHub Release automatically
