//! Google Calendar synchronization for the user's chosen calendar.
//! Calls are blocking and are intended to run on a worker thread.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{
    DateTime, Duration as ChronoDuration, FixedOffset, NaiveDate, NaiveTime, SecondsFormat,
    Timelike,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;

use crate::model::{Date, Event};
use crate::store::Store;

const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/calendar.readonly";
const API: &str = "https://www.googleapis.com/calendar/v3";
const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";

#[derive(Serialize, Deserialize)]
pub struct GoogleConfig {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default = "primary_calendar")]
    pub calendar_id: String,
}

fn primary_calendar() -> String {
    "primary".to_owned()
}

impl Default for GoogleConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: None,
            calendar_id: primary_calendar(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    client_id: String,
    #[serde(default)]
    account_id: String,
}

#[derive(Default, Serialize, Deserialize)]
struct SyncState {
    #[serde(default)]
    namespace: String,
    #[serde(default)]
    next_sync_token: Option<String>,
    #[serde(default)]
    links: HashMap<String, Link>,
    #[serde(default)]
    retired_remote_ids: HashSet<String>,
    #[serde(default)]
    palette: HashMap<String, String>,
    #[serde(default)]
    palette_fetched_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Link {
    remote_id: String,
    etag: String,
    local_fingerprint: String,
    remote_fingerprint: String,
}

#[derive(Default)]
struct Totals {
    imported: usize,
    uploaded: usize,
    updated: usize,
    deleted: usize,
    conflicts: usize,
    skipped: usize,
}

pub fn load_config() -> Result<GoogleConfig, String> {
    let path = data_dir()?.join("google-config.json");
    if !path.exists() {
        return Ok(GoogleConfig::default());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Google settings are invalid: {e}"))
}

pub fn save_config(config: &GoogleConfig) -> Result<(), String> {
    validate_config(config)?;
    let path = data_dir()?.join("google-config.json");
    atomic_write(
        &path,
        &serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?,
    )
}

pub fn is_connected() -> bool {
    match (load_config(), load_tokens()) {
        (Ok(config), Ok(tokens)) => {
            !config.client_id.is_empty()
                && config.client_id == tokens.client_id
                && !tokens.account_id.is_empty()
        }
        _ => false,
    }
}

/// Forget local credentials. Mappings remain for a safe reconnect to the same account.
/// This does not revoke consent at Google.
pub fn forget_connection() -> Result<String, String> {
    match fs::remove_file(data_dir()?.join("google-tokens.dpapi")) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(format!("Cannot disconnect Google Calendar: {error}")),
    }
    Ok("Google Calendar disconnected on this PC. Google account consent was not revoked.".into())
}

/// Open the system browser, wait for an installed-app loopback OAuth callback,
/// and store the access and refresh tokens using Windows DPAPI for this user.
pub fn connect(config: &GoogleConfig) -> Result<String, String> {
    validate_config(config)?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Cannot listen for Google sign-in: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/oauth/callback");
    let verifier = random_url_token(48)?;
    let state = random_url_token(24)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let auth_url = format!(
        "{AUTH}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&code_challenge={}&code_challenge_method=S256&state={}",
        percent_encode(&config.client_id), percent_encode(&redirect), percent_encode(SCOPE),
        percent_encode(&challenge), percent_encode(&state)
    );
    open_browser(&auth_url)?;

    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let code = loop {
        if std::time::Instant::now() >= deadline {
            return Err("Google sign-in timed out after 3 minutes".into());
        }
        match listener.accept() {
            Ok((mut stream, address)) => {
                if !address.ip().is_loopback() {
                    continue;
                }
                stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
                stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
                let mut buf = [0u8; 8192];
                let mut size = 0;
                while size < buf.len() && !buf[..size].contains(&b'\n') {
                    let read = stream
                        .read(&mut buf[size..])
                        .map_err(|e| format!("OAuth callback failed: {e}"))?;
                    if read == 0 {
                        break;
                    }
                    size += read;
                }
                let request = std::str::from_utf8(&buf[..size])
                    .map_err(|_| "OAuth callback was not UTF-8")?;
                let Some(first_line) = request.lines().next() else {
                    continue;
                };
                let Some(path) = first_line
                    .strip_prefix("GET ")
                    .and_then(|s| s.split_once(' ').map(|p| p.0))
                else {
                    continue;
                };
                let Some(query) = path.strip_prefix("/oauth/callback?") else {
                    continue;
                };
                let args = parse_query(query)?;
                if args.get("state") != Some(&state) {
                    let _ = browser_response(&mut stream, false);
                    continue;
                }
                if let Some(error) = args.get("error") {
                    return Err(format!("Google sign-in was denied: {error}"));
                }
                let code = args
                    .get("code")
                    .ok_or("Google sign-in returned no code")?
                    .clone();
                browser_response(&mut stream, true).ok();
                break code;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(format!("Google sign-in listener failed: {e}")),
        }
    };

    let agent = agent();
    let mut fields = vec![
        ("client_id", config.client_id.as_str()),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect.as_str()),
    ];
    if let Some(secret) = &config.client_secret {
        fields.push(("client_secret", secret.as_str()));
    }
    let response: Value = agent
        .post(TOKEN)
        .send_form(&fields)
        .map_err(http_error)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let access_token = json_str(&response, "access_token")?.to_owned();
    let refresh_token = json_str(&response, "refresh_token")?.to_owned();
    let expires_in = response["expires_in"].as_u64().unwrap_or(3600);
    let account_id = canonical_calendar_id(&agent, &access_token, "primary")?;
    save_tokens(&Tokens {
        access_token,
        refresh_token,
        expires_at: now_epoch().saturating_add(expires_in),
        client_id: config.client_id.clone(),
        account_id,
    })?;
    Ok("Google Calendar account connected. Run Sync to exchange calendar events.".into())
}

fn validate_config(config: &GoogleConfig) -> Result<(), String> {
    if config.client_id.trim().is_empty() || config.client_id.len() > 512 {
        return Err("Enter a Google Desktop OAuth client ID".into());
    }
    if config.calendar_id.trim().is_empty() || config.calendar_id.len() > 1024 {
        return Err("Enter a Google calendar ID or primary".into());
    }
    if config.client_id.chars().any(char::is_control)
        || config.calendar_id.chars().any(char::is_control)
    {
        return Err("Google settings contain control characters".into());
    }
    Ok(())
}

fn data_dir() -> Result<PathBuf, String> {
    let path = match std::env::var_os("DOT_CALENDAR_DATA_DIR") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?)
            .join("DotCalendar"),
    };
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    fs::canonicalize(path).map_err(|e| e.to_string())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", random_url_token(12)?));
    {
        let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    let source = wide(tmp.as_os_str());
    let target = wide(path.as_os_str());
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        let _ = fs::remove_file(&tmp);
        return Err(format!(
            "Cannot save Google data: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_url_token(bytes: usize) -> Result<String, String> {
    let mut random = vec![0u8; bytes];
    getrandom::getrandom(&mut random)
        .map_err(|e| format!("Cannot generate OAuth randomness: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

fn open_browser(url: &str) -> Result<(), String> {
    let operation = wide(std::ffi::OsStr::new("open"));
    let url = wide(std::ffi::OsStr::new(url));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if result as usize <= 32 {
        return Err(format!(
            "Cannot open browser (ShellExecute code {})",
            result as usize
        ));
    }
    Ok(())
}

fn browser_response(stream: &mut std::net::TcpStream, success: bool) -> std::io::Result<()> {
    let body = if success {
        "Google Calendar 연결을 완료했습니다. 이 창을 닫고 달력으로 돌아가세요."
    } else {
        "인증 상태가 일치하지 않습니다. 달력으로 돌아가 다시 시도하세요."
    };
    let html =
        format!("<!doctype html><meta charset=utf-8><title>Dot Calendar</title><p>{body}</p>");
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}", html.len(), html)
}

fn parse_query(query: &str) -> Result<HashMap<String, String>, String> {
    query
        .split('&')
        .map(|part| {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            Ok((percent_decode(k)?, percent_decode(v)?))
        })
        .collect()
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(value: &str) -> Result<String, String> {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("Malformed OAuth callback encoding".into());
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|e| e.to_string())?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "Malformed OAuth callback encoding")?);
            i += 3;
        } else {
            out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

fn dpapi(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(format!(
            "Windows protected token storage failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut core::ffi::c_void) };
    Ok(bytes)
}

fn save_tokens(tokens: &Tokens) -> Result<(), String> {
    let bytes = serde_json::to_vec(tokens).map_err(|e| e.to_string())?;
    atomic_write(
        &data_dir()?.join("google-tokens.dpapi"),
        &dpapi(&bytes, true)?,
    )
}

fn load_tokens() -> Result<Tokens, String> {
    let bytes = fs::read(data_dir()?.join("google-tokens.dpapi"))
        .map_err(|_| "Google Calendar is not connected; use Connect in settings".to_owned())?;
    serde_json::from_slice(&dpapi(&bytes, false)?)
        .map_err(|e| format!("Protected Google token is invalid: {e}"))
}

fn valid_tokens(agent: &ureq::Agent, config: &GoogleConfig) -> Result<Tokens, String> {
    let mut tokens = load_tokens()?;
    if tokens.client_id != config.client_id {
        return Err("Google OAuth client ID changed; connect again".into());
    }
    if tokens.expires_at > now_epoch().saturating_add(60) {
        return Ok(tokens);
    }
    let mut fields = vec![
        ("client_id", config.client_id.as_str()),
        ("refresh_token", tokens.refresh_token.as_str()),
        ("grant_type", "refresh_token"),
    ];
    if let Some(secret) = &config.client_secret {
        fields.push(("client_secret", secret.as_str()));
    }
    let response: Value = agent
        .post(TOKEN)
        .send_form(&fields)
        .map_err(http_error)?
        .into_json()
        .map_err(|e| e.to_string())?;
    tokens.access_token = json_str(&response, "access_token")?.to_owned();
    if let Some(refresh) = response["refresh_token"].as_str() {
        tokens.refresh_token = refresh.to_owned();
    }
    tokens.expires_at = now_epoch().saturating_add(response["expires_in"].as_u64().unwrap_or(3600));
    save_tokens(&tokens)?;
    Ok(tokens)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(20))
        .build()
}

fn http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            format!("Google API HTTP {code}: {}", response.status_text())
        }
        ureq::Error::Transport(error) => format!("Google API connection failed: {error}"),
    }
}

fn json_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Google response missing {key}"))
}

fn read_state(path: &Path) -> Result<SyncState, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SyncState::default());
        }
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("Google sync state is invalid; no changes were made: {e}"))
}

fn canonical_calendar_id(
    agent: &ureq::Agent,
    token: &str,
    calendar_id: &str,
) -> Result<String, String> {
    let url = format!(
        "{API}/users/me/calendarList/{}",
        percent_encode(calendar_id)
    );
    let response: Value = agent
        .get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(http_error)?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(json_str(&response, "id")?.to_owned())
}

fn sync_namespace(account_id: &str, canonical_calendar_id: &str) -> String {
    format!("{account_id}\n{canonical_calendar_id}")
}

fn sync_state_path(namespace: &str) -> Result<PathBuf, String> {
    Ok(data_dir()?.join(format!("google-sync-{}.json", hex_hash(namespace))))
}

fn imported_request_id(namespace: &str, remote_id: &str) -> String {
    format!("google-{}", hex_hash(&format!("{namespace}\n{remote_id}")))
}

/// Google-origin rows keep this deterministic request ID through local edits.
/// Checking it in memory keeps the widget visibility toggle free of disk I/O.
pub fn is_imported_event(event: &Event) -> bool {
    event
        .request_id
        .as_deref()
        .is_some_and(is_imported_request_id)
}

fn is_imported_request_id(request_id: &str) -> bool {
    request_id.strip_prefix("google-").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn events_url(config: &GoogleConfig) -> String {
    format!(
        "{API}/calendars/{}/events",
        percent_encode(&config.calendar_id)
    )
}

fn event_url(config: &GoogleConfig, event_id: &str) -> String {
    format!("{}/{}", events_url(config), percent_encode(event_id))
}

fn get_remote(
    agent: &ureq::Agent,
    token: &str,
    config: &GoogleConfig,
    remote_id: &str,
) -> Result<Option<Value>, String> {
    match agent
        .get(&event_url(config, remote_id))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
    {
        Ok(response) => response.into_json().map(Some).map_err(|e| e.to_string()),
        Err(ureq::Error::Status(404 | 410, _)) => Ok(None),
        Err(error) => Err(http_error(error)),
    }
}

fn list_changes(
    agent: &ureq::Agent,
    token: &str,
    config: &GoogleConfig,
    sync_token: Option<&str>,
) -> Result<(HashMap<String, Value>, String, bool), String> {
    match list_pages(agent, token, config, sync_token) {
        Err(e) if sync_token.is_some() && e.starts_with("Google API HTTP 410:") => {
            list_pages(agent, token, config, None).map(|(events, token)| (events, token, true))
        }
        other => other.map(|(events, token)| (events, token, sync_token.is_none())),
    }
}

fn list_pages(
    agent: &ureq::Agent,
    token: &str,
    config: &GoogleConfig,
    sync_token: Option<&str>,
) -> Result<(HashMap<String, Value>, String), String> {
    let mut events = HashMap::new();
    let mut page: Option<String> = None;
    for _ in 0..100 {
        let mut url = format!("{}?showDeleted=true&maxResults=2500", events_url(config));
        if let Some(sync_token) = sync_token {
            url.push_str("&syncToken=");
            url.push_str(&percent_encode(sync_token));
        }
        if let Some(page) = &page {
            url.push_str("&pageToken=");
            url.push_str(&percent_encode(page));
        }
        let mut result: Value = agent
            .get(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .call()
            .map_err(http_error)?
            .into_json()
            .map_err(|e| e.to_string())?;
        merge_page_items(&mut events, &mut result)?;
        if let Some(next) = result["nextPageToken"].as_str() {
            page = Some(next.to_owned());
        } else {
            let next = json_str(&result, "nextSyncToken")?.to_owned();
            return Ok((events, next));
        }
    }
    Err("Google event list exceeded 100 pages; sync state was not advanced".into())
}

fn merge_page_items(events: &mut HashMap<String, Value>, result: &mut Value) -> Result<(), String> {
    match result.get_mut("items") {
        Some(Value::Array(items)) => {
            for item in items.drain(..) {
                if let Some(id) = item["id"].as_str().map(str::to_owned) {
                    events.insert(id, item);
                }
            }
        }
        Some(Value::Null) | None => {}
        _ => return Err("Google event list contains an invalid items field".into()),
    }
    Ok(())
}

fn get_palette(agent: &ureq::Agent, token: &str) -> Result<HashMap<String, String>, String> {
    let result: Value = agent
        .get(&format!("{API}/colors"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(http_error)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let mut colors = HashMap::new();
    if let Some(palette) = result["event"].as_object() {
        for (id, value) in palette {
            if let Some(hex) = value["background"].as_str() {
                colors.insert(id.clone(), hex.to_owned());
            }
        }
    }
    Ok(colors)
}

fn palette_needs_refresh(state: &SyncState, now: u64) -> bool {
    state.palette.is_empty() || now.saturating_sub(state.palette_fetched_at) >= 24 * 60 * 60
}

fn insert_remote(
    agent: &ureq::Agent,
    token: &str,
    config: &GoogleConfig,
    remote_id: &str,
    event: &Event,
    palette: &HashMap<String, String>,
) -> Result<Value, String> {
    let mut body = export_event(event, palette)?;
    body["id"] = json!(remote_id);
    let result = agent
        .post(&events_url(config))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(body);
    match result {
        Ok(response) => response.into_json().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(409, _)) => {
            // The previous insert may have succeeded before we could save state.
            let existing: Value = agent
                .get(&event_url(config, remote_id))
                .set("Authorization", &format!("Bearer {token}"))
                .call()
                .map_err(http_error)?
                .into_json()
                .map_err(|e| e.to_string())?;
            if existing["extendedProperties"]["private"]["dotCalendarLocalId"].as_str()
                == Some(&event.id)
            {
                Ok(existing)
            } else {
                Err(format!(
                    "Google event id collision for local event {}",
                    event.id
                ))
            }
        }
        Err(error) => Err(http_error(error)),
    }
}

fn patch_remote(
    agent: &ureq::Agent,
    token: &str,
    config: &GoogleConfig,
    remote_id: &str,
    etag: &str,
    event: &Event,
    palette: &HashMap<String, String>,
) -> Result<Value, String> {
    let current = get_remote(agent, token, config, remote_id)?
        .ok_or("Google event missing (precondition failed)")?;
    if current["etag"].as_str() != Some(etag) {
        return Err("Google event changed (precondition failed)".into());
    }
    if has_guests(&current) {
        return Err(
            "Google event has guests; automatic update skipped (precondition failed)".into(),
        );
    }
    let mut body = export_event(event, palette)?;
    preserve_remote_interval(&mut body, &current, event)?;
    if event.color.is_none() {
        body["colorId"] = Value::Null;
    }
    match agent
        .patch(&event_url(config, remote_id))
        .set("Authorization", &format!("Bearer {token}"))
        .set("If-Match", etag)
        .send_json(body)
    {
        Ok(response) => response.into_json().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(412, _)) => {
            Err("Google event changed concurrently (precondition failed)".into())
        }
        Err(error) => Err(http_error(error)),
    }
}

fn delete_remote(
    agent: &ureq::Agent,
    token: &str,
    config: &GoogleConfig,
    remote_id: &str,
    etag: &str,
) -> Result<(), String> {
    let Some(current) = get_remote(agent, token, config, remote_id)? else {
        return Ok(());
    };
    if current["etag"].as_str() != Some(etag) {
        return Err("Google event changed (precondition failed)".into());
    }
    if current["status"] == "cancelled" {
        return Ok(());
    }
    if import_event(&current, &HashMap::new())
        .ok()
        .flatten()
        .is_none()
    {
        return Err("Google event is now outside supported sync scope; automatic deletion skipped (precondition failed)".into());
    }
    match agent
        .delete(&event_url(config, remote_id))
        .set("Authorization", &format!("Bearer {token}"))
        .set("If-Match", etag)
        .call()
    {
        Ok(_) | Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(410, _)) => Ok(()),
        Err(ureq::Error::Status(412, _)) => {
            Err("Google event changed concurrently (precondition failed)".into())
        }
        Err(error) => Err(http_error(error)),
    }
}

fn fingerprint(event: &Event) -> String {
    let value = json!({
        "date": event.date, "time": event.time, "title": event.title, "notes": event.notes,
        "completed": event.completed, "color": event.color, "recurrence": event.recurrence,
        "reminder_minutes": event.reminder_minutes
    });
    hex_hash(&value.to_string())
}

fn changes_conflict(
    local_changed: bool,
    remote_changed: bool,
    local_fp: &str,
    remote_fp: &str,
) -> bool {
    local_changed && remote_changed && local_fp != remote_fp
}

fn can_advance_etag(
    local_changed: bool,
    remote_changed: bool,
    local_fp: &str,
    remote_fp: &str,
) -> bool {
    (!local_changed && !remote_changed) || local_fp == remote_fp
}

fn hex_hash(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(value.as_bytes());
    let mut result = String::with_capacity(digest.len() * 2);
    for byte in digest {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn replacement(existing: &Event, incoming: &Event) -> Event {
    let mut value = incoming.clone();
    value.id = existing.id.clone();
    value.request_id = existing.request_id.clone();
    value
}

fn has_guests(remote: &Value) -> bool {
    remote["attendees"]
        .as_array()
        .is_some_and(|people| !people.is_empty())
        || remote["organizer"]["self"].as_bool() == Some(false)
}

fn supported_interval(remote: &Value) -> bool {
    if let (Some(start), Some(end)) = (
        remote["start"]["date"].as_str(),
        remote["end"]["date"].as_str(),
    ) {
        return next_date(start).ok().as_deref() == Some(end);
    }
    if let (Some(start), Some(end)) = (
        remote["start"]["dateTime"].as_str(),
        remote["end"]["dateTime"].as_str(),
    ) {
        if let (Ok(start), Ok(end)) = (
            DateTime::parse_from_rfc3339(start),
            DateTime::parse_from_rfc3339(end),
        ) {
            let duration = end.signed_duration_since(start);
            return duration > ChronoDuration::zero() && duration < ChronoDuration::days(1);
        }
    }
    false
}

/// Keep Google's precise start, end, and duration when local edits only
/// change title/details. A local date/time move shifts both ends together.
fn preserve_remote_interval(body: &mut Value, remote: &Value, local: &Event) -> Result<(), String> {
    if !supported_interval(remote) {
        return Err("Unsupported Google event duration (precondition failed)".into());
    }
    if let (Some(old_date), None) = (remote["start"]["date"].as_str(), local.time.as_deref()) {
        if old_date == local.date {
            body["start"] = remote["start"].clone();
            body["end"] = remote["end"].clone();
        }
        return Ok(());
    }
    let (Some(old_start), Some(old_end), Some(time)) = (
        remote["start"]["dateTime"].as_str(),
        remote["end"]["dateTime"].as_str(),
        local.time.as_deref(),
    ) else {
        return Ok(());
    }; // The user explicitly changed all-day/timed mode.
    let old_start =
        DateTime::parse_from_rfc3339(old_start).map_err(|_| "Unsupported Google start time")?;
    let old_end =
        DateTime::parse_from_rfc3339(old_end).map_err(|_| "Unsupported Google end time")?;
    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    let old_kst = old_start.with_timezone(&kst);
    let old_minute = old_kst
        .date_naive()
        .and_hms_opt(old_kst.hour(), old_kst.minute(), 0)
        .ok_or("Unsupported Google start time")?;
    let desired_date =
        NaiveDate::parse_from_str(&local.date, "%Y-%m-%d").map_err(|_| "Invalid local date")?;
    let desired_time =
        NaiveTime::parse_from_str(time, "%H:%M").map_err(|_| "Invalid local time")?;
    let delta = desired_date
        .and_time(desired_time)
        .signed_duration_since(old_minute);
    if delta == ChronoDuration::zero() {
        body["start"] = remote["start"].clone();
        body["end"] = remote["end"].clone();
        return Ok(());
    }
    if local.recurrence.is_some() && remote["start"]["timeZone"].as_str() != Some("Asia/Seoul") {
        return Err("Moving a recurring Google event from another time zone is unsupported (precondition failed)".into());
    }
    let new_start = old_kst
        .checked_add_signed(delta)
        .ok_or("Google event move is outside date range")?;
    let new_end = old_end
        .with_timezone(&kst)
        .checked_add_signed(delta)
        .ok_or("Google event move is outside date range")?;
    body["start"] = json!({"dateTime": new_start.to_rfc3339_opts(SecondsFormat::AutoSi, false), "timeZone": "Asia/Seoul"});
    body["end"] = json!({"dateTime": new_end.to_rfc3339_opts(SecondsFormat::AutoSi, false), "timeZone": "Asia/Seoul"});
    Ok(())
}

fn import_event(
    remote: &Value,
    palette: &HashMap<String, String>,
) -> Result<Option<Event>, String> {
    if remote["status"] == "cancelled"
        || remote["recurringEventId"].is_string()
        || has_guests(remote)
        || !supported_interval(remote)
    {
        return Ok(None);
    }
    if remote["eventType"].as_str().is_some_and(|v| v != "default") {
        return Ok(None);
    }
    let start = &remote["start"];
    let (date, time) = if let Some(date) = start["date"].as_str() {
        Date::parse(date)?;
        (date.to_owned(), None)
    } else if let Some(date_time) = start["dateTime"].as_str() {
        let date_time =
            DateTime::parse_from_rfc3339(date_time).map_err(|_| "Unsupported Google event time")?;
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let local = date_time.with_timezone(&kst);
        (
            local.format("%Y-%m-%d").to_string(),
            Some(local.format("%H:%M").to_string()),
        )
    } else {
        return Ok(None);
    };
    let recurrence = match remote["recurrence"].as_array() {
        None => None,
        Some(lines) if lines.is_empty() => None,
        Some(lines) if lines.len() == 1 => match lines[0].as_str() {
            Some("RRULE:FREQ=DAILY") => Some("daily".into()),
            Some("RRULE:FREQ=WEEKLY") => Some("weekly".into()),
            Some("RRULE:FREQ=MONTHLY") => Some("monthly".into()),
            Some("RRULE:FREQ=YEARLY") => Some("yearly".into()),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    let title = remote["summary"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("(Untitled)")
        .to_owned();
    let notes = remote["description"].as_str().unwrap_or("").to_owned();
    if title.chars().count() > 200 || notes.chars().count() > 4096 {
        return Ok(None);
    }
    let private = &remote["extendedProperties"]["private"];
    let completed = private["dotCompleted"].as_str() == Some("true");
    let color_id = remote["colorId"].as_str();
    let color = if color_id == private["dotColorId"].as_str() {
        private["dotColor"]
            .as_str()
            .filter(|hex| rgb(hex).is_some())
            .map(str::to_owned)
    } else {
        None
    }
    .or_else(|| color_id.and_then(|id| palette.get(id)).cloned());
    let reminder_minutes = if remote["reminders"]["useDefault"] == false {
        remote["reminders"]["overrides"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["method"] == "popup"))
            .and_then(|item| item["minutes"].as_u64())
            .and_then(|n| u32::try_from(n).ok())
    } else {
        None
    };
    Ok(Some(Event {
        id: String::new(),
        date,
        time,
        title,
        notes,
        request_id: None,
        completed,
        color,
        recurrence,
        reminder_minutes,
    }))
}

fn export_event(event: &Event, palette: &HashMap<String, String>) -> Result<Value, String> {
    if event
        .reminder_minutes
        .is_some_and(|minutes| minutes > 40_320)
    {
        return Err(format!(
            "Event {} has a reminder beyond Google's 4-week limit",
            event.id
        ));
    }
    let start = if let Some(time) = &event.time {
        json!({"dateTime": format!("{}T{}:00+09:00", event.date, time), "timeZone": "Asia/Seoul"})
    } else {
        json!({"date": event.date})
    };
    let end = if let Some(time) = &event.time {
        let (date, time) = add_one_hour(&event.date, time)?;
        json!({"dateTime": format!("{date}T{time}:00+09:00"), "timeZone": "Asia/Seoul"})
    } else {
        json!({"date": next_date(&event.date)?})
    };
    let recurrence = event
        .recurrence
        .as_deref()
        .map(|value| {
            let rule = match value {
                "daily" => "DAILY",
                "weekly" => "WEEKLY",
                "monthly" => "MONTHLY",
                "yearly" => "YEARLY",
                _ => "",
            };
            if rule.is_empty() {
                Vec::<String>::new()
            } else {
                vec![format!("RRULE:FREQ={rule}")]
            }
        })
        .unwrap_or_default();
    let color_id = event
        .color
        .as_deref()
        .and_then(|hex| nearest_color(hex, palette));
    let reminders = if let Some(minutes) = event.reminder_minutes {
        json!({"useDefault": false, "overrides": [{"method": "popup", "minutes": minutes}]})
    } else {
        json!({"useDefault": false, "overrides": []})
    };
    let mut body = json!({
        "summary": event.title, "description": event.notes, "start": start, "end": end,
        "recurrence": recurrence, "reminders": reminders,
        "extendedProperties": {"private": {"dotCalendarLocalId": event.id, "dotCompleted": if event.completed { "true" } else { "false" }}}
    });
    if let Some(id) = color_id {
        body["colorId"] = json!(id.clone());
        if let Some(hex) = &event.color {
            body["extendedProperties"]["private"]["dotColor"] = json!(hex);
            body["extendedProperties"]["private"]["dotColorId"] = json!(id);
        }
    }
    Ok(body)
}

fn next_date(date: &str) -> Result<String, String> {
    let mut date = Date::parse(date)?;
    if date.day < crate::model::days_in_month(date.year, date.month) {
        date.day += 1;
    } else if date.month < 12 {
        date.day = 1;
        date.month += 1;
    } else if date.year < 9999 {
        date.day = 1;
        date.month = 1;
        date.year += 1;
    } else {
        return Err("Date exceeds Google Calendar range".into());
    }
    Ok(date.to_string())
}

fn add_one_hour(date: &str, time: &str) -> Result<(String, String), String> {
    let hour: u32 = time
        .get(..2)
        .ok_or("Invalid time")?
        .parse()
        .map_err(|_| "Invalid time")?;
    let minute: u32 = time
        .get(3..)
        .ok_or("Invalid time")?
        .parse()
        .map_err(|_| "Invalid time")?;
    if time.len() != 5 || hour > 23 || minute > 59 {
        return Err("Invalid time".into());
    }
    if hour < 23 {
        Ok((date.to_owned(), format!("{:02}:{minute:02}", hour + 1)))
    } else {
        Ok((next_date(date)?, format!("00:{minute:02}")))
    }
}

fn nearest_color(hex: &str, palette: &HashMap<String, String>) -> Option<String> {
    let target = rgb(hex)?;
    palette
        .iter()
        .filter_map(|(id, value)| {
            let candidate = rgb(value)?;
            let distance = (0..3)
                .map(|i| (i32::from(target[i]) - i32::from(candidate[i])).pow(2))
                .sum::<i32>();
            Some((distance, id))
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, id)| id.clone())
}

fn rgb(hex: &str) -> Option<[u8; 3]> {
    if hex.len() != 7 || !hex.starts_with('#') {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[1..3], 16).ok()?,
        u8::from_str_radix(&hex[3..5], 16).ok()?,
        u8::from_str_radix(&hex[5..7], 16).ok()?,
    ])
}

/// Exchange local and Google changes. The caller should serialize calls to this
/// function and run it away from the UI thread.
pub fn sync(store: &Store, config: &GoogleConfig) -> Result<String, String> {
    validate_config(config)?;
    let agent = agent();
    let tokens = valid_tokens(&agent, config)?;
    let token = tokens.access_token;
    let account_id = tokens.account_id;
    if account_id.is_empty() {
        return Err("Google account identity is missing; connect again".into());
    }
    let target_id = canonical_calendar_id(&agent, &token, &config.calendar_id)?;
    let namespace = sync_namespace(&account_id, &target_id);
    let state_path = sync_state_path(&namespace)?;
    let mut state = read_state(&state_path)?;
    if state.namespace.is_empty() {
        state.namespace = namespace.clone();
    } else if state.namespace != namespace {
        return Err("Google sync state account/calendar mismatch".into());
    }
    let (remote_changes, new_sync_token, full_listing) =
        list_changes(&agent, &token, config, state.next_sync_token.as_deref())?;
    let now = now_epoch();
    if palette_needs_refresh(&state, now) {
        match get_palette(&agent, &token) {
            Ok(colors) if !colors.is_empty() => {
                state.palette = colors;
                state.palette_fetched_at = now;
            }
            Ok(_) if state.palette.is_empty() => {
                return Err("Google color palette is empty".into());
            }
            Err(error) if state.palette.is_empty() => return Err(error),
            _ => {}
        }
    }
    let palette = &state.palette;
    let local_events = store.list_events(None)?;
    let local_by_id: HashMap<&str, &Event> = local_events
        .iter()
        .map(|event| (event.id.as_str(), event))
        .collect();
    let mut totals = Totals::default();

    // A missing remote entry on incremental sync means unchanged. In a full
    // listing it means deletion, including after an expired sync token.
    let local_ids: Vec<String> = state.links.keys().cloned().collect();
    for local_id in local_ids {
        let Some(link) = state.links.get(&local_id).cloned() else {
            continue;
        };
        let local = local_by_id.get(local_id.as_str()).copied();
        let remote = remote_changes.get(&link.remote_id);
        let remote_deleted = remote
            .map(|r| r["status"] == "cancelled")
            .unwrap_or(full_listing);
        let local_fingerprint = local.map(fingerprint);
        let local_changed = local_fingerprint
            .as_deref()
            .map(|fp| fp != link.local_fingerprint)
            .unwrap_or(true);
        if remote_deleted {
            if let Some(local) = local {
                if local_changed || !store.delete_if_unchanged(local)? {
                    totals.conflicts += 1;
                    continue;
                }
            }
            state.links.remove(&local_id);
            state.retired_remote_ids.insert(link.remote_id);
            totals.deleted += 1;
            continue;
        }
        if local.is_none() {
            if let Some(remote) = remote {
                if remote["status"] != "cancelled" {
                    let current = import_event(remote, palette).ok().flatten();
                    if current.as_ref().map(fingerprint) != Some(link.remote_fingerprint.clone()) {
                        totals.conflicts += 1;
                        continue;
                    }
                }
            }
            let etag = remote
                .and_then(|r| r["etag"].as_str())
                .unwrap_or(&link.etag);
            match delete_remote(&agent, &token, config, &link.remote_id, etag) {
                Ok(()) => {
                    state.links.remove(&local_id);
                    state.retired_remote_ids.insert(link.remote_id);
                    totals.deleted += 1;
                }
                Err(e) if e.contains("precondition") => totals.conflicts += 1,
                Err(e) => return Err(e),
            }
            continue;
        }
        let local = local.unwrap();
        let local_fingerprint = local_fingerprint.unwrap();
        if let Some(remote) = remote {
            let converted = match import_event(remote, palette) {
                Ok(Some(value)) => value,
                Ok(None) | Err(_) => {
                    totals.conflicts += 1;
                    continue;
                }
            };
            let remote_fingerprint = fingerprint(&converted);
            let remote_changed = remote_fingerprint != link.remote_fingerprint;
            if changes_conflict(
                local_changed,
                remote_changed,
                &local_fingerprint,
                &remote_fingerprint,
            ) {
                totals.conflicts += 1;
                continue;
            }
            if remote_changed && !local_changed {
                let replacement = replacement(local, &converted);
                if !store.replace_if_unchanged(local, &replacement)? {
                    totals.conflicts += 1;
                    continue;
                }
                state.links.insert(
                    local_id.clone(),
                    Link {
                        remote_id: link.remote_id.clone(),
                        etag: json_str(remote, "etag")?.to_owned(),
                        local_fingerprint: remote_fingerprint.clone(),
                        remote_fingerprint,
                    },
                );
                totals.updated += 1;
                continue;
            }
            if can_advance_etag(
                local_changed,
                remote_changed,
                &local_fingerprint,
                &remote_fingerprint,
            ) {
                state.links.insert(
                    local_id.clone(),
                    Link {
                        remote_id: link.remote_id.clone(),
                        etag: json_str(remote, "etag")?.to_owned(),
                        local_fingerprint,
                        remote_fingerprint,
                    },
                );
                continue;
            }
        }
        if local_changed {
            let etag = remote
                .and_then(|r| r["etag"].as_str())
                .unwrap_or(&link.etag);
            match patch_remote(
                &agent,
                &token,
                config,
                &link.remote_id,
                etag,
                local,
                palette,
            ) {
                Ok(updated) => {
                    let new_etag = json_str(&updated, "etag")?.to_owned();
                    state.links.insert(
                        local_id.clone(),
                        Link {
                            remote_id: link.remote_id.clone(),
                            etag: new_etag,
                            local_fingerprint: local_fingerprint.clone(),
                            remote_fingerprint: local_fingerprint,
                        },
                    );
                    totals.updated += 1;
                }
                Err(e) if e.contains("precondition") => totals.conflicts += 1,
                Err(e) => return Err(e),
            }
        }
    }

    // Import every new Google event in the change feed. Deterministic request
    // IDs recover from a crash between the local write and sidecar write.
    let mut blocked_local = HashSet::new();
    let mut local_by_request_id: Option<HashMap<&str, &Event>> = None;
    let linked_remote: HashSet<String> = if remote_changes.is_empty() {
        HashSet::new()
    } else {
        state
            .links
            .values()
            .map(|link| link.remote_id.clone())
            .collect()
    };
    for (remote_id, remote) in &remote_changes {
        if linked_remote.contains(remote_id)
            || state.retired_remote_ids.contains(remote_id)
            || remote["status"] == "cancelled"
        {
            continue;
        }
        if let Some(local_id) =
            remote["extendedProperties"]["private"]["dotCalendarLocalId"].as_str()
        {
            if let Some(local) = local_by_id.get(local_id).copied() {
                let local_fingerprint = fingerprint(local);
                if import_event(remote, palette)
                    .ok()
                    .flatten()
                    .as_ref()
                    .map(fingerprint)
                    == Some(local_fingerprint.clone())
                {
                    state.links.insert(
                        local_id.to_owned(),
                        Link {
                            remote_id: remote_id.clone(),
                            etag: json_str(remote, "etag")?.to_owned(),
                            local_fingerprint: local_fingerprint.clone(),
                            remote_fingerprint: local_fingerprint,
                        },
                    );
                } else {
                    blocked_local.insert(local_id.to_owned());
                    totals.conflicts += 1;
                }
                continue;
            }
        }
        let converted = match import_event(remote, palette) {
            Ok(Some(value)) => value,
            _ => {
                totals.skipped += 1;
                continue;
            }
        };
        let request_id = imported_request_id(&namespace, remote_id);
        let existing = local_by_request_id
            .get_or_insert_with(|| {
                local_events
                    .iter()
                    .filter_map(|event| {
                        event
                            .request_id
                            .as_deref()
                            .filter(|id| is_imported_request_id(id))
                            .map(|id| (id, event))
                    })
                    .collect()
            })
            .get(request_id.as_str())
            .copied();
        let remote_fingerprint = fingerprint(&converted);
        let local = if let Some(existing) = existing {
            let replacement = replacement(existing, &converted);
            if !store.replace_if_unchanged(existing, &replacement)? {
                totals.conflicts += 1;
                continue;
            }
            replacement
        } else {
            store.create_event_with_details(
                &converted.date,
                converted.time.as_deref(),
                &converted.title,
                &converted.notes,
                Some(&request_id),
                converted.completed,
                converted.color.as_deref(),
                converted.recurrence.as_deref(),
                converted.reminder_minutes,
            )?
        };
        state.links.insert(
            local.id.clone(),
            Link {
                remote_id: remote_id.clone(),
                etag: json_str(remote, "etag")?.to_owned(),
                local_fingerprint: fingerprint(&local),
                remote_fingerprint,
            },
        );
        totals.imported += 1;
    }

    // Publish local-only events. A deterministic Google id makes retry safe
    // after a timeout or a crash before state persistence.
    for local in local_by_id.values() {
        if state.links.contains_key(&local.id)
            || blocked_local.contains(&local.id)
            || is_imported_event(local)
        {
            continue;
        }
        let remote_id = format!("dotcal{}", hex_hash(&local.id));
        let created = insert_remote(&agent, &token, config, &remote_id, local, palette)?;
        let fp = fingerprint(local);
        if import_event(&created, palette)
            .ok()
            .flatten()
            .as_ref()
            .map(fingerprint)
            != Some(fp.clone())
        {
            totals.conflicts += 1;
            continue;
        }
        state.links.insert(
            local.id.clone(),
            Link {
                remote_id,
                etag: json_str(&created, "etag")?.to_owned(),
                local_fingerprint: fp.clone(),
                remote_fingerprint: fp,
            },
        );
        totals.uploaded += 1;
    }
    // Keep the old token while conflicts remain so the conflicting remote
    // change is reconsidered on the next manual or scheduled sync.
    if totals.conflicts == 0 {
        state.next_sync_token = Some(new_sync_token);
    }
    atomic_write(
        &state_path,
        &serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?,
    )?;
    Ok(format!(
        "Google sync: {} imported, {} uploaded, {} updated, {} deleted, {} conflicts, {} skipped",
        totals.imported,
        totals.uploaded,
        totals.updated,
        totals.deleted,
        totals.conflicts,
        totals.skipped
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> Event {
        Event {
            id: "local-1".into(),
            date: "2026-12-31".into(),
            time: Some("23:30".into()),
            title: "Meeting".into(),
            notes: "Notes".into(),
            request_id: None,
            completed: true,
            color: Some("#ff0000".into()),
            recurrence: Some("weekly".into()),
            reminder_minutes: Some(15),
        }
    }

    #[test]
    fn pkce_and_url_encoding_have_no_reserved_bytes() {
        assert_eq!(percent_encode("a b+c"), "a%20b%2Bc");
        assert_eq!(percent_decode("a%20b%2Bc").unwrap(), "a b+c");
        assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(b"abc")).len(), 43);
    }

    #[test]
    fn paged_events_move_into_result_and_keep_latest_id() {
        let mut events = HashMap::from([("same".into(), json!({"id":"same","summary":"old"}))]);
        let mut page = json!({
            "items": [{"id":"same","summary":"new"}, {"id":"other"}],
            "nextSyncToken": "keep"
        });
        merge_page_items(&mut events, &mut page).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events["same"]["summary"], "new");
        assert_eq!(page["items"], json!([]));
        assert_eq!(page["nextSyncToken"], "keep");
        assert!(merge_page_items(&mut events, &mut json!({"items": 1})).is_err());
    }

    #[test]
    fn event_export_round_trip_and_rollover() {
        let palette = HashMap::from([("11".into(), "#ff0000".into())]);
        let mut remote = export_event(&event(), &palette).unwrap();
        remote["id"] = json!("google-1");
        assert_eq!(remote["end"]["dateTime"], "2027-01-01T00:30:00+09:00");
        assert_eq!(remote["recurrence"][0], "RRULE:FREQ=WEEKLY");
        assert_eq!(remote["colorId"], "11");
        let imported = import_event(&remote, &palette).unwrap().unwrap();
        assert_eq!(fingerprint(&imported), fingerprint(&event()));
    }

    #[test]
    fn complex_google_recurrence_is_preserved_by_skipping() {
        let mut remote = export_event(&event(), &HashMap::new()).unwrap();
        remote["recurrence"] = json!(["RRULE:FREQ=WEEKLY;COUNT=4"]);
        assert!(import_event(&remote, &HashMap::new()).unwrap().is_none());
    }

    #[test]
    fn remote_timezone_is_converted_to_seoul() {
        let remote = json!({
            "start": {"dateTime":"2026-10-07T01:30:00Z"}, "summary":"Call",
            "end": {"dateTime":"2026-10-07T02:15:00Z"},
            "description":"", "status":"confirmed"
        });
        let imported = import_event(&remote, &HashMap::new()).unwrap().unwrap();
        assert_eq!(imported.date, "2026-10-07");
        assert_eq!(imported.time.as_deref(), Some("10:30"));
    }

    #[test]
    fn remote_update_keeps_duration_and_precise_seconds() {
        let remote = json!({
            "start": {"dateTime":"2026-10-07T09:30:45+09:00", "timeZone":"Asia/Seoul"},
            "end": {"dateTime":"2026-10-07T11:15:45+09:00", "timeZone":"Asia/Seoul"},
            "summary":"Original", "status":"confirmed"
        });
        let mut local = event();
        local.date = "2026-10-07".into();
        local.time = Some("09:30".into());
        local.title = "Renamed".into();
        local.recurrence = None;
        let mut body = export_event(&local, &HashMap::new()).unwrap();
        preserve_remote_interval(&mut body, &remote, &local).unwrap();
        assert_eq!(body["start"], remote["start"]);
        assert_eq!(body["end"], remote["end"]);

        local.date = "2026-10-08".into();
        local.time = Some("10:00".into());
        preserve_remote_interval(&mut body, &remote, &local).unwrap();
        assert_eq!(body["start"]["dateTime"], "2026-10-08T10:00:45+09:00");
        assert_eq!(body["end"]["dateTime"], "2026-10-08T11:45:45+09:00");
    }

    #[test]
    fn guest_and_multiday_events_are_not_imported() {
        let mut remote = export_event(&event(), &HashMap::new()).unwrap();
        remote["attendees"] = json!([{"email":"guest@example.com"}]);
        assert!(import_event(&remote, &HashMap::new()).unwrap().is_none());
        remote["attendees"] = json!([]);
        remote["start"] = json!({"date":"2026-10-07"});
        remote["end"] = json!({"date":"2026-10-09"});
        assert!(import_event(&remote, &HashMap::new()).unwrap().is_none());
    }

    #[test]
    fn concurrent_different_edits_are_not_auto_merged() {
        assert!(changes_conflict(true, true, "local", "remote"));
        assert!(!changes_conflict(true, true, "same", "same"));
        assert!(!changes_conflict(true, false, "local", "remote"));
        assert!(!changes_conflict(false, true, "local", "remote"));
    }

    #[test]
    fn unchanged_distinct_fingerprints_can_refresh_etag() {
        // The Store normalizes colors while Google's palette may use lower case.
        assert!(can_advance_etag(false, false, "LOCAL", "remote"));
        assert!(!can_advance_etag(true, false, "LOCAL", "remote"));
        assert!(!can_advance_etag(false, true, "LOCAL", "remote"));
    }

    #[test]
    fn imported_ids_are_scoped_to_account_and_calendar() {
        let first = sync_namespace("alice@example.com", "alice@example.com");
        let other_account = sync_namespace("bob@example.com", "bob@example.com");
        let other_calendar = sync_namespace("alice@example.com", "team@example.com");
        let id = "same-google-event-id";
        assert_eq!(
            imported_request_id(&first, id),
            imported_request_id(&first, id)
        );
        assert_ne!(
            imported_request_id(&first, id),
            imported_request_id(&other_account, id)
        );
        assert_ne!(
            imported_request_id(&first, id),
            imported_request_id(&other_calendar, id)
        );
        assert_ne!(hex_hash(&first), hex_hash(&other_account));
    }

    #[test]
    fn visibility_distinguishes_imports_from_local_events_uploaded_to_google() {
        let mut local = event();
        local.request_id = Some("dot-created-123".into());
        assert!(!is_imported_event(&local));
        local.request_id = None;
        assert!(!is_imported_event(&local));
        local.request_id = Some("google-short-local-id".into());
        assert!(!is_imported_event(&local));

        local.request_id = Some(imported_request_id("account\ncalendar", "remote-id"));
        assert!(is_imported_event(&local));
        local.title = "Edited in Dot Calendar".into();
        assert!(is_imported_event(&local));
        local.request_id = local.request_id.map(|id| id.to_uppercase());
        assert!(!is_imported_event(&local));
    }

    #[test]
    fn palette_cache_refreshes_only_when_missing_or_stale() {
        let mut state = SyncState::default();
        assert!(palette_needs_refresh(&state, 100));
        state.palette.insert("1".into(), "#000000".into());
        state.palette_fetched_at = 100;
        assert!(!palette_needs_refresh(&state, 100 + 24 * 60 * 60 - 1));
        assert!(palette_needs_refresh(&state, 100 + 24 * 60 * 60));
        assert!(!palette_needs_refresh(&state, 99));
    }
}
