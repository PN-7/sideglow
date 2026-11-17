// SPDX-License-Identifier: MPL-2.0
//! B transport: one local RPC receiver and a bounded, on-demand HTTPS sender.
use super::{current, elapsed_ms, ClientEvent, DirectReadiness, Outgoing, Runtime, SendTiming};
use crate::{
    config::{self, Settings},
    discord_auth::{self, Tokens},
    discord_http::{DiscordHttp, Webhook},
    model::EventBatch,
    rpc::{RpcClient, RpcError},
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub(super) fn validate_required(s: &Settings) -> Result<(), String> {
    config::validate(s)?;
    if s.discord_app_id.trim().is_empty() || s.channel_url.trim().is_empty() {
        return Err(crate::i18n::text(crate::i18n::Key::ME977FDFD26F6));
    }
    Ok(())
}

struct Context {
    runtime: Arc<Runtime>,
    tx: SyncSender<(u64, ClientEvent)>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    ready: Arc<DirectReadiness>,
    receiver_finished: AtomicBool,
    rpc_epoch: AtomicU64,
    pending_webhook_check: AtomicU64,
    receiver_gate: Mutex<()>,
    display_name: Mutex<String>,
}
impl Context {
    fn active(&self) -> bool {
        !self.cancel.load(Ordering::Acquire) && current(&self.runtime, self.generation)
    }
    fn emit(&self, mut event: ClientEvent) -> bool {
        if let ClientEvent::Batch(batch) = &mut event {
            batch.connection_epoch = self.ready.session_epoch.load(Ordering::Acquire);
        }
        while self.active() {
            match self.tx.try_send((self.generation, event)) {
                Ok(()) => return true,
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
                Err(mpsc::TrySendError::Full((_, returned))) => event = returned,
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
    fn status(&self, s: impl Into<String>) -> bool {
        self.emit(ClientEvent::Status(s.into()))
    }
    fn notice(&self, s: impl Into<String>) -> bool {
        self.emit(ClientEvent::Notice(s.into()))
    }
    fn disconnected(&self) {
        let _guard = self.receiver_gate.lock().unwrap_or_else(|p| p.into_inner());
        self.rpc_epoch.fetch_add(1, Ordering::AcqRel);
        self.pending_webhook_check.store(0, Ordering::Release);
        self.ready.begin_connecting();
        self.emit(ClientEvent::Batch(EventBatch {
            connection_epoch: 0,
            cursor: 0,
            reset: false,
            connected: false,
            events: vec![],
        }));
    }
    fn finish_receiver(&self) {
        let _guard = self.receiver_gate.lock().unwrap_or_else(|p| p.into_inner());
        self.receiver_finished.store(true, Ordering::Release);
        self.rpc_epoch.fetch_add(1, Ordering::AcqRel);
        self.pending_webhook_check.store(0, Ordering::Release);
        self.ready.clear();
    }
    fn current_receiver(&self, epoch: u64) -> bool {
        self.active()
            && !self.receiver_finished.load(Ordering::Acquire)
            && self.ready.receive.load(Ordering::Acquire)
            && self.rpc_epoch.load(Ordering::Acquire) == epoch
    }
    fn pause(&self, seconds: u64) {
        for _ in 0..seconds * 10 {
            if !self.active() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn finish_webhook_check(context: &Context, epoch: u64, result: Result<(), String>) {
    let _guard = context
        .receiver_gate
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if !context.current_receiver(epoch) {
        return;
    }
    match result {
        Ok(()) => {
            context.ready.send.store(true, Ordering::Release);
            context.notice(crate::i18n::text(crate::i18n::Key::M53307172E197));
        }
        Err(error) => {
            context.ready.send.store(false, Ordering::Release);
            context.status(crate::i18n::format(
                crate::i18n::Key::MB10570AA4999,
                &[("error", error.to_string())],
            ));
        }
    }
    context.ready.connecting.store(false, Ordering::Release);
}

fn prepare_webhook(
    context: &Context,
    webhook: Option<&Webhook>,
    channel: &str,
    http: &mut Option<DiscordHttp>,
) {
    let epoch = context.pending_webhook_check.swap(0, Ordering::AcqRel);
    if epoch == 0 || !context.current_receiver(epoch) {
        return;
    }
    let Some(webhook) = webhook else { return };
    let result = (|| {
        if http.is_none() {
            *http = Some(DiscordHttp::new()?);
        }
        webhook.verify(http.as_mut().unwrap(), channel).map(|_| ())
    })();
    if result.is_err() {
        *http = None;
    }
    finish_webhook_check(context, epoch, result);
}

pub(super) fn start(
    runtime: Arc<Runtime>,
    tx: SyncSender<(u64, ClientEvent)>,
    generation: u64,
    settings: Settings,
    interactive: bool,
    cancel: Arc<AtomicBool>,
    ready: Arc<DirectReadiness>,
) -> Result<SyncSender<Outgoing>, String> {
    let channel = config::channel_id(&settings.channel_url)?;
    // The setup guide proves read permission before asking the owner to create/share a webhook.
    let webhook = if settings.webhook_url.trim().is_empty() {
        None
    } else {
        Some(Webhook::parse(&settings.webhook_url)?)
    };
    let context = Arc::new(Context {
        runtime,
        tx,
        generation,
        cancel,
        ready,
        receiver_finished: AtomicBool::new(false),
        rpc_epoch: AtomicU64::new(1),
        pending_webhook_check: AtomicU64::new(0),
        receiver_gate: Mutex::new(()),
        display_name: Mutex::new(String::new()),
    });
    let (send_tx, send_rx) = mpsc::sync_channel::<Outgoing>(4);
    let sender_context = context.clone();
    let sender_webhook = webhook.clone();
    let sender_channel = channel.clone();
    std::thread::Builder::new()
        .name("discord-webhook-send".into())
        .spawn(move || {
            // Setup verification and actual sends share one worker-owned session.
            // No raw handles cross threads and no extra networking worker is needed.
            let mut http: Option<DiscordHttp> = None;
            while sender_context.active() {
                prepare_webhook(
                    &sender_context,
                    sender_webhook.as_ref(),
                    &sender_channel,
                    &mut http,
                );
                let request = match send_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(Outgoing::Message(request)) => request,
                    Ok(Outgoing::PrepareWebhook) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if sender_context.receiver_finished.load(Ordering::Acquire) {
                            break;
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                if !sender_context.active() {
                    break;
                }
                let started = Instant::now();
                let mut timing = SendTiming {
                    queue_ms: elapsed_ms(started.saturating_duration_since(request.queued_at)),
                    verify_ms: 0,
                    post_ms: 0,
                    http_send_ms: None,
                    http_response_ms: None,
                    http_body_ms: None,
                    worker_ms: 0,
                    ui_queue_ms: 0,
                    succeeded: false,
                    finished_at: started,
                };
                let mut confirmed = None;
                let mut recreate_http = false;
                let result = (|| -> Result<(), String> {
                    let epoch = sender_context.rpc_epoch.load(Ordering::Acquire);
                    if !sender_context.current_receiver(epoch)
                        || !sender_context.ready.send.load(Ordering::Acquire)
                    {
                        return Err(crate::i18n::text(crate::i18n::Key::M3650575887DD));
                    }
                    let webhook = sender_webhook
                        .as_ref()
                        .ok_or(crate::i18n::text(crate::i18n::Key::M94C62A5BB6D8))?;
                    sender_context.notice(crate::i18n::text(crate::i18n::Key::MF14DC2ED7191));
                    let verify_started = Instant::now();
                    let verified = (|| {
                        if http.is_none() {
                            http = Some(DiscordHttp::new()?);
                        }
                        webhook.verify(http.as_mut().unwrap(), &sender_channel)
                    })();
                    timing.verify_ms = elapsed_ms(verify_started.elapsed());
                    if let Err(error) = verified {
                        recreate_http = true;
                        if sender_context.current_receiver(epoch) {
                            sender_context.ready.send.store(false, Ordering::Release);
                            sender_context.status(crate::i18n::format(
                                crate::i18n::Key::M12F895EE62E1,
                                &[("error", error.to_string())],
                            ));
                        }
                        return Err(error);
                    }
                    if !sender_context.current_receiver(epoch)
                        || !sender_context.ready.send.load(Ordering::Acquire)
                    {
                        return Err(crate::i18n::text(crate::i18n::Key::M666EA7ACD537));
                    }
                    let name = sender_context
                        .display_name
                        .lock()
                        .map_err(|_| crate::i18n::text(crate::i18n::Key::MC2900C29DB96))?
                        .clone();
                    sender_context.notice(crate::i18n::text(crate::i18n::Key::MB49A47D8133A));
                    let post_started = Instant::now();
                    let sent = webhook.send_with_input_policy(
                        http.as_mut().unwrap(),
                        &sender_channel,
                        &request.text,
                        &name,
                        request.romaji_japanese_input,
                    );
                    timing.post_ms = elapsed_ms(post_started.elapsed());
                    match sent {
                        Ok(receipt) => {
                            timing.http_send_ms = Some(receipt.timing.send_ms);
                            timing.http_response_ms = Some(receipt.timing.response_ms);
                            timing.http_body_ms = Some(receipt.timing.body_ms);
                            confirmed = Some(Box::new(receipt.message));
                            Ok(())
                        }
                        Err(error) => {
                            recreate_http = error.recreate_http;
                            if error.recheck_webhook && sender_context.current_receiver(epoch) {
                                sender_context.ready.send.store(false, Ordering::Release);
                            }
                            Err(error.reason)
                        }
                    }
                })();
                if recreate_http {
                    http = None;
                }
                timing.succeeded = result.is_ok();
                timing.worker_ms = elapsed_ms(started.elapsed());
                timing.finished_at = Instant::now();
                if !sender_context.emit(ClientEvent::SendResult {
                    text: request.text,
                    result,
                    confirmed,
                    timing: Some(timing),
                }) {
                    break;
                }
            }
        })
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M3BE4694F14E5))?;

    let receive_context = context.clone();
    let wake_sender = send_tx.clone();
    if std::thread::Builder::new()
        .name("discord-rpc-receive".into())
        .spawn(move || {
            let result = receive_loop(
                &receive_context,
                settings.discord_app_id.trim(),
                &channel,
                webhook.as_ref(),
                interactive,
                &wake_sender,
            );
            // The sender may still be finishing a metadata request. Atomically
            // invalidate its epoch before it can publish a late ready state.
            receive_context.finish_receiver();
            if let Err(e) = result {
                if receive_context.active() {
                    receive_context.emit(ClientEvent::Batch(EventBatch {
                        connection_epoch: 0,
                        cursor: 0,
                        reset: false,
                        connected: false,
                        events: vec![],
                    }));
                    receive_context.status(e);
                }
            }
            // Let a completed POST report its outcome and queued drafts receive failure results.
            // The sender then exits once drained, without an idle thread after a terminal login failure.
        })
        .is_err()
    {
        context.cancel.store(true, Ordering::Release);
        return Err(crate::i18n::text(crate::i18n::Key::M1F8892734ECE));
    }
    Ok(send_tx)
}

fn receive_loop(
    context: &Context,
    app_id: &str,
    channel: &str,
    webhook: Option<&Webhook>,
    interactive: bool,
    wake_sender: &SyncSender<Outgoing>,
) -> Result<(), String> {
    if !context.active() {
        return Ok(());
    }
    let mut tokens = if interactive {
        context.notice(crate::i18n::text(crate::i18n::Key::MB469321DE769));
        discord_auth::authorize(app_id, &context.cancel)?
    } else {
        match discord_auth::load(app_id)? {
            Some(tokens) => tokens,
            None => {
                context.status(crate::i18n::text(crate::i18n::Key::M2024137831B6));
                return Ok(());
            }
        }
    };
    let mut backoff = 1u64;
    let mut refresh_attempted = false;
    let mut saved = !interactive;
    while context.active() {
        context.ready.begin_connecting();
        if tokens.needs_refresh() {
            context.disconnected();
            context.notice(crate::i18n::text(crate::i18n::Key::MDD66B6203E3A));
            tokens = refresh_and_save(context, &tokens)?;
            saved = true;
        }
        context.notice(crate::i18n::text(crate::i18n::Key::MF8771CFCC813));
        let connection = (|| -> Result<(RpcClient, String, EventBatch), RpcError> {
            let mut rpc = RpcClient::connect(app_id, context.cancel.clone())?;
            let identity = rpc.authenticate(&tokens.access_token)?;
            let initial = rpc.subscribe_and_snapshot(channel)?;
            Ok((rpc, identity.display_name, initial))
        })();
        let (mut rpc, name, initial) = match connection {
            Ok(v) => v,
            Err(e) => {
                if e.is_cancelled() || !context.active() {
                    return Ok(());
                }
                context.disconnected();
                if e.is_authentication_required() {
                    if refresh_attempted {
                        return Err(crate::i18n::text(crate::i18n::Key::MABA59DFEB324));
                    }
                    context.notice(crate::i18n::text(crate::i18n::Key::MB0B5B465B241));
                    tokens = refresh_and_save(context, &tokens)?;
                    refresh_attempted = true;
                    saved = true;
                    continue;
                }
                if e.is_permission_denied() {
                    return Err(crate::i18n::format(
                        crate::i18n::Key::M494C49C412D7,
                        &[("e", format!("{}", e))],
                    ));
                }
                if e.is_disconnected() || matches!(e, RpcError::Timeout | RpcError::Io(_)) {
                    context.status(crate::i18n::format(
                        crate::i18n::Key::MD79677ACA954,
                        &[("e", format!("{}", e))],
                    ));
                    backoff = (backoff * 2).min(30);
                    context.pause(backoff);
                    continue;
                }
                return Err(e.to_string());
            }
        };
        if !context.active() {
            return Ok(());
        }
        if !saved {
            discord_auth::save(&tokens, &context.cancel)?;
            saved = true;
        }
        *context
            .display_name
            .lock()
            .map_err(|_| crate::i18n::text(crate::i18n::Key::MC2900C29DB96))? = name;
        if !context.emit(ClientEvent::Batch(initial)) {
            return Ok(());
        }
        context.ready.receive.store(true, Ordering::Release);
        context.ready.send.store(false, Ordering::Release);
        if webhook.is_some() {
            context.notice(crate::i18n::text(crate::i18n::Key::MD80FA0F6FF1D));
            context
                .pending_webhook_check
                .store(context.rpc_epoch.load(Ordering::Acquire), Ordering::Release);
            // A full queue already wakes the worker. The pending epoch is the
            // source of truth; a wake signal must never block RPC reception.
            let _ = wake_sender.try_send(Outgoing::PrepareWebhook);
        } else {
            context.notice(crate::i18n::text(crate::i18n::Key::M26FCB5CC7EA0));
            context.ready.connecting.store(false, Ordering::Release);
        }
        backoff = 1;
        let connected_at = Instant::now();
        while context.active() && !tokens.needs_refresh() {
            // Blocking pipe receive: no timer-based GET_CHANNEL, Discord REST history polling, or image fetches.
            match rpc.next_batch(Duration::from_secs(30)) {
                Ok(Some(batch)) => {
                    if !context.emit(ClientEvent::Batch(batch)) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    if e.is_cancelled() || !context.active() {
                        return Ok(());
                    }
                    context.disconnected();
                    if e.is_permission_denied() {
                        return Err(crate::i18n::format(
                            crate::i18n::Key::M279F8E77CEF9,
                            &[("e", format!("{}", e))],
                        ));
                    }
                    if e.is_authentication_required() {
                        if refresh_attempted {
                            return Err(crate::i18n::text(crate::i18n::Key::MD5F6D4EFD824));
                        }
                        tokens = refresh_and_save(context, &tokens)?;
                        saved = true;
                        refresh_attempted = true;
                    } else if e.is_disconnected()
                        || matches!(e, RpcError::Timeout | RpcError::Io(_))
                    {
                        context.status(crate::i18n::format(
                            crate::i18n::Key::M42F47B95F3C7,
                            &[("e", format!("{}", e))],
                        ));
                        context.pause(2);
                    } else {
                        return Err(e.to_string());
                    }
                    break;
                }
            }
            // A handshake alone is not evidence of a healthy session. This prevents a
            // rapid AUTHENTICATE → immediate auth-error → refresh/reconnect loop.
            if connected_at.elapsed() >= Duration::from_secs(60) {
                refresh_attempted = false;
            }
        }
        context.disconnected();
    }
    Ok(())
}

fn refresh_and_save(context: &Context, tokens: &Tokens) -> Result<Tokens, String> {
    let fresh = discord_auth::refresh(tokens, &context.cancel)?;
    // Refresh tokens may rotate: persist the replacement before using the new session.
    discord_auth::save(&fresh, &context.cancel)?;
    Ok(fresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> (Context, mpsc::Receiver<(u64, ClientEvent)>) {
        let (tx, rx) = mpsc::sync_channel(32);
        let context = Context {
            runtime: Arc::new(Runtime {
                generation: AtomicU64::new(7),
                shutdown: AtomicBool::new(false),
            }),
            tx,
            generation: 7,
            cancel: Arc::new(AtomicBool::new(false)),
            ready: Arc::new(DirectReadiness::default()),
            receiver_finished: AtomicBool::new(false),
            rpc_epoch: AtomicU64::new(1),
            pending_webhook_check: AtomicU64::new(0),
            receiver_gate: Mutex::new(()),
            display_name: Mutex::new(String::new()),
        };
        context.ready.begin_connecting();
        context.ready.receive.store(true, Ordering::Release);
        context.ready.connecting.store(true, Ordering::Release);
        (context, rx)
    }

    #[test]
    fn direct_batches_are_stamped_before_reconnect() {
        let (context, events) = context();
        let before = context.ready.session_epoch.load(Ordering::Acquire);
        assert!(before > 0);
        assert!(context.emit(ClientEvent::Batch(EventBatch {
            connected: true,
            ..EventBatch::default()
        })));
        context.disconnected();
        let after = context.ready.session_epoch.load(Ordering::Acquire);
        assert!(after > before);
        let batches: Vec<_> = events
            .try_iter()
            .filter_map(|(_, event)| match event {
                ClientEvent::Batch(batch) => Some(batch),
                _ => None,
            })
            .collect();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].connection_epoch, before);
        assert_eq!(batches[1].connection_epoch, after);
        assert!(!batches[1].connected);
    }

    #[test]
    fn stale_webhook_check_cannot_complete_a_reconnecting_session() {
        let (context, events) = context();
        let old_epoch = context.rpc_epoch.load(Ordering::Acquire);
        context.disconnected();
        let new_epoch = context.rpc_epoch.load(Ordering::Acquire);
        context.ready.receive.store(true, Ordering::Release);
        finish_webhook_check(&context, old_epoch, Ok(()));
        assert!(!context.ready.send.load(Ordering::Acquire));
        assert!(context.ready.connecting.load(Ordering::Acquire));
        finish_webhook_check(&context, new_epoch, Ok(()));
        assert!(context.ready.send.load(Ordering::Acquire));
        assert!(!context.ready.connecting.load(Ordering::Acquire));
        let emitted: Vec<_> = events.try_iter().map(|(_, event)| event).collect();
        assert_eq!(emitted.len(), 2);
        assert!(matches!(&emitted[0], ClientEvent::Batch(batch) if !batch.connected));
        assert!(matches!(&emitted[1], ClientEvent::Notice(_)));
    }

    #[test]
    fn failed_webhook_preparation_keeps_reading_but_cannot_send() {
        let (context, events) = context();
        finish_webhook_check(&context, 1, Err("웹후크 확인 실패".into()));
        assert!(context.ready.receive.load(Ordering::Acquire));
        assert!(!context.ready.send.load(Ordering::Acquire));
        assert!(!context.ready.connecting.load(Ordering::Acquire));
        assert!(events.try_iter().any(|(_, event)| {
            matches!(event, ClientEvent::Status(s) if s.contains("전송 설정 확인 필요"))
        }));
    }

    #[test]
    fn completed_or_cancelled_receiver_rejects_late_verification() {
        let (context, events) = context();
        context.finish_receiver();
        finish_webhook_check(&context, 1, Ok(()));
        assert!(!context.ready.receive.load(Ordering::Acquire));
        assert!(!context.ready.send.load(Ordering::Acquire));
        assert!(!context.ready.connecting.load(Ordering::Acquire));
        assert!(events.try_recv().is_err());

        let (cancelled, events) = self::context();
        cancelled.cancel.store(true, Ordering::Release);
        cancelled.ready.clear();
        finish_webhook_check(&cancelled, 1, Ok(()));
        assert!(!cancelled.ready.send.load(Ordering::Acquire));
        assert!(!cancelled.ready.connecting.load(Ordering::Acquire));
        assert!(events.try_recv().is_err());
    }
}
