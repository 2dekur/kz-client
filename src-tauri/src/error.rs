use serde::{Serialize, Serializer};

/// Erreur unique du backend : chaque variante a un message lisible côté UI.
#[derive(Debug, thiserror::Error)]
pub enum KzError {
    #[error("Erreur réseau : {0}")]
    Http(#[from] reqwest::Error),
    #[error("Erreur disque : {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON invalide : {0}")]
    Json(#[from] serde_json::Error),
    #[error("Archive invalide : {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("Trousseau système : {0}")]
    Keyring(#[from] keyring::Error),
    #[error("Tâche interrompue : {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Authentification : {0}")]
    Auth(String),
    #[error("Ce compte Microsoft ne possède pas Minecraft: Java Edition")]
    NotOwned,
    #[error("Fichier corrompu {path} (SHA-1 attendu {expected}, reçu {actual})")]
    Integrity {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("{0}")]
    Unsupported(String),
    #[error("Aucun compte actif : ajoute un compte Microsoft")]
    NoAccount,
    #[error("Minecraft est déjà en cours d'exécution")]
    AlreadyRunning,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Serialize for KzError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type KzResult<T> = Result<T, KzError>;
