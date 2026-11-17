// SPDX-License-Identifier: MPL-2.0
//! Fixed-origin HTTPS for public OAuth and one explicitly configured incoming webhook.
//! No user-account token scraping, redirects, embedded browser, or HTTP message polling.
use crate::{config, model::Message};
use serde_json::{json, Value};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HttpTiming {
    pub send_ms: u64,
    pub response_ms: u64,
    pub body_ms: u64,
}

pub(crate) struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    pub timing: HttpTiming,
}

pub(crate) struct SendReceipt {
    pub message: Message,
    pub timing: HttpTiming,
}

#[derive(Debug)]
pub(crate) struct SendError {
    pub reason: String,
    pub recheck_webhook: bool,
    pub recreate_http: bool,
    /// An operator must repair permissions or destination settings before an
    /// automatic translation worker may spend requests on more messages.
    pub fatal: bool,
    /// Validated Discord rate-limit hint, rounded up to a whole second.
    pub retry_after_secs: Option<u32>,
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl SendError {
    pub(crate) fn transport_failure(reason: String) -> Self {
        Self {
            reason,
            recheck_webhook: false,
            recreate_http: true,
            fatal: false,
            retry_after_secs: None,
        }
    }

    fn unknown_response(reason: &str) -> Self {
        Self::transport_failure(crate::i18n::format(
            crate::i18n::Key::M7D791B76D75B,
            &[("reason", reason.to_string())],
        ))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Webhook {
    path: String,
}
impl fmt::Debug for Webhook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Webhook([redacted])")
    }
}
impl Webhook {
    pub fn id(&self) -> &str {
        self.path.split('/').nth(4).unwrap_or("")
    }

    pub fn parse(url: &str) -> Result<Self, String> {
        let url = url.trim();
        let path = url
            .strip_prefix("https://discord.com/api/")
            .ok_or(crate::i18n::text(crate::i18n::Key::M54ED814B788C))?;
        let path = path.strip_prefix("v10/").unwrap_or(path);
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() != 3
            || parts[0] != "webhooks"
            || !config::snowflake(parts[1])
            || !(32..=256).contains(&parts[2].len())
            || !parts[2]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(crate::i18n::text(crate::i18n::Key::MAC38C01D66DD));
        }
        Ok(Self {
            path: format!("/api/v10/{path}"),
        })
    }

    pub fn verify(&self, http: &mut DiscordHttp, channel: &str) -> Result<HttpTiming, String> {
        self.verify_detailed(http, channel)
            .map_err(|error| error.reason)
    }

    pub fn verify_detailed(
        &self,
        http: &mut DiscordHttp,
        channel: &str,
    ) -> Result<HttpTiming, SendError> {
        let response = http
            .request("GET", &self.path, None, &[])
            .map_err(SendError::transport_failure)?;
        parse_verify_response(&response, channel)
    }

    pub fn send(
        &self,
        http: &mut DiscordHttp,
        expected_channel: &str,
        text: &str,
        display_name: &str,
    ) -> Result<SendReceipt, SendError> {
        self.send_with_input_policy(http, expected_channel, text, display_name, false)
    }

    pub fn send_with_input_policy(
        &self,
        http: &mut DiscordHttp,
        expected_channel: &str,
        text: &str,
        display_name: &str,
        romaji_japanese_input: bool,
    ) -> Result<SendReceipt, SendError> {
        if !config::snowflake(expected_channel) {
            return Err(SendError {
                reason: crate::i18n::text(crate::i18n::Key::ME23722E5042E),
                recheck_webhook: true,
                recreate_http: false,
                fatal: true,
                retry_after_secs: None,
            });
        }
        let body = serde_json::to_vec(&webhook_payload(text, display_name, romaji_japanese_input))
            .map_err(|_| SendError {
                reason: crate::i18n::text(crate::i18n::Key::M36BBBEF7D3D1),
                recheck_webhook: false,
                recreate_http: false,
                fatal: false,
                retry_after_secs: None,
            })?;
        // `wait=true` returns the stored message. Never automatically repeat an ambiguous POST.
        let response = http
            .request(
                "POST",
                &format!("{}?wait=true", self.path),
                Some("application/json"),
                &body,
            )
            .map_err(|error| {
                SendError::unknown_response(&crate::i18n::format(
                    crate::i18n::Key::M20E9BEE2F037,
                    &[("error", error.to_string())],
                ))
            })?;
        parse_send_response(&response, expected_channel)
    }
}

fn parse_verify_response(response: &Response, channel: &str) -> Result<HttpTiming, SendError> {
    check_webhook_status(response)?;
    let value: Value = serde_json::from_slice(&response.body).map_err(|_| SendError {
        reason: crate::i18n::text(crate::i18n::Key::MBB8470FCE69A),
        recheck_webhook: false,
        recreate_http: false,
        fatal: false,
        retry_after_secs: None,
    })?;
    validate_webhook_channel(&value, channel).map_err(|reason| SendError {
        reason,
        recheck_webhook: true,
        recreate_http: false,
        fatal: true,
        retry_after_secs: None,
    })?;
    Ok(response.timing)
}

fn parse_send_response(
    response: &Response,
    expected_channel: &str,
) -> Result<SendReceipt, SendError> {
    check_webhook_status(response)?;
    let value: Value = serde_json::from_slice(&response.body).map_err(|_| {
        SendError::unknown_response(&crate::i18n::text(crate::i18n::Key::M6E3E768EFBF3))
    })?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| config::snowflake(id))
        .ok_or_else(|| {
            SendError::unknown_response(&crate::i18n::text(crate::i18n::Key::MBF0BF2EA8F5D))
        })?;
    let channel = value
        .get("channel_id")
        .and_then(Value::as_str)
        .filter(|id| config::snowflake(id))
        .ok_or_else(|| {
            SendError::unknown_response(&crate::i18n::text(crate::i18n::Key::MF85353EF3D94))
        })?;
    if channel != expected_channel {
        return Err(SendError {
            reason: crate::i18n::text(crate::i18n::Key::MC8070EFA894D),
            recheck_webhook: true,
            recreate_http: false,
            fatal: true,
            retry_after_secs: None,
        });
    }
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            SendError::unknown_response(&crate::i18n::text(crate::i18n::Key::MD18AE0925461))
        })?;
    let author = value.get("author").ok_or_else(|| {
        SendError::unknown_response(&crate::i18n::text(crate::i18n::Key::MD100EE5DD4F0))
    })?;
    let author_name = author
        .get("global_name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            author
                .get("username")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
        })
        .ok_or_else(|| {
            SendError::unknown_response(&crate::i18n::text(crate::i18n::Key::MD100EE5DD4F0))
        })?;
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .filter(|timestamp| {
            !timestamp.is_empty()
                && timestamp.len() <= 64
                && !timestamp.chars().any(char::is_control)
        })
        .ok_or_else(|| {
            SendError::unknown_response(&crate::i18n::text(crate::i18n::Key::M0364F1F17733))
        })?;
    let parent_id = value
        .get("message_reference")
        .and_then(|reference| reference.get("message_id"))
        .and_then(Value::as_str)
        .filter(|id| config::snowflake(id))
        .map(str::to_owned);
    // Use only the confirmed server fields. Discord may sanitize content, so
    // the original draft cannot stand in for a missing or malformed response.
    Ok(SendReceipt {
        message: Message {
            id: id.to_owned(),
            parent_id,
            author: bounded(author_name, 256),
            content: bounded(content, 16_000),
            original_text: None,
            timestamp: timestamp.to_owned(),
            bot: author.get("bot").and_then(Value::as_bool).unwrap_or(true),
            author_id: author
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| config::snowflake(id))
                .unwrap_or("")
                .to_owned(),
            webhook_id: value
                .get("webhook_id")
                .and_then(Value::as_str)
                .filter(|id| config::snowflake(id))
                .map(str::to_owned),
            translation: None,
        },
        timing: response.timing,
    })
}

fn bounded(value: &str, max_bytes: usize) -> String {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn validate_webhook_channel(value: &Value, channel: &str) -> Result<(), String> {
    if value.get("type").and_then(Value::as_u64) != Some(1) {
        return Err(crate::i18n::text(crate::i18n::Key::M87CD328441B1));
    }
    if value.get("channel_id").and_then(Value::as_str) != Some(channel) {
        return Err(crate::i18n::text(crate::i18n::Key::MB18A7980F4DE));
    }
    Ok(())
}

fn webhook_payload(text: &str, display_name: &str, romaji_japanese_input: bool) -> Value {
    json!({
        "content": text,
        "username": crate::sender_label::webhook_username(display_name, romaji_japanese_input),
        "allowed_mentions": { "parse": [], "replied_user": false }
    })
}

fn check_webhook_status(response: &Response) -> Result<(), SendError> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    if response.status == 429 {
        let wait = serde_json::from_slice::<Value>(&response.body)
            .ok()
            .and_then(|body| body.get("retry_after").and_then(Value::as_f64))
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0 && *seconds <= 86_400.0);
        return Err(SendError {
            reason: match wait {
                Some(seconds) => {
                    let __ui_arg0 = &((seconds * 10.0).ceil() / 10.0);
                    crate::i18n::format(
                        crate::i18n::Key::M5F801D6F6ECA,
                        &[("arg0", format!("{:.1}", __ui_arg0))],
                    )
                }
                None => crate::i18n::text(crate::i18n::Key::M83434976FB9F),
            },
            recheck_webhook: false,
            recreate_http: false,
            fatal: false,
            retry_after_secs: wait.map(|seconds| seconds.ceil() as u32),
        });
    }
    let reason = match response.status {
        401 | 403 => crate::i18n::text(crate::i18n::Key::ME0110BF539A3),
        404 => crate::i18n::text(crate::i18n::Key::MD18A1BFBC2CD),
        400 => crate::i18n::text(crate::i18n::Key::MFD6F0121AC04),
        300..=399 => crate::i18n::text(crate::i18n::Key::M67AB73BB2C20),
        _ => crate::i18n::text(crate::i18n::Key::MDC34E43C1437),
    };
    // Error bodies can contain credentials or user text: expose only a fixed message and HTTP status.
    Err(SendError {
        reason: format!("{reason} (HTTP {})", response.status),
        recheck_webhook: matches!(response.status, 401 | 403 | 404),
        // An ordinary HTTP error is not evidence of a broken HTTP session.
        // Retain connection and proxy caches, including after rate limiting.
        recreate_http: false,
        fatal: matches!(response.status, 300..=399 | 401 | 403 | 404),
        retry_after_secs: None,
    })
}

#[cfg(windows)]
pub(crate) use native::DiscordHttp;
#[cfg(windows)]
mod native {
    use super::{HttpTiming, Response};
    use std::{
        ffi::c_void,
        ptr::{null, null_mut},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::Networking::WinHttp::*;
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
    pub(crate) struct DiscordHttp {
        connection: Handle,
        _session: Handle,
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn failure(stage: &str) -> String {
        // Numeric WinHTTP error only; no URL, token, response, proxy credentials, or request headers.
        format!(
            "{stage} (Windows {})",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        )
    }
    fn elapsed_ms(started: Instant) -> u64 {
        started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
    impl DiscordHttp {
        pub fn new() -> Result<Self, String> {
            unsafe {
                let agent = wide(concat!("Sideglow/", env!("CARGO_PKG_VERSION")));
                let raw = WinHttpOpen(
                    agent.as_ptr(),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    null(),
                    null(),
                    0,
                );
                if raw.is_null() {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M37B7D16E98A3)));
                }
                let session = Handle(raw);
                if WinHttpSetTimeouts(session.0, 5000, 7000, 10000, 10000) == 0 {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M94BAD0A3F39F)));
                }
                let host = wide("discord.com");
                let raw = WinHttpConnect(session.0, host.as_ptr(), 443, 0);
                if raw.is_null() {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M0B99331EA6A4)));
                }
                Ok(Self {
                    connection: Handle(raw),
                    _session: session,
                })
            }
        }
        pub fn request(
            &mut self,
            method: &str,
            path: &str,
            content_type: Option<&str>,
            body: &[u8],
        ) -> Result<Response, String> {
            if !path.starts_with("/api/")
                || path.contains(['\r', '\n', '\0'])
                || body.len() > 64 * 1024
            {
                return Err(crate::i18n::text(crate::i18n::Key::MFA8D1B08FB2B));
            }
            unsafe {
                let method = wide(method);
                let path = wide(path);
                let raw = WinHttpOpenRequest(
                    self.connection.0,
                    method.as_ptr(),
                    path.as_ptr(),
                    null(),
                    null(),
                    null(),
                    WINHTTP_FLAG_SECURE,
                );
                if raw.is_null() {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::MBD72EAA84AD3)));
                }
                let request = Handle(raw);
                let disabled = WINHTTP_DISABLE_REDIRECTS
                    | WINHTTP_DISABLE_COOKIES
                    | WINHTTP_DISABLE_AUTHENTICATION;
                if WinHttpSetOption(
                    request.0,
                    WINHTTP_OPTION_DISABLE_FEATURE,
                    &disabled as *const _ as *const c_void,
                    4,
                ) == 0
                {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M8F54754D11A2)));
                }
                let autologon = WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH;
                if WinHttpSetOption(
                    request.0,
                    WINHTTP_OPTION_AUTOLOGON_POLICY,
                    &autologon as *const _ as *const c_void,
                    4,
                ) == 0
                {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M2A89E500C24A)));
                }
                let mut headers =
                    "Accept: application/json\r\nCache-Control: no-store\r\n".to_owned();
                if let Some(t) = content_type {
                    headers.push_str(&format!("Content-Type: {t}\r\n"));
                }
                let headers = wide(&headers);
                let started = Instant::now();
                if WinHttpSendRequest(
                    request.0,
                    headers.as_ptr(),
                    (headers.len() - 1) as u32,
                    if body.is_empty() {
                        null()
                    } else {
                        body.as_ptr() as *const c_void
                    },
                    body.len() as u32,
                    body.len() as u32,
                    0,
                ) == 0
                {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::MFAA6A904728B)));
                }
                let send_ms = elapsed_ms(started);
                let response_started = Instant::now();
                if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M7F8B393A8AD6)));
                }
                let mut status = 0u32;
                let mut size = 4u32;
                if WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    null(),
                    &mut status as *mut _ as *mut c_void,
                    &mut size,
                    null_mut(),
                ) == 0
                {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M6FFF917FD9DF)));
                }
                let response_ms = elapsed_ms(response_started);
                let body_started = Instant::now();
                let mut data = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    if started.elapsed() > Duration::from_secs(35) {
                        return Err(crate::i18n::text(crate::i18n::Key::M9E81E8997839));
                    }
                    let mut read = 0;
                    if WinHttpReadData(
                        request.0,
                        buffer.as_mut_ptr() as *mut c_void,
                        buffer.len() as u32,
                        &mut read,
                    ) == 0
                    {
                        return Err(failure(&crate::i18n::text(crate::i18n::Key::MCBB2A7807DF9)));
                    }
                    if read == 0 {
                        break;
                    }
                    if data.len() + read as usize > 256 * 1024 {
                        return Err(crate::i18n::text(crate::i18n::Key::M5EF193E301E3));
                    }
                    data.extend_from_slice(&buffer[..read as usize]);
                }
                Ok(Response {
                    status,
                    body: data,
                    timing: HttpTiming {
                        send_ms,
                        response_ms,
                        body_ms: elapsed_ms(body_started),
                    },
                })
            }
        }
    }
}

#[cfg(not(windows))]
pub(crate) struct DiscordHttp;
#[cfg(not(windows))]
impl DiscordHttp {
    pub fn new() -> Result<Self, String> {
        Err(crate::i18n::text(crate::i18n::Key::MB3C69780E79A))
    }
    pub fn request(
        &mut self,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: &[u8],
    ) -> Result<Response, String> {
        Err(crate::i18n::text(crate::i18n::Key::M5C4C80556AC4))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CHANNEL: &str = "223456789012345678";
    const MESSAGE: &str = "323456789012345678";
    const URL: &str =
        "https://discord.com/api/webhooks/123456789012345678/ABCDEFGHIJKLMNOPQRSTUVWXYZ_1234567890";

    fn confirmation() -> Value {
        json!({
            "id": MESSAGE,
            "channel_id": CHANNEL,
            "content": "서버가 확인한 문장",
            "timestamp": "2026-11-11T01:00:00.000000+00:00",
            "author": {"username":"Sideglow · Alex","global_name":null,"bot":true}
        })
    }

    fn response(status: u32, value: Value) -> Response {
        Response {
            status,
            body: serde_json::to_vec(&value).unwrap(),
            timing: HttpTiming {
                send_ms: 17,
                response_ms: 29,
                body_ms: 3,
            },
        }
    }
    #[test]
    fn webhook_origin_and_path_are_strict() {
        assert!(Webhook::parse(URL).is_ok());
        for u in [
            URL.replace("discord.com", "discord.com.evil.test"),
            URL.replace("https://", "http://"),
            format!("{URL}?thread_id=123"),
            format!("{URL}/messages/1"),
            URL.replace("discord.com", "user@discord.com"),
            URL.replace("_", "%2f"),
            format!("{URL}#x"),
        ] {
            assert!(Webhook::parse(&u).is_err());
        }
        assert!(
            !format!("{:?}", Webhook::parse(URL).unwrap()).contains("ABCDEFGHIJKLMNOPQRSTUVWXYZ")
        );
    }
    #[test]
    fn refuses_webhook_pointing_to_a_different_channel() {
        assert!(validate_webhook_channel(&json!({"type":1,"channel_id":"123"}), "123").is_ok());
        assert!(validate_webhook_channel(&json!({"type":1,"channel_id":"123"}), "456").is_err());
        assert!(validate_webhook_channel(&json!({"type":2,"channel_id":"123"}), "123").is_err());
    }
    #[test]
    fn sending_cannot_ping_users_or_everyone() {
        let p = webhook_payload("@everyone <@123456789012345678>", "Alex", false);
        assert_eq!(p["allowed_mentions"]["parse"], json!([]));
        assert_eq!(p["allowed_mentions"]["replied_user"], false);
        assert!(p.get("avatar_url").is_none());
    }

    #[test]
    fn input_policy_preserves_text_and_nickname_without_extra_message_fields() {
        for (enabled, expected) in [(false, "Sideglow · ☆"), (true, "Sideglow [romaji] · ☆")]
        {
            let payload = webhook_payload("arigatou @everyone", "☆", enabled);
            assert_eq!(payload["username"], expected);
            assert_eq!(payload["content"], "arigatou @everyone");
            assert_eq!(payload["allowed_mentions"]["parse"], json!([]));
            assert_eq!(payload["allowed_mentions"]["replied_user"], false);
            assert_eq!(payload.as_object().unwrap().len(), 3);
        }
    }
    #[test]
    fn remote_errors_do_not_echo_credentials() {
        let e = check_webhook_status(&Response {
            status: 403,
            body: br#"{"message":"secret-token"}"#.to_vec(),
            timing: HttpTiming::default(),
        })
        .unwrap_err();
        assert!(!e.reason.contains("secret-token"));
        assert!(e.reason.contains("403"));
    }

    #[test]
    fn confirmed_send_uses_server_message_and_preserves_http_timings() {
        let data = confirmation();
        let receipt = parse_send_response(&response(200, data), CHANNEL).unwrap();
        assert_eq!(receipt.message.id, MESSAGE);
        assert_eq!(receipt.message.content, "서버가 확인한 문장");
        assert_eq!(receipt.message.author, "Sideglow · Alex");
        assert_eq!(
            receipt.message.timestamp,
            "2026-11-11T01:00:00.000000+00:00"
        );
        assert!(receipt.message.bot);
        assert!(receipt.message.parent_id.is_none());
        assert_eq!(
            receipt.timing,
            HttpTiming {
                send_ms: 17,
                response_ms: 29,
                body_ms: 3
            }
        );

        let mut data = confirmation();
        data["author"]["global_name"] = json!("서버 표시 이름");
        data["content"] = json!(""); // A server-sanitized empty body is still its real response.
        let receipt = parse_send_response(&response(200, data), CHANNEL).unwrap();
        assert_eq!(receipt.message.author, "서버 표시 이름");
        assert!(receipt.message.content.is_empty());
    }

    #[test]
    fn missing_or_malformed_confirmation_cannot_become_success() {
        for field in ["id", "channel_id", "content", "author", "timestamp"] {
            let mut data = confirmation();
            data.as_object_mut().unwrap().remove(field);
            data["message"] = json!("private-token-that-must-not-be-shown");
            let error = parse_send_response(&response(200, data), CHANNEL)
                .err()
                .unwrap();
            assert!(error.recreate_http, "{field}");
            assert!(!error.reason.contains("private-token"));
        }
        for (field, value) in [
            ("id", json!("not-a-message-id")),
            ("channel_id", json!(3)),
            ("content", json!(false)),
            ("author", json!({"username":"  "})),
            ("timestamp", json!("")),
            ("timestamp", json!("x".repeat(65))),
            ("timestamp", json!("2026-11-11\nprivate-token")),
        ] {
            let mut data = confirmation();
            data[field] = value;
            let error = parse_send_response(&response(200, data), CHANNEL)
                .err()
                .unwrap();
            assert!(error.recreate_http, "{field}");
            assert!(!error.reason.contains("private-token"));
        }
        for (status, body) in [(200, b"not json".to_vec()), (204, Vec::new())] {
            let response = Response {
                status,
                body,
                timing: HttpTiming::default(),
            };
            assert!(
                parse_send_response(&response, CHANNEL)
                    .err()
                    .unwrap()
                    .recreate_http
            );
        }
    }

    #[test]
    fn confirmed_different_channel_requires_recheck_without_claiming_unsent() {
        let mut data = confirmation();
        data["channel_id"] = json!("423456789012345678");
        data["content"] = json!("private message");
        let error = parse_send_response(&response(200, data), CHANNEL)
            .err()
            .unwrap();
        assert!(error.recheck_webhook);
        assert!(!error.recreate_http);
        assert!(error.fatal);
        assert!(error.reason.contains("다른 채널에 메시지가 전송"));
        assert!(error.reason.contains("자동 재전송하지 않습니다"));
        assert!(!error.reason.contains("423456789012345678"));
        assert!(!error.reason.contains("private message"));
    }

    #[test]
    fn confirmed_message_bounds_preserve_utf8_and_reject_untrusted_reference() {
        let mut data = confirmation();
        data["content"] = json!("가".repeat(8000));
        data["author"]["username"] = json!("나".repeat(200));
        data["message_reference"] = json!({"message_id":"not-an-id"});
        let receipt = parse_send_response(&response(200, data), CHANNEL).unwrap();
        assert_eq!(receipt.message.content.len(), 15_999);
        assert_eq!(receipt.message.author.len(), 255);
        assert!(receipt.message.content.chars().all(|ch| ch == '가'));
        assert!(receipt.message.author.chars().all(|ch| ch == '나'));
        assert!(receipt.message.parent_id.is_none());
    }

    #[test]
    fn known_http_failures_keep_session_and_only_invalid_webhooks_require_recheck() {
        for status in [400, 401, 403, 404, 429, 500, 503] {
            let error = parse_send_response(
                &response(
                    status,
                    json!({"message":"private-token", "retry_after":1.21}),
                ),
                CHANNEL,
            )
            .err()
            .unwrap();
            assert_eq!(error.recheck_webhook, matches!(status, 401 | 403 | 404));
            assert_eq!(error.fatal, matches!(status, 401 | 403 | 404));
            assert!(!error.recreate_http);
            assert!(!error.reason.contains("private-token"));
            if status == 429 {
                assert!(error.reason.contains("1.3초"));
                assert_eq!(error.retry_after_secs, Some(2));
            } else {
                assert_eq!(error.retry_after_secs, None);
            }
        }
        for wait in [json!(-1), json!(0), json!(86401), json!("1.5"), json!(null)] {
            let error =
                check_webhook_status(&response(429, json!({"retry_after":wait}))).unwrap_err();
            assert!(error.reason.contains("잠시 기다린"));
            assert_eq!(error.retry_after_secs, None);
        }
        let error = check_webhook_status(&Response {
            status: 429,
            body: br#"{"retry_after":1e999,"message":"private-token"}"#.to_vec(),
            timing: HttpTiming::default(),
        })
        .unwrap_err();
        assert!(error.reason.contains("잠시 기다린"));
        assert!(!error.reason.contains("private-token"));
        assert_eq!(error.retry_after_secs, None);
    }

    #[test]
    fn detailed_rate_limits_preserve_full_validated_delay_for_verify_and_send() {
        for (seconds, expected) in [
            (0.001, 1),
            (1.21, 2),
            (300.5, 301),
            (3600.0, 3600),
            (86400.0, 86400),
        ] {
            let limited = response(
                429,
                json!({"retry_after": seconds, "message": "private-token"}),
            );
            let verify_error = parse_verify_response(&limited, CHANNEL).unwrap_err();
            let send_error = parse_send_response(&limited, CHANNEL).err().unwrap();
            for error in [verify_error, send_error] {
                assert_eq!(error.retry_after_secs, Some(expected));
                assert!(!error.fatal);
                assert!(!error.recheck_webhook);
                assert!(!error.recreate_http);
                assert!(!error.reason.contains("private-token"));
            }
        }
    }

    #[test]
    fn detailed_verification_distinguishes_setup_failure_from_transient_transport() {
        assert!(parse_verify_response(
            &response(200, json!({"type":1,"channel_id":CHANNEL})),
            CHANNEL
        )
        .is_ok());
        for invalid in [
            response(200, json!({"type":1,"channel_id":"423456789012345678"})),
            response(200, json!({"type":2,"channel_id":CHANNEL})),
            response(401, json!({"message":"private-token"})),
            response(403, json!({"message":"private-token"})),
            response(404, json!({"message":"private-token"})),
        ] {
            let error = parse_verify_response(&invalid, CHANNEL).unwrap_err();
            assert!(error.fatal);
            assert!(error.recheck_webhook);
            assert!(!error.recreate_http);
            assert_eq!(error.retry_after_secs, None);
            assert!(!error.reason.contains("private-token"));
        }
        let error = parse_verify_response(
            &Response {
                status: 200,
                body: b"invalid JSON".to_vec(),
                timing: HttpTiming::default(),
            },
            CHANNEL,
        )
        .unwrap_err();
        assert!(!error.fatal);
        assert!(!error.recreate_http);
        let error = SendError::transport_failure("connection failed".into());
        assert!(error.recreate_http);
        assert!(!error.fatal);
        assert!(!error.recheck_webhook);
    }
}
