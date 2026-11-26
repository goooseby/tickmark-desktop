use reqwest::{Client, StatusCode, Url, RequestBuilder};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::account_vault;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountError {
    code: &'static str,
    retry_after_seconds: Option<u32>,
}

impl AccountError {
    fn new(code: &'static str) -> Self {
        Self { code, retry_after_seconds: None }
    }

    fn storage(_: std::io::Error) -> Self {
        Self::new("local_storage")
    }

    fn from_response(status: StatusCode, retry_after_seconds: Option<u32>, body: &[u8]) -> Self {
        #[derive(Deserialize)]
        struct Problem { code: String }
        let problem = serde_json::from_slice::<Problem>(body).ok();
        let code = match (status.as_u16(), problem.as_ref().map(|p| p.code.as_str())) {
            (429, _) => "rate_limited",
            (400 | 401 | 422, Some("invalid_code")) => "invalid_code",
            (400 | 410 | 422, Some("challenge_expired")) => "challenge_expired",
            (400 | 410 | 422, Some("challenge_exhausted")) => "challenge_exhausted",
            (401 | 403, Some("session_revoked" | "invalid_refresh" | "device_revoked")) => "session_revoked",
            (500..=599, _) => "unavailable",
            _ => "unexpected_response",
        };
        Self { code, retry_after_seconds }
    }
}

type AccountResult<T> = Result<T, AccountError>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    id: String,
    email: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SavedSession {
    account: Identity,
    refresh_token: String,
    refresh_request: Option<Uuid>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Vault {
    installation_id: Uuid,
    session: Option<SavedSession>,
    pending_logout: Option<String>,
}

impl Default for Vault {
    fn default() -> Self {
        Self { installation_id: Uuid::new_v4(), session: None, pending_logout: None }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    configured: bool,
    account: Option<Identity>,
    offline: bool,
    logout_pending: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
    challenge_id: String,
    expires_in_seconds: u32,
    retry_after_seconds: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionResponse {
    account: Identity,
    access_token: String,
    access_expires_in_seconds: u32,
    refresh_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
    request_id: Uuid,
}

pub struct AccountService {
    url: Option<Url>,
    client: Client,
    path: PathBuf,
    runtime: Mutex<Runtime>,
}

pub struct SyncSession {
    pub device_id: Uuid,
    pub cache: PathBuf,
    token: String,
}

#[derive(Default)]
struct Runtime {
    access: Option<(String, Instant)>,
    next_network_attempt: Option<Instant>,
}

impl AccountService {
    pub fn new(dir: PathBuf) -> AccountResult<Self> {
        let url = option_env!("TICKMARK_ACCOUNT_API").unwrap_or("https://kovra.top/tickmark");
        #[cfg(debug_assertions)]
        let test_endpoint = std::env::var("TICKMARK_ACCOUNT_API_TEST").ok();
        #[cfg(debug_assertions)]
        let url = test_endpoint.as_deref().unwrap_or(url);
        Self::with_endpoint(dir, Some(url).filter(|url| !url.is_empty()), cfg!(debug_assertions))
    }

    fn with_endpoint(dir: PathBuf, url: Option<&str>, allow_loopback_http: bool) -> AccountResult<Self> {
        let url = url.map(|value| {
            let url = Url::parse(value).map_err(|_| AccountError::new("invalid_configuration"))?;
            let allowed_scheme = url.scheme() == "https" ||
                (allow_loopback_http && url.scheme() == "http" &&
                    matches!(url.host_str(), Some("127.0.0.1" | "[::1]")));
            if !allowed_scheme || url.host().is_none() || url.query().is_some() ||
                !url.path().trim_matches('/').split('/').all(|part| part.is_empty() || valid_id(part)) ||
                url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
                return Err(AccountError::new("invalid_configuration"));
            }
            Ok(url)
        }).transpose()?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .https_only(url.as_ref().is_none_or(|url| url.scheme() == "https"))
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(10))
            .build().map_err(|_| AccountError::new("invalid_configuration"))?;
        let scope = url.as_ref().map_or("unconfigured", Url::as_str);
        let vault_name = format!("{:x}.bin", Sha256::digest(scope.as_bytes()));
        Ok(Self { url, client, path: dir.join("accounts-v1").join(vault_name), runtime: Mutex::default() })
    }

    fn load(&self) -> AccountResult<Vault> {
        Ok(account_vault::read(&self.path).map_err(AccountError::storage)?.unwrap_or_default())
    }

    fn save(&self, vault: &Vault) -> AccountResult<()> {
        account_vault::write(&self.path, vault).map_err(AccountError::storage)
    }

    fn endpoint(&self, path: &str) -> AccountResult<Url> {
        let mut url = self.url.clone().ok_or_else(|| AccountError::new("not_configured"))?;
        url.set_path(&format!("{}{path}", url.path().trim_end_matches('/')));
        Ok(url)
    }

    pub fn cache(&self) -> Option<PathBuf> {
        let vault = self.load().ok()?;
        let session = vault.session?;
        Some(self.cache_for(&session.account.id))
    }

    fn cache_for(&self, account: &str) -> PathBuf {
        self.path.with_extension("").join(format!("{:x}.sqlite3", Sha256::digest(account.as_bytes())))
    }

    pub async fn sync_session(&self) -> AccountResult<Option<SyncSession>> {
        let status = self.status().await?;
        if status.account.is_none() || status.offline { return Ok(None); }
        let runtime = self.runtime.lock().await;
        let vault = self.load()?;
        Ok(vault.session.and_then(|session| runtime.access.as_ref().map(|(token, _)| SyncSession {
            device_id: vault.installation_id,
            cache: self.cache_for(&session.account.id),
            token: token.clone()
        })))
    }

    pub fn changes(&self, session: &SyncSession, after: i64, epoch: Option<&str>) -> AccountResult<RequestBuilder> {
        let mut endpoint = self.endpoint("/v1/changes")?;
        endpoint.query_pairs_mut().append_pair("after", &after.to_string()).append_pair("limit", "8");
        if let Some(epoch) = epoch { endpoint.query_pairs_mut().append_pair("epoch", epoch); }
        Ok(self.client.get(endpoint).bearer_auth(&session.token))
    }

    pub fn upload(&self, session: &SyncSession, body: impl Serialize) -> AccountResult<RequestBuilder> {
        let endpoint = self.endpoint(&format!("/v1/devices/{}/shards", session.device_id))?;
        Ok(self.client.put(endpoint).bearer_auth(&session.token).json(&body))
    }

    pub async fn forget_access(&self, session: &SyncSession) {
        let mut runtime = self.runtime.lock().await;
        if runtime.access.as_ref().is_some_and(|(token, _)| token == &session.token) {
            runtime.access = None;
        }
    }

    async fn post(&self, path: &str, body: impl Serialize) -> AccountResult<Vec<u8>> {
        let mut response = self.client.post(self.endpoint(path)?).json(&body).send().await
            .map_err(|_| AccountError::new("unavailable"))?;
        let status = response.status();
        let retry_after_seconds = response.headers().get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()?.parse::<u32>().ok());
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| AccountError::new("unavailable"))? {
            if chunk.len() > 16_384 - body.len() {
                return Err(AccountError::new("unexpected_response"));
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(AccountError::from_response(status, retry_after_seconds, &body));
        }
        Ok(body)
    }

    async fn json<T: DeserializeOwned>(&self, path: &str, body: impl Serialize) -> AccountResult<T> {
        serde_json::from_slice(&self.post(path, body).await?)
            .map_err(|_| AccountError::new("unexpected_response"))
    }

    fn status_of(&self, vault: &Vault, offline: bool) -> AccountStatus {
        AccountStatus {
            configured: self.url.is_some(),
            account: vault.session.as_ref().map(|session| session.account.clone()),
            offline,
            logout_pending: vault.pending_logout.is_some(),
        }
    }

    pub async fn status(&self) -> AccountResult<AccountStatus> {
        let mut runtime = self.runtime.lock().await;
        let mut vault = self.load()?;
        if self.url.is_none() || runtime.next_network_attempt.is_some_and(|when| when > Instant::now()) {
            return Ok(self.status_of(&vault, vault.session.is_some()));
        }
        if vault.pending_logout.is_some() {
            if let Err(error) = self.finish_logout(&mut vault).await {
                if error.code == "local_storage" { return Err(error); }
                runtime.defer(error.retry_after_seconds);
                return Ok(self.status_of(&vault, false));
            }
        }
        let Some(session) = vault.session.as_mut() else {
            return Ok(self.status_of(&vault, false));
        };
        if runtime.access.as_ref().is_some_and(|(_, until)| *until > Instant::now()) {
            return Ok(self.status_of(&vault, false));
        }
        if session.refresh_request.is_none() {
            session.refresh_request = Some(Uuid::new_v4());
            self.save(&vault)?;
        }
        let session = vault.session.as_ref().unwrap();
        let response: AccountResult<SessionResponse> = self.json("/v1/auth/refresh", RefreshRequest {
            refresh_token: &session.refresh_token,
            request_id: session.refresh_request.unwrap(),
        }).await;
        match response {
            Ok(response) => {
                if response.account.id != session.account.id {
                    return Err(AccountError::new("unexpected_response"));
                }
                self.accept_session(&mut vault, &mut runtime, response)?;
                Ok(self.status_of(&vault, false))
            }
            Err(error) if error.code == "session_revoked" => {
                vault.session = None;
                self.save(&vault)?;
                *runtime = Runtime::default();
                Ok(self.status_of(&vault, false))
            }
            Err(error) => {
                runtime.defer(error.retry_after_seconds);
                Ok(self.status_of(&vault, true))
            }
        }
    }

    pub async fn challenge(&self, email: &str) -> AccountResult<Challenge> {
        let _runtime = self.runtime.lock().await;
        let email = normalize_email(email)?;
        if self.load()?.session.is_some() {
            return Err(AccountError::new("already_signed_in"));
        }
        let challenge: Challenge = self.json("/v1/auth/challenges", serde_json::json!({ "email": email })).await?;
        if !valid_id(&challenge.challenge_id) || !(1..=300).contains(&challenge.expires_in_seconds) ||
            challenge.retry_after_seconds > 900 {
            return Err(AccountError::new("unexpected_response"));
        }
        Ok(challenge)
    }

    pub async fn verify(&self, email: &str, challenge_id: &str, code: &str) -> AccountResult<AccountStatus> {
        let email = normalize_email(email)?;
        if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) || !valid_id(challenge_id) {
            return Err(AccountError::new("invalid_code"));
        }
        let mut runtime = self.runtime.lock().await;
        let mut vault = self.load()?;
        if vault.session.is_some() {
            return Err(AccountError::new("already_signed_in"));
        }
        self.finish_logout(&mut vault).await?;
        self.save(&vault)?;
        let response: SessionResponse = self.json("/v1/auth/verify", serde_json::json!({
            "email": email,
            "challengeId": challenge_id,
            "code": code,
            "device": {
                "id": vault.installation_id,
                "name": std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".to_owned()),
                "platform": "windows",
                "kind": "computer"
            }
        })).await?;
        if normalize_email(&response.account.email).ok().as_deref() != Some(&email) {
            return Err(AccountError::new("unexpected_response"));
        }
        self.accept_session(&mut vault, &mut runtime, response)?;
        Ok(self.status_of(&vault, false))
    }

    fn accept_session(&self, vault: &mut Vault, runtime: &mut Runtime, value: SessionResponse) -> AccountResult<()> {
        if !valid_id(&value.account.id) || normalize_email(&value.account.email).is_err() ||
            !valid_token(&value.access_token) || !valid_token(&value.refresh_token) ||
            !(1..=900).contains(&value.access_expires_in_seconds) {
            return Err(AccountError::new("unexpected_response"));
        }
        vault.session = Some(SavedSession { account: value.account, refresh_token: value.refresh_token, refresh_request: None });
        self.save(vault)?;
        let refresh_after = value.access_expires_in_seconds.saturating_sub(30).max(1);
        runtime.access = Some((value.access_token, Instant::now() + Duration::from_secs(refresh_after.into())));
        runtime.next_network_attempt = None;
        Ok(())
    }

    pub async fn logout(&self) -> AccountResult<AccountStatus> {
        let mut runtime = self.runtime.lock().await;
        let mut vault = self.load()?;
        if let Some(session) = vault.session.take() {
            // Keep the final refresh token until the server confirms revocation.
            vault.pending_logout = Some(session.refresh_token);
            self.save(&vault)?;
        }
        *runtime = Runtime::default();
        if let Err(error) = self.finish_logout(&mut vault).await {
            if error.code == "local_storage" { return Err(error); }
            runtime.defer(error.retry_after_seconds);
        }
        Ok(self.status_of(&vault, false))
    }

    async fn finish_logout(&self, vault: &mut Vault) -> AccountResult<()> {
        if let Some(token) = &vault.pending_logout {
            // A spent refresh token still identifies the session family for revocation.
            match self.post("/v1/auth/logout", serde_json::json!({ "refreshToken": token })).await {
                Ok(_) => (),
                Err(error) if error.code == "session_revoked" => (),
                Err(error) => return Err(error),
            }
            vault.pending_logout = None;
            self.save(vault)?;
        }
        Ok(())
    }
}

impl Runtime {
    fn defer(&mut self, seconds: Option<u32>) {
        self.next_network_attempt = Some(Instant::now() + Duration::from_secs(seconds.unwrap_or(30).clamp(30, 900).into()));
    }
}

fn normalize_email(email: &str) -> AccountResult<String> {
    let email = email.trim();
    let valid = email.len() <= 254 && email.is_ascii() && !email.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control()) &&
        email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && local.len() <= 64 && !local.starts_with('.') && !local.ends_with('.') && !local.contains("..") &&
                local.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&byte)) &&
                domain.contains('.') && domain.split('.').all(|part| {
                    !part.is_empty() && part.len() <= 63 && !part.starts_with('-') && !part.ends_with('-') &&
                        part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                })
        });
    if valid { Ok(email.to_ascii_lowercase()) } else { Err(AccountError::new("invalid_email")) }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

fn valid_token(value: &str) -> bool {
    (16..=4096).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_graphic())
}

#[tauri::command]
pub async fn account_status(service: tauri::State<'_, AccountResult<AccountService>>) -> AccountResult<AccountStatus> {
    service.inner().as_ref().map_err(Clone::clone)?.status().await
}

#[tauri::command]
pub async fn account_challenge(service: tauri::State<'_, AccountResult<AccountService>>, email: String) -> AccountResult<Challenge> {
    service.inner().as_ref().map_err(Clone::clone)?.challenge(&email).await
}

#[tauri::command]
pub async fn account_verify(service: tauri::State<'_, AccountResult<AccountService>>, email: String, challenge_id: String, code: String) -> AccountResult<AccountStatus> {
    service.inner().as_ref().map_err(Clone::clone)?.verify(&email, &challenge_id, &code).await
}

#[tauri::command]
pub async fn account_logout(service: tauri::State<'_, AccountResult<AccountService>>) -> AccountResult<AccountStatus> {
    service.inner().as_ref().map_err(Clone::clone)?.logout().await
}
