// SPDX-License-Identifier: MPL-2.0
use crate::{
    config::{self, ConnectionMode, Endpoint, Settings},
    model::{EventBatch, Message},
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
mod direct;
pub use crate::translation::TranslationTiming;

#[derive(PartialEq, Eq)]
enum ConnectionIdentity {
    Relay(String, String, String),
    Rpc(String, String, String),
}
impl ConnectionIdentity {
    fn from_settings(s: &Settings) -> Self {
        match s.connection_mode {
            ConnectionMode::Relay => Self::Relay(
                s.relay_url.trim().into(),
                s.channel_url.trim().into(),
                s.access_key.clone(),
            ),
            ConnectionMode::Rpc => Self::Rpc(
                s.discord_app_id.trim().into(),
                s.channel_url.trim().into(),
                s.webhook_url.trim().into(),
            ),
        }
    }
}

#[derive(Debug)]
pub enum ClientEvent {
    Batch(EventBatch),
    /// Routine progress/state for settings, never an overlay footer line.
    Notice(String),
    /// An error or required user action that must remain visible.
    Status(String),
    SendResult {
        text: String,
        result: Result<(), String>,
        confirmed: Option<Box<Message>>,
        timing: Option<SendTiming>,
    },
    TranslationResult {
        message: Box<Message>,
        status: String,
    },
}

/// Local duration measurements only. No message text, URLs, IDs or credentials.
#[derive(Clone, Debug)]
pub struct SendTiming {
    pub queue_ms: u64,
    pub verify_ms: u64,
    pub post_ms: u64,
    pub http_send_ms: Option<u64>,
    pub http_response_ms: Option<u64>,
    pub http_body_ms: Option<u64>,
    pub worker_ms: u64,
    pub ui_queue_ms: u64,
    pub succeeded: bool,
    finished_at: Instant,
}
impl SendTiming {
    pub fn total_ms(&self) -> u64 {
        self.queue_ms
            .saturating_add(self.worker_ms)
            .saturating_add(self.ui_queue_ms)
    }

    pub fn summary(&self) -> String {
        {
            let __ui_arg0 = &(if self.succeeded {
                crate::i18n::text(crate::i18n::Key::M223B4A2C877B)
            } else {
                crate::i18n::text(crate::i18n::Key::M2274ACC3EF3C)
            });
            let __ui_arg1 = &(self.total_ms() as f64 / 1000.0);
            let __ui_arg2 = &(self.queue_ms as f64 / 1000.0);
            let __ui_arg3 = &(self.verify_ms as f64 / 1000.0);
            let __ui_arg4 = &(self.post_ms as f64 / 1000.0);
            crate::i18n::format(
                crate::i18n::Key::M26DD9791E5EA,
                &[
                    ("arg0", __ui_arg0.to_string()),
                    ("arg1", format!("{:.2}", __ui_arg1)),
                    ("arg2", format!("{:.2}", __ui_arg2)),
                    ("arg3", format!("{:.2}", __ui_arg3)),
                    ("arg4", format!("{:.2}", __ui_arg4)),
                ],
            )
        }
    }

    pub fn details(&self) -> String {
        let phases = match (self.http_send_ms, self.http_response_ms, self.http_body_ms) {
            (Some(send), Some(response), Some(body)) => crate::i18n::format(
                crate::i18n::Key::M78A21A92B54B,
                &[
                    ("send", format!("{}", send)),
                    ("response", format!("{}", response)),
                    ("body", format!("{}", body)),
                ],
            ),
            _ => crate::i18n::text(crate::i18n::Key::M8817BFB776F8),
        };
        {
            let __ui_arg0 = &(if self.succeeded {
                crate::i18n::text(crate::i18n::Key::MAC5BD4F0569F)
            } else {
                crate::i18n::text(crate::i18n::Key::M0314BEEF9C97)
            });
            let __ui_arg1 = &(self.total_ms());
            let __ui_arg2 = &(self.queue_ms);
            let __ui_arg3 = &(self.verify_ms);
            let __ui_arg4 = &(self.post_ms);
            let __ui_arg5 = &(self.ui_queue_ms);
            let __ui_arg6 = &(phases);
            crate::i18n::format(
                crate::i18n::Key::MF21D7736F477,
                &[
                    ("arg0", __ui_arg0.to_string()),
                    ("arg1", format!("{}", __ui_arg1)),
                    ("arg2", format!("{}", __ui_arg2)),
                    ("arg3", format!("{}", __ui_arg3)),
                    ("arg4", format!("{}", __ui_arg4)),
                    ("arg5", format!("{}", __ui_arg5)),
                    ("arg6", __ui_arg6.to_string()),
                ],
            )
        }
    }
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

struct Runtime {
    generation: AtomicU64,
    shutdown: AtomicBool,
}
struct OutgoingMessage {
    text: String,
    nonce: String,
    queued_at: Instant,
    romaji_japanese_input: bool,
}
enum Outgoing {
    Message(OutgoingMessage),
    PrepareWebhook,
}

#[derive(Default)]
struct DirectReadiness {
    receive: AtomicBool,
    send: AtomicBool,
    connecting: AtomicBool,
    session_epoch: AtomicU64,
}
impl DirectReadiness {
    fn begin_connecting(&self) {
        // Retrying is still one pending setup attempt. Never publish an idle gap
        // that could make the UI lose ownership of its cancelable operation.
        self.send.store(false, Ordering::Release);
        self.receive.store(false, Ordering::Release);
        self.session_epoch.fetch_add(1, Ordering::AcqRel);
        self.connecting.store(true, Ordering::Release);
    }

    fn clear(&self) {
        self.send.store(false, Ordering::Release);
        self.receive.store(false, Ordering::Release);
        self.session_epoch.fetch_add(1, Ordering::AcqRel);
        self.connecting.store(false, Ordering::Release);
    }

    fn available_epoch(&self) -> Option<u64> {
        let epoch = self.session_epoch.load(Ordering::Acquire);
        if epoch == 0 || !self.receive.load(Ordering::Acquire) || !self.send.load(Ordering::Acquire)
        {
            return None;
        }
        (self.session_epoch.load(Ordering::Acquire) == epoch).then_some(epoch)
    }
}

pub struct Client {
    runtime: Arc<Runtime>,
    tx: SyncSender<(u64, ClientEvent)>,
    rx: Receiver<(u64, ClientEvent)>,
    outgoing: Option<SyncSender<Outgoing>>,
    identity: Option<ConnectionIdentity>,
    direct_cancel: Option<Arc<AtomicBool>>,
    direct_ready: Option<Arc<DirectReadiness>>,
    nonce: AtomicU64,
    pending: Vec<ClientEvent>,
    last_send: Option<SendTiming>,
    translator: crate::translation::Controller,
    translation_origin: Option<(String, String)>,
    romaji_japanese_input: bool,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}
impl Client {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel(32);
        Self {
            runtime: Arc::new(Runtime {
                generation: AtomicU64::new(0),
                shutdown: AtomicBool::new(false),
            }),
            tx,
            rx,
            outgoing: None,
            identity: None,
            direct_cancel: None,
            direct_ready: None,
            nonce: AtomicU64::new(1),
            pending: Vec::new(),
            last_send: None,
            translator: crate::translation::Controller::default(),
            translation_origin: None,
            romaji_japanese_input: false,
        }
    }

    pub fn configure(&mut self, s: &Settings) {
        self.configure_internal(s, false);
        self.configure_translation(s);
    }

    pub fn last_send_timing(&self) -> Option<&SendTiming> {
        self.last_send.as_ref()
    }

    pub fn last_translation_timing(&self) -> Option<&TranslationTiming> {
        self.translator.last_timing()
    }

    pub fn login_discord(&mut self, s: &Settings) -> Result<(), String> {
        if s.connection_mode != ConnectionMode::Rpc {
            return Err(crate::i18n::text(crate::i18n::Key::MCA8E65481DA8));
        }
        direct::validate_required(s)?;
        self.identity = None;
        self.configure_internal(s, true);
        self.configure_translation(s);
        Ok(())
    }

    /// Explicit retry using already approved credentials; never opens a browser.
    pub fn reconnect_discord(&mut self, s: &Settings) -> Result<(), String> {
        if s.connection_mode != ConnectionMode::Rpc {
            return Err(crate::i18n::text(crate::i18n::Key::M9A16419F2F26));
        }
        direct::validate_required(s)?;
        self.identity = None;
        self.configure_internal(s, false);
        self.configure_translation(s);
        Ok(())
    }

    /// Only true after authentication, all channel subscriptions, and the initial snapshot.
    /// Callers displaying draft settings must also match their app ID/channel to the saved settings.
    pub fn is_discord_receive_ready(&self) -> bool {
        self.direct_ready
            .as_ref()
            .is_some_and(|ready| ready.receive.load(Ordering::Acquire))
    }

    /// Receipt permission alone does not authorize/enqueue a webhook send.
    pub fn is_discord_send_ready(&self) -> bool {
        self.is_discord_receive_ready()
            && self
                .direct_ready
                .as_ref()
                .is_some_and(|ready| ready.send.load(Ordering::Acquire))
    }

    pub fn is_discord_connecting(&self) -> bool {
        self.direct_ready
            .as_ref()
            .is_some_and(|ready| ready.connecting.load(Ordering::Acquire))
    }

    /// Stops this attempt without removing approved OAuth credentials or saved settings.
    pub fn cancel_discord_login(&mut self) {
        if !matches!(self.identity, Some(ConnectionIdentity::Rpc(..))) {
            return;
        }
        self.stop_direct();
        self.runtime.generation.fetch_add(1, Ordering::SeqCst);
        self.outgoing = None;
        self.pending.clear();
        self.pending.push(ClientEvent::Batch(EventBatch {
            connection_epoch: 0,
            cursor: 0,
            reset: true,
            connected: false,
            events: vec![],
        }));
        self.pending.push(ClientEvent::Notice(crate::i18n::text(
            crate::i18n::Key::M16DEFABFDA99,
        )));
    }

    pub fn logout_discord(&mut self, s: &Settings) -> Result<(), String> {
        if s.connection_mode != ConnectionMode::Rpc {
            return Err(crate::i18n::text(crate::i18n::Key::M7BADDF58605F));
        }
        self.stop_direct();
        self.runtime.generation.fetch_add(1, Ordering::SeqCst);
        self.outgoing = None;
        self.pending.clear();
        self.pending.push(ClientEvent::Batch(EventBatch {
            connection_epoch: 0,
            cursor: 0,
            reset: true,
            connected: false,
            events: vec![],
        }));
        // Cancellation precedes the token-store lock, so an old login cannot restore a deleted token.
        crate::discord_auth::clear()?;
        self.identity = Some(ConnectionIdentity::from_settings(s));
        self.pending.push(ClientEvent::Notice(crate::i18n::text(
            crate::i18n::Key::M0324B706CAFE,
        )));
        Ok(())
    }

    fn stop_direct(&mut self) {
        self.translator.stop();
        if let Some(cancel) = self.direct_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        if let Some(ready) = self.direct_ready.take() {
            ready.clear();
        }
    }

    fn configure_translation(&mut self, s: &Settings) {
        self.translation_origin = if s.connection_mode == ConnectionMode::Rpc {
            config::channel_id(&s.channel_url).ok().zip(
                crate::discord_http::Webhook::parse(&s.webhook_url)
                    .ok()
                    .map(|webhook| webhook.id().to_owned()),
            )
        } else {
            None
        };
        if !s.translation.enabled {
            self.translator.configure(None);
            return;
        }
        let Some(ready) = self.direct_ready.clone() else {
            self.translator.configure(None);
            self.pending.push(ClientEvent::Status(crate::i18n::text(
                crate::i18n::Key::M5F35D2EB75CE,
            )));
            return;
        };
        let runtime = self.runtime.clone();
        let generation = runtime.generation.load(Ordering::Acquire);
        let available: crate::translation::Guard = Arc::new(move || {
            current(&runtime, generation)
                .then(|| ready.available_epoch())
                .flatten()
        });
        match crate::translation::Spec::new(s, generation, available) {
            Ok(spec) => self.translator.configure(Some(spec)),
            Err(error) => {
                self.translator.configure(None);
                self.pending.push(ClientEvent::Status(crate::i18n::format(
                    crate::i18n::Key::M26F8804B28C3,
                    &[("error", error.to_string())],
                )));
            }
        }
    }

    fn configure_internal(&mut self, s: &Settings, interactive: bool) {
        // A sender preference takes effect on future enqueues without reconnecting
        // Discord or restarting the channel's independent translation workers.
        self.romaji_japanese_input = s.romaji_japanese_input;
        let identity = ConnectionIdentity::from_settings(s);
        if self.identity.as_ref() == Some(&identity) {
            return;
        }
        self.identity = Some(identity);
        self.stop_direct();
        self.last_send = None;
        let generation = self.runtime.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.outgoing = None;
        self.pending.clear();
        self.pending.push(ClientEvent::Batch(EventBatch {
            connection_epoch: 0,
            cursor: 0,
            reset: true,
            connected: false,
            events: vec![],
        }));
        if s.connection_mode == ConnectionMode::Rpc {
            if let Err(e) = direct::validate_required(s) {
                self.pending.push(ClientEvent::Status(e));
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let ready = Arc::new(DirectReadiness::default());
            ready.begin_connecting();
            self.direct_cancel = Some(cancel.clone());
            self.direct_ready = Some(ready.clone());
            match direct::start(
                self.runtime.clone(),
                self.tx.clone(),
                generation,
                s.clone(),
                interactive,
                cancel,
                ready,
            ) {
                Ok(sender) => self.outgoing = Some(sender),
                Err(error) => {
                    if let Some(ready) = &self.direct_ready {
                        ready.clear();
                    }
                    self.pending.push(ClientEvent::Status(error));
                }
            }
            return;
        }
        if s.relay_url.trim().is_empty()
            || s.channel_url.trim().is_empty()
            || s.access_key.is_empty()
        {
            self.pending.push(ClientEvent::Status(crate::i18n::text(
                crate::i18n::Key::M9A9B7250EE67,
            )));
            return;
        }
        let params = (|| -> Result<_, String> {
            config::validate(s)?;
            Ok((
                config::endpoint(&s.relay_url)?,
                config::channel_id(&s.channel_url)?,
            ))
        })();
        let (endpoint, channel) = match params {
            Ok(v) => v,
            Err(e) => {
                self.pending.push(ClientEvent::Status(e));
                return;
            }
        };
        let receive_runtime = self.runtime.clone();
        let receive_tx = self.tx.clone();
        let receive_endpoint = endpoint.clone();
        let receive_channel = channel.clone();
        let receive_key = s.access_key.clone();
        let receiver = std::thread::Builder::new()
            .name("relay-receive".into())
            .spawn(move || {
                receive_loop(
                    receive_runtime,
                    receive_tx,
                    generation,
                    receive_endpoint,
                    receive_channel,
                    receive_key,
                )
            });
        if receiver.is_err() {
            self.pending.push(ClientEvent::Status(crate::i18n::text(
                crate::i18n::Key::M8EE6D0FA762E,
            )));
            return;
        }
        let (send_tx, send_rx) = mpsc::sync_channel::<Outgoing>(4);
        let runtime = self.runtime.clone();
        let tx = self.tx.clone();
        let key = s.access_key.clone();
        let sender = std::thread::Builder::new()
            .name("relay-send".into())
            .spawn(move || {
                let mut http: Option<NativeHttp> = None;
                while current(&runtime, generation) {
                    let request = match send_rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(Outgoing::Message(r)) => r,
                        Ok(Outgoing::PrepareWebhook) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    };
                    if !current(&runtime, generation) {
                        break;
                    }
                    let result = (|| -> Result<(), String> {
                        if http.is_none() {
                            http = Some(NativeHttp::new(&endpoint)?);
                        }
                        let body = serde_json::to_vec(
                            &serde_json::json!({"content":request.text,"nonce":request.nonce,
                                "romaji_japanese_input":request.romaji_japanese_input}),
                        )
                        .map_err(|_| crate::i18n::text(crate::i18n::Key::M9EBA0E91B90A))?;
                        // A potentially successful POST is not retried automatically.
                        let response = http
                            .as_mut()
                            .unwrap()
                            .request("POST", "/v1/messages", &key, &channel, Some(&body))
                            .map_err(|e| {
                                crate::i18n::format(
                                    crate::i18n::Key::ME606E31D4EFA,
                                    &[("e", e.to_string())],
                                )
                            })?;
                        check_status(&response, &key)?;
                        let parsed: serde_json::Value = serde_json::from_slice(&response.body)
                            .map_err(|_| crate::i18n::text(crate::i18n::Key::M281F124AF2E7))?;
                        if parsed
                            .get("id")
                            .and_then(|v| v.as_str())
                            .filter(|id| !id.is_empty())
                            .is_none()
                        {
                            return Err(crate::i18n::text(crate::i18n::Key::M59FA48D2F5FB));
                        }
                        Ok(())
                    })();
                    if result.is_err() {
                        http = None;
                    }
                    if tx
                        .send((
                            generation,
                            ClientEvent::SendResult {
                                text: request.text,
                                result,
                                confirmed: None,
                                timing: None,
                            },
                        ))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        if sender.is_ok() {
            self.outgoing = Some(send_tx);
        } else {
            self.pending.push(ClientEvent::Status(crate::i18n::text(
                crate::i18n::Key::M88294BB7E4E1,
            )));
        }
    }

    pub fn send(&self, text: String) -> Result<(), String> {
        validate_message(&text)?;
        if self.direct_ready.is_some() {
            if !self.is_discord_receive_ready() {
                return Err(crate::i18n::text(crate::i18n::Key::MEF564E5D8E8E));
            }
            if !self.is_discord_send_ready() {
                return Err(crate::i18n::text(crate::i18n::Key::M45C922D5EE6F));
            }
        }
        let tx = self
            .outgoing
            .as_ref()
            .ok_or(crate::i18n::text(crate::i18n::Key::MEDDD96E44FF2))?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let nonce = format!(
            "{:x}-{:x}-{:x}",
            std::process::id(),
            stamp,
            self.nonce.fetch_add(1, Ordering::Relaxed)
        );
        tx.try_send(Outgoing::Message(OutgoingMessage {
            text,
            nonce,
            queued_at: Instant::now(),
            romaji_japanese_input: self.romaji_japanese_input,
        }))
        .map_err(|e| match e {
            mpsc::TrySendError::Full(_) => crate::i18n::text(crate::i18n::Key::MDCA25A1E592F),
            mpsc::TrySendError::Disconnected(_) => {
                crate::i18n::text(crate::i18n::Key::M7EEE958755F0)
            }
        })
    }

    pub fn drain(&mut self) -> Vec<ClientEvent> {
        let generation = self.runtime.generation.load(Ordering::Acquire);
        let mut events = std::mem::take(&mut self.pending);
        events.extend(self.rx.try_iter().take(64).filter_map(|(g, event)| {
            if g == generation {
                Some(event)
            } else {
                None
            }
        }));
        // A reconnect can complete before queued events from the old local RPC
        // session reach the UI. Never feed those batches to display or translation.
        events.retain(|event| match event {
            ClientEvent::Batch(batch) if batch.connection_epoch != 0 => {
                self.direct_ready.as_ref().is_some_and(|ready| {
                    ready.session_epoch.load(Ordering::Acquire) == batch.connection_epoch
                })
            }
            _ => true,
        });
        for event in &mut events {
            if let Some((channel, webhook)) = &self.translation_origin {
                match event {
                    ClientEvent::Batch(batch) => {
                        for event in &mut batch.events {
                            if let crate::model::ChatEvent::Upsert { message } = event {
                                crate::translation::wire::normalize(message, channel, webhook);
                            }
                        }
                    }
                    ClientEvent::SendResult {
                        confirmed: Some(message),
                        ..
                    } => {
                        crate::translation::wire::normalize(message, channel, webhook);
                    }
                    _ => {}
                }
            }
            if let ClientEvent::Batch(batch) = event {
                self.translator.observe(batch);
            }
            if let ClientEvent::SendResult {
                timing: Some(timing),
                ..
            } = event
            {
                timing.ui_queue_ms = elapsed_ms(timing.finished_at.elapsed());
                self.last_send = Some(timing.clone());
            }
        }
        events.extend(self.translator.poll().into_iter().map(|event| match event {
            crate::translation::Event::Notice(status) => ClientEvent::Notice(status),
            crate::translation::Event::Status(status) => ClientEvent::Status(status),
            crate::translation::Event::Published {
                message, status, ..
            } => ClientEvent::TranslationResult { message, status },
        }));
        events
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop_direct();
        self.runtime.shutdown.store(true, Ordering::Release);
        self.runtime.generation.fetch_add(1, Ordering::SeqCst);
    }
}

fn validate_message(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err(crate::i18n::text(crate::i18n::Key::M8A4B8005593A));
    }
    if text.encode_utf16().count() > 2000 {
        return Err(crate::i18n::text(crate::i18n::Key::MC76B73DA12D1));
    }
    if text.contains('\0') {
        return Err(crate::i18n::text(crate::i18n::Key::M04800716096B));
    }
    Ok(())
}
fn current(runtime: &Runtime, generation: u64) -> bool {
    !runtime.shutdown.load(Ordering::Acquire)
        && runtime.generation.load(Ordering::Acquire) == generation
}
fn pause(runtime: &Runtime, generation: u64, millis: u64) {
    for _ in 0..millis.div_ceil(100) {
        if !current(runtime, generation) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn receive_loop(
    runtime: Arc<Runtime>,
    tx: SyncSender<(u64, ClientEvent)>,
    generation: u64,
    endpoint: Endpoint,
    channel: String,
    key: String,
) {
    let mut cursor = 0u64;
    let mut failures = 0u32;
    let mut previous_status = String::new();
    let mut http: Option<NativeHttp> = None;
    while current(&runtime, generation) {
        let result = (|| -> Result<EventBatch, String> {
            if http.is_none() {
                http = Some(NativeHttp::new(&endpoint)?);
            }
            let response = http.as_mut().unwrap().request(
                "GET",
                &format!("/v1/events?after={cursor}&wait=25000"),
                &key,
                &channel,
                None,
            )?;
            check_status(&response, &key)?;
            let batch: EventBatch = serde_json::from_slice(&response.body)
                .map_err(|_| crate::i18n::text(crate::i18n::Key::MD8016810E9AB))?;
            if batch.events.len() > 2000 {
                return Err(crate::i18n::text(crate::i18n::Key::M5231D670CE34));
            }
            Ok(batch)
        })();
        match result {
            Ok(batch) => {
                failures = 0;
                cursor = batch.cursor;
                let status = if batch.connected {
                    crate::i18n::text(crate::i18n::Key::M2E5C29479B13)
                } else {
                    crate::i18n::text(crate::i18n::Key::MBE7C96F92310)
                };
                if previous_status != status {
                    previous_status.clone_from(&status);
                    if tx.send((generation, ClientEvent::Notice(status))).is_err() {
                        break;
                    }
                }
                if tx.send((generation, ClientEvent::Batch(batch))).is_err() {
                    break;
                }
                // Bound work even if a misconfigured peer answers empty polls immediately.
                pause(&runtime, generation, 100);
            }
            Err(error) => {
                http = None;
                failures = (failures + 1).min(5);
                let status = crate::i18n::format(
                    crate::i18n::Key::MEAAB90ECAF24,
                    &[("error", error.to_string())],
                );
                if previous_status != status {
                    previous_status = status.clone();
                    if tx.send((generation, ClientEvent::Status(status))).is_err() {
                        break;
                    }
                }
                pause(&runtime, generation, (1u64 << failures).min(30) * 1000);
            }
        }
    }
}

struct HttpResponse {
    status: u32,
    body: Vec<u8>,
}
fn check_status(response: &HttpResponse, key: &str) -> Result<(), String> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let base = match response.status {
        401 => crate::i18n::text(crate::i18n::Key::M10606D875C72),
        403 => crate::i18n::text(crate::i18n::Key::M40554B3A21A9),
        404 => crate::i18n::text(crate::i18n::Key::M0A01FBB94525),
        429 => crate::i18n::text(crate::i18n::Key::M98F29797198C),
        300..=399 => crate::i18n::text(crate::i18n::Key::MB6D1D0FEC789),
        _ => crate::i18n::text(crate::i18n::Key::M637319B70D01),
    };
    let detail = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
        .map(|s| {
            s.replace(key, "[redacted]")
                .chars()
                .filter(|c| !c.is_control())
                .take(180)
                .collect::<String>()
        })
        .unwrap_or_default();
    Err(if detail.is_empty() {
        format!("{base} (HTTP {})", response.status)
    } else {
        format!("{base} {detail} (HTTP {})", response.status)
    })
}

#[cfg(windows)]
use native::NativeHttp;
#[cfg(windows)]
mod native {
    use super::{Endpoint, HttpResponse};
    use std::{
        ffi::c_void,
        ptr::{null, null_mut},
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
    pub(super) struct NativeHttp {
        connection: Handle,
        _session: Handle,
        secure: bool,
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn failure(stage: &str) -> String {
        format!("{stage}: {}", std::io::Error::last_os_error())
    }
    impl NativeHttp {
        pub(super) fn new(endpoint: &Endpoint) -> Result<Self, String> {
            unsafe {
                let agent = wide(concat!("Sideglow/", env!("CARGO_PKG_VERSION")));
                let local = endpoint.host == "localhost"
                    || endpoint.host == "127.0.0.1"
                    || endpoint.host == "::1";
                let raw = WinHttpOpen(
                    agent.as_ptr(),
                    if local {
                        WINHTTP_ACCESS_TYPE_NO_PROXY
                    } else {
                        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
                    },
                    null(),
                    null(),
                    0,
                );
                if raw.is_null() {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::ME70ED7709386)));
                }
                let session = Handle(raw);
                if WinHttpSetTimeouts(session.0, 5000, 7000, 10000, 35000) == 0 {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M8DF7A8A1B339)));
                }
                let host = wide(&endpoint.host);
                let raw = WinHttpConnect(session.0, host.as_ptr(), endpoint.port, 0);
                if raw.is_null() {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M5F0DC4FCB415)));
                }
                Ok(Self {
                    _session: session,
                    connection: Handle(raw),
                    secure: endpoint.secure,
                })
            }
        }
        pub(super) fn request(
            &mut self,
            method: &str,
            path: &str,
            key: &str,
            channel: &str,
            body: Option<&[u8]>,
        ) -> Result<HttpResponse, String> {
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
                    if self.secure { WINHTTP_FLAG_SECURE } else { 0 },
                );
                if raw.is_null() {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M354925AC572A)));
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
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M6440A8B97DD6)));
                }
                let autologon = WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH;
                if WinHttpSetOption(
                    request.0,
                    WINHTTP_OPTION_AUTOLOGON_POLICY,
                    &autologon as *const _ as *const c_void,
                    4,
                ) == 0
                {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M91922BFD56E6)));
                }
                let headers=wide(&format!("Authorization: Bearer {key}\r\nX-Discord-Channel: {channel}\r\nContent-Type: application/json; charset=utf-8\r\nAccept: application/json\r\nCache-Control: no-store\r\n"));
                let body = body.unwrap_or(&[]);
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
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M3F5B4BA3088D)));
                }
                if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M0B761E48B65A)));
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
                    return Err(failure(&crate::i18n::text(crate::i18n::Key::M85C15DB5C735)));
                }
                let mut data = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let mut read = 0u32;
                    if WinHttpReadData(
                        request.0,
                        buffer.as_mut_ptr() as *mut c_void,
                        buffer.len() as u32,
                        &mut read,
                    ) == 0
                    {
                        return Err(failure(&crate::i18n::text(crate::i18n::Key::M67F17E0CA8FD)));
                    }
                    if read == 0 {
                        break;
                    }
                    if data.len() + read as usize > 16 * 1024 * 1024 {
                        return Err(crate::i18n::text(crate::i18n::Key::M590E9311D254));
                    }
                    data.extend_from_slice(&buffer[..read as usize]);
                }
                Ok(HttpResponse { status, body: data })
            }
        }
    }
}

#[cfg(not(windows))]
struct NativeHttp;
#[cfg(not(windows))]
impl NativeHttp {
    fn new(_: &Endpoint) -> Result<Self, String> {
        Err(crate::i18n::text(crate::i18n::Key::M68F40882632B))
    }
    fn request(
        &mut self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: Option<&[u8]>,
    ) -> Result<HttpResponse, String> {
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discord_utf16_limit_handles_emoji() {
        assert!(validate_message(&"😀".repeat(1000)).is_ok());
        assert!(validate_message(&"😀".repeat(1001)).is_err());
        assert!(validate_message(" \t\n").is_err());
        assert!(validate_message("안녕하세요").is_ok());
    }
    #[test]
    fn disconnected_send_is_explicit_not_silently_dropped() {
        assert!(Client::new()
            .send("hello".into())
            .unwrap_err()
            .contains("설정"));
    }

    #[test]
    fn reconnect_discards_stamped_old_batches_before_display_or_translation() {
        let mut client = Client::new();
        let ready = Arc::new(DirectReadiness::default());
        ready.begin_connecting();
        ready.receive.store(true, Ordering::Release);
        ready.send.store(true, Ordering::Release);
        let old_epoch = ready.available_epoch().unwrap();
        client.direct_ready = Some(ready.clone());
        client
            .tx
            .try_send((
                0,
                ClientEvent::Batch(EventBatch {
                    connection_epoch: old_epoch,
                    cursor: 10,
                    connected: true,
                    ..EventBatch::default()
                }),
            ))
            .unwrap();
        ready.begin_connecting();
        assert!(ready.available_epoch().is_none());
        ready.receive.store(true, Ordering::Release);
        ready.send.store(true, Ordering::Release);
        let new_epoch = ready.available_epoch().unwrap();
        assert!(new_epoch > old_epoch);
        client
            .tx
            .try_send((
                0,
                ClientEvent::Batch(EventBatch {
                    connection_epoch: new_epoch,
                    cursor: 20,
                    reset: true,
                    connected: true,
                    ..EventBatch::default()
                }),
            ))
            .unwrap();
        // Local configuration resets are unstamped and must still be delivered.
        client.pending.push(ClientEvent::Batch(EventBatch {
            cursor: 30,
            reset: true,
            ..EventBatch::default()
        }));
        let cursors: Vec<_> = client
            .drain()
            .into_iter()
            .filter_map(|event| match event {
                ClientEvent::Batch(batch) => Some(batch.cursor),
                _ => None,
            })
            .collect();
        assert_eq!(cursors, vec![30, 20]);
        ready.clear();
        assert!(ready.available_epoch().is_none());
    }

    #[test]
    fn read_permission_check_does_not_require_a_webhook() {
        let s = Settings {
            discord_app_id: "123456789012345678".into(),
            channel_url: "https://discord.com/channels/123456789012345678/234567890123456789"
                .into(),
            ..Default::default()
        };
        assert!(direct::validate_required(&s).is_ok());
        let without_channel = Settings {
            channel_url: String::new(),
            ..s
        };
        assert!(direct::validate_required(&without_channel).is_err());
    }

    #[test]
    fn cancelling_setup_prevents_late_connection_success_from_reappearing() {
        let mut client = Client::new();
        client.identity = Some(ConnectionIdentity::Rpc(
            "123456789012345678".into(),
            "234567890123456789".into(),
            String::new(),
        ));
        let old_ready = Arc::new(DirectReadiness::default());
        old_ready.connecting.store(true, Ordering::Release);
        client.direct_ready = Some(old_ready.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        client.direct_cancel = Some(cancel.clone());
        assert!(client.is_discord_connecting());
        client.cancel_discord_login();
        assert!(cancel.load(Ordering::Acquire));
        assert!(!client.is_discord_connecting());
        // A completion that was already in flight belongs to the cancelled generation.
        old_ready.receive.store(true, Ordering::Release);
        client
            .tx
            .try_send((
                0,
                ClientEvent::Batch(EventBatch {
                    connection_epoch: 0,
                    cursor: 1,
                    reset: true,
                    connected: true,
                    events: vec![],
                }),
            ))
            .unwrap();
        assert!(!client.is_discord_receive_ready());
        let events = client.drain();
        assert!(events
            .iter()
            .any(|e| matches!(e,ClientEvent::Batch(b) if b.reset&&!b.connected)));
        assert!(!events
            .iter()
            .any(|e| matches!(e,ClientEvent::Batch(b) if b.connected)));
    }

    #[test]
    fn receive_success_cannot_enqueue_a_send_until_webhook_is_verified() {
        let mut client = Client::new();
        let ready = Arc::new(DirectReadiness::default());
        let (tx, rx) = mpsc::sync_channel(4);
        client.outgoing = Some(tx);
        client.direct_ready = Some(ready.clone());
        ready.receive.store(true, Ordering::Release);
        assert!(client.is_discord_receive_ready());
        assert!(!client.is_discord_send_ready());
        assert!(client.send("not sent".into()).is_err());
        assert!(rx.try_recv().is_err());
        ready.send.store(true, Ordering::Release);
        assert!(client.send("ready".into()).is_ok());
        assert!(matches!(rx.try_recv().unwrap(), Outgoing::Message(r) if r.text == "ready"));
        client.stop_direct();
        assert!(!client.is_discord_receive_ready());
        assert!(!ready.send.load(Ordering::Acquire));
    }
    #[test]
    fn old_connection_events_do_not_leak_into_new_channel() {
        let mut client = Client::new();
        client.configure(&Settings::default());
        client
            .tx
            .try_send((0, ClientEvent::Status("stale".into())))
            .unwrap();
        assert!(!client
            .drain()
            .iter()
            .any(|e| matches!(e,ClientEvent::Status(s) if s=="stale")));
    }

    #[test]
    fn sender_policy_is_captured_per_message_without_reconnecting() {
        let mut client = Client::new();
        let mut settings = Settings::default();
        client.identity = Some(ConnectionIdentity::from_settings(&settings));
        let (tx, rx) = mpsc::sync_channel(4);
        client.outgoing = Some(tx);
        let original_generation = client.runtime.generation.load(Ordering::Acquire);

        client.configure(&settings);
        client.send("same text".into()).unwrap();
        settings.romaji_japanese_input = true;
        client.configure(&settings);
        client.send("same text".into()).unwrap();
        settings.romaji_japanese_input = false;
        client.configure(&settings);

        for expected in [false, true] {
            let Outgoing::Message(request) = rx.try_recv().unwrap() else {
                panic!("unexpected preparation request");
            };
            assert_eq!(request.text, "same text");
            assert_eq!(request.romaji_japanese_input, expected);
        }
        assert_eq!(
            client.runtime.generation.load(Ordering::Acquire),
            original_generation
        );
        assert!(client.pending.is_empty());
        assert!(client.outgoing.is_some());
    }
    #[test]
    fn disconnect_reset_survives_a_saturated_old_event_queue() {
        let mut client = Client::new();
        for _ in 0..32 {
            client
                .tx
                .try_send((0, ClientEvent::Status("old connected state".into())))
                .unwrap();
        }
        client.configure(&Settings::default());
        let events = client.drain();
        assert!(matches!(events.first(),Some(ClientEvent::Batch(b)) if b.reset && !b.connected));
        assert!(events
            .iter()
            .any(|e| matches!(e,ClientEvent::Status(s) if s.contains("설정"))));
        assert!(!events
            .iter()
            .any(|e| matches!(e,ClientEvent::Status(s) if s.contains("old connected"))));
    }
    #[test]
    fn server_error_cannot_echo_secret_in_ui() {
        let key = "secret_0123456789";
        let response = HttpResponse {
            status: 403,
            body: format!("{{\"error\":\"bad token {key}\"}}").into_bytes(),
        };
        let error = check_status(&response, key).unwrap_err();
        assert!(!error.contains(key));
        assert!(error.contains("[redacted]"));
    }
}
