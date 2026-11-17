// SPDX-License-Identifier: MPL-2.0
//! Small, process-local history for destinations that have no live receiver.
//! This module owns no connection, worker, renderer, window or persistent log.

use crate::{
    config::{ChannelTab, ConnectionMode, Settings, MAX_CHANNEL_TABS},
    model::ChatStore,
};
use std::collections::{BTreeMap, VecDeque};

const INACTIVE_MESSAGES: usize = 40;

#[derive(Default)]
pub(crate) struct LocalChannelState {
    pub chat: ChatStore,
    pub draft: String,
    pub failed_texts: VecDeque<String>,
    pub restored_draft: bool,
}

// A webhook URL is a credential. Deliberately do not derive Debug here or on
// its containing types, and keep destination matching separate from display.
struct Destination {
    mode: ConnectionMode,
    app_id: String,
    channel_url: String,
    webhook_url: String,
}

struct CachedChannel {
    destination: Destination,
    state: LocalChannelState,
}

#[derive(Default)]
pub(crate) struct ChannelCache {
    entries: BTreeMap<u64, CachedChannel>,
}

impl ChannelCache {
    /// Call before changing the active destination, using its old settings.
    /// Drafts retain their exact text; only the received history is trimmed.
    pub(crate) fn stash(&mut self, settings: &Settings, mut state: LocalChannelState) {
        self.reconcile(settings);
        let Some(tab) = eligible_tab(settings, settings.active_channel_tab) else {
            return;
        };
        if !active_projection_matches(settings, tab) {
            return;
        }
        state.chat.trim_for_inactive_tab(INACTIVE_MESSAGES);
        self.entries.insert(
            tab.id,
            CachedChannel {
                destination: Destination {
                    mode: settings.connection_mode,
                    app_id: settings.discord_app_id.clone(),
                    channel_url: settings.channel_url.clone(),
                    webhook_url: settings.webhook_url.clone(),
                },
                state,
            },
        );
    }

    /// Move the selected tab's exact state back to the active UI once. A stable
    /// tab ID by itself never authorizes reuse after editing its destination.
    pub(crate) fn take(&mut self, settings: &Settings) -> Option<LocalChannelState> {
        self.reconcile(settings);
        self.entries
            .remove(&settings.active_channel_tab)
            .map(|entry| entry.state)
    }

    /// Signing out discards received conversations across all tabs while
    /// keeping unsent drafts and failed sends available for recovery.
    pub(crate) fn clear_history(&mut self) {
        for entry in self.entries.values_mut() {
            entry.state.chat.clear();
        }
    }

    /// The caller checks this before accepting deletion or retargeting, so it
    /// can ask the user to send/clear that tab's draft without losing it.
    pub(crate) fn has_unsent_for_invalidated(&self, settings: &Settings) -> bool {
        self.entries.iter().any(|(id, entry)| {
            !matches_settings(*id, &entry.destination, settings)
                && (!entry.state.draft.is_empty() || !entry.state.failed_texts.is_empty())
        })
    }

    /// Remove missing or changed destinations, including global app changes.
    /// A tab's name, appearance and translation choice do not affect identity.
    pub(crate) fn reconcile(&mut self, settings: &Settings) {
        self.entries
            .retain(|id, entry| matches_settings(*id, &entry.destination, settings));
    }
}

fn eligible_tab(settings: &Settings, id: u64) -> Option<&ChannelTab> {
    if settings.connection_mode != ConnectionMode::Rpc || id == 0 {
        return None;
    }
    // Settings validates this same limit. Bounding the lookup also prevents
    // malformed/in-progress settings from ever growing the cache beyond it;
    // no valid configuration needs eviction of another tab's unsent draft.
    settings
        .channel_tabs
        .iter()
        .take(MAX_CHANNEL_TABS)
        .find(|tab| tab.id == id)
}

fn active_projection_matches(settings: &Settings, tab: &ChannelTab) -> bool {
    settings.channel_url == tab.channel_url && settings.webhook_url == tab.webhook_url
}

fn matches_settings(id: u64, destination: &Destination, settings: &Settings) -> bool {
    let Some(tab) = eligible_tab(settings, id) else {
        return false;
    };
    destination.mode == settings.connection_mode
        && destination.app_id == settings.discord_app_id
        && destination.channel_url == tab.channel_url
        && destination.webhook_url == tab.webhook_url
        && (id != settings.active_channel_tab || active_projection_matches(settings, tab))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatEvent, EventBatch, Message, MAX_MESSAGES};

    fn channel(number: u64) -> String {
        format!(
            "https://discord.com/channels/100000000000000001/{}",
            200000000000000000 + number
        )
    }

    fn webhook(number: u64) -> String {
        format!(
            "https://discord.com/api/webhooks/{}/fictional_secret_ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            300000000000000000 + number
        )
    }

    fn settings_with_tabs(count: usize) -> Settings {
        let mut settings = Settings {
            discord_app_id: "100000000000000009".into(),
            channel_url: channel(1),
            webhook_url: webhook(1),
            ..Settings::default()
        };
        settings.ensure_channel_tabs().unwrap();
        for id in 2..=count as u64 {
            settings
                .add_channel_tab(&format!("채널 {id}"), &channel(id), &webhook(id))
                .unwrap();
        }
        settings
    }

    fn state(label: &str, messages: usize) -> LocalChannelState {
        let mut state = LocalChannelState {
            draft: format!("{label} 미전송 초안 😌"),
            failed_texts: VecDeque::from([format!("{label} 실패한 전송")]),
            restored_draft: true,
            ..LocalChannelState::default()
        };
        state.chat.apply(&EventBatch {
            reset: true,
            events: (1..=messages)
                .map(|id| ChatEvent::Upsert {
                    message: Message {
                        id: id.to_string(),
                        author: label.into(),
                        content: format!("{label} 메시지 {id}"),
                        ..Message::default()
                    },
                })
                .collect(),
            ..EventBatch::default()
        });
        state
    }

    #[test]
    fn two_tabs_keep_history_and_unsent_text_separate_and_take_moves_state_once() {
        let mut settings = settings_with_tabs(2);
        let mut cache = ChannelCache::default();
        let mut first = state("첫째", 2);
        first.draft = "긴 초안 😌".repeat(200);
        let first_draft = first.draft.clone();
        cache.stash(&settings, first);

        settings.select_channel_tab(2).unwrap();
        assert!(!cache.has_unsent_for_invalidated(&settings));
        assert!(cache.take(&settings).is_none());
        cache.stash(&settings, state("둘째", 3));

        settings.select_channel_tab(1).unwrap();
        let first = cache.take(&settings).unwrap();
        assert_eq!(first.draft, first_draft);
        assert_eq!(first.failed_texts, ["첫째 실패한 전송"]);
        assert!(first.restored_draft);
        assert_eq!(first.chat.rows().len(), 2);
        assert!(first.chat.rows().iter().all(|row| row.author == "첫째"));
        assert!(cache.take(&settings).is_none());

        settings.select_channel_tab(2).unwrap();
        let second = cache.take(&settings).unwrap();
        assert_eq!(second.draft, "둘째 미전송 초안 😌");
        assert_eq!(second.failed_texts, ["둘째 실패한 전송"]);
        assert_eq!(second.chat.rows().len(), 3);
        assert!(second.chat.rows().iter().all(|row| row.author == "둘째"));
    }

    #[test]
    fn logout_clears_every_cached_history_without_losing_unsent_state() {
        let mut settings = settings_with_tabs(3);
        let mut cache = ChannelCache::default();
        for id in 1..=3 {
            settings.select_channel_tab(id).unwrap();
            let mut saved = state(&format!("채널 {id}"), id as usize);
            saved.draft = format!("  채널 {id} 미전송 초안 😌\n");
            saved
                .failed_texts
                .push_back(format!("채널 {id} 두 번째 실패"));
            saved.restored_draft = id % 2 != 0;
            cache.stash(&settings, saved);
        }

        cache.clear_history();
        assert_eq!(cache.entries.len(), 3);
        for id in 1..=3 {
            settings.select_channel_tab(id).unwrap();
            let saved = cache.take(&settings).unwrap();
            assert!(saved.chat.rows().is_empty());
            assert_eq!(saved.draft, format!("  채널 {id} 미전송 초안 😌\n"));
            assert_eq!(
                saved.failed_texts,
                [
                    format!("채널 {id} 실패한 전송"),
                    format!("채널 {id} 두 번째 실패")
                ]
            );
            assert_eq!(saved.restored_draft, id % 2 != 0);
        }
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn changed_webhook_channel_or_global_app_cannot_recover_old_text() {
        for change in 0..3 {
            let mut settings = settings_with_tabs(2);
            let mut cache = ChannelCache::default();
            cache.stash(&settings, state("기존", 1));
            settings.select_channel_tab(2).unwrap();
            match change {
                0 => settings
                    .edit_channel_tab(1, "새 웹후크", &channel(1), &webhook(3))
                    .unwrap(),
                1 => settings
                    .edit_channel_tab(1, "새 채널", &channel(3), &webhook(3))
                    .unwrap(),
                _ => settings.discord_app_id = "100000000000000010".into(),
            }
            assert!(cache.has_unsent_for_invalidated(&settings));
            cache.reconcile(&settings);
            assert!(cache.entries.is_empty());
            settings.select_channel_tab(1).unwrap();
            assert!(cache.take(&settings).is_none());
        }
    }

    #[test]
    fn pending_active_projection_edits_are_already_invalidations() {
        let mut settings = settings_with_tabs(2);
        let mut cache = ChannelCache::default();
        cache.stash(&settings, state("기존", 1));
        settings.webhook_url = webhook(3); // Not yet copied into channel_tabs.
        assert!(cache.has_unsent_for_invalidated(&settings));
        assert!(cache.take(&settings).is_none());
        cache.stash(&settings, state("모호한 대상", 1));
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn removed_tabs_invalidate_only_their_own_state_and_failed_queue_is_protected() {
        let mut settings = settings_with_tabs(2);
        let mut cache = ChannelCache::default();
        let mut first = state("첫째", 1);
        first.draft.clear();
        cache.stash(&settings, first);
        settings.select_channel_tab(2).unwrap();
        cache.stash(&settings, state("둘째", 1));
        settings.remove_channel_tab(1).unwrap();
        assert!(cache.has_unsent_for_invalidated(&settings));
        cache.reconcile(&settings);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.take(&settings).unwrap().draft, "둘째 미전송 초안 😌");
    }

    #[test]
    fn history_without_unsent_text_does_not_block_destination_changes() {
        let mut settings = settings_with_tabs(1);
        let mut cache = ChannelCache::default();
        let mut history = state("이력", 1);
        history.draft.clear();
        history.failed_texts.clear();
        cache.stash(&settings, history);
        settings
            .edit_channel_tab(1, "변경", &channel(2), &webhook(2))
            .unwrap();
        assert!(!cache.has_unsent_for_invalidated(&settings));
        assert!(cache.take(&settings).is_none());
    }

    #[test]
    fn renaming_appearance_and_translation_changes_preserve_destination_state() {
        let mut settings = settings_with_tabs(2);
        let mut cache = ChannelCache::default();
        cache.stash(&settings, state("첫째", 1));
        settings
            .edit_channel_tab(1, "새 이름", &channel(1), &webhook(1))
            .unwrap();
        settings.font_size += 2;
        settings.translation.enabled = !settings.translation.enabled;
        settings.sync_active_tab().unwrap();
        assert!(!cache.has_unsent_for_invalidated(&settings));
        assert_eq!(cache.take(&settings).unwrap().draft, "첫째 미전송 초안 😌");
    }

    #[test]
    fn inactive_histories_are_bounded_without_truncating_valid_drafts_or_eviction() {
        let mut settings = settings_with_tabs(MAX_CHANNEL_TABS);
        let mut cache = ChannelCache::default();
        for id in 1..=MAX_CHANNEL_TABS as u64 {
            settings.select_channel_tab(id).unwrap();
            let active = state(&format!("채널 {id}"), MAX_MESSAGES + 40);
            assert_eq!(active.chat.rows().len(), MAX_MESSAGES);
            cache.stash(&settings, active);
        }
        assert_eq!(cache.entries.len(), MAX_CHANNEL_TABS);
        for id in 1..=MAX_CHANNEL_TABS as u64 {
            settings.select_channel_tab(id).unwrap();
            let inactive = cache.take(&settings).unwrap();
            let rows = inactive.chat.rows();
            assert_eq!(rows.len(), INACTIVE_MESSAGES);
            assert_eq!(rows[0].id, (MAX_MESSAGES + 1).to_string());
            assert_eq!(inactive.draft, format!("채널 {id} 미전송 초안 😌"));
            assert_eq!(inactive.failed_texts.len(), 1);
        }
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn relay_mode_never_caches_or_restores_rpc_state() {
        let mut settings = settings_with_tabs(1);
        let mut cache = ChannelCache::default();
        cache.stash(&settings, state("직접 연결", 1));
        settings.connection_mode = ConnectionMode::Relay;
        assert!(cache.has_unsent_for_invalidated(&settings));
        assert!(cache.take(&settings).is_none());
        cache.stash(&settings, state("중계", 1));
        assert!(cache.entries.is_empty());
        settings.connection_mode = ConnectionMode::Rpc;
        assert!(cache.take(&settings).is_none());
    }
}
