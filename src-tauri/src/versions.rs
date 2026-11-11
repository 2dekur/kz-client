//! Manifeste Mojang, modèle des profils de version, règles OS, fusion `inheritsFrom`,
//! installation Fabric / Quilt.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    download::{sha1_hex, Task},
    error::{KzError, KzResult},
    paths::{join_rel, safe_component, write_atomic, Paths},
};

pub const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const MOJANG_LIBRARIES: &str = "https://libraries.minecraft.net/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    Neoforge,
}

impl Loader {
    pub fn slug(self) -> &'static str {
        match self {
            Loader::Vanilla => "vanilla",
            Loader::Fabric => "fabric",
            Loader::Quilt => "quilt",
            Loader::Forge => "forge",
            Loader::Neoforge => "neoforge",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Loader::Vanilla => "Vanilla",
            Loader::Fabric => "Fabric",
            Loader::Quilt => "Quilt",
            Loader::Forge => "Forge",
            Loader::Neoforge => "NeoForge",
        }
    }
}

// ---------------------------------------------------------------- manifeste

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub versions: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub release_time: String,
    pub sha1: String,
}

/// Récupère le manifeste ; hors ligne, retombe sur la dernière copie en cache.
pub async fn fetch_manifest(http: &reqwest::Client, paths: &Paths) -> KzResult<Manifest> {
    let cache = paths.versions().join("version_manifest_v2.json");
    let fetched = async {
        let bytes = http.get(MANIFEST_URL).send().await?.error_for_status()?.bytes().await?;
        Ok::<_, KzError>(bytes)
    }
    .await;
    match fetched {
        Ok(bytes) => {
            let manifest: Manifest = serde_json::from_slice(&bytes)?;
            write_atomic(&cache, &bytes).await?;
            Ok(manifest)
        }
        Err(e) => match tokio::fs::read(&cache).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(_) => Err(e),
        },
    }
}

// ---------------------------------------------------------------- profil de version

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    #[serde(default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub jar: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub main_class: Option<String>,
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    #[serde(default)]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub downloads: Option<VersionDownloads>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub logging: Option<Logging>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Ruled { rules: Vec<Rule>, value: ArgValue },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ArgValue {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: String,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionDownloads {
    #[serde(default)]
    pub client: Option<DownloadInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadInfo {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub downloads: Option<LibDownloads>,
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub extract: Option<Extract>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibDownloads {
    #[serde(default)]
    pub artifact: Option<DownloadInfo>,
    #[serde(default)]
    pub classifiers: Option<HashMap<String, DownloadInfo>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
    pub major_version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Logging {
    #[serde(default)]
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingClient {
    pub argument: String,
    pub file: LoggingFile,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
    #[serde(rename = "virtual", default)]
    pub is_virtual: bool,
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

// ---------------------------------------------------------------- règles & bibliothèques

pub fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    }
}

fn rule_matches(rule: &Rule) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name {
            if name != os_name() {
                return false;
            }
        }
        if let Some(arch) = &os.arch {
            let ok = match arch.as_str() {
                "x86" => std::env::consts::ARCH == "x86",
                "x86_64" | "x64" => std::env::consts::ARCH == "x86_64",
                "arm64" | "aarch64" => std::env::consts::ARCH == "aarch64",
                _ => false,
            };
            if !ok {
                return false;
            }
        }
    }
    // Aucune « feature » (démo, résolution custom, quick play) n'est activée par le launcher.
    if let Some(features) = &rule.features {
        if features.values().any(|v| *v) {
            return false;
        }
    }
    true
}

pub fn rules_allow(rules: &[Rule]) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule) {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

/// `group:artifact:version[:classifier][@ext]` → chemin Maven relatif.
pub fn maven_path(coord: &str) -> KzResult<String> {
    let (coord, ext) = coord.split_once('@').unwrap_or((coord, "jar"));
    let parts: Vec<&str> = coord.split(':').collect();
    let bad = || KzError::Unsupported(format!("Coordonnée Maven invalide : {coord}"));
    if parts.len() < 3 || parts.len() > 4 {
        return Err(bad());
    }
    if parts
        .iter()
        .chain(std::iter::once(&ext))
        .any(|p| p.is_empty() || p.contains(['/', '\\']) || *p == "..")
    {
        return Err(bad());
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[2]);
    let classifier = parts.get(3).map(|c| format!("-{c}")).unwrap_or_default();
    Ok(format!(
        "{}/{artifact}/{version}/{artifact}-{version}{classifier}.{ext}",
        group.replace('.', "/")
    ))
}

pub fn library_path(libraries_dir: &Path, coord: &str) -> KzResult<PathBuf> {
    join_rel(libraries_dir, &maven_path(coord)?)
}

/// Clé de déduplication : group:artifact[:classifier] (sans la version).
fn lib_key(name: &str) -> String {
    let base = name.split('@').next().unwrap_or(name);
    let parts: Vec<&str> = base.split(':').collect();
    match parts.as_slice() {
        [g, a, _, c, ..] => format!("{g}:{a}:{c}"),
        [g, a, ..] => format!("{g}:{a}"),
        _ => base.to_string(),
    }
}

fn base_url(lib: &Library) -> String {
    let raw = lib
        .url
        .as_deref()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or(MOJANG_LIBRARIES);
    let fixed = raw
        .replace("http://files.minecraftforge.net/maven", "https://maven.minecraftforge.net")
        .replace("https://files.minecraftforge.net/maven", "https://maven.minecraftforge.net");
    if fixed.ends_with('/') {
        fixed
    } else {
        format!("{fixed}/")
    }
}

#[derive(Debug, Clone)]
pub struct LibEntry {
    pub path: PathBuf,
    /// `None` si le fichier est produit localement (processeurs Forge).
    pub task: Option<Task>,
    pub classpath: bool,
    /// `Some(exclusions)` pour les anciennes natives à extraire.
    pub extract: Option<Vec<String>>,
}

fn make_entry(
    libraries_dir: &Path,
    rel: &str,
    url: String,
    sha1: Option<String>,
    size: Option<u64>,
    classpath: bool,
    extract: Option<Vec<String>>,
) -> KzResult<LibEntry> {
    let path = join_rel(libraries_dir, rel)?;
    let task = (!url.trim().is_empty()).then(|| Task::new(url, path.clone(), sha1, size));
    Ok(LibEntry {
        path,
        task,
        classpath,
        extract,
    })
}

/// Filtre les bibliothèques par règles OS, résout les natives et déduplique (le premier gagne :
/// le profil enfant — loader — passe avant Minecraft).
pub fn library_entries(libs: &[Library], libraries_dir: &Path) -> KzResult<Vec<LibEntry>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let arch_bits = if std::env::consts::ARCH == "x86" { "32" } else { "64" };

    for lib in libs {
        if !rules_allow(lib.rules.as_deref().unwrap_or(&[])) {
            continue;
        }
        let key = lib_key(&lib.name);

        // Natives « ancien format » (< 1.19) : jar classifié à extraire.
        if let Some(template) = lib.natives.as_ref().and_then(|n| n.get(os_name())) {
            let classifier = template.replace("${arch}", arch_bits);
            if seen.insert(format!("{key}#{classifier}")) {
                let coord = format!("{}:{classifier}", lib.name.split('@').next().unwrap_or(&lib.name));
                let excludes = lib.extract.clone().unwrap_or_default().exclude;
                let info = lib
                    .downloads
                    .as_ref()
                    .and_then(|d| d.classifiers.as_ref())
                    .and_then(|c| c.get(&classifier));
                let entry = match info {
                    Some(i) => {
                        let rel = match &i.path {
                            Some(p) => p.clone(),
                            None => maven_path(&coord)?,
                        };
                        make_entry(libraries_dir, &rel, i.url.clone(), i.sha1.clone(), i.size, false, Some(excludes))?
                    }
                    None => {
                        let rel = maven_path(&coord)?;
                        let url = format!("{}{rel}", base_url(lib));
                        make_entry(libraries_dir, &rel, url, None, None, false, Some(excludes))?
                    }
                };
                out.push(entry);
            }
        }

        let artifact = lib.downloads.as_ref().and_then(|d| d.artifact.as_ref());
        if lib.natives.is_some() && artifact.is_none() {
            continue;
        }
        if !seen.insert(key) {
            continue;
        }
        let entry = match artifact {
            Some(a) => {
                let rel = match &a.path {
                    Some(p) => p.clone(),
                    None => maven_path(&lib.name)?,
                };
                make_entry(libraries_dir, &rel, a.url.clone(), a.sha1.clone(), a.size, true, None)?
            }
            None => {
                let rel = maven_path(&lib.name)?;
                let url = format!("{}{rel}", base_url(lib));
                make_entry(libraries_dir, &rel, url, lib.sha1.clone(), lib.size, true, None)?
            }
        };
        out.push(entry);
    }
    Ok(out)
}

// ---------------------------------------------------------------- fusion & résolution

/// Fusionne un profil enfant (loader) avec son parent.
pub fn merge(child: VersionJson, parent: VersionJson) -> VersionJson {
    let mut libraries = child.libraries;
    libraries.extend(parent.libraries);
    let arguments = match (parent.arguments, child.arguments) {
        (Some(mut p), Some(c)) => {
            p.game.extend(c.game);
            p.jvm.extend(c.jvm);
            Some(p)
        }
        (p, c) => c.or(p),
    };
    VersionJson {
        id: child.id,
        inherits_from: None,
        jar: child.jar.or(parent.jar).or(Some(parent.id)),
        kind: child.kind.or(parent.kind),
        main_class: child.main_class.or(parent.main_class),
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        arguments,
        asset_index: child.asset_index.or(parent.asset_index),
        downloads: child.downloads.or(parent.downloads),
        libraries,
        java_version: child.java_version.or(parent.java_version),
        logging: child.logging.or(parent.logging),
    }
}

async fn read_version(paths: &Paths, id: &str) -> KzResult<VersionJson> {
    let path = paths.version_json(safe_component(id)?);
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(KzError::Unsupported(format!("Profil de version {id} introuvable")))
        }
        Err(e) => Err(e.into()),
    }
}

/// Résout la chaîne `inheritsFrom` jusqu'au profil vanilla et fusionne.
pub async fn resolve(paths: &Paths, id: &str, vanilla: &VersionJson) -> KzResult<VersionJson> {
    if id == vanilla.id {
        return Ok(vanilla.clone());
    }
    let mut chain = vec![read_version(paths, id).await?];
    while let Some(parent_id) = chain.last().and_then(|v| v.inherits_from.clone()) {
        if chain.len() > 8 {
            return Err(KzError::Unsupported(format!("Chaîne d'héritage trop longue pour {id}")));
        }
        let parent = if parent_id == vanilla.id {
            vanilla.clone()
        } else {
            read_version(paths, &parent_id).await?
        };
        chain.push(parent);
    }
    let mut merged = chain
        .pop()
        .ok_or_else(|| KzError::Unsupported(format!("Profil {id} vide")))?;
    while let Some(child) = chain.pop() {
        merged = merge(child, merged);
    }
    Ok(merged)
}

/// Garantit le profil vanilla sur disque (SHA-1 contrôlé contre le manifeste).
pub async fn ensure_vanilla(
    http: &reqwest::Client,
    paths: &Paths,
    manifest: &Manifest,
    id: &str,
) -> KzResult<VersionJson> {
    let entry = manifest
        .versions
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| KzError::Unsupported(format!("Version Minecraft inconnue : {id}")))?;
    let path = paths.version_json(safe_component(id)?);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) if sha1_hex(&b).eq_ignore_ascii_case(&entry.sha1) => b,
        _ => {
            let b = http.get(&entry.url).send().await?.error_for_status()?.bytes().await?.to_vec();
            let actual = sha1_hex(&b);
            if !actual.eq_ignore_ascii_case(&entry.sha1) {
                return Err(KzError::Integrity {
                    path: path.display().to_string(),
                    expected: entry.sha1.clone(),
                    actual,
                });
            }
            write_atomic(&path, &b).await?;
            b
        }
    };
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn get_json<T: DeserializeOwned>(http: &reqwest::Client, url: &str) -> KzResult<T> {
    let resp = http.get(url).send().await?;
    if matches!(resp.status().as_u16(), 400 | 404) {
        return Err(KzError::Unsupported(format!("Ressource introuvable : {url}")));
    }
    Ok(resp.error_for_status()?.json::<T>().await?)
}

// ---------------------------------------------------------------- Fabric / Quilt

#[derive(Deserialize)]
struct LoaderEntry {
    loader: LoaderInfo,
}

#[derive(Deserialize)]
struct LoaderInfo {
    version: String,
    #[serde(default)]
    stable: Option<bool>,
}

/// Installe le profil Fabric/Quilt (méta API officielle) et renvoie l'id de version à lancer.
pub async fn install_fabric_like(
    http: &reqwest::Client,
    paths: &Paths,
    loader: Loader,
    mc: &str,
    requested: Option<&str>,
) -> KzResult<String> {
    let (meta, prefix) = match loader {
        Loader::Fabric => ("https://meta.fabricmc.net/v2", "fabric-loader"),
        Loader::Quilt => ("https://meta.quiltmc.org/v3", "quilt-loader"),
        other => {
            return Err(KzError::Unsupported(format!("{} n'est pas un loader Fabric-like", other.label())))
        }
    };
    let unavailable = || KzError::Unsupported(format!("{} n'est pas disponible pour Minecraft {mc}", loader.label()));
    let loader_version = match requested {
        Some(v) => safe_component(v)?.to_string(),
        None => {
            let list: Vec<LoaderEntry> = get_json(http, &format!("{meta}/versions/loader/{mc}"))
                .await
                .map_err(|e| match e {
                    KzError::Unsupported(_) => unavailable(),
                    other => other,
                })?;
            let pick = match loader {
                Loader::Fabric => list.iter().find(|e| e.loader.stable == Some(true)),
                _ => list
                    .iter()
                    .find(|e| !e.loader.version.contains("beta") && !e.loader.version.contains("pre")),
            }
            .or(list.first())
            .ok_or_else(unavailable)?;
            safe_component(&pick.loader.version)?.to_string()
        }
    };

    let expected_id = format!("{prefix}-{loader_version}-{mc}");
    if tokio::fs::try_exists(paths.version_json(&expected_id)).await? {
        return Ok(expected_id);
    }
    let text = http
        .get(format!("{meta}/versions/loader/{mc}/{loader_version}/profile/json"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let parsed: VersionJson = serde_json::from_str(&text)?;
    let id = safe_component(&parsed.id)?.to_string();
    write_atomic(&paths.version_json(&id), text.as_bytes()).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_paths() {
        assert_eq!(
            maven_path("net.fabricmc:fabric-loader:0.16.9").ok().as_deref(),
            Some("net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar")
        );
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.3:natives-windows").ok().as_deref(),
            Some("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar")
        );
        assert_eq!(
            maven_path("de.oceanlabs.mcp:mcp_config:1.20.1@zip").ok().as_deref(),
            Some("de/oceanlabs/mcp/mcp_config/1.20.1/mcp_config-1.20.1.zip")
        );
        assert!(maven_path("a:b").is_err());
        assert!(maven_path("a:..:c").is_err());
    }

    #[test]
    fn rules_evaluation() {
        let allow_all = Rule { action: "allow".into(), os: None, features: None };
        let disallow_mine = Rule {
            action: "disallow".into(),
            os: Some(OsRule { name: Some(os_name().into()), arch: None }),
            features: None,
        };
        let feature = Rule {
            action: "allow".into(),
            os: None,
            features: Some(HashMap::from([("is_demo_user".to_string(), true)])),
        };
        assert!(rules_allow(&[]));
        assert!(rules_allow(&[allow_all.clone()]));
        assert!(!rules_allow(&[allow_all, disallow_mine]));
        assert!(!rules_allow(&[feature]));
    }

    #[test]
    fn merge_child_first() {
        let lib = |n: &str| Library {
            name: n.into(),
            url: None,
            sha1: None,
            size: None,
            downloads: None,
            natives: None,
            rules: None,
            extract: None,
        };
        let parent = VersionJson {
            id: "1.21.1".into(),
            main_class: Some("net.minecraft.client.main.Main".into()),
            libraries: vec![lib("org.ow2.asm:asm:9.0")],
            ..Default::default()
        };
        let child = VersionJson {
            id: "fabric-loader-0.16.9-1.21.1".into(),
            inherits_from: Some("1.21.1".into()),
            main_class: Some("net.fabricmc.loader.impl.launch.knot.KnotClient".into()),
            libraries: vec![lib("org.ow2.asm:asm:9.7")],
            ..Default::default()
        };
        let merged = merge(child, parent);
        assert_eq!(merged.jar.as_deref(), Some("1.21.1"));
        assert_eq!(merged.main_class.as_deref(), Some("net.fabricmc.loader.impl.launch.knot.KnotClient"));
        let entries = library_entries(&merged.libraries, Path::new("/libs")).unwrap_or_default();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].path.to_string_lossy().contains("9.7"));
    }
}
