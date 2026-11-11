//! Orchestration du lancement : auth → résolution → Java → téléchargements → loader → KZ Engine
//! → natives → classpath/arguments → spawn JVM avec flux stdout/stderr vers le frontend.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
};
use zip::ZipArchive;

use crate::{
    discord::Presence,
    download::{self, Task},
    error::{KzError, KzResult},
    forge, java, kzengine,
    paths::{join_rel, path_str, safe_component, write_atomic},
    versions::{self, ArgValue, Argument, AssetIndex, Loader},
    AppState,
};

const RESOURCES_URL: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOptions {
    pub version: String,
    pub loader: Loader,
    #[serde(default)]
    pub loader_version: Option<String>,
    #[serde(default)]
    pub kz_engine: bool,
    pub ram_mb: u64,
}

#[derive(Serialize, Clone)]
struct PhaseEvent {
    phase: &'static str,
    detail: String,
}

#[derive(Serialize, Clone)]
pub struct LogLine {
    pub stream: &'static str,
    pub level: Option<String>,
    pub line: String,
}

#[derive(Serialize, Clone)]
struct ExitEvent {
    code: Option<i32>,
}

pub fn phase(app: &AppHandle, phase: &'static str, detail: impl Into<String>) {
    let _ = app.emit("kz://phase", PhaseEvent { phase, detail: detail.into() });
}

pub fn log(app: &AppHandle, stream: &'static str, level: Option<String>, line: String) {
    let _ = app.emit("kz://log", LogLine { stream, level, line });
}

/// Pas de fenêtre console pour java.exe sous Windows.
pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn launch(app: AppHandle, opts: LaunchOptions) -> KzResult<()> {
    let state = app.state::<AppState>();
    if state.running.swap(true, Ordering::SeqCst) {
        return Err(KzError::AlreadyRunning);
    }
    let result = prepare_and_spawn(&app, state.inner(), opts).await;
    if result.is_err() {
        state.running.store(false, Ordering::SeqCst);
        phase(&app, "idle", String::new());
    }
    result
}

async fn prepare_and_spawn(app: &AppHandle, state: &AppState, opts: LaunchOptions) -> KzResult<()> {
    let mc = safe_component(opts.version.trim())?.to_string();
    let loader_version = opts
        .loader_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| safe_component(v).map(str::to_string))
        .transpose()?;
    let ram_mb = clamp_ram(opts.ram_mb);
    let (http, paths) = (&state.http, &state.paths);

    // 1. Authentification (refresh silencieux si nécessaire)
    phase(app, "auth", "Authentification...");
    let uuid = state.auth.active().await.ok_or(KzError::NoAccount)?;
    let session = state.auth.ensure(&uuid, 300).await?;

    // 2. Profil vanilla + loader
    phase(app, "resolve", format!("Minecraft {mc}"));
    let manifest = state.manifest().await?;
    let vanilla = versions::ensure_vanilla(http, paths, manifest, &mc).await?;

    let mut extra_tasks = Vec::new();
    let mut pending = None;
    let launch_id = match opts.loader {
        Loader::Vanilla => vanilla.id.clone(),
        Loader::Fabric | Loader::Quilt => {
            phase(app, "loader", format!("{} pour {mc}", opts.loader.label()));
            versions::install_fabric_like(http, paths, opts.loader, &mc, loader_version.as_deref()).await?
        }
        Loader::Forge | Loader::Neoforge => {
            phase(app, "loader", format!("{} pour {mc}", opts.loader.label()));
            let flavor = if opts.loader == Loader::Forge { forge::Flavor::Forge } else { forge::Flavor::NeoForge };
            let prepared = forge::prepare(app, http, paths, flavor, &mc, loader_version.as_deref()).await?;
            extra_tasks = prepared.tasks;
            pending = prepared.pending;
            prepared.version_id
        }
    };
    let version = versions::resolve(paths, &launch_id, &vanilla).await?;

    // 3. Java adapté (Java 8 / 17 / 21+ selon le profil)
    let (component, java_major) = version
        .java_version
        .as_ref()
        .map(|j| (j.component.clone(), j.major_version))
        .unwrap_or_else(|| ("jre-legacy".to_string(), 8));
    phase(app, "java", format!("Java {java_major}"));
    let runtime = java::prepare(http, paths, &component).await?;

    // 4. Index des assets
    let asset_ref = version
        .asset_index
        .clone()
        .ok_or_else(|| KzError::Unsupported(format!("{launch_id} : assetIndex manquant")))?;
    let assets_dir = paths.assets();
    let index_path = assets_dir.join("indexes").join(format!("{}.json", safe_component(&asset_ref.id)?));
    download::download_all(
        http,
        app,
        "Index des ressources",
        vec![Task::new(&asset_ref.url, index_path.clone(), Some(asset_ref.sha1.clone()), Some(asset_ref.size))],
        false,
    )
    .await?;
    let index: AssetIndex = serde_json::from_slice(&tokio::fs::read(&index_path).await?)?;

    // 5. Liste complète des fichiers
    let libraries = versions::library_entries(&version.libraries, &paths.libraries())?;
    let client = vanilla
        .downloads
        .as_ref()
        .and_then(|d| d.client.as_ref())
        .ok_or_else(|| KzError::Unsupported(format!("client.jar absent du profil {mc}")))?;
    let vanilla_jar = paths.version_jar(&mc);

    let mut tasks = runtime.tasks.clone();
    tasks.extend(extra_tasks);
    tasks.push(Task::new(&client.url, vanilla_jar.clone(), client.sha1.clone(), client.size));
    tasks.extend(libraries.iter().filter_map(|l| l.task.clone()));
    let objects_dir = assets_dir.join("objects");
    for obj in index.objects.values() {
        if obj.hash.len() != 40 || !obj.hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let prefix = &obj.hash[..2];
        tasks.push(Task::new(
            format!("{RESOURCES_URL}/{prefix}/{}", obj.hash),
            objects_dir.join(prefix).join(&obj.hash),
            Some(obj.hash.clone()),
            Some(obj.size),
        ));
    }
    let mut logging_arg = None;
    if let Some(lc) = version.logging.as_ref().and_then(|l| l.client.as_ref()) {
        let path = assets_dir.join("log_configs").join(safe_component(&lc.file.id)?);
        tasks.push(Task::new(&lc.file.url, path.clone(), Some(lc.file.sha1.clone()), Some(lc.file.size)));
        logging_arg = Some(lc.argument.replace("${path}", &path_str(&path)));
    }

    // 6. Téléchargement + vérification (rapide si la même liste a déjà été validée)
    let stamp = verification_stamp(&tasks);
    let stamp_path = paths.version_dir(&launch_id).join(".kz-verified");
    let quick = tokio::fs::read_to_string(&stamp_path)
        .await
        .map(|s| s.trim() == stamp)
        .unwrap_or(false);
    phase(app, "download", if quick { "Vérification rapide" } else { "Téléchargement & vérification SHA-1" });
    download::download_all(http, app, "Fichiers du jeu", tasks, quick).await?;
    java::link(&runtime).await?;
    write_atomic(&stamp_path, stamp.as_bytes()).await?;

    // 7. Processeurs Forge / NeoForge (première installation uniquement)
    if let Some(p) = pending {
        phase(app, "loader", format!("Installation de {}", opts.loader.label()));
        forge::run_processors(app, paths, p, &runtime.java, &mc).await?;
    }

    // 8. Instance + KZ Engine
    let instance = paths.instance(&format!("{mc}-{}", opts.loader.slug()));
    tokio::fs::create_dir_all(&instance).await?;
    if opts.loader != Loader::Vanilla {
        phase(app, "kzengine", if opts.kz_engine { "Mise à jour KZ Engine..." } else { "Nettoyage KZ Engine" });
        if let Err(e) = kzengine::sync(http, app, &instance.join("mods"), opts.loader, &mc, opts.kz_engine).await {
            log(app, "stderr", Some("WARN".into()), format!("KZ Engine non mis à jour, lancement avec les mods existants : {e}"));
        }
    }

    // 9. Natives, assets legacy, jar client
    phase(app, "launching", "Lancement...");
    let natives_dir = paths.natives(&launch_id);
    let native_jars: Vec<(PathBuf, Vec<String>)> = libraries
        .iter()
        .filter_map(|l| l.extract.clone().map(|ex| (l.path.clone(), ex)))
        .collect();
    {
        let dir = natives_dir.clone();
        tokio::task::spawn_blocking(move || extract_natives(&native_jars, &dir)).await??;
    }

    let game_assets = if index.is_virtual {
        assets_dir.join("virtual").join(&asset_ref.id)
    } else if index.map_to_resources {
        instance.join("resources")
    } else {
        assets_dir.clone()
    };
    if index.is_virtual || index.map_to_resources {
        let (objects, target) = (objects_dir.clone(), game_assets.clone());
        tokio::task::spawn_blocking(move || materialize_legacy_assets(&index, &objects, &target)).await??;
    }

    let client_jar = if launch_id == mc {
        vanilla_jar.clone()
    } else {
        let dest = paths.version_jar(&launch_id);
        copy_if_changed(&vanilla_jar, &dest).await?;
        dest
    };

    // 10. Classpath
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut cp: Vec<PathBuf> = Vec::new();
    for lib in libraries.iter().filter(|l| l.classpath) {
        if !cp.contains(&lib.path) {
            cp.push(lib.path.clone());
        }
    }
    cp.push(client_jar);
    let mut missing = Vec::new();
    for p in &cp {
        if !tokio::fs::try_exists(p).await? {
            missing.push(path_str(p));
        }
    }
    if !missing.is_empty() {
        return Err(KzError::Unsupported(format!("Fichiers manquants : {}", missing.join(", "))));
    }
    let classpath = cp.iter().map(|p| path_str(p)).collect::<Vec<_>>().join(sep);

    // 11. Arguments
    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", session.account.username.clone()),
        ("version_name", launch_id.clone()),
        ("game_directory", path_str(&instance)),
        ("assets_root", path_str(&assets_dir)),
        ("game_assets", path_str(&game_assets)),
        ("assets_index_name", asset_ref.id.clone()),
        ("auth_uuid", session.account.uuid.clone()),
        ("auth_access_token", session.access_token.clone()),
        ("auth_session", format!("token:{}:{}", session.access_token, session.account.uuid)),
        ("auth_xuid", session.xuid.clone()),
        ("clientid", crate::auth::client_id().unwrap_or_default()),
        ("user_type", "msa".to_string()),
        ("user_properties", "{}".to_string()),
        ("version_type", "KZ Client".to_string()),
        ("natives_directory", path_str(&natives_dir)),
        ("launcher_name", "KZ Client".to_string()),
        ("launcher_version", env!("CARGO_PKG_VERSION").to_string()),
        ("classpath", classpath),
        ("classpath_separator", sep.to_string()),
        ("library_directory", path_str(&paths.libraries())),
        ("resolution_width", "854".to_string()),
        ("resolution_height", "480".to_string()),
    ]);

    let mut args = jvm_tuning(java_major, ram_mb);
    let jvm_raw = version.arguments.as_ref().map(|a| flatten(&a.jvm)).unwrap_or_default();
    if !jvm_raw.iter().any(|a| a.contains("${classpath}")) {
        args.extend(legacy_jvm_args().iter().map(|a| substitute(a, &vars)));
    }
    args.extend(jvm_raw.iter().map(|a| substitute(a, &vars)));
    if let Some(l) = logging_arg {
        args.push(l);
    }
    args.push(
        version
            .main_class
            .clone()
            .ok_or_else(|| KzError::Unsupported(format!("{launch_id} : mainClass manquant")))?,
    );
    let game_raw = version.arguments.as_ref().map(|a| flatten(&a.game)).unwrap_or_default();
    if game_raw.is_empty() {
        if let Some(legacy) = &version.minecraft_arguments {
            args.extend(legacy.split_whitespace().map(|a| substitute(a, &vars)));
        }
    } else {
        args.extend(game_raw.iter().map(|a| substitute(a, &vars)));
    }

    // 12. Spawn
    let mut cmd = Command::new(&runtime.java);
    cmd.args(&args)
        .current_dir(&instance)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| {
        KzError::Unsupported(format!("Impossible de démarrer Java ({}) : {e}", runtime.java.display()))
    })?;
    if let Some(out) = child.stdout.take() {
        tauri::async_runtime::spawn(pump(app.clone(), out, "stdout"));
    }
    if let Some(err) = child.stderr.take() {
        tauri::async_runtime::spawn(pump(app.clone(), err, "stderr"));
    }

    let label = format!(
        "Joue en {mc} - {}{}",
        opts.loader.label(),
        if opts.kz_engine && opts.loader != Loader::Vanilla { " + KZ Engine" } else { "" }
    );
    state.discord.set(Presence::Playing {
        username: session.account.username.clone(),
        label: label.clone(),
        started: now_secs(),
    });
    phase(app, "running", label);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let code = child.wait().await.ok().and_then(|s| s.code());
        let state = app.state::<AppState>();
        state.running.store(false, Ordering::SeqCst);
        state.discord.set(Presence::Idle);
        let _ = app.emit("kz://exit", ExitEvent { code });
        phase(&app, "idle", String::new());
    });
    Ok(())
}

// ------------------------------------------------------------------ JVM

fn clamp_ram(requested: u64) -> u64 {
    let max = crate::total_memory_mb().saturating_sub(1024).max(1024);
    requested.clamp(1024, max)
}

/// Flags JVM KZ : ZGC générationnel pour Java 21+ avec beaucoup de RAM, sinon G1 réglé basse latence.
pub fn jvm_tuning(java_major: u32, ram_mb: u64) -> Vec<String> {
    let mut flags = vec![format!("-Xms{ram_mb}M"), format!("-Xmx{ram_mb}M")];
    if java_major >= 21 && ram_mb >= 6144 {
        flags.push("-XX:+UseZGC".into());
        // Mode générationnel : explicite en 21-23, seul mode existant à partir de 24.
        if java_major < 24 {
            flags.push("-XX:+ZGenerational".into());
        }
    } else {
        flags.extend(
            [
                "-XX:+UseG1GC",
                "-XX:+ParallelRefProcEnabled",
                "-XX:MaxGCPauseMillis=50",
                "-XX:+UnlockExperimentalVMOptions",
                "-XX:G1NewSizePercent=30",
                "-XX:G1MaxNewSizePercent=40",
                "-XX:G1HeapRegionSize=8M",
                "-XX:G1ReservePercent=20",
                "-XX:G1HeapWastePercent=5",
                "-XX:G1MixedGCCountTarget=4",
                "-XX:InitiatingHeapOccupancyPercent=15",
                "-XX:G1MixedGCLiveThresholdPercent=90",
                "-XX:SurvivorRatio=32",
                "-XX:MaxTenuringThreshold=1",
            ]
            .map(String::from),
        );
    }
    flags.extend(
        ["-XX:+UseStringDeduplication", "-XX:+DisableExplicitGC", "-XX:+PerfDisableSharedMem"].map(String::from),
    );
    flags
}

fn legacy_jvm_args() -> Vec<&'static str> {
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        v.push("-XstartOnFirstThread");
    }
    if cfg!(windows) {
        v.push("-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump");
    }
    v.extend([
        "-Djava.library.path=${natives_directory}",
        "-Dminecraft.launcher.brand=${launcher_name}",
        "-Dminecraft.launcher.version=${launcher_version}",
        "-cp",
        "${classpath}",
    ]);
    v
}

fn flatten(args: &[Argument]) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        match arg {
            Argument::Plain(s) => out.push(s.clone()),
            Argument::Ruled { rules, value } => {
                if versions::rules_allow(rules) {
                    match value {
                        ArgValue::One(s) => out.push(s.clone()),
                        ArgValue::Many(v) => out.extend(v.iter().cloned()),
                    }
                }
            }
        }
    }
    out
}

/// Remplace les `${clé}` connues ; les inconnues restent telles quelles.
fn substitute(s: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push_str("${");
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn verification_stamp(tasks: &[Task]) -> String {
    let mut keys: Vec<String> = tasks
        .iter()
        .map(|t| format!("{}|{}|{}", t.path.display(), t.sha1.as_deref().unwrap_or(""), t.size.unwrap_or(0)))
        .collect();
    keys.sort();
    let mut hasher = Sha1::new();
    for k in keys {
        hasher.update(k.as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

// ------------------------------------------------------------------ fichiers

fn extract_natives(jars: &[(PathBuf, Vec<String>)], dir: &Path) -> KzResult<()> {
    std::fs::create_dir_all(dir)?;
    for (jar, excludes) in jars {
        let mut zip = ZipArchive::new(std::fs::File::open(jar)?)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            if name.starts_with("META-INF/") || excludes.iter().any(|e| name.starts_with(e.as_str())) {
                continue;
            }
            let Some(rel) = entry.enclosed_name() else { continue };
            let dest = dir.join(rel);
            if let Ok(meta) = std::fs::metadata(&dest) {
                if meta.len() == entry.size() {
                    continue;
                }
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}

fn materialize_legacy_assets(index: &AssetIndex, objects: &Path, target: &Path) -> KzResult<()> {
    for (name, obj) in &index.objects {
        if obj.hash.len() != 40 {
            continue;
        }
        let src = objects.join(&obj.hash[..2]).join(&obj.hash);
        let dest = join_rel(target, name)?;
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dest)?;
    }
    Ok(())
}

async fn copy_if_changed(src: &Path, dest: &Path) -> KzResult<()> {
    let src_len = tokio::fs::metadata(src).await?.len();
    if let Ok(meta) = tokio::fs::metadata(dest).await {
        if meta.len() == src_len {
            return Ok(());
        }
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::copy(src, dest).await?;
    Ok(())
}

// ------------------------------------------------------------------ logs du jeu

fn xml_attr(line: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = line.find(&key)? + key.len();
    let end = line[start..].find('"')?;
    Some(line[start..start + end].to_string())
}

fn cdata(block: &str) -> String {
    let mut parts = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find("<![CDATA[") {
        let after = &rest[start + 9..];
        match after.find("]]>") {
            Some(end) => {
                parts.push(after[..end].trim_end().to_string());
                rest = &after[end + 3..];
            }
            None => break,
        }
    }
    parts.join("\n")
}

/// Relaie la sortie du jeu ligne par ligne. Les évènements log4j XML (config Mojang)
/// sont convertis en lignes lisibles avec leur niveau.
async fn pump<R: AsyncRead + Unpin + Send + 'static>(app: AppHandle, reader: R, stream: &'static str) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    let mut event: Option<(Option<String>, String)> = None;
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.trim_end_matches(['\r', '\n']);

        if let Some(current) = event.as_mut() {
            current.1.push_str(line);
            current.1.push('\n');
            if line.contains("</log4j:Event>") {
                if let Some((level, block)) = event.take() {
                    log(&app, stream, level, cdata(&block));
                }
            }
            continue;
        }
        if line.trim_start().starts_with("<log4j:Event") {
            let level = xml_attr(line, "level");
            if line.contains("</log4j:Event>") {
                log(&app, stream, level, cdata(line));
            } else {
                event = Some((level, format!("{line}\n")));
            }
            continue;
        }
        log(&app, stream, None, line.to_string());
    }
    if let Some((level, block)) = event.take() {
        log(&app, stream, level, cdata(&block));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_vars() {
        let vars = HashMap::from([("a", "1".to_string()), ("b", "two".to_string())]);
        assert_eq!(substitute("-D${a}=${b}", &vars), "-D1=two");
        assert_eq!(substitute("${unknown}", &vars), "${unknown}");
        assert_eq!(substitute("tail ${a", &vars), "tail ${a");
        assert_eq!(substitute("plain", &vars), "plain");
    }

    #[test]
    fn log4j_parsing() {
        let block = r#"<log4j:Event logger="x" level="WARN" thread="main">
  <log4j:Message><![CDATA[Hello world]]></log4j:Message>
</log4j:Event>"#;
        assert_eq!(xml_attr(block, "level").as_deref(), Some("WARN"));
        assert_eq!(cdata(block), "Hello world");
    }

    #[test]
    fn gc_selection() {
        assert!(jvm_tuning(21, 8192).iter().any(|f| f == "-XX:+UseZGC"));
        assert!(jvm_tuning(17, 8192).iter().any(|f| f == "-XX:+UseG1GC"));
        assert!(jvm_tuning(25, 8192).iter().all(|f| f != "-XX:+ZGenerational"));
        assert!(jvm_tuning(8, 4096).iter().any(|f| f == "-Xmx4096M"));
    }
}
