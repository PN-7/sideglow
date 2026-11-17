// SPDX-License-Identifier: MPL-2.0
//! Cosmetic webhook labels also carry a sender's opt-in translation hint.
//! A label alone is never proof of account identity or webhook authorization.

const PREFIX: &str = "Sideglow · ";
const ROMAJI_PREFIX: &str = "Sideglow [romaji] · ";
const LEGACY_PREFIX: &str = "Whisper · ";

pub(crate) fn webhook_username(display_name: &str, romaji_japanese_input: bool) -> String {
    let name: String = display_name
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    let name = if name.trim().is_empty() {
        crate::i18n::text(crate::i18n::Key::M201E52791935)
    } else {
        name
    };
    let prefix = if romaji_japanese_input {
        ROMAJI_PREFIX
    } else {
        PREFIX
    };
    format!("{prefix}{name}")
}

/// Callers must separately verify the configured webhook before using this
/// hint for translation. A human nickname may contain the same text.
pub(crate) fn has_romaji_marker(author: &str) -> bool {
    author
        .strip_prefix(ROMAJI_PREFIX)
        .is_some_and(|name| !name.trim().is_empty())
}

/// Call only for bot/webhook display rows; keep ordinary human names intact.
pub(crate) fn display_nickname(author: &str) -> &str {
    author
        .strip_prefix(ROMAJI_PREFIX)
        .or_else(|| author.strip_prefix(PREFIX))
        .or_else(|| author.strip_prefix(LEGACY_PREFIX))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(author)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_opt_in_round_trips_without_changing_the_nickname() {
        for enabled in [false, true] {
            let label = webhook_username("☆", enabled);
            assert_eq!(has_romaji_marker(&label), enabled);
            assert_eq!(display_nickname(&label), "☆");
            let bounded = webhook_username(&format!("{}\n\0", "名".repeat(80)), enabled);
            assert_eq!(display_nickname(&bounded), "名".repeat(60));
            assert!(bounded.chars().count() <= 80);
        }
        assert!(!has_romaji_marker("Sideglow · ☆"));
        assert!(!has_romaji_marker("Whisper · ☆"));
        assert!(!has_romaji_marker("Sideglow [romaji] · "));
        assert!(!has_romaji_marker("Other Sideglow [romaji] · ☆"));
    }
}
