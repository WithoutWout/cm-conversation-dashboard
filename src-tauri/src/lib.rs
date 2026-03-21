use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

// ── Return types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct DataFiles {
    articles: Option<String>,
    dialogs: Option<String>,
}

#[derive(Serialize)]
struct AppData {
    articles: serde_json::Value,
    dialogs: serde_json::Value,
    #[serde(rename = "tDialogs")]
    t_dialogs: serde_json::Value,
    files: DataFiles,
}

#[derive(Serialize)]
struct ImportResult {
    ok: bool,
    reason: Option<String>,
    filename: Option<String>,
}

#[derive(Serialize)]
struct UpdateResult {
    status: String,
    version: Option<String>,
    message: Option<String>,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn app_data_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_default()
}

fn find_file(dirs: &[PathBuf], pattern: &str) -> Option<PathBuf> {
    for dir in dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.contains(pattern) && name_str.ends_with(".json") {
                    return Some(entry.path());
                }
            }
        }
    }
    None
}

fn search_dirs(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut dirs = vec![app_data_dir(app)];
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    dirs
}

// ── Commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
fn get_data(app: tauri::AppHandle) -> AppData {
    let dirs = search_dirs(&app);

    let mut articles = serde_json::Value::Array(vec![]);
    let mut dialogs = serde_json::Value::Array(vec![]);
    let mut t_dialogs = serde_json::Value::Array(vec![]);
    let mut files = DataFiles {
        articles: None,
        dialogs: None,
    };

    if let Some(path) = find_file(&dirs, "ArticlesExport") {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                articles = json
                    .get("Articles")
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(vec![]));
                files.articles = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
            }
        }
    }

    if let Some(path) = find_file(&dirs, "DialogsExport") {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                dialogs = json
                    .pointer("/dialogs/result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(vec![]));
                t_dialogs = match json.get("tDialogs") {
                    Some(serde_json::Value::Array(arr)) => {
                        serde_json::Value::Array(arr.clone())
                    }
                    Some(obj) => obj
                        .pointer("/result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Array(vec![])),
                    None => serde_json::Value::Array(vec![]),
                };
                files.dialogs = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
            }
        }
    }

    AppData {
        articles,
        dialogs,
        t_dialogs,
        files,
    }
}

#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) {
    use tauri_plugin_opener::OpenerExt;
    if url.starts_with("https://") || url.starts_with("http://") {
        let _ = app.opener().open_url(url, None::<String>);
    }
}

#[tauri::command]
async fn import_file(app: tauri::AppHandle, file_type: String) -> ImportResult {
    if file_type != "ArticlesExport" && file_type != "DialogsExport" {
        return ImportResult {
            ok: false,
            reason: Some("Invalid type".into()),
            filename: None,
        };
    }

    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();

    app.dialog()
        .file()
        .add_filter("JSON Files", &["json"])
        .pick_file(move |path| {
            let p: Option<PathBuf> = path.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(p);
        });

    let src: PathBuf = match rx.await.ok().flatten() {
        Some(p) => p,
        None => {
            return ImportResult {
                ok: false,
                reason: Some("canceled".into()),
                filename: None,
            }
        }
    };

    let basename = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let dest_basename = if basename.contains(&*file_type) {
        basename
    } else {
        format!("{}_{}", file_type, basename)
    };

    let dest_dir = app_data_dir(&app);
    let _ = fs::create_dir_all(&dest_dir);

    // Remove old file with different name
    if let Some(old_path) = find_file(&search_dirs(&app), &file_type) {
        if old_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            != Some(dest_basename.clone())
        {
            let _ = fs::remove_file(old_path);
        }
    }

    match fs::copy(&src, dest_dir.join(&dest_basename)) {
        Ok(_) => ImportResult {
            ok: true,
            reason: None,
            filename: Some(dest_basename),
        },
        Err(e) => ImportResult {
            ok: false,
            reason: Some(e.to_string()),
            filename: None,
        },
    }
}

#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> UpdateResult {
    let current = app.package_info().version.to_string();

    let client = match reqwest::Client::builder()
        .user_agent("cm-conversation-dashboard")
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return UpdateResult {
                status: "error".into(),
                version: None,
                message: Some(format!("Client error: {}", e)),
            }
        }
    };

    let resp = match client
        .get("https://api.github.com/repos/WithoutWout/cm-conversation-dashboard/releases/latest")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return UpdateResult {
                status: "error".into(),
                version: None,
                message: Some(format!("Network: {}", e)),
            }
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            return UpdateResult {
                status: "error".into(),
                version: None,
                message: Some(format!("Parse error: {}", e)),
            }
        }
    };

    let latest = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches('v').to_string())
        .unwrap_or_default();

    if latest.is_empty() {
        let msg = json
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("no tag_name");
        return UpdateResult {
            status: "error".into(),
            version: None,
            message: Some(format!("API: {}", msg)),
        };
    }

    if latest == current {
        UpdateResult {
            status: "up-to-date".into(),
            version: None,
            message: None,
        }
    } else {
        UpdateResult {
            status: "available".into(),
            version: Some(latest),
            message: None,
        }
    }
}

#[tauri::command]
fn get_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_data,
            open_url,
            import_file,
            check_for_updates,
            get_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application")
}
