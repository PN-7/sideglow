// SPDX-License-Identifier: MPL-2.0
//! A bounded pair of independent translation workers. Shared state is locked
//! only for bookkeeping; no lock is held during API or Discord network calls.
use super::{timing::milliseconds, valid_job, wire, Job, Spec, Tracker, TranslationTiming};
use crate::{
    config,
    discord_http::{DiscordHttp, SendError},
    model::Message,
    openai::{TranslationApiClient, TranslationRequest, TranslationResponse},
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(super) const MAX_WORKERS: usize = 2;
const MAX_CACHE: usize = 64;
const CACHE_AGE: Duration = Duration::from_secs(300);
type JobReceiver = Arc<Mutex<Receiver<Job>>>;
type ClaimKey = (String, String, bool, u64, u64);

pub(super) enum WorkerEvent {
    Status(String),
    Finished {
        result: Result<Box<Message>, String>,
        timing: Box<TranslationTiming>,
        connection_epoch: u64,
        fatal: bool,
    },
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<(CacheKey, TranslationResponse, Instant)>,
}

struct CacheKey {
    text: String,
    romaji_japanese_input: bool,
}

impl Cache {
    fn get(
        &mut self,
        text: &str,
        romaji_japanese_input: bool,
        now: Instant,
    ) -> Option<TranslationResponse> {
        self.entries
            .retain(|(_, _, at)| now.saturating_duration_since(*at) < CACHE_AGE);
        let (_, response, _) = self.entries.iter().find(|(key, _, _)| {
            key.text == text && key.romaji_japanese_input == romaji_japanese_input
        })?;
        let mut response = response.clone();
        // A cache hit does not consume this earlier request's tokens again.
        response.input_tokens = 0;
        response.output_tokens = 0;
        response.reasoning_tokens = None;
        response.service_tier = None;
        response.timing = Default::default();
        Some(response)
    }

    fn insert(
        &mut self,
        text: &str,
        romaji_japanese_input: bool,
        response: &TranslationResponse,
        now: Instant,
    ) {
        self.entries.retain(|(key, _, at)| {
            (key.text != text || key.romaji_japanese_input != romaji_japanese_input)
                && now.saturating_duration_since(*at) < CACHE_AGE
        });
        self.entries.push_back((
            CacheKey {
                text: text.to_owned(),
                romaji_japanese_input,
            },
            response.clone(),
            now,
        ));
        while self.entries.len() > MAX_CACHE {
            self.entries.pop_front();
        }
    }
}

#[derive(Default)]
struct Shared {
    in_flight: BTreeSet<ClaimKey>,
    cooldown: Option<Instant>,
    // API throttling need not discard an already completed sibling result.
    // Discord throttling also pauses the publication of in-flight results.
    publication_cooldown: Option<Instant>,
    cache: Cache,
}

struct Claim {
    shared: Arc<Mutex<Shared>>,
    key: ClaimKey,
}

impl Claim {
    fn acquire(shared: &Arc<Mutex<Shared>>, job: &Job, now: Instant) -> Option<Self> {
        let mut state = shared.lock().ok()?;
        if state.cooldown.is_some_and(|until| now < until) {
            return None;
        }
        let key = (
            job.id.clone(),
            job.revision.clone(),
            job.romaji_japanese_input,
            job.epoch,
            job.connection_epoch,
        );
        if !state.in_flight.insert(key.clone()) {
            return None;
        }
        Some(Self {
            shared: shared.clone(),
            key,
        })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.lock() {
            state.in_flight.remove(&self.key);
        }
    }
}

fn cool_down(shared: &Mutex<Shared>, seconds: u64) {
    cool_down_at(shared, seconds, Instant::now(), false);
}

fn cool_down_publication(shared: &Mutex<Shared>, seconds: u64) {
    cool_down_at(shared, seconds, Instant::now(), true);
}

fn cool_down_at(shared: &Mutex<Shared>, seconds: u64, now: Instant, publication: bool) {
    if let Ok(mut state) = shared.lock() {
        // OpenAI validates up to one hour; Discord validates up to one day.
        // Never shorten those accepted server hints to the old five-minute cap.
        let until = now + Duration::from_secs(seconds.min(86_400));
        state.cooldown = Some(state.cooldown.map_or(until, |previous| previous.max(until)));
        if publication {
            state.publication_cooldown = Some(
                state
                    .publication_cooldown
                    .map_or(until, |previous| previous.max(until)),
            );
        }
    }
}

fn publication_paused(shared: &Mutex<Shared>, now: Instant) -> bool {
    shared.lock().map_or(true, |state| {
        state.publication_cooldown.is_some_and(|until| now < until)
    })
}

fn receive_job(receiver: &JobReceiver) -> Option<Job> {
    // This guard ends inside this function. Keeping it in a while-let scrutinee
    // would retain the receiver lock through network work and serialize both workers.
    let result = receiver.lock().ok()?.recv();
    result.ok()
}

pub(super) struct WorkerPool {
    pub jobs: SyncSender<Job>,
    pub events: Receiver<WorkerEvent>,
    pub cancel: Arc<AtomicBool>,
    pub tracker: Arc<Mutex<Tracker>>,
    handles: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    pub fn start(spec: Spec) -> Result<Self, Vec<JoinHandle<()>>> {
        let (job_tx, job_rx) = mpsc::sync_channel(super::MAX_PENDING);
        let (event_tx, event_rx) = mpsc::sync_channel(32);
        let cancel = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(Mutex::new(Tracker::default()));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let receiver = Arc::new(Mutex::new(job_rx));
        let mut handles = Vec::with_capacity(MAX_WORKERS);
        for index in 0..MAX_WORKERS {
            let context = Context {
                spec: spec.clone(),
                receiver: receiver.clone(),
                events: event_tx.clone(),
                cancel: cancel.clone(),
                tracker: tracker.clone(),
                shared: shared.clone(),
            };
            match std::thread::Builder::new()
                .name(format!("sideglow-ai-{}", index + 1))
                .spawn(move || run(context))
            {
                Ok(handle) => handles.push(handle),
                Err(_) => {
                    cancel.store(true, Ordering::Release);
                    drop(job_tx); // Wake every idle receiver before retiring partial startup.
                    return Err(handles);
                }
            }
        }
        Ok(Self {
            jobs: job_tx,
            events: event_rx,
            cancel,
            tracker,
            handles,
        })
    }

    pub fn failed(&self) -> bool {
        self.cancel.load(Ordering::Acquire) || self.handles.iter().any(JoinHandle::is_finished)
    }

    pub fn stop(&mut self) -> Vec<JoinHandle<()>> {
        self.cancel.store(true, Ordering::Release);
        if let Ok(mut tracker) = self.tracker.lock() {
            tracker.epoch = tracker.epoch.wrapping_add(1);
        }
        std::mem::take(&mut self.handles)
        // The caller immediately drops this pool, including its sole job sender.
    }
}

struct Context {
    spec: Spec,
    receiver: JobReceiver,
    events: SyncSender<WorkerEvent>,
    cancel: Arc<AtomicBool>,
    tracker: Arc<Mutex<Tracker>>,
    shared: Arc<Mutex<Shared>>,
}

impl Context {
    fn valid(&self, job: &Job) -> bool {
        valid_job(&self.spec, &self.cancel, &self.tracker, job)
    }

    fn fatal(&self, message: String) {
        let _ = self.events.try_send(WorkerEvent::Status(message));
        self.cancel.store(true, Ordering::Release);
    }

    fn finish(
        &self,
        job: &Job,
        result: Result<Box<Message>, String>,
        mut timing: TranslationTiming,
        started: Instant,
        fatal: bool,
    ) {
        timing.worker_ms = milliseconds(started.elapsed());
        timing.finished_at = Instant::now();
        timing.succeeded = result.is_ok();
        let _ = self.events.try_send(WorkerEvent::Finished {
            result,
            timing: Box::new(timing),
            connection_epoch: job.connection_epoch,
            fatal,
        });
    }
}

fn run(context: Context) {
    if context.cancel.load(Ordering::Acquire) {
        return;
    }
    let key = match config::load_translation_key(&context.spec.options) {
        Ok(key) => key,
        Err(error) => {
            context.fatal(crate::i18n::format(
                crate::i18n::Key::M8B5E75DEE3C3,
                &[("error", error.to_string())],
            ));
            return;
        }
    };
    let mut api = match TranslationApiClient::new(
        key,
        context.spec.options.provider,
        &context.spec.options.api_base_url,
        context.spec.options.model.clone(),
        context.spec.options.fast_mode,
    ) {
        Ok(api) => api,
        Err(error) => {
            context.fatal({
                let __ui_arg0 = &(error.message);
                crate::i18n::format(
                    crate::i18n::Key::M49DD17FDCF82,
                    &[("arg0", __ui_arg0.to_string())],
                )
            });
            return;
        }
    };
    let mut discord: Option<DiscordHttp> = None;
    while let Some(job) = receive_job(&context.receiver) {
        if context.cancel.load(Ordering::Acquire) {
            break;
        }
        if !context.valid(&job) {
            continue;
        }
        let started = Instant::now();
        let Some(_claim) = Claim::acquire(&context.shared, &job, started) else {
            continue;
        };
        let mut timing = TranslationTiming::new(&context.spec.options.model, job.queued, started);
        timing.provider = context.spec.options.provider;
        timing.fast_requested = context.spec.options.fast_mode;
        let cached = context.shared.lock().ok().and_then(|mut state| {
            state
                .cache
                .get(&job.text, job.romaji_japanese_input, started)
        });
        let response = if let Some(response) = cached {
            timing.cache_hit = true;
            response
        } else {
            // Another worker may have observed a fatal failure since this job was dequeued.
            if !context.valid(&job) {
                continue;
            }
            if context
                .shared
                .lock()
                .ok()
                .is_some_and(|state| state.cooldown.is_some_and(|until| Instant::now() < until))
            {
                continue;
            }
            let request = TranslationRequest {
                text: job.text.clone(),
                language_a: context.spec.options.language_a.clone(),
                language_b: context.spec.options.language_b.clone(),
                glossary: context.spec.options.glossary.clone(),
                romaji_japanese_input: job.romaji_japanese_input,
            };
            let api_started = Instant::now();
            let result = api.translate(&request, &context.cancel);
            timing.api_ms = milliseconds(api_started.elapsed());
            match result {
                Ok(response) => {
                    timing.api_phases = Some(response.timing);
                    timing.input_tokens = Some(response.input_tokens);
                    timing.output_tokens = Some(response.output_tokens);
                    timing.reasoning_tokens = response.reasoning_tokens;
                    timing.service_tier = response.service_tier;
                    if !context.valid(&job) {
                        continue;
                    }
                    if let Ok(mut state) = context.shared.lock() {
                        state.cache.insert(
                            &job.text,
                            job.romaji_japanese_input,
                            &response,
                            Instant::now(),
                        );
                    }
                    response
                }
                Err(error) => {
                    if context.cancel.load(Ordering::Acquire) {
                        break;
                    }
                    cool_down(
                        &context.shared,
                        u64::from(error.retry_after_secs.unwrap_or(3)),
                    );
                    let status = if error.fatal {
                        {
                            let __ui_arg0 = &(error.message);
                            crate::i18n::format(
                                crate::i18n::Key::MFF071F3C0D47,
                                &[("arg0", __ui_arg0.to_string())],
                            )
                        }
                    } else {
                        {
                            let __ui_arg0 = &(error.message);
                            crate::i18n::format(
                                crate::i18n::Key::M07731072358B,
                                &[("arg0", __ui_arg0.to_string())],
                            )
                        }
                    };
                    context.finish(&job, Err(status), timing, started, error.fatal);
                    if error.fatal {
                        // Queue the diagnosis before poll() can retire the pool
                        // and drop its event receiver on observing cancellation.
                        context.cancel.store(true, Ordering::Release);
                        break;
                    }
                    continue;
                }
            }
        };
        if !context.valid(&job) {
            continue;
        }
        let text = match wire::encode(
            &context.spec.guild,
            &context.spec.channel,
            &job.id,
            &job.revision,
            &response.target_language,
            &response.text,
        ) {
            Ok(text) => text,
            Err(error) => {
                context.finish(
                    &job,
                    Err(crate::i18n::format(
                        crate::i18n::Key::MA3ECFDC3DCBB,
                        &[("error", error.to_string())],
                    )),
                    timing,
                    started,
                    false,
                );
                continue;
            }
        };
        let result = publish(&context, &job, &text, &mut discord, &mut timing);
        if context.cancel.load(Ordering::Acquire)
            || (context.spec.available)() != Some(job.connection_epoch)
        {
            continue;
        }
        match result {
            Ok(Some(mut message)) => {
                if !context
                    .tracker
                    .lock()
                    .is_ok_and(|state| state.source_current(&job))
                {
                    continue;
                }
                wire::normalize(
                    &mut message,
                    &context.spec.channel,
                    context.spec.webhook.id(),
                );
                if let Ok(mut state) = context.tracker.lock() {
                    if state.current(&job) {
                        state.published.insert(job.id.clone(), job.revision.clone());
                    }
                }
                context.finish(&job, Ok(Box::new(message)), timing, started, false);
            }
            Ok(None) => continue,
            Err(error) => {
                if !context.valid(&job) {
                    continue;
                }
                if error.recreate_http {
                    discord = None;
                }
                if !error.fatal {
                    cool_down_publication(
                        &context.shared,
                        u64::from(error.retry_after_secs.unwrap_or(5)),
                    );
                }
                let status = if error.fatal {
                    {
                        let __ui_arg0 = &(error.reason);
                        crate::i18n::format(
                            crate::i18n::Key::M02D70FE79F75,
                            &[("arg0", __ui_arg0.to_string())],
                        )
                    }
                } else {
                    {
                        let __ui_arg0 = &(error.reason);
                        crate::i18n::format(
                            crate::i18n::Key::M3C5B0A1F225B,
                            &[("arg0", __ui_arg0.to_string())],
                        )
                    }
                };
                context.finish(&job, Err(status), timing, started, error.fatal);
                if error.fatal {
                    context.cancel.store(true, Ordering::Release);
                    break;
                }
            }
        }
    }
}

fn publish(
    context: &Context,
    job: &Job,
    text: &str,
    discord: &mut Option<DiscordHttp>,
    timing: &mut TranslationTiming,
) -> Result<Option<Message>, SendError> {
    if !context.valid(job) || publication_paused(&context.shared, Instant::now()) {
        return Ok(None);
    }
    if discord.is_none() {
        *discord = Some(DiscordHttp::new().map_err(SendError::transport_failure)?);
    }
    let http = discord.as_mut().unwrap();
    let started = Instant::now();
    let verified = context
        .spec
        .webhook
        .verify_detailed(http, &context.spec.channel);
    timing.verify_ms = milliseconds(started.elapsed());
    // A local edit/delete/reconnect is cancellation, not a Discord failure.
    // It must not apply the shared error cooldown to unrelated messages.
    if !context.valid(job) {
        return Ok(None);
    }
    verified?;
    if publication_paused(&context.shared, Instant::now()) {
        return Ok(None);
    }
    let started = Instant::now();
    let receipt = context
        .spec
        .webhook
        .send(http, &context.spec.channel, text, "AI 번역");
    timing.post_ms = milliseconds(started.elapsed());
    receipt.map(|receipt| Some(receipt.message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;

    fn job(text: &str, epoch: u64) -> Job {
        Job {
            id: "100000000000000001".into(),
            revision: wire::revision(text),
            text: text.into(),
            romaji_japanese_input: false,
            epoch,
            connection_epoch: 1,
            queued: Instant::now(),
        }
    }

    #[test]
    fn claims_block_aba_duplicates_and_retired_guard_cannot_release_new_epoch() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let first = job("A", 1);
        let original = Claim::acquire(&shared, &first, Instant::now()).unwrap();
        assert!(Claim::acquire(&shared, &job("B", 1), Instant::now()).is_some());
        assert!(Claim::acquire(&shared, &job("A", 1), Instant::now()).is_none());
        let newer = Claim::acquire(&shared, &job("A", 2), Instant::now()).unwrap();
        drop(original);
        assert!(Claim::acquire(&shared, &job("A", 2), Instant::now()).is_none());
        drop(newer);
        assert!(shared.lock().unwrap().in_flight.is_empty());
    }

    #[test]
    fn shared_cooldown_blocks_other_worker_before_next_request() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        cool_down(&shared, 30);
        assert!(Claim::acquire(&shared, &job("next", 1), Instant::now()).is_none());
    }

    #[test]
    fn cooldown_preserves_full_server_wait_and_never_shortens_an_existing_deadline() {
        let now = Instant::now();
        let shared = Arc::new(Mutex::new(Shared::default()));
        cool_down_at(&shared, 3600, now, false);
        assert_eq!(
            shared.lock().unwrap().cooldown,
            Some(now + Duration::from_secs(3600))
        );
        assert!(
            Claim::acquire(&shared, &job("limited", 1), now + Duration::from_secs(3599)).is_none()
        );
        assert!(
            !publication_paused(&shared, now),
            "API throttling need not drop a completed sibling translation"
        );
        cool_down_at(&shared, 30, now, false);
        assert_eq!(
            shared.lock().unwrap().cooldown,
            Some(now + Duration::from_secs(3600))
        );

        cool_down_at(&shared, 86_400, now, true);
        let deadline = now + Duration::from_secs(86_400);
        assert_eq!(shared.lock().unwrap().cooldown, Some(deadline));
        assert!(Claim::acquire(
            &shared,
            &job("limited", 1),
            deadline - Duration::from_secs(1)
        )
        .is_none());
        assert!(publication_paused(
            &shared,
            deadline - Duration::from_secs(1)
        ));
        assert!(!publication_paused(&shared, deadline));
        assert!(Claim::acquire(&shared, &job("ready", 1), deadline).is_some());
    }

    #[test]
    fn completed_cache_is_bounded_expires_and_does_not_rebill_tokens() {
        let now = Instant::now();
        let response = TranslationResponse {
            text: "번역".into(),
            source_language: "en".into(),
            target_language: "ko".into(),
            input_tokens: 12,
            output_tokens: 3,
            reasoning_tokens: Some(0),
            service_tier: Some(crate::openai::ServiceTier::Fast),
            timing: Default::default(),
        };
        let mut cache = Cache::default();
        for index in 0..MAX_CACHE + 5 {
            cache.insert(&format!("text-{index}"), false, &response, now);
        }
        assert_eq!(cache.entries.len(), MAX_CACHE);
        assert!(cache.get("text-0", false, now).is_none());
        let hit = cache.get("text-10", false, now).unwrap();
        assert_eq!(hit.text, "번역");
        assert_eq!((hit.input_tokens, hit.output_tokens), (0, 0));
        assert_eq!(hit.service_tier, None);
        assert!(cache.get("Text-10", false, now).is_none());
        assert!(Cache::default().get("text-10", false, now).is_none());
        assert!(cache.get("text-10", false, now + CACHE_AGE).is_none());
        let first = wire::encode(
            "100000000000000002",
            "100000000000000003",
            "100000000000000004",
            &wire::revision("text-10"),
            &hit.target_language,
            &hit.text,
        )
        .unwrap();
        let second = wire::encode(
            "100000000000000002",
            "100000000000000003",
            "100000000000000005",
            &wire::revision("text-10"),
            &hit.target_language,
            &hit.text,
        )
        .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn cache_keeps_opposite_sender_romaji_policies_separate_for_identical_text() {
        let now = Instant::now();
        let plain = TranslationResponse {
            text: "plain result".into(),
            source_language: "en".into(),
            target_language: "ko".into(),
            input_tokens: 12,
            output_tokens: 3,
            reasoning_tokens: Some(0),
            service_tier: None,
            timing: Default::default(),
        };
        let mut romaji = plain.clone();
        romaji.text = "고마워".into();
        romaji.source_language = "ja".into();
        let mut cache = Cache::default();
        cache.insert("arigatou", false, &plain, now);
        assert!(cache.get("arigatou", true, now).is_none());
        cache.insert("arigatou", true, &romaji, now);
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(
            cache.get("arigatou", false, now).unwrap().text,
            "plain result"
        );
        assert_eq!(cache.get("arigatou", true, now).unwrap().text, "고마워");
        cache.insert("arigatou", true, &romaji, now + Duration::from_secs(1));
        assert_eq!(cache.entries.len(), 2);
        assert!(cache.get("arigatou", false, now + CACHE_AGE).is_none());
        assert!(cache.get("arigatou", true, now + CACHE_AGE).is_some());
    }

    #[test]
    fn receiver_allows_two_overlapping_jobs_and_drop_wakes_idle_workers() {
        let (tx, rx) = mpsc::sync_channel(16);
        let receiver = Arc::new(Mutex::new(rx));
        let (started_tx, started_rx) = mpsc::channel();
        let mut handles = Vec::new();
        let mut releases = Vec::new();
        for _ in 0..MAX_WORKERS {
            let receiver = receiver.clone();
            let started = started_tx.clone();
            let (release_tx, release_rx) = mpsc::channel::<()>();
            releases.push(release_tx);
            handles.push(std::thread::spawn(move || {
                while let Some(work) = receive_job(&receiver) {
                    started.send(work.text).unwrap();
                    if release_rx.recv().is_err() {
                        break;
                    }
                }
            }));
        }
        tx.send(job("one", 1)).unwrap();
        tx.send(job("two", 1)).unwrap();
        tx.send(job("three", 1)).unwrap();
        let first = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_ne!(first, second);
        assert_eq!(started_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        // Dropping the sender wakes a receiver once its active fake job returns.
        drop(tx);
        drop(releases);
        drop(started_tx);
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            started_rx.recv_timeout(Duration::from_millis(1)),
            Err(RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn reconnect_discards_old_timing_but_keeps_the_reason_for_a_fatal_stop() {
        use super::super::{Controller, Event};
        use crate::{config::TranslationSettings, discord_http::Webhook};
        for fatal in [false, true] {
            let now = Instant::now();
            let (jobs, _receiver) = mpsc::sync_channel(16);
            let (sender, events) = mpsc::sync_channel(32);
            sender
                .send(WorkerEvent::Finished {
                    result: Err("AI 번역 중단 · test diagnosis".into()),
                    timing: Box::new(TranslationTiming::new("test-model", now, now)),
                    connection_epoch: 1,
                    fatal,
                })
                .unwrap();
            let mut controller = Controller {
                desired: Some(Spec {
                    options: TranslationSettings::default(),
                    guild: "100000000000000001".into(),
                    channel: "100000000000000002".into(),
                    webhook: Webhook::parse("https://discord.com/api/webhooks/100000000000000004/ABCDEFGHIJKLMNOPQRSTUVWXYZ_1234567890").unwrap(),
                    generation: 1,
                    available: Arc::new(|| Some(2)),
                }),
                worker: Some(WorkerPool {
                    jobs, events, cancel: Arc::new(AtomicBool::new(fatal)),
                    tracker: Arc::new(Mutex::new(Tracker::default())),
                    handles: Vec::new(),
                }),
                retiring: Vec::new(), pending_status: None,
                failed_to_start: false, last_timing: None,
            };
            let output = controller.poll();
            assert!(controller.last_timing().is_none());
            assert_eq!(output.len(), usize::from(fatal));
            if fatal {
                assert!(
                    matches!(&output[0], Event::Status(message) if message.contains("test diagnosis"))
                );
                assert!(controller.worker.is_none());
                assert!(controller.failed_to_start);
            }
        }
    }

    #[test]
    fn fatal_shutdown_wakes_idle_sibling_and_retains_busy_worker_before_restart() {
        use super::super::Controller;
        use crate::{config::TranslationSettings, discord_http::Webhook};

        let (jobs, receiver) = mpsc::sync_channel(16);
        let receiver = Arc::new(Mutex::new(receiver));
        let (_events, events) = mpsc::sync_channel(32);
        let cancel = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(Mutex::new(Tracker::default()));
        let (started_tx, started_rx) = mpsc::channel();
        let (ended_tx, ended_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let mut handles = Vec::new();
        for index in 0..MAX_WORKERS {
            let receiver = receiver.clone();
            let cancel = cancel.clone();
            let started = started_tx.clone();
            let ended = ended_tx.clone();
            let release = release_rx.clone();
            handles.push(std::thread::spawn(move || {
                while receive_job(&receiver).is_some() {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    started.send(index).unwrap();
                    let _ = release.lock().unwrap().recv();
                }
                ended.send(index).unwrap();
            }));
        }
        jobs.send(job("busy", 1)).unwrap();
        let active = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut controller = Controller {
            desired: None,
            worker: Some(WorkerPool {
                jobs,
                events,
                cancel: cancel.clone(),
                tracker,
                handles,
            }),
            retiring: Vec::new(),
            pending_status: None,
            failed_to_start: false,
            last_timing: None,
        };

        // A fatal failure must make the controller drop the sole sender so
        // the idle sibling leaves recv, even while the other request is busy.
        cancel.store(true, Ordering::Release);
        controller.poll();
        assert!(controller.worker.is_none());
        assert!(controller.failed_to_start);
        assert_ne!(
            ended_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            active
        );
        assert_eq!(controller.retiring.len(), MAX_WORKERS);

        controller.desired = Some(Spec {
            options: TranslationSettings::default(),
            guild: "100000000000000001".into(),
            channel: "100000000000000002".into(),
            webhook: Webhook::parse("https://discord.com/api/webhooks/100000000000000004/ABCDEFGHIJKLMNOPQRSTUVWXYZ_1234567890").unwrap(),
            generation: 2,
            available: Arc::new(|| Some(2)),
        });
        controller.failed_to_start = false;
        controller.start_if_ready();
        assert!(
            controller.worker.is_none(),
            "a new pool must wait for both retiring workers"
        );
        controller.desired = None;
        drop(release_tx);
        assert_eq!(
            ended_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            active
        );
        for handle in std::mem::take(&mut controller.retiring) {
            handle.join().unwrap();
        }
        assert!(controller.dormant());
    }
}
