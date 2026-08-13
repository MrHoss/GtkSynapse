//! Ollama local runtime manager.
//!
//! Wraps the `ollama` CLI so the application can detect/install it, keep the
//! local server running (auto-started when the app opens), and pull (download)
//! new models — all without the user touching a terminal.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TCommand;

/// Default endpoint where the Ollama REST API listens.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

// ─── OS / environment detection ─────────────────────────────────

/// Human-readable name of the current OS/distro for UI messages.
pub fn os_description() -> String {
    if cfg!(target_os = "windows") {
        return "Windows".to_string();
    }
    if cfg!(target_os = "macos") {
        return "macOS".to_string();
    }
    os_release_field("PRETTY_NAME")
        .or_else(|| os_release_field("NAME"))
        .unwrap_or_else(|| "Linux".to_string())
}

/// Read a field from `/etc/os-release` (Linux).
fn os_release_field(field: &str) -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in content.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if key == field {
                return Some(value.trim_matches('"').to_string());
            }
        }
    }
    None
}

// ─── Installation ───────────────────────────────────────────────

/// The command that installs Ollama on this machine.
///
/// The official one-line installer is used as a unified alternative to
/// per-distro package managers: it detects the Linux distribution (or macOS)
/// and installs the right binary automatically. On Windows the official
/// installer executable is downloaded and launched.
pub fn install_command() -> Vec<String> {
    if cfg!(target_os = "windows") {
        vec![
            "powershell".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Invoke-WebRequest -Uri https://ollama.com/download/OllamaSetup.exe -OutFile \"$env:TEMP\\OllamaSetup.exe\"; Start-Process \"$env:TEMP\\OllamaSetup.exe\"".to_string(),
        ]
    } else {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "curl -fsSL https://ollama.com/install.sh | sh".to_string(),
        ]
    }
}

/// Locate the `ollama` executable (PATH lookup first, then well-known paths).
pub fn find_binary() -> Option<PathBuf> {
    if let Ok(out) = std::process::Command::new("ollama").arg("--version").output() {
        if out.status.success() {
            return Some(PathBuf::from("ollama"));
        }
    }
    const KNOWN: &[&str] = &[
        "/usr/local/bin/ollama",
        "/usr/bin/ollama",
        "/opt/homebrew/bin/ollama",
    ];
    KNOWN.iter().map(PathBuf::from).find(|p| p.exists())
}

/// Whether the `ollama` CLI is available on this machine.
pub fn is_installed() -> bool {
    find_binary().is_some()
}

// ─── Server management ─────────────────────────────────────────

/// Thin REST probe of the Ollama API at `base_url`.
async fn server_ready(base_url: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    matches!(
        client.get(format!("{}/api/tags", base_url)).send().await,
        Ok(r) if r.status().is_success()
    )
}

/// Poll the server until it responds, or until `max_secs` elapse.
pub async fn wait_until_ready(base_url: &str, max_secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(max_secs);
    loop {
        if server_ready(base_url).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Ensure the Ollama server is up.
///
/// No-op when a server is already reachable. Otherwise it launches `ollama
/// serve` detached (own process group) so it keeps running in the background.
/// Returns `true` when it had to start the server.
pub async fn ensure_server_running() -> Result<bool> {
    if server_ready(DEFAULT_BASE_URL).await {
        return Ok(false);
    }
    let bin = find_binary().context("Ollama is not installed")?;

    let mut cmd = std::process::Command::new(bin);
    cmd.arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Put the server into its own process group so closing the app does not
    // send it SIGHUP; it keeps serving in the background afterwards.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn().context("Failed to launch `ollama serve`")?;
    Ok(true)
}

/// Stop the local Ollama server (best effort).
pub async fn stop_server() -> Result<()> {
    if !is_installed() {
        bail!("Ollama is not installed");
    }
    #[cfg(unix)]
    {
        // Kill the server process(es). Pull/run helpers are short-lived CLI
        // processes, so targeting `ollama serve` keeps downloads safe.
        let _ = std::process::Command::new("pkill")
            .args(["-f", "ollama serve"])
            .status();
    }
    Ok(())
}

// ─── Model management ──────────────────────────────────────────

/// A model installed locally on the machine.
#[derive(Debug, Clone)]
pub struct InstalledModel {
    pub name: String,
    pub size_human: String,
    pub parameter_size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
struct TagModel {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    details: TagDetails,
}

#[derive(Debug, Deserialize, Default)]
struct TagDetails {
    #[serde(default)]
    parameter_size: Option<String>,
}

/// List the models stored on this machine (requires the server to be up).
pub async fn list_installed_models() -> Result<Vec<InstalledModel>> {
    let url = format!("{}/api/tags", DEFAULT_BASE_URL);
    let resp = reqwest::get(&url)
        .await
        .context("Cannot reach the Ollama server")?;
    if !resp.status().is_success() {
        bail!("Server returned HTTP {}", resp.status());
    }
    let data: TagsResponse = resp.json().await?;
    let mut models: Vec<InstalledModel> = data
        .models
        .into_iter()
        .map(|m| InstalledModel {
            name: m.name,
            size_human: human_size(m.size),
            parameter_size: m.details.parameter_size,
        })
        .collect();
    models.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(models)
}

/// Format a byte count in a human-friendly way.
pub fn human_size(bytes: u64) -> String {
    if bytes == 0 {
        return "unknown".to_string();
    }
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", value, UNITS[unit])
}

/// Curated list of popular models offered in the downloader.
pub const CATALOG: &[&str] = &[
    "llama3.2:3b",
    "llama3.2:1b",
    "llama3.1:8b",
    "qwen2.5:7b",
    "qwen2.5:3b",
    "gemma2:9b",
    "gemma2:2b",
    "mistral:7b",
    "phi3:mini",
    "deepseek-r1:7b",
];

// ─── Streaming CLI operations ──────────────────────────────────

/// An event emitted while a background CLI operation (install / pull) runs.
#[derive(Debug)]
pub enum CliEvent {
    /// A line of output (stdout or stderr).
    Output(String),
    /// The operation finished successfully.
    Success,
    /// The operation failed.
    Error(String),
}

/// Run a CLI program and stream its (combined) output line by line over an
/// mpsc channel. Used for install and pull, which can take a while and need
/// progress shown in the UI.
pub async fn run_cli(
    program: &str,
    args: &[&str],
    tx: tokio::sync::mpsc::Sender<CliEvent>,
) {
    let mut child = match TCommand::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx
                .send(CliEvent::Error(format!("Failed to run {}: {}", program, e)))
                .await;
            return;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Stream both pipes concurrently so output stays ordered by arrival.
    let mut readers = Vec::new();
    if let Some(out) = stdout {
        let tx_c = tx.clone();
        readers.push(tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    let _ = tx_c.send(CliEvent::Output(line)).await;
                }
            }
        }));
    }
    if let Some(err) = stderr {
        let tx_c = tx.clone();
        readers.push(tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    let _ = tx_c.send(CliEvent::Output(line)).await;
                }
            }
        }));
    }

    let status = child.wait().await;
    for handle in readers {
        let _ = handle.await;
    }

    match status {
        Ok(s) if s.success() => {
            let _ = tx.send(CliEvent::Success).await;
        }
        Ok(s) => {
            let code = s.code().unwrap_or(-1);
            let _ = tx
                .send(CliEvent::Error(format!("Command exited with code {}", code)))
                .await;
        }
        Err(e) => {
            let _ = tx.send(CliEvent::Error(format!("Command failed: {}", e))).await;
        }
    }
}

/// Install Ollama, streaming progress to `tx`.
pub async fn install_ollama(tx: tokio::sync::mpsc::Sender<CliEvent>) {
    let cmd = install_command();
    let args: Vec<&str> = cmd.iter().skip(1).map(String::as_str).collect();
    run_cli(&cmd[0], &args, tx).await;
}

/// Pull (download) a model by tag, streaming progress to `tx`.
pub async fn pull_model(model: &str, tx: tokio::sync::mpsc::Sender<CliEvent>) {
    let Some(bin) = find_binary() else {
        let _ = tx
            .send(CliEvent::Error("Ollama is not installed".to_string()))
            .await;
        return;
    };
    if let Err(e) = ensure_server_running().await {
        let _ = tx
            .send(CliEvent::Error(format!("Could not start the Ollama server: {}", e)))
            .await;
        return;
    }
    if !wait_until_ready(DEFAULT_BASE_URL, 30).await {
        let _ = tx
            .send(CliEvent::Error("The Ollama server did not become ready".to_string()))
            .await;
        return;
    }
    let bin_str = bin.to_string_lossy().to_string();
    run_cli(&bin_str, &["pull", model], tx).await;
}

/// Snapshot of the local Ollama environment used by the startup flow.
pub struct OllamaStatus {
    pub installed: bool,
    pub server_running: bool,
}

/// Detect whether Ollama is installed and whether its server is running.
pub async fn status() -> OllamaStatus {
    let installed = is_installed();
    let server_running = installed && server_ready(DEFAULT_BASE_URL).await;
    OllamaStatus {
        installed,
        server_running,
    }
}
