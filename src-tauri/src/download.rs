//! Moteur de téléchargement parallèle (HTTP/2, reprise sur erreur, vérification SHA-1).

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use futures::{StreamExt, TryStreamExt};
use serde::Serialize;
use sha1::{Digest, Sha1};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use crate::{
    error::{KzError, KzResult},
    paths::with_suffix,
};

const CONCURRENCY: usize = 48;
const RETRIES: u32 = 4;

#[derive(Debug, Clone)]
pub struct Task {
    pub url: String,
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub executable: bool,
}

impl Task {
    pub fn new(url: impl Into<String>, path: PathBuf, sha1: Option<String>, size: Option<u64>) -> Self {
        Self {
            url: url.into(),
            path,
            sha1,
            size,
            executable: false,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub stage: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Octets/seconde (moyenne glissante).
    pub speed: f64,
}

#[derive(Default)]
struct Counters {
    /// Octets « acquis » (téléchargés ou déjà valides) des fichiers de taille connue.
    bytes: AtomicU64,
    /// Octets réellement reçus du réseau (pour la vitesse).
    net: AtomicU64,
    files: AtomicU64,
}

/// Télécharge toutes les tâches en parallèle.
/// `quick` : un fichier présent avec la bonne taille est accepté sans recalcul SHA-1
/// (utilisé quand la même liste a déjà été entièrement vérifiée au lancement précédent).
pub async fn download_all(
    http: &reqwest::Client,
    app: &AppHandle,
    stage: &str,
    tasks: Vec<Task>,
    quick: bool,
) -> KzResult<()> {
    let mut seen = HashSet::new();
    let tasks: Vec<Task> = tasks.into_iter().filter(|t| seen.insert(t.path.clone())).collect();
    let files_total = tasks.len() as u64;
    let bytes_total: u64 = tasks.iter().filter_map(|t| t.size).sum();
    let counters = Arc::new(Counters::default());

    let ticker = {
        let app = app.clone();
        let c = counters.clone();
        let stage = stage.to_string();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            let (mut last_net, mut last_at, mut speed) = (0u64, Instant::now(), 0f64);
            loop {
                interval.tick().await;
                let net = c.net.load(Ordering::Relaxed);
                let dt = last_at.elapsed().as_secs_f64();
                if dt > 0.0 {
                    let instant = net.saturating_sub(last_net) as f64 / dt;
                    speed = speed * 0.8 + instant * 0.2;
                }
                last_net = net;
                last_at = Instant::now();
                emit_progress(&app, &stage, &c, files_total, bytes_total, speed);
            }
        })
    };

    let result = futures::stream::iter(tasks.into_iter().map(Ok::<Task, KzError>))
        .try_for_each_concurrent(CONCURRENCY, |task| {
            let c = counters.clone();
            async move { fetch(http, task, &c, quick).await }
        })
        .await;

    ticker.abort();
    emit_progress(app, stage, &counters, files_total, bytes_total, 0.0);
    result
}

fn emit_progress(app: &AppHandle, stage: &str, c: &Counters, files_total: u64, bytes_total: u64, speed: f64) {
    let _ = app.emit(
        "kz://progress",
        ProgressEvent {
            stage: stage.to_string(),
            files_done: c.files.load(Ordering::Relaxed),
            files_total,
            bytes_done: c.bytes.load(Ordering::Relaxed).min(bytes_total),
            bytes_total,
            speed,
        },
    );
}

async fn fetch(http: &reqwest::Client, task: Task, c: &Counters, quick: bool) -> KzResult<()> {
    if is_valid(&task, quick).await? {
        c.bytes.fetch_add(task.size.unwrap_or(0), Ordering::Relaxed);
        c.files.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    if let Some(parent) = task.path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut last = None;
    for attempt in 0..RETRIES {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(300 * 2u64.pow(attempt))).await;
        }
        match transfer(http, &task, c).await {
            Ok(()) => {
                set_executable(&task).await?;
                c.files.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            // 404 : inutile de réessayer.
            Err(e @ KzError::Unsupported(_)) => return Err(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| KzError::Unsupported(format!("Téléchargement impossible : {}", task.url))))
}

async fn transfer(http: &reqwest::Client, task: &Task, c: &Counters) -> KzResult<()> {
    let tmp = with_suffix(&task.path, ".kzpart");
    let counted = task.size.is_some();
    let mut written = 0u64;
    let outcome: KzResult<()> = async {
        let resp = http.get(&task.url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(KzError::Unsupported(format!("Fichier introuvable (404) : {}", task.url)));
        }
        let resp = resp.error_for_status()?;
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut hasher = Sha1::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            let n = chunk.len() as u64;
            written += n;
            if counted {
                c.bytes.fetch_add(n, Ordering::Relaxed);
            }
            c.net.fetch_add(n, Ordering::Relaxed);
        }
        file.flush().await?;
        drop(file);
        let actual = hex::encode(hasher.finalize());
        if let Some(expected) = &task.sha1 {
            if !expected.eq_ignore_ascii_case(&actual) {
                return Err(KzError::Integrity {
                    path: task.path.display().to_string(),
                    expected: expected.clone(),
                    actual,
                });
            }
        }
        tokio::fs::rename(&tmp, &task.path).await?;
        Ok(())
    }
    .await;
    if outcome.is_err() {
        if counted {
            c.bytes.fetch_sub(written, Ordering::Relaxed);
        }
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    outcome
}

async fn is_valid(task: &Task, quick: bool) -> KzResult<bool> {
    let meta = match tokio::fs::metadata(&task.path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    if !meta.is_file() {
        return Ok(false);
    }
    if let Some(size) = task.size {
        if meta.len() != size {
            return Ok(false);
        }
    }
    match &task.sha1 {
        None => Ok(true),
        Some(_) if quick => Ok(true),
        Some(expected) => Ok(sha1_file(task.path.clone()).await?.eq_ignore_ascii_case(expected)),
    }
}

async fn set_executable(task: &Task) -> KzResult<()> {
    #[cfg(unix)]
    {
        if task.executable {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&task.path, std::fs::Permissions::from_mode(0o755)).await?;
        }
    }
    #[cfg(not(unix))]
    let _ = task;
    Ok(())
}

/// SHA-1 d'un fichier, calculé hors du runtime async.
pub async fn sha1_file(path: PathBuf) -> KzResult<String> {
    tokio::task::spawn_blocking(move || -> KzResult<String> {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha1::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(hex::encode(hasher.finalize()))
    })
    .await?
}

pub fn sha1_hex(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}
