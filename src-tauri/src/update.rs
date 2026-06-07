use serde::Serialize;
use tauri_plugin_updater::UpdaterExt;

#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub is_deb: bool,
}

#[cfg(target_os = "linux")]
pub(crate) fn is_deb_install() -> bool {
    std::env::var("APPIMAGE").is_err()
}

// ── platform-specific helpers ────────────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
async fn do_check(app: &tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    Ok(update.map(|u| UpdateInfo {
        version: u.version,
        is_deb: false,
    }))
}

#[cfg(target_os = "linux")]
async fn do_check(app: &tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let is_deb = is_deb_install();
    let update = if is_deb {
        app.updater_builder()
            .target("linux-x86_64-deb".to_string())
            .build()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?
    } else {
        app.updater()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?
    };
    Ok(update.map(|u| UpdateInfo {
        version: u.version,
        is_deb,
    }))
}

#[cfg(not(target_os = "linux"))]
async fn do_install(app: &tauri::AppHandle, _is_deb: bool) -> Result<(), String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update available".to_string())?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart()
}

#[cfg(target_os = "linux")]
async fn do_install(app: &tauri::AppHandle, is_deb: bool) -> Result<(), String> {
    if is_deb {
        let update = app
            .updater_builder()
            .target("linux-x86_64-deb".to_string())
            .build()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "No update available".to_string())?;

        let bytes = update
            .download(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())?;

        let tmp = std::env::temp_dir().join("fh6-tel-update.deb");
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;

        let status = tokio::process::Command::new("pkexec")
            .args(["dpkg", "-i", &tmp.to_string_lossy()])
            .status()
            .await
            .map_err(|e| e.to_string())?;

        let _ = std::fs::remove_file(&tmp);

        if !status.success() {
            return Err(format!(
                "dpkg failed (exit {})",
                status.code().unwrap_or(-1)
            ));
        }

        app.restart()
    } else {
        let update = app
            .updater()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "No update available".to_string())?;
        update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
        app.restart()
    }
}

// ── public commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    do_check(&app).await
}

#[tauri::command]
pub async fn install_update(app: tauri::AppHandle, is_deb: bool) -> Result<(), String> {
    do_install(&app, is_deb).await
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
#[cfg(test)]
mod tests {
    use super::is_deb_install;

    #[test]
    fn no_appimage_env_means_deb() {
        // SAFETY: single-threaded test only — run with --test-threads=1
        unsafe { std::env::remove_var("APPIMAGE") };
        assert!(is_deb_install());
    }

    #[test]
    fn appimage_env_present_means_not_deb() {
        unsafe { std::env::set_var("APPIMAGE", "/tmp/fake.AppImage") };
        assert!(!is_deb_install());
        unsafe { std::env::remove_var("APPIMAGE") };
    }
}
