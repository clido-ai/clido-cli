use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use crate::tui::events::AgentEvent;

const GITHUB_REPO: &str = "clido-ai/clido-cli";
/// Only hit the GitHub API once per 24 hours.
const CHECK_INTERVAL_SECS: u64 = 86400;
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Startup background check ───────────────────────────────────────────────────

/// Spawn a background task that checks for a newer release at most once per 24h.
/// If a newer version is found, sends `AgentEvent::UpdateAvailable { version }`.
/// Set `force` to true to bypass the rate limit (for `/update` command).
pub(crate) fn spawn_update_check(tx: mpsc::Sender<AgentEvent>, force: bool) {
    tokio::spawn(async move {
        if !force && !should_check() {
            return;
        }
        let Some(latest) = fetch_latest_version().await else {
            return;
        };
        // Only record a successful check after we actually got a version from the API.
        mark_checked();
        if remote_is_newer(&latest, CURRENT_VERSION) {
            let _ = tx
                .send(AgentEvent::UpdateAvailable { version: latest })
                .await;
        }
    });
}

// ── /update command ────────────────────────────────────────────────────────────

/// Spawn a task for the `/update` slash command.
/// Checks for the latest version, then downloads and replaces the running binary.
/// Progress is reported via `AgentEvent::UpdateStatus` messages.
pub(crate) fn spawn_do_update(known_version: Option<String>, tx: mpsc::Sender<AgentEvent>) {
    tokio::spawn(async move {
        // Resolve latest version: use what we already know, or fetch fresh.
        let version = if let Some(v) = known_version {
            v
        } else {
            let Some(v) = fetch_latest_version().await else {
                let _ = tx
                    .send(AgentEvent::UpdateStatus(
                        "✗ Could not reach github.com — check your connection.".into(),
                    ))
                    .await;
                return;
            };
            if !remote_is_newer(&v, CURRENT_VERSION) {
                let _ = tx
                    .send(AgentEvent::UpdateStatus(format!(
                        "✓ Already on the latest version ({}).",
                        CURRENT_VERSION
                    )))
                    .await;
                return;
            }
            v
        };

        let Some(artifact) = platform_artifact() else {
            let _ = tx
                .send(AgentEvent::UpdateStatus(
                    "✗ Unsupported platform — self-update not available here.".into(),
                ))
                .await;
            return;
        };

        let url = format!(
            "https://github.com/{}/releases/download/{}/{}",
            GITHUB_REPO, version, artifact
        );
        let _ = tx
            .send(AgentEvent::UpdateStatus(format!(
                "↓ Downloading {} ...",
                version
            )))
            .await;

        match download_and_replace(&url).await {
            Ok(()) => {
                let _ = tx
                    .send(AgentEvent::UpdateStatus(format!(
                        "✓ Updated to {}. Restart clido to apply.",
                        version
                    )))
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(AgentEvent::UpdateStatus(format!("✗ Update failed: {}", e)))
                    .await;
            }
        }
    });
}

// ── Internals ──────────────────────────────────────────────────────────────────

/// True if `latest_tag` (e.g. `v0.2.0`) is a semver **greater** than `current_pkg_version`
/// (e.g. `0.1.0-beta.8` from `CARGO_PKG_VERSION`).
fn remote_is_newer(latest_tag: &str, current_pkg_version: &str) -> bool {
    let l = latest_tag.trim().trim_start_matches('v');
    let c = current_pkg_version.trim().trim_start_matches('v');
    match (semver::Version::parse(l), semver::Version::parse(c)) {
        (Ok(lv), Ok(cv)) => lv > cv,
        _ => {
            // If tags are non-semver, fall back to inequality (legacy behavior).
            l != c
        }
    }
}

pub(crate) async fn fetch_latest_version() -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(format!("clido/{}", CURRENT_VERSION))
        .build()
        .ok()?;

    // Use /releases (not /releases/latest) — the "latest" endpoint ignores
    // prereleases and has aggressive caching that lags behind new releases.
    let url = format!(
        "https://api.github.com/repos/{}/releases?per_page=1",
        GITHUB_REPO
    );
    let releases: Vec<Release> = client.get(&url).send().await.ok()?.json().await.ok()?;
    releases.into_iter().next().map(|r| r.tag_name)
}

async fn download_and_replace(url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(format!("clido/{}", CURRENT_VERSION))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("read failed: {e}"))?;

    let current_exe = std::env::current_exe().map_err(|e| format!("can't locate binary: {e}"))?;
    // Write to a sibling temp file, then rename (atomic on same filesystem).
    let tmp = current_exe.with_extension("new");

    std::fs::write(&tmp, &bytes).map_err(|e| format!("write failed: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod failed: {e}"))?;
    }

    std::fs::rename(&tmp, &current_exe)
        .map_err(|e| format!("replace failed (permission issue?): {e}"))?;

    Ok(())
}

fn should_check() -> bool {
    let Some(path) = timestamp_path() else {
        return true;
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return true;
    };
    let Ok(ts) = content.trim().parse::<u64>() else {
        return true;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.saturating_sub(ts) >= CHECK_INTERVAL_SECS
}

fn mark_checked() {
    let Some(path) = timestamp_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = std::fs::write(path, now.to_string());
}

fn timestamp_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("", "", "clido")
        .map(|d| d.data_local_dir().join("last_update_check"))
}

fn platform_artifact() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("clido-linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("clido-linux-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("clido-macos-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("clido-macos-x86_64")
    } else {
        None
    }
}

pub async fn run_update() -> Result<(), anyhow::Error> {
    println!("Checking for updates...");
    
    let Some(latest) = fetch_latest_version().await else {
        return Err(anyhow::anyhow!("Could not reach github.com — check your connection."));
    };
    
    if !remote_is_newer(&latest, CURRENT_VERSION) {
        println!("✓ Already on the latest version ({}).", CURRENT_VERSION);
        return Ok(());
    }
    
    println!("New version available: {}", latest);
    println!("Current version: {}", CURRENT_VERSION);
    
    let Some(artifact) = platform_artifact() else {
        return Err(anyhow::anyhow!("Unsupported platform — self-update not available here."));
    };
    
    let url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        GITHUB_REPO, latest, artifact
    );
    
    println!("Downloading {} ...", latest);
    
    match download_and_replace(&url).await {
        Ok(()) => {
            println!("✓ Updated to {}. Restart clido to apply.", latest);
        }
        Err(e) => {
            return Err(anyhow::anyhow!("Update failed: {}", e));
        }
    }
    
    Ok(())
}
