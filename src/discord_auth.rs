// SPDX-License-Identifier: MPL-2.0
//! Browser OAuth authorization code + PKCE. The desktop binary contains no client secret.
//! The exact loopback redirect must be registered on the operator's public Discord app.
use crate::{config, discord_http::DiscordHttp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(crate) const REDIRECT_URI: &str = "http://127.0.0.1:43821/callback";
const CALLBACK_ADDRESS: &str = "127.0.0.1:43821";
const SCOPES: &str = "identify rpc messages.read";
static TOKEN_FILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
pub(crate) struct Tokens {
    app_id: String,
    pub access_token: String,
    refresh_token: String,
    pub expires_at: u64,
    scope: String,
}
// Deliberately no Debug implementation for authorization material.
impl Tokens {
    pub fn needs_refresh(&self) -> bool {
        self.expires_at <= now().saturating_add(120)
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err(crate::i18n::text(crate::i18n::Key::M1F2C892921B3))
    } else {
        Ok(())
    }
}

pub(crate) fn load(app_id: &str) -> Result<Option<Tokens>, String> {
    let _guard = TOKEN_FILE_LOCK
        .lock()
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M8E3B09F55E74))?;
    let path = config::path()?.with_file_name("rpc-session.bin");
    if !path.exists() {
        return Ok(None);
    }
    let size = fs::metadata(&path)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M15BF1E5EFE1B))?
        .len();
    if size > 64 * 1024 {
        return Err(crate::i18n::text(crate::i18n::Key::MAB39DF41DFC6));
    }
    let encrypted =
        fs::read(path).map_err(|_| crate::i18n::text(crate::i18n::Key::M15BF1E5EFE1B))?;
    let clear = config::unprotect(&encrypted)?;
    let tokens: Tokens = serde_json::from_str(&clear)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MAB39DF41DFC6))?;
    if tokens.app_id != app_id {
        return Ok(None);
    }
    validate_tokens(&tokens)?;
    Ok(Some(tokens))
}

pub(crate) fn save(tokens: &Tokens, cancel: &AtomicBool) -> Result<(), String> {
    let _guard = TOKEN_FILE_LOCK
        .lock()
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M8E3B09F55E74))?;
    check_cancel(cancel)?;
    let encoded = serde_json::to_string(tokens)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MA2E43607220B))?;
    let encrypted = config::protect(&encoded)?;
    let path = config::path()?.with_file_name("rpc-session.bin");
    fs::create_dir_all(path.parent().unwrap())
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MD7B5525E8509))?;
    let temp = path.with_extension("bin.tmp");
    let result = (|| -> Result<(), String> {
        let mut file = fs::File::create(&temp)
            .map_err(|_| crate::i18n::text(crate::i18n::Key::M5F93A5CF7901))?;
        file.write_all(&encrypted)
            .and_then(|_| file.sync_all())
            .map_err(|_| crate::i18n::text(crate::i18n::Key::M431B37D3546D))?;
        drop(file);
        check_cancel(cancel)?;
        config::replace_file(&temp, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub(crate) fn clear() -> Result<(), String> {
    let _guard = TOKEN_FILE_LOCK
        .lock()
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M8E3B09F55E74))?;
    let path = config::path()?.with_file_name("rpc-session.bin");
    let mut failed = false;
    for file in [&path, &path.with_extension("bin.tmp")] {
        if let Err(e) = fs::remove_file(file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                failed = true;
            }
        }
    }
    if failed {
        Err(crate::i18n::text(crate::i18n::Key::MCF32823D9968))
    } else {
        Ok(())
    }
}

pub(crate) fn authorize(app_id: &str, cancel: &AtomicBool) -> Result<Tokens, String> {
    if !config::snowflake(app_id) {
        return Err(crate::i18n::text(crate::i18n::Key::M9355CAB3AEC1));
    }
    check_cancel(cancel)?;
    let state = base64url(&random_bytes()?);
    let verifier = base64url(&random_bytes()?);
    let challenge = base64url(&sha256(verifier.as_bytes())?);
    // Bind before opening the browser, and never listen on 0.0.0.0 or a LAN address.
    let listener = TcpListener::bind(CALLBACK_ADDRESS)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M73849C5F22E6))?;
    listener
        .set_nonblocking(true)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M308794DA4F71))?;
    let query = form(&[
        ("client_id", app_id),
        ("response_type", "code"),
        ("redirect_uri", REDIRECT_URI),
        ("scope", SCOPES),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("prompt", "consent"),
    ]);
    open_browser(&format!("https://discord.com/oauth2/authorize?{query}"))?;
    let code = wait_callback(&listener, &state, cancel)?;
    check_cancel(cancel)?;
    let payload = form(&[
        ("grant_type", "authorization_code"),
        ("client_id", app_id),
        ("code", &code),
        ("redirect_uri", REDIRECT_URI),
        ("code_verifier", &verifier),
    ]);
    exchange(app_id, payload, None)
}

pub(crate) fn refresh(tokens: &Tokens, cancel: &AtomicBool) -> Result<Tokens, String> {
    check_cancel(cancel)?;
    if tokens.refresh_token.is_empty() {
        return Err(crate::i18n::text(crate::i18n::Key::M9BA045A6BD8B));
    }
    let payload = form(&[
        ("grant_type", "refresh_token"),
        ("client_id", &tokens.app_id),
        ("refresh_token", &tokens.refresh_token),
    ]);
    exchange(&tokens.app_id, payload, Some(tokens))
}

fn exchange(app_id: &str, payload: String, previous: Option<&Tokens>) -> Result<Tokens, String> {
    let mut http = DiscordHttp::new()?;
    let response = http.request(
        "POST",
        "/api/oauth2/token",
        Some("application/x-www-form-urlencoded"),
        payload.as_bytes(),
    )?;
    if !(200..300).contains(&response.status) {
        let stage = if previous.is_some() {
            OAuthStage::Refresh
        } else {
            OAuthStage::Token
        };
        return Err(token_exchange_error(response.status, &response.body, stage));
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MD6D14CA7893F))?;
    parse_token_response(app_id, &value, previous, now())
}

#[derive(Clone, Copy)]
enum OAuthStage {
    Authorize,
    Token,
    Refresh,
}

/// Preserve enough information to diagnose a rejection without copying arbitrary
/// response text, callback URLs, authorization codes, or credentials into the UI.
fn oauth_error_message(stage: OAuthStage, error: Option<&str>, status: Option<u32>) -> String {
    let code = match error {
        Some("invalid_scope") => "invalid_scope",
        Some("unauthorized_client") => "unauthorized_client",
        Some("access_denied") => "access_denied",
        Some("invalid_client") => "invalid_client",
        Some("invalid_grant") => "invalid_grant",
        Some("invalid_request") => "invalid_request",
        Some("unsupported_response_type") => "unsupported_response_type",
        Some("unsupported_grant_type") => "unsupported_grant_type",
        Some("server_error") => "server_error",
        Some("temporarily_unavailable") => "temporarily_unavailable",
        Some(_) => "unrecognized_error",
        None => "missing_error",
    };
    // An invalid scope can also depend on the authorizing user's tester status.
    // Neither it nor unauthorized_client proves a permanent app-wide refusal.
    let key = match code {
        "invalid_scope" => crate::i18n::Key::M2E99DBE33375,
        "unauthorized_client" => crate::i18n::Key::M4FE33C222835,
        "access_denied" => crate::i18n::Key::ME856B7836127,
        "invalid_client" => crate::i18n::Key::MFEBD23A8C97C,
        "invalid_grant" => crate::i18n::Key::M6148B9E75091,
        _ if matches!(stage, OAuthStage::Authorize) => crate::i18n::Key::M1AB3FE74156A,
        _ => crate::i18n::Key::MCAA11F01414A,
    };
    let stage = match stage {
        OAuthStage::Authorize => "authorize",
        OAuthStage::Token => "token",
        OAuthStage::Refresh => "refresh",
    };
    let mut message = format!("{}\r\n[OAuth2/{stage}; {code}", crate::i18n::text(key));
    if let Some(status) = status {
        message.push_str(&format!("; HTTP {status}"));
    }
    message.push(']');
    message
}

fn token_exchange_error(status: u32, body: &[u8], stage: OAuthStage) -> String {
    let value = serde_json::from_slice::<Value>(body).ok();
    let code = value
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(Value::as_str);
    oauth_error_message(stage, code, Some(status))
}

fn parse_token_response(
    app_id: &str,
    v: &Value,
    previous: Option<&Tokens>,
    now: u64,
) -> Result<Tokens, String> {
    if !v
        .get("token_type")
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("Bearer"))
    {
        return Err(crate::i18n::text(crate::i18n::Key::MA5A88C2FB40D));
    }
    let lifetime = v
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|n| (121..=366 * 24 * 3600).contains(n))
        .ok_or(crate::i18n::text(crate::i18n::Key::MA20A6E0EA879))?;
    let tokens = Tokens {
        app_id: app_id.into(),
        access_token: v
            .get("access_token")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        refresh_token: v
            .get("refresh_token")
            .and_then(Value::as_str)
            .or_else(|| previous.map(|p| p.refresh_token.as_str()))
            .unwrap_or("")
            .into(),
        expires_at: now
            .checked_add(lifetime)
            .ok_or(crate::i18n::text(crate::i18n::Key::MA20A6E0EA879))?,
        scope: v
            .get("scope")
            .and_then(Value::as_str)
            .or_else(|| previous.map(|p| p.scope.as_str()))
            .unwrap_or("")
            .into(),
    };
    validate_tokens(&tokens)?;
    Ok(tokens)
}

fn validate_tokens(tokens: &Tokens) -> Result<(), String> {
    if !config::snowflake(&tokens.app_id)
        || !valid_token(&tokens.access_token)
        || (!tokens.refresh_token.is_empty() && !valid_token(&tokens.refresh_token))
    {
        return Err(crate::i18n::text(crate::i18n::Key::M945455F2EA8A));
    }
    if tokens.scope.len() > 4096
        || !["rpc", "messages.read"].iter().all(|required| {
            tokens
                .scope
                .split_ascii_whitespace()
                .any(|s| s == *required)
        })
    {
        return Err(crate::i18n::text(crate::i18n::Key::M8587176A536A));
    }
    Ok(())
}
fn valid_token(token: &str) -> bool {
    (16..=4096).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
}

enum Callback {
    Unrelated,
    Code(String),
    Denied(String),
}
fn parse_callback(request: &str, state: &str) -> Result<Callback, &'static str> {
    let (head, _) = request.split_once("\r\n\r\n").ok_or("Incomplete request")?;
    let mut lines = head.split("\r\n");
    let line = lines.next().ok_or("Missing request")?;
    let fields: Vec<_> = line.split(' ').collect();
    if fields.len() != 3 || fields[0] != "GET" || !["HTTP/1.1", "HTTP/1.0"].contains(&fields[2]) {
        return Err("Invalid method");
    }
    let mut host = None;
    for line in lines {
        let (key, value) = line.split_once(':').ok_or("Invalid header")?;
        if key.eq_ignore_ascii_case("host") && host.replace(value.trim()).is_some() {
            return Err("Duplicate host");
        }
    }
    if host != Some(CALLBACK_ADDRESS) {
        return Err("Invalid host");
    }
    let (path, query) = fields[1].split_once('?').unwrap_or((fields[1], ""));
    if path != "/callback" {
        return Ok(Callback::Unrelated);
    }
    let mut params = BTreeMap::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (key, value) = pair.split_once('=').ok_or("Invalid query")?;
        let key = percent_decode(key)?;
        let value = percent_decode(value)?;
        if params.insert(key, value).is_some() {
            return Err("Duplicate query field");
        }
    }
    let actual = params.get("state").ok_or("Missing state")?;
    if !constant_time_equal(actual.as_bytes(), state.as_bytes()) {
        return Err("Invalid state");
    }
    if params
        .get("iss")
        .is_some_and(|v| v != "https://discord.com")
    {
        return Err("Invalid issuer");
    }
    if let Some(error) = params.get("error") {
        return Ok(Callback::Denied(oauth_error_message(
            OAuthStage::Authorize,
            Some(error),
            None,
        )));
    }
    let code = params.remove("code").ok_or("Missing code")?;
    if !valid_token(&code) {
        return Err("Invalid code");
    }
    Ok(Callback::Code(code))
}

fn wait_callback(
    listener: &TcpListener,
    state: &str,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        check_cancel(cancel)?;
        match listener.accept() {
            Ok((mut stream, address)) => {
                if !address.ip().is_loopback() {
                    continue;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_millis(700)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(700)));
                let request = read_request(&mut stream);
                match request
                    .as_deref()
                    .map_err(|_| "Bad request")
                    .and_then(|r| parse_callback(r, state))
                {
                    Ok(Callback::Code(code)) => {
                        reply(
                            &mut stream,
                            "200 OK",
                            &crate::i18n::text(crate::i18n::Key::M701C07B31F1F),
                        );
                        return Ok(code);
                    }
                    Ok(Callback::Denied(reason)) => {
                        reply(
                            &mut stream,
                            "400 Bad Request",
                            &crate::i18n::text(crate::i18n::Key::M0659284AA870),
                        );
                        return Err(reason);
                    }
                    Ok(Callback::Unrelated) => reply(&mut stream, "404 Not Found", "Not found"),
                    Err(_) => reply(
                        &mut stream,
                        "400 Bad Request",
                        "Invalid authorization response",
                    ),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(_) => return Err(crate::i18n::text(crate::i18n::Key::M353BD2AB2FB4)),
        }
    }
    Err(crate::i18n::text(crate::i18n::Key::M488D8F83845B))
}
fn read_request(stream: &mut TcpStream) -> Result<String, ()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut data = Vec::new();
    let mut buffer = [0u8; 1024];
    while data.len() < 8192 && Instant::now() < deadline {
        let n = stream.read(&mut buffer).map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        data.extend_from_slice(&buffer[..n]);
        if data.len() > 8192 {
            return Err(());
        }
        if data.windows(4).any(|v| v == b"\r\n\r\n") {
            return String::from_utf8(data).map_err(|_| ());
        }
    }
    Err(())
}
fn reply(stream: &mut TcpStream, status: &str, text: &str) {
    let body = callback_body(text);
    let response=format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{body}",body.len());
    let _ = stream.write_all(response.as_bytes());
}

fn callback_body(text: &str) -> String {
    // Locale files are editable. Their messages are text, never HTML markup.
    let text = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<!doctype html><meta charset=\"utf-8\"><title>Sideglow</title><p>{text}</p>")
}

fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}
fn percent_decode(input: &str) -> Result<String, &'static str> {
    let bytes = input.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).ok_or("Invalid escape")?;
                let hex = std::str::from_utf8(hex).map_err(|_| "Invalid escape")?;
                result.push(u8::from_str_radix(hex, 16).map_err(|_| "Invalid escape")?);
                i += 3;
            }
            b'+' => {
                result.push(b' ');
                i += 1;
            }
            b => {
                result.push(b);
                i += 1;
            }
        }
    }
    if result.iter().any(|b| b.is_ascii_control()) {
        return Err("Control character");
    }
    String::from_utf8(result).map_err(|_| "Invalid UTF-8")
}
fn form(pairs: &[(&str, &str)]) -> String {
    fn encode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                out.push(b as char);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
        out
    }
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}
fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        }
    }
    out
}

#[cfg(windows)]
fn random_bytes() -> Result<[u8; 32], String> {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut bytes = [0u8; 32];
    if unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err(crate::i18n::text(crate::i18n::Key::MDCD6073FA321));
    }
    Ok(bytes)
}
#[cfg(windows)]
fn sha256(bytes: &[u8]) -> Result<[u8; 32], String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Security::Cryptography::*;
    let mut algorithm = null_mut();
    let mut output = [0u8; 32];
    unsafe {
        if BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_SHA256_ALGORITHM, null(), 0) < 0 {
            return Err(crate::i18n::text(crate::i18n::Key::M0C3EE566C5F3));
        }
        let status = BCryptHash(
            algorithm,
            null(),
            0,
            bytes.as_ptr(),
            bytes.len() as u32,
            output.as_mut_ptr(),
            32,
        );
        BCryptCloseAlgorithmProvider(algorithm, 0);
        if status < 0 {
            return Err(crate::i18n::text(crate::i18n::Key::M85A8A0363710));
        }
    }
    Ok(output)
}
#[cfg(windows)]
fn open_browser(url: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let url: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as usize <= 32 {
        Err(crate::i18n::text(crate::i18n::Key::M6AABCC228901))
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn random_bytes() -> Result<[u8; 32], String> {
    Err(crate::i18n::text(crate::i18n::Key::MC5301EFE77F2))
}
#[cfg(not(windows))]
fn sha256(_: &[u8]) -> Result<[u8; 32], String> {
    Err(crate::i18n::text(crate::i18n::Key::MCCB4CBC74631))
}
#[cfg(not(windows))]
fn open_browser(_: &str) -> Result<(), String> {
    Err(crate::i18n::text(crate::i18n::Key::M34137085A508))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_catalog_text_cannot_insert_markup() {
        let body = callback_body("日本語 & <a href='https://example.invalid'>link</a>");
        assert!(body.contains("日本語 &amp; &lt;a href="));
        assert!(body.contains("&gt;link&lt;/a&gt;"));
        assert!(!body.contains("<a "));
    }

    fn request(query: &str) -> String {
        format!("GET /callback?{query} HTTP/1.1\r\nHost: {CALLBACK_ADDRESS}\r\n\r\n")
    }
    #[test]
    fn callback_requires_matching_state_host_and_issuer() {
        let ok = request("code=01234567890123456789&state=expected&iss=https%3A%2F%2Fdiscord.com");
        assert!(matches!(
            parse_callback(&ok, "expected"),
            Ok(Callback::Code(_))
        ));
        for bad in [
            ok.replace("state=expected", "state=wrong"),
            ok.replace(CALLBACK_ADDRESS, "evil.test"),
            ok.replace("discord.com", "evil.test"),
            ok.replace("state=expected", "state=expected&state=expected"),
            ok.replace("code=01234567890123456789", "code=%0D%0Ainjected"),
        ] {
            assert!(parse_callback(&bad, "expected").is_err());
        }
    }
    #[test]
    fn oauth_error_never_echoes_remote_description() {
        let req = request("state=x&error=access_denied&error_description=SECRET");
        match parse_callback(&req, "x").unwrap_or(Callback::Unrelated) {
            Callback::Denied(s) => assert!(!s.contains("SECRET")),
            _ => panic!("denial expected"),
        }
        assert!(parse_callback(&req, "another-state").is_err());
    }
    #[test]
    fn callback_denials_distinguish_scope_client_and_user_rejection() {
        let cases = [
            ("invalid_scope", crate::i18n::Key::M2E99DBE33375),
            ("unauthorized_client", crate::i18n::Key::M4FE33C222835),
            ("access_denied", crate::i18n::Key::ME856B7836127),
        ];
        for (code, key) in cases {
            let req = request(&format!(
                "state=x&error={code}&error_description=SECRET&code=OTHER_SECRET"
            ));
            let Ok(Callback::Denied(message)) = parse_callback(&req, "x") else {
                panic!("expected denial");
            };
            assert!(message.starts_with(&crate::i18n::text(key)));
            assert!(message.ends_with(&format!("[OAuth2/authorize; {code}]")));
            assert!(!message.contains("SECRET"));
        }
        let req = request("state=x&error=SECRET_TOKEN&error_description=OTHER_SECRET");
        let Ok(Callback::Denied(message)) = parse_callback(&req, "x") else {
            panic!("expected denial");
        };
        assert!(message.ends_with("[OAuth2/authorize; unrecognized_error]"));
        assert!(!message.contains("SECRET"));
    }
    #[test]
    fn token_exchange_diagnostics_preserve_stage_status_and_safe_code_only() {
        let body = br#"{"error":"invalid_scope","error_description":"SECRET","access_token":"OTHER_SECRET"}"#;
        let message = token_exchange_error(400, body, OAuthStage::Token);
        assert!(message.starts_with(&crate::i18n::text(crate::i18n::Key::M2E99DBE33375)));
        assert!(message.ends_with("[OAuth2/token; invalid_scope; HTTP 400]"));
        assert!(!message.contains("SECRET"));

        let body = br#"{"error":"invalid_grant","refresh_token":"SECRET"}"#;
        let message = token_exchange_error(400, body, OAuthStage::Refresh);
        assert!(message.ends_with("[OAuth2/refresh; invalid_grant; HTTP 400]"));
        assert!(!message.contains("SECRET"));
        for body in [
            br#"{"error":"SECRET_TOKEN","error_description":"SECRET"}"#.as_slice(),
            b"<html>SECRET</html>",
            br#"{"error":{"credential":"SECRET"}}"#,
        ] {
            let message = token_exchange_error(500, body, OAuthStage::Token);
            assert!(message.contains("[OAuth2/token; "));
            assert!(message.ends_with("; HTTP 500]"));
            assert!(!message.contains("SECRET"));
        }
    }
    #[test]
    fn oauth_grant_must_have_chat_read_permissions() {
        let mut v = serde_json::json!({"token_type":"Bearer","access_token":"example_only_0123456789","refresh_token":"refresh_example_0123456789","expires_in":604800,"scope":"identify"});
        assert!(parse_token_response("123456789012345678", &v, None, 100).is_err());
        v["scope"] = serde_json::json!("identify rpc messages.read");
        let old = parse_token_response("123456789012345678", &v, None, 100).unwrap();
        v.as_object_mut().unwrap().remove("scope");
        v.as_object_mut().unwrap().remove("refresh_token");
        let next = parse_token_response("123456789012345678", &v, Some(&old), 200).unwrap();
        assert_eq!(next.refresh_token, old.refresh_token);
        assert_eq!(next.expires_at, 605000);
        v["expires_in"] = serde_json::json!(u64::MAX);
        assert!(parse_token_response("123456789012345678", &v, Some(&old), 100).is_err());
    }
    #[test]
    fn form_encoding_cannot_inject_oauth_parameters() {
        let encoded = form(&[
            ("code", "a&client_secret=evil+%"),
            ("redirect_uri", REDIRECT_URI),
        ]);
        assert!(encoded.contains("a%26client_secret%3Devil%2B%25"));
        assert_eq!(encoded.split('&').count(), 2);
        assert_eq!(percent_decode("a%26b%2Bc+d").unwrap(), "a&b+c d");
    }
    #[test]
    fn base64url_matches_standard_vectors() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[251, 255, 255]), "-___");
        // SHA-256 digest from RFC 7636 Appendix B; hashing uses Windows CNG, not custom crypto.
        let digest =
            config::decode_hex("13d31e961a1ad8ec2f16b10c4c982e0876a878ad6df144566ee1894acb70f9c3")
                .unwrap();
        assert_eq!(
            base64url(&digest),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_cng_pkce_matches_rfc7636() {
        let hash = sha256(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk").unwrap();
        assert_eq!(
            base64url(&hash),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
