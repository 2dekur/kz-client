use std::path::{Path, PathBuf};

use crate::error::{KzError, KzResult};

/// Arborescence de données du launcher (dans le dossier AppData de Tauri).
#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }
    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions().join(id)
    }
    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }
    pub fn version_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }
    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }
    pub fn runtimes(&self) -> PathBuf {
        self.root.join("runtimes")
    }
    pub fn natives(&self, id: &str) -> PathBuf {
        self.root.join("natives").join(id)
    }
    pub fn instance(&self, name: &str) -> PathBuf {
        self.root.join("instances").join(name)
    }
    pub fn installers(&self) -> PathBuf {
        self.root.join("installers")
    }
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }
    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }
}

/// Valide un identifiant utilisé comme nom de fichier (version, loader, composant Java).
pub fn safe_component(s: &str) -> KzResult<&str> {
    let ok = !s.is_empty()
        && s.len() <= 128
        && s.trim() == s
        && !s.chars().all(|c| c == '.')
        && s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | ' '));
    if ok {
        Ok(s)
    } else {
        Err(KzError::Unsupported(format!("Identifiant invalide : {s:?}")))
    }
}

/// Joint un chemin relatif venant d'un JSON distant en refusant toute sortie du dossier de base.
pub fn join_rel(base: &Path, rel: &str) -> KzResult<PathBuf> {
    let mut out = base.to_path_buf();
    let mut pushed = false;
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return Err(KzError::Unsupported(format!("Chemin refusé : {rel}"))),
            p if p.contains(':') => return Err(KzError::Unsupported(format!("Chemin refusé : {rel}"))),
            p => {
                out.push(p);
                pushed = true;
            }
        }
    }
    if pushed {
        Ok(out)
    } else {
        Err(KzError::Unsupported(format!("Chemin vide : {rel:?}")))
    }
}

pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

pub fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Écriture atomique (fichier temporaire + rename) : jamais de fichier à moitié écrit.
pub async fn write_atomic(path: &Path, data: &[u8]) -> KzResult<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = with_suffix(path, ".kztmp");
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_rel_blocks_traversal() {
        let base = Path::new("/data");
        assert!(join_rel(base, "a/b/c.jar").is_ok());
        assert!(join_rel(base, "../etc/passwd").is_err());
        assert!(join_rel(base, "a/../../x").is_err());
        assert!(join_rel(base, "C:/Windows").is_err());
        assert!(join_rel(base, "").is_err());
    }

    #[test]
    fn safe_component_rules() {
        assert!(safe_component("1.21.1").is_ok());
        assert!(safe_component("1.14 Pre-Release 1").is_ok());
        assert!(safe_component("..").is_err());
        assert!(safe_component("a/b").is_err());
        assert!(safe_component(" x").is_err());
    }
}
