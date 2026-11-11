//! Authentification Microsoft → Xbox Live → XSTS → Minecraft (OAuth2 + PKCE, boucle locale).
//! Les refresh tokens sont stockés uniquement dans le trousseau système (`keyring`).

use std::{
    collections::HashMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use url::Url;

use crate::{
    error::{KzError, KzResult},
    paths::{with_suffix, write_atomic, Paths},
};

const AUTHORIZE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_ENTITLEMENTS_URL: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";
const SCOPE: &str = "XboxLive.signin offline_access";
const KEYRING_SERVICE: &str = "KZClient";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// Contenu de `<AppData>/azure-client-id.txt`, chargé au démarrage.
pub static FILE_CLIENT_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Client ID Azure (application « comptes Microsoft personnels », client public).
/// Priorité : variable d'environnement, puis `azure-client-id.txt` dans AppData, puis valeur figée à la compilation.
pub fn client_id() -> KzResult<String> {
    std::env::var("KZ_AZURE_CLIENT_ID")
        .ok()
        .or_else(|| FILE_CLIENT_ID.get().cloned())
        .or_else(|| option_env!("KZ_AZURE_CLIENT_ID").map(String::from))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            KzError::Auth(
                "Client ID Azure absent : définis KZ_AZURE_CLIENT_ID ou crée azure-client-id.txt dans le dossier de données (voir README)"
                    .into(),
            )
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub uuid: String,
    pub username: String,
    pub skin_url: Option<String>,
    pub skin_slim: bool,
    pub cape_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AccountsFile {
    accounts: Vec<Account>,
    active: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsView {
    pub accounts: Vec<Account>,
    pub active: Option<String>,
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    xuid: String,
    expires_at: u64,
}

#[derive(Clone)]
pub struct Session {
    pub access_token: String,
    pub xuid: String,
    pub account: Account,
}

struct McLogin {
    access_token: String,
    xuid: String,
    expires_at: u64,
    account: Account,
}

struct Store {
    file: AccountsFile,
    sessions: HashMap<String, CachedToken>,
}

pub struct AuthManager {
    http: reqwest::Client,
    paths: Paths,
    inner: Mutex<Store>,
    refresh_lock: Mutex<()>,
}

// ------------------------------------------------------------------ réponses API

#[derive(Deserialize)]
struct MsToken {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct XboxAuth {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Deserialize)]
struct DisplayClaims {
    xui: Vec<Xui>,
}

#[derive(Deserialize)]
struct Xui {
    uhs: String,
}

#[derive(Deserialize, Default)]
struct XstsError {
    #[serde(rename = "XErr", default)]
    x_err: u64,
}

#[derive(Deserialize)]
struct McToken {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize, Default)]
struct Entitlements {
    #[serde(default)]
    items: Vec<Entitlement>,
}

#[derive(Deserialize)]
struct Entitlement {
    name: String,
}

#[derive(Deserialize)]
struct Profile {
    id: String,
    name: String,
    #[serde(default)]
    skins: Vec<Texture>,
    #[serde(default)]
    capes: Vec<Texture>,
}

#[derive(Deserialize)]
struct Texture {
    url: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    variant: Option<String>,
}

// ------------------------------------------------------------------ helpers

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

async fn parse<T: DeserializeOwned>(resp: reqwest::Response, ctx: &str) -> KzResult<T> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(KzError::Auth(format!("{ctx} : HTTP {status} — {}", truncate(&body, 300))));
    }
    Ok(resp.json::<T>().await?)
}

/// Lit une revendication du payload JWT (sans vérification de signature : usage informatif).
fn jwt_claim(token: &str, claim: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    match value.get(claim)? {
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn xsts_message(code: u64) -> String {
    match code {
        2148916233 => "Ce compte Microsoft n'a pas de profil Xbox : crée-le sur xbox.com puis réessaie.".into(),
        2148916235 => "Xbox Live n'est pas disponible dans ton pays.".into(),
        2148916236 | 2148916237 => "Vérification d'âge requise sur xbox.com (Corée du Sud).".into(),
        2148916238 => "Compte enfant : un adulte doit l'ajouter à une famille Microsoft.".into(),
        other => format!("XSTS a refusé l'authentification (code {other})."),
    }
}

fn keyring_user(uuid: &str) -> String {
    format!("msa-refresh:{uuid}")
}

async fn keyring_set(uuid: &str, secret: String) -> KzResult<()> {
    let user = keyring_user(uuid);
    tokio::task::spawn_blocking(move || -> KzResult<()> {
        keyring::Entry::new(KEYRING_SERVICE, &user)?.set_password(&secret)?;
        Ok(())
    })
    .await?
}

async fn keyring_get(uuid: &str) -> KzResult<Option<String>> {
    let user = keyring_user(uuid);
    tokio::task::spawn_blocking(move || -> KzResult<Option<String>> {
        match keyring::Entry::new(KEYRING_SERVICE, &user)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    })
    .await?
}

async fn keyring_delete(uuid: &str) -> KzResult<()> {
    let user = keyring_user(uuid);
    tokio::task::spawn_blocking(move || -> KzResult<()> {
        match keyring::Entry::new(KEYRING_SERVICE, &user)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    })
    .await?
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn page(ok: bool, title: &str, message: &str) -> String {
    let color = if ok { "#a78bfa" } else { "#f87171" };
    format!(
        "<!doctype html><html lang=\"fr\"><head><meta charset=\"utf-8\"><title>KZ Client</title></head>\
         <body style=\"margin:0;height:100vh;display:grid;place-items:center;background:#09090b;color:#e4e4e7;font-family:system-ui,sans-serif\">\
         <div style=\"text-align:center;padding:48px;border:1px solid rgba(255,255,255,.1);border-radius:24px;background:rgba(255,255,255,.03)\">\
         <h1 style=\"color:{color};margin:0 0 12px\">{}</h1><p style=\"margin:0;color:#a1a1aa\">{}</p></div></body></html>",
        html_escape(title),
        html_escape(message)
    )
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = vec![0u8; 16 * 1024];
    let mut len = 0;
    loop {
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf[len..]))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            break;
        }
        len += n;
        if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") || len == buf.len() {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..len]);
    let mut parts = head.lines().next()?.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    parts.next().map(str::to_string)
}

/// Traite une connexion du navigateur. `None` = requête sans intérêt (favicon, preconnect).
async fn handle_redirect(mut stream: TcpStream, expected_state: &str) -> Option<KzResult<String>> {
    let target = read_request_target(&mut stream).await?;
    let params: HashMap<String, String> = Url::parse(&format!("http://localhost{target}"))
        .map(|u| u.query_pairs().into_owned().collect())
        .unwrap_or_default();
    if let Some(error) = params.get("error") {
        let desc = params.get("error_description").cloned().unwrap_or_else(|| error.clone());
        respond(&mut stream, "200 OK", &page(false, "Connexion annulée", &desc)).await;
        return Some(Err(KzError::Auth(format!("Microsoft a refusé la connexion : {desc}"))));
    }
    let Some(code) = params.get("code") else {
        respond(&mut stream, "404 Not Found", "").await;
        return None;
    };
    if params.get("state").map(String::as_str) != Some(expected_state) {
        respond(&mut stream, "400 Bad Request", &page(false, "Requête refusée", "Paramètre state invalide.")).await;
        return Some(Err(KzError::Auth("Paramètre state invalide (tentative CSRF ?)".into())));
    }
    respond(
        &mut stream,
        "200 OK",
        &page(true, "Connexion réussie", "Tu peux fermer cet onglet et revenir sur KZ Client."),
    )
    .await;
    Some(Ok(code.clone()))
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn wait_for_code(listener: TcpListener, expected_state: String) -> KzResult<String> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<KzResult<String>>(4);
    let _server = AbortOnDrop(tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let tx = tx.clone();
            let state = expected_state.clone();
            tokio::spawn(async move {
                if let Some(result) = handle_redirect(stream, &state).await {
                    let _ = tx.send(result).await;
                }
            });
        }
    }));
    rx.recv()
        .await
        .unwrap_or_else(|| Err(KzError::Auth("Serveur de redirection local interrompu".into())))
}

// ------------------------------------------------------------------ gestionnaire

impl AuthManager {
    pub fn load(paths: Paths, http: reqwest::Client) -> Self {
        let path = paths.accounts_file();
        let file = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<AccountsFile>(&bytes) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("accounts.json illisible ({e}), sauvegardé en .corrupt");
                    let _ = std::fs::rename(&path, with_suffix(&path, ".corrupt"));
                    AccountsFile::default()
                }
            },
            Err(_) => AccountsFile::default(),
        };
        Self {
            http,
            paths,
            inner: Mutex::new(Store { file, sessions: HashMap::new() }),
            refresh_lock: Mutex::new(()),
        }
    }

    pub async fn view(&self) -> AccountsView {
        let store = self.inner.lock().await;
        AccountsView {
            accounts: store.file.accounts.clone(),
            active: store.file.active.clone(),
        }
    }

    pub async fn active(&self) -> Option<String> {
        self.inner.lock().await.file.active.clone()
    }

    pub async fn account_ids(&self) -> Vec<String> {
        self.inner.lock().await.file.accounts.iter().map(|a| a.uuid.clone()).collect()
    }

    async fn save(&self, store: &Store) -> KzResult<()> {
        let bytes = serde_json::to_vec_pretty(&store.file)?;
        write_atomic(&self.paths.accounts_file(), &bytes).await
    }

    /// Connexion interactive : ouvre le navigateur, attend la redirection locale.
    pub async fn login(&self) -> KzResult<Account> {
        let client_id = client_id()?;
        let verifier = b64url(&rand::random::<[u8; 32]>());
        let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
        let state = b64url(&rand::random::<[u8; 16]>());

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let redirect = format!("http://localhost:{port}");

        let mut auth_url = Url::parse(AUTHORIZE_URL).map_err(|e| KzError::Auth(e.to_string()))?;
        auth_url
            .query_pairs_mut()
            .append_pair("client_id", &client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect)
            .append_pair("response_mode", "query")
            .append_pair("scope", SCOPE)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("prompt", "select_account");
        open::that_detached(auth_url.as_str())?;

        let code = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_code(listener, state))
            .await
            .map_err(|_| KzError::Auth("Délai de connexion dépassé (5 minutes)".into()))??;

        let ms = self
            .ms_token(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("code_verifier", verifier.as_str()),
            ])
            .await?;
        let refresh = ms
            .refresh_token
            .clone()
            .ok_or_else(|| KzError::Auth("Microsoft n'a pas renvoyé de refresh token".into()))?;
        let mc = self.minecraft_login(&ms.access_token).await?;
        keyring_set(&mc.account.uuid, refresh).await?;
        let account = mc.account.clone();
        self.store_session(mc, true).await?;
        Ok(account)
    }

    /// Bascule instantanée : le rafraîchissement éventuel se fait en arrière-plan.
    pub async fn switch(&self, uuid: &str) -> KzResult<Account> {
        let mut store = self.inner.lock().await;
        let account = store
            .file
            .accounts
            .iter()
            .find(|a| a.uuid == uuid)
            .cloned()
            .ok_or_else(|| KzError::Auth("Compte inconnu".into()))?;
        store.file.active = Some(uuid.to_string());
        self.save(&store).await?;
        Ok(account)
    }

    pub async fn remove(&self, uuid: &str) -> KzResult<AccountsView> {
        if let Err(e) = keyring_delete(uuid).await {
            eprintln!("Suppression trousseau impossible pour {uuid} : {e}");
        }
        let mut store = self.inner.lock().await;
        store.file.accounts.retain(|a| a.uuid != uuid);
        store.sessions.remove(uuid);
        if store.file.active.as_deref() == Some(uuid) {
            store.file.active = store.file.accounts.first().map(|a| a.uuid.clone());
        }
        self.save(&store).await?;
        Ok(AccountsView {
            accounts: store.file.accounts.clone(),
            active: store.file.active.clone(),
        })
    }

    async fn cached(&self, uuid: &str, min_valid: u64) -> Option<Session> {
        let store = self.inner.lock().await;
        let token = store.sessions.get(uuid)?;
        if token.expires_at <= now() + min_valid {
            return None;
        }
        let account = store.file.accounts.iter().find(|a| a.uuid == uuid)?.clone();
        Some(Session {
            access_token: token.access_token.clone(),
            xuid: token.xuid.clone(),
            account,
        })
    }

    /// Session valide au moins `min_valid` secondes ; rafraîchit silencieusement via le trousseau.
    pub async fn ensure(&self, uuid: &str, min_valid: u64) -> KzResult<Session> {
        if let Some(s) = self.cached(uuid, min_valid).await {
            return Ok(s);
        }
        let _guard = self.refresh_lock.lock().await;
        if let Some(s) = self.cached(uuid, min_valid).await {
            return Ok(s);
        }
        let refresh = keyring_get(uuid)
            .await?
            .ok_or_else(|| KzError::Auth("Session expirée pour ce compte : reconnecte-le".into()))?;
        let ms = self
            .ms_token(&[("grant_type", "refresh_token"), ("refresh_token", refresh.as_str())])
            .await?;
        let mc = self.minecraft_login(&ms.access_token).await?;
        if mc.account.uuid != uuid {
            return Err(KzError::Auth("Le jeton rafraîchi appartient à un autre profil".into()));
        }
        if let Some(rotated) = ms.refresh_token {
            keyring_set(uuid, rotated).await?;
        }
        let session = Session {
            access_token: mc.access_token.clone(),
            xuid: mc.xuid.clone(),
            account: mc.account.clone(),
        };
        self.store_session(mc, false).await?;
        Ok(session)
    }

    async fn store_session(&self, mc: McLogin, make_active: bool) -> KzResult<()> {
        let mut store = self.inner.lock().await;
        let uuid = mc.account.uuid.clone();
        store.sessions.insert(
            uuid.clone(),
            CachedToken {
                access_token: mc.access_token,
                xuid: mc.xuid,
                expires_at: mc.expires_at,
            },
        );
        match store.file.accounts.iter_mut().find(|a| a.uuid == uuid) {
            Some(existing) => *existing = mc.account,
            None => store.file.accounts.push(mc.account),
        }
        if make_active || store.file.active.is_none() {
            store.file.active = Some(uuid);
        }
        self.save(&store).await
    }

    async fn ms_token(&self, extra: &[(&str, &str)]) -> KzResult<MsToken> {
        let client_id = client_id()?;
        let mut form: Vec<(&str, &str)> = vec![("client_id", client_id.as_str()), ("scope", SCOPE)];
        form.extend_from_slice(extra);
        let resp = self.http.post(TOKEN_URL).form(&form).send().await?;
        parse(resp, "Microsoft OAuth").await
    }

    async fn minecraft_login(&self, ms_access_token: &str) -> KzResult<McLogin> {
        // 1. Xbox Live
        let resp = self
            .http
            .post(XBL_URL)
            .header("Accept", "application/json")
            .json(&json!({
                "Properties": {
                    "AuthMethod": "RPS",
                    "SiteName": "user.auth.xboxlive.com",
                    "RpsTicket": format!("d={ms_access_token}")
                },
                "RelyingParty": "http://auth.xboxlive.com",
                "TokenType": "JWT"
            }))
            .send()
            .await?;
        let xbl: XboxAuth = parse(resp, "Xbox Live").await?;
        let uhs = xbl
            .display_claims
            .xui
            .first()
            .map(|x| x.uhs.clone())
            .ok_or_else(|| KzError::Auth("Réponse Xbox Live sans user hash".into()))?;

        // 2. XSTS (relying party Minecraft)
        let resp = self
            .http
            .post(XSTS_URL)
            .header("Accept", "application/json")
            .json(&json!({
                "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl.token] },
                "RelyingParty": "rp://api.minecraftservices.com/",
                "TokenType": "JWT"
            }))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let err: XstsError = resp.json().await.unwrap_or_default();
            return Err(KzError::Auth(xsts_message(err.x_err)));
        }
        let xsts: XboxAuth = parse(resp, "XSTS").await?;

        // 3. Minecraft
        let resp = self
            .http
            .post(MC_LOGIN_URL)
            .json(&json!({ "identityToken": format!("XBL3.0 x={uhs};{}", xsts.token) }))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(KzError::Auth(
                "Client ID Azure non autorisé pour l'API Minecraft : demande l'accès via https://aka.ms/mce-reviewappid"
                    .into(),
            ));
        }
        let mc: McToken = parse(resp, "Minecraft").await?;

        // 4. Possession du jeu
        let resp = self.http.get(MC_ENTITLEMENTS_URL).bearer_auth(&mc.access_token).send().await?;
        let entitlements: Entitlements = parse(resp, "Entitlements").await?;
        let owns = entitlements
            .items
            .iter()
            .any(|i| i.name == "game_minecraft" || i.name == "product_minecraft");

        // 5. Profil (les comptes Game Pass ont un profil même sans entitlement mcstore)
        let resp = self.http.get(MC_PROFILE_URL).bearer_auth(&mc.access_token).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(if owns {
                KzError::Auth("Minecraft possédé mais aucun pseudo créé : choisis-le sur minecraft.net".into())
            } else {
                KzError::NotOwned
            });
        }
        let profile: Profile = parse(resp, "Profil Minecraft").await?;

        let skin = profile.skins.iter().find(|s| s.state == "ACTIVE").or(profile.skins.first());
        let cape = profile.capes.iter().find(|c| c.state == "ACTIVE");
        let account = Account {
            uuid: profile.id,
            username: profile.name,
            skin_url: skin.map(|s| s.url.clone()),
            skin_slim: skin
                .and_then(|s| s.variant.as_deref())
                .map(|v| v.eq_ignore_ascii_case("slim"))
                .unwrap_or(false),
            cape_url: cape.map(|c| c.url.clone()),
        };
        Ok(McLogin {
            xuid: jwt_claim(&mc.access_token, "xuid").unwrap_or_else(|| "0".into()),
            expires_at: now() + mc.expires_in,
            access_token: mc.access_token,
            account,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_claim_reads_payload() {
        let payload = b64url(br#"{"xuid":"2535400000000000","exp":1}"#);
        let token = format!("eyJhbGciOiJIUzI1NiJ9.{payload}.sig");
        assert_eq!(jwt_claim(&token, "xuid").as_deref(), Some("2535400000000000"));
        assert_eq!(jwt_claim("garbage", "xuid"), None);
    }

    #[test]
    fn html_is_escaped() {
        assert_eq!(html_escape("<script>&\""), "&lt;script&gt;&amp;&quot;");
    }
}
