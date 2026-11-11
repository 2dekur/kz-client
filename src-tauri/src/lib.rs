mod auth;
mod discord;
mod download;
mod error;
mod forge;
mod java;
mod kzengine;
mod launcher;
mod paths;
mod versions;

use std::{sync::atomic::AtomicBool, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use error::{KzError, KzResult};

pub struct AppState {
    pub http: reqwest::Client,
    pub paths: paths::Paths,
    pub auth: auth::AuthManager,
    pub discord: discord::Discord,
    pub running: AtomicBool,
    manifest: tokio::sync::OnceCell<versions::Manifest>,
}

impl AppState {
    /// Manifeste Mojang, récupéré une fois par session (les erreurs ne sont pas mises en cache).
    pub async fn manifest(&self) -> KzResult<&versions::Manifest> {
        self.manifest
            .get_or_try_init(|| versions::fetch_manifest(&self.http, &self.paths))
            .await
    }
}

/// Variable d'environnement à l'exécution, sinon valeur injectée à la compilation.
pub fn env_or_build(name: &str, built: Option<&'static str>) -> Option<String> {
    std::env::var(name)
        .ok()
        .or_else(|| built.map(String::from))
        .filter(|s| !s.trim().is_empty())
}

pub fn total_memory_mb() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory() / 1024 / 1024
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    total_mb: u64,
    recommended_mb: u64,
    max_mb: u64,
}

#[derive(Serialize, Clone)]
struct AuthWarning {
    uuid: String,
    message: String,
}

#[tauri::command]
fn system_info() -> SystemInfo {
    let total_mb = total_memory_mb();
    let max_mb = total_mb.saturating_sub(1024).max(1024);
    let recommended = match total_mb {
        0..=8192 => (total_mb / 2).clamp(2048, 4096),
        8193..=16384 => 6144,
        _ => 8192,
    };
    SystemInfo {
        total_mb,
        recommended_mb: recommended.min(max_mb),
        max_mb,
    }
}

#[tauri::command]
async fn list_versions(state: State<'_, AppState>) -> KzResult<Vec<versions::ManifestEntry>> {
    Ok(state.manifest().await?.versions.clone())
}

#[tauri::command]
async fn auth_login(state: State<'_, AppState>) -> KzResult<auth::Account> {
    state.auth.login().await
}

#[tauri::command]
async fn auth_accounts(state: State<'_, AppState>) -> KzResult<auth::AccountsView> {
    Ok(state.auth.view().await)
}

#[tauri::command]
async fn auth_switch(app: AppHandle, state: State<'_, AppState>, uuid: String) -> KzResult<auth::Account> {
    let account = state.auth.switch(&uuid).await?;
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let Err(e) = state.auth.ensure(&uuid, 300).await {
            let _ = app.emit("kz://auth-warning", AuthWarning { uuid, message: e.to_string() });
        }
    });
    Ok(account)
}

#[tauri::command]
async fn auth_remove(state: State<'_, AppState>, uuid: String) -> KzResult<auth::AccountsView> {
    state.auth.remove(&uuid).await
}

/// Proxy de skin (textures.minecraft.net uniquement) → data URL, pour éviter CORS côté WebView.
#[tauri::command]
async fn fetch_skin(state: State<'_, AppState>, url: String) -> KzResult<String> {
    let parsed = url::Url::parse(&url).map_err(|e| KzError::Unsupported(format!("URL de skin invalide : {e}")))?;
    if parsed.host_str() != Some("textures.minecraft.net") {
        return Err(KzError::Unsupported("Hôte de skin non autorisé".into()));
    }
    let https = format!("https://textures.minecraft.net{}", parsed.path());
    let bytes = state.http.get(&https).send().await?.error_for_status()?.bytes().await?;
    if bytes.len() > 512 * 1024 {
        return Err(KzError::Unsupported("Skin trop volumineux".into()));
    }
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(&bytes)))
}

#[tauri::command]
async fn launch(app: AppHandle, opts: launcher::LaunchOptions) -> KzResult<()> {
    launcher::launch(app, opts).await
}

/// Rafraîchit silencieusement tous les comptes (au démarrage puis toutes les 15 min).
fn spawn_background_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            let state = app.state::<AppState>();
            for uuid in state.auth.account_ids().await {
                if let Err(e) = state.auth.ensure(&uuid, 3600).await {
                    let _ = app.emit("kz://auth-warning", AuthWarning { uuid, message: e.to_string() });
                }
            }
            tokio::time::sleep(Duration::from_secs(15 * 60)).await;
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .setup(|app| {
            let root = app.path().app_data_dir()?;
            std::fs::create_dir_all(&root)?;
            if let Ok(id) = std::fs::read_to_string(root.join("azure-client-id.txt")) {
                let _ = auth::FILE_CLIENT_ID.set(id.trim().to_string());
            }
            let paths = paths::Paths::new(root);
            let http = reqwest::Client::builder()
                .user_agent(concat!("KZClient/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(600))
                .pool_max_idle_per_host(64)
                .build()?;
            let auth = auth::AuthManager::load(paths.clone(), http.clone());
            let discord = discord::Discord::start(env_or_build("KZ_DISCORD_APP_ID", option_env!("KZ_DISCORD_APP_ID")));
            app.manage(AppState {
                http,
                paths,
                auth,
                discord,
                running: AtomicBool::new(false),
                manifest: tokio::sync::OnceCell::new(),
            });
            spawn_background_refresh(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            system_info,
            list_versions,
            auth_login,
            auth_accounts,
            auth_switch,
            auth_remove,
            fetch_skin,
            launch
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("KZ Client n'a pas pu démarrer : {e}");
        std::process::exit(1);
    }
}
