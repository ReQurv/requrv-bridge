use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::Manager;

// Local development override: a hive.env file in the app config dir may set
// the HIVE_* base URLs (KEY=VALUE lines, '#' comments); real environment
// variables win, and without the file the production defaults apply.
const LOCAL_ENV_FILE_NAME: &str = "hive.env";

fn local_env_entries(content: &str) -> Vec<(&str, &str)> {
  content
    .lines()
    .filter_map(|line| {
      let line = line.trim();
      if line.is_empty() || line.starts_with('#') {
        return None;
      }
      let (name, value) = line.split_once('=')?;
      let (name, value) = (name.trim(), value.trim().trim_matches('"'));
      name.starts_with("HIVE_").then_some((name, value))
    })
    .collect()
}

pub fn load_local_env_file(app: &tauri::AppHandle) {
  let Ok(dir) = app.path().app_config_dir() else {
    return;
  };
  let Ok(content) = std::fs::read_to_string(dir.join(LOCAL_ENV_FILE_NAME)) else {
    return;
  };
  for (name, value) in local_env_entries(&content) {
    if std::env::var(name).is_err() {
      std::env::set_var(name, value);
    }
  }
}

// Base URLs are overridable via environment variables so the whole app can
// be pointed at a local requrv-proxy instance during development; the
// defaults are the production AI Hive gateway.
fn hive_base_url(env_key: &str, default: &str) -> String {
  std::env::var(env_key)
    .ok()
    .filter(|v| !v.trim().is_empty())
    .unwrap_or_else(|| default.to_string())
}

pub fn hive_openai_base_url() -> String {
  hive_base_url("HIVE_OPENAI_BASE_URL", "https://hive.requrv.ai/api/v1")
}

// Claude Code speaks the Anthropic Messages API and appends /v1/messages to
// ANTHROPIC_BASE_URL, so the base is the gateway without the trailing /v1.
pub fn hive_anthropic_base_url() -> String {
  hive_base_url("HIVE_ANTHROPIC_BASE_URL", "https://hive.requrv.ai/api")
}

// Le release pubbliche dell'app: la più recente è la candidata aggiornamento.
pub const GITHUB_LATEST_RELEASE_URL: &str =
  "https://api.github.com/repos/ReQurv/requrv-bridge/releases/latest";

const KEY_FILE_NAME: &str = "hive.json";

fn key_file_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
  let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
  Ok(dir.join(KEY_FILE_NAME))
}

#[tauri::command]
pub fn get_hive_key(app: tauri::AppHandle) -> Result<Option<String>, String> {
  let path = key_file_path(&app)?;
  let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
  let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
  Ok(value
    .get("apiKey")
    .and_then(|k| k.as_str())
    .map(|s| s.to_string()))
}

// Persist the key (and the model it was verified against) and realign every
// config this launcher already wrote, so the key on disk can never diverge
// from the one in the app.
#[tauri::command]
pub async fn set_hive_key(app: tauri::AppHandle, key: String, model: String) -> Result<(), String> {
  let key = key.trim();
  let model = model.trim();
  if key.is_empty() {
    return Err("La chiave non può essere vuota".into());
  }
  let path = key_file_path(&app)?;
  write_key_file(&path, key, model)?;
  // The catalog (context window, modalities, reasoning levels) is only needed
  // by the Codex config, so it is fetched once, here, when one exists.
  let failures = match home_dir() {
    Some(home) if !model.is_empty() => {
      let models = list_hive_models(key.to_string()).await.unwrap_or_default();
      reapply_persisted_configs(&home, model, key, &models)
    }
    _ => Vec::new(),
  };
  if !failures.is_empty() {
    return Err(format!(
      "Chiave salvata, ma la configurazione esistente non è stata aggiornata: {}",
      failures.join("; ")
    ));
  }
  Ok(())
}

fn write_key_file(path: &Path, key: &str, model: &str) -> Result<(), String> {
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  if let Ok(old) = std::fs::read_to_string(path) {
    let _ = std::fs::write(path.with_extension("json.bak"), old);
  }
  let value = serde_json::json!({ "apiKey": key, "model": model });
  std::fs::write(path, value.to_string()).map_err(|e| e.to_string())
}

// The .bak holds the previous key in plaintext; deleting the key must not
// leave it readable on disk.
fn delete_key_files(path: &Path) -> Result<(), String> {
  for file in [path.to_path_buf(), path.with_extension("json.bak")] {
    match std::fs::remove_file(&file) {
      Ok(()) => {}
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(e.to_string()),
    }
  }
  Ok(())
}

#[tauri::command]
pub fn delete_hive_key(app: tauri::AppHandle) -> Result<(), String> {
  delete_key_files(&key_file_path(&app)?)
}

// A key change must reach every file this launcher already wrote, or the key
// on disk silently diverges from the one in the app. Only the targets that
// persist something are touched (Hermes is env-only). Returns the failures so
// the caller reports them instead of leaving a half-applied key behind.
fn reapply_persisted_configs(
  home: &Path,
  model: &str,
  key: &str,
  models: &[HiveModel],
) -> Vec<String> {
  let mut failures = Vec::new();
  if opencode_configured_in(home) {
    if let Err(e) = write_opencode_config_in(home, model, key).map(|_| ()) {
      failures.push(format!("OpenCode: {e}"));
    }
  }
  if claude_code_configured_in(home) {
    if let Err(e) = write_claude_code_settings_in(home, model, key) {
      failures.push(format!("Claude Code: {e}"));
    }
  }
  // Runs even without a model list: the key must reach config.toml/auth.json
  // either way, and configure_chatgpt_app_in keeps the existing catalog when
  // the list is empty.
  if chatgpt_app_configured_in(home) {
    if let Err(e) = configure_chatgpt_app_in(home, model, models, key) {
      failures.push(format!("Codex: {e}"));
    }
  }
  failures
}

#[derive(Serialize, Deserialize, Clone)]
pub struct HiveModel {
  pub id: String,
  pub model_type: String,
}

#[tauri::command]
pub async fn list_hive_models(key: String) -> Result<Vec<HiveModel>, String> {
  let client = reqwest::Client::new();
  let url = format!("{}/models", hive_openai_base_url());
  let response = client
    .get(&url)
    .bearer_auth(key.trim())
    .timeout(std::time::Duration::from_secs(20))
    .send()
    .await
    .map_err(|e| format!("Impossibile raggiungere AI Hive: {e}"))?;

  let status = response.status();
  if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
    return Err("Chiave non valida: AI Hive ha rifiutato l'autenticazione (401/403)".into());
  }
  if !status.is_success() {
    return Err(format!("AI Hive ha risposto con lo stato {status}"));
  }

  let body: serde_json::Value = response
    .json()
    .await
    .map_err(|e| format!("Risposta non valida da AI Hive: {e}"))?;

  let models = body
    .get("data")
    .and_then(|d| d.as_array())
    .map(|items| {
      items
        .iter()
        .filter_map(|m| {
          let id = m.get("id")?.as_str()?.to_string();
          let model_type = m
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
          Some(HiveModel { id, model_type })
        })
        .collect()
    })
    .unwrap_or_default();

  Ok(models)
}

// Error for a gateway probe status, if any. 4xx from the handler (422
// validation on the empty body, 401, 405, ...) prove the route exists, so
// only 404 (route missing) and 5xx (gateway failure) are errors.
fn probe_error(status: reqwest::StatusCode, agent: &str, endpoint: &str) -> Option<String> {
  if status == reqwest::StatusCode::NOT_FOUND {
    Some(format!(
      "{agent} richiede l'endpoint {endpoint}, che AI Hive non espone (HTTP 404). Riprova più tardi."
    ))
  } else if status.is_server_error() {
    Some(format!("AI Hive ha risposto con lo stato {status}"))
  } else {
    None
  }
}

// POST an empty body to the gateway endpoint the agent needs. `base` is the
// same base URL the agent's own client will use, so the probe proves the route
// on the host the session will really hit (the OpenAI and Anthropic gateways
// are separate hosts/proxies and only one of them may expose a given path).
// A 4xx from the handler (e.g. 422 validation on the empty body) proves the
// route exists without invoking a model; only 404 and 5xx are errors. Probing
// before opening a session fails fast with an actionable message instead of a
// broken TUI.
async fn assert_endpoint_available(base: &str, key: &str, endpoint: &str, agent: &str) -> Result<(), String> {
  let client = reqwest::Client::new();
  let url = format!("{base}{endpoint}");
  let response = client
    .post(&url)
    .bearer_auth(key.trim())
    .json(&serde_json::json!({}))
    .timeout(std::time::Duration::from_secs(10))
    .send()
    .await
    .map_err(|e| format!("Impossibile raggiungere AI Hive: {e}"))?;
  if let Some(err) = probe_error(response.status(), agent, endpoint) {
    return Err(err);
  }
  Ok(())
}

// Codex (>= 0.136) accepts only `wire_api = "responses"`, so the gateway must
// expose POST /responses. The gateway routes only POST on that path (HEAD/GET
// never complete), so the probe POSTs an empty body.
async fn assert_responses_available(key: &str) -> Result<(), String> {
  assert_endpoint_available(&hive_openai_base_url(), key, "/responses", "Codex").await
}

// Claude Code speaks the Anthropic Messages API and appends /v1/messages to
// ANTHROPIC_BASE_URL, so the probe targets the same host+path. The gateway must
// also accept `role: "system"` entries in messages[], which Claude Code v2.x
// sends.
async fn assert_messages_available(key: &str, agent: &str) -> Result<(), String> {
  let endpoint = "/v1/messages";
  assert_endpoint_available(&hive_anthropic_base_url(), key, endpoint, agent).await
}

#[derive(Serialize)]
pub struct ServiceStatus {
  pub opencode: bool,
  pub codex: bool,
  pub claude_code: bool,
  pub hermes: bool,
  pub opencode_app: bool,
  pub opencode_cli: bool,
  pub codex_app: bool,
  pub codex_cli: bool,
  pub codex_app_configured: bool,
  pub claude_code_cli: bool,
  pub hermes_app: bool,
  pub hermes_cli: bool,
  pub claude_desktop: bool,
  pub claude_desktop_app: bool,
  pub claude_desktop_configured: bool,
  pub opencode_configured: bool,
  pub claude_code_cli_configured: bool,
  pub hermes_configured: bool,
}

// Reports every launch target separately so the UI can offer the app/terminal
// choice only when both destinations exist.
#[tauri::command]
pub fn check_services() -> ServiceStatus {
  let opencode_app = opencode_app_path().is_some();
  let opencode_cli = find_service_binary("opencode").is_some();
  let codex_app = chatgpt_app_bundle().is_some();
  let codex_cli = find_service_binary("codex").is_some() || codex_app_binary().is_some();
  let codex_app_configured = home_dir().is_some_and(|home| chatgpt_app_configured_in(&home));
  // Claude Code is terminal-only; Claude Desktop is app-only (3p gateway).
  let claude_code_cli = find_service_binary("claude").is_some();
  // Hermes Agent ships a desktop app and a standalone CLI; they are detected
  // and launched independently.
  let hermes_app = hermes_app_path().is_some();
  let hermes_cli = hermes_cli_path().is_some();
  let claude_desktop_app = claude_desktop_app_path().is_some();
  let claude_desktop_configured =
    home_dir().is_some_and(|home| claude_desktop_configured_in(&home));
  let opencode_configured = home_dir().is_some_and(|home| opencode_configured_in(&home));
  let claude_code_cli_configured =
    home_dir().is_some_and(|home| claude_code_configured_in(&home));
  let hermes_configured = home_dir().is_some_and(|home| hermes_configured_in(&home));
  ServiceStatus {
    opencode: opencode_app || opencode_cli,
    codex: codex_app || codex_cli,
    claude_code: claude_code_cli,
    hermes: hermes_app,
    opencode_app,
    opencode_cli,
    codex_app,
    codex_cli,
    codex_app_configured,
    claude_code_cli,
    hermes_app,
    hermes_cli,
    claude_desktop: claude_desktop_app,
    claude_desktop_app,
    claude_desktop_configured,
    opencode_configured,
    claude_code_cli_configured,
    hermes_configured,
  }
}

// Bin directories of every nvm-managed node version, newest first. GUI apps
// inherit a minimal PATH, so version-manager installs are probed explicitly.
fn nvm_bin_dirs() -> Vec<PathBuf> {
  let Some(home) = home_dir() else {
    return Vec::new();
  };
  let Ok(entries) = std::fs::read_dir(home.join(".nvm").join("versions").join("node")) else {
    return Vec::new();
  };
  let mut dirs: Vec<(String, PathBuf)> = entries
    .flatten()
    .filter_map(|entry| {
      let name = entry.file_name().to_string_lossy().to_string();
      if name.starts_with('v') {
        Some((name, entry.path().join("bin")))
      } else {
        None
      }
    })
    .collect();
  dirs.sort_by(|a, b| compare_versions(&b.0, &a.0));
  dirs.into_iter().map(|(_, path)| path).collect()
}

// Locate the npm executable, tolerating GUI-launched apps whose PATH misses
// version managers (nvm/volta) or Homebrew.
pub fn find_npm() -> Option<PathBuf> {
  if let Some(path) = find_on_path("npm") {
    return Some(path);
  }
  let home = home_dir()?;
  let mut candidates = vec![
    PathBuf::from("/opt/homebrew/bin/npm"),
    PathBuf::from("/usr/local/bin/npm"),
    home.join(".volta").join("bin").join("npm"),
  ];
  candidates.extend(nvm_bin_dirs().into_iter().map(|dir| dir.join("npm")));
  if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
    return Some(found);
  }
  npm_via_login_shell()
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
  let parse = |s: &str| s.trim_start_matches('v').split('.').filter_map(|p| p.parse::<u32>().ok()).collect::<Vec<_>>();
  parse(a).cmp(&parse(b))
}

// Last resort: ask the user's login shell (which sources .zprofile/.zshrc,
// where nvm & co are usually initialized) where npm lives.
#[cfg(target_os = "macos")]
fn npm_via_login_shell() -> Option<PathBuf> {
  for shell in ["/bin/zsh", "/bin/bash"] {
    let Ok(output) = Command::new(shell)
      .args(["-lic", "command -v npm"])
      .stdin(Stdio::null())
      .output()
    else {
      continue;
    };
    if !output.status.success() {
      continue;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let Some(line) = raw.lines().rev().find(|l| {
      let l = l.trim();
      l.starts_with('/') && l.ends_with("/npm")
    }) else {
      continue;
    };
    let path = PathBuf::from(line);
    if path.is_file() {
      return Some(path);
    }
  }
  None
}

#[cfg(not(target_os = "macos"))]
fn npm_via_login_shell() -> Option<PathBuf> {
  None
}

// PATH of the current process can be stale (e.g. the app is launched from an
// explorer that started before a PATH update), so on Windows we also read the
// live PATH values from the registry and expand their %VARS%.
#[cfg(windows)]
fn registry_env() -> std::collections::HashMap<String, String> {
  let mut map = std::collections::HashMap::new();
  for hive in [
    "HKCU\\Environment",
    "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
  ] {
    let Ok(output) = Command::new("reg").args(["query", hive]).output() else {
      continue;
    };
    let Ok(raw) = String::from_utf8(output.stdout) else {
      continue;
    };
    for line in raw.lines() {
      let trimmed = line.trim();
      if trimmed.is_empty() || trimmed.starts_with("HKEY") || trimmed.starts_with("---") {
        continue;
      }
      let mut parts = trimmed.split_whitespace();
      let Some(name) = parts.next() else { continue };
      let Some(kind) = parts.next() else { continue };
      if kind != "REG_SZ" && kind != "REG_EXPAND_SZ" {
        continue;
      }
      let value = parts.collect::<Vec<_>>().join(" ");
      if !value.is_empty() {
        map.insert(name.to_uppercase(), value);
      }
    }
  }
  map
}

#[cfg(windows)]
fn expand_registry_vars(value: &str, registry: &std::collections::HashMap<String, String>) -> String {
  let mut out = String::new();
  let mut rest = value;
  while let Some(start) = rest.find('%') {
    out.push_str(&rest[..start]);
    let after = &rest[start + 1..];
    if let Some(end) = after.find('%') {
      let var = &after[..end];
      let expanded = registry
        .get(&var.to_uppercase())
        .cloned()
        .or_else(|| std::env::var(var).ok())
        .unwrap_or_default();
      out.push_str(&expanded);
      rest = &after[end + 1..];
    } else {
      out.push('%');
      rest = after;
    }
  }
  out.push_str(rest);
  out
}

fn effective_path_dirs() -> Vec<String> {
  let mut dirs: Vec<String> = std::env::var("PATH")
    .unwrap_or_default()
    .split(if cfg!(windows) { ';' } else { ':' })
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .collect();
  #[cfg(windows)]
  {
    let registry = registry_env();
    if let Some(raw) = registry.get("PATH") {
      let expanded = expand_registry_vars(raw, &registry);
      for entry in expanded.split(';') {
        let entry = entry.trim();
        if !entry.is_empty() {
          dirs.push(entry.to_string());
        }
      }
    }
  }
  let mut seen = std::collections::HashSet::new();
  dirs.retain(|d| seen.insert(d.clone()));
  dirs
}

fn find_on_path(name: &str) -> Option<PathBuf> {
  // On Windows the extensionless "shim" npm drops next to the .cmd/.ps1 shims
  // is a bash script and cannot be executed, so skip the empty extension.
  let extensions: &[&str] = if cfg!(windows) {
    &[".exe", ".cmd", ".bat", ".ps1"]
  } else {
    &[""]
  };
  for dir in effective_path_dirs() {
    for ext in extensions {
      let candidate = Path::new(&dir).join(format!("{name}{ext}"));
      if candidate.is_file() {
        return Some(candidate);
      }
    }
  }
  None
}

fn home_dir() -> Option<PathBuf> {
  std::env::var("USERPROFILE")
    .or_else(|_| std::env::var("HOME"))
    .ok()
    .map(PathBuf::from)
}

// Per-OS root where a desktop app keeps its user config/data, derived from the
// home dir: "~/Library/Application Support" on macOS, "~\AppData\Roaming"
// (the default %APPDATA%) on Windows, "~/.config" on Linux. Derived from the
// home parameter rather than the process environment so the pure
// config-path builders stay deterministic and test-isolated (matching the
// home-relative convention used by the other agents' config paths). Only the
// current platform's branch compiles.
fn application_support_root(home: &Path) -> PathBuf {
  #[cfg(target_os = "macos")]
  {
    home.join("Library").join("Application Support")
  }
  #[cfg(windows)]
  {
    home.join("AppData").join("Roaming")
  }
  #[cfg(target_os = "linux")]
  {
    home.join(".config")
  }
}

// Directory of the active npm global prefix (covers nvm/nvm4w layouts where
// `npm i -g` shims live outside the default %APPDATA%\npm).
fn npm_global_dir() -> Option<PathBuf> {
  // find_npm (not find_on_path): a GUI-launched app has a minimal PATH and
  // would miss nvm/Homebrew installs otherwise.
  let npm = find_npm()?;
  let mut command = if cfg!(windows) {
    let mut c = Command::new("cmd");
    c.arg("/c").arg(&npm);
    c
  } else {
    Command::new(&npm)
  };
  command.args(["config", "get", "prefix"]);
  let output = command.output().ok()?;
  if !output.status.success() {
    return None;
  }
  let raw = String::from_utf8_lossy(&output.stdout);
  let line = raw.lines().next()?.trim().to_string();
  if line.is_empty() {
    return None;
  }
  Some(PathBuf::from(line))
}

fn find_service_binary(service: &str) -> Option<PathBuf> {
  if let Some(path) = find_on_path(service) {
    return Some(path);
  }
  let home = home_dir()?;
  let candidates: Vec<PathBuf> = match service {
    "opencode" => {
      let mut candidates = vec![
        home.join(".opencode").join("bin").join("opencode"),
        home.join(".opencode").join("bin").join("opencode.exe"),
      ];
      if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = PathBuf::from(appdata);
        candidates.push(appdata.join("npm").join("opencode.cmd"));
        candidates.push(appdata.join("npm").join("opencode.exe"));
      }
      candidates
    }
    "codex" => {
      if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = PathBuf::from(appdata);
        vec![
          appdata.join("npm").join("codex.cmd"),
          appdata.join("npm").join("codex.exe"),
        ]
      } else {
        vec![home.join(".local").join("bin").join("codex")]
      }
    }
    "claude" => {
      if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = PathBuf::from(appdata);
        vec![
          appdata.join("npm").join("claude.cmd"),
          appdata.join("npm").join("claude.exe"),
        ]
      } else {
        // npm global installs fall through to the nvm/npm-prefix fallbacks
        // below; the native installer puts the binary in ~/.local/bin.
        vec![home.join(".local").join("bin").join("claude")]
      }
    }
    _ => return None,
  };
  if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
    return Some(found);
  }

  // nvm per-version bins (macOS/Linux): the app's PATH may miss them, and the
  // active prefix is not necessarily the version the CLI was installed on.
  #[cfg(not(windows))]
  for dir in nvm_bin_dirs() {
    let candidate = dir.join(service);
    if candidate.is_file() {
      return Some(candidate);
    }
  }

  // Fallback: nvm4w per-version directories on Windows (%APPDATA%\nvm\v*\).
  #[cfg(windows)]
  if let Ok(appdata) = std::env::var("APPDATA") {
    let nvm = Path::new(&appdata).join("nvm");
    if let Ok(entries) = std::fs::read_dir(&nvm) {
      for entry in entries.flatten() {
        if !entry.path().is_dir() {
          continue;
        }
        let candidate = entry.path().join(format!("{service}.cmd"));
        if candidate.is_file() {
          return Some(candidate);
        }
      }
    }
  }

  // Fallback: active npm global prefix. On Windows the shims sit next to the
  // prefix, on macOS/Linux the executables live in <prefix>/bin.
  let npm_dir = npm_global_dir()?;
  let npm_candidates: Vec<PathBuf> = match service {
    "opencode" => vec![
      npm_dir.join("bin").join("opencode"),
      npm_dir.join("opencode.cmd"),
      npm_dir.join("opencode.exe"),
      npm_dir
        .join("node_modules")
        .join("opencode-ai")
        .join("bin")
        .join("opencode.exe"),
    ],
    "codex" => vec![
      npm_dir.join("bin").join("codex"),
      npm_dir.join("codex.cmd"),
      npm_dir.join("codex.exe"),
    ],
    "claude" => vec![
      npm_dir.join("bin").join("claude"),
      npm_dir.join("claude.cmd"),
      npm_dir.join("claude.exe"),
    ],
    _ => return None,
  };
  npm_candidates.into_iter().find(|c| c.is_file())
}

// Location of a desktop .app bundle among the given candidates.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_bundle_path_in(candidates: &[PathBuf]) -> Option<PathBuf> {
  candidates.iter().find(|path| path.is_dir()).cloned()
}

// ---------------------------------------------------------------------------
// Desktop-app lifecycle helpers for Windows and Linux
// ---------------------------------------------------------------------------
// On macOS every desktop app goes through LaunchServices (`open`) plus
// osascript/pkill, so the per-app code there stays platform-specific. On the
// other platforms the desktop app is a plain executable, so these shared
// helpers give each harness the same launch/running/quit plumbing without
// repeating the tasklist/pgrep details. Each helper is compiled only on its
// own platform; macOS never sees them.

// Launch a desktop app executable detached. The cross-platform sibling of
// `open_app_bundle` (macOS): stdio is nulled so the child outlives this call.
// No CREATE_NEW_CONSOLE here, unlike `spawn_cli`, because these are GUI
// (windows-subsystem) apps that own their window rather than a console.
#[cfg(any(windows, target_os = "linux"))]
fn desktop_spawn_app(bin: &Path, env: &[(&str, &str)], label: &str) -> Result<(), String> {
  let mut command = Command::new(bin);
  for (name, value) in env {
    command.env(name, value);
  }
  command
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .map(|_| ())
    .map_err(|e| format!("Impossibile avviare {label}: {e}"))
}

// True when any of the given process image names is live. Callers pass
// platform-appropriate names (`.exe` on Windows, bare on Linux).
#[cfg(windows)]
fn desktop_process_running(names: &[&str]) -> bool {
  for name in names {
    let Ok(output) = Command::new("tasklist")
      .args(["/FI", &format!("IMAGENAME eq {name}"), "/NH"])
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .output()
    else {
      continue;
    };
    if String::from_utf8_lossy(&output.stdout)
      .to_lowercase()
      .contains(&name.to_lowercase())
    {
      return true;
    }
  }
  false
}

#[cfg(target_os = "linux")]
fn desktop_process_running(names: &[&str]) -> bool {
  for name in names {
    let running = Command::new("pgrep")
      .arg("-x")
      .arg(name)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()
      .is_ok_and(|s| s.success());
    if running {
      return true;
    }
  }
  false
}

// Force-terminate the given process image names. Best effort: a missing or
// already-exited process is not an error (same contract as the macOS quit).
#[cfg(windows)]
fn desktop_process_quit(names: &[&str]) {
  for name in names {
    Command::new("taskkill")
      .args(["/F", "/IM", name])
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .ok();
  }
}

#[cfg(target_os = "linux")]
fn desktop_process_quit(names: &[&str]) {
  for name in names {
    Command::new("pkill")
      .arg("-x")
      .arg(name)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .ok();
  }
}

// Extract the launcher binary from a freedesktop `Exec=` value: the first
// whitespace-separated token, minus any leading `path=` prefix. Kept pure (no
// fs) so the parsing is testable on every host; the caller verifies the
// result is a real file.
#[cfg(any(target_os = "linux", test))]
fn desktop_entry_target(exec_value: &str) -> Option<String> {
  let token = exec_value.split_whitespace().next()?;
  let token = token.strip_prefix("path=").unwrap_or(token);
  if token.is_empty() {
    None
  } else {
    Some(token.to_string())
  }
}

// Resolve a GUI app from its freedesktop `.desktop` entry. Linux packages do
// not share an install convention (no %LOCALAPPDATA%\Programs equivalent), so
// guessing binary names is unreliable; the `.desktop` file the .deb/.rpm ships
// carries the real Exec path the vendor chose. `matcher` is a case-insensitive
// substring of the entry file name (e.g. "opencode", "claude").
#[cfg(target_os = "linux")]
fn desktop_app_from_desktop_entry(matcher: &str) -> Option<PathBuf> {
  let mut dirs = vec![PathBuf::from("/usr/share/applications")];
  if let Some(home) = home_dir() {
    dirs.push(home.join(".local").join("share").join("applications"));
  }
  let matcher = matcher.to_lowercase();
  for dir in dirs {
    let Ok(entries) = std::fs::read_dir(&dir) else {
      continue;
    };
    for entry in entries.flatten() {
      let file_name = entry.file_name();
      let name = file_name.to_string_lossy();
      if !name.ends_with(".desktop") || !name.to_lowercase().contains(&matcher) {
        continue;
      }
      let Ok(content) = std::fs::read_to_string(entry.path()) else {
        continue;
      };
      for line in content.lines() {
        let Some(value) = line.trim().strip_prefix("Exec=") else {
          continue;
        };
        // Exec is "binary [args]" — the launcher is the first token.
        let Some(binary) = desktop_entry_target(value) else {
          continue;
        };
        let path = PathBuf::from(&binary);
        if path.is_file() {
          return Some(path);
        }
      }
    }
  }
  None
}

#[cfg(target_os = "macos")]
fn opencode_app_path() -> Option<PathBuf> {
  let home = home_dir()?;
  app_bundle_path_in(&[
    PathBuf::from("/Applications/OpenCode.app"),
    home.join("Applications").join("OpenCode.app"),
  ])
}

#[cfg(windows)]
fn opencode_app_path() -> Option<PathBuf> {
  // NSIS installers lay the Electron app under %LOCALAPPDATA%\Programs; a
  // system-wide install may land in one of the Program Files roots.
  let mut roots: Vec<PathBuf> = Vec::new();
  if let Ok(local) = std::env::var("LOCALAPPDATA") {
    roots.push(Path::new(&local).join("Programs"));
  }
  for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
    if let Ok(dir) = std::env::var(var) {
      roots.push(PathBuf::from(dir));
    }
  }
  let mut candidates: Vec<PathBuf> = Vec::new();
  for root in roots {
    for dir in ["OpenCode", "opencode", "OpenCode Desktop"] {
      for exe in ["OpenCode.exe", "opencode-desktop.exe", "OpenCode Desktop.exe"] {
        candidates.push(root.join(dir).join(exe));
      }
    }
  }
  candidates.into_iter().find(|c| c.is_file())
}

#[cfg(target_os = "linux")]
fn opencode_app_path() -> Option<PathBuf> {
  // Prefer the .deb/.rpm's own .desktop entry, which names the real binary.
  // The bare `opencode` on PATH is the CLI (installed via curl|bash / npm), so
  // it is deliberately not a candidate: this is the *desktop* app.
  desktop_app_from_desktop_entry("opencode").or_else(|| {
    let mut candidates = vec![
      PathBuf::from("/usr/bin/opencode-desktop"),
      PathBuf::from("/usr/bin/OpenCode"),
      PathBuf::from("/usr/local/bin/opencode-desktop"),
      PathBuf::from("/opt/opencode-desktop/opencode-desktop"),
      PathBuf::from("/opt/opencode/OpenCode"),
    ];
    if let Some(home) = home_dir() {
      candidates.push(home.join(".local").join("bin").join("opencode-desktop"));
    }
    candidates.into_iter().find(|c| c.is_file())
  })
}

// The ChatGPT desktop app (macOS) is what users install as "Codex"; it also
// ships the codex engine in its resources.
#[cfg(target_os = "macos")]
fn chatgpt_app_bundle() -> Option<PathBuf> {
  let home = home_dir()?;
  app_bundle_path_in(&[
    PathBuf::from("/Applications/ChatGPT.app"),
    home.join("Applications").join("ChatGPT.app"),
  ])
}

// On Windows the ChatGPT app ships as a Microsoft Store (MSIX) package, so it
// has no fixed install path: locate the per-user package that matches "chatgpt"
// and find its launcher exe. The exact package family and exe sub-path are
// device-dependent — the candidates here cover the common MSIX layouts and
// must be confirmed on a real Store install.
#[cfg(windows)]
fn chatgpt_app_bundle() -> Option<PathBuf> {
  let Ok(local) = std::env::var("LOCALAPPDATA") else {
    return None;
  };
  let packages = Path::new(&local).join("Packages");
  let Ok(entries) = std::fs::read_dir(&packages) else {
    return None;
  };
  for entry in entries.flatten() {
    let family = entry.file_name().to_string_lossy();
    if !family.to_lowercase().contains("chatgpt") {
      continue;
    }
    let base = entry.path();
    for sub in ["", "Main", "ChatGPT", "app"] {
      for exe in ["ChatGPT.exe", "app.exe"] {
        let candidate = if sub.is_empty() {
          base.join(exe)
        } else {
          base.join(sub).join(exe)
        };
        if candidate.is_file() {
          return Some(candidate);
        }
      }
    }
  }
  None
}

#[cfg(target_os = "linux")]
fn chatgpt_app_bundle() -> Option<PathBuf> {
  // There is no Linux build of the ChatGPT desktop app; the Codex CLI is the
  // supported Codex surface there and shares this config.
  None
}

fn codex_app_binary_in(candidates: &[PathBuf]) -> Option<PathBuf> {
  candidates
    .iter()
    .map(|bundle| bundle.join("Contents").join("Resources").join("codex"))
    .find(|bin| bin.is_file())
}

// The codex engine bundled inside ChatGPT.app counts as an installation when
// the standalone CLI is absent.
fn codex_app_binary() -> Option<PathBuf> {
  let bundle = chatgpt_app_bundle()?;
  codex_app_binary_in(std::slice::from_ref(&bundle))
}

#[tauri::command]
pub async fn launch_service(
  app: tauri::AppHandle,
  service: String,
  model: String,
  key: String,
  mode: String,
  project_directory: Option<String>,
  models: Vec<HiveModel>,
) -> Result<(), String> {
  let model = model.trim();
  let key = key.trim();
  if model.is_empty() {
    return Err("Nessun modello selezionato".into());
  }
  if key.is_empty() {
    return Err("Chiave Hive non salvata".into());
  }
  // Desktop apps that read their config at startup are not handled here: they
  // need a restart confirmation, so the frontend drives them via their own
  // configure/open/restart commands — ("codex","app") through
  // configure_chatgpt_app, ("hermes","app") through launch_hermes_app,
  // ("opencode","app") through configure_opencode_app.
  match (service.as_str(), mode.as_str()) {
    ("opencode", "terminal") => {
      let directory = require_project_directory(project_directory.as_deref())?;
      launch_opencode_cli(&app, model, key, &directory)
    }
    ("codex", "terminal") => {
      let directory = require_project_directory(project_directory.as_deref())?;
      launch_codex_cli(&app, model, key, &models, &directory).await
    }
    ("claude_code", "terminal") => {
      let directory = require_project_directory(project_directory.as_deref())?;
      launch_claude_cli(&app, model, key, &directory).await
    }
    ("hermes", "terminal") => {
      let directory = require_project_directory(project_directory.as_deref())?;
      launch_hermes_cli(&app, model, key, &directory).await
    }
    _ => Err(format!("Avvio non valido: {service} in modalità {mode}")),
  }
}

fn require_project_directory(directory: Option<&str>) -> Result<PathBuf, String> {
  let directory = directory
    .map(str::trim)
    .filter(|directory| !directory.is_empty())
    .ok_or_else(|| String::from("Scegli una cartella di progetto prima di avviare la CLI."))?;
  let path = Path::new(directory);
  if !path.is_dir() {
    return Err(String::from("La cartella di progetto selezionata non è più disponibile."));
  }
  path
    .canonicalize()
    .map_err(|_| String::from("Non è possibile accedere alla cartella di progetto selezionata."))
}

const OPENCODE_PROVIDER_ID: &str = "requrv-hive";

// Capabilities declared explicitly by the launcher. The other Hive models
// keep conservative defaults: only the name in the client configs and a
// 128k context window.
const KNOWN_HIVE_MODEL: &str = "requrv-small-3.8";
const KNOWN_HIVE_MODEL_CONTEXT: u32 = 262_144;
const KNOWN_HIVE_MODEL_OUTPUT: u32 = 32_768;
const KNOWN_HIVE_MODEL_INPUTS: [&str; 3] = ["text", "image", "video"];
const KNOWN_HIVE_MODEL_OUTPUTS: [&str; 1] = ["text"];
// Modalities the codex model catalog can express: `input_modalities` only
// accepts text/image/audio, so video is dropped for the ChatGPT app.
const KNOWN_HIVE_MODEL_CATALOG_INPUTS: [&str; 2] = ["text", "image"];
const KNOWN_HIVE_MODEL_REASONING_LEVELS: [&str; 3] = ["low", "medium", "xhigh"];
const KNOWN_HIVE_MODEL_DEFAULT_REASONING: &str = "xhigh";
const DEFAULT_HIVE_CONTEXT: u32 = 128_000;
const DEFAULT_HIVE_INPUTS: [&str; 1] = ["text"];

// Context window to pin in clients that do not know the model: the real limit
// for the known model, a conservative default otherwise.
fn hive_model_context(model: &str) -> u32 {
  if model == KNOWN_HIVE_MODEL {
    KNOWN_HIVE_MODEL_CONTEXT
  } else {
    DEFAULT_HIVE_CONTEXT
  }
}

// Model entry for the OpenCode provider block: the known model gets its full
// capabilities, the others only the name.
fn opencode_model_entry(model: &str) -> serde_json::Value {
  if model == KNOWN_HIVE_MODEL {
    serde_json::json!({
      "name": model,
      // Without this flag the client hides the thinking-effort control, so the
      // user cannot pick the effort `options.reasoningEffort` sets.
      "reasoning": true,
      "limit": {
        "context": KNOWN_HIVE_MODEL_CONTEXT,
        "output": KNOWN_HIVE_MODEL_OUTPUT,
      },
      "modalities": {
        "input": KNOWN_HIVE_MODEL_INPUTS,
        "output": KNOWN_HIVE_MODEL_OUTPUTS,
      },
      // OpenCode's default ladder for a reasoning model is low/medium/high: the
      // gateway rejects `high` with 502 Bad Gateway, and `xhigh` — which it
      // does accept — is not in the ladder. Disable the former, add the latter.
      "variants": {
        "high": { "disabled": true },
        "xhigh": { "reasoningEffort": "xhigh" },
      },
      // Provider options: the effort sent to the gateway when no variant is
      // picked.
      "options": {
        "reasoningEffort": "medium",
      },
    })
  } else {
    serde_json::json!({ "name": model })
  }
}

// Strip // and /* */ comments outside of JSON strings so the JSONC config
// can be parsed with serde_json.
fn strip_jsonc_comments(input: &str) -> String {
  let mut out = String::with_capacity(input.len());
  let mut chars = input.chars().peekable();
  let mut in_string = false;
  let mut in_line_comment = false;
  let mut in_block_comment = false;
  while let Some(c) = chars.next() {
    if in_line_comment {
      if c == '\n' {
        in_line_comment = false;
        out.push(c);
      }
      continue;
    }
    if in_block_comment {
      if c == '*' && chars.peek() == Some(&'/') {
        chars.next();
        in_block_comment = false;
      }
      continue;
    }
    if in_string {
      out.push(c);
      match c {
        '\\' => {
          if let Some(escaped) = chars.next() {
            out.push(escaped);
          }
        }
        '"' => in_string = false,
        _ => {}
      }
      continue;
    }
    match c {
      '"' => {
        in_string = true;
        out.push(c);
      }
      '/' if chars.peek() == Some(&'/') => {
        chars.next();
        in_line_comment = true;
      }
      '/' if chars.peek() == Some(&'*') => {
        chars.next();
        in_block_comment = true;
      }
      _ => out.push(c),
    }
  }
  out
}

// JSONC allows trailing commas (e.g. `"a": 1,` right before `}` or `]`) but
// strict serde_json rejects them. Drop any comma whose next non-whitespace
// char is `}` or `]`, leaving string contents untouched. Runs after the
// comment strip, so only strings and whitespace need handling.
fn strip_trailing_commas(input: &str) -> String {
  let chars: Vec<char> = input.chars().collect();
  let mut out = String::with_capacity(input.len());
  let mut in_string = false;
  let mut i = 0;
  while i < chars.len() {
    let c = chars[i];
    if in_string {
      out.push(c);
      if c == '\\' && i + 1 < chars.len() {
        out.push(chars[i + 1]);
        i += 1;
      } else if c == '"' {
        in_string = false;
      }
    } else if c == '"' {
      in_string = true;
      out.push(c);
    } else if c == ',' {
      let mut j = i + 1;
      while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
      }
      if !(j < chars.len() && (chars[j] == '}' || chars[j] == ']')) {
        out.push(c);
      }
    } else {
      out.push(c);
    }
    i += 1;
  }
  out
}

const OPENCODE_SCHEMA_KEY: &str = "$schema";
const OPENCODE_SCHEMA_URL: &str = "https://opencode.ai/config.json";

// Merge the ReQurv Hive provider block and the default model into the global
// opencode config, leaving every other field untouched.
fn merge_hive_provider(config: &mut serde_json::Value, model: &str, key: &str) {
  if !config.is_object() {
    *config = serde_json::json!({});
  }
  let Some(root) = config.as_object_mut() else {
    return;
  };
  root.entry(OPENCODE_SCHEMA_KEY.to_string())
    .or_insert_with(|| serde_json::Value::String(OPENCODE_SCHEMA_URL.into()));
  let provider_block = serde_json::json!({
    "npm": "@ai-sdk/openai-compatible",
    "name": "ReQurv Hive",
    "options": {
      "baseURL": hive_openai_base_url(),
      "apiKey": key,
    },
    "models": {
      model: opencode_model_entry(model),
    },
  });
  let existing = root
    .get("provider")
    .cloned()
    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
  let mut providers = existing.as_object().cloned().unwrap_or_default();
  providers.insert(OPENCODE_PROVIDER_ID.to_string(), provider_block);
  root.insert("provider".to_string(), serde_json::Value::Object(providers));
  // A disabled provider is not loaded by opencode, so the model we just set
  // would resolve to "Model not found". Drop only our own id: the other
  // entries are the user's choices.
  if let Some(disabled) = root.get_mut("disabled_providers").and_then(|d| d.as_array_mut()) {
    disabled.retain(|id| id.as_str() != Some(OPENCODE_PROVIDER_ID));
  }
  root.insert(
    "model".to_string(),
    serde_json::Value::String(format!("{OPENCODE_PROVIDER_ID}/{model}")),
  );
}

// Resolve the global opencode config file: prefer an existing one
// (.jsonc or .json), defaulting to creating opencode.jsonc.
fn opencode_config_path_in(home: &Path) -> PathBuf {
  let dir = home.join(".config").join("opencode");
  let jsonc = dir.join("opencode.jsonc");
  if jsonc.exists() {
    return jsonc;
  }
  let json = dir.join("opencode.json");
  if json.exists() {
    return json;
  }
  jsonc
}
// The backup sits next to the resolved config file and keeps the extension
// convention of the writer ("opencode.jsonc" -> "opencode.jsonc.bak").
fn opencode_backup_for(path: &Path) -> PathBuf {
  let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("json");
  path.with_extension(format!("{ext}.bak"))
}

// Which backup of the two candidate extensions exists, if any: the writer
// picked one file, but an older version may have used the other name.
fn opencode_backup_path_in(home: &Path) -> Option<PathBuf> {
  let dir = home.join(".config").join("opencode");
  ["opencode.jsonc", "opencode.json"]
    .iter()
    .map(|name| opencode_backup_for(&dir.join(name)))
    .find(|backup| backup.exists())
}

// True when the global config is pointed at AI Hive by this launcher.
fn opencode_configured_in(home: &Path) -> bool {
  let Ok(raw) = std::fs::read_to_string(opencode_config_path_in(home)) else {
    return false;
  };
  let Ok(cleaned) =
    serde_json::from_str::<serde_json::Value>(&strip_trailing_commas(&strip_jsonc_comments(&raw)))
  else {
    return false;
  };
  cleaned
    .get("provider")
    .and_then(|p| p.get(OPENCODE_PROVIDER_ID))
    .and_then(|p| p.get("options"))
    .and_then(|o| o.get("baseURL"))
    .and_then(|v| v.as_str())
    == Some(hive_openai_base_url().as_str())
}

// Back to the pre-Hive config: the original file when it was backed up, or a
// stripped config (foreign providers and models untouched) when it was not.
fn restore_opencode_config_in(home: &Path) -> Result<(), String> {
  let path = opencode_config_path_in(home);
  if let Some(backup) = opencode_backup_path_in(home) {
    std::fs::copy(&backup, &path)
      .map_err(|e| format!("Impossibile scrivere {}: {e}", path.display()))?;
    std::fs::remove_file(&backup).map_err(|e| e.to_string())?;
    return Ok(());
  }
  if !path.exists() {
    return Ok(());
  }
  let raw = std::fs::read_to_string(&path)
    .map_err(|e| format!("Impossibile leggere {}: {e}", path.display()))?;
  let cleaned = strip_trailing_commas(&strip_jsonc_comments(&raw));
  let mut config: serde_json::Value = serde_json::from_str(&cleaned).map_err(|e| {
    format!("{} non è un JSON valido: {e}. Correggi il file e riprova.", path.display())
  })?;
  if let Some(root) = config.as_object_mut() {
    if let Some(providers) = root.get_mut("provider").and_then(|p| p.as_object_mut()) {
      providers.remove(OPENCODE_PROVIDER_ID);
    }
    if root.get("provider").and_then(|p| p.as_object()).is_some_and(|p| p.is_empty()) {
      root.remove("provider");
    }
    // Only a Hive model is ours; a model chosen for another provider stays.
    if root
      .get("model")
      .and_then(|v| v.as_str())
      .is_some_and(|m| m.starts_with(&format!("{OPENCODE_PROVIDER_ID}/")))
    {
      root.remove("model");
    }
    // $schema is only dropped when it is the URL this launcher writes; losing
    // it is harmless, and a foreign schema reference must stay.
    if root.get(OPENCODE_SCHEMA_KEY).and_then(|v| v.as_str()) == Some(OPENCODE_SCHEMA_URL) {
      root.remove(OPENCODE_SCHEMA_KEY);
    }
  }
  if config.as_object().is_some_and(|root| root.is_empty()) {
    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    return Ok(());
  }
  let mut rendered = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
  rendered.push('\n');
  std::fs::write(&path, rendered)
    .map_err(|e| format!("Impossibile scrivere {}: {e}", path.display()))
}

// Restore the pre-Hive global config and report whether a running desktop
// instance must be restarted to drop the Hive provider.
#[tauri::command]
pub fn restore_opencode() -> Result<AppRestartResult, String> {
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  restore_opencode_config_in(&home)?;
  Ok(AppRestartResult {
    restart_required: opencode_app_running(),
  })
}

// Back up and rewrite the global opencode config with the Hive provider and
// the selected model. Refuses to touch the file when it cannot be parsed.
fn write_opencode_config_in(home: &Path, model: &str, key: &str) -> Result<PathBuf, String> {
  let dir = home.join(".config").join("opencode");
  std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
  let path = opencode_config_path_in(home);

  let mut config = if path.exists() {
    let raw = std::fs::read_to_string(&path)
      .map_err(|e| format!("Impossibile leggere {}: {e}", path.display()))?;
    let cleaned = strip_trailing_commas(&strip_jsonc_comments(&raw));
    let parsed: serde_json::Value = serde_json::from_str(&cleaned).map_err(|e| {
      format!("{} non è un JSON valido: {e}. Correggi il file e riprova.", path.display())
    })?;
    // Backup una-tantum e solo se il file non è ancora il nostro: altrimenti il
    // secondo lancio (o un cambio di chiave) salva come "originale" il file
    // già su Hive e il ripristino non riporta più indietro nulla.
    if !opencode_configured_in(home) {
      let backup = opencode_backup_for(&path);
      if !backup.exists() {
        let _ = std::fs::write(&backup, raw.as_str());
      }
    }
    parsed
  } else {
    serde_json::json!({})
  };

  merge_hive_provider(&mut config, model, key);
  let mut rendered = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
  rendered.push('\n');
  std::fs::write(&path, rendered)
    .map_err(|e| format!("Impossibile scrivere {}: {e}", path.display()))?;
  Ok(path)
}

fn write_opencode_config(model: &str, key: &str) -> Result<(), String> {
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  write_opencode_config_in(&home, model, key).map(|_| ())
}

// Opens a desktop .app bundle via LaunchServices. `open` forwards the caller's
// environment to the app, so `env` pins the variables the app must see.
#[cfg(target_os = "macos")]
fn open_app_bundle(bundle: &Path, label: &str, env: &[(&str, &str)]) -> Result<(), String> {
  let mut command = Command::new("open");
  command.arg(bundle);
  for (name, value) in env {
    command.env(name, value);
  }
  command
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .map(|_| ())
    .map_err(|e| format!("Impossibile avviare {label}: {e}"))
}

// The codex home this launcher writes and reads. Pinned explicitly whenever a
// codex surface is launched, because `open` and the spawned shells forward the
// caller's environment: a CODEX_HOME inherited from the shell that started the
// bridge (e.g. an editor-managed codex runtime) would otherwise send codex to
// a different config than the one written here.
fn hive_codex_home() -> Option<PathBuf> {
  home_dir().map(|home| codex_dir_in(&home))
}

// True when the OpenCode desktop app is live. `pgrep -x` is case-sensitive and
// the bundle executable is "OpenCode", so the lowercase CLI is not matched.
#[cfg(target_os = "macos")]
fn opencode_app_running() -> bool {
  Command::new("pgrep")
    .args(["-x", "OpenCode"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .is_ok_and(|s| s.success())
}

// On Windows/Linux the app is a plain executable, so the process name is the
// detected binary's file name — deriving it here keeps the probe and the quit
// (which uses the same) in lockstep with whatever the installer named it.
#[cfg(not(target_os = "macos"))]
fn opencode_app_running() -> bool {
  let Some(bin) = opencode_app_path() else {
    return false;
  };
  let Some(name) = bin.file_name().and_then(|n| n.to_str()) else {
    return false;
  };
  desktop_process_running(&[name])
}

// Point the OpenCode desktop app at AI Hive: write the global config and report
// whether a running instance must be restarted, because the app reads that
// config only at startup.
#[tauri::command]
pub fn configure_opencode_app(model: String, key: String) -> Result<AppRestartResult, String> {
  let model = model.trim();
  let key = key.trim();
  if model.is_empty() {
    return Err("Nessun modello selezionato".into());
  }
  if key.is_empty() {
    return Err("Chiave Hive non salvata".into());
  }
  if opencode_app_path().is_none() {
    #[cfg(target_os = "macos")]
    let not_found = "OpenCode.app non trovato in /Applications: installalo e riprova.";
    #[cfg(not(target_os = "macos"))]
    let not_found = "OpenCode Desktop non trovato: installalo e riprova.";
    return Err(String::from(not_found));
  }
  write_opencode_config(model, key)?;
  Ok(AppRestartResult {
    restart_required: opencode_app_running(),
  })
}

#[tauri::command]
#[cfg(target_os = "macos")]
pub fn open_opencode_app() -> Result<(), String> {
  let bundle = opencode_app_path()
    .ok_or_else(|| String::from("OpenCode.app non trovato in /Applications: installalo e riprova."))?;
  open_app_bundle(&bundle, "OpenCode.app", &[])
}

#[tauri::command]
#[cfg(not(target_os = "macos"))]
pub fn open_opencode_app() -> Result<(), String> {
  let bin = opencode_app_path()
    .ok_or_else(|| String::from("OpenCode Desktop non trovato: installalo e riprova."))?;
  desktop_spawn_app(&bin, &[], "OpenCode Desktop")
}

// Quit the running instance (if any) and relaunch it so it loads the config
// written by configure_opencode_app.
#[cfg(target_os = "macos")]
fn quit_and_reopen_opencode() -> Result<(), String> {
  let bundle = opencode_app_path()
    .ok_or_else(|| String::from("OpenCode.app non trovato in /Applications: installalo e riprova."))?;
  if opencode_app_running() {
    Command::new("pkill")
      .args(["-x", "OpenCode"])
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .map_err(|e| format!("Impossibile chiudere OpenCode: {e}"))?;
    for _ in 0..10 {
      if !opencode_app_running() {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(500));
    }
  }
  open_app_bundle(&bundle, "OpenCode.app", &[])
}

#[cfg(not(target_os = "macos"))]
fn quit_and_reopen_opencode() -> Result<(), String> {
  let bin = opencode_app_path()
    .ok_or_else(|| String::from("OpenCode Desktop non trovato: installalo e riprova."))?;
  let name = bin
    .file_name()
    .and_then(|n| n.to_str())
    .map(str::to_owned)
    .ok_or_else(|| String::from("OpenCode Desktop non trovato: installalo e riprova."))?;
  if opencode_app_running() {
    desktop_process_quit(&[name.as_str()]);
    for _ in 0..10 {
      if !opencode_app_running() {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(500));
    }
  }
  desktop_spawn_app(&bin, &[], "OpenCode Desktop")
}

#[tauri::command]
#[cfg(target_os = "macos")]
pub fn restart_opencode_app() -> Result<(), String> {
  quit_and_reopen_opencode()
}

#[tauri::command]
#[cfg(not(target_os = "macos"))]
pub fn restart_opencode_app() -> Result<(), String> {
  quit_and_reopen_opencode()
}

fn launch_opencode_cli(
  app: &tauri::AppHandle,
  model: &str,
  key: &str,
  working_directory: &Path,
) -> Result<(), String> {
  let bin = find_service_binary("opencode")
    .ok_or_else(|| String::from("OpenCode CLI non trovata. Installala da https://opencode.ai/download."))?;
  write_opencode_config(model, key)?;
  launch_cli(app, "opencode", &bin, &[], &[], working_directory)
}

// Opens ChatGPT.app. The Hive settings (root config, model catalog, auth) are
// written by configure_chatgpt_app before the first launch.
#[tauri::command]
#[cfg(target_os = "macos")]
pub fn open_chatgpt_app() -> Result<(), String> {
  let bundle = chatgpt_app_bundle()
    .ok_or_else(|| String::from("ChatGPT.app non trovato in /Applications: installalo e riprova."))?;
  let codex_home = hive_codex_home().ok_or_else(|| "Home directory non trovata".to_string())?;
  let codex_home = codex_home.to_string_lossy();
  open_app_bundle(&bundle, "ChatGPT.app", &[("CODEX_HOME", codex_home.as_ref())])
}

#[tauri::command]
#[cfg(windows)]
pub fn open_chatgpt_app() -> Result<(), String> {
  let bin = chatgpt_app_bundle().ok_or_else(|| {
    String::from("L'app ChatGPT non è stata trovata: installala dal Microsoft Store e riprova.")
  })?;
  let codex_home = hive_codex_home().ok_or_else(|| "Home directory non trovata".to_string())?;
  let codex_home = codex_home.to_string_lossy();
  // CODEX_HOME points at %USERPROFILE%\.codex, which is also codex's own
  // default there, so the config lands correctly even if a Store launch drops
  // the forwarded variable.
  desktop_spawn_app(&bin, &[("CODEX_HOME", codex_home.as_ref())], "ChatGPT")
}

#[tauri::command]
#[cfg(target_os = "linux")]
pub fn open_chatgpt_app() -> Result<(), String> {
  Err(String::from("L'app ChatGPT non è disponibile su Linux."))
}

// ---------------------------------------------------------------------------
// ChatGPT.app (Codex desktop) su AI Hive
// ---------------------------------------------------------------------------
// The ChatGPT desktop app reads the same ~/.codex files as the CLI. Since the
// 26.x builds the Responses transport defaults to WebSocket (wss://<host>/api/v1/responses),
// which the Hive gateway does not implement, the app must use a custom
// provider with supports_websockets = false (built-in providers cannot be
// overridden). The provider carries the Hive key via experimental_bearer_token
// because custom providers ignore auth.json; auth.json (apikey mode) is still
// written so the app keeps a coherent auth state. A generated model catalog
// feeds the app picker. The original config.toml/auth.json are backed up
// (.hive.bak) so the previous setup can be restored.
const HIVE_CATALOG_FILE: &str = "hive-models.json";
const HIVE_CONFIG_BACKUP: &str = "config.toml.hive.bak";
const HIVE_AUTH_BACKUP: &str = "auth.json.hive.bak";
const HIVE_PROVIDER_ID: &str = "requrv-hive";

fn codex_dir_in(home: &Path) -> PathBuf {
  home.join(".codex")
}

fn codex_config_path_in(home: &Path) -> PathBuf {
  codex_dir_in(home).join("config.toml")
}

fn codex_config_backup_path_in(home: &Path) -> PathBuf {
  codex_dir_in(home).join(HIVE_CONFIG_BACKUP)
}

fn codex_auth_path_in(home: &Path) -> PathBuf {
  codex_dir_in(home).join("auth.json")
}

fn codex_auth_backup_path_in(home: &Path) -> PathBuf {
  codex_dir_in(home).join(HIVE_AUTH_BACKUP)
}

fn codex_catalog_path_in(home: &Path) -> PathBuf {
  codex_dir_in(home).join(HIVE_CATALOG_FILE)
}

// model_catalog_json requires Codex >= 0.134.0; older CLIs fail to start
// with an opaque config error, so check the version up front.
const CODEX_MIN_VERSION: &str = "0.134.0";

fn codex_version_ok(version: &str) -> bool {
  !version.is_empty()
    && compare_versions(version, CODEX_MIN_VERSION) >= std::cmp::Ordering::Equal
}

fn check_codex_version(bin: &Path) -> Result<(), String> {
  let output = Command::new(bin)
    .arg("--version")
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .output()
    .map_err(|e| format!("Impossibile verificare la versione di Codex: {e}"))?;
  let out = String::from_utf8_lossy(&output.stdout);
  // Output looks like "codex-cli 0.87.0"; the version is the last field.
  let version = out
    .split_whitespace()
    .last()
    .map(str::to_string)
    .unwrap_or_default();
  if !codex_version_ok(&version) {
    return Err(format!(
      "Codex {version} è troppo vecchio: serve almeno {CODEX_MIN_VERSION}. Aggiorna con: npm update -g @openai/codex"
    ));
  }
  Ok(())
}

// The catalog schema takes each reasoning level as a `ReasoningEffortPreset`
// object; the descriptions mirror the texts ChatGPT.app ships for its own
// models.
fn reasoning_preset(level: &str) -> serde_json::Value {
  let description = match level {
    "low" => "Fast responses with lighter reasoning",
    "medium" => "Balances speed and reasoning depth for everyday tasks",
    "high" => "Greater reasoning depth for complex problems",
    "xhigh" => "Extra high reasoning depth for complex problems",
    "max" => "Maximum reasoning depth for the hardest problems",
    _ => "Maximum reasoning with automatic task delegation",
  };
  serde_json::json!({ "effort": level, "description": description })
}

// Catalog entry for the app model picker. The Codex desktop engine's schema
// is strict, so mirror the full field set ollama ships to ChatGPT (models
// without thinking metadata get null/empty reasoning fields).
fn hive_catalog_entry(model: &str, priority: i64) -> serde_json::Value {
  let known = model == KNOWN_HIVE_MODEL;
  let context = hive_model_context(model);
  let inputs: &[&str] = if known { &KNOWN_HIVE_MODEL_CATALOG_INPUTS } else { &DEFAULT_HIVE_INPUTS };
  let levels: Vec<serde_json::Value> = if known {
    KNOWN_HIVE_MODEL_REASONING_LEVELS
      .iter()
      .map(|level| reasoning_preset(level))
      .collect()
  } else {
    Vec::new()
  };
  serde_json::json!({
    "slug": model,
    "display_name": model,
    "description": "Modello ReQurv AI Hive",
    "default_reasoning_level": known.then_some(KNOWN_HIVE_MODEL_DEFAULT_REASONING),
    "supported_reasoning_levels": levels,
    "shell_type": "unified_exec",
    "visibility": "list",
    "supported_in_api": true,
    "priority": priority,
    "additional_speed_tiers": [],
    "service_tiers": [],
    "default_service_tier": null,
    "availability_nux": null,
    "upgrade": null,
    "base_instructions": "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.",
    "model_messages": null,
    "include_skills_usage_instructions": true,
    "include_plugin_usage_instructions": true,
    "include_apps_usage_instructions": true,
    "supports_reasoning_summary_parameter": false,
    "supports_reasoning_summaries": false,
    "default_reasoning_summary": "auto",
    "support_verbosity": false,
    "default_verbosity": null,
    "apply_patch_tool_type": null,
    "web_search_tool_type": "text",
    "truncation_policy": { "mode": "tokens", "limit": 10_000 },
    "supports_parallel_tool_calls": true,
    "supports_image_detail_original": false,
    "context_window": context,
    "max_context_window": context,
    "auto_compact_token_limit": null,
    "effective_context_window_percent": 95,
    "experimental_supported_tools": [],
    "input_modalities": inputs,
    "supports_search_tool": true
  })
}

// Catalog for the app picker: only TEXT_GENERATION models; fall back to the
// full list if the gateway stops reporting the type (same rule as the UI).
fn build_hive_catalog(models: &[HiveModel]) -> serde_json::Value {
  let text: Vec<&HiveModel> = models.iter().filter(|m| m.model_type == "TEXT_GENERATION").collect();
  let picked: Vec<&HiveModel> = if text.is_empty() {
    models.iter().collect()
  } else {
    text
  };
  let entries = picked
    .iter()
    .enumerate()
    .map(|(i, m)| hive_catalog_entry(&m.id, i as i64))
    .collect::<Vec<_>>();
  serde_json::json!({ "models": entries })
}

// The Hive provider table when present in a parsed config.
fn hive_provider_table(table: &toml::Table) -> Option<&toml::Table> {
  table
    .get("model_providers")
    .and_then(|v| v.as_table())
    .and_then(|providers| providers.get(HIVE_PROVIDER_ID))
    .and_then(|v| v.as_table())
}

// True when the provider (or its legacy root openai_base_url) points at AI Hive.
fn hive_config_ours(table: &toml::Table) -> bool {
  table.get("openai_base_url").and_then(|v| v.as_str()) == Some(hive_openai_base_url().as_str())
    || hive_provider_table(table)
      .and_then(|p| p.get("base_url").and_then(|v| v.as_str()))
      == Some(hive_openai_base_url().as_str())
}

// True when the codex config is pointed at AI Hive by this launcher.
fn chatgpt_app_configured_in(home: &Path) -> bool {
  let Ok(raw) = std::fs::read_to_string(codex_config_path_in(home)) else {
    return false;
  };
  let Ok(table) = raw.parse::<toml::Table>() else {
    return false;
  };
  hive_config_ours(&table)
    && table
      .get("model_catalog_json")
      .and_then(|v| v.as_str())
      == Some(codex_catalog_path_in(home).to_string_lossy().as_ref())
}

// Back up (once) and rewrite the codex config, catalog and auth so the
// ChatGPT app talks to AI Hive. Every other root key is preserved; files that
// cannot be parsed are left untouched.
fn configure_chatgpt_app_in(home: &Path, model: &str, models: &[HiveModel], key: &str) -> Result<(), String> {
  let dir = codex_dir_in(home);
  std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
  // Read before writing anything: once config.toml holds the Hive provider,
  // this returns true and the user's files would never be backed up.
  let was_ours = chatgpt_app_configured_in(home);
  let catalog_path = codex_catalog_path_in(home);
  // An empty list means the caller could not fetch the models (e.g. the key
  // change path). Keeping the catalog on disk is right: overwriting it with
  // {"models": []} would empty the picker and drop the context windows.
  if !models.is_empty() {
    let catalog = build_hive_catalog(models);
    let rendered = serde_json::to_string_pretty(&catalog).map_err(|e| e.to_string())?;
    std::fs::write(&catalog_path, rendered + "\n")
      .map_err(|e| format!("Impossibile scrivere {}: {e}", catalog_path.display()))?;
  }

  let config_path = codex_config_path_in(home);
  let backup_path = codex_config_backup_path_in(home);
  let mut table: toml::Table = if config_path.exists() {
    let raw = std::fs::read_to_string(&config_path)
      .map_err(|e| format!("Impossibile leggere {}: {e}", config_path.display()))?;
    let parsed: toml::Table = raw.parse().map_err(|e| {
      format!("{} non è un TOML valido: {e}. Correggi il file e riprova.", config_path.display())
    })?;
    // Backup only while the config is not ours yet: this writer runs on every
    // CLI launch and on every key change, so a plain "!backup exists" guard
    // would snapshot the Hive config as the user's "original".
    if !backup_path.exists() && !was_ours {
      let _ = std::fs::write(&backup_path, raw.as_str());
    }
    parsed
  } else {
    toml::Table::new()
  };
  table.insert("model".into(), toml::Value::String(model.to_string()));
  table.insert("model_provider".into(), toml::Value::String(HIVE_PROVIDER_ID.into()));
  table.insert("model_catalog_json".into(), toml::Value::String(catalog_path.to_string_lossy().into_owned()));
  // Legacy layouts pointed the root openai_base_url at Hive; the provider
  // table supersedes it, so drop it when reconfiguring.
  table.remove("openai_base_url");
  let mut provider = toml::Table::new();
  provider.insert("name".into(), toml::Value::String("ReQurv AI Hive".into()));
  provider.insert("base_url".into(), toml::Value::String(hive_openai_base_url()));
  provider.insert("wire_api".into(), toml::Value::String("responses".into()));
  provider.insert("supports_websockets".into(), toml::Value::Boolean(false));
  provider.insert("experimental_bearer_token".into(), toml::Value::String(key.to_string()));
  // No `env_key`: codex refuses to start with "Missing environment variable"
  // when the provider declares one and the variable is absent (a CLI typed by
  // hand, or the app), and the bearer token above already authenticates both
  // surfaces.
  let providers = table
    .entry("model_providers")
    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
  let providers = providers
    .as_table_mut()
    .ok_or_else(|| "model_providers non modificabile in config.toml".to_string())?;
  providers.insert(HIVE_PROVIDER_ID.into(), toml::Value::Table(provider));
  let rendered = toml::to_string(&table).map_err(|e| e.to_string())?;
  std::fs::write(&config_path, rendered)
    .map_err(|e| format!("Impossibile scrivere {}: {e}", config_path.display()))?;

  // The app authenticates through auth.json: the Hive key goes in apikey mode,
  // keeping the previous content in .bak for the restore.
  let auth_path = codex_auth_path_in(home);
  let auth_backup = codex_auth_backup_path_in(home);
  // Same reasoning as the config backup: an auth.json this launcher already
  // wrote must not become the user's "original" on the next launch.
  if auth_path.exists() && !auth_backup.exists() && !was_ours {
    let raw = std::fs::read_to_string(&auth_path)
      .map_err(|e| format!("Impossibile leggere {}: {e}", auth_path.display()))?;
    let _ = std::fs::write(&auth_backup, raw.as_str());
  }
  let auth = serde_json::json!({
    "OPENAI_API_KEY": key,
    "auth_mode": "apikey"
  });
  let rendered = serde_json::to_string_pretty(&auth).map_err(|e| e.to_string())?;
  std::fs::write(&auth_path, rendered + "\n")
    .map_err(|e| format!("Impossibile scrivere {}: {e}", auth_path.display()))?;
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&auth_path, std::fs::Permissions::from_mode(0o600));
  }

  Ok(())
}

// Undo the Hive configuration: restore the backed-up files, or strip the root
// keys and the apikey auth managed by this launcher when no backup exists. A
// ChatGPT OAuth auth.json is never touched.
fn restore_chatgpt_app_in(home: &Path, key: &str) -> Result<(), String> {
  let config_path = codex_config_path_in(home);
  let backup_path = codex_config_backup_path_in(home);
  let mut strip_catalog = false;

  if config_path.exists() || backup_path.exists() {
    if backup_path.exists() {
      let raw = std::fs::read_to_string(&backup_path)
        .map_err(|e| format!("Impossibile leggere {}: {e}", backup_path.display()))?;
      std::fs::write(&config_path, raw)
        .map_err(|e| format!("Impossibile scrivere {}: {e}", config_path.display()))?;
      std::fs::remove_file(&backup_path).map_err(|e| e.to_string())?;
      strip_catalog = true;
    } else {
      let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Impossibile leggere {}: {e}", config_path.display()))?;
      let mut table: toml::Table = raw.parse().map_err(|e| {
        format!("{} non è un TOML valido: {e}. Correggi il file e riprova.", config_path.display())
      })?;
      let is_hive = hive_config_ours(&table);
      if is_hive {
        table.remove("model");
        table.remove("openai_base_url");
        table.remove("model_catalog_json");
        if table.get("model_provider").and_then(|v| v.as_str()) == Some(HIVE_PROVIDER_ID) {
          table.remove("model_provider");
        }
        if let Some(providers) = table.get_mut("model_providers").and_then(|v| v.as_table_mut()) {
          let ours = providers
            .get(HIVE_PROVIDER_ID)
            .and_then(|p| p.as_table())
            .and_then(|p| p.get("base_url").and_then(|v| v.as_str()))
            == Some(hive_openai_base_url().as_str());
          if ours {
            providers.remove(HIVE_PROVIDER_ID);
          }
          if providers.is_empty() {
            table.remove("model_providers");
          }
        }
        if table.is_empty() {
          std::fs::remove_file(&config_path).map_err(|e| e.to_string())?;
        } else {
          let rendered = toml::to_string(&table).map_err(|e| e.to_string())?;
          std::fs::write(&config_path, rendered)
            .map_err(|e| format!("Impossibile scrivere {}: {e}", config_path.display()))?;
        }
        strip_catalog = true;
      }
    }
  }

  let auth_path = codex_auth_path_in(home);
  let auth_backup = codex_auth_backup_path_in(home);
  if auth_path.exists() {
    if auth_backup.exists() {
      let raw = std::fs::read_to_string(&auth_backup)
        .map_err(|e| format!("Impossibile leggere {}: {e}", auth_backup.display()))?;
      std::fs::write(&auth_path, raw)
        .map_err(|e| format!("Impossibile scrivere {}: {e}", auth_path.display()))?;
      std::fs::remove_file(&auth_backup).map_err(|e| e.to_string())?;
    } else if let Ok(raw) = std::fs::read_to_string(&auth_path) {
      // Without a backup the auth was created by this launcher: remove it only
      // while it still holds the Hive key, never a user login or other key.
      let is_ours = serde_json::from_str::<serde_json::Value>(&raw)
        .map(|v| {
          v.get("auth_mode").and_then(|m| m.as_str()) == Some("apikey")
            && v.get("OPENAI_API_KEY").and_then(|k| k.as_str()) == Some(key)
        })
        .unwrap_or(false);
      if is_ours {
        std::fs::remove_file(&auth_path).map_err(|e| e.to_string())?;
      }
    }
  }

  if strip_catalog {
    let _ = std::fs::remove_file(codex_catalog_path_in(home));
  }

  // Files written by earlier versions of the launcher (dedicated CLI profile
  // and its own catalog): nothing reads them any more, so the restore takes
  // them away instead of leaving them behind in ~/.codex.
  for legacy in ["hive.config.toml", "hive.config.bak", "hive-cli-models.json"] {
    let _ = std::fs::remove_file(codex_dir_in(home).join(legacy));
  }

  Ok(())
}

// The model catalog is read at startup, so a running instance keeps the old
// models until it is restarted.
#[cfg(target_os = "macos")]
fn chatgpt_app_running() -> bool {
  Command::new("pgrep")
    .args(["-x", "ChatGPT"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .is_ok_and(|s| s.success())
}

#[cfg(windows)]
fn chatgpt_app_running() -> bool {
  let Some(bin) = chatgpt_app_bundle() else {
    return false;
  };
  let Some(name) = bin.file_name().and_then(|n| n.to_str()) else {
    return false;
  };
  desktop_process_running(&[name])
}

#[cfg(target_os = "linux")]
fn chatgpt_app_running() -> bool {
  false
}

// Quit ChatGPT (gracefully, then forcefully) and relaunch it so the app picks
// up the new model catalog.
#[cfg(target_os = "macos")]
fn quit_and_reopen_chatgpt() -> Result<(), String> {
  let bundle = chatgpt_app_bundle()
    .ok_or_else(|| String::from("ChatGPT.app non trovato in /Applications: installalo e riprova."))?;
  if chatgpt_app_running() {
    Command::new("osascript")
      .args(["-e", "tell application \"ChatGPT\" to quit"])
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .map(|_| ())
      .map_err(|e| format!("Impossibile chiudere ChatGPT: {e}"))?;
    // Graceful quit can take a moment; give it a chance before forcing.
    for _ in 0..20 {
      if !chatgpt_app_running() {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if chatgpt_app_running() {
      Command::new("pkill")
        .args(["-x", "ChatGPT"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Impossibile chiudere ChatGPT: {e}"))?;
      for _ in 0..10 {
        if !chatgpt_app_running() {
          break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
      }
    }
  }
  let codex_home = hive_codex_home().ok_or_else(|| "Home directory non trovata".to_string())?;
  let codex_home = codex_home.to_string_lossy();
  open_app_bundle(&bundle, "ChatGPT.app", &[("CODEX_HOME", codex_home.as_ref())])
}

#[cfg(windows)]
fn quit_and_reopen_chatgpt() -> Result<(), String> {
  let bin = chatgpt_app_bundle().ok_or_else(|| {
    String::from("L'app ChatGPT non è stata trovata: installala dal Microsoft Store e riprova.")
  })?;
  let name = bin
    .file_name()
    .and_then(|n| n.to_str())
    .map(str::to_owned)
    .ok_or_else(|| String::from("L'app ChatGPT non è stata trovata."))?;
  if chatgpt_app_running() {
    desktop_process_quit(&[name.as_str()]);
    for _ in 0..10 {
      if !chatgpt_app_running() {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(500));
    }
  }
  let codex_home = hive_codex_home().ok_or_else(|| "Home directory non trovata".to_string())?;
  let codex_home = codex_home.to_string_lossy();
  desktop_spawn_app(&bin, &[("CODEX_HOME", codex_home.as_ref())], "ChatGPT")
}

#[cfg(target_os = "linux")]
fn quit_and_reopen_chatgpt() -> Result<(), String> {
  Err(String::from("L'app ChatGPT non è disponibile su Linux."))
}

#[derive(Serialize)]
pub struct AppRestartResult {
  pub restart_required: bool,
}

// Point ChatGPT.app at AI Hive (config, catalog, auth) and report whether a
// running instance needs a restart to load the new catalog.
#[tauri::command]
pub async fn configure_chatgpt_app(model: String, key: String, models: Vec<HiveModel>) -> Result<AppRestartResult, String> {
  let model = model.trim();
  let key = key.trim();
  if model.is_empty() {
    return Err("Nessun modello selezionato".into());
  }
  if key.is_empty() {
    return Err("Chiave Hive non salvata".into());
  }
  if models.is_empty() {
    return Err("Nessun modello disponibile da AI Hive".into());
  }
  assert_responses_available(key).await?;
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  configure_chatgpt_app_in(&home, model, &models, key)?;
  Ok(AppRestartResult {
    restart_required: chatgpt_app_running(),
  })
}

#[tauri::command]
pub fn restart_chatgpt_app() -> Result<(), String> {
  quit_and_reopen_chatgpt()
}

// Restore the original ChatGPT setup (config.toml, auth.json, catalog) and
// report whether a running instance needs a restart to pick it up.
#[tauri::command]
pub fn restore_chatgpt_app(app: tauri::AppHandle) -> Result<AppRestartResult, String> {
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  let key = get_hive_key(app).ok().flatten().unwrap_or_default();
  restore_chatgpt_app_in(&home, &key)?;
  Ok(AppRestartResult {
    restart_required: chatgpt_app_running(),
  })
}

// ---------------------------------------------------------------------------
// Claude Desktop su AI Hive
// ---------------------------------------------------------------------------
// Third-party ("3p") deployment mode: the managed profile in configLibrary
// declares the gateway URL, credential and model rows; the app sends the
// claude-* slot to POST /v1/messages on the same Anthropic gateway Claude Code
// uses, and the gateway resolves it to the real model.
// Fixed profile id (same convention as Ollama's launcher); opaque to the app.
const CLAUDE_DESKTOP_PROFILE_ID: &str = "00000000-0000-4000-8000-000000000114";
const CLAUDE_DESKTOP_PROFILE_NAME: &str = "ReQurv AI Hive";
// Claude Desktop's 3p mode drops any inferenceModels entry whose name is not
// an Anthropic model id, so the profile advertises an Anthropic id and the
// gateway maps it to the selected Hive model (org alias or claude-* fallback);
// labelOverride is what the user actually sees.
const CLAUDE_DESKTOP_SLOT: &str = "claude-sonnet-5";
const CLAUDE_DESKTOP_BACKUP: &str = "hive.bak";

fn claude_desktop_app_path() -> Option<PathBuf> {
  #[cfg(target_os = "macos")]
  {
    let candidates = [
      PathBuf::from("/Applications/Claude.app"),
      home_dir()?.join("Applications/Claude.app"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
  }
  // Windows: the NSIS installer puts the Electron app under
  // %LOCALAPPDATA%\Program\Claude (or a Program Files root for system installs).
  #[cfg(windows)]
  {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
      roots.push(Path::new(&local).join("Program").join("Claude"));
    }
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
      if let Ok(dir) = std::env::var(var) {
        roots.push(PathBuf::from(dir).join("Claude"));
      }
    }
    let candidates = roots.into_iter().map(|root| root.join("Claude.exe")).collect::<Vec<_>>();
    candidates.into_iter().find(|c| c.is_file())
  }
  // Linux: prefer the .deb/.rpm's .desktop entry (it names the real binary);
  // the bare `claude` on PATH is the CLI, so it is not a candidate here.
  #[cfg(target_os = "linux")]
  {
    desktop_app_from_desktop_entry("claude").or_else(|| {
      let candidates = [
        PathBuf::from("/usr/bin/claude-desktop"),
        PathBuf::from("/usr/local/bin/claude-desktop"),
        PathBuf::from("/opt/claude-desktop/claude"),
      ];
      candidates.into_iter().find(|c| c.is_file())
    })
  }
}

struct ClaudeDesktopPaths {
  // deploymentMode here selects which profile root the app boots into.
  normal_config: PathBuf,
  // 3p profile root config (deploymentMode must be "3p" there too).
  third_party_config: PathBuf,
  // configLibrary metadata: appliedId selects the active profile.
  meta: PathBuf,
  // The managed gateway profile.
  profile: PathBuf,
}

fn claude_desktop_paths_in(home: &Path) -> ClaudeDesktopPaths {
  let support = application_support_root(home);
  let normal = support.join("Claude");
  let third_party = support.join("Claude-3p");
  let library = third_party.join("configLibrary");
  ClaudeDesktopPaths {
    normal_config: normal.join("claude_desktop_config.json"),
    third_party_config: third_party.join("claude_desktop_config.json"),
    meta: library.join("_meta.json"),
    profile: library.join(format!("{CLAUDE_DESKTOP_PROFILE_ID}.json")),
  }
}

fn backup_path_for(path: &Path) -> PathBuf {
  path.with_file_name(format!(
    "{}.{}",
    path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
    CLAUDE_DESKTOP_BACKUP
  ))
}

// Back up a file once (first .hive.bak wins) so restore always returns to
// the pre-Hive state, even across repeated configure/restore cycles.
fn backup_once(path: &Path) -> Result<(), String> {
  if !path.exists() {
    return Ok(());
  }
  let backup = backup_path_for(path);
  if !backup.exists() {
    std::fs::copy(path, &backup)
      .map_err(|e| format!("Backup di {} non riuscito: {e}", path.display()))?;
  }
  Ok(())
}

fn read_json_allow_missing(path: &Path) -> Result<serde_json::Value, String> {
  match std::fs::read_to_string(path) {
    Ok(raw) => serde_json::from_str(&raw)
      .map_err(|e| format!("{} non è JSON valido: {e}", path.display())),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
    Err(e) => Err(format!("Impossibile leggere {}: {e}", path.display())),
  }
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  let rendered = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
  std::fs::write(path, rendered + "\n").map_err(|e| e.to_string())
}

fn set_deployment_mode(path: &Path, mode: &str) -> Result<(), String> {
  let mut cfg = read_json_allow_missing(path)?;
  if !cfg.is_object() {
    return Err(format!("{} non è un oggetto JSON", path.display()));
  }
  cfg["deploymentMode"] = serde_json::json!(mode);
  write_json(path, &cfg)
}

fn claude_desktop_profile_value(model: &str, key: &str) -> serde_json::Value {
  serde_json::json!({
    "inferenceProvider": "gateway",
    "inferenceCredentialKind": "static",
    "inferenceGatewayApiKey": key,
    "inferenceGatewayAuthScheme": "x-api-key",
    "inferenceGatewayBaseUrl": hive_anthropic_base_url(),
    "inferenceModels": [{
      "name": CLAUDE_DESKTOP_SLOT,
      "labelOverride": format!("{model} (ReQurv)"),
      "anthropicFamilyTier": "sonnet",
      "isFamilyDefault": true,
      "maxEffort": "max"
    }]
  })
}

// True when the applied profile is ours (right id + right gateway URL), so
// the UI can offer the restore button.
fn claude_desktop_configured_in(home: &Path) -> bool {
  let paths = claude_desktop_paths_in(home);
  let Ok(meta) = read_json_allow_missing(&paths.meta) else {
    return false;
  };
  if meta.get("appliedId") != Some(&serde_json::json!(CLAUDE_DESKTOP_PROFILE_ID)) {
    return false;
  }
  let Ok(profile) = read_json_allow_missing(&paths.profile) else {
    return false;
  };
  profile.get("inferenceGatewayBaseUrl") == Some(&serde_json::json!(hive_anthropic_base_url()))
}

fn configure_claude_desktop_in(home: &Path, model: &str, key: &str) -> Result<(), String> {
  let paths = claude_desktop_paths_in(home);
  for path in [
    &paths.normal_config,
    &paths.third_party_config,
    &paths.meta,
    &paths.profile,
  ] {
    backup_once(path)?;
  }
  set_deployment_mode(&paths.normal_config, "3p")?;
  set_deployment_mode(&paths.third_party_config, "3p")?;
  // Merge into any existing meta: other 3p profiles must survive.
  let mut meta = read_json_allow_missing(&paths.meta)?;
  if !meta.is_object() {
    meta = serde_json::json!({});
  }
  if !meta.get("entries").and_then(|e| e.as_array()).is_some() {
    meta["entries"] = serde_json::json!([]);
  }
  let entries = meta["entries"].as_array_mut().unwrap();
  entries.retain(|e| e.get("id") != Some(&serde_json::json!(CLAUDE_DESKTOP_PROFILE_ID)));
  entries.push(serde_json::json!({
    "id": CLAUDE_DESKTOP_PROFILE_ID,
    "name": CLAUDE_DESKTOP_PROFILE_NAME
  }));
  meta["appliedId"] = serde_json::json!(CLAUDE_DESKTOP_PROFILE_ID);
  write_json(&paths.meta, &meta)?;
  write_json(&paths.profile, &claude_desktop_profile_value(model, key))?;
  Ok(())
}

fn restore_claude_desktop_in(home: &Path) -> Result<(), String> {
  let paths = claude_desktop_paths_in(home);
  // Restore every backup taken at configure time: the original content (and
  // deployment mode) comes back exactly as it was.
  let mut restored = Vec::new();
  for path in [
    &paths.normal_config,
    &paths.third_party_config,
    &paths.meta,
    &paths.profile,
  ] {
    let backup = backup_path_for(path);
    if backup.exists() {
      std::fs::copy(&backup, path)
        .map_err(|e| format!("Ripristino di {} non riuscito: {e}", path.display()))?;
      std::fs::remove_file(&backup).map_err(|e| e.to_string())?;
      restored.push(path);
    }
  }
  // Fallbacks for files configure created from scratch (no backup existed):
  // drop the managed profile when it is ours, and undo the 3p switch we made
  // in configs that did not exist before.
  if claude_desktop_our_profile(&paths.profile) {
    let _ = std::fs::remove_file(&paths.profile);
  }
  for path in [&paths.normal_config, &paths.third_party_config] {
    if !restored.contains(&path) && path.exists() {
      let cfg = read_json_allow_missing(path)?;
      if cfg.get("deploymentMode") == Some(&serde_json::json!("3p")) {
        set_deployment_mode(path, "1p")?;
      }
    }
  }
  if paths.meta.exists() {
    let mut meta = read_json_allow_missing(&paths.meta)?;
    if let Some(entries) = meta.get_mut("entries").and_then(|e| e.as_array_mut()) {
      entries.retain(|e| e.get("id") != Some(&serde_json::json!(CLAUDE_DESKTOP_PROFILE_ID)));
    }
    if meta.get("appliedId") == Some(&serde_json::json!(CLAUDE_DESKTOP_PROFILE_ID)) {
      meta.as_object_mut().unwrap().remove("appliedId");
    }
    write_json(&paths.meta, &meta)?;
  }
  Ok(())
}

fn claude_desktop_our_profile(path: &Path) -> bool {
  read_json_allow_missing(path)
    .map(|p| {
      p.get("inferenceGatewayBaseUrl") == Some(&serde_json::json!(hive_anthropic_base_url()))
    })
    .unwrap_or(false)
}

// The gateway must be reachable and accept the key before we rewrite the
// app's profile, otherwise Claude Desktop would be left without a working
// model.
async fn assert_claude_gateway_available(key: &str) -> Result<(), String> {
  let url = format!("{}/v1/models", hive_anthropic_base_url());
  let response = reqwest::Client::new()
    .get(&url)
    .header("x-api-key", key.trim())
    .timeout(std::time::Duration::from_secs(10))
    .send()
    .await
    .map_err(|e| {
      format!(
        "Gateway Claude non raggiungibile su {url}: {e}. Avvia il proxy locale (bun dev) e riprova."
      )
    })?;
  let status = response.status();
  if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
    return Err("Chiave Hive rifiutata dal gateway Claude (401/403).".into());
  }
  if !status.is_success() {
    return Err(format!("Gateway Claude ha risposto con lo stato {status}"));
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn claude_desktop_running() -> bool {
  Command::new("pgrep")
    .args(["-f", "Claude.app/Contents/MacOS/Claude"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .map(|s| s.success())
    .unwrap_or(false)
}

// On Windows/Linux the app is a plain executable, so its process name is the
// detected binary's file name (mirrors the OpenCode probe).
#[cfg(not(target_os = "macos"))]
fn claude_desktop_running() -> bool {
  let Some(bin) = claude_desktop_app_path() else {
    return false;
  };
  let Some(name) = bin.file_name().and_then(|n| n.to_str()) else {
    return false;
  };
  desktop_process_running(&[name])
}

#[cfg(target_os = "macos")]
fn quit_claude_desktop() {
  Command::new("osascript")
    .args(["-e", "tell application \"Claude\" to quit"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .ok();
  for _ in 0..20 {
    if !claude_desktop_running() {
      return;
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
  }
  Command::new("pkill")
    .args(["-f", "Claude.app/Contents/MacOS/Claude"])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .ok();
  for _ in 0..10 {
    if !claude_desktop_running() {
      return;
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
  }
}

#[cfg(not(target_os = "macos"))]
fn quit_claude_desktop() {
  let Some(bin) = claude_desktop_app_path() else {
    return;
  };
  let Some(name) = bin.file_name().and_then(|n| n.to_str()) else {
    return;
  };
  desktop_process_quit(&[name]);
  for _ in 0..10 {
    if !claude_desktop_running() {
      return;
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
  }
}

#[cfg(target_os = "macos")]
fn open_claude_desktop() -> Result<(), String> {
  let bundle = claude_desktop_app_path()
    .ok_or_else(|| String::from("Claude.app non trovato: installalo da https://claude.com/download."))?;
  open_app_bundle(&bundle, "Claude.app", &[])
}

#[cfg(not(target_os = "macos"))]
fn open_claude_desktop() -> Result<(), String> {
  let bin = claude_desktop_app_path()
    .ok_or_else(|| String::from("Claude Desktop non trovato: installalo da https://claude.com/download."))?;
  desktop_spawn_app(&bin, &[], "Claude Desktop")
}

// Claude persists its settings while shutting down: the profile must be
// re-applied AFTER the process exits, otherwise its last write can restore
// stale gateway values (same ordering as Ollama's launcher).
fn restart_claude_desktop_with(reapply: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  if claude_desktop_running() {
    quit_claude_desktop();
  }
  reapply(&home)?;
  open_claude_desktop()
}

#[tauri::command]
pub async fn configure_claude_desktop(model: String, key: String) -> Result<AppRestartResult, String> {
  let model = model.trim();
  let key = key.trim();
  if model.is_empty() {
    return Err("Nessun modello selezionato".into());
  }
  if key.is_empty() {
    return Err("Chiave Hive non salvata".into());
  }
  assert_claude_gateway_available(key).await?;
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  configure_claude_desktop_in(&home, model, key)?;
  Ok(AppRestartResult {
    restart_required: claude_desktop_running(),
  })
}

#[tauri::command]
pub fn restart_claude_desktop(model: String, key: String) -> Result<(), String> {
  restart_claude_desktop_with(|home| configure_claude_desktop_in(home, model.trim(), key.trim()))
}

// Restart after a restore: re-apply the restored state after the app exits so
// the shutdown persistence cannot bring the Hive profile back.
#[tauri::command]
pub fn restart_claude_desktop_restored() -> Result<(), String> {
  restart_claude_desktop_with(restore_claude_desktop_in)
}

#[tauri::command]
pub fn restore_claude_desktop() -> Result<AppRestartResult, String> {
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  restore_claude_desktop_in(&home)?;
  Ok(AppRestartResult {
    restart_required: claude_desktop_running(),
  })
}

#[tauri::command]
pub fn open_claude_desktop_app() -> Result<(), String> {
  open_claude_desktop()
}

async fn launch_codex_cli(
  app: &tauri::AppHandle,
  model: &str,
  key: &str,
  models: &[HiveModel],
  working_directory: &Path,
) -> Result<(), String> {
  let bin = find_service_binary("codex")
    .or_else(codex_app_binary)
    .ok_or_else(|| String::from("Codex non è installato. Scaricalo da https://chatgpt.com/codex."))?;

  check_codex_version(&bin)?;

  assert_responses_available(key).await?;
  // An empty catalog would replace the models ChatGPT.app already knows.
  if models.is_empty() {
    return Err("Nessun modello disponibile da AI Hive".into());
  }

  // One configuration for both surfaces: the CLI reads ~/.codex/config.toml
  // like ChatGPT.app does, so a codex typed by hand also goes to AI Hive. The
  // catalog travels with it (context window, modalities, reasoning levels).
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  configure_chatgpt_app_in(&home, model, models, key)?;

  // CODEX_HOME is pinned: the shell may carry one from another tool (an
  // editor-managed codex runtime), and codex would then read a config this
  // launcher never wrote. The provider itself authenticates with the bearer
  // token in config.toml, so no key env is needed.
  let codex_home = hive_codex_home().ok_or_else(|| "Home directory non trovata".to_string())?;
  let codex_home = codex_home.to_string_lossy();
  launch_cli(app, "codex", &bin, &[], &[("CODEX_HOME", codex_home.as_ref())], working_directory)
}

// Claude Code speaks the Anthropic Messages API, so the launcher only sets
// env vars and never writes to ~/.claude: ANTHROPIC_BASE_URL routes the client
// through the gateway (it appends /v1/messages), ANTHROPIC_API_KEY is sent as
// the x-api-key header in place of a Claude subscription, and ANTHROPIC_MODEL
// pins the selected Hive model. No file is persisted, so nothing to restore.
// ANTHROPIC_AUTH_TOKEN is the only other credential the client accepts, and
// it is not used: with the token alone (and no API key) 2.1.289 answers "Not
// logged in", so the API key is the only working credential here.
// The gateway configuration for Claude Code: the same env vars the Hermes
// Agent mode uses, because both run the Claude Agent SDK. The context window
// is the real limit for the known model (conservative default otherwise):
// the model is not in the client's catalog, so auto-compact would assume 200k.
// Every model tier (opus/sonnet/haiku) and the subagent model are pinned to
// the selected Hive model: otherwise Claude Code sends its internal claude-*
// defaults for background work and subagents, which the gateway does not know.
fn claude_hive_env(model: &str, key: &str) -> Vec<(&'static str, String)> {
  vec![
    ("ANTHROPIC_BASE_URL", hive_anthropic_base_url()),
    ("ANTHROPIC_API_KEY", key.to_string()),
    ("ANTHROPIC_MODEL", model.to_string()),
    ("ANTHROPIC_DEFAULT_OPUS_MODEL", model.to_string()),
    ("ANTHROPIC_DEFAULT_SONNET_MODEL", model.to_string()),
    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", model.to_string()),
    ("CLAUDE_CODE_SUBAGENT_MODEL", model.to_string()),
    ("CLAUDE_CODE_MAX_CONTEXT_TOKENS", hive_model_context(model).to_string()),
  ]
}

// Claude Code's only global settings file; the client applies its env block
// over the process environment, so persisting it here makes the configuration
// survive the launcher instead of dying with the terminal that started it.
fn claude_code_settings_path_in(home: &Path) -> PathBuf {
  home.join(".claude").join("settings.json")
}

// Merge the Hive env into the user settings: every existing top-level key and
// every non-Hive env entry survives. Refuses to write when the file is not
// valid JSON, so a broken settings.json is never overwritten.
fn write_claude_code_settings_in(home: &Path, model: &str, key: &str) -> Result<(), String> {
  let path = claude_code_settings_path_in(home);
  std::fs::create_dir_all(home.join(".claude")).map_err(|e| e.to_string())?;
  let mut settings = if path.exists() {
    let raw = std::fs::read_to_string(&path)
      .map_err(|e| format!("Impossibile leggere {}: {e}", path.display()))?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
      format!("{} non è un JSON valido: {e}. Correggi il file e riprova.", path.display())
    })?;
    // Solo se il file non è ancora il nostro: un secondo lancio (o un cambio
    // di chiave) deve trovare il backup già fatto, non sostituirlo con la
    // versione su Hive.
    if !claude_code_configured_in(home) {
      backup_once(&path)?;
    }
    parsed
  } else {
    serde_json::json!({})
  };
  let Some(root) = settings.as_object_mut() else {
    return Err(format!("{} non è un oggetto JSON", path.display()));
  };
  let mut env = root
    .get("env")
    .and_then(|v| v.as_object())
    .cloned()
    .unwrap_or_default();
  for (name, value) in claude_hive_env(model, key) {
    env.insert(name.to_string(), serde_json::Value::String(value));
  }
  // Settings files are merged, not replaced: a user token left here would be
  // sent as a Bearer header and win over ANTHROPIC_API_KEY.
  env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), serde_json::Value::String(String::new()));
  root.insert("env".to_string(), serde_json::Value::Object(env));
  write_json(&path, &settings)?;
  // Never fatal: a failure here only brings the consent prompt back.
  let _ = approve_claude_custom_api_key_in(home, key);
  Ok(())
}

// Claude Code asks whether to use an ANTHROPIC_API_KEY it has not seen yet,
// once per distinct key, and blocks the session until answered (it also stops
// using the key when the answer is "No", which is why a refusal used to break
// the session). The answer lives in ~/.claude.json as
// customApiKeyResponses.approved, keyed by the last 20 characters of the key
// (its own fingerprint: `key.trim().slice(-20)`). Recording the approval here,
// every time the key is written, keeps the launcher from putting that prompt
// in front of the user at every start and after every key change. An
// unreadable or non-object file is left alone: it belongs to Claude, and the
// prompt returning is better than losing its state.
fn approve_claude_custom_api_key_in(home: &Path, key: &str) -> Result<(), String> {
  let key = key.trim();
  if key.is_empty() {
    return Ok(());
  }
  let fingerprint: String = key
    .chars()
    .rev()
    .take(20)
    .collect::<Vec<char>>()
    .into_iter()
    .rev()
    .collect();
  let path = home.join(".claude.json");
  let mut config: serde_json::Value = if path.exists() {
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if raw.trim().is_empty() {
      serde_json::json!({})
    } else {
      serde_json::from_str(&raw).map_err(|e| e.to_string())?
    }
  } else {
    serde_json::json!({})
  };
  let Some(root) = config.as_object_mut() else {
    return Err(format!("{} non è un oggetto JSON", path.display()));
  };
  let responses = root
    .entry("customApiKeyResponses")
    .or_insert_with(|| serde_json::json!({ "approved": [], "rejected": [] }));
  if !responses.is_object() {
    *responses = serde_json::json!({ "approved": [], "rejected": [] });
  }
  let responses = responses.as_object_mut().expect("objecto appena forzato");
  // A previous "No" for this same key must not outrank the approval.
  if let Some(rejected) = responses
    .get_mut("rejected")
    .and_then(|list| list.as_array_mut())
  {
    rejected.retain(|value| value.as_str() != Some(fingerprint.as_str()));
  }
  let approved = responses
    .entry("approved")
    .or_insert_with(|| serde_json::json!([]));
  if !approved.is_array() {
    *approved = serde_json::json!([]);
  }
  let approved = approved.as_array_mut().expect("array appena forzato");
  if !approved
    .iter()
    .any(|value| value.as_str() == Some(fingerprint.as_str()))
  {
    approved.push(serde_json::Value::String(fingerprint));
  }
  write_json(&path, &config)
}


// True when the user settings are the ones this launcher wrote.
fn claude_code_configured_in(home: &Path) -> bool {
  read_json_allow_missing(&claude_code_settings_path_in(home))
    .map(|settings| {
      settings.get("env").and_then(|e| e.get("ANTHROPIC_BASE_URL")).and_then(|v| v.as_str())
        == Some(hive_anthropic_base_url().as_str())
    })
    .unwrap_or(false)
}

fn restore_claude_code_settings_in(home: &Path) -> Result<(), String> {
  let path = claude_code_settings_path_in(home);
  let backup = backup_path_for(&path);
  if backup.exists() {
    std::fs::copy(&backup, &path)
      .map_err(|e| format!("Impossibile scrivere {}: {e}", path.display()))?;
    std::fs::remove_file(&backup).map_err(|e| e.to_string())?;
    return Ok(());
  }
  if !path.exists() {
    return Ok(());
  }
  let mut settings = read_json_allow_missing(&path)?;
  if !claude_code_configured_in(home) {
    return Ok(());
  }
  if let Some(root) = settings.as_object_mut() {
    if let Some(env) = root.get_mut("env").and_then(|e| e.as_object_mut()) {
      for name in claude_hive_env("", "").iter().map(|(name, _)| *name) {
        env.remove(name);
      }
      env.remove("ANTHROPIC_AUTH_TOKEN");
      if env.is_empty() {
        root.remove("env");
      }
    }
  }
  if settings.as_object().is_some_and(|root| root.is_empty()) {
    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    return Ok(());
  }
  write_json(&path, &settings)
}

#[tauri::command]
pub fn restore_claude_code_cli() -> Result<(), String> {
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  restore_claude_code_settings_in(&home)
}

async fn launch_claude_cli(
  app: &tauri::AppHandle,
  model: &str,
  key: &str,
  working_directory: &Path,
) -> Result<(), String> {
  let bin = find_service_binary("claude")
    .ok_or_else(|| String::from("Claude Code non è installato. Installalo con npm install -g @anthropic-ai/claude-code."))?;

  assert_messages_available(key, "Claude Code").await?;

  // Persist the routing in ~/.claude/settings.json: the client applies it over
  // the process environment, so a claude started by hand (no Bridge, no
  // exported vars) also goes to AI Hive.
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  write_claude_code_settings_in(&home, model, key)?;

  let hive_env = claude_hive_env(model, key);
  let env: Vec<(&str, &str)> = hive_env.iter().map(|(k, v)| (*k, v.as_str())).collect();
  launch_cli(app, "claude", &bin, &[], &env, working_directory)
}

// Launch the Hermes Agent CLI against AI Hive. It reads the same config.yaml
// as the desktop app, so the launcher only persists the ReQurv provider and
// the selected model and then runs `hermes` with no arguments or environment.
async fn launch_hermes_cli(
  app: &tauri::AppHandle,
  model: &str,
  key: &str,
  working_directory: &Path,
) -> Result<(), String> {
  let bin = hermes_cli_path()
    .ok_or_else(|| format!("Hermes non è installato. Scaricalo da {HERMES_DOWNLOAD_URL}."))?;

  assert_endpoint_available(&hive_openai_base_url(), key, "/chat/completions", "Hermes").await?;

  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  write_hermes_config_in(&home, model, key)?;
  launch_cli(app, "hermes", &bin, &[], &[], working_directory)
}

// ---------------------------------------------------------------------------
// Hermes Agent su AI Hive
// ---------------------------------------------------------------------------
// Hermes Agent (hermes-agent.nousresearch.com) ships two surfaces: a desktop
// app and a standalone CLI. Both are configured with environment variables
// only, so nothing is persisted and there is no restore.
//
// The desktop app runs its own agent runtime and cannot receive env vars once
// it is open, so a running instance needs a confirmed restart (like
// ChatGPT.app). The CLI is pointed at AI Hive through the built-in openai-api
// provider, which reads its endpoint from OPENAI_BASE_URL and its key from
// OPENAI_API_KEY; the provider and model are pinned per invocation.
const HERMES_DOWNLOAD_URL: &str = "https://hermes-agent.nousresearch.com/";

// The macOS bundle is named HERMES-IDE.app; match any *.app whose name
// contains "hermes" (case-insensitive) so renames and product-name changes
// keep working. Outside macOS it is exercised by tests only.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_hermes_bundle(name: &str) -> bool {
  let lower = name.to_lowercase();
  lower.contains("hermes") && lower.ends_with(".app")
}

#[cfg(target_os = "macos")]
fn find_hermes_bundle_in(dirs: &[PathBuf]) -> Option<PathBuf> {
  for dir in dirs {
    let Ok(entries) = std::fs::read_dir(dir) else {
      continue;
    };
    let mut found: Vec<PathBuf> = entries
      .flatten()
      .filter_map(|e| e.file_name().to_str().filter(|n| is_hermes_bundle(n)).map(|_| e.path()))
      .filter(|p| !is_setup_bundle(p))
      .collect();
    // Deterministic pick across filesystems.
    found.sort();
    if let Some(bundle) = found.into_iter().find(|p| p.is_dir()) {
      return Some(bundle);
    }
  }
  None
}

// The bootstrap installer ships as a bundle named like the agent
// (com.nousresearch.hermes.setup) but its binary ignores the Hive environment
// and exits at once, so treating it as an installation makes the launch a
// silent no-op.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_setup_bundle_id(identifier: &str) -> bool {
  identifier.ends_with(".setup")
}

#[cfg(target_os = "macos")]
fn is_setup_bundle(bundle: &Path) -> bool {
  let Ok(output) = Command::new("plutil")
    .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
    .arg(bundle.join("Contents").join("Info.plist"))
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .output()
  else {
    return false;
  };
  output.status.success() && is_setup_bundle_id(String::from_utf8_lossy(&output.stdout).trim())
}

// The Hermes checkout builds the desktop app under
// <root>/hermes-agent/apps/desktop/release/<arch>/Hermes.app; the bundle in
// /Applications is only the bootstrap installer (dropped by is_setup_bundle),
// so that build output is searched as well.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn hermes_repo_app_path_in(root: &Path) -> Option<PathBuf> {
  let release = root.join("hermes-agent").join("apps").join("desktop").join("release");
  let mut found: Vec<PathBuf> = std::fs::read_dir(release)
    .ok()?
    .flatten()
    .map(|entry| entry.path().join("Hermes.app"))
    .filter(|bundle| bundle.is_dir())
    .collect();
  found.sort();
  found.into_iter().next()
}

// Detected Hermes installation: the .app bundle on macOS, the binary itself
// elsewhere. The Linux/Windows binary names vary by packaging, so the known
// spellings are probed on PATH plus the standard install locations.
#[cfg(target_os = "macos")]
fn hermes_app_path() -> Option<PathBuf> {
  let home = home_dir()?;
  find_hermes_bundle_in(&[PathBuf::from("/Applications"), home.join("Applications")])
    .or_else(|| hermes_repo_app_path_in(&hermes_root_in(&home)))
}

// Windows: the desktop app is the GUI build (Hermes.exe), a separate surface
// from the CLI, which is launched through hermes_cli_path — so `hermes` on PATH
// is deliberately not a candidate here. The NSIS installer lays the app out
// under %LOCALAPPDATA%\Programs; the HERMES-IDE spellings cover the older name.
#[cfg(windows)]
fn hermes_app_path() -> Option<PathBuf> {
  let Ok(local) = std::env::var("LOCALAPPDATA") else {
    return None;
  };
  let base = Path::new(&local).join("Programs");
  for dir in ["hermes", "Hermes", "hermes-ide", "HERMES-IDE"] {
    for exe in ["Hermes.exe", "HERMES-IDE.exe", "hermes-ide.exe"] {
      let candidate = base.join(dir).join(exe);
      if candidate.is_file() {
        return Some(candidate);
      }
    }
  }
  None
}

#[cfg(target_os = "linux")]
fn hermes_app_path() -> Option<PathBuf> {
  // There is no Linux build of the Hermes desktop app; the CLI is the only
  // supported Hermes surface there.
  None
}

// The Hermes CLI launcher: `hermes` on PATH, or the wrapper install.sh writes
// to ~/.local/bin on POSIX. Windows installs the checkout under
// %HERMES_HOME%/%LOCALAPPDATA%[\hermes] and exposes hermes.exe in its venv.
fn hermes_cli_path() -> Option<PathBuf> {
  if let Some(bin) = find_on_path("hermes") {
    return Some(bin);
  }
  if cfg!(windows) {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HERMES_HOME") {
      roots.push(PathBuf::from(home));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
      roots.push(PathBuf::from(&local));
      roots.push(PathBuf::from(local).join("hermes"));
    }
    for root in roots {
      let candidate = root.join("hermes-agent").join("venv").join("Scripts").join("hermes.exe");
      if candidate.is_file() {
        return Some(candidate);
      }
    }
    return None;
  }
  let fallback = home_dir()?.join(".local").join("bin").join("hermes");
  fallback.is_file().then_some(fallback)
}

// True when the Hermes desktop app is live. The macOS bundle ships the app as
// "Hermes" (CFBundleExecutable), so the probe is case-sensitive on the exact
// names; the lowercase spellings cover the older HERMES-IDE packaging.
#[cfg(target_os = "macos")]
fn hermes_app_running() -> bool {
  for name in ["Hermes", "HERMES-IDE", "hermes-ide"] {
    let running = Command::new("pgrep")
      .arg("-x")
      .arg(name)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()
      .is_ok_and(|s| s.success());
    if running {
      return true;
    }
  }
  false
}

// Windows: probe the detected app's own image name (Hermes.exe, or the legacy
// HERMES-IDE exe) so the running check and the quit stay in lockstep.
#[cfg(windows)]
fn hermes_app_running() -> bool {
  let Some(bin) = hermes_app_path() else {
    return false;
  };
  let Some(name) = bin.file_name().and_then(|n| n.to_str()) else {
    return false;
  };
  desktop_process_running(&[name])
}

#[cfg(target_os = "linux")]
fn hermes_app_running() -> bool {
  // No Linux desktop app; the CLI running does not count as the app being up.
  false
}

// ---------------------------------------------------------------------------
// Hermes Agent: config.yaml su AI Hive
// ---------------------------------------------------------------------------
// Hermes resolves its provider from ~/.hermes/config.yaml, so the launcher
// rewrites `model` and registers a named provider instead of exporting env
// vars: a provider named "ReQurv" is what the UI shows, and the key travels
// inline in the provider entry (no OPENAI_*/CUSTOM_BASE_URL in the process
// environment). The file is machine-generated with one top-level key per line,
// so the edit is textual — a full YAML round-trip would need a new dependency
// and would reformat the user's file.
const HERMES_CONFIG_FILE: &str = "config.yaml";
const HERMES_PROVIDER_KEY: &str = "requrv";
const HERMES_PROVIDER_NAME: &str = "ReQurv";

// Root of the Hermes installation/config. Mirrors what Hermes itself uses:
// $HERMES_HOME when set, ~/.hermes otherwise.
fn hermes_root_in(home: &Path) -> PathBuf {
  std::env::var("HERMES_HOME")
    .ok()
    .filter(|value| !value.trim().is_empty())
    .map(PathBuf::from)
    .unwrap_or_else(|| home.join(".hermes"))
}

fn hermes_config_path_at(root: &Path) -> PathBuf {
  root.join(HERMES_CONFIG_FILE)
}

// Every config the launcher must write: the default home plus one per named
// profile. The desktop app launches its backend with `--profile <name>`, which
// pins HERMES_HOME to <root>/profiles/<name>, so a provider written only to the
// root config is invisible to the app — while the CLI, which uses the default
// home, sees it.
fn hermes_config_paths_at(root: &Path) -> Vec<PathBuf> {
  let mut paths = vec![hermes_config_path_at(root)];
  let Ok(entries) = std::fs::read_dir(root.join("profiles")) else {
    return paths;
  };
  let mut profiles: Vec<PathBuf> = entries
    .flatten()
    .map(|entry| entry.path().join(HERMES_CONFIG_FILE))
    .filter(|path| path.is_file())
    .collect();
  profiles.sort();
  paths.extend(profiles);
  paths
}

// Range of a top-level `key:` block: its own line plus every following indented
// (or blank) line, stopping at the next top-level key.
fn top_level_block(lines: &[&str], key: &str) -> Option<(usize, usize)> {
  let needle = format!("{key}:");
  let start = lines.iter().position(|line| line.starts_with(&needle))?;
  let mut end = start + 1;
  while end < lines.len() {
    let line = lines[end];
    if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('-') {
      break;
    }
    end += 1;
  }
  Some((start, end))
}

// Rewrite `model` and the `providers` entry, leaving every other key untouched.
fn merge_hermes_provider(raw: &str, model: &str, key: &str) -> String {
  let base = hive_openai_base_url();
  let model_block = [
    String::from("model:"),
    format!("  default: {model}"),
    format!("  provider: custom:{HERMES_PROVIDER_KEY}"),
    format!("  base_url: {base}"),
  ];
  let provider_block = [
    String::from("providers:"),
    format!("  {HERMES_PROVIDER_KEY}:"),
    format!("    name: {HERMES_PROVIDER_NAME}"),
    format!("    api: {base}"),
    format!("    api_key: {key}"),
    String::from("    models:"),
    format!("      {model}: {{}}"),
  ];

  let original: Vec<&str> = raw.lines().collect();
  let mut out: Vec<String> = Vec::with_capacity(original.len() + 12);
  let mut providers_written = false;
  let mut index = 0;
  while index < original.len() {
    if let Some((start, end)) = top_level_block(&original, "model") {
      if index == start {
        out.extend(model_block.iter().cloned());
        index = end;
        continue;
      }
    }
    if let Some((start, end)) = top_level_block(&original, "providers") {
      if index == start {
        out.extend(provider_block.iter().cloned());
        providers_written = true;
        index = end;
        continue;
      }
    }
    out.push(original[index].to_string());
    index += 1;
  }
  if !providers_written {
    out.extend(provider_block.iter().cloned());
  }
  let mut rendered = out.join("\n");
  rendered.push('\n');
  rendered
}

// True when this config already carries the provider block the launcher writes.
fn hermes_config_has_provider(path: &Path) -> bool {
  let Ok(raw) = std::fs::read_to_string(path) else {
    return false;
  };
  let lines: Vec<&str> = raw.lines().collect();
  let Some((start, end)) = top_level_block(&lines, "providers") else {
    return false;
  };
  let base = hive_openai_base_url();
  let block = &lines[start..end];
  block.iter().any(|line| line.trim() == format!("{HERMES_PROVIDER_KEY}:"))
    && block.iter().any(|line| line.trim() == format!("api: {base}"))
}

// Configured only when every home (default + profiles) carries the provider.
fn hermes_configured_at(root: &Path) -> bool {
  hermes_config_paths_at(root).iter().all(|path| hermes_config_has_provider(path))
}

fn hermes_configured_in(home: &Path) -> bool {
  hermes_configured_at(&hermes_root_in(home))
}

// Back up (once, only while a file is not already ours) and rewrite each config
// with the Hive provider and the selected model.
fn write_hermes_config_at(root: &Path, model: &str, key: &str) -> Result<(), String> {
  for path in hermes_config_paths_at(root) {
    let raw = std::fs::read_to_string(&path).map_err(|_| {
      format!("Configurazione di Hermes non trovata ({}). Avvia Hermes una volta e riprova.", path.display())
    })?;
    if !hermes_config_has_provider(&path) {
      let backup = backup_path_for(&path);
      if !backup.exists() {
        let _ = std::fs::write(&backup, raw.as_str());
      }
    }
    let updated = merge_hermes_provider(&raw, model, key);
    std::fs::write(&path, updated)
      .map_err(|e| format!("Impossibile scrivere {}: {e}", path.display()))?;
  }
  Ok(())
}

fn write_hermes_config_in(home: &Path, model: &str, key: &str) -> Result<(), String> {
  write_hermes_config_at(&hermes_root_in(home), model, key)
}

// Back to the pre-Hive config: the one-shot backups written before the first
// rewrite, for the default home and every profile.
fn restore_hermes_config_at(root: &Path) -> Result<(), String> {
  for path in hermes_config_paths_at(root) {
    let backup = backup_path_for(&path);
    if !backup.exists() {
      continue;
    }
    std::fs::copy(&backup, &path).map_err(|e| format!("Impossibile scrivere {}: {e}", path.display()))?;
    std::fs::remove_file(&backup).map_err(|e| e.to_string())?;
  }
  Ok(())
}

fn restore_hermes_config_in(home: &Path) -> Result<(), String> {
  restore_hermes_config_at(&hermes_root_in(home))
}

// Restore the pre-Hive Hermes config and report whether a running instance must
// be restarted to drop the ReQurv provider.
#[tauri::command]
pub fn restore_hermes() -> Result<AppRestartResult, String> {
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  restore_hermes_config_in(&home)?;
  Ok(AppRestartResult {
    restart_required: hermes_app_running(),
  })
}

// Launch the detected installation. On macOS it goes through LaunchServices
// (`open`): spawning the Mach-O as a child would make this launcher the
// "responsible process" for the app's file access, so every protected folder
// the agent touches (~/Documents, ~/Desktop) would raise a TCC prompt naming
// the bridge instead of Hermes.
fn spawn_hermes_app() -> Result<(), String> {
  let Some(install) = hermes_app_path() else {
    return Err(format!("Hermes non è installato. Scaricalo da {HERMES_DOWNLOAD_URL}."));
  };
  #[cfg(target_os = "macos")]
  {
    open_app_bundle(&install, "Hermes.app", &[])
  }
  #[cfg(not(target_os = "macos"))]
  {
    Command::new(&install)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .map(|_| ())
      .map_err(|e| format!("Impossibile avviare Hermes: {e}"))
  }
}

// Quit Hermes so the next launch can carry the AI Hive environment. Graceful
// AppleScript quit on macOS, then a forced kill; best-effort pkill/taskkill
// elsewhere. The wait mirrors the ChatGPT restart flow.
#[cfg(target_os = "macos")]
fn quit_hermes_app() {
  let app_name = hermes_app_path()
    .and_then(|b| b.file_stem().map(|s| s.to_string_lossy().to_string()))
    .unwrap_or_else(|| String::from("HERMES-IDE"));
  let _ = Command::new("osascript")
    .args(["-e", &format!("tell application \"{app_name}\" to quit")])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn();
  for _ in 0..20 {
    if !hermes_app_running() {
      return;
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
  }
  for name in ["HERMES-IDE", "hermes-ide", "hermes"] {
    let _ = Command::new("pkill")
      .arg("-x")
      .arg(name)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn();
  }
  for _ in 0..10 {
    if !hermes_app_running() {
      return;
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
  }
}

#[cfg(not(target_os = "macos"))]
fn quit_hermes_app() {
  // Kill only the detected desktop app's own image name; on Linux there is no
  // desktop app so this is a no-op (it must never take down the CLI).
  let Some(bin) = hermes_app_path() else {
    return;
  };
  let Some(name) = bin.file_name().and_then(|n| n.to_str()) else {
    return;
  };
  desktop_process_quit(&[name]);
  for _ in 0..10 {
    if !hermes_app_running() {
      return;
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
  }
}

// Launch Hermes on AI Hive: env vars only, nothing persisted. If an instance
// is already running it cannot receive the environment, so the caller (the
// frontend) asks for a restart confirmation before invoking restart_hermes_app.
#[tauri::command]
pub async fn launch_hermes_app(model: String, key: String) -> Result<AppRestartResult, String> {
  let model = model.trim();
  let key = key.trim();
  if model.is_empty() {
    return Err("Nessun modello selezionato".into());
  }
  if key.is_empty() {
    return Err("Chiave Hive non salvata".into());
  }
  if hermes_app_path().is_none() {
    return Err(format!("Hermes non è installato. Scaricalo da {HERMES_DOWNLOAD_URL}."));
  }
  assert_endpoint_available(&hive_openai_base_url(), key, "/chat/completions", "Hermes").await?;
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  write_hermes_config_in(&home, model, key)?;
  if hermes_app_running() {
    return Ok(AppRestartResult { restart_required: true });
  }
  spawn_hermes_app()?;
  Ok(AppRestartResult { restart_required: false })
}

// Quit the running instance (if any) and relaunch it so it loads the config
// written above.
#[tauri::command]
pub fn restart_hermes_app(model: String, key: String) -> Result<(), String> {
  let model = model.trim();
  let key = key.trim();
  if model.is_empty() {
    return Err("Nessun modello selezionato".into());
  }
  if key.is_empty() {
    return Err("Chiave Hive non salvata".into());
  }
  if hermes_app_path().is_none() {
    return Err(format!("Hermes non è installato. Scaricalo da {HERMES_DOWNLOAD_URL}."));
  }
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  write_hermes_config_in(&home, model, key)?;
  quit_hermes_app();
  spawn_hermes_app()
}

// Quote a value for safe inclusion in a single-quoted shell word.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn shell_quote(value: &str) -> String {
  format!("'{}'", value.replace('\'', "'\\''"))
}

// Build a bash script that sets the Hive env vars and runs the CLI so the
// terminal window stays attached to the process (TUI apps need a real TTY).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn build_terminal_script(
  bin: &Path,
  args: &[String],
  env: &[(&str, &str)],
  working_directory: &Path,
) -> String {
  let mut out = format!(
    "#!/bin/bash\ncd -- {}\n",
    shell_quote(&working_directory.to_string_lossy()),
  );
  for (name, value) in env {
    out.push_str(&format!("export {name}={}\n", shell_quote(value)));
  }
  let mut command = shell_quote(&bin.to_string_lossy());
  for arg in args {
    command.push(' ');
    command.push_str(&shell_quote(arg));
  }
  out.push_str(&command);
  out.push_str(
    "\necho \"\"\necho \"Processo terminato (exit code: $?). Premi Invio per chiudere la finestra.\"\nread -r _\n",
  );
  out
}

// Single entry point for launching a CLI: on macOS the agents are TUI apps
// that need a terminal, so we hand a .command script to the system's default
// terminal via `open`; elsewhere we keep the detached spawn.
fn launch_cli(
  app: &tauri::AppHandle,
  service: &str,
  bin: &Path,
  args: &[String],
  env: &[(&str, &str)],
  working_directory: &Path,
) -> Result<(), String> {
  #[cfg(target_os = "macos")]
  {
    let script = build_terminal_script(bin, args, env, working_directory);
    open_terminal_script(app, service, &script)
  }
  #[cfg(not(target_os = "macos"))]
  {
    let _ = (app, service);
    spawn_cli(bin, args, env, working_directory)
  }
}

#[cfg(target_os = "macos")]
fn open_terminal_script(app: &tauri::AppHandle, service: &str, script: &str) -> Result<(), String> {
  let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
  std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
  let path = dir.join(format!("launch-{service}.command"));
  std::fs::write(&path, script).map_err(|e| format!("Scrittura script non riuscita: {e}"))?;
  use std::os::unix::fs::PermissionsExt;
  let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));

  // Launch via AppleScript `do script`: `open` on .command files goes through
  // LaunchServices and can silently no-op, while `do script` is deterministic
  // (first run may trigger the macOS automation permission prompt).
  let applescript = format!(
    "tell application \"Terminal\"\n  activate\n  do script \"bash {}\"\nend tell\n",
    applescript_escape(&shell_quote(&path.to_string_lossy())),
  );
  let mut child = Command::new("osascript")
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|e| format!("Impossibile avviare osascript: {e}"))?;
  use std::io::Write;
  if let Some(mut stdin) = child.stdin.take() {
    stdin
      .write_all(applescript.as_bytes())
      .map_err(|e| e.to_string())?;
  }
  let output = child
    .wait_with_output()
    .map_err(|e| format!("osascript terminato in modo anomalo: {e}"))?;
  if !output.status.success() {
    let err = String::from_utf8_lossy(&output.stderr);
    return Err(format!("Terminal non ha eseguito lo script: {err}"));
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn applescript_escape(value: &str) -> String {
  value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x00000010;

#[cfg(windows)]
fn is_shell_shim(path: &Path) -> bool {
  path.extension()
    .and_then(|e| e.to_str())
    .is_some_and(|e| matches!(e.to_lowercase().as_str(), "cmd" | "bat" | "ps1"))
}

#[cfg(windows)]
fn is_powershell_shim(path: &Path) -> bool {
  path.extension()
    .and_then(|e| e.to_str())
    .is_some_and(|e| e.to_lowercase().as_str() == "ps1")
}

// On macOS every launch goes through the terminal script; the detached spawn
// is the fallback for Windows (new console) and Linux.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn spawn_cli(
  bin: &Path,
  args: &[String],
  env: &[(&str, &str)],
  working_directory: &Path,
) -> Result<(), String> {
  let mut command = Command::new(bin);
  #[cfg(windows)]
  if is_shell_shim(bin) {
    if is_powershell_shim(bin) {
      command = Command::new("powershell.exe");
      command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
      command.arg(bin);
    } else {
      command = Command::new("cmd");
      command.arg("/c");
      command.arg(bin);
    }
  }
  command.current_dir(working_directory);
  command.args(args);
  command.stdin(Stdio::null());
  command.stdout(Stdio::null());
  command.stderr(Stdio::null());
  for (name, value) in env {
    command.env(name, value);
  }
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NEW_CONSOLE);
  }
  command
    .spawn()
    .map_err(|e| format!("Avvio non riuscito: {e}"))?;
  Ok(())
}

#[derive(Serialize, Clone)]
pub struct UpdateInfo {
  pub current_version: String,
  pub latest_version: Option<String>,
  pub update_available: bool,
  pub release_url: Option<String>,
}

// Segmenti numerici di una versione semver ("0.1.3" -> [0, 1, 3]).
// Restituisce None se un segmento non è numerico (tag male formattati).
fn version_segments(version: &str) -> Option<Vec<u64>> {
  version
    .split('.')
    .map(|part| part.parse::<u64>().ok())
    .collect()
}

// True solo se `latest` è strettamente maggiore di `current`, confrontando i
// segmenti da sinistra a destra. Le lunghezze diverse si completano con zero
// ("1.2" == "1.2.0"). Versioni non parseabili non contano mai come novità:
// un falso positivo mostrerebbe un banner di aggiornamento a vuoto.
fn is_newer_version(latest: &str, current: &str) -> bool {
  let Some(latest) = version_segments(latest) else {
    return false;
  };
  let Some(current) = version_segments(current) else {
    return false;
  };
  if latest.is_empty() || current.is_empty() {
    return false;
  }
  for i in 0..latest.len().max(current.len()) {
    let l = latest.get(i).copied().unwrap_or(0);
    let c = current.get(i).copied().unwrap_or(0);
    if l != c {
      return l > c;
    }
  }
  false
}

// Confronta la versione installata con la release più recente di GitHub.
// L'avviso è solo informativo: l'utente scarica la release dalla pagina
// collegata, senza auto-update.
#[tauri::command]
pub async fn check_for_updates(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
  let current_version = app.package_info().version.to_string();
  let client = reqwest::Client::new();
  // GitHub risponde 403 alle richieste senza User-Agent: senza questa
  // intestazione la verifica fallisce sempre e il banner non compare.
  let response = client
    .get(GITHUB_LATEST_RELEASE_URL)
    .header(reqwest::header::USER_AGENT, format!("ReQurv Bridge/{current_version}"))
    .timeout(std::time::Duration::from_secs(10))
    .send()
    .await
    .map_err(|e| format!("Impossibile verificare gli aggiornamenti: {e}"))?;

  let status = response.status();
  if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
    return Err("Verifica aggiornamenti non disponibile al momento (limiti di GitHub).".into());
  }
  if !status.is_success() {
    return Err(format!("Verifica aggiornamenti non riuscita (stato {status})."));
  }

  let body: serde_json::Value = response
    .json()
    .await
    .map_err(|e| format!("Risposta non valida da GitHub: {e}"))?;

  // tag_name è nel formato "v0.1.3": si confronta senza il prefisso "v".
  let tag = body.get("tag_name").and_then(|t| t.as_str()).unwrap_or_default();
  let latest_version = tag.strip_prefix('v').unwrap_or(tag).to_string();
  let update_available = is_newer_version(&latest_version, &current_version);
  let release_url = body
    .get("html_url")
    .and_then(|u| u.as_str())
    .map(|u| u.to_string());

  Ok(UpdateInfo {
    current_version,
    latest_version: (!latest_version.is_empty()).then_some(latest_version),
    update_available,
    release_url,
  })
}

// ---------------------------------------------------------------------------
// Server MCP: lista libera + variabili d'ambiente
// ---------------------------------------------------------------------------
// Bridge keeps its own registry of MCP servers the user wants to expose to the
// agents it launches. Each server is written into the native MCP config of the
// selected targets (Claude Code, OpenCode, Claude Desktop). Entries are
// inserted/removed surgically by id, so an agent's existing `mcpServers`
// entries are never touched and there is no whole-file backup to restore.

const MCP_FILE_NAME: &str = "mcp.json";

#[derive(Serialize, Deserialize, Clone)]
pub struct McpServer {
  pub id: String,
  pub name: String,
  pub command: String,
  #[serde(default)]
  pub args: Vec<String>,
  #[serde(default)]
  pub env: serde_json::Map<String, serde_json::Value>,
  #[serde(default)]
  pub targets: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct McpTarget {
  pub id: String,
  pub label: String,
  pub installed: bool,
}

#[derive(Serialize)]
pub struct McpStatus {
  pub servers: Vec<McpServer>,
  pub targets: Vec<McpTarget>,
  // server id -> target id -> present in that agent's config
  pub configured: serde_json::Map<String, serde_json::Value>,
}

fn mcp_file_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
  let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
  Ok(dir.join(MCP_FILE_NAME))
}

fn read_mcp_registry(app: &tauri::AppHandle) -> Vec<McpServer> {
  let Ok(path) = mcp_file_path(app) else {
    return Vec::new();
  };
  match std::fs::read_to_string(path) {
    Ok(raw) if !raw.trim().is_empty() => serde_json::from_str(&raw).unwrap_or_default(),
    _ => Vec::new(),
  }
}

fn write_mcp_registry(app: &tauri::AppHandle, servers: &[McpServer]) -> Result<(), String> {
  let path = mcp_file_path(app)?;
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  if let Ok(old) = std::fs::read_to_string(&path) {
    let _ = std::fs::write(path.with_extension("json.bak"), old);
  }
  let rendered = serde_json::to_string_pretty(servers).map_err(|e| e.to_string())?;
  std::fs::write(&path, rendered).map_err(|e| e.to_string())
}

// Native MCP config path of a target agent; None when it is not an MCP target.
fn mcp_target_path_in(home: &Path, target: &str) -> Option<PathBuf> {
  match target {
    "claude_code" => Some(home.join(".claude.json")),
    "opencode" => Some(opencode_config_path_in(home)),
    "claude_desktop" => Some(claude_desktop_paths_in(home).normal_config),
    _ => None,
  }
}

fn mcp_container_key(target: &str) -> &'static str {
  if target == "opencode" {
    "mcp"
  } else {
    "mcpServers"
  }
}

// Claude Code and Claude Desktop share the standard `mcpServers` shape;
// OpenCode's local server puts the command first in the `command` array and
// names the environment map `environment`.
fn mcp_entry_value(target: &str, server: &McpServer) -> serde_json::Value {
  match target {
    "opencode" => {
      let mut command = vec![server.command.clone()];
      command.extend(server.args.iter().cloned());
      serde_json::json!({
        "type": "local",
        "command": command,
        "enabled": true,
        "environment": server.env,
      })
    },
    _ => serde_json::json!({
      "command": server.command,
      "args": server.args,
      "env": server.env,
    }),
  }
}

fn read_mcp_config_in(
  home: &Path,
  target: &str,
) -> Result<(PathBuf, serde_json::Value), String> {
  let path = mcp_target_path_in(home, target)
    .ok_or_else(|| format!("Target MCP non valido: {target}"))?;
  let raw = match std::fs::read_to_string(&path) {
    Ok(raw) => raw,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
      return Ok((path, serde_json::json!({})))
    },
    Err(e) => return Err(format!("Impossibile leggere {}: {e}", path.display())),
  };
  let cleaned = if target == "opencode" {
    strip_trailing_commas(&strip_jsonc_comments(&raw))
  } else {
    raw
  };
  let config: serde_json::Value = serde_json::from_str(&cleaned)
    .map_err(|e| format!("{} non è JSON valido: {e}", path.display()))?;
  Ok((path, config))
}

fn mcp_target_present_in(home: &Path, target: &str, id: &str) -> bool {
  let Ok((_, config)) = read_mcp_config_in(home, target) else {
    return false;
  };
  config
    .get(mcp_container_key(target))
    .and_then(|container| container.get(id))
    .is_some()
}

fn write_mcp_target_in(home: &Path, target: &str, server: &McpServer) -> Result<(), String> {
  let (path, mut config) = read_mcp_config_in(home, target)?;
  let Some(root) = config.as_object_mut() else {
    return Err(format!("{} non è un oggetto JSON", path.display()));
  };
  let container = root
    .entry(mcp_container_key(target))
    .or_insert_with(|| serde_json::json!({}));
  if !container.is_object() {
    *container = serde_json::json!({});
  }
  let container = container.as_object_mut().expect("oggetto appena forzato");
  container.insert(server.id.clone(), mcp_entry_value(target, server));
  write_json(&path, &config)
}

fn strip_mcp_target_in(home: &Path, target: &str, id: &str) -> Result<(), String> {
  let (path, mut config) = read_mcp_config_in(home, target)?;
  let Some(root) = config.as_object_mut() else {
    return Err(format!("{} non è un oggetto JSON", path.display()));
  };
  let Some(container) = root
    .get_mut(mcp_container_key(target))
    .and_then(|c| c.as_object_mut())
  else {
    return Ok(());
  };
  if container.remove(id).is_none() {
    return Ok(());
  }
  if container.is_empty() {
    root.remove(mcp_container_key(target));
  }
  write_json(&path, &config)
}

fn mcp_targets() -> Vec<McpTarget> {
  vec![
    McpTarget {
      id: "claude_code".into(),
      label: "Claude Code".into(),
      installed: find_service_binary("claude").is_some(),
    },
    McpTarget {
      id: "opencode".into(),
      label: "OpenCode".into(),
      installed: opencode_app_path().is_some() || find_service_binary("opencode").is_some(),
    },
    McpTarget {
      id: "claude_desktop".into(),
      label: "Claude Desktop".into(),
      installed: claude_desktop_app_path().is_some(),
    },
  ]
}

#[tauri::command]
pub fn list_mcp_servers(app: tauri::AppHandle) -> McpStatus {
  let servers = read_mcp_registry(&app);
  let targets = mcp_targets();
  let mut configured = serde_json::Map::new();
  if let Some(home) = home_dir() {
    for server in &servers {
      let mut map = serde_json::Map::new();
      for target in &server.targets {
        map.insert(
          target.clone(),
          serde_json::Value::Bool(mcp_target_present_in(&home, target, &server.id)),
        );
      }
      configured.insert(server.id.clone(), serde_json::Value::Object(map));
    }
  }
  McpStatus {
    servers,
    targets,
    configured,
  }
}

#[tauri::command]
pub fn save_mcp_server(app: tauri::AppHandle, server: McpServer) -> Result<(), String> {
  let id = server.id.trim();
  if id.is_empty() {
    return Err("L'identificativo MCP non può essere vuoto".into());
  }
  let command = server.command.trim();
  if command.is_empty() {
    return Err("Il comando MCP non può essere vuoto".into());
  }
  if server.targets.is_empty() {
    return Err("Scegli almeno un agente in cui configurare l'MCP".into());
  }
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  let mut canonical = server.clone();
  canonical.id = id.to_string();
  canonical.command = command.to_string();
  for target in &canonical.targets {
    if mcp_target_path_in(&home, target).is_none() {
      return Err(format!("Agente MCP non valido: {target}"));
    }
  }
  // Write every selected target before touching the registry: a failure keeps
  // mcp.json consistent with what is actually on disk.
  for target in &canonical.targets {
    write_mcp_target_in(&home, target, &canonical)
      .map_err(|e| format!("{target}: {e}"))?;
  }
  let mut registry = read_mcp_registry(&app);
  if let Some(slot) = registry.iter_mut().find(|s| s.id == canonical.id) {
    *slot = canonical.clone();
  } else {
    registry.push(canonical.clone());
  }
  write_mcp_registry(&app, &registry)
}

#[tauri::command]
pub fn delete_mcp_server(app: tauri::AppHandle, id: String) -> Result<(), String> {
  let id = id.trim();
  if id.is_empty() {
    return Err("L'identificativo MCP non può essere vuoto".into());
  }
  let home = home_dir().ok_or_else(|| "Home directory non trovata".to_string())?;
  let registry = read_mcp_registry(&app);
  let Some(server) = registry.iter().find(|s| s.id == id).cloned() else {
    return Ok(());
  };
  // Remove the entry from every target the server was written to; if any file
  // cannot be cleaned the registry entry is kept so the UI does not hide it.
  let failures: Vec<String> = server
    .targets
    .iter()
    .filter_map(|target| {
      strip_mcp_target_in(&home, target, id)
        .err()
        .map(|e| format!("{target}: {e}"))
    })
    .collect();
  if !failures.is_empty() {
    return Err(format!("Rimozione MCP non completata: {}", failures.join("; ")));
  }
  let mut registry = registry;
  registry.retain(|s| s.id != id);
  write_mcp_registry(&app, &registry)
}

#[cfg(test)]
mod tests {
  use super::*;

  // La radice dove un'app desktop tiene i dati utente è la base su cui Claude
  // Desktop aggancia i suoi path di config; su macOS deve restare l'intero
  // "Application Support" stabile, invariato rispetto al passato.
  #[cfg(target_os = "macos")]
  #[test]
  fn application_support_root_macos_is_application_support() {
    let home = PathBuf::from("/home/bridge");
    assert_eq!(
      application_support_root(&home),
      home.join("Library").join("Application Support")
    );
  }

  // L'Exec di un .desktop è "binario [argomenti]": il lanciatore è il primo
  // token, eventualmente prefissato da `path=`. Funzione pura, testabile su
  // ogni host.
  #[test]
  fn desktop_entry_target_parses_exec_value() {
    assert_eq!(
      desktop_entry_target("/opt/opencode/opencode --app %U"),
      Some("/opt/opencode/opencode".to_string())
    );
    assert_eq!(
      desktop_entry_target("path=/usr/bin/claude-desktop --flag"),
      Some("/usr/bin/claude-desktop".to_string())
    );
    assert_eq!(desktop_entry_target("opencode"), Some("opencode".to_string()));
    assert_eq!(desktop_entry_target("   "), None);
    assert_eq!(desktop_entry_target(""), None);
  }

  // hive.json tiene anche il modello verificato, così un cambio di chiave
  // sa quale modello rimettere nelle config già scritte.
  #[test]
  fn key_file_keeps_key_and_model() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let path = tmp.join("hive.json");
    write_key_file(&path, "requrv_sk_vecchia", "requrv-small-3.8").unwrap();
    write_key_file(&path, "requrv_sk_nuova", "requrv-small-3.8").unwrap();
    let saved: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["apiKey"], "requrv_sk_nuova");
    assert_eq!(saved["model"], "requrv-small-3.8");

    // Il .bak contiene la chiave precedente in chiaro: cancellare la chiave
    // deve portare via anche quello.
    assert!(path.with_extension("json.bak").exists());
    delete_key_files(&path).unwrap();
    assert!(!path.exists());
    assert!(!path.with_extension("json.bak").exists());
    // Idempotente: cancellare di nuovo non deve fallire.
    delete_key_files(&path).unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Il riallineamento scrive la chiave nuova solo nei target già configurati e
  // non tocca i backup (che devono restare lo stato pre-Hive).
  #[test]
  fn reapply_writes_the_new_key_into_configured_targets_only() {
    let tmp =
      std::env::temp_dir().join(format!("requrv-bridge-test-reapply-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let oc_dir = tmp.join(".config").join("opencode");
    std::fs::create_dir_all(&oc_dir).unwrap();
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    // Config utente preesistenti: il ripristino deve tornare a queste.
    let user_opencode = r#"{ "theme": "dark" }"#;
    let user_settings = r#"{ "env": { "FOO": "bar" } }"#;
    std::fs::write(oc_dir.join("opencode.jsonc"), user_opencode).unwrap();
    let settings_path = claude_code_settings_path_in(&tmp);
    std::fs::write(&settings_path, user_settings).unwrap();

    write_opencode_config_in(&tmp, "requrv-small-3.8", "requrv_sk_vecchia").unwrap();
    write_claude_code_settings_in(&tmp, "requrv-small-3.8", "requrv_sk_vecchia").unwrap();

    // Nessun target Codex è configurato qui, quindi il catalogo non serve.
    let failures = reapply_persisted_configs(&tmp, "requrv-medium-4", "requrv_sk_nuova", &[]);
    assert!(failures.is_empty(), "{failures:?}");

    let opencode: serde_json::Value = serde_json::from_str(
      &std::fs::read_to_string(opencode_config_path_in(&tmp)).unwrap(),
    )
    .unwrap();
    assert_eq!(opencode["provider"]["requrv-hive"]["options"]["apiKey"], "requrv_sk_nuova");
    assert_eq!(opencode["model"], "requrv-hive/requrv-medium-4");
    assert!(opencode_configured_in(&tmp));

    let settings: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(settings["env"]["ANTHROPIC_API_KEY"], "requrv_sk_nuova");
    assert_eq!(settings["env"]["ANTHROPIC_MODEL"], "requrv-medium-4");
    assert_eq!(settings["env"]["FOO"], "bar");
    assert!(claude_code_configured_in(&tmp));

    // Il riallineamento non deve aver scambiato il backup con la versione Hive:
    // il ripristino torna al file originale dell'utente, senza chiave.
    assert_eq!(std::fs::read_to_string(backup_path_for(&settings_path)).unwrap(), user_settings);
    restore_claude_code_settings_in(&tmp).unwrap();
    assert_eq!(std::fs::read_to_string(&settings_path).unwrap(), user_settings);
    assert!(!claude_code_configured_in(&tmp));
    restore_opencode_config_in(&tmp).unwrap();
    assert_eq!(std::fs::read_to_string(opencode_config_path_in(&tmp)).unwrap(), user_opencode);
    assert!(!opencode_configured_in(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn parses_local_env_file() {
    let entries = local_env_entries(
      "# local overrides\n\nHIVE_OPENAI_BASE_URL=http://localhost:3000/api/v1\nOTHER=ignored\nHIVE_ANTHROPIC_BASE_URL = \"http://localhost:3000/api\"\n",
    );
    assert_eq!(
      entries,
      vec![
        ("HIVE_OPENAI_BASE_URL", "http://localhost:3000/api/v1"),
        ("HIVE_ANTHROPIC_BASE_URL", "http://localhost:3000/api"),
      ]
    );
  }

  #[test]
  fn finds_opencode_if_installed() {
    let bin = find_service_binary("opencode");
    println!("opencode rilevato come: {bin:?}");
    if let Some(bin) = &bin {
      assert!(bin.is_file());
    }
  }

  #[test]
  fn finds_npm_if_installed() {
    let npm = find_npm();
    println!("npm rilevato come: {npm:?}");
    if let Some(npm) = &npm {
      assert!(npm.is_file());
    }
  }

  #[test]
  fn shell_quote_wraps_and_escapes_single_quotes() {
    assert_eq!(shell_quote("abc"), "'abc'");
    assert_eq!(shell_quote(""), "''");
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
    assert_eq!(shell_quote("a b c"), "'a b c'");
  }
  #[test]
  fn requires_an_existing_project_directory() {
    let current = std::env::current_dir().unwrap();
    assert_eq!(
      require_project_directory(current.to_str()),
      Ok(current.canonicalize().unwrap()),
    );
    assert!(require_project_directory(None).is_err());
    assert!(require_project_directory(Some(" ")).is_err());
    assert!(require_project_directory(std::env::current_exe().unwrap().to_str()).is_err());
  }

  #[test]
  fn builds_opencode_terminal_script() {
    let script = build_terminal_script(
      Path::new("/opt/homebrew/bin/opencode"),
      &[],
      &[("OPENCODE_CONFIG_CONTENT", "{\"model\":\"hive/x\"}")],
      Path::new("/Users/x/Projects/it's fine"),
    );
    assert!(script.starts_with("#!/bin/bash\ncd -- '/Users/x/Projects/it'\\''s fine'\n"));
    assert!(script.contains(r#"export OPENCODE_CONFIG_CONTENT='{"model":"hive/x"}'"#));
    assert!(script.contains("'/opt/homebrew/bin/opencode'"));
    assert!(script.ends_with("read -r _\n"));
  }

  // La CLI codex non riceve argomenti; CODEX_HOME è pinnato (la shell può
  // portarne uno di un altro tool) e l'autenticazione sta nella tabella
  // provider di config.toml (bearer token).
  #[test]
  fn builds_codex_terminal_script() {
    let script = build_terminal_script(
      Path::new("/Users/x/.nvm/versions/node/v24/bin/codex"),
      &[],
      &[("CODEX_HOME", "/Users/x/.codex")],
      Path::new("/Users/x/Projects/codex"),
    );
    assert!(script.contains("export CODEX_HOME='/Users/x/.codex'"));
    assert!(!script.contains("--profile"));
    assert!(script.contains("'/Users/x/.nvm/versions/node/v24/bin/codex'\n"));
  }

  // Claude Code non prende argomenti: il terminale lo punta ad AI Hive solo
  // con le variabili d'ambiente, senza toccare ~/.claude.
  #[test]
  fn builds_claude_terminal_script() {
    let anthropic_base = hive_anthropic_base_url();
    let script = build_terminal_script(
      Path::new("/Users/x/.local/bin/claude"),
      &[],
      &[
        ("ANTHROPIC_BASE_URL", anthropic_base.as_str()),
        ("ANTHROPIC_API_KEY", "sk-test"),
        ("ANTHROPIC_MODEL", "model-a"),
        ("CLAUDE_CODE_MAX_CONTEXT_TOKENS", "128000"),
      ],
      Path::new("/Users/x/Projects/claude"),
    );
    assert!(script.contains(&format!("export ANTHROPIC_BASE_URL='{anthropic_base}'")));
    assert!(script.contains("export ANTHROPIC_API_KEY='sk-test'"));
    assert!(script.contains("export ANTHROPIC_MODEL='model-a'"));
    assert!(script.contains("export CLAUDE_CODE_MAX_CONTEXT_TOKENS='128000'"));
    assert!(script.contains("'/Users/x/.local/bin/claude'\necho \"\""));
  }

  // La configurazione vive nel file: il merge conserva hooks, theme e le chiavi
  // env non nostre, e azzera il token Bearer perché i file vengono uniti.
  #[test]
  fn claude_code_settings_merge_preserves_user_keys_and_env() {
    let home = claude_code_test_home();
    let path = claude_code_settings_path_in(&home);
    let original_raw = serde_json::to_string_pretty(&serde_json::json!({
      "theme": "dark",
      "hooks": { "PreToolUse": [] },
      "env": { "FOO": "bar" }
    }))
    .unwrap()
    + "\n";
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &original_raw).unwrap();

    write_claude_code_settings_in(&home, "requrv-small-3.8", "requrv_sk_test").unwrap();

    let settings: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(settings["theme"], "dark");
    assert!(settings["hooks"].is_object());
    assert_eq!(settings["env"]["FOO"], "bar");
    assert_eq!(settings["env"]["ANTHROPIC_BASE_URL"], hive_anthropic_base_url());
    assert_eq!(settings["env"]["ANTHROPIC_API_KEY"], "requrv_sk_test");
    assert_eq!(settings["env"]["ANTHROPIC_MODEL"], "requrv-small-3.8");
    assert_eq!(settings["env"]["ANTHROPIC_AUTH_TOKEN"], "");
    assert!(claude_code_configured_in(&home));

    // Il ripristino torna al file originale byte per byte e rimuove il backup.
    restore_claude_code_settings_in(&home).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original_raw);
    assert!(!backup_path_for(&path).exists());
    assert!(!claude_code_configured_in(&home));
    let _ = std::fs::remove_dir_all(&home);
  }

  // Claude Code chiede il consenso per ogni chiave nuova e, se rispondi "No",
  // smette di usare la chiave: il launcher registra l'approvazione da solo.
  #[test]
  fn claude_code_preapproves_the_custom_api_key() {
    let home = claude_code_test_home();
    let path = home.join(".claude.json");
    std::fs::write(
      &path,
      serde_json::to_string_pretty(&serde_json::json!({
        "numStartups": 7,
        "customApiKeyResponses": {
          "approved": ["00000000000000000000"],
          "rejected": ["requrv_sk_vecchia"]
        }
      }))
      .unwrap(),
    )
    .unwrap();

    let key = "requrv_sk_abcdefghijklmnopqrstuvwxyz0123456789";
    let fingerprint = "qrstuvwxyz0123456789";
    write_claude_code_settings_in(&home, "requrv-small-3.8", key).unwrap();
    // Un secondo lancio non deve duplicare la voce.
    write_claude_code_settings_in(&home, "requrv-small-3.8", key).unwrap();

    let config: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let approved = config["customApiKeyResponses"]["approved"]
      .as_array()
      .cloned()
      .unwrap();
    assert_eq!(config["numStartups"], 7, "lo stato di Claude resta intatto");
    assert_eq!(approved.len(), 2);
    assert!(approved.contains(&serde_json::json!(fingerprint)));

    // Un "No" sulla stessa chiave non può sopravvivere all'approvazione, e un
    // rifiuto su una chiave diversa resta intatto.
    let mut config = config;
    config["customApiKeyResponses"]["rejected"] =
      serde_json::json!(["requrv_sk_vecchia", fingerprint]);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    write_claude_code_settings_in(&home, "requrv-small-3.8", key).unwrap();
    let config: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
      config["customApiKeyResponses"]["rejected"],
      serde_json::json!(["requrv_sk_vecchia"])
    );
    let _ = std::fs::remove_dir_all(&home);
  }

  // Un settings.json rotto non viene mai sovrascritto.
  #[test]
  fn claude_code_settings_refuse_to_overwrite_invalid_json() {
    let home = claude_code_test_home();
    let path = claude_code_settings_path_in(&home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ non è json").unwrap();
    let error = write_claude_code_settings_in(&home, "requrv-small-3.8", "requrv_sk_test")
      .expect_err("deve fallire");
    assert!(error.contains("non è un JSON valido"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ non è json");
    let _ = std::fs::remove_dir_all(&home);
  }

  // Senza backup il ripristino toglie solo le nostre chiavi env.
  #[test]
  fn claude_code_restore_without_backup_keeps_foreign_env() {
    let home = claude_code_test_home();
    let path = claude_code_settings_path_in(&home);
    write_claude_code_settings_in(&home, "requrv-small-3.8", "requrv_sk_test").unwrap();
    let mut settings: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    settings["env"]["FOO"] = serde_json::json!("bar");
    write_json(&path, &settings).unwrap();

    restore_claude_code_settings_in(&home).unwrap();
    let restored: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(restored["env"]["FOO"], "bar");
    assert!(restored["env"].get("ANTHROPIC_BASE_URL").is_none());
    assert!(restored["env"].get("ANTHROPIC_API_KEY").is_none());
    assert!(!claude_code_configured_in(&home));
    let _ = std::fs::remove_dir_all(&home);
  }

  // La configurazione di Hermes vive in config.yaml: provider "ReQurv" con la
  // chiave inline e il modello scelto, senza variabili d'ambiente. Va scritta
  // sia nella home di default sia in ogni profilo: l'app desktop lancia il
  // backend con --profile, che sposta HERMES_HOME.
  #[test]
  fn writes_hermes_provider_into_every_home() {
    let root = std::env::temp_dir().join(format!("requrv-bridge-test-hermes-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let profile_dir = root.join("profiles").join("developer");
    std::fs::create_dir_all(&profile_dir).unwrap();
    let original = "model:\n  default: gpt-6.1-sol\n  provider: custom\n  base_url: http://localhost:8086/v1\ndatabase:\n  journal_mode: wal\ncustom_providers: []\n";
    let root_path = hermes_config_path_at(&root);
    let profile_path = hermes_config_path_at(&profile_dir);
    std::fs::write(&root_path, original).unwrap();
    std::fs::write(&profile_path, original).unwrap();
    assert_eq!(hermes_config_paths_at(&root).len(), 2);

    write_hermes_config_at(&root, "requrv-small-3.8", "requrv_sk_test").unwrap();
    for path in [&root_path, &profile_path] {
      let written = std::fs::read_to_string(path).unwrap();
      assert!(written.contains("  provider: custom:requrv\n"), "{path:?}");
      assert!(written.contains("  default: requrv-small-3.8\n"), "{path:?}");
      assert!(written.contains(&format!("  base_url: {}\n", hive_openai_base_url())), "{path:?}");
      assert!(written.contains("providers:\n  requrv:\n    name: ReQurv\n"), "{path:?}");
      assert!(written.contains("    api_key: requrv_sk_test\n"), "{path:?}");
      assert!(written.contains("      requrv-small-3.8: {}\n"), "{path:?}");
      // Il resto del file resta intatto.
      assert!(written.contains("database:\n  journal_mode: wal\n"), "{path:?}");
      assert!(written.contains("custom_providers: []\n"), "{path:?}");
    }
    assert!(hermes_configured_at(&root));

    // Il ripristino torna all'originale su ogni home e rimuove i backup.
    restore_hermes_config_at(&root).unwrap();
    for path in [&root_path, &profile_path] {
      assert_eq!(std::fs::read_to_string(path).unwrap(), original);
      assert!(!backup_path_for(path).exists());
    }
    assert!(!hermes_configured_at(&root));
    let _ = std::fs::remove_dir_all(&root);
  }

  #[test]
  fn claude_env_pins_context_per_model() {
    let env = claude_hive_env("requrv-small-3.8", "requrv_sk_test");
    let map: std::collections::HashMap<&str, String> = env.into_iter().collect();
    assert_eq!(map.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"), Some(&"262144".to_string()));
    // All model tiers point at the selected model, not claude-* defaults.
    assert_eq!(map.get("ANTHROPIC_DEFAULT_OPUS_MODEL"), Some(&"requrv-small-3.8".to_string()));
    assert_eq!(map.get("ANTHROPIC_DEFAULT_SONNET_MODEL"), Some(&"requrv-small-3.8".to_string()));
    assert_eq!(map.get("ANTHROPIC_DEFAULT_HAIKU_MODEL"), Some(&"requrv-small-3.8".to_string()));
    assert_eq!(map.get("CLAUDE_CODE_SUBAGENT_MODEL"), Some(&"requrv-small-3.8".to_string()));
    let env = claude_hive_env("model-a", "requrv_sk_test");
    let map: std::collections::HashMap<&str, String> = env.into_iter().collect();
    assert_eq!(map.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"), Some(&"128000".to_string()));
  }

  // Il bundle macOS si chiama HERMES-IDE.app: il matcher copre le varianti di
  // casing e scarta le altre app.
  #[test]
  fn hermes_bundle_matcher_accepts_known_spellings() {
    assert!(is_hermes_bundle("HERMES-IDE.app"));
    assert!(is_hermes_bundle("Hermes IDE.app"));
    assert!(is_hermes_bundle("hermes.app"));
    assert!(!is_hermes_bundle("ChatGPT.app"));
    assert!(!is_hermes_bundle("hermes.txt"));
    assert!(!is_hermes_bundle("OpenHermesX"));
  }

  // Il bootstrap installer ha lo stesso nome del bundle dell'agente ma un
  // identificatore diverso: va scartato, altrimenti il lancio non fa nulla.
  #[test]
  fn setup_bundle_id_is_not_the_agent() {
    assert!(is_setup_bundle_id("com.nousresearch.hermes.setup"));
    assert!(!is_setup_bundle_id("com.nousresearch.hermes"));
    assert!(!is_setup_bundle_id("com.nousresearch.hermes.app"));
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn finds_hermes_bundle_among_applications() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-hermes-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create fake Applications");
    let bundle = tmp.join("HERMES-IDE.app");
    std::fs::create_dir_all(&bundle).expect("create fake bundle");
    std::fs::create_dir_all(tmp.join("ChatGPT.app")).expect("create other app");
    assert_eq!(find_hermes_bundle_in(&[tmp.clone()]), Some(bundle));
    let empty = tmp.join("vuoto");
    assert_eq!(find_hermes_bundle_in(&[empty]), None);
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // L'app desktop del checkout Hermes vive sotto apps/desktop/release/<arch>/,
  // non in /Applications: va trovata lì.
  #[test]
  fn finds_hermes_app_in_the_checkout_release_dir() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-hermes-repo-{}", std::process::id()));
    let bundle = tmp
      .join("hermes-agent")
      .join("apps")
      .join("desktop")
      .join("release")
      .join("mac-arm64")
      .join("Hermes.app");
    std::fs::create_dir_all(&bundle).expect("create fake bundle");
    assert_eq!(hermes_repo_app_path_in(&tmp), Some(bundle));
    assert_eq!(hermes_repo_app_path_in(&tmp.join("vuoto")), None);
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Il gateway risponde 422 al POST di prova con corpo vuoto (validazione del
  // body prima dell'invocazione del modello): qualsiasi 4xx dal handler prova
  // che il percorso esiste. Solo 404 e 5xx bloccano l'avvio.
  #[test]
  fn probe_treats_handler_rejections_as_available() {
    for status in [
      reqwest::StatusCode::OK,
      reqwest::StatusCode::UNAUTHORIZED,
      reqwest::StatusCode::FORBIDDEN,
      reqwest::StatusCode::METHOD_NOT_ALLOWED,
      reqwest::StatusCode::UNPROCESSABLE_ENTITY,
      reqwest::StatusCode::TOO_MANY_REQUESTS,
    ] {
      assert_eq!(probe_error(status, "Codex", "/responses"), None, "status {status}");
    }
    let missing = probe_error(reqwest::StatusCode::NOT_FOUND, "Claude Code", "/messages").expect("404");
    assert!(missing.contains("Claude Code"));
    assert!(missing.contains("/messages"));
    assert!(probe_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "Codex", "/responses").is_some());
    assert!(probe_error(reqwest::StatusCode::BAD_GATEWAY, "Codex", "/responses").is_some());
  }

  // Simula un processo con PATH vecchio/stripped (es. app lanciata da un
  // explorer partito prima dell'aggiornamento PATH): su Windows la
  // rilevazione deve comunque funzionare leggendo il PATH dal registry.
  #[test]
  fn finds_opencode_with_stale_path() {
    let original = std::env::var("PATH").unwrap_or_default();
    std::env::set_var(
      "PATH",
      if cfg!(windows) {
        "C:\\Windows\\system32"
      } else {
        "/usr/bin:/bin"
      },
    );
    let bin = find_service_binary("opencode");
    std::env::set_var("PATH", original);
    println!("opencode con PATH strippato: {bin:?}");
    if cfg!(windows) {
      assert!(bin.is_some(), "opencode non trovato con PATH strippato");
    }
  }

  // Simula un'installazione under nvm con PATH minimo e HOME fittizia (app
  // lanciata dal Dock/Finder): la rilevazione deve trovare il bin nella
  // directory bin della versione node, senza passare dal PATH.
  #[cfg(not(windows))]
  #[test]
  fn finds_codex_in_nvm_bin_with_minimal_path() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-nvm-{}", std::process::id()));
    let bin_dir = tmp.join(".nvm").join("versions").join("node").join("v24.0.0").join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create fake nvm bin");
    let fake = bin_dir.join("codex");
    std::fs::write(&fake, "#!/bin/sh\n").expect("write fake codex");

    let original_path = std::env::var("PATH").unwrap_or_default();
    let original_home = std::env::var("HOME").unwrap_or_default();
    std::env::set_var("PATH", "/usr/bin:/bin");
    std::env::set_var("HOME", &tmp);
    let bin = find_service_binary("codex");
    std::env::set_var("PATH", &original_path);
    std::env::set_var("HOME", &original_home);

    assert_eq!(bin, Some(fake));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn strips_jsonc_comments_outside_strings() {
    let input = r#"{
  // commento di riga
  "a": "https://hive.requrv.ai//x", /* commento di blocco */
  "b": 1
}"#;
    let cleaned = strip_jsonc_comments(input);
    let value: serde_json::Value = serde_json::from_str(&cleaned).expect("parsable");
    assert_eq!(value["a"], "https://hive.requrv.ai//x");
    assert_eq!(value["b"], 1);
  }

  #[test]
  fn strips_jsonc_keeps_slashes_inside_strings() {
    let input = r#"{"note": "dire // non è un commento", "url": "https://x.ai/y"}"#;
    let cleaned = strip_jsonc_comments(input);
    let value: serde_json::Value = serde_json::from_str(&cleaned).expect("parsable");
    assert_eq!(value["note"], "dire // non è un commento");
    assert_eq!(value["url"], "https://x.ai/y");
  }

  // Le virgole finali sono consentite in JSONC (formato di opencode) ma
  // rifiutate da serde_json: vanno rimosse fuori dalle stringhe, mentre una
  // `,}` dentro una stringa resta intatta.
  #[test]
  fn strips_trailing_commas_outside_strings() {
    let input = r#"{
  "mcp": {
    "nuxt": { "enabled": true, },
  },
  "list": [ "a", "b", ],
  "tricky": "x,} y,] z",
}"#;
    let cleaned = strip_trailing_commas(&strip_jsonc_comments(input));
    let value: serde_json::Value = serde_json::from_str(&cleaned).expect("parsable");
    assert_eq!(value["mcp"]["nuxt"]["enabled"], true);
    assert_eq!(value["list"][1], "b");
    assert_eq!(value["tricky"], "x,} y,] z");
  }

  #[test]
  fn merge_creates_provider_block_in_empty_config() {
    let mut config = serde_json::json!({});
    merge_hive_provider(&mut config, "requrv-small-3.8", "requrv_sk_test");
    assert_eq!(config["$schema"], "https://opencode.ai/config.json");
    let provider = &config["provider"]["requrv-hive"];
    assert_eq!(provider["npm"], "@ai-sdk/openai-compatible");
    assert_eq!(provider["name"], "ReQurv Hive");
    assert_eq!(provider["options"]["baseURL"], hive_openai_base_url().as_str());
    assert_eq!(provider["options"]["apiKey"], "requrv_sk_test");
    let entry = &provider["models"]["requrv-small-3.8"];
    assert_eq!(entry["name"], "requrv-small-3.8");
    assert_eq!(entry["limit"]["context"], 262_144);
    assert_eq!(entry["limit"]["output"], 32_768);
    assert_eq!(entry["modalities"]["input"], serde_json::json!(["text", "image", "video"]));
    assert_eq!(entry["modalities"]["output"], serde_json::json!(["text"]));
    assert_eq!(entry["reasoning"], true);
    assert_eq!(entry["options"]["reasoningEffort"], "medium");
    assert_eq!(entry["variants"]["high"]["disabled"], true);
    assert_eq!(entry["variants"]["xhigh"]["reasoningEffort"], "xhigh");
    assert_eq!(config["model"], "requrv-hive/requrv-small-3.8");
  }

  #[test]
  fn opencode_entry_is_name_only_for_unknown_models() {
    let mut config = serde_json::json!({});
    merge_hive_provider(&mut config, "model-x", "requrv_sk_test");
    assert_eq!(
      &config["provider"]["requrv-hive"]["models"]["model-x"],
      &serde_json::json!({ "name": "model-x" })
    );
    assert_eq!(config["model"], "requrv-hive/model-x");
  }

  #[test]
  fn merge_preserves_foreign_fields_and_updates_hive_block() {
    let mut config = serde_json::json!({
      "$schema": "https://opencode.ai/config.json",
      "permission": { "edit": "allow" },
      "model": "anthropic/claude-sonnet-4-6",
      "provider": {
        "anthropic": { "name": "Anthropic" },
        "requrv-hive": { "npm": "@ai-sdk/openai-compatible", "options": { "apiKey": "vecchia" } }
      }
    });
    merge_hive_provider(&mut config, "requrv-small-3.8", "requrv_sk_nuova");
    assert_eq!(config["permission"]["edit"], "allow");
    assert_eq!(config["provider"]["anthropic"]["name"], "Anthropic");
    assert_eq!(config["provider"]["requrv-hive"]["options"]["apiKey"], "requrv_sk_nuova");
    assert_eq!(config["provider"]["requrv-hive"]["models"]["requrv-small-3.8"]["name"], "requrv-small-3.8");
    assert_eq!(config["model"], "requrv-hive/requrv-small-3.8");
  }

  // Un provider in disabled_providers non viene caricato: il modello che
  // impostiamo diventerebbe "Model not found". Togliamo il nostro id e
  // lasciamo intatti quelli dell'utente.
  #[test]
  fn merge_re_enables_the_hive_provider() {
    let mut config = serde_json::json!({
      "disabled_providers": ["req_hive", "requrv-hive", "proxy-memory"],
      "provider": { "anthropic": { "name": "Anthropic" } }
    });
    merge_hive_provider(&mut config, "requrv-small-3.8", "requrv_sk_nuova");
    let disabled = config["disabled_providers"].as_array().expect("array");
    assert!(!disabled.contains(&serde_json::json!("requrv-hive")));
    assert!(disabled.contains(&serde_json::json!("req_hive")));
    assert!(disabled.contains(&serde_json::json!("proxy-memory")));
    assert_eq!(config["model"], "requrv-hive/requrv-small-3.8");
  }

  #[test]
  fn config_path_prefers_existing_jsonc() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-cfg-{}", std::process::id()));
    let dir = tmp.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(dir.join("opencode.jsonc"), "{}").expect("write jsonc");
    std::fs::write(dir.join("opencode.json"), "{}").expect("write json");
    let resolved = opencode_config_path_in(&tmp);
    assert!(resolved.to_string_lossy().ends_with("opencode.jsonc"));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn config_path_defaults_to_jsonc_when_missing() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-cfg-empty-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let resolved = opencode_config_path_in(&tmp);
    assert!(resolved.to_string_lossy().ends_with("opencode.jsonc"));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn writes_and_backs_up_opencode_config() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-write-{}", std::process::id()));
    let dir = tmp.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let existing = r#"{
  // commento
  "share": "manual",
  "provider": { "anthropic": { "name": "Anthropic" } }
}"#;
    std::fs::write(dir.join("opencode.jsonc"), existing).expect("write jsonc");

    let path = write_opencode_config_in(&tmp, "requrv-small-3.8", "requrv_sk_test").expect("write");
    assert!(path.to_string_lossy().ends_with("opencode.jsonc"));

    let rendered = std::fs::read_to_string(&path).expect("read back");
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");
    assert_eq!(value["share"], "manual");
    assert_eq!(value["provider"]["anthropic"]["name"], "Anthropic");
    assert_eq!(value["provider"]["requrv-hive"]["options"]["apiKey"], "requrv_sk_test");
    assert_eq!(value["model"], "requrv-hive/requrv-small-3.8");
    assert!(!rendered.contains("commento"));

    let backup = dir.join("opencode.jsonc.bak");
    let backed = std::fs::read_to_string(&backup).expect("backup exists");
    assert!(backed.contains("commento"));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Il backup deve restare lo stato pre-Hive: un rilancio con un altro modello
  // non può sovrascriverlo col file già su Hive.
  #[test]
  fn opencode_backup_keeps_the_pre_hive_original_across_launches() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-oc-bak-{}", std::process::id()));
    let dir = tmp.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let original = r#"{ "theme": "dark" }"#;
    std::fs::write(dir.join("opencode.jsonc"), original).expect("write jsonc");

    write_opencode_config_in(&tmp, "requrv-small-3.8", "requrv_sk_test").expect("first write");
    write_opencode_config_in(&tmp, "requrv-medium-4", "requrv_sk_test2").expect("second write");

    let backed = std::fs::read_to_string(dir.join("opencode.jsonc.bak")).expect("backup exists");
    assert_eq!(backed, original);
    assert!(opencode_configured_in(&tmp));

    restore_opencode_config_in(&tmp).expect("restore");
    assert_eq!(std::fs::read_to_string(dir.join("opencode.jsonc")).unwrap(), original);
    assert!(!dir.join("opencode.jsonc.bak").exists());
    assert!(!opencode_configured_in(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Senza backup il ripristino toglie solo ciò che è nostro: provider e modello
  // scelti per altri provider restano intatti.
  #[test]
  fn opencode_restore_without_backup_keeps_foreign_providers_and_models() {
    let tmp =
      std::env::temp_dir().join(format!("requrv-bridge-test-oc-strip-{}", std::process::id()));
    let dir = tmp.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    write_opencode_config_in(&tmp, "requrv-small-3.8", "requrv_sk_test").expect("write");

    let mut config: serde_json::Value = serde_json::from_str(
      &std::fs::read_to_string(dir.join("opencode.jsonc")).expect("read back"),
    )
    .expect("valid json");
    config["provider"]["anthropic"] = serde_json::json!({ "name": "Anthropic" });
    config["model"] = serde_json::json!("anthropic/claude-sonnet-4");
    std::fs::write(dir.join("opencode.jsonc"), config.to_string()).expect("write back");

    restore_opencode_config_in(&tmp).expect("restore");
    let restored: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.jsonc")).unwrap())
        .expect("valid json");
    assert!(restored["provider"].get("requrv-hive").is_none());
    assert_eq!(restored["provider"]["anthropic"]["name"], "Anthropic");
    assert_eq!(restored["model"], "anthropic/claude-sonnet-4");
    assert!(!opencode_configured_in(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Un config vuoto prima della configurazione: il ripristino rimuove il file
  // invece di lasciare una config vuota.
  #[test]
  fn opencode_restore_removes_the_file_when_nothing_else_is_left() {
    let tmp =
      std::env::temp_dir().join(format!("requrv-bridge-test-oc-empty-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    write_opencode_config_in(&tmp, "requrv-small-3.8", "requrv_sk_test").expect("write");
    restore_opencode_config_in(&tmp).expect("restore");
    assert!(!opencode_config_path_in(&tmp).exists());
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Caso reale: una config JSONC piena di virgole finali (es. blocco mcp) che
  // prima bloccava l'avvio. Deve essere letta, il provider fuso e il file
  // riscritto come JSON valido, preservando i campi esterni.
  #[test]
  fn writes_opencode_config_with_trailing_commas() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-write-tc-{}", std::process::id()));
    let dir = tmp.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let existing = r#"{
  "mcp": {
    "nuxt": {
      "type": "remote",
      "url": "https://nuxt.com/mcp",
      "enabled": true,
    },
  },
}"#;
    std::fs::write(dir.join("opencode.jsonc"), existing).expect("write jsonc");

    let path = write_opencode_config_in(&tmp, "requrv-small-3.8", "requrv_sk_test").expect("write");
    let rendered = std::fs::read_to_string(&path).expect("read back");
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");
    assert_eq!(value["mcp"]["nuxt"]["url"], "https://nuxt.com/mcp");
    assert_eq!(value["mcp"]["nuxt"]["enabled"], true);
    assert_eq!(value["provider"]["requrv-hive"]["options"]["apiKey"], "requrv_sk_test");
    assert_eq!(value["model"], "requrv-hive/requrv-small-3.8");
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn finds_desktop_app_bundle_among_candidates() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-app-{}", std::process::id()));
    let bundle = tmp.join("OpenCode.app");
    std::fs::create_dir_all(&bundle).expect("create fake bundle");
    let missing = tmp.join("Assente.app");
    assert_eq!(app_bundle_path_in(&[missing.clone(), bundle.clone()]), Some(bundle));
    assert_eq!(app_bundle_path_in(&[missing]), None);
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn finds_chatgpt_bundle_among_candidates() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-bundle-{}", std::process::id()));
    let bundle = tmp.join("ChatGPT.app");
    std::fs::create_dir_all(&bundle).expect("create fake bundle");
    let missing = tmp.join("Assente.app");
    assert_eq!(app_bundle_path_in(&[missing.clone(), bundle.clone()]), Some(bundle));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn finds_codex_binary_in_chatgpt_bundle() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-{}", std::process::id()));
    let resources = tmp.join("ChatGPT.app").join("Contents").join("Resources");
    std::fs::create_dir_all(&resources).expect("create fake bundle");
    std::fs::write(resources.join("codex"), "fake").expect("write fake codex");
    let bundle = tmp.join("ChatGPT.app");
    let expected = bundle.join("Contents").join("Resources").join("codex");
    let missing = tmp.join("Assente.app");
    assert_eq!(codex_app_binary_in(&[missing.clone(), bundle.clone()]), Some(expected));
    assert_eq!(codex_app_binary_in(&[missing]), None);
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn builds_hive_model_catalog_with_text_models_only() {
    let models = vec![
      HiveModel { id: "model-a".into(), model_type: "TEXT_GENERATION".into() },
      HiveModel { id: "image-x".into(), model_type: "IMAGE_GENERATION".into() },
      HiveModel { id: "model-b".into(), model_type: "TEXT_GENERATION".into() },
    ];
    let catalog = build_hive_catalog(&models);
    let entries = catalog["models"].as_array().expect("models array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["slug"], "model-a");
    assert_eq!(entries[1]["slug"], "model-b");
    assert_eq!(entries[0]["display_name"], "model-a");
    assert_eq!(entries[0]["supported_in_api"], true);
    assert_eq!(entries[0]["context_window"], 128_000);
    // The app schema requires the reasoning fields even when empty.
    assert_eq!(entries[0]["supported_reasoning_levels"].as_array().unwrap().len(), 0);
    assert!(entries[0].get("default_reasoning_level").is_some());
  }

  #[test]
  fn hive_catalog_declares_known_model_capabilities() {
    let models = vec![HiveModel {
      id: "requrv-small-3.8".into(),
      model_type: "TEXT_GENERATION".into(),
    }];
    let catalog = build_hive_catalog(&models);
    let entry = &catalog["models"][0];
    assert_eq!(entry["context_window"], 262_144);
    assert_eq!(entry["max_context_window"], 262_144);
    // Video is a Hive modality but not a codex one: the catalog drops it.
    assert_eq!(entry["input_modalities"], serde_json::json!(["text", "image"]));
    assert_eq!(entry["default_reasoning_level"], "xhigh");
    // The catalog schema requires {effort, description} objects, not strings.
    assert_eq!(
      entry["supported_reasoning_levels"],
      serde_json::json!([
        { "effort": "low", "description": "Fast responses with lighter reasoning" },
        { "effort": "medium", "description": "Balances speed and reasoning depth for everyday tasks" },
        { "effort": "xhigh", "description": "Extra high reasoning depth for complex problems" }
      ])
    );
  }

  // Una sola tabella provider serve entrambe le superfici: il bearer token
  // autentica sia l'app sia la CLI, senza env_key (codex rifiuta di partire
  // se la variabile dichiarata manca).
  #[test]
  fn codex_provider_carries_bearer_token_for_app_and_cli() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-codex-key-{}", std::process::id()));
    let models = vec![HiveModel { id: "requrv-small-3.8".into(), model_type: "TEXT_GENERATION".into() }];
    configure_chatgpt_app_in(&tmp, "requrv-small-3.8", &models, "requrv_sk_test").unwrap();
    let table: toml::Table = std::fs::read_to_string(codex_config_path_in(&tmp))
      .unwrap()
      .parse()
      .unwrap();
    let provider = hive_provider_table(&table).expect("hive provider table");
    assert_eq!(provider["experimental_bearer_token"].as_str(), Some("requrv_sk_test"));
    assert!(provider.get("env_key").is_none());
    assert!(chatgpt_app_configured_in(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // I file delle versioni precedenti (profilo CLI e catalogo dedicato) non
  // lasciano spazzatura in ~/.codex dopo il ripristino.
  #[test]
  fn restore_chatgpt_app_removes_legacy_cli_files() {
    let tmp =
      std::env::temp_dir().join(format!("requrv-bridge-test-codex-legacy-{}", std::process::id()));
    let dir = tmp.join(".codex");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(dir.join("hive.config.toml"), "model = \"x\"\n").unwrap();
    std::fs::write(dir.join("hive.config.bak"), "model = \"y\"\n").unwrap();
    std::fs::write(dir.join("hive-cli-models.json"), "{}\n").unwrap();

    restore_chatgpt_app_in(&tmp, "requrv_sk_test").unwrap();

    assert!(!dir.join("hive.config.toml").exists());
    assert!(!dir.join("hive.config.bak").exists());
    assert!(!dir.join("hive-cli-models.json").exists());
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // model_catalog_json esiste da Codex 0.134.0: sotto, il CLI parte con un
  // errore di config opaco, quindi il launcher blocca prima.
  #[test]
  fn codex_version_gate_accepts_only_new_enough_clis() {
    assert!(!codex_version_ok(""));
    assert!(!codex_version_ok("0.133.9"));
    assert!(!codex_version_ok("0.99.0"));
    assert!(codex_version_ok("0.134.0"));
    assert!(codex_version_ok("0.140.2"));
    assert!(codex_version_ok("1.0.0"));
  }

  #[test]
  fn hive_catalog_falls_back_to_full_list_without_type() {
    let models = vec![HiveModel { id: "model-x".into(), model_type: String::new() }];
    let catalog = build_hive_catalog(&models);
    assert_eq!(catalog["models"].as_array().expect("models array").len(), 1);
  }

  #[test]
  fn configure_chatgpt_app_writes_config_catalog_auth_and_backups() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-cfg-{}", std::process::id()));
    let dir = tmp.join(".codex");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let original_config = format!(
      "model = \"gpt-5-codex\"\nnotify = [\"bar\"]\nopenai_base_url = \"{}\"\n[desktop]\ntheme = \"dark\"\n",
      hive_openai_base_url()
    );
    let original_auth = r#"{"auth_mode": "chatgpt"}"#;
    std::fs::write(dir.join("config.toml"), original_config.as_str()).expect("write config");
    std::fs::write(dir.join("auth.json"), original_auth).expect("write auth");

    let models = vec![HiveModel { id: "model-a".into(), model_type: "TEXT_GENERATION".into() }];
    configure_chatgpt_app_in(&tmp, "model-a", &models, "requrv_sk_test").expect("configure");

    let rendered = std::fs::read_to_string(dir.join("config.toml")).expect("read config back");
    let value: toml::Table = rendered.parse().expect("valid toml");
    assert_eq!(value.get("model").and_then(|v| v.as_str()), Some("model-a"));
    assert_eq!(value.get("model_provider").and_then(|v| v.as_str()), Some(HIVE_PROVIDER_ID));
    // The legacy root key is superseded by the provider table.
    assert!(value.get("openai_base_url").is_none());
    assert!(value
      .get("model_catalog_json")
      .and_then(|v| v.as_str())
      .unwrap()
      .ends_with("hive-models.json"));
    let provider = value
      .get("model_providers")
      .and_then(|v| v.as_table())
      .and_then(|p| p.get(HIVE_PROVIDER_ID))
      .and_then(|v| v.as_table())
      .expect("hive provider table");
    assert_eq!(provider.get("base_url").and_then(|v| v.as_str()), Some(hive_openai_base_url().as_str()));
    assert_eq!(provider.get("wire_api").and_then(|v| v.as_str()), Some("responses"));
    assert_eq!(provider.get("supports_websockets").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
      provider.get("experimental_bearer_token").and_then(|v| v.as_str()),
      Some("requrv_sk_test")
    );
    assert_eq!(value.get("notify").and_then(|v| v.as_array()).unwrap()[0].as_str(), Some("bar"));
    assert_eq!(
      value.get("desktop").and_then(|v| v.as_table()).and_then(|t| t.get("theme")).and_then(|v| v.as_str()),
      Some("dark")
    );

    let catalog_raw = std::fs::read_to_string(dir.join("hive-models.json")).expect("read catalog");
    let catalog: serde_json::Value = serde_json::from_str(&catalog_raw).expect("valid catalog");
    assert_eq!(catalog["models"][0]["slug"], "model-a");

    let auth_raw = std::fs::read_to_string(dir.join("auth.json")).expect("read auth back");
    let auth: serde_json::Value = serde_json::from_str(&auth_raw).expect("valid auth");
    assert_eq!(auth["auth_mode"], "apikey");
    assert_eq!(auth["OPENAI_API_KEY"], "requrv_sk_test");

    assert_eq!(
      std::fs::read_to_string(dir.join("config.toml.hive.bak")).expect("config backup"),
      original_config
    );
    assert_eq!(
      std::fs::read_to_string(dir.join("auth.json.hive.bak")).expect("auth backup"),
      original_auth
    );

    assert!(chatgpt_app_configured_in(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn configure_twice_keeps_the_original_backup() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-again-{}", std::process::id()));
    let dir = tmp.join(".codex");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let original = "model = \"gpt-5-codex\"\n";
    std::fs::write(dir.join("config.toml"), original).expect("write config");

    let models = vec![
      HiveModel { id: "model-a".into(), model_type: "TEXT_GENERATION".into() },
      HiveModel { id: "model-b".into(), model_type: "TEXT_GENERATION".into() },
    ];
    configure_chatgpt_app_in(&tmp, "model-a", &models, "requrv_sk_test").expect("first configure");
    configure_chatgpt_app_in(&tmp, "model-b", &models, "requrv_sk_other").expect("second configure");

    let value: toml::Table = std::fs::read_to_string(dir.join("config.toml")).expect("read back").parse().expect("valid toml");
    assert_eq!(value.get("model").and_then(|v| v.as_str()), Some("model-b"));
    assert_eq!(std::fs::read_to_string(dir.join("config.toml.hive.bak")).expect("original backup"), original);
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Utente senza ~/.codex: il primo configure non ha nulla da salvare, e i
  // successivi (lancio CLI, cambio di chiave) non devono salvare come
  // "originale" il config.toml già su Hive.
  #[test]
  fn configure_on_scratch_home_never_backs_up_the_hive_config() {
    let tmp =
      std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-scratch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let dir = tmp.join(".codex");
    let models = vec![HiveModel { id: "model-a".into(), model_type: "TEXT_GENERATION".into() }];

    configure_chatgpt_app_in(&tmp, "model-a", &models, "requrv_sk_test").expect("first configure");
    configure_chatgpt_app_in(&tmp, "model-a", &models, "requrv_sk_other").expect("second configure");
    let failures = reapply_persisted_configs(&tmp, "model-a", "requrv_sk_third", &[]);
    assert!(failures.is_empty(), "{failures:?}");

    assert!(!dir.join("config.toml.hive.bak").exists());
    assert!(!dir.join("auth.json.hive.bak").exists());
    let table: toml::Table = std::fs::read_to_string(dir.join("config.toml")).unwrap().parse().unwrap();
    let provider = hive_provider_table(&table).expect("hive provider");
    assert_eq!(provider["experimental_bearer_token"].as_str(), Some("requrv_sk_third"));

    // Il ripristino su una config creata da noi deve azzerare tutto.
    restore_chatgpt_app_in(&tmp, "requrv_sk_third").unwrap();
    assert!(!dir.join("config.toml").exists());
    assert!(!dir.join("auth.json").exists());
    assert!(!dir.join("hive-models.json").exists());
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Un catalogo già buono non deve essere svuotato da un elenco modelli vuoto
  // (la lista non è arrivata dal gateway): il picker resterebbe senza modelli.
  #[test]
  fn empty_model_list_keeps_the_existing_catalog() {
    let tmp =
      std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-catalog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let models = vec![
      HiveModel { id: "model-a".into(), model_type: "TEXT_GENERATION".into() },
      HiveModel { id: "model-b".into(), model_type: "TEXT_GENERATION".into() },
    ];
    configure_chatgpt_app_in(&tmp, "model-a", &models, "requrv_sk_test").expect("configure");
    let catalog = codex_catalog_path_in(&tmp);
    let before = std::fs::read_to_string(&catalog).unwrap();

    configure_chatgpt_app_in(&tmp, "model-b", &[], "requrv_sk_other").expect("reconfigure without models");

    assert_eq!(std::fs::read_to_string(&catalog).unwrap(), before);
    // La chiave arriva comunque: è il punto del riallineamento.
    let table: toml::Table = std::fs::read_to_string(codex_config_path_in(&tmp)).unwrap().parse().unwrap();
    assert_eq!(
      hive_provider_table(&table).unwrap()["experimental_bearer_token"].as_str(),
      Some("requrv_sk_other")
    );
    assert_eq!(table.get("model").and_then(|v| v.as_str()), Some("model-b"));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn restore_chatgpt_app_roundtrip() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-restore-{}", std::process::id()));
    let dir = tmp.join(".codex");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let original_config = "model = \"gpt-5-codex\"\n";
    let original_auth = r#"{"auth_mode": "chatgpt"}"#;
    std::fs::write(dir.join("config.toml"), original_config).expect("write config");
    std::fs::write(dir.join("auth.json"), original_auth).expect("write auth");

    let models = vec![HiveModel { id: "model-a".into(), model_type: "TEXT_GENERATION".into() }];
    configure_chatgpt_app_in(&tmp, "model-a", &models, "requrv_sk_test").expect("configure");
    restore_chatgpt_app_in(&tmp, "requrv_sk_test").expect("restore");

    assert_eq!(std::fs::read_to_string(dir.join("config.toml")).expect("config restored"), original_config);
    assert_eq!(std::fs::read_to_string(dir.join("auth.json")).expect("auth restored"), original_auth);
    assert!(!dir.join("config.toml.hive.bak").exists());
    assert!(!dir.join("auth.json.hive.bak").exists());
    assert!(!dir.join("hive-models.json").exists());
    assert!(!chatgpt_app_configured_in(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn restore_without_backup_strips_legacy_hive_keys_and_own_auth() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-strip-{}", std::process::id()));
    let dir = tmp.join(".codex");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let configured = format!(
      "model = \"model-a\"\nnotify = [\"bar\"]\nopenai_base_url = \"{}\"\nmodel_catalog_json = \"{}\"\n",
      hive_openai_base_url(),
      dir.join("hive-models.json").display()
    );
    std::fs::write(dir.join("config.toml"), configured).expect("write config");
    std::fs::write(dir.join("auth.json"), r#"{"OPENAI_API_KEY": "requrv_sk_test", "auth_mode": "apikey"}"#)
      .expect("write auth");

    restore_chatgpt_app_in(&tmp, "requrv_sk_test").expect("restore");

    let value: toml::Table = std::fs::read_to_string(dir.join("config.toml")).expect("config kept").parse().expect("valid toml");
    assert!(value.get("model").is_none());
    assert!(value.get("model_provider").is_none());
    assert!(value.get("openai_base_url").is_none());
    assert!(value.get("model_catalog_json").is_none());
    assert_eq!(value.get("notify").and_then(|v| v.as_array()).unwrap()[0].as_str(), Some("bar"));
    assert!(!dir.join("auth.json").exists());
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn restore_without_backup_strips_hive_provider_and_keeps_foreign_ones() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-provider-{}", std::process::id()));
    let dir = tmp.join(".codex");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let configured = format!(
      "model = \"model-a\"\nnotify = [\"bar\"]\nmodel_provider = \"{HIVE_PROVIDER_ID}\"\nmodel_catalog_json = \"{}\"\n\n[model_providers.{HIVE_PROVIDER_ID}]\nbase_url = \"{}\"\nwire_api = \"responses\"\nsupports_websockets = false\n\n[model_providers.other]\nbase_url = \"https://example.com/v1\"\n",
      dir.join("hive-models.json").display(),
      hive_openai_base_url()
    );
    std::fs::write(dir.join("config.toml"), configured).expect("write config");

    restore_chatgpt_app_in(&tmp, "requrv_sk_test").expect("restore");

    let value: toml::Table = std::fs::read_to_string(dir.join("config.toml")).expect("config kept").parse().expect("valid toml");
    assert!(value.get("model").is_none());
    assert!(value.get("model_provider").is_none());
    assert!(value.get("model_catalog_json").is_none());
    let providers = value.get("model_providers").and_then(|v| v.as_table()).expect("providers kept");
    assert!(providers.get(HIVE_PROVIDER_ID).is_none());
    assert_eq!(
      providers.get("other").and_then(|p| p.as_table()).and_then(|p| p.get("base_url")).and_then(|v| v.as_str()),
      Some("https://example.com/v1")
    );
    assert_eq!(value.get("notify").and_then(|v| v.as_array()).unwrap()[0].as_str(), Some("bar"));
    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn restore_without_backup_keeps_foreign_auth() {
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-chatgpt-auth-{}", std::process::id()));
    let dir = tmp.join(".codex");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let configured = format!("model = \"model-a\"\nopenai_base_url = \"{}\"\n", hive_openai_base_url());
    std::fs::write(dir.join("config.toml"), configured).expect("write config");
    let foreign_auth = r#"{"OPENAI_API_KEY": "sk-altra-chiave", "auth_mode": "apikey"}"#;
    std::fs::write(dir.join("auth.json"), foreign_auth).expect("write auth");

    restore_chatgpt_app_in(&tmp, "requrv_sk_test").expect("restore");

    assert_eq!(std::fs::read_to_string(dir.join("auth.json")).expect("auth kept"), foreign_auth);
    let _ = std::fs::remove_dir_all(&tmp);
  }

  // Confronto versioni per il check di aggiornamento: nessun falso positivo
  // su versioni uguali, precedenti o male formattati.
  #[test]
  fn version_comparison_detects_newer_releases() {
    assert!(!is_newer_version("0.1.3", "0.1.3"));
    assert!(is_newer_version("0.1.4", "0.1.3"));
    assert!(is_newer_version("0.2.0", "0.1.9"));
    assert!(is_newer_version("1.0.0", "0.9.9"));
    assert!(!is_newer_version("0.1.2", "0.1.3"));
    assert!(!is_newer_version("0.0.9", "0.1.0"));
    assert!(is_newer_version("0.10.0", "0.9.9"));
  }

  #[test]
  fn version_comparison_handles_uneven_and_invalid_versions() {
    assert!(!is_newer_version("1.2", "1.2.0"));
    assert!(is_newer_version("1.2.1", "1.2"));
    assert!(!is_newer_version("0.1.4-beta", "0.1.3"));
    assert!(!is_newer_version("", "0.1.3"));
    assert!(!is_newer_version("0.1.3", ""));
    assert!(!is_newer_version("non-a-versione", "0.1.3"));
  }

  // Home isolata per i test della persistenza di ~/.claude/settings.json.
  fn claude_code_test_home() -> PathBuf {
    // A counter, not the clock: parallel tests in the same process can read
    // the same timestamp and would then share (and clobber) one temp home.
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-cc-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    tmp
  }

  // Claude Desktop: configure writes the managed 3p profile and flips both
  // configs to 3p; restore returns every file to its pre-Hive state.
  fn claude_test_home() -> PathBuf {
    // Counter, not clock: parallel tests can read the same timestamp and would
    // then share (and clobber) one temp home.
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("requrv-bridge-test-cd-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let support = application_support_root(&tmp);
    std::fs::create_dir_all(support.join("Claude")).unwrap();
    std::fs::create_dir_all(support.join("Claude-3p")).unwrap();
    // Pre-existing user state that restore must bring back.
    std::fs::write(
      support.join("Claude/claude_desktop_config.json"),
      r#"{"deploymentMode":"1p","custom":true}"#,
    )
    .unwrap();
    std::fs::write(
      support.join("Claude-3p/claude_desktop_config.json"),
      r#"{"deploymentMode":"1p"}"#,
    )
    .unwrap();
    tmp
  }

  #[test]
  fn claude_desktop_configure_and_restore_roundtrip() {
    let home = claude_test_home();
    let paths = claude_desktop_paths_in(&home);

    assert!(!claude_desktop_configured_in(&home));
    configure_claude_desktop_in(&home, "requrv-small-3.8", "requrv_sk_test").unwrap();

    // Both configs are in 3p mode and the managed profile is applied.
    let normal = serde_json::from_str::<serde_json::Value>(
      &std::fs::read_to_string(&paths.normal_config).unwrap(),
    )
    .unwrap();
    assert_eq!(normal["deploymentMode"], "3p");
    // User keys survive the configure.
    assert_eq!(normal["custom"], true);
    let profile = serde_json::from_str::<serde_json::Value>(
      &std::fs::read_to_string(&paths.profile).unwrap(),
    )
    .unwrap();
    assert_eq!(profile["inferenceProvider"], "gateway");
    assert_eq!(profile["inferenceGatewayBaseUrl"], hive_anthropic_base_url().as_str());
    assert_eq!(profile["inferenceGatewayApiKey"], "requrv_sk_test");
    assert_eq!(profile["inferenceModels"][0]["name"], CLAUDE_DESKTOP_SLOT);
    assert_eq!(
      profile["inferenceModels"][0]["labelOverride"],
      "requrv-small-3.8 (ReQurv)"
    );
    assert!(claude_desktop_configured_in(&home));

    // Backups were taken for the pre-existing files.
    assert!(paths.normal_config.with_file_name("claude_desktop_config.json.hive.bak").exists());

    restore_claude_desktop_in(&home).unwrap();

    // The original user config is back, the managed profile is gone, and the
    // app is no longer flagged as configured.
    let restored = serde_json::from_str::<serde_json::Value>(
      &std::fs::read_to_string(&paths.normal_config).unwrap(),
    )
    .unwrap();
    assert_eq!(restored["deploymentMode"], "1p");
    assert_eq!(restored["custom"], true);
    assert!(!paths.profile.exists());
    assert!(!claude_desktop_configured_in(&home));
    let _ = std::fs::remove_dir_all(&home);
  }

  // A profile created from scratch (no pre-existing file) is removed on
  // restore rather than restored from a backup.
  #[test]
  fn claude_desktop_restore_removes_scratch_profile() {
    let home = claude_test_home();
    let paths = claude_desktop_paths_in(&home);
    // Remove the 3p config so the profile/meta are created from nothing.
    std::fs::remove_file(&paths.third_party_config).unwrap();
    configure_claude_desktop_in(&home, "model-a", "requrv_sk_test").unwrap();
    assert!(paths.profile.exists());
    restore_claude_desktop_in(&home).unwrap();
    assert!(!paths.profile.exists());
    let _ = std::fs::remove_dir_all(&home);
  }

  // PoC against the REAL home (not run by default):
  //   cargo test claude_desktop_poc_real_home -- --ignored --nocapture
  // Point Claude Desktop at the local proxy for the desktop app validation.
  #[test]
  #[ignore]
  fn claude_desktop_poc_real_home() {
    let home = home_dir().expect("home dir");
    let key = std::env::var("POC_HIVE_KEY")
      .unwrap_or_else(|_| "requrv_sk_localtest0000000000000000000000".to_string());
    let model = std::env::var("POC_HIVE_MODEL").unwrap_or_else(|_| "requrv-small-3.8".to_string());
    configure_claude_desktop_in(&home, &model, &key).unwrap();
    let paths = claude_desktop_paths_in(&home);
    println!("profile: {}", std::fs::read_to_string(&paths.profile).unwrap());
    println!("meta:    {}", std::fs::read_to_string(&paths.meta).unwrap());
    println!("configured: {}", claude_desktop_configured_in(&home));
  }

  // PoC restore against the REAL home (not run by default):
  //   cargo test claude_desktop_poc_restore_real_home -- --ignored --nocapture
  #[test]
  #[ignore]
  fn claude_desktop_poc_restore_real_home() {
    let home = home_dir().expect("home dir");
    restore_claude_desktop_in(&home).unwrap();
    println!("configured after restore: {}", claude_desktop_configured_in(&home));
  }

  // MCP: write/strip a server into each native client config, preserving
  // foreign entries and using OpenCode's local-server shape.
  fn mcp_test_home() -> PathBuf {
    // Counter, not clock: parallel tests must not share a temp home.
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp =
      std::env::temp_dir().join(format!("requrv-bridge-test-mcp-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    tmp
  }

  fn mcp_server_fixture() -> McpServer {
    let env = serde_json::json!({"NEXTCLOUD_URL": "https://nc.local"})
      .as_object()
      .unwrap()
      .clone();
    McpServer {
      id: "nextcloud".into(),
      name: "Nextcloud".into(),
      command: "nextcloud-mcp-server".into(),
      args: vec!["run".into(), "--transport".into(), "stdio".into()],
      env,
      targets: vec!["claude_code".into(), "opencode".into(), "claude_desktop".into()],
    }
  }

  #[test]
  fn mcp_claude_code_roundtrip() {
    let home = mcp_test_home();
    let path = home.join(".claude.json");
    std::fs::write(
      &path,
      r#"{"theme":"dark","mcpServers":{"existing":{"command":"existing-mcp","args":[]}}}"#,
    )
    .unwrap();

    write_mcp_target_in(&home, "claude_code", &mcp_server_fixture()).unwrap();

    let cfg: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(cfg["theme"], "dark");
    assert_eq!(cfg["mcpServers"]["existing"]["command"], "existing-mcp");
    assert_eq!(cfg["mcpServers"]["nextcloud"]["command"], "nextcloud-mcp-server");
    assert_eq!(cfg["mcpServers"]["nextcloud"]["args"][0], "run");
    assert_eq!(
      cfg["mcpServers"]["nextcloud"]["env"]["NEXTCLOUD_URL"],
      "https://nc.local"
    );

    strip_mcp_target_in(&home, "claude_code", "nextcloud").unwrap();
    let cfg: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(cfg["mcpServers"].get("nextcloud").is_none());
    assert_eq!(cfg["mcpServers"]["existing"]["command"], "existing-mcp");
    assert_eq!(cfg["theme"], "dark");
    let _ = std::fs::remove_dir_all(&home);
  }

  #[test]
  fn mcp_opencode_roundtrip_keeps_foreign_keys() {
    let home = mcp_test_home();
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("opencode.jsonc");
    std::fs::write(
      &path,
      r#"// machine-generated
{
  "$schema": "https://opencode.ai/config.json",
  "provider": { "requrv-hive": { "npm": "@ai-sdk/openai-compatible" } },
}"#,
    )
    .unwrap();

    write_mcp_target_in(&home, "opencode", &mcp_server_fixture()).unwrap();

    let read_cfg = |home: &Path| -> serde_json::Value {
      let raw = std::fs::read_to_string(&home.join(".config/opencode/opencode.jsonc")).unwrap();
      serde_json::from_str(&strip_trailing_commas(&strip_jsonc_comments(&raw))).unwrap()
    };
    let cfg = read_cfg(&home);
    assert_eq!(cfg["$schema"], "https://opencode.ai/config.json");
    assert_eq!(cfg["provider"]["requrv-hive"]["npm"], "@ai-sdk/openai-compatible");
    let entry = &cfg["mcp"]["nextcloud"];
    assert_eq!(entry["type"], "local");
    assert_eq!(entry["enabled"], true);
    assert_eq!(entry["command"][0], "nextcloud-mcp-server");
    assert_eq!(entry["command"][1], "run");
    assert_eq!(entry["environment"]["NEXTCLOUD_URL"], "https://nc.local");

    strip_mcp_target_in(&home, "opencode", "nextcloud").unwrap();
    let cfg = read_cfg(&home);
    // The now-empty mcp container is dropped, everything else survives.
    assert!(cfg.get("mcp").is_none());
    assert_eq!(cfg["provider"]["requrv-hive"]["npm"], "@ai-sdk/openai-compatible");
    let _ = std::fs::remove_dir_all(&home);
  }

  #[test]
  fn mcp_claude_desktop_roundtrip() {
    let home = mcp_test_home();
    let support = application_support_root(&home);
    std::fs::create_dir_all(support.join("Claude")).unwrap();
    let path = support.join("Claude/claude_desktop_config.json");
    std::fs::write(&path, r#"{"deploymentMode":"1p"}"#).unwrap();

    write_mcp_target_in(&home, "claude_desktop", &mcp_server_fixture()).unwrap();

    let cfg: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(cfg["deploymentMode"], "1p");
    assert_eq!(cfg["mcpServers"]["nextcloud"]["command"], "nextcloud-mcp-server");
    assert_eq!(cfg["mcpServers"]["nextcloud"]["env"]["NEXTCLOUD_URL"], "https://nc.local");

    strip_mcp_target_in(&home, "claude_desktop", "nextcloud").unwrap();
    let cfg: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(cfg.get("mcpServers").is_none());
    assert_eq!(cfg["deploymentMode"], "1p");
    let _ = std::fs::remove_dir_all(&home);
  }

  // Stripping from a file that never had the entry is a no-op and does not
  // create the file.
  #[test]
  fn mcp_strip_missing_is_noop() {
    let home = mcp_test_home();
    let path = home.join(".claude.json");
    assert!(!path.exists());
    strip_mcp_target_in(&home, "claude_code", "nextcloud").unwrap();
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&home);
  }
}
