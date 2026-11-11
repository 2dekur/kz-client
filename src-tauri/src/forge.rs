//! Forge & NeoForge : téléchargement de l'installeur officiel, lecture de `install_profile.json`,
//! exécution des processeurs (format moderne) ou extraction du jar universel (format legacy ≤ 1.12).

use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde::Deserialize;
use serde_json::json;
use tauri::AppHandle;
use tokio::process::Command;
use zip::ZipArchive;

use crate::{
    download::{self, Task},
    error::{KzError, KzResult},
    launcher::{log, no_window},
    paths::{join_rel, path_str, safe_component, with_suffix, write_atomic, Paths},
    versions::{self, library_path, maven_path, Library, VersionJson},
};

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
const FORGE_PROMOS: &str = "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const NEO_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
const NEO_VERSIONS: &str = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
const INSTALLED_MARKER: &str = ".kz-installed";

#[derive(Debug, Clone, Copy)]
pub enum Flavor {
    Forge,
    NeoForge,
}

impl Flavor {
    fn label(self) -> &'static str {
        match self {
            Flavor::Forge => "Forge",
            Flavor::NeoForge => "NeoForge",
        }
    }
}

pub struct Prepared {
    pub version_id: String,
    /// Bibliothèques nécessaires aux processeurs (vide si déjà installé).
    pub tasks: Vec<Task>,
    pub pending: Option<Pending>,
}

pub struct Pending {
    installer: PathBuf,
    profile: InstallProfile,
    version_id: String,
}

#[derive(Deserialize)]
struct Promotions {
    promos: HashMap<String, String>,
}

#[derive(Deserialize)]
struct NeoVersions {
    versions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct InstallProfile {
    #[serde(default)]
    data: HashMap<String, SidedValue>,
    #[serde(default)]
    processors: Vec<Processor>,
    #[serde(default)]
    libraries: Vec<Library>,
    #[serde(default)]
    json: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SidedValue {
    #[serde(default)]
    client: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Processor {
    jar: String,
    #[serde(default)]
    classpath: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    outputs: HashMap<String, String>,
    #[serde(default)]
    sides: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyInstall {
    path: String,
    file_path: String,
}

enum InstallerData {
    Legacy(serde_json::Value),
    Modern(InstallProfile, serde_json::Value),
}

// ------------------------------------------------------------------ résolution de version

fn neoforge_prefix(mc: &str) -> KzResult<String> {
    if mc == "1.20.1" {
        return Err(KzError::Unsupported(
            "NeoForge 1.20.1 utilise l'ancien format Forge : choisis Forge pour cette version".into(),
        ));
    }
    let nums: Vec<&str> = mc.split('.').collect();
    if !nums.iter().all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
        return Err(KzError::Unsupported(format!("NeoForge ne supporte pas {mc}")));
    }
    Ok(match nums.as_slice() {
        ["1", minor] => format!("{minor}.0."),
        ["1", minor, patch] => format!("{minor}.{patch}."),
        [year, drop] => format!("{year}.{drop}.0."),
        [year, drop, fix] => format!("{year}.{drop}.{fix}."),
        _ => return Err(KzError::Unsupported(format!("NeoForge ne supporte pas {mc}"))),
    })
}

async fn latest_neoforge(http: &reqwest::Client, mc: &str) -> KzResult<String> {
    let prefix = neoforge_prefix(mc)?;
    let list: NeoVersions = versions::get_json(http, NEO_VERSIONS).await?;
    let matching: Vec<&String> = list.versions.iter().filter(|v| v.starts_with(&prefix)).collect();
    matching
        .iter()
        .rev()
        .find(|v| !v.contains('-'))
        .or(matching.last())
        .map(|v| v.to_string())
        .ok_or_else(|| KzError::Unsupported(format!("NeoForge n'est pas disponible pour Minecraft {mc}")))
}

/// Liste des (URL installeur, nom de cache) candidats.
async fn installer_candidates(
    http: &reqwest::Client,
    flavor: Flavor,
    mc: &str,
    requested: Option<&str>,
) -> KzResult<Vec<(String, String)>> {
    match flavor {
        Flavor::Forge => {
            let full = match requested {
                Some(v) if v.starts_with(&format!("{mc}-")) => v.to_string(),
                Some(v) => format!("{mc}-{v}"),
                None => {
                    let promos: Promotions = versions::get_json(http, FORGE_PROMOS).await?;
                    let v = promos
                        .promos
                        .get(&format!("{mc}-recommended"))
                        .or_else(|| promos.promos.get(&format!("{mc}-latest")))
                        .ok_or_else(|| KzError::Unsupported(format!("Forge n'est pas disponible pour Minecraft {mc}")))?;
                    format!("{mc}-{v}")
                }
            };
            safe_component(&full)?;
            // Les très anciennes builds (≤ 1.9) suffixent la version MC une seconde fois.
            Ok([full.clone(), format!("{full}-{mc}")]
                .into_iter()
                .map(|f| (format!("{FORGE_MAVEN}/{f}/forge-{f}-installer.jar"), format!("forge-{f}-installer.jar")))
                .collect())
        }
        Flavor::NeoForge => {
            let v = match requested {
                Some(v) => v.to_string(),
                None => latest_neoforge(http, mc).await?,
            };
            safe_component(&v)?;
            Ok(vec![(format!("{NEO_MAVEN}/{v}/neoforge-{v}-installer.jar"), format!("neoforge-{v}-installer.jar"))])
        }
    }
}

async fn remote_sha1(http: &reqwest::Client, url: &str) -> Option<String> {
    let resp = http.get(format!("{url}.sha1")).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    let s = text.split_whitespace().next()?.to_ascii_lowercase();
    (s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())).then_some(s)
}

// ------------------------------------------------------------------ lecture de l'installeur (bloquant)

fn extract_to(reader: &mut impl Read, dest: &Path) -> KzResult<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = with_suffix(dest, ".kzpart");
    {
        let mut out = File::create(&tmp)?;
        std::io::copy(reader, &mut out)?;
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

fn extract_entry(zip: &mut ZipArchive<File>, name: &str, dest: &Path) -> KzResult<()> {
    let mut entry = zip.by_name(name)?;
    extract_to(&mut entry, dest)
}

fn read_installer(installer: &Path, libs: &Path) -> KzResult<InstallerData> {
    let mut zip = ZipArchive::new(File::open(installer)?)?;
    let mut text = String::new();
    zip.by_name("install_profile.json")?.read_to_string(&mut text)?;
    let profile: serde_json::Value = serde_json::from_str(&text)?;

    // Artefacts embarqués (maven/…) : forge universal, shims, etc.
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let Some(rel) = name.strip_prefix("maven/") else { continue };
        let dest = join_rel(libs, rel)?;
        if dest.exists() {
            continue;
        }
        extract_to(&mut entry, &dest)?;
    }

    if let (Some(install), Some(version)) = (profile.get("install"), profile.get("versionInfo")) {
        let install: LegacyInstall = serde_json::from_value(install.clone())?;
        let dest = library_path(libs, &install.path)?;
        if !dest.exists() {
            extract_entry(&mut zip, &install.file_path, &dest)?;
        }
        return Ok(InstallerData::Legacy(version.clone()));
    }

    let parsed: InstallProfile = serde_json::from_value(profile)?;
    let json_name = parsed.json.clone().unwrap_or_else(|| "/version.json".into());
    let mut version_text = String::new();
    zip.by_name(json_name.trim_start_matches('/'))?.read_to_string(&mut version_text)?;
    Ok(InstallerData::Modern(parsed, serde_json::from_str(&version_text)?))
}

fn main_class(jar: &Path) -> KzResult<String> {
    let mut zip = ZipArchive::new(File::open(jar)?)?;
    let mut manifest = String::new();
    zip.by_name("META-INF/MANIFEST.MF")?.read_to_string(&mut manifest)?;
    // Les lignes de manifeste > 72 octets continuent sur la ligne suivante préfixée d'un espace.
    let mut unfolded = String::new();
    for line in manifest.lines() {
        match line.strip_prefix(' ') {
            Some(cont) => unfolded.push_str(cont),
            None => {
                unfolded.push('\n');
                unfolded.push_str(line);
            }
        }
    }
    unfolded
        .lines()
        .find_map(|l| l.strip_prefix("Main-Class:"))
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .ok_or_else(|| KzError::Unsupported(format!("Main-Class absent dans {}", jar.display())))
}

struct DataContext<'a> {
    installer: &'a Path,
    tmp: &'a Path,
    libs: &'a Path,
    root: &'a Path,
    mc_jar: &'a Path,
    mc: &'a str,
}

fn resolve_data(raw: &HashMap<String, SidedValue>, ctx: &DataContext) -> KzResult<HashMap<String, String>> {
    let mut zip = ZipArchive::new(File::open(ctx.installer)?)?;
    let mut out = HashMap::new();
    for (key, value) in raw {
        let Some(val) = &value.client else { continue };
        let resolved = if let Some(coord) = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            path_str(&library_path(ctx.libs, coord)?)
        } else if let Some(lit) = val.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
            lit.to_string()
        } else if let Some(inner) = val.strip_prefix('/') {
            let dest = join_rel(ctx.tmp, inner)?;
            extract_entry(&mut zip, inner, &dest)?;
            path_str(&dest)
        } else {
            val.clone()
        };
        out.insert(key.clone(), resolved);
    }
    out.insert("SIDE".into(), "client".into());
    out.insert("MINECRAFT_JAR".into(), path_str(ctx.mc_jar));
    out.insert("MINECRAFT_VERSION".into(), ctx.mc.to_string());
    out.insert("ROOT".into(), path_str(ctx.root));
    out.insert("INSTALLER".into(), path_str(ctx.installer));
    out.insert("LIBRARY_DIR".into(), path_str(ctx.libs));
    Ok(out)
}

fn resolve_arg(arg: &str, data: &HashMap<String, String>, libs: &Path) -> KzResult<String> {
    if let Some(coord) = arg.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return Ok(path_str(&library_path(libs, coord)?));
    }
    if let Some(lit) = arg.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return Ok(lit.to_string());
    }
    let mut out = arg.to_string();
    for (k, v) in data {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    Ok(out)
}

async fn outputs_ok(outputs: &[(PathBuf, String)]) -> KzResult<bool> {
    for (path, sha1) in outputs {
        if !tokio::fs::try_exists(path).await? {
            return Ok(false);
        }
        if !download::sha1_file(path.clone()).await?.eq_ignore_ascii_case(sha1) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn tail(bytes: &[u8], max: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(max)..].iter().collect()
}

// ------------------------------------------------------------------ API publique

/// Télécharge/lit l'installeur, écrit le profil de version et prépare l'installation si nécessaire.
pub async fn prepare(
    app: &AppHandle,
    http: &reqwest::Client,
    paths: &Paths,
    flavor: Flavor,
    mc: &str,
    requested: Option<&str>,
) -> KzResult<Prepared> {
    let candidates = installer_candidates(http, flavor, mc, requested).await?;
    let mut chosen = None;
    for (url, name) in candidates {
        let path = paths.installers().join(&name);
        if tokio::fs::try_exists(&path).await? {
            chosen = Some((url, path, true));
            break;
        }
        let reachable = http.head(&url).send().await.map(|r| r.status().is_success()).unwrap_or(false);
        if reachable {
            chosen = Some((url, path, false));
            break;
        }
    }
    let (url, installer, cached) = chosen.ok_or_else(|| {
        KzError::Unsupported(format!("Installeur {} introuvable pour Minecraft {mc}", flavor.label()))
    })?;
    if !cached {
        let sha1 = remote_sha1(http, &url).await;
        download::download_all(http, app, "Installeur", vec![Task::new(&url, installer.clone(), sha1, None)], false)
            .await?;
    }

    let libs = paths.libraries();
    let data = {
        let (i, l) = (installer.clone(), libs.clone());
        tokio::task::spawn_blocking(move || read_installer(&i, &l)).await??
    };

    match data {
        InstallerData::Legacy(mut value) => {
            if let Some(obj) = value.as_object_mut() {
                obj.entry("inheritsFrom").or_insert_with(|| json!(mc));
                obj.entry("jar").or_insert_with(|| json!(mc));
            }
            let parsed: VersionJson = serde_json::from_value(value.clone())?;
            let id = safe_component(&parsed.id)?.to_string();
            write_atomic(&paths.version_json(&id), &serde_json::to_vec_pretty(&value)?).await?;
            Ok(Prepared { version_id: id, tasks: Vec::new(), pending: None })
        }
        InstallerData::Modern(profile, value) => {
            let parsed: VersionJson = serde_json::from_value(value.clone())?;
            let id = safe_component(&parsed.id)?.to_string();
            write_atomic(&paths.version_json(&id), &serde_json::to_vec_pretty(&value)?).await?;
            let installed = tokio::fs::try_exists(paths.version_dir(&id).join(INSTALLED_MARKER)).await?;
            if installed || profile.processors.is_empty() {
                return Ok(Prepared { version_id: id, tasks: Vec::new(), pending: None });
            }
            let tasks = versions::library_entries(&profile.libraries, &libs)?
                .into_iter()
                .filter_map(|e| e.task)
                .collect();
            Ok(Prepared {
                version_id: id.clone(),
                tasks,
                pending: Some(Pending { installer, profile, version_id: id }),
            })
        }
    }
}

/// Exécute les processeurs client (binpatch, remapping, extraction des mappings…).
pub async fn run_processors(app: &AppHandle, paths: &Paths, pending: Pending, java: &Path, mc: &str) -> KzResult<()> {
    let libs = paths.libraries();
    let tmp = paths.tmp().join(format!("forge-{}", pending.version_id));
    tokio::fs::create_dir_all(&tmp).await?;
    let data = {
        let raw = pending.profile.data.clone();
        let (installer, tmp, libs, root, mc_jar, mc) = (
            pending.installer.clone(),
            tmp.clone(),
            libs.clone(),
            paths.root.clone(),
            paths.version_jar(mc),
            mc.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            resolve_data(
                &raw,
                &DataContext {
                    installer: &installer,
                    tmp: &tmp,
                    libs: &libs,
                    root: &root,
                    mc_jar: &mc_jar,
                    mc: &mc,
                },
            )
        })
        .await??
    };
    let sep = if cfg!(windows) { ";" } else { ":" };

    let client_side = |p: &&Processor| p.sides.as_ref().map(|s| s.iter().any(|x| x == "client")).unwrap_or(true);
    for step in pending.profile.processors.iter().filter(client_side) {
        let jar = library_path(&libs, &step.jar)?;
        let outputs = step
            .outputs
            .iter()
            .map(|(k, v)| Ok((PathBuf::from(resolve_arg(k, &data, &libs)?), resolve_arg(v, &data, &libs)?)))
            .collect::<KzResult<Vec<_>>>()?;
        if !outputs.is_empty() && outputs_ok(&outputs).await? {
            continue;
        }
        let main = {
            let j = jar.clone();
            tokio::task::spawn_blocking(move || main_class(&j)).await??
        };
        let mut classpath = vec![path_str(&jar)];
        for coord in &step.classpath {
            classpath.push(path_str(&library_path(&libs, coord)?));
        }
        let args = step
            .args
            .iter()
            .map(|a| resolve_arg(a, &data, &libs))
            .collect::<KzResult<Vec<_>>>()?;

        log(app, "stdout", Some("INFO".into()), format!("[Installeur] {} ({})", maven_path(&step.jar)?, main));
        let mut cmd = Command::new(java);
        cmd.arg("-cp")
            .arg(classpath.join(sep))
            .arg(&main)
            .args(&args)
            .current_dir(&tmp)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_window(&mut cmd);
        let out = cmd.output().await?;
        if !out.status.success() {
            return Err(KzError::Unsupported(format!(
                "Le processeur {} a échoué : {}{}",
                step.jar,
                tail(&out.stdout, 600),
                tail(&out.stderr, 1200)
            )));
        }
        if !outputs.is_empty() && !outputs_ok(&outputs).await? {
            return Err(KzError::Unsupported(format!("Sortie invalide du processeur {}", step.jar)));
        }
    }

    tokio::fs::write(paths.version_dir(&pending.version_id).join(INSTALLED_MARKER), b"ok").await?;
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neoforge_prefixes() {
        assert_eq!(neoforge_prefix("1.21.1").ok().as_deref(), Some("21.1."));
        assert_eq!(neoforge_prefix("1.21").ok().as_deref(), Some("21.0."));
        assert_eq!(neoforge_prefix("26.1").ok().as_deref(), Some("26.1.0."));
        assert!(neoforge_prefix("1.20.1").is_err());
        assert!(neoforge_prefix("24w14a").is_err());
    }

    #[test]
    fn processor_args() {
        let data = HashMap::from([("SIDE".to_string(), "client".to_string())]);
        let libs = Path::new("/libs");
        assert_eq!(resolve_arg("{SIDE}", &data, libs).ok().as_deref(), Some("client"));
        assert_eq!(resolve_arg("'abc'", &data, libs).ok().as_deref(), Some("abc"));
        assert!(resolve_arg("[net.minecraftforge:installertools:1.4.1]", &data, libs)
            .map(|p| p.ends_with("installertools-1.4.1.jar"))
            .unwrap_or(false));
    }
}
