// SPDX-License-Identifier: MPL-2.0
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration, Instant};

pub const MAX_MESSAGES: usize = 200;
const MAX_ACK_OBSERVATIONS: usize = MAX_MESSAGES * 2;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    pub author: String,
    pub content: String,
    /// Only populated when rendered embeds/attachments differ from typed text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    pub timestamp: String,
    #[serde(default)]
    pub bot: bool,
    #[serde(default)]
    pub author_id: String,
    #[serde(default)]
    pub webhook_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<Box<TranslationReference>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranslationReference {
    pub revision: String,
    pub language: String,
}

impl Message {
    pub fn source_text(&self) -> &str {
        self.original_text.as_deref().unwrap_or(&self.content)
    }

    /// Hide the app's webhook decoration only when building display rows.
    /// RPC and relay messages can omit webhook_id but retain the bot flag.
    /// This label is cosmetic; the original author and IDs remain authoritative.
    fn display_author(&self) -> &str {
        if !self.bot
            && !self
                .webhook_id
                .as_deref()
                .is_some_and(crate::config::snowflake)
        {
            return &self.author;
        }
        crate::sender_label::display_nickname(&self.author)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum ChatEvent {
    Upsert { message: Message },
    Delete { id: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EventBatch {
    /// Local direct-connection identity. Never accepted from relay JSON.
    #[serde(skip)]
    pub connection_epoch: u64,
    pub cursor: u64,
    pub reset: bool,
    pub connected: bool,
    pub events: Vec<ChatEvent>,
}

#[derive(Clone, Debug)]
pub struct DisplayRow {
    pub id: String,
    pub author: String,
    pub original: String,
    pub translations: Vec<String>,
    /// Indices of actual AI translations or bot replies in `translations`.
    /// Human reply text shares grouping but keeps the ordinary body style.
    pub translated_replies: Vec<usize>,
    pub timestamp: String,
}

#[derive(Default)]
pub struct ChatStore {
    messages: BTreeMap<String, Message>,
    /// Local first receipt time; historical snapshot entries have no age.
    received_at: BTreeMap<String, Instant>,
    initialized: bool,
    observed_ack_ids: BTreeSet<u64>,
    acknowledgement_floor: u64,
}

impl ChatStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initial history and edits do not count as fresh messages.
    pub fn apply(&mut self, batch: &EventBatch) -> bool {
        self.apply_at(batch, Instant::now())
    }

    fn apply_at(&mut self, batch: &EventBatch, now: Instant) -> bool {
        let announce_new = self.initialized && !batch.reset;
        if batch.reset {
            self.messages.clear();
            // Keep receipt times until the snapshot is rebuilt. Messages still
            // present after a reconnect retain their original age.
            // A reconnect snapshot does not cancel an in-flight HTTP reply.
            // Keep observed IDs so that an old acknowledgement cannot restore
            // a message absent from this newer snapshot.
        }
        let mut new_message = false;
        for event in batch.events.iter().take(2000) {
            match event {
                ChatEvent::Upsert { message } => {
                    if message.id.is_empty() || message.id.len() > 64 {
                        continue;
                    }
                    let inserted = announce_new && !self.messages.contains_key(&message.id);
                    let already_observed = acknowledgement_id(&message.id).is_some_and(|id| {
                        id <= self.acknowledgement_floor || self.observed_ack_ids.contains(&id)
                    });
                    if inserted && !already_observed {
                        self.received_at.entry(message.id.clone()).or_insert(now);
                    }
                    self.observe_ack_id(&message.id);
                    new_message |= inserted;
                    self.insert_message(message);
                }
                ChatEvent::Delete { id } => {
                    self.observe_ack_id(id);
                    self.messages.remove(id);
                    self.received_at.remove(id);
                }
            }
        }
        self.initialized = true;
        self.trim_history();
        new_message
    }

    /// Display a Discord-confirmed HTTP send without waiting for the RPC echo.
    ///
    /// Acknowledgements only fill a previously unseen message. RPC events and
    /// snapshots remain authoritative: a late HTTP reply must not undo an edit
    /// or resurrect a deletion, even after bounded observation history prunes.
    /// Returns true only when a new message remains in the displayed history.
    pub fn acknowledge(&mut self, message: &Message) -> bool {
        self.acknowledge_at(message, Instant::now())
    }

    fn acknowledge_at(&mut self, message: &Message, now: Instant) -> bool {
        let Some(id) = acknowledgement_id(&message.id) else {
            return false;
        };
        if id <= self.acknowledgement_floor
            || self.observed_ack_ids.contains(&id)
            || self.messages.contains_key(&message.id)
        {
            return false;
        }
        self.observe_ack_id(&message.id);
        self.insert_message(message);
        self.received_at.insert(message.id.clone(), now);
        self.initialized = true;
        self.trim_history();
        self.messages.contains_key(&message.id)
    }

    fn observe_ack_id(&mut self, value: &str) {
        let Some(id) = acknowledgement_id(value) else {
            return;
        };
        if id <= self.acknowledgement_floor {
            return;
        }
        self.observed_ack_ids.insert(id);
        while self.observed_ack_ids.len() > MAX_ACK_OBSERVATIONS {
            if let Some(oldest) = self.observed_ack_ids.pop_first() {
                // Keep a constant-size lower bound after removing individual
                // tombstones. Discarding an unusually old ACK is safer than
                // losing its deletion history and displaying it again.
                self.acknowledgement_floor = self.acknowledgement_floor.max(oldest);
            }
        }
    }

    fn insert_message(&mut self, message: &Message) {
        let mut message = message.clone();
        truncate(&mut message.content, 16000);
        truncate(&mut message.author, 256);
        if let Some(text) = &mut message.original_text {
            truncate(text, 16000);
        }
        if !crate::config::snowflake(&message.author_id) {
            message.author_id.clear();
        }
        if message
            .webhook_id
            .as_deref()
            .is_some_and(|id| !crate::config::snowflake(id))
        {
            message.webhook_id = None;
        }
        self.messages.insert(message.id.clone(), message);
    }

    fn trim_history(&mut self) {
        self.trim_history_to(MAX_MESSAGES);
    }

    /// Keep a small in-memory preview while a tab has no active receiver. The
    /// tombstone/floor guard remains bounded and still rejects late old ACKs.
    pub fn trim_for_inactive_tab(&mut self, limit: usize) {
        self.trim_history_to(limit.min(MAX_MESSAGES));
    }

    fn trim_history_to(&mut self, limit: usize) {
        if self.messages.len() > limit {
            let mut ordered: Vec<_> = self.messages.values().collect();
            ordered.sort_by(|a, b| compare_messages(a, b));
            let remove: Vec<String> = ordered
                .iter()
                .take(ordered.len() - limit)
                .map(|m| m.id.clone())
                .collect();
            for id in remove {
                self.messages.remove(&id);
            }
        }
        self.received_at
            .retain(|id, _| self.messages.contains_key(id));
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.received_at.clear();
        self.initialized = false;
        self.observed_ack_ids.clear();
        self.acknowledgement_floor = 0;
    }

    pub fn rows(&self) -> Vec<DisplayRow> {
        self.rows_with_translations(true)
    }

    pub fn rows_with_translations(&self, show_translations: bool) -> Vec<DisplayRow> {
        self.rows_visible(show_translations, Instant::now(), None)
    }

    /// Expire each conversation row from its newest visible member's receipt.
    /// Passing `None` includes all history while the user reads or types.
    pub fn rows_visible(
        &self,
        show_translations: bool,
        now: Instant,
        max_age: Option<Duration>,
    ) -> Vec<DisplayRow> {
        self.rows_visible_with_deadline(show_translations, now, max_age)
            .0
    }

    /// The next deadline lets the existing UI timer repaint only when a row
    /// expires, without rebuilding and cloning message history on every tick.
    pub fn rows_visible_with_deadline(
        &self,
        show_translations: bool,
        now: Instant,
        max_age: Option<Duration>,
    ) -> (Vec<DisplayRow>, Option<Instant>) {
        struct TranslationSlot {
            row: usize,
            text: usize,
            received_at: Option<Instant>,
        }
        let mut ordered: Vec<_> = self.messages.values().collect();
        ordered.sort_by(|a, b| compare_messages(a, b));
        ordered.retain(|message| {
            let Some(reference) = &message.translation else {
                return true;
            };
            if !show_translations {
                return false;
            }
            message
                .parent_id
                .as_ref()
                .and_then(|id| self.messages.get(id))
                .is_some_and(|source| {
                    crate::translation::wire::revision(source.source_text()) == reference.revision
                })
        });
        let mut rows: Vec<DisplayRow> = Vec::new();
        let mut row_received: Vec<Option<Instant>> = Vec::new();
        let mut indices: BTreeMap<String, usize> = BTreeMap::new();
        let mut translated = BTreeMap::<(String, String), TranslationSlot>::new();
        // Build all roots first: a reply may arrive before its referenced source.
        for message in &ordered {
            if self.root_for(message) == message.id {
                indices.insert(message.id.clone(), rows.len());
                rows.push(DisplayRow {
                    id: message.id.clone(),
                    author: message.display_author().to_owned(),
                    original: message.content.clone(),
                    translations: Vec::new(),
                    translated_replies: Vec::new(),
                    timestamp: message.timestamp.clone(),
                });
                row_received.push(self.received_at.get(&message.id).copied());
            }
        }
        for message in &ordered {
            let root = self.root_for(message);
            if root != message.id {
                if let Some(index) = indices.get(&root) {
                    let text = if message.translation.is_some() {
                        message.content.clone()
                    } else {
                        format!("{} · {}", message.display_author(), message.content)
                    };
                    if let Some(reference) = &message.translation {
                        // Different source messages may share a conversation
                        // root. Deduplicate only translations of the same source.
                        let key = (
                            message.parent_id.clone().unwrap_or(root),
                            reference.language.clone(),
                        );
                        let received_at = self.received_at.get(&message.id).copied();
                        if let Some(slot) = translated.get_mut(&key) {
                            rows[*index].translations[slot.text] = text;
                            slot.received_at = received_at;
                        } else {
                            translated.insert(
                                key,
                                TranslationSlot {
                                    row: *index,
                                    text: rows[*index].translations.len(),
                                    received_at,
                                },
                            );
                            let row = &mut rows[*index];
                            row.translated_replies.push(row.translations.len());
                            row.translations.push(text);
                        }
                    } else {
                        let row = &mut rows[*index];
                        if message.bot {
                            row.translated_replies.push(row.translations.len());
                        }
                        row.translations.push(text);
                        row_received[*index] =
                            row_received[*index].max(self.received_at.get(&message.id).copied());
                    }
                }
            }
        }
        // Only the deduplicated, actually displayed AI result can renew its
        // row. An older result arriving late must not keep invisible text alive.
        for slot in translated.values() {
            row_received[slot.row] = row_received[slot.row].max(slot.received_at);
        }
        let Some(max_age) = max_age else {
            return (rows, None);
        };
        let mut next_expiration = None;
        let visible = rows
            .into_iter()
            .zip(row_received)
            .filter_map(|(row, received)| {
                let received = received?;
                if now.saturating_duration_since(received) >= max_age {
                    return None;
                }
                if let Some(expires) = received.checked_add(max_age) {
                    next_expiration = Some(
                        next_expiration.map_or(expires, |earliest: Instant| earliest.min(expires)),
                    );
                }
                Some(row)
            })
            .collect();
        (visible, next_expiration)
    }

    fn root_for(&self, message: &Message) -> String {
        let mut current = message;
        let mut visited = HashSet::new();
        visited.insert(message.id.as_str());
        for _ in 0..32 {
            let Some(parent) = current
                .parent_id
                .as_ref()
                .and_then(|id| self.messages.get(id))
            else {
                return current.id.clone();
            };
            if !visited.insert(&parent.id) {
                return message.id.clone();
            }
            current = parent;
        }
        message.id.clone()
    }

    pub fn demo() -> Self {
        let mut store = Self::new();
        let messages = [
            Message {
                id: "100000000000000001".into(),
                parent_id: None,
                author: "Mina".into(),
                content: "Let's meet by the north gate.".into(),
                timestamp: "2026-01-01T12:00:00.000Z".into(),
                bot: false,
                ..Message::default()
            },
            Message {
                id: "100000000000000002".into(),
                parent_id: Some("100000000000000001".into()),
                author: "번역".into(),
                content: "북쪽 문에서 만나자.".into(),
                timestamp: "2026-01-01T12:00:01.000Z".into(),
                bot: true,
                ..Message::default()
            },
            Message {
                id: "100000000000000003".into(),
                parent_id: None,
                author: "Alex".into(),
                content: "준비되면 알려줘. 같이 이동하자!".into(),
                timestamp: "2026-01-01T12:01:00.000Z".into(),
                bot: false,
                ..Message::default()
            },
            Message {
                id: "100000000000000004".into(),
                parent_id: Some("100000000000000003".into()),
                author: "Translation".into(),
                content: "Let me know when you're ready. Let's move together!".into(),
                timestamp: "2026-01-01T12:01:01.000Z".into(),
                bot: true,
                ..Message::default()
            },
        ];
        store.apply(&EventBatch {
            connection_epoch: 0,
            cursor: 4,
            reset: true,
            connected: false,
            events: messages
                .into_iter()
                .map(|message| ChatEvent::Upsert { message })
                .collect(),
        });
        // The synthetic preview should appear immediately. Real connection
        // snapshots deliberately do not receive a fresh timestamp.
        let now = Instant::now();
        store.received_at = store.messages.keys().map(|id| (id.clone(), now)).collect();
        store
    }
}

fn acknowledgement_id(value: &str) -> Option<u64> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().filter(|id| *id != 0)
}

fn compare_messages(a: &Message, b: &Message) -> std::cmp::Ordering {
    a.timestamp
        .cmp(&b.timestamp)
        .then(a.id.len().cmp(&b.id.len()))
        .then(a.id.cmp(&b.id))
}
fn truncate(value: &mut String, max: usize) {
    if value.len() > max {
        let mut at = max;
        while !value.is_char_boundary(at) {
            at -= 1;
        }
        value.truncate(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(id: &str, parent: Option<&str>, content: &str) -> Message {
        Message {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            author: "A".into(),
            content: content.into(),
            timestamp: format!("2026-{id:0>5}"),
            bot: parent.is_some(),
            ..Message::default()
        }
    }
    fn batch(reset: bool, messages: Vec<Message>) -> EventBatch {
        EventBatch {
            connection_epoch: 0,
            cursor: 1,
            reset,
            connected: true,
            events: messages
                .into_iter()
                .map(|message| ChatEvent::Upsert { message })
                .collect(),
        }
    }
    fn deletion(id: &str) -> EventBatch {
        EventBatch {
            connection_epoch: 0,
            cursor: 2,
            reset: false,
            connected: true,
            events: vec![ChatEvent::Delete { id: id.into() }],
        }
    }
    fn ai(id: &str, source: &Message, language: &str, text: &str) -> Message {
        let mut message = m(id, Some(&source.id), text);
        message.translation = Some(Box::new(TranslationReference {
            revision: crate::translation::wire::revision(source.source_text()),
            language: language.into(),
        }));
        message
    }
    #[test]
    fn webhook_display_names_keep_the_nickname_and_leave_human_names_unchanged() {
        let cases = [
            ("Sideglow · ☆", true, None, "☆"),
            ("Sideglow [romaji] · ☆", true, None, "☆"),
            ("Whisper · Alex", true, None, "Alex"),
            ("Sideglow · ☆", false, Some("123456789012345678"), "☆"),
            ("Sideglow · ☆", false, None, "Sideglow · ☆"),
            (
                "Sideglow [romaji] · ☆",
                false,
                None,
                "Sideglow [romaji] · ☆",
            ),
            ("Whisper · Alex", false, None, "Whisper · Alex"),
            ("Sideglow · ☆", false, Some("invalid"), "Sideglow · ☆"),
            ("Other bot · ☆", true, None, "Other bot · ☆"),
            ("Sideglow · ", true, None, "Sideglow · "),
            ("Whisper ·  ", true, None, "Whisper ·  "),
            ("Sideglow · Sideglow · ☆", true, None, "Sideglow · ☆"),
        ];
        for (author, bot, webhook, expected) in cases {
            let message = Message {
                author: author.into(),
                bot,
                webhook_id: webhook.map(str::to_owned),
                ..Message::default()
            };
            assert_eq!(message.display_author(), expected, "{author}");
            assert_eq!(message.author, author);
        }
    }

    #[test]
    fn display_rows_hide_webhook_prefixes_without_changing_stored_identity() {
        let mut store = ChatStore::new();
        let mut source = m("101", None, "테스트");
        source.author = "Sideglow · ☆".into();
        source.author_id = "123456789012345678".into();
        source.webhook_id = Some(source.author_id.clone());
        source.bot = true;
        let mut reply = m("102", Some("101"), "reply");
        reply.author = "Whisper · Alex".into();
        let translated = ai("103", &source, "ja", "テスト");
        let mut human = m("104", None, "human");
        human.author = "Sideglow · ☆".into();
        store.apply(&batch(
            true,
            vec![
                source.clone(),
                reply.clone(),
                translated.clone(),
                human.clone(),
            ],
        ));

        let rows = store.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].author, "☆");
        assert_eq!(rows[0].original, "테스트");
        assert_eq!(rows[0].translations, ["Alex · reply", "テスト"]);
        assert_eq!(rows[0].translated_replies, [0, 1]);
        assert_eq!(rows[1].author, "Sideglow · ☆");
        for original in [source, reply, translated, human] {
            assert_eq!(store.messages[&original.id], original);
        }
    }

    #[test]
    fn confirmed_send_precedes_echo_without_duplicate_or_lost_translation() {
        let mut store = ChatStore::new();
        store.apply(&batch(true, vec![]));
        let sent = m("101", None, "original");
        assert!(store.acknowledge(&sent));
        assert!(!store.acknowledge(&sent));
        assert!(!store.apply(&batch(false, vec![sent])));
        assert!(store.apply(&batch(false, vec![m("102", Some("101"), "번역")])));
        let rows = store.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].original, "original");
        assert_eq!(rows[0].translations, vec!["A · 번역"]);
        assert_eq!(store.messages.len(), 2);
    }
    #[test]
    fn late_acknowledgement_never_reverts_rpc_content() {
        let mut store = ChatStore::new();
        store.apply(&batch(true, vec![m("101", None, "snapshot")]));
        assert!(!store.acknowledge(&m("101", None, "old HTTP response")));
        assert_eq!(store.rows()[0].original, "snapshot");
        store.apply(&batch(false, vec![m("101", None, "edited over RPC")]));
        assert!(!store.acknowledge(&m("101", None, "old HTTP response")));
        assert_eq!(store.rows()[0].original, "edited over RPC");

        assert!(store.acknowledge(&m("102", None, "HTTP first")));
        assert!(!store.apply(&batch(false, vec![m("102", None, "RPC edit")])));
        assert!(!store.acknowledge(&m("102", None, "HTTP first")));
        assert_eq!(store.messages["102"].content, "RPC edit");
    }
    #[test]
    fn deletion_before_acknowledgement_prevents_resurrection() {
        let mut store = ChatStore::new();
        store.apply(&deletion("101"));
        assert!(!store.acknowledge(&m("101", None, "already deleted")));
        assert!(store.rows().is_empty());

        store.apply(&batch(false, vec![m("102", None, "RPC first")]));
        store.apply(&deletion("102"));
        assert!(!store.acknowledge(&m("102", None, "HTTP later")));
        assert!(store.rows().is_empty());
    }
    #[test]
    fn ack_then_echo_then_delete_survives_duplicate_ack_and_snapshot_reset() {
        let mut store = ChatStore::new();
        let sent = m("101", None, "sent");
        assert!(store.acknowledge(&sent));
        store.apply(&batch(false, vec![sent.clone()]));
        store.apply(&deletion("101"));
        assert!(!store.acknowledge(&sent));
        store.apply(&batch(true, vec![]));
        assert!(!store.acknowledge(&sent));
        assert!(store.rows().is_empty());
    }
    #[test]
    fn pruned_tombstones_still_reject_old_acknowledgements() {
        let mut store = ChatStore::new();
        let total = MAX_ACK_OBSERVATIONS + 40;
        for id in 1..=total {
            store.apply(&deletion(&id.to_string()));
        }
        assert_eq!(store.observed_ack_ids.len(), MAX_ACK_OBSERVATIONS);
        assert!(!store.observed_ack_ids.contains(&1));
        assert!(store.acknowledgement_floor >= 1);
        assert!(!store.acknowledge(&m("1", None, "old deleted message")));
        assert!(!store.acknowledge(&m(&total.to_string(), None, "recently deleted")));
        assert!(store.acknowledge(&m(&(total + 1).to_string(), None, "new message")));
        assert_eq!(store.messages.len(), 1);
        assert!(store.observed_ack_ids.len() <= MAX_ACK_OBSERVATIONS);

        // The ACK guard does not change authoritative RPC/history behavior.
        assert!(store.apply(&batch(false, vec![m("1", None, "RPC authoritative")])));
        assert_eq!(store.messages["1"].content, "RPC authoritative");
        assert!(!store.acknowledge(&m("1", None, "stale HTTP body")));
    }
    #[test]
    fn acknowledged_history_and_text_are_bounded() {
        let mut store = ChatStore::new();
        let content = "가".repeat(6000);
        for id in 1..=MAX_ACK_OBSERVATIONS + 40 {
            assert!(store.acknowledge(&m(&id.to_string(), None, &content)));
        }
        assert_eq!(store.messages.len(), MAX_MESSAGES);
        assert_eq!(store.received_at.len(), MAX_MESSAGES);
        assert!(store
            .received_at
            .keys()
            .all(|id| store.messages.contains_key(id)));
        assert_eq!(store.observed_ack_ids.len(), MAX_ACK_OBSERVATIONS);
        assert!(store
            .messages
            .values()
            .all(|message| message.content.len() <= 16000));
        assert!(!store.acknowledge(&m("1", None, "already evicted")));
        assert!(!store.messages.contains_key("1"));
    }
    #[test]
    fn malformed_ack_ids_are_ignored_and_clear_starts_a_fresh_store() {
        let mut store = ChatStore::new();
        for id in ["", "0", "+1", "-1", "1x", "18446744073709551616"] {
            assert!(!store.acknowledge(&m(id, None, "invalid")));
        }
        assert!(store.messages.is_empty());
        assert!(store.observed_ack_ids.is_empty());
        assert!(store.acknowledge(&m("101", None, "first session")));
        store.clear();
        assert!(store.messages.is_empty());
        assert!(store.received_at.is_empty());
        assert!(store.observed_ack_ids.is_empty());
        assert_eq!(store.acknowledgement_floor, 0);
        assert!(!store.initialized);
        assert!(store.acknowledge(&m("101", None, "fresh store")));
    }
    #[test]
    fn out_of_order_replies_join_source() {
        let mut store = ChatStore::new();
        store.apply(&batch(true, vec![m("2", Some("1"), "번역")]));
        assert_eq!(store.rows()[0].original, "번역");
        assert!(store.apply(&batch(false, vec![m("1", None, "original")])));
        let rows = store.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].original, "original");
        assert_eq!(rows[0].translations, vec!["A · 번역"]);
    }
    #[test]
    fn edits_duplicates_and_initial_snapshot_do_not_count_as_new_activity() {
        let mut store = ChatStore::new();
        assert!(!store.apply(&batch(true, vec![m("1", None, "first")])));
        assert!(!store.apply(&batch(false, vec![m("1", None, "edited")])));
        assert_eq!(store.rows()[0].original, "edited");
        assert!(store.apply(&batch(false, vec![m("2", Some("1"), "translation")])));
        assert!(!store.apply(&batch(false, vec![m("2", Some("1"), "translation")])));
    }

    #[test]
    fn rows_expire_independently_and_new_chat_never_revives_an_older_row() {
        let start = Instant::now();
        let after = |seconds| start + Duration::from_secs(seconds);
        let max_age = Some(Duration::from_secs(10));
        let mut store = ChatStore::new();
        store.apply_at(&batch(true, vec![]), start);
        store.apply_at(&batch(false, vec![m("1", None, "first")]), start);
        store.apply_at(&batch(false, vec![m("2", None, "second")]), after(5));

        let (rows, deadline) = store.rows_visible_with_deadline(true, after(9), max_age);
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(deadline, Some(after(10)));
        let (rows, deadline) = store.rows_visible_with_deadline(true, after(10), max_age);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "2");
        assert_eq!(deadline, Some(after(15)));
        let (rows, deadline) = store.rows_visible_with_deadline(true, after(15), max_age);
        assert!(rows.is_empty());
        assert_eq!(deadline, None);
        let (history, deadline) = store.rows_visible_with_deadline(true, after(50), None);
        assert_eq!(history.len(), 2);
        assert_eq!(deadline, None);
    }

    #[test]
    fn confirmed_receipt_age_survives_echoes_edits_and_duplicate_acknowledgements() {
        let start = Instant::now();
        let after = |seconds| start + Duration::from_secs(seconds);
        let mut store = ChatStore::new();
        let sent = m("1", None, "sent");
        assert!(store.acknowledge_at(&sent, start));
        assert!(!store.apply_at(&batch(false, vec![sent.clone()]), after(3)));
        assert!(!store.apply_at(&batch(false, vec![m("1", None, "edited")]), after(8)));
        assert!(!store.acknowledge_at(&sent, after(9)));
        assert_eq!(store.received_at.get("1"), Some(&start));
        assert!(store
            .rows_visible(true, after(10), Some(Duration::from_secs(10)))
            .is_empty());
        assert_eq!(store.rows()[0].original, "edited");

        store.apply_at(&deletion("1"), after(11));
        assert!(store.received_at.is_empty());
        // An authoritative event may restore history, but an already observed
        // ID must not become fresh simply because its stored row was removed.
        store.apply_at(&batch(false, vec![sent]), after(12));
        assert_eq!(store.rows().len(), 1);
        assert!(store.received_at.is_empty());
    }

    #[test]
    fn reconnect_preserves_live_ages_without_making_unknown_history_fresh() {
        let start = Instant::now();
        let after = |seconds| start + Duration::from_secs(seconds);
        let max_age = Some(Duration::from_secs(10));
        let mut store = ChatStore::new();
        store.apply_at(&batch(true, vec![m("1", None, "old history")]), start);
        assert!(store.rows_visible(true, start, max_age).is_empty());
        store.apply_at(
            &batch(false, vec![m("1", None, "history edited")]),
            after(1),
        );
        store.apply_at(&batch(false, vec![m("2", None, "live")]), after(2));
        store.apply_at(
            &batch(
                true,
                vec![
                    m("1", None, "history edited"),
                    m("2", None, "live after reconnect"),
                    m("3", None, "newly loaded history"),
                ],
            ),
            after(8),
        );
        assert_eq!(store.received_at.len(), 1);
        assert_eq!(store.received_at.get("2"), Some(&after(2)));
        let visible = store.rows_visible(true, after(11), max_age);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "2");
        assert!(store.rows_visible(true, after(12), max_age).is_empty());
        assert_eq!(store.rows().len(), 3);
        store.apply_at(&batch(true, vec![]), after(13));
        assert!(store.received_at.is_empty());
    }

    #[test]
    fn new_visible_translation_or_reply_refreshes_only_its_own_group() {
        let start = Instant::now();
        let after = |seconds| start + Duration::from_secs(seconds);
        let max_age = Some(Duration::from_secs(10));
        let first = m("1", None, "first source");
        let second = m("2", None, "second source");
        let mut store = ChatStore::new();
        store.apply_at(&batch(true, vec![]), start);
        store.apply_at(&batch(false, vec![first.clone(), second]), start);
        store.apply_at(&batch(false, vec![ai("3", &first, "ko", "번역")]), after(8));

        let (visible, deadline) = store.rows_visible_with_deadline(true, after(11), max_age);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "1");
        assert_eq!(visible[0].translations, ["번역"]);
        assert_eq!(deadline, Some(after(18)));
        // Disabling received AI does not let hidden translations extend age.
        assert!(store.rows_visible(false, after(11), max_age).is_empty());

        store.apply_at(&batch(false, vec![m("4", Some("2"), "reply")]), after(11));
        let visible = store.rows_visible(false, after(12), max_age);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "2");
        assert_eq!(store.rows_visible(true, after(12), max_age).len(), 2);
        // Editing a source invalidates its old AI result without refreshing it.
        store.apply_at(
            &batch(false, vec![m("1", None, "edited source")]),
            after(13),
        );
        let visible = store.rows_visible(true, after(13), max_age);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "2");
    }

    #[test]
    fn hidden_older_translation_cannot_renew_the_displayed_result() {
        let start = Instant::now();
        let after = |seconds| start + Duration::from_secs(seconds);
        let max_age = Some(Duration::from_secs(10));
        let source = m("1", None, "source");
        let mut store = ChatStore::new();
        store.apply_at(&batch(true, vec![]), start);
        store.apply_at(&batch(false, vec![source.clone()]), start);
        store.apply_at(
            &batch(false, vec![ai("3", &source, "ko", "latest result")]),
            after(2),
        );
        // An out-of-order event introduces an older duplicate which grouping
        // hides in favor of the newer Discord result already being displayed.
        store.apply_at(
            &batch(false, vec![ai("2", &source, "ko", "older result")]),
            after(9),
        );
        let (visible, deadline) = store.rows_visible_with_deadline(true, after(11), max_age);
        assert_eq!(visible[0].translations, ["latest result"]);
        assert_eq!(deadline, Some(after(12)));
        assert!(store.rows_visible(true, after(12), max_age).is_empty());
    }
    #[test]
    fn deleting_source_keeps_orphan_reply_visible() {
        let mut store = ChatStore::new();
        store.apply(&batch(
            true,
            vec![m("1", None, "source"), m("2", Some("1"), "reply")],
        ));
        store.apply(&EventBatch {
            connection_epoch: 0,
            cursor: 2,
            reset: false,
            connected: true,
            events: vec![ChatEvent::Delete { id: "1".into() }],
        });
        assert_eq!(store.rows()[0].original, "reply");
    }
    #[test]
    fn cyclic_or_nested_references_cannot_hang_or_drop_messages() {
        let mut store = ChatStore::new();
        store.apply(&batch(
            true,
            vec![m("1", Some("2"), "a"), m("2", Some("1"), "b")],
        ));
        assert_eq!(store.rows().len(), 2);
        store.apply(&batch(
            true,
            vec![
                m("1", None, "a"),
                m("2", Some("1"), "b"),
                m("3", Some("2"), "c"),
            ],
        ));
        assert_eq!(store.rows()[0].translations.len(), 2);
    }
    #[test]
    fn history_and_unicode_are_bounded() {
        let mut store = ChatStore::new();
        store.apply(&batch(
            true,
            (1..=300)
                .map(|i| m(&i.to_string(), None, "가".repeat(6000).as_str()))
                .collect(),
        ));
        assert_eq!(store.messages.len(), MAX_MESSAGES);
        assert!(store.messages.values().all(|m| m.content.len() <= 16000));
    }
    #[test]
    fn protocol_discriminators_roundtrip() {
        let b = batch(true, vec![m("1", None, "한국어 😀")]);
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.contains("\"op\":\"upsert\""));
        assert_eq!(
            serde_json::from_str::<EventBatch>(&json)
                .unwrap()
                .events
                .len(),
            1
        );
    }

    #[test]
    fn translation_sources_sharing_a_reply_root_are_not_overwritten() {
        let source_a = m("1", None, "source A");
        let mut source_b = m("2", Some("1"), "source B");
        source_b.bot = false;
        let a = ai("3", &source_a, "ko", "translation A");
        let b = ai("4", &source_b, "ko", "translation B");
        let a_new = ai("5", &source_a, "ko", "latest translation A");
        let mut store = ChatStore::new();
        store.apply(&batch(true, vec![source_a, source_b, a, b, a_new]));
        let rows = store.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].translations,
            vec!["A · source B", "latest translation A", "translation B"]
        );
    }

    #[test]
    fn translation_display_toggle_preserves_originals_and_regular_replies() {
        let source = m("1", None, "source");
        let mut reply = m("2", Some("1"), "human reply");
        reply.bot = false;
        let translation = ai("3", &source, "ko", "번역");
        let mut store = ChatStore::new();
        store.apply(&batch(true, vec![source, reply, translation]));

        let hidden = store.rows_with_translations(false);
        assert_eq!(hidden.len(), 1);
        assert_eq!(hidden[0].original, "source");
        assert_eq!(hidden[0].translations, vec!["A · human reply"]);
        assert_eq!(store.messages.len(), 3);
        assert_eq!(
            store.rows_with_translations(true)[0].translations,
            vec!["A · human reply", "번역"]
        );
    }

    #[test]
    fn translated_reply_indices_exclude_humans_and_survive_ai_replacement() {
        let source = m("1", None, "source");
        let mut human = m("2", Some("1"), "human reply");
        human.bot = false;
        let mut bot = m("3", Some("1"), "bot translation");
        bot.bot = true;
        let older = ai("4", &source, "ko", "older AI");
        let newer = ai("5", &source, "ko", "newer AI");
        let mut store = ChatStore::new();
        store.apply(&batch(true, vec![source, human, bot, older, newer]));
        let row = &store.rows()[0];
        assert_eq!(
            row.translations,
            ["A · human reply", "A · bot translation", "newer AI"]
        );
        assert_eq!(row.translated_replies, [1, 2]);
        let hidden = &store.rows_with_translations(false)[0];
        assert_eq!(
            hidden.translations,
            ["A · human reply", "A · bot translation"]
        );
        assert_eq!(hidden.translated_replies, [1]);
    }

    #[test]
    fn inactive_cache_prunes_content_and_ages_without_reviving_old_acknowledgements() {
        let mut store = ChatStore::new();
        for id in 1..=MAX_ACK_OBSERVATIONS + 20 {
            assert!(store.acknowledge(&m(&id.to_string(), None, "text")));
        }
        store.trim_for_inactive_tab(40);
        assert_eq!(store.messages.len(), 40);
        assert_eq!(store.received_at.len(), 40);
        assert_eq!(
            store.rows()[0].id,
            (MAX_ACK_OBSERVATIONS + 20 - 39).to_string()
        );
        assert!(store.observed_ack_ids.len() <= MAX_ACK_OBSERVATIONS);
        assert!(!store.acknowledge(&m("1", None, "very old ACK")));
        assert!(!store.acknowledge(&m("300", None, "recent pruned ACK")));
        store.trim_for_inactive_tab(usize::MAX);
        assert_eq!(store.messages.len(), 40);
        store.trim_for_inactive_tab(0);
        assert!(store.messages.is_empty());
        assert!(store.received_at.is_empty());
        assert!(!store.acknowledge(&m("420", None, "old ACK after empty cache")));
    }

    #[test]
    fn source_edits_and_deletion_hide_stale_ai_while_preview_changes_preserve_it() {
        let source = m("1", None, "source");
        let translation = ai("2", &source, "ko", "번역");
        let mut store = ChatStore::new();
        store.apply(&batch(true, vec![source.clone(), translation]));
        assert_eq!(store.rows()[0].translations, vec!["번역"]);

        let mut preview = source.clone();
        preview.content = "source\npreview title".into();
        preview.original_text = Some("source".into());
        store.apply(&batch(false, vec![preview]));
        assert_eq!(store.rows()[0].translations, vec!["번역"]);

        let mut edited = source;
        edited.content = "edited source".into();
        store.apply(&batch(false, vec![edited.clone()]));
        assert!(store.rows()[0].translations.is_empty());
        store.apply(&batch(false, vec![ai("3", &edited, "ko", "수정 번역")]));
        assert_eq!(store.rows()[0].translations, vec!["수정 번역"]);

        store.apply(&deletion("1"));
        assert!(store.rows_with_translations(true).is_empty());
        assert!(store.rows_with_translations(false).is_empty());
    }

    #[test]
    fn connection_epoch_is_local_only_metadata() {
        let mut b = batch(true, vec![]);
        b.connection_epoch = 123;
        let json = serde_json::to_string(&b).unwrap();
        assert!(!json.contains("connection_epoch"));
        let forged =
            r#"{"connection_epoch":999,"cursor":0,"reset":false,"connected":true,"events":[]}"#;
        assert_eq!(
            serde_json::from_str::<EventBatch>(forged)
                .unwrap()
                .connection_epoch,
            0
        );
    }
}
