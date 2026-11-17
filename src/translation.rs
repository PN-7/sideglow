// SPDX-License-Identifier: MPL-2.0
//! Optional single-PC translation. No thread, queue, HTTP session, cache or key
//! decryption is created while disabled. A retiring request is never replaced
//! by a new worker pair until both retiring workers have actually returned.
mod timing;
pub(crate) mod wire;
mod worker;
pub use timing::TranslationTiming;

use crate::{
    config::{self, ConnectionMode, Settings, TranslationSettings},
    discord_http::Webhook,
    model::{ChatEvent, EventBatch, Message},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const MAX_TRACKED: usize = 400;
const MAX_PENDING: usize = 16;
const MAX_JOB_AGE: Duration = Duration::from_secs(30);
pub(crate) type Guard = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Spec {
    options: TranslationSettings,
    guild: String,
    channel: String,
    webhook: Webhook,
    generation: u64,
    available: Guard,
}

impl Spec {
    pub(crate) fn new(
        settings: &Settings,
        generation: u64,
        available: Guard,
    ) -> Result<Self, String> {
        config::validate_translation(&settings.translation)?;
        if !settings.translation.enabled || settings.connection_mode != ConnectionMode::Rpc {
            return Err(crate::i18n::text(crate::i18n::Key::MAA047CC48D4A));
        }
        let mut options = settings.translation.clone();
        // Display-only changes must not restart a paid request.
        options.show_received = true;
        Ok(Self {
            options,
            guild: wire::guild_id(&settings.channel_url)?,
            channel: config::channel_id(&settings.channel_url)?,
            webhook: Webhook::parse(&settings.webhook_url)?,
            generation,
            available,
        })
    }
    fn same(&self, other: &Self) -> bool {
        self.options == other.options
            && self.guild == other.guild
            && self.channel == other.channel
            && self.webhook == other.webhook
            && self.generation == other.generation
    }
}

pub(crate) enum Event {
    Notice(String),
    Status(String),
    Published {
        message: Box<Message>,
        status: String,
        connection_epoch: u64,
    },
}

#[derive(Clone)]
struct Job {
    id: String,
    revision: String,
    text: String,
    romaji_japanese_input: bool,
    epoch: u64,
    connection_epoch: u64,
    queued: Instant,
}

#[derive(Default)]
struct Tracker {
    epoch: u64,
    // The public translation revision stays text-only for wire compatibility.
    // Internal validity also tracks the sender's policy so an edited label
    // cannot publish a result produced with a stale interpretation hint.
    revisions: BTreeMap<String, (String, bool)>,
    published: BTreeMap<String, String>,
    skipped: usize,
}

impl Tracker {
    fn current(&self, job: &Job) -> bool {
        self.source_current(job) && self.published.get(&job.id) != Some(&job.revision)
    }
    fn source_current(&self, job: &Job) -> bool {
        self.epoch == job.epoch
            && self
                .revisions
                .get(&job.id)
                .is_some_and(|(revision, romaji)| {
                    revision == &job.revision && *romaji == job.romaji_japanese_input
                })
    }
    fn observe(&mut self, batch: &EventBatch, webhook: &str) -> Vec<Job> {
        self.skipped = 0;
        if batch.reset {
            self.epoch = self.epoch.wrapping_add(1);
            self.revisions.clear();
            self.published.clear();
        }
        let mut jobs = Vec::new();
        for event in batch.events.iter().take(2000) {
            match event {
                ChatEvent::Delete { id } => {
                    self.revisions.remove(id);
                    self.published.remove(id);
                }
                ChatEvent::Upsert { message } => {
                    if let Some(reference) = &message.translation {
                        if let Some(parent) = &message.parent_id {
                            self.published
                                .insert(parent.clone(), reference.revision.clone());
                        }
                        continue;
                    }
                    let revision = wire::revision(message.source_text());
                    let romaji_japanese_input = wire::source_is_webhook(message, webhook)
                        && crate::sender_label::has_romaji_marker(&message.author);
                    let previous = self.revisions.insert(
                        message.id.clone(),
                        (revision.clone(), romaji_japanese_input),
                    );
                    if batch.reset
                        || !batch.connected
                        // Changing a sender hint does not retranslate old text.
                        || previous.as_ref().is_some_and(|(old, _)| old == &revision)
                        || !eligible(message, webhook)
                    {
                        continue;
                    }
                    if self.published.get(&message.id) == Some(&revision) {
                        continue;
                    }
                    if jobs.len() < MAX_PENDING {
                        jobs.push(Job {
                            id: message.id.clone(),
                            revision,
                            text: message.source_text().to_owned(),
                            romaji_japanese_input,
                            epoch: self.epoch,
                            connection_epoch: batch.connection_epoch,
                            queued: Instant::now(),
                        });
                    } else {
                        self.skipped += 1;
                    }
                }
            }
        }
        while self.revisions.len() > MAX_TRACKED {
            self.revisions.pop_first();
        }
        while self.published.len() > MAX_TRACKED {
            self.published.pop_first();
        }
        jobs
    }
}

fn eligible(message: &Message, webhook: &str) -> bool {
    if message.translation.is_some()
        || message.source_text().trim().is_empty()
        || message.source_text().contains('\0')
        || message.source_text().encode_utf16().count() > 2000
        || !config::snowflake(&message.id)
    {
        return false;
    }
    wire::source_is_webhook(message, webhook)
        || (!message.bot && message.webhook_id.is_none() && config::snowflake(&message.author_id))
}

#[derive(Default)]
pub(crate) struct Controller {
    desired: Option<Spec>,
    worker: Option<worker::WorkerPool>,
    retiring: Vec<JoinHandle<()>>,
    pending_status: Option<Event>,
    failed_to_start: bool,
    last_timing: Option<TranslationTiming>,
}

impl Controller {
    pub(crate) fn configure(&mut self, desired: Option<Spec>) {
        let same = match (&self.desired, &desired) {
            (Some(a), Some(b)) => a.same(b),
            (None, None) => true,
            _ => false,
        };
        if same {
            if self.failed_to_start && desired.is_some() {
                self.failed_to_start = false;
                self.start_if_ready();
            }
            return;
        }
        self.stop_worker();
        self.desired = desired;
        self.last_timing = None;
        self.failed_to_start = false;
        if !self.retiring.is_empty() {
            self.pending_status = Some(Event::Notice(crate::i18n::text(
                crate::i18n::Key::ME6E27657B048,
            )));
        } else if self.desired.is_none() {
            self.pending_status = Some(Event::Notice(crate::i18n::text(
                crate::i18n::Key::M4A1B08A2EAD3,
            )));
        }
        self.start_if_ready();
    }

    pub(crate) fn stop(&mut self) {
        self.configure(None);
    }

    fn stop_worker(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            self.retiring.extend(worker.stop());
            // Dropping the pool's only sender wakes both idle receivers.
        }
    }

    fn start_if_ready(&mut self) {
        if self.worker.is_some() || !self.retiring.is_empty() || self.failed_to_start {
            return;
        }
        let Some(spec) = self.desired.clone() else {
            return;
        };
        match worker::WorkerPool::start(spec) {
            Ok(worker) => {
                self.worker = Some(worker);
                self.pending_status = Some(Event::Notice(crate::i18n::text(
                    crate::i18n::Key::M424FC0329993,
                )));
            }
            Err(handles) => {
                self.retiring.extend(handles);
                self.failed_to_start = true;
                self.pending_status = Some(Event::Status(crate::i18n::text(
                    crate::i18n::Key::MC3792893412E,
                )));
            }
        }
    }

    pub(crate) fn observe(&mut self, batch: &EventBatch) {
        let (Some(worker), Some(spec)) = (&self.worker, &self.desired) else {
            return;
        };
        let (jobs, mut rejected) = match worker.tracker.lock() {
            Ok(mut tracker) => {
                let jobs = tracker.observe(batch, spec.webhook.id());
                (jobs, tracker.skipped)
            }
            Err(_) => return,
        };
        let mut disconnected = false;
        for job in jobs {
            match worker.jobs.try_send(job) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) => rejected += 1,
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if rejected != 0 && !disconnected {
            self.pending_status = Some(Event::Status(crate::i18n::format(
                crate::i18n::Key::M50A8DA839E5F,
                &[("rejected", format!("{}", rejected))],
            )));
        }
    }

    pub(crate) fn last_timing(&self) -> Option<&TranslationTiming> {
        self.last_timing.as_ref()
    }

    pub(crate) fn poll(&mut self) -> Vec<Event> {
        let was_retiring = !self.retiring.is_empty();
        let mut index = 0;
        while index < self.retiring.len() {
            if self.retiring[index].is_finished() {
                let handle = self.retiring.swap_remove(index);
                let _ = handle.join();
            } else {
                index += 1;
            }
        }
        if was_retiring && self.retiring.is_empty() {
            if self.desired.is_none() {
                self.pending_status = Some(Event::Notice(crate::i18n::text(
                    crate::i18n::Key::ME1F7511C7C91,
                )));
            }
            self.start_if_ready();
        }
        let mut events = Vec::new();
        if let Some(status) = self.pending_status.take() {
            events.push(status);
        }
        // Snapshot failure before draining. Workers enqueue the fatal reason
        // before cancelling, so an observed failure has an available event.
        // A failure arriving during this poll keeps its receiver until next poll.
        let failed = self.worker.as_ref().is_some_and(worker::WorkerPool::failed);
        let updates: Vec<_> = self.worker.as_ref().map_or_else(Vec::new, |worker| {
            worker.events.try_iter().take(32).collect()
        });
        for update in updates {
            match update {
                worker::WorkerEvent::Status(status) => events.push(Event::Status(status)),
                worker::WorkerEvent::Finished {
                    result,
                    timing,
                    connection_epoch,
                    fatal,
                } => {
                    if !self
                        .desired
                        .as_ref()
                        .is_some_and(|spec| (spec.available)() == Some(connection_epoch))
                    {
                        // A key/model/webhook failure stops this configuration,
                        // even if its Discord session changed while HTTP ran.
                        // Keep that reason, but discard the old message/timing.
                        if fatal {
                            if let Err(error) = result {
                                events.push(Event::Status(error));
                            }
                        }
                        continue;
                    }
                    let mut timing = *timing;
                    timing.ui_queue_ms = timing::milliseconds(timing.finished_at.elapsed());
                    let summary = timing.summary();
                    self.last_timing = Some(timing);
                    match result {
                        Ok(message) => events.push(Event::Published {
                            message,
                            status: summary,
                            connection_epoch,
                        }),
                        Err(error) => events.push(Event::Status(format!("{error} · {summary}"))),
                    }
                }
            }
        }
        if failed {
            self.stop_worker();
            // Explicit Save/Apply can retry. A fatal failure cancels the sibling
            // and drops the sender so neither worker stays blocked in recv.
            self.failed_to_start = true;
        }
        // A published acknowledgement can itself wait in the UI queue while a
        // reconnect occurs. Do not announce a result from the retired session.
        events.retain(|event| match event {
            Event::Published {
                connection_epoch, ..
            } => self
                .desired
                .as_ref()
                .is_some_and(|spec| (spec.available)() == Some(*connection_epoch)),
            Event::Notice(_) | Event::Status(_) => true,
        });
        events
    }

    #[cfg(test)]
    fn dormant(&self) -> bool {
        self.worker.is_none() && self.retiring.is_empty() && self.desired.is_none()
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

fn valid_job(spec: &Spec, cancel: &AtomicBool, tracker: &Mutex<Tracker>, job: &Job) -> bool {
    !cancel.load(Ordering::Acquire)
        && job.connection_epoch != 0
        && (spec.available)() == Some(job.connection_epoch)
        && job.queued.elapsed() < MAX_JOB_AGE
        && tracker.lock().is_ok_and(|state| state.current(job))
}

#[cfg(test)]
mod tests {
    use super::*;
    const W: &str = "100000000000000004";
    fn message(id: u64, text: &str) -> Message {
        Message {
            id: format!("10000000000000{id:04}"),
            author_id: "100000000000000001".into(),
            content: text.into(),
            ..Message::default()
        }
    }
    fn batch(reset: bool, messages: Vec<Message>) -> EventBatch {
        EventBatch {
            connection_epoch: 0,
            reset,
            connected: true,
            cursor: 0,
            events: messages
                .into_iter()
                .map(|message| ChatEvent::Upsert { message })
                .collect(),
        }
    }
    #[test]
    fn disabled_controller_never_allocates_worker_or_decrypts() {
        let mut c = Controller::default();
        c.configure(None);
        c.observe(&batch(false, vec![message(1, "hello")]));
        assert!(c.poll().is_empty());
        assert!(c.dormant());
    }

    #[test]
    fn explicit_stop_is_a_notice_without_starting_or_decrypting_work() {
        let mut c = Controller {
            desired: Some(Spec {
                options: TranslationSettings {
                    enabled: true,
                    protected_api_key: "never-decrypt-when-stopping".into(),
                    ..TranslationSettings::default()
                },
                guild: "100000000000000001".into(),
                channel: "100000000000000002".into(),
                webhook: Webhook::parse("https://discord.com/api/webhooks/100000000000000004/ABCDEFGHIJKLMNOPQRSTUVWXYZ_1234567890").unwrap(),
                generation: 1,
                available: Arc::new(|| None),
            }),
            worker: None,
            retiring: Vec::new(),
            pending_status: None,
            failed_to_start: false,
            last_timing: None,
        };
        c.stop();
        let events = c.poll();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Notice(_)));
        assert!(c.dormant());
        assert!(c.poll().is_empty());
    }
    #[test]
    fn history_reconnect_and_duplicate_echo_do_not_translate() {
        let mut t = Tracker::default();
        let a = message(1, "history");
        assert!(t.observe(&batch(true, vec![a.clone()]), W).is_empty());
        assert!(t.observe(&batch(false, vec![a]), W).is_empty());
        let b = message(2, "new");
        let jobs = t.observe(&batch(false, vec![b.clone()]), W);
        assert_eq!(jobs.len(), 1);
        assert!(t.current(&jobs[0]));
        assert!(t.observe(&batch(false, vec![b.clone()]), W).is_empty());
        t.observe(&batch(true, vec![b]), W);
        assert!(!t.current(&jobs[0]));
    }
    #[test]
    fn edits_deletes_and_reset_invalidate_paid_results() {
        let mut t = Tracker::default();
        let m = message(1, "first");
        let first = t.observe(&batch(false, vec![m.clone()]), W).remove(0);
        let mut edited = m.clone();
        edited.content = "edited".into();
        let second = t.observe(&batch(false, vec![edited]), W).remove(0);
        assert!(!t.current(&first));
        assert!(t.current(&second));
        t.observe(
            &EventBatch {
                connection_epoch: 0,
                reset: false,
                connected: true,
                cursor: 0,
                events: vec![ChatEvent::Delete { id: m.id }],
            },
            W,
        );
        assert!(!t.current(&second));
    }
    #[test]
    fn webhook_allowlist_and_translation_loop_filter() {
        let mut m = message(1, "webhook");
        m.bot = true;
        assert!(!eligible(&m, W));
        m.author_id = W.into();
        assert!(eligible(&m, W));
        m.translation = Some(Box::new(crate::model::TranslationReference {
            revision: wire::revision("hello"),
            language: "ko".into(),
        }));
        assert!(!eligible(&m, W));
        let mut t = Tracker::default();
        m.parent_id = Some(message(2, "").id);
        t.observe(&batch(false, vec![m]), W);
        assert!(t
            .observe(&batch(false, vec![message(2, "hello")]), W)
            .is_empty());
    }

    #[test]
    fn romaji_hint_requires_the_configured_webhook_not_a_matching_human_name() {
        let mut human = message(1, "arigatou");
        human.author = crate::sender_label::webhook_username("☆", true);
        let mut opted_in = message(2, "arigatou");
        opted_in.author = human.author.clone();
        opted_in.author_id = W.into();
        opted_in.bot = true;
        // The RPC path may omit webhook_id, so its confirmed author ID is valid evidence.
        let mut opted_in_with_id = message(3, "arigatou");
        opted_in_with_id.author = human.author.clone();
        opted_in_with_id.webhook_id = Some(W.into());
        opted_in_with_id.bot = true;
        let mut legacy = opted_in.clone();
        legacy.id = message(4, "").id;
        legacy.author = "Whisper · ☆".into();
        let mut unchecked = opted_in.clone();
        unchecked.id = message(5, "").id;
        unchecked.author = crate::sender_label::webhook_username("☆", false);
        let mut other_webhook = opted_in.clone();
        other_webhook.id = message(6, "").id;
        other_webhook.author_id = "100000000000000009".into();
        other_webhook.webhook_id = Some(other_webhook.author_id.clone());
        let mut tracker = Tracker::default();
        let jobs = tracker.observe(
            &batch(
                false,
                vec![
                    human,
                    opted_in,
                    opted_in_with_id,
                    legacy,
                    unchecked,
                    other_webhook,
                ],
            ),
            W,
        );
        assert_eq!(jobs.len(), 5);
        assert_eq!(
            jobs.iter()
                .map(|job| job.romaji_japanese_input)
                .collect::<Vec<_>>(),
            [false, true, true, false, false]
        );
        assert!(jobs.iter().all(|job| job.text == "arigatou"));
        assert!(jobs
            .iter()
            .all(|job| job.revision == wire::revision("arigatou")));
    }

    #[test]
    fn changed_sender_policy_cancels_old_work_without_retranslating_old_messages() {
        let mut tracker = Tracker::default();
        let mut source = message(1, "arigatou");
        source.author_id = W.into();
        source.bot = true;
        source.author = crate::sender_label::webhook_username("☆", true);
        let original = tracker
            .observe(&batch(false, vec![source.clone()]), W)
            .remove(0);
        assert!(original.romaji_japanese_input);
        assert!(tracker.current(&original));
        source.author = crate::sender_label::webhook_username("☆", false);
        assert!(tracker
            .observe(&batch(false, vec![source.clone()]), W)
            .is_empty());
        assert!(!tracker.current(&original));
        source.id = message(2, "").id;
        let next = tracker
            .observe(&batch(false, vec![source.clone()]), W)
            .remove(0);
        assert!(!next.romaji_japanese_input);
        assert_eq!(next.revision, original.revision);
        assert!(tracker.observe(&batch(true, vec![source]), W).is_empty());
        assert!(!tracker.current(&next));
    }

    #[test]
    fn tracker_is_bounded() {
        let mut t = Tracker::default();
        for i in 1..1000 {
            t.observe(&batch(false, vec![message(i, "x")]), W);
        }
        assert_eq!(t.revisions.len(), MAX_TRACKED);
    }

    #[test]
    fn translation_reconnect_epoch_invalidates_job_before_reset_is_drained() {
        use std::sync::atomic::AtomicU64;
        let live_epoch = Arc::new(AtomicU64::new(7));
        let available_epoch = live_epoch.clone();
        let spec = Spec {
            options: TranslationSettings::default(),
            guild: "100000000000000001".into(),
            channel: "100000000000000002".into(),
            webhook: Webhook::parse(&format!(
                "https://discord.com/api/webhooks/{W}/ABCDEFGHIJKLMNOPQRSTUVWXYZ_1234567890"
            ))
            .unwrap(),
            generation: 1,
            available: Arc::new(move || {
                let epoch = available_epoch.load(Ordering::Acquire);
                (epoch != 0).then_some(epoch)
            }),
        };
        let mut observed = batch(false, vec![message(1, "original")]);
        observed.connection_epoch = 7;
        let mut tracker = Tracker::default();
        let job = tracker.observe(&observed, W).remove(0);
        let tracker = Mutex::new(tracker);
        let cancel = AtomicBool::new(false);
        assert!(valid_job(&spec, &cancel, &tracker, &job));
        // Readiness has returned to true in another RPC session, while the UI
        // has not yet observed its queued reset snapshot.
        live_epoch.store(8, Ordering::Release);
        assert!(tracker.lock().unwrap().source_current(&job));
        assert!(!valid_job(&spec, &cancel, &tracker, &job));
        live_epoch.store(0, Ordering::Release);
        assert!(!valid_job(&spec, &cancel, &tracker, &job));
    }
}
