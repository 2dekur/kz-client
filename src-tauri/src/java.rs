//! Runtimes Java officiels Mojang (Java 8 / 17 / 21+ selon `javaVersion.component`).

use std::{collections::HashMap, path::PathBuf};

use serde::Deserialize;

use crate::{
    download::{sha1_hex, Task},
    error::{KzError, KzResult},
    paths::{join_rel, safe_component, Paths},
    versions::get_json,
};

const RUNTIME_INDEX: &str =
    "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

#[derive(Deserialize)]
struct RuntimeEntry {
    manifest: RemoteFile,
}

#[derive(Deserialize)]
struct RemoteFile {
    sha1: String,
    url: String,
}

#[derive(Deserialize)]
struct RuntimeManifest {
    files: HashMap<String, RuntimeFile>,
}

#[derive(Deserialize)]
struct RuntimeFile {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    executable: bool,
    #[serde(default)]
    downloads: Option<RuntimeDownloads>,
    #[serde(default)]
    target: Option<String>,
}

#[derive(Deserialize)]
struct RuntimeDownloads {
    raw: RawDownload,
}

#[derive(Deserialize)]
struct RawDownload {
    sha1: String,
    size: u64,
    url: String,
}

pub struct Runtime {
    pub java: PathBuf,
    pub tasks: Vec<Task>,
    #[cfg_attr(not(unix), allow(dead_code))]
    links: Vec<(PathBuf, String)>,
}

fn platform() -> KzResult<&'static str> {
    use std::env::consts::{ARCH, OS};
    Ok(match (OS, ARCH) {
        ("windows", "x86_64") => "windows-x64",
        ("windows", "x86") => "windows-x86",
        ("windows", "aarch64") => "windows-arm64",
        ("macos", "aarch64") => "mac-os-arm64",
        ("macos", _) => "mac-os",
        ("linux", "x86_64") => "linux",
        ("linux", "x86") => "linux-i386",
        (os, arch) => return Err(KzError::Unsupported(format!("Plateforme non supportée : {os}/{arch}"))),
    })
}

fn java_binary(base: &std::path::Path) -> PathBuf {
    if cfg!(windows) {
        base.join("bin").join("java.exe")
    } else if cfg!(target_os = "macos") {
        base.join("jre.bundle").join("Contents").join("Home").join("bin").join("java")
    } else {
        base.join("bin").join("java")
    }
}

/// Prépare la liste des fichiers du runtime. Hors ligne, réutilise un runtime déjà installé.
pub async fn prepare(http: &reqwest::Client, paths: &Paths, component: &str) -> KzResult<Runtime> {
    let component = safe_component(component)?;
    let base = paths.runtimes().join(component);
    let java = java_binary(&base);
    let manifest = match fetch_manifest(http, component).await {
        Ok(m) => m,
        Err(e) => {
            if tokio::fs::try_exists(&java).await.unwrap_or(false) {
                return Ok(Runtime { java, tasks: Vec::new(), links: Vec::new() });
            }
            return Err(e);
        }
    };
    let mut tasks = Vec::new();
    let mut links = Vec::new();
    for (rel, file) in manifest.files {
        let path = join_rel(&base, &rel)?;
        match (file.kind.as_str(), file.downloads, file.target) {
            ("file", Some(d), _) => {
                let mut t = Task::new(d.raw.url, path, Some(d.raw.sha1), Some(d.raw.size));
                t.executable = file.executable;
                tasks.push(t);
            }
            ("link", _, Some(target)) => links.push((path, target)),
            _ => {}
        }
    }
    Ok(Runtime { java, tasks, links })
}

async fn fetch_manifest(http: &reqwest::Client, component: &str) -> KzResult<RuntimeManifest> {
    let index: HashMap<String, HashMap<String, Vec<RuntimeEntry>>> = get_json(http, RUNTIME_INDEX).await?;
    let platform = platform()?;
    let lookup = |p: &str| index.get(p).and_then(|m| m.get(component)).and_then(|v| v.first());
    // Pas de Java 8 natif Apple Silicon : on prend la build x64 (Rosetta).
    let entry = lookup(platform)
        .or_else(|| if platform == "mac-os-arm64" { lookup("mac-os") } else { None })
        .ok_or_else(|| KzError::Unsupported(format!("Java « {component} » indisponible pour {platform}")))?;
    let bytes = http.get(&entry.manifest.url).send().await?.error_for_status()?.bytes().await?;
    let actual = sha1_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&entry.manifest.sha1) {
        return Err(KzError::Integrity {
            path: format!("runtime {component}"),
            expected: entry.manifest.sha1.clone(),
            actual,
        });
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Crée les liens symboliques du runtime (macOS/Linux uniquement).
pub async fn link(runtime: &Runtime) -> KzResult<()> {
    #[cfg(unix)]
    {
        for (path, target) in &runtime.links {
            if tokio::fs::symlink_metadata(path).await.is_ok() {
                continue;
            }
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::symlink(target, path).await?;
        }
    }
    #[cfg(not(unix))]
    let _ = runtime;
    Ok(())
}
