// SPDX-License-Identifier: MPL-2.0
//! Read-only Discord RPC over the local IPC pipe.
//!
//! The caller owns OAuth, reconnect policy, and webhook sending. This module
//! never reads a Discord session token or polls channel history. A connection is
//! usable only after AUTHENTICATE, three subscriptions, and one GET_CHANNEL.

use crate::model::{ChatEvent, EventBatch, Message, MAX_MESSAGES};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[cfg(windows)]
#[path = "rpc/pipe.rs"]
mod pipe;

const MAX_FRAME: usize = 1024 * 1024;
const MAX_QUEUED_EVENTS: usize = 256;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TEXT: usize = 16_000;
const MAX_EMBED_TEXT: usize = 8_000;
const MAX_ATTACHMENT_TEXT: usize = 2_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub id: String,
    pub display_name: String,
}

/// All variants contain only local diagnostics or a numeric Discord error code.
/// In particular, Discord error messages and received JSON are never displayed.
#[derive(Debug)]
pub enum RpcError {
    #[cfg(not(windows))]
    UnsupportedPlatform,
    #[cfg_attr(not(windows), allow(dead_code))]
    Unavailable,
    #[cfg_attr(not(windows), allow(dead_code))]
    UntrustedPeer,
    Cancelled,
    Disconnected,
    Timeout,
    AuthenticationRequired {
        code: Option<i64>,
    },
    InvalidInput(String),
    Protocol(String),
    Remote {
        code: Option<i64>,
        permission: bool,
    },
    #[cfg_attr(not(windows), allow(dead_code))]
    Io(u32),
}

impl RpcError {
    pub fn is_permission_denied(&self) -> bool {
        matches!(
            self,
            Self::Remote {
                permission: true,
                ..
            }
        )
    }

    pub fn is_disconnected(&self) -> bool {
        matches!(self, Self::Disconnected | Self::Unavailable)
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub fn is_authentication_required(&self) -> bool {
        matches!(self, Self::AuthenticationRequired { .. })
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(not(windows))]
            Self::UnsupportedPlatform => {
                write!(f, "{}", crate::i18n::text(crate::i18n::Key::M8844636386F2))
            }
            Self::Unavailable => {
                write!(f, "{}", crate::i18n::text(crate::i18n::Key::M4D369400AB76))
            }
            Self::UntrustedPeer => {
                write!(f, "{}", crate::i18n::text(crate::i18n::Key::M3C8E2C206A1D))
            }
            Self::Cancelled => write!(f, "{}", crate::i18n::text(crate::i18n::Key::MEC6D057515AA)),
            Self::Disconnected => {
                write!(f, "{}", crate::i18n::text(crate::i18n::Key::M5E3C7F76DBBA))
            }
            Self::Timeout => write!(f, "{}", crate::i18n::text(crate::i18n::Key::M6EEC9BFB9771)),
            Self::AuthenticationRequired { code } => {
                write!(f, "{}", crate::i18n::text(crate::i18n::Key::MF54EFB5BCE0C))?;
                if let Some(code) = code {
                    write!(
                        f,
                        "{}",
                        crate::i18n::format(
                            crate::i18n::Key::MB070D0F3B9B7,
                            &[("code", format!("{}", code))]
                        )
                    )?;
                }
                write!(f, ".")
            }
            Self::InvalidInput(reason) => write!(
                f,
                "{}",
                crate::i18n::format(
                    crate::i18n::Key::M169214655F72,
                    &[("reason", reason.to_string())]
                )
            ),
            Self::Protocol(reason) => write!(
                f,
                "{}",
                crate::i18n::format(
                    crate::i18n::Key::M8F48B067E90A,
                    &[("reason", reason.to_string())]
                )
            ),
            Self::Remote { code, permission } => {
                if *permission {
                    write!(f, "{}", crate::i18n::text(crate::i18n::Key::M92BB8A537444))?;
                } else {
                    write!(f, "{}", crate::i18n::text(crate::i18n::Key::M2506A8F1679E))?;
                }
                if let Some(code) = code {
                    write!(
                        f,
                        "{}",
                        crate::i18n::format(
                            crate::i18n::Key::MB070D0F3B9B7,
                            &[("code", format!("{}", code))]
                        )
                    )?;
                }
                write!(f, ".")
            }
            Self::Io(code) => write!(
                f,
                "{}",
                crate::i18n::format(
                    crate::i18n::Key::M28BC993F2F34,
                    &[("code", format!("{}", code))]
                )
            ),
        }
    }
}

impl std::error::Error for RpcError {}

/// None means the read deadline elapsed; Some(0) means EOF. Implementations must
/// complete/cancel an operation before returning and releasing its buffer.
trait Wire: Send {
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<Option<usize>, RpcError>;
    fn write_all(&mut self, buf: &[u8], timeout: Duration) -> Result<(), RpcError>;
}

pub struct RpcClient {
    wire: Box<dyn Wire>,
    cancellation: Arc<AtomicBool>,
    decoder: Decoder,
    nonce: u64,
    authenticated: bool,
    ready: bool,
    channel: Option<String>,
    queued: VecDeque<RawEvent>,
    messages: BTreeMap<String, CachedMessage>,
    cursor: u64,
}

impl RpcClient {
    pub fn connect(app_id: &str, cancellation: Arc<AtomicBool>) -> Result<Self, RpcError> {
        if !snowflake(app_id) {
            return Err(RpcError::InvalidInput(crate::i18n::text(
                crate::i18n::Key::M42DE164AE352,
            )));
        }
        if cancellation.load(Ordering::Acquire) {
            return Err(RpcError::Cancelled);
        }
        #[cfg(windows)]
        {
            let wire = pipe::Pipe::connect(cancellation.clone())?;
            Self::start(Box::new(wire), app_id, cancellation)
        }
        #[cfg(not(windows))]
        {
            Err(RpcError::UnsupportedPlatform)
        }
    }

    #[cfg(any(windows, test))]
    fn start(
        wire: Box<dyn Wire>,
        app_id: &str,
        cancellation: Arc<AtomicBool>,
    ) -> Result<Self, RpcError> {
        let mut client = Self {
            wire,
            cancellation,
            decoder: Decoder::default(),
            nonce: 0,
            authenticated: false,
            ready: false,
            channel: None,
            queued: VecDeque::new(),
            messages: BTreeMap::new(),
            cursor: 0,
        };
        client.send_frame(
            0,
            &serde_json::to_vec(&json!({"v":1,"client_id":app_id}))?,
            COMMAND_TIMEOUT,
        )?;
        let reply = client
            .read_json(COMMAND_TIMEOUT)?
            .ok_or(RpcError::Timeout)?;
        if reply.get("evt").and_then(Value::as_str) == Some("ERROR") {
            return Err(remote_error(&reply));
        }
        if reply.get("cmd").and_then(Value::as_str) != Some("DISPATCH")
            || reply.get("evt").and_then(Value::as_str) != Some("READY")
        {
            return Err(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::MC0158D384D67,
            )));
        }
        Ok(client)
    }

    /// `token` must be an OAuth access token obtained by the application flow.
    /// The token is not retained in RpcClient or emitted through errors/logging.
    pub fn authenticate(&mut self, token: &str) -> Result<Identity, RpcError> {
        if self.authenticated || self.channel.is_some() {
            return Err(RpcError::InvalidInput(crate::i18n::text(
                crate::i18n::Key::M6F413218D95C,
            )));
        }
        if token.is_empty() || token.len() > 8192 || token.chars().any(char::is_control) {
            return Err(RpcError::InvalidInput(crate::i18n::text(
                crate::i18n::Key::M7F86319CB45B,
            )));
        }
        let data = self.command("AUTHENTICATE", None, json!({"access_token":token}))?;
        if let Some(scopes) = data.get("scopes") {
            let scopes = scopes
                .as_array()
                .ok_or(RpcError::Protocol(crate::i18n::text(
                    crate::i18n::Key::M26E4458FFC74,
                )))?;
            if !["rpc", "messages.read"]
                .iter()
                .all(|required| scopes.iter().any(|scope| scope.as_str() == Some(required)))
            {
                return Err(RpcError::Remote {
                    code: None,
                    permission: true,
                });
            }
        }
        let user = data
            .get("user")
            .and_then(Value::as_object)
            .ok_or(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::MCDCBAEB7A774,
            )))?;
        let id = user
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| snowflake(s))
            .ok_or(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::MB25E1885D36D,
            )))?;
        let name = user
            .get("global_name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| user.get("username").and_then(Value::as_str))
            .unwrap_or("Discord");
        let identity = Identity {
            id: id.to_owned(),
            display_name: bounded(name, 256),
        };
        self.authenticated = true;
        Ok(identity)
    }

    /// Subscribe first, then load the local client's initial cache exactly once.
    /// Events interleaved with acknowledgements are replayed after that cache.
    /// Any failure must cause the caller to discard this connection.
    pub fn subscribe_and_snapshot(&mut self, channel_id: &str) -> Result<EventBatch, RpcError> {
        if !self.authenticated || self.channel.is_some() {
            return Err(RpcError::InvalidInput(crate::i18n::text(
                crate::i18n::Key::M4FCDC28F25BD,
            )));
        }
        if !snowflake(channel_id) {
            return Err(RpcError::InvalidInput(crate::i18n::text(
                crate::i18n::Key::M37DC8B6C42CF,
            )));
        }
        self.channel = Some(channel_id.to_owned());
        for event in ["MESSAGE_CREATE", "MESSAGE_UPDATE", "MESSAGE_DELETE"] {
            self.command("SUBSCRIBE", Some(event), json!({"channel_id":channel_id}))?;
        }
        let data = self.command("GET_CHANNEL", None, json!({"channel_id":channel_id}))?;
        if data.get("id").and_then(Value::as_str) != Some(channel_id) {
            return Err(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::M5B070BEA3EA4,
            )));
        }
        let initial = data
            .get("messages")
            .and_then(Value::as_array)
            .ok_or(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::MB719D666C93B,
            )))?;
        for raw in initial {
            if let Some(other) = raw.get("channel_id").and_then(Value::as_str) {
                if other != channel_id {
                    continue;
                }
            }
            let patch = MessagePatch::parse(raw)?;
            let mut message = CachedMessage::new(patch.id.clone());
            message.apply(patch);
            self.messages.insert(message.id.clone(), message);
            self.trim_cache();
        }
        while let Some(event) = self.queued.pop_front() {
            self.apply_event(event);
        }
        let events = self
            .messages
            .values()
            .filter_map(|m| m.display())
            .map(|message| ChatEvent::Upsert { message })
            .collect();
        self.ready = true;
        Ok(self.batch(true, events))
    }

    /// Waits for IPC data. A timeout is an ordinary idle state, not disconnection.
    pub fn next_batch(&mut self, timeout: Duration) -> Result<Option<EventBatch>, RpcError> {
        if !self.ready {
            return Err(RpcError::InvalidInput(crate::i18n::text(
                crate::i18n::Key::ME78930AA8EBF,
            )));
        }
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        loop {
            self.check_cancelled()?;
            let wait = if events.is_empty() {
                deadline.saturating_duration_since(Instant::now())
            } else {
                Duration::ZERO
            };
            let Some(payload) = self.read_json(wait)? else {
                break;
            };
            if payload.get("evt").and_then(Value::as_str) == Some("ERROR") {
                return Err(remote_error(&payload));
            }
            if let Some(event) = self.parse_event(&payload)? {
                if let Some(event) = self.apply_event(event) {
                    events.push(event);
                }
            }
            if events.len() >= 64 {
                break;
            }
        }
        if events.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.batch(false, events)))
        }
    }

    fn batch(&mut self, reset: bool, events: Vec<ChatEvent>) -> EventBatch {
        self.cursor = self.cursor.saturating_add(1);
        EventBatch {
            connection_epoch: 0,
            cursor: self.cursor,
            reset,
            connected: self.ready,
            events,
        }
    }

    fn command(&mut self, cmd: &str, evt: Option<&str>, args: Value) -> Result<Value, RpcError> {
        self.check_cancelled()?;
        self.nonce = self
            .nonce
            .checked_add(1)
            .ok_or(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::M3146E4A90F7F,
            )))?;
        let nonce = format!("sideglow-{}", self.nonce);
        let mut request = json!({"cmd":cmd,"args":args,"nonce":nonce});
        if let Some(evt) = evt {
            request["evt"] = json!(evt);
        }
        self.send_frame(1, &serde_json::to_vec(&request)?, COMMAND_TIMEOUT)?;
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut unrelated_replies = 0;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let reply = self.read_json(remaining)?.ok_or(RpcError::Timeout)?;
            let reply_nonce = reply.get("nonce").and_then(Value::as_str);
            if reply_nonce == Some(nonce.as_str()) {
                if reply.get("evt").and_then(Value::as_str) == Some("ERROR") {
                    return Err(remote_error(&reply));
                }
                if reply.get("cmd").and_then(Value::as_str) != Some(cmd)
                    || evt.is_some_and(|expected| {
                        reply
                            .get("data")
                            .and_then(|data| data.get("evt"))
                            .and_then(Value::as_str)
                            != Some(expected)
                    })
                {
                    return Err(RpcError::Protocol(crate::i18n::text(
                        crate::i18n::Key::M9BD0222DBCA2,
                    )));
                }
                return reply.get("data").filter(|v| v.is_object()).cloned().ok_or(
                    RpcError::Protocol(crate::i18n::text(crate::i18n::Key::M83015F6B718F)),
                );
            }
            if reply_nonce.is_some() {
                unrelated_replies += 1;
                if unrelated_replies > 32 {
                    return Err(RpcError::Protocol(crate::i18n::text(
                        crate::i18n::Key::M4301E177C1ED,
                    )));
                }
                continue;
            }
            if reply.get("evt").and_then(Value::as_str) == Some("ERROR") {
                return Err(remote_error(&reply));
            }
            if let Some(event) = self.parse_event(&reply)? {
                if self.queued.len() >= MAX_QUEUED_EVENTS {
                    return Err(RpcError::Protocol(crate::i18n::text(
                        crate::i18n::Key::M568EB83A5E16,
                    )));
                }
                self.queued.push_back(event);
            }
            if Instant::now() >= deadline {
                return Err(RpcError::Timeout);
            }
        }
    }

    fn parse_event(&self, payload: &Value) -> Result<Option<RawEvent>, RpcError> {
        if payload.get("cmd").and_then(Value::as_str) != Some("DISPATCH") {
            return Ok(None);
        }
        let Some(channel) = self.channel.as_deref() else {
            return Ok(None);
        };
        let Some(data) = payload.get("data") else {
            return Ok(None);
        };
        if data.get("channel_id").and_then(Value::as_str) != Some(channel) {
            return Ok(None);
        }
        let Some(kind) = payload.get("evt").and_then(Value::as_str) else {
            return Ok(None);
        };
        if !matches!(kind, "MESSAGE_CREATE" | "MESSAGE_UPDATE" | "MESSAGE_DELETE") {
            return Ok(None);
        }
        let message = data
            .get("message")
            .ok_or(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::M11961B99267B,
            )))?;
        if message
            .get("channel_id")
            .and_then(Value::as_str)
            .is_some_and(|v| v != channel)
        {
            return Ok(None);
        }
        let patch = MessagePatch::parse(message)?;
        Ok(Some(if kind == "MESSAGE_DELETE" {
            RawEvent::Delete(patch.id)
        } else {
            RawEvent::Upsert {
                patch: Box::new(patch),
                create: kind == "MESSAGE_CREATE",
            }
        }))
    }

    fn apply_event(&mut self, event: RawEvent) -> Option<ChatEvent> {
        match event {
            RawEvent::Delete(id) => {
                self.messages.remove(&id);
                Some(ChatEvent::Delete { id })
            }
            RawEvent::Upsert { patch, create } => {
                let id = patch.id.clone();
                if !create && !self.messages.contains_key(&id) {
                    return None;
                }
                let message = self
                    .messages
                    .entry(id.clone())
                    .or_insert_with(|| CachedMessage::new(id.clone()));
                message.apply(*patch);
                let output = message
                    .display()
                    .map(|message| ChatEvent::Upsert { message })
                    .unwrap_or_else(|| ChatEvent::Delete { id });
                self.trim_cache();
                Some(output)
            }
        }
    }

    fn trim_cache(&mut self) {
        if self.messages.len() > MAX_MESSAGES {
            let oldest = self
                .messages
                .values()
                .min_by(|a, b| {
                    a.timestamp
                        .cmp(&b.timestamp)
                        .then(a.id.len().cmp(&b.id.len()))
                        .then(a.id.cmp(&b.id))
                })
                .map(|m| m.id.clone());
            if let Some(id) = oldest {
                self.messages.remove(&id);
            }
        }
    }

    fn check_cancelled(&self) -> Result<(), RpcError> {
        if self.cancellation.load(Ordering::Acquire) {
            Err(RpcError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn send_frame(&mut self, opcode: u32, body: &[u8], timeout: Duration) -> Result<(), RpcError> {
        self.check_cancelled()?;
        if body.len() > MAX_FRAME {
            return Err(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::M474FA400BECD,
            )));
        }
        let mut bytes = Vec::with_capacity(8 + body.len());
        bytes.extend_from_slice(&opcode.to_le_bytes());
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(body);
        self.wire.write_all(&bytes, timeout)
    }

    fn read_json(&mut self, timeout: Duration) -> Result<Option<Value>, RpcError> {
        let deadline = Instant::now() + timeout;
        loop {
            self.check_cancelled()?;
            if let Some((opcode, body)) = self.decoder.take_frame()? {
                match opcode {
                    1 => {
                        let value: Value = serde_json::from_slice(&body)?;
                        if !value.is_object() || value.get("cmd").and_then(Value::as_str).is_none()
                        {
                            return Err(RpcError::Protocol(crate::i18n::text(
                                crate::i18n::Key::M5D3A91221417,
                            )));
                        }
                        return Ok(Some(value));
                    }
                    2 => {
                        if !body.is_empty() {
                            let data: Value = serde_json::from_slice(&body)?;
                            let error = remote_error(&json!({"data":data}));
                            if error.is_permission_denied() || error.is_authentication_required() {
                                return Err(error);
                            }
                        }
                        return Err(RpcError::Disconnected);
                    }
                    3 => self.send_frame(4, &body, COMMAND_TIMEOUT)?,
                    4 => (),
                    _ => {
                        return Err(RpcError::Protocol(crate::i18n::text(
                            crate::i18n::Key::MB959D5DEB3F8,
                        )))
                    }
                }
                continue;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let mut chunk = [0u8; 8192];
            let capacity = (MAX_FRAME + 8 - self.decoder.bytes.len()).min(chunk.len());
            if capacity == 0 {
                return Err(RpcError::Protocol(crate::i18n::text(
                    crate::i18n::Key::MBB0E8DC306B0,
                )));
            }
            match self.wire.read(&mut chunk[..capacity], remaining)? {
                None => return Ok(None),
                Some(0) => return Err(RpcError::Disconnected),
                Some(n) if n <= capacity => self.decoder.bytes.extend_from_slice(&chunk[..n]),
                Some(_) => {
                    return Err(RpcError::Protocol(crate::i18n::text(
                        crate::i18n::Key::MB5BB91DF4EC4,
                    )))
                }
            }
        }
    }
}

impl From<serde_json::Error> for RpcError {
    fn from(_: serde_json::Error) -> Self {
        Self::Protocol(crate::i18n::text(crate::i18n::Key::M5119A1EACDAD))
    }
}

#[derive(Default)]
struct Decoder {
    bytes: Vec<u8>,
}

impl Decoder {
    fn take_frame(&mut self) -> Result<Option<(u32, Vec<u8>)>, RpcError> {
        if self.bytes.len() < 8 {
            return Ok(None);
        }
        let opcode = u32::from_le_bytes(self.bytes[..4].try_into().unwrap());
        let size = u32::from_le_bytes(self.bytes[4..8].try_into().unwrap()) as usize;
        if size > MAX_FRAME {
            return Err(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::M474FA400BECD,
            )));
        }
        if self.bytes.len() < size + 8 {
            return Ok(None);
        }
        let body = self.bytes[8..size + 8].to_vec();
        self.bytes.drain(..size + 8);
        Ok(Some((opcode, body)))
    }
}

fn remote_error(payload: &Value) -> RpcError {
    let data = &payload["data"];
    let code = data.get("code").and_then(Value::as_i64);
    // Classify locally but do not copy Discord's message into logs or errors.
    let description = data
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let error_name = data.get("error").and_then(Value::as_str).unwrap_or("");
    if code == Some(4006)
        || matches!(error_name, "invalid_token" | "invalid_grant")
        || [
            "invalid_token",
            "invalid access token",
            "invalid token",
            "token expired",
            "expired token",
            "not authenticated",
        ]
        .iter()
        .any(|needle| description.contains(needle))
    {
        return RpcError::AuthenticationRequired { code };
    }
    let permission = matches!(
        error_name,
        "access_denied" | "insufficient_scope" | "invalid_scope"
    ) || [
        "scope",
        "permission",
        "access denied",
        "unauthorized",
        "not authorized",
    ]
    .iter()
    .any(|needle| description.contains(needle));
    RpcError::Remote { code, permission }
}

fn snowflake(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 20
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().is_ok_and(|n| n != 0)
}

fn bounded(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[derive(Debug)]
enum RawEvent {
    Upsert {
        patch: Box<MessagePatch>,
        create: bool,
    },
    Delete(String),
}

/// Retain only the fields displayed by the overlay, never arbitrarily large
/// content_parsed trees, image bytes, avatars, or other Discord caches.
#[derive(Debug, Default)]
struct MessagePatch {
    id: String,
    author_id: Option<String>,
    webhook_id: Option<Option<String>>,
    content: Option<String>,
    embeds: Option<String>,
    attachments: Option<String>,
    username: Option<String>,
    nickname: Option<Option<String>>,
    bot: Option<bool>,
    timestamp: Option<String>,
    parent_id: Option<Option<String>>,
    blocked: Option<bool>,
}

impl MessagePatch {
    fn parse(value: &Value) -> Result<Self, RpcError> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| snowflake(s))
            .ok_or(RpcError::Protocol(crate::i18n::text(
                crate::i18n::Key::M4D6194740E9C,
            )))?;
        let mut patch = Self {
            id: id.to_owned(),
            ..Self::default()
        };
        if let Some(content) = value.get("content") {
            patch.content = Some(bounded(
                content
                    .as_str()
                    .ok_or(RpcError::Protocol(crate::i18n::text(
                        crate::i18n::Key::MCBABA4586142,
                    )))?,
                MAX_TEXT,
            ));
        }
        if let Some(timestamp) = value.get("timestamp").and_then(Value::as_str) {
            patch.timestamp = Some(bounded(timestamp, 64));
        }
        if let Some(author) = value.get("author") {
            patch.author_id = author
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| snowflake(id))
                .map(str::to_owned);
            let name = author
                .get("global_name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| author.get("username").and_then(Value::as_str));
            patch.username = name.map(|s| bounded(s, 256));
            patch.bot = author.get("bot").and_then(Value::as_bool);
        }
        if let Some(webhook_id) = value.get("webhook_id") {
            patch.webhook_id = Some(
                webhook_id
                    .as_str()
                    .filter(|id| snowflake(id))
                    .map(str::to_owned),
            );
        }
        if let Some(nickname) = value.get("member").and_then(|m| m.get("nick")) {
            patch.nickname = Some(
                nickname
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| bounded(s, 256)),
            );
        }
        if let Some(reference) = value.get("message_reference") {
            patch.parent_id = Some(
                reference
                    .get("message_id")
                    .and_then(Value::as_str)
                    .filter(|s| snowflake(s))
                    .map(str::to_owned),
            );
        }
        patch.blocked = value.get("blocked").and_then(Value::as_bool);
        if let Some(embeds) = value.get("embeds") {
            let embeds = embeds
                .as_array()
                .ok_or(RpcError::Protocol(crate::i18n::text(
                    crate::i18n::Key::M61E632D8CB75,
                )))?;
            let mut text = String::new();
            for embed in embeds.iter().take(10) {
                for field in ["title", "description"] {
                    append_text(
                        &mut text,
                        embed.get(field).and_then(Value::as_str),
                        MAX_EMBED_TEXT,
                    );
                }
                if let Some(fields) = embed.get("fields").and_then(Value::as_array) {
                    for field in fields.iter().take(25) {
                        append_text(
                            &mut text,
                            field.get("name").and_then(Value::as_str),
                            MAX_EMBED_TEXT,
                        );
                        append_text(
                            &mut text,
                            field.get("value").and_then(Value::as_str),
                            MAX_EMBED_TEXT,
                        );
                    }
                }
            }
            patch.embeds = Some(text);
        }
        if let Some(attachments) = value.get("attachments") {
            let attachments =
                attachments
                    .as_array()
                    .ok_or(RpcError::Protocol(crate::i18n::text(
                        crate::i18n::Key::M6711950F0E45,
                    )))?;
            let mut text = String::new();
            for attachment in attachments.iter().take(10) {
                if let Some(name) = attachment.get("filename").and_then(Value::as_str) {
                    append_text(
                        &mut text,
                        Some(&format!("[첨부: {}]", bounded(name, 256))),
                        MAX_ATTACHMENT_TEXT,
                    );
                }
            }
            patch.attachments = Some(text);
        }
        Ok(patch)
    }
}

fn append_text(target: &mut String, text: Option<&str>, max: usize) {
    if let Some(text) = text.filter(|s| !s.is_empty()) {
        if target.len() >= max {
            return;
        }
        if !target.is_empty() {
            target.push('\n');
        }
        target.push_str(&bounded(text, max.saturating_sub(target.len())));
    }
}

struct CachedMessage {
    id: String,
    author_id: String,
    webhook_id: Option<String>,
    content: String,
    embeds: String,
    attachments: String,
    username: String,
    nickname: Option<String>,
    timestamp: String,
    parent_id: Option<String>,
    bot: bool,
    blocked: bool,
}

impl CachedMessage {
    fn new(id: String) -> Self {
        Self {
            id,
            author_id: String::new(),
            webhook_id: None,
            content: String::new(),
            embeds: String::new(),
            attachments: String::new(),
            username: "Discord".to_owned(),
            nickname: None,
            timestamp: String::new(),
            parent_id: None,
            bot: false,
            blocked: false,
        }
    }

    fn apply(&mut self, patch: MessagePatch) {
        if let Some(v) = patch.author_id {
            self.author_id = v;
        }
        if let Some(v) = patch.webhook_id {
            self.webhook_id = v;
        }
        if let Some(v) = patch.content {
            self.content = v;
        }
        if let Some(v) = patch.embeds {
            self.embeds = v;
        }
        if let Some(v) = patch.attachments {
            self.attachments = v;
        }
        if let Some(v) = patch.username {
            self.username = v;
        }
        if let Some(v) = patch.nickname {
            self.nickname = v;
        }
        if let Some(v) = patch.timestamp {
            self.timestamp = v;
        }
        if let Some(v) = patch.parent_id {
            self.parent_id = v;
        }
        if let Some(v) = patch.bot {
            self.bot = v;
        }
        if let Some(v) = patch.blocked {
            self.blocked = v;
        }
    }

    fn display(&self) -> Option<Message> {
        if self.blocked {
            return None;
        }
        let mut content = self.content.clone();
        append_text(&mut content, Some(&self.embeds), MAX_TEXT);
        append_text(&mut content, Some(&self.attachments), MAX_TEXT);
        Some(Message {
            id: self.id.clone(),
            parent_id: self.parent_id.clone(),
            author: self.nickname.as_ref().unwrap_or(&self.username).clone(),
            original_text: (content != self.content).then(|| self.content.clone()),
            content,
            timestamp: self.timestamp.clone(),
            bot: self.bot,
            author_id: self.author_id.clone(),
            webhook_id: self.webhook_id.clone(),
            translation: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockWire {
        chunks: VecDeque<Option<Vec<u8>>>,
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl Wire for MockWire {
        fn read(&mut self, buf: &mut [u8], _: Duration) -> Result<Option<usize>, RpcError> {
            match self.chunks.pop_front() {
                Some(Some(mut bytes)) => {
                    let n = buf.len().min(bytes.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    if n < bytes.len() {
                        self.chunks.push_front(Some(bytes.split_off(n)));
                    }
                    Ok(Some(n))
                }
                _ => Ok(None),
            }
        }
        fn write_all(&mut self, buf: &[u8], _: Duration) -> Result<(), RpcError> {
            self.writes.lock().unwrap().push(buf.to_owned());
            Ok(())
        }
    }

    fn frame(op: u32, bytes: &[u8]) -> Vec<u8> {
        [
            op.to_le_bytes().as_slice(),
            (bytes.len() as u32).to_le_bytes().as_slice(),
            bytes,
        ]
        .concat()
    }
    fn rpc(v: Value) -> Vec<u8> {
        frame(1, &serde_json::to_vec(&v).unwrap())
    }
    fn reply(cmd: &str, n: u32, data: Value) -> Vec<u8> {
        rpc(json!({"cmd":cmd,"nonce":format!("sideglow-{n}"),"data":data}))
    }
    fn ready() -> Vec<u8> {
        rpc(json!({"cmd":"DISPATCH","evt":"READY","data":{}}))
    }
    fn message(id: &str, text: &str) -> Value {
        json!({"id":id,"content":text,"timestamp":"2026-01-01T00:00:00Z","author":{"username":"Alice","bot":false}})
    }
    fn event(kind: &str, channel: &str, message: Value) -> Vec<u8> {
        rpc(json!({"cmd":"DISPATCH","evt":kind,"data":{"channel_id":channel,"message":message}}))
    }
    fn subscribe(n: u32, kind: &str) -> Vec<u8> {
        rpc(json!({"cmd":"SUBSCRIBE","nonce":format!("sideglow-{n}"),"data":{"evt":kind}}))
    }
    fn fixture(chunks: Vec<Option<Vec<u8>>>) -> (RpcClient, Arc<Mutex<Vec<Vec<u8>>>>) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let wire = MockWire {
            chunks: chunks.into(),
            writes: writes.clone(),
        };
        let client =
            RpcClient::start(Box::new(wire), "123", Arc::new(AtomicBool::new(false))).unwrap();
        (client, writes)
    }
    fn startup() -> Vec<Vec<u8>> {
        vec![
            ready(),
            reply(
                "AUTHENTICATE",
                1,
                json!({"user":{"id":"42","username":"Me"}}),
            ),
            subscribe(2, "MESSAGE_CREATE"),
            subscribe(3, "MESSAGE_UPDATE"),
            subscribe(4, "MESSAGE_DELETE"),
            reply(
                "GET_CHANNEL",
                5,
                json!({"id":"100","messages":[message("200","initial")]}),
            ),
        ]
    }

    #[test]
    fn fragmented_frames_nonce_matching_and_interleaved_snapshot_events() {
        let mut chunks = vec![
            ready(),
            reply(
                "AUTHENTICATE",
                1,
                json!({"user":{"id":"42","username":"Me"}}),
            ),
            event("MESSAGE_CREATE", "100", message("201", "new")),
            reply("IGNORED", 987, json!({})),
            subscribe(2, "MESSAGE_CREATE"),
            subscribe(3, "MESSAGE_UPDATE"),
            subscribe(4, "MESSAGE_DELETE"),
            event(
                "MESSAGE_UPDATE",
                "100",
                json!({"id":"200","content":"edited"}),
            ),
            event("MESSAGE_CREATE", "999", message("999", "foreign")),
            reply(
                "GET_CHANNEL",
                5,
                json!({"id":"100","messages":[message("200","initial")]}),
            ),
        ];
        let bytes: Vec<u8> = chunks.drain(..).flatten().collect();
        let (mut client, writes) = fixture(bytes.chunks(3).map(|v| Some(v.to_vec())).collect());
        assert!(!client.ready);
        assert_eq!(client.authenticate("oauth-test").unwrap().id, "42");
        assert!(!client.ready);
        let batch = client.subscribe_and_snapshot("100").unwrap();
        assert!(batch.reset && batch.connected);
        assert_eq!(batch.events.len(), 2);
        assert!(batch.events.iter().any(|e| matches!(e,ChatEvent::Upsert{message} if message.id=="200" && message.content=="edited" && message.author=="Alice")));
        let commands: Vec<Value> = writes
            .lock()
            .unwrap()
            .iter()
            .skip(1)
            .map(|b| serde_json::from_slice(&b[8..]).unwrap())
            .collect();
        assert_eq!(
            commands
                .iter()
                .filter(|v| v["cmd"] == "GET_CHANNEL")
                .count(),
            1
        );
        assert_eq!(
            commands.iter().filter(|v| v["cmd"] == "SUBSCRIBE").count(),
            3
        );
    }

    #[test]
    fn preserves_partial_frames_across_idle_timeout_and_handles_ping() {
        let update = event(
            "MESSAGE_UPDATE",
            "100",
            json!({"id":"200","content":"after timeout"}),
        );
        let mut chunks: Vec<_> = startup().into_iter().map(Some).collect();
        chunks.extend([
            Some(update[..11].to_vec()),
            None,
            Some(update[11..].to_vec()),
            Some(frame(3, b"ping bytes")),
            None,
        ]);
        let (mut client, writes) = fixture(chunks);
        client.authenticate("oauth-test").unwrap();
        client.subscribe_and_snapshot("100").unwrap();
        assert!(client.next_batch(Duration::from_secs(1)).unwrap().is_none());
        let batch = client.next_batch(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(batch.events.len(), 1);
        client.next_batch(Duration::from_secs(1)).unwrap();
        assert!(writes
            .lock()
            .unwrap()
            .iter()
            .any(|v| v == &frame(4, b"ping bytes")));
        // Idle waits never issue GET_CHANNEL or any other periodic request.
        let writes_before = writes.lock().unwrap().len();
        assert!(client.next_batch(Duration::from_secs(1)).unwrap().is_none());
        assert_eq!(writes.lock().unwrap().len(), writes_before);
    }

    #[test]
    fn permission_error_is_safe_and_never_marks_connected() {
        let secret = "TOKEN_SHOULD_NEVER_APPEAR";
        let (mut client, _) = fixture(vec![
            Some(ready()),
            Some(rpc(
                json!({"cmd":"AUTHENTICATE","nonce":"sideglow-1","evt":"ERROR","data":{"code":5000,"message":format!("No permission {secret}")}}),
            )),
        ]);
        let error = client.authenticate(secret).unwrap_err();
        assert!(error.is_permission_denied());
        assert!(!error.to_string().contains(secret));
        assert!(!format!("{error:?}").contains(secret));
        assert!(!client.ready && !client.authenticated);
    }

    #[test]
    fn subscription_rejection_does_not_report_connected() {
        let (mut client, _) = fixture(vec![
            Some(ready()),
            Some(reply("AUTHENTICATE", 1, json!({"user":{"id":"42"}}))),
            Some(rpc(
                json!({"cmd":"SUBSCRIBE","nonce":"sideglow-2","evt":"ERROR","data":{"code":5000,"message":"Missing scope messages.read"}}),
            )),
        ]);
        client.authenticate("oauth-test").unwrap();
        assert!(client
            .subscribe_and_snapshot("100")
            .unwrap_err()
            .is_permission_denied());
        assert!(!client.ready);
    }

    #[test]
    fn rejects_oversized_and_malformed_frames_before_parsing() {
        let mut decoder = Decoder {
            bytes: [1u32.to_le_bytes(), ((MAX_FRAME + 1) as u32).to_le_bytes()].concat(),
        };
        assert!(decoder.take_frame().is_err());
        let (mut client, _) = fixture(vec![Some(ready()), Some(frame(1, b"not json"))]);
        assert!(matches!(
            client.authenticate("oauth-test"),
            Err(RpcError::Protocol(_))
        ));
        let (mut client, _) = fixture(vec![Some(ready()), Some(frame(99, b""))]);
        assert!(matches!(
            client.authenticate("oauth-test"),
            Err(RpcError::Protocol(_))
        ));
    }

    #[test]
    fn deletes_partial_edits_blocked_and_foreign_messages() {
        let (mut client, _) = fixture(startup().into_iter().map(Some).collect());
        client.authenticate("oauth-test").unwrap();
        client.subscribe_and_snapshot("100").unwrap();
        let partial = client.parse_event(&json!({"cmd":"DISPATCH","evt":"MESSAGE_UPDATE","data":{"channel_id":"100","message":{"id":"200","content":"edited"}}})).unwrap().unwrap();
        let result = client.apply_event(partial).unwrap();
        assert!(
            matches!(result,ChatEvent::Upsert{message} if message.author=="Alice" && message.content=="edited")
        );
        assert!(client
            .apply_event(RawEvent::Upsert {
                patch: MessagePatch::parse(&json!({"id":"300","content":"unknown edit"}))
                    .unwrap()
                    .into(),
                create: false
            })
            .is_none());
        assert!(client.parse_event(&json!({"cmd":"DISPATCH","evt":"MESSAGE_CREATE","data":{"channel_id":"999","message":{"id":"400"}}})).unwrap().is_none());
        assert!(
            matches!(client.apply_event(RawEvent::Upsert{patch:MessagePatch::parse(&json!({"id":"200","blocked":true})).unwrap().into(),create:false}),Some(ChatEvent::Delete{id}) if id=="200")
        );
        assert!(
            matches!(client.apply_event(RawEvent::Delete("200".into())),Some(ChatEvent::Delete{id}) if id=="200")
        );
        assert!(!client.messages.contains_key("200"));
    }

    #[test]
    fn partial_edits_preserve_human_and_webhook_author_identity() {
        let (mut client, _) = fixture(vec![Some(ready())]);
        for (id, bot, webhook) in [("200", false, None), ("201", true, Some("42"))] {
            let mut value = message(id, "original");
            value["author"] = json!({"id":"42","username":"Alice","bot":bot});
            if let Some(webhook) = webhook {
                value["webhook_id"] = json!(webhook);
            }
            client.apply_event(RawEvent::Upsert {
                patch: MessagePatch::parse(&value).unwrap().into(),
                create: true,
            });
            let event = client
                .apply_event(RawEvent::Upsert {
                    patch: MessagePatch::parse(&json!({"id":id,"content":"edited"}))
                        .unwrap()
                        .into(),
                    create: false,
                })
                .unwrap();
            let ChatEvent::Upsert { message } = event else {
                panic!("expected a partial edit");
            };
            assert_eq!(message.author_id, "42");
            assert_eq!(message.webhook_id.as_deref(), webhook);
            assert_eq!(message.bot, bot);
            assert_eq!(message.author, "Alice");
            assert_eq!(message.source_text(), "edited");
        }
    }

    #[test]
    fn embed_and_attachment_updates_do_not_change_translation_source() {
        let mut cached = CachedMessage::new("200".into());
        cached.apply(MessagePatch::parse(&message("200", "read https://example.com/")).unwrap());
        let original = cached.display().unwrap();
        assert!(original.original_text.is_none());
        let revision = crate::translation::wire::revision(original.source_text());
        cached.apply(
            MessagePatch::parse(&json!({
                "id":"200",
                "embeds":[{"title":"preview title","description":"preview description"}],
                "attachments":[{"filename":"map.png"}]
            }))
            .unwrap(),
        );
        let preview = cached.display().unwrap();
        assert!(preview.content.contains("preview title"));
        assert!(preview.content.contains("map.png"));
        assert_eq!(preview.source_text(), original.source_text());
        assert_eq!(
            crate::translation::wire::revision(preview.source_text()),
            revision
        );

        cached
            .apply(MessagePatch::parse(&json!({"id":"200","embeds":[],"attachments":[]})).unwrap());
        let removed = cached.display().unwrap();
        assert_eq!(removed.content, original.content);
        assert!(removed.original_text.is_none());
        assert_eq!(
            crate::translation::wire::revision(removed.source_text()),
            revision
        );

        cached.apply(MessagePatch::parse(&json!({"id":"200","content":"new source"})).unwrap());
        assert_ne!(
            crate::translation::wire::revision(cached.display().unwrap().source_text()),
            revision
        );
    }

    #[test]
    fn cache_and_projected_text_are_bounded() {
        let (mut client, _) = fixture(vec![Some(ready())]);
        for id in 1..=MAX_MESSAGES + 40 {
            client.apply_event(RawEvent::Upsert {
                patch: MessagePatch::parse(&message(&id.to_string(), "text"))
                    .unwrap()
                    .into(),
                create: true,
            });
        }
        assert_eq!(client.messages.len(), MAX_MESSAGES);
        assert!(!client.messages.contains_key("1"));
        assert!(client
            .messages
            .contains_key(&(MAX_MESSAGES + 40).to_string()));
        let patch = MessagePatch::parse(&message("999", &"한글".repeat(MAX_TEXT))).unwrap();
        assert!(patch.content.as_ref().unwrap().len() <= MAX_TEXT);
        let mut cached = CachedMessage::new("999".into());
        cached.apply(patch);
        assert!(cached.display().unwrap().content.len() <= MAX_TEXT);
    }

    #[test]
    fn queue_overflow_cancels_initialization_instead_of_losing_messages() {
        let mut chunks = vec![
            Some(ready()),
            Some(reply("AUTHENTICATE", 1, json!({"user":{"id":"42"}}))),
        ];
        for id in 1..=MAX_QUEUED_EVENTS + 1 {
            chunks.push(Some(event(
                "MESSAGE_CREATE",
                "100",
                message(&id.to_string(), "queued"),
            )));
        }
        let (mut client, _) = fixture(chunks);
        client.authenticate("oauth-test").unwrap();
        assert!(matches!(
            client.subscribe_and_snapshot("100"),
            Err(RpcError::Protocol(_))
        ));
        assert!(!client.ready);
        assert_eq!(client.queued.len(), MAX_QUEUED_EVENTS);
    }

    #[test]
    fn cancellation_is_reported_without_further_io() {
        let (mut client, writes) = fixture(vec![Some(ready())]);
        client.cancellation.store(true, Ordering::Release);
        assert!(client
            .authenticate("oauth-test")
            .unwrap_err()
            .is_cancelled());
        assert_eq!(writes.lock().unwrap().len(), 1);
    }

    #[test]
    fn expired_authentication_is_distinct_from_permission_denial() {
        for data in [
            json!({"code":4006}),
            json!({"error":"invalid_token"}),
            json!({"code":5000,"message":"Invalid access token SECRET"}),
        ] {
            let (mut client, _) = fixture(vec![
                Some(ready()),
                Some(rpc(json!({
                    "cmd":"AUTHENTICATE", "nonce":"sideglow-1", "evt":"ERROR", "data":data
                }))),
            ]);
            let error = client.authenticate("SECRET").unwrap_err();
            assert!(error.is_authentication_required());
            assert!(!error.is_permission_denied());
            assert!(!format!("{error:?} {error}").contains("SECRET"));
            assert!(!client.authenticated && !client.ready);
        }
    }

    #[test]
    fn authentication_scopes_must_allow_rpc_and_messages_when_reported() {
        for scopes in [
            json!(["rpc", "identify"]),
            json!(["messages.read"]),
            json!([]),
        ] {
            let (mut client, _) = fixture(vec![
                Some(ready()),
                Some(reply(
                    "AUTHENTICATE",
                    1,
                    json!({"user":{"id":"42"},"scopes":scopes}),
                )),
            ]);
            assert!(client
                .authenticate("oauth-test")
                .unwrap_err()
                .is_permission_denied());
            assert!(!client.authenticated);
        }
        let (mut client, _) = fixture(vec![
            Some(ready()),
            Some(reply(
                "AUTHENTICATE",
                1,
                json!({"user":{"id":"42"},"scopes":["rpc","messages.read"]}),
            )),
        ]);
        client.authenticate("oauth-test").unwrap();
        assert!(client.authenticated && !client.ready);
    }

    #[test]
    fn close_frame_retains_auth_or_permission_classification_without_payload() {
        let (mut client, _) = fixture(vec![
            Some(ready()),
            Some(frame(
                2,
                br#"{"code":5000,"message":"Missing scope SECRET"}"#,
            )),
        ]);
        let error = client.authenticate("SECRET").unwrap_err();
        assert!(error.is_permission_denied());
        assert!(!format!("{error:?} {error}").contains("SECRET"));
        let (mut client, _) = fixture(vec![Some(ready()), Some(frame(2, b"{}"))]);
        assert!(client
            .authenticate("oauth-test")
            .unwrap_err()
            .is_disconnected());
    }

    #[test]
    fn matching_nonce_must_also_match_command_and_subscription_event() {
        let (mut client, _) = fixture(vec![
            Some(ready()),
            Some(reply("GET_CHANNEL", 1, json!({}))),
        ]);
        assert!(matches!(
            client.authenticate("oauth-test"),
            Err(RpcError::Protocol(_))
        ));
        let (mut client, _) = fixture(vec![
            Some(ready()),
            Some(reply("AUTHENTICATE", 1, json!({"user":{"id":"42"}}))),
            Some(subscribe(2, "MESSAGE_DELETE")),
        ]);
        client.authenticate("oauth-test").unwrap();
        assert!(matches!(
            client.subscribe_and_snapshot("100"),
            Err(RpcError::Protocol(_))
        ));
        assert!(!client.ready);
    }
}
