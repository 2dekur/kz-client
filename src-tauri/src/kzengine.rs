//! KZ Performance Engine : injection automatique de mods d'optimisation via l'API Modrinth.
//! Les fichiers injectés sont listés dans `mods/.kz-engine.json` pour pouvoir être retirés proprement ;
//! les mods déjà ajoutés par le joueur (détectés par hash) ne sont jamais dupliqués.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::AppHandle;

use crate::{
    download::{self, Task},
    error::{KzError, KzResult},
    launcher::log,
    paths::write_atomic,
    versions::Loader,
};

const MODRINTH: &str = "https://api.modrinth.com/v2";
const MANIFEST_NAME: &str = ".kz-engine.json";

#[derive(Debug, Clone, Deserialize)]
struct MrVersion {
    project_id: String,
    #[serde(default)]
    version_type: String,
    files: Vec<MrFile>,
    #[serde(default)]
    dependencies: Vec<MrDep>,
}

#[derive(Debug, Clone, Deserialize)]
struct MrFile {
    url: String,
    filename: String,
    #[serde(default)]
    primary: bool,
    size: u64,
    hashes: MrHashes,
}

#[derive(Debug, Clone, Deserialize)]
struct MrHashes {
    sha1: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MrDep {
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    dependency_type: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EngineManifest {
    files: Vec<String>,
}

/// Groupes d'alternatives : le premier projet disponible pour la version gagne.
fn groups(loader: Loader) -> &'static [&'static [&'static str]] {
    match loader {
        Loader::Fabric | Loader::Quilt => &[&["sodium"], &["iris"], &["lithium"], &["ferrite-core"], &["modmenu"]],
        Loader::Forge => &[&["embeddium", "rubidium"], &["ferrite-core"]],
        Loader::Neoforge => &[&["sodium", "embeddium"], &["iris"], &["lithium"], &["ferrite-core"]],
        Loader::Vanilla => &[],
    }
}

fn modrinth_loaders(loader: Loader) -> Vec<&'static str> {
    match loader {
        Loader::Quilt => vec!["quilt", "fabric"],
        other => vec![other.slug()],
    }
}

fn safe_filename(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', ':']) && name != ".." && name.ends_with(".jar")
}

async fn latest(http: &reqwest::Client, project: &str, loaders: &[&str], mc: &str) -> KzResult<Option<MrVersion>> {
    let resp = http
        .get(format!("{MODRINTH}/project/{project}/version"))
        .query(&[
            ("loaders", serde_json::to_string(loaders)?),
            ("game_versions", serde_json::to_string(&[mc])?),
        ])
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let list: Vec<MrVersion> = resp.error_for_status()?.json().await?;
    Ok(list.iter().find(|v| v.version_type == "release").or(list.first()).cloned())
}

async fn version_by_id(http: &reqwest::Client, id: &str) -> KzResult<Option<MrVersion>> {
    let resp = http.get(format!("{MODRINTH}/version/{id}")).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    Ok(Some(resp.error_for_status()?.json().await?))
}

/// Projets déjà présents dans le dossier mods (ajoutés par le joueur), identifiés par SHA-1.
async fn user_projects(http: &reqwest::Client, mods_dir: &Path, previous: &EngineManifest) -> KzResult<HashSet<String>> {
    let mut hashes = Vec::new();
    let mut dir = tokio::fs::read_dir(mods_dir).await?;
    while let Some(entry) = dir.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".jar") || previous.files.contains(&name) {
            continue;
        }
        hashes.push(download::sha1_file(entry.path()).await?);
    }
    if hashes.is_empty() {
        return Ok(HashSet::new());
    }
    let resp = http
        .post(format!("{MODRINTH}/version_files"))
        .json(&json!({ "hashes": hashes, "algorithm": "sha1" }))
        .send()
        .await?
        .error_for_status()?;
    let map: HashMap<String, MrVersion> = resp.json().await?;
    Ok(map.into_values().map(|v| v.project_id).collect())
}

/// Résout un mod et ses dépendances obligatoires. `None` si une dépendance est introuvable
/// (le mod est alors ignoré plutôt que de faire crasher le jeu).
async fn with_dependencies(
    http: &reqwest::Client,
    root: MrVersion,
    loaders: &[&str],
    mc: &str,
    taken: &HashMap<String, MrVersion>,
    owned: &HashSet<String>,
) -> KzResult<Option<HashMap<String, MrVersion>>> {
    let mut bundle: HashMap<String, MrVersion> = HashMap::new();
    let mut stack = vec![root];
    while let Some(version) = stack.pop() {
        let pid = version.project_id.clone();
        if taken.contains_key(&pid) || bundle.contains_key(&pid) || owned.contains(&pid) {
            continue;
        }
        for dep in version.dependencies.iter().filter(|d| d.dependency_type == "required") {
            if let Some(dpid) = &dep.project_id {
                if taken.contains_key(dpid) || bundle.contains_key(dpid) || owned.contains(dpid) {
                    continue;
                }
            }
            let resolved = match (&dep.version_id, &dep.project_id) {
                (Some(vid), _) => version_by_id(http, vid).await?,
                (None, Some(dpid)) => latest(http, dpid, loaders, mc).await?,
                (None, None) => continue,
            };
            match resolved {
                Some(v) => stack.push(v),
                None => return Ok(None),
            }
        }
        bundle.insert(pid, version);
        if bundle.len() > 32 {
            return Err(KzError::Unsupported("Arbre de dépendances KZ Engine anormalement grand".into()));
        }
    }
    Ok(Some(bundle))
}

/// Synchronise le dossier `mods` : injecte (enabled) ou retire (disabled) les mods KZ Engine.
pub async fn sync(
    http: &reqwest::Client,
    app: &AppHandle,
    mods_dir: &Path,
    loader: Loader,
    mc: &str,
    enabled: bool,
) -> KzResult<Vec<String>> {
    tokio::fs::create_dir_all(mods_dir).await?;
    let manifest_path = mods_dir.join(MANIFEST_NAME);
    let previous: EngineManifest = match tokio::fs::read(&manifest_path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => EngineManifest::default(),
    };

    let mut picked: HashMap<String, MrVersion> = HashMap::new();
    if enabled {
        let loaders = modrinth_loaders(loader);
        let owned = user_projects(http, mods_dir, &previous).await?;
        for group in groups(loader) {
            for slug in group.iter() {
                let Some(version) = latest(http, slug, &loaders, mc).await? else { continue };
                if owned.contains(&version.project_id) {
                    break;
                }
                if let Some(bundle) = with_dependencies(http, version, &loaders, mc, &picked, &owned).await? {
                    picked.extend(bundle);
                    break;
                }
            }
        }
    }

    let mut tasks = Vec::new();
    let mut files = Vec::new();
    for version in picked.values() {
        let Some(file) = version.files.iter().find(|f| f.primary).or(version.files.first()) else { continue };
        if !safe_filename(&file.filename) {
            continue;
        }
        files.push(file.filename.clone());
        tasks.push(Task::new(
            &file.url,
            mods_dir.join(&file.filename),
            Some(file.hashes.sha1.clone()),
            Some(file.size),
        ));
    }
    download::download_all(http, app, "KZ Engine", tasks, false).await?;

    for old in previous.files.iter().filter(|f| !files.contains(f) && safe_filename(f)) {
        match tokio::fs::remove_file(mods_dir.join(old)).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    write_atomic(&manifest_path, &serde_json::to_vec_pretty(&EngineManifest { files: files.clone() })?).await?;
    if enabled {
        log(app, "stdout", Some("INFO".into()), format!("[KZ Engine] {} mods actifs : {}", files.len(), files.join(", ")));
    }
    Ok(files)
}
