// SPDX-License-Identifier: MPL-2.0
//! A readable Discord message link also carries source identity for other Sideglow clients.
//! Only the explicitly configured incoming webhook may supply this envelope.
use crate::{
    config,
    model::{Message, TranslationReference},
};

const PREFIX: &str = "[Sideglow 번역 · ";
const TAG: &str = "sideglow_translation=v1";
const LEGACY_PREFIX: &str = "[Whisper 번역 · ";
const LEGACY_TAG: &str = "whisper_translation=v1";

pub fn revision(text: &str) -> String {
    // Stable content version, not an authentication or cryptographic signature.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn source_is_webhook(message: &Message, webhook: &str) -> bool {
    !webhook.is_empty()
        && (message.webhook_id.as_deref() == Some(webhook) || message.author_id == webhook)
}

pub fn normalize(message: &mut Message, channel: &str, webhook: &str) {
    // Never accept serialized/relay-provided translation metadata as proof.
    message.translation = None;
    if !source_is_webhook(message, webhook) {
        return;
    }
    let Some((header, text)) = message.source_text().split_once('\n') else {
        return;
    };
    let (header, tag) = if let Some(header) = header.strip_prefix(PREFIX) {
        (header, TAG)
    } else if let Some(header) = header.strip_prefix(LEGACY_PREFIX) {
        (header, LEGACY_TAG)
    } else {
        return;
    };
    let Some((label, link)) = header.split_once("](https://discord.com/channels/") else {
        return;
    };
    let Some(link) = link.strip_suffix(')') else {
        return;
    };
    let Some((path, query)) = link.split_once('?') else {
        return;
    };
    let ids: Vec<_> = path.split('/').collect();
    if ids.len() != 3
        || !ids.iter().all(|id| config::snowflake(id))
        || ids[1] != channel
        || ids[2] == message.id
    {
        return;
    }
    let fields: Vec<_> = query.split('&').collect();
    if fields.len() != 3 || fields[0] != tag {
        return;
    }
    let Some(source_revision) = fields[1].strip_prefix("revision=") else {
        return;
    };
    let Some(language) = fields[2].strip_prefix("lang=") else {
        return;
    };
    if source_revision.len() != 16
        || !source_revision.bytes().all(|b| b.is_ascii_hexdigit())
        || !valid_language(language)
        || label != language.to_ascii_uppercase()
        || text.trim().is_empty()
    {
        return;
    }
    let parent = ids[2].to_owned();
    let reference = TranslationReference {
        revision: source_revision.to_owned(),
        language: language.to_owned(),
    };
    let text = text.to_owned();
    message.parent_id = Some(parent);
    message.translation = Some(Box::new(reference));
    message.content = text;
    message.original_text = None;
}

pub fn encode(
    guild: &str,
    channel: &str,
    source: &str,
    source_revision: &str,
    language: &str,
    text: &str,
) -> Result<String, String> {
    if ![guild, channel, source]
        .iter()
        .all(|id| config::snowflake(id))
        || !valid_language(language)
        || source_revision.len() != 16
        || !source_revision.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(crate::i18n::text(crate::i18n::Key::M62EE4EF5A65B));
    }
    let content = format!("{PREFIX}{}](https://discord.com/channels/{guild}/{channel}/{source}?{TAG}&revision={source_revision}&lang={language})\n{text}", language.to_ascii_uppercase());
    if text.trim().is_empty() || content.encode_utf16().count() > 2000 || text.contains('\0') {
        return Err(crate::i18n::text(crate::i18n::Key::MADA00E19E364));
    }
    Ok(content)
}

pub fn valid_language(language: &str) -> bool {
    !language.is_empty()
        && language.len() <= 12
        && language
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
}

pub fn guild_id(link: &str) -> Result<String, String> {
    config::channel_id(link)?;
    let path = link
        .trim()
        .strip_prefix("https://")
        .and_then(|s| s.split_once('/'))
        .map(|(_, p)| p)
        .ok_or(crate::i18n::text(crate::i18n::Key::M8BA8098B7CE5))?;
    let id = path
        .split('/')
        .nth(1)
        .filter(|id| config::snowflake(id))
        .ok_or(crate::i18n::text(crate::i18n::Key::MD17569CC26EF))?;
    Ok(id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    const G: &str = "100000000000000001";
    const C: &str = "100000000000000002";
    const S: &str = "100000000000000003";
    const W: &str = "100000000000000004";
    fn envelope() -> Message {
        Message {
            id: "100000000000000005".into(),
            author_id: W.into(),
            bot: true,
            content: encode(G, C, S, &revision("hello"), "ko", "안녕하세요").unwrap(),
            ..Message::default()
        }
    }
    #[test]
    fn only_configured_webhook_can_link_translation() {
        let mut m = envelope();
        normalize(&mut m, C, W);
        assert_eq!(m.parent_id.as_deref(), Some(S));
        assert_eq!(m.content, "안녕하세요");
        assert!(m.translation.is_some());
        let mut fake = envelope();
        fake.author_id = "someone".into();
        normalize(&mut fake, C, W);
        assert!(fake.translation.is_none());
        assert!(fake.parent_id.is_none());
        let mut wrong_channel = envelope();
        normalize(&mut wrong_channel, G, W);
        assert!(wrong_channel.translation.is_none());
    }
    #[test]
    fn prior_whisper_translations_remain_readable_after_rename() {
        // A fixed prior-release envelope, independent of the current encoder.
        let legacy = "[Whisper 번역 · KO](https://discord.com/channels/100000000000000001/100000000000000002/100000000000000003?whisper_translation=v1&revision=a430d84680aabd0b&lang=ko)\n안녕하세요";
        let mut message = Message {
            id: "100000000000000005".into(),
            author_id: W.into(),
            bot: true,
            content: legacy.into(),
            ..Message::default()
        };
        normalize(&mut message, C, W);
        assert_eq!(message.parent_id.as_deref(), Some(S));
        assert_eq!(message.content, "안녕하세요");
        assert_eq!(
            message.translation.as_ref().unwrap().revision,
            revision("hello")
        );

        let mut mixed = Message {
            id: "100000000000000005".into(),
            author_id: W.into(),
            bot: true,
            content: legacy.replace(LEGACY_TAG, TAG),
            ..Message::default()
        };
        normalize(&mut mixed, C, W);
        assert!(mixed.translation.is_none());
        assert!(mixed.parent_id.is_none());
    }
    #[test]
    fn malformed_foreign_and_self_links_are_not_normalized() {
        for replacement in [
            "https://evil.example/channels/",
            "https://discord.com.evil/channels/",
        ] {
            let mut m = envelope();
            m.content = m
                .content
                .replace("https://discord.com/channels/", replacement);
            normalize(&mut m, C, W);
            assert!(m.translation.is_none());
        }
        let mut m = envelope();
        m.id = S.into();
        normalize(&mut m, C, W);
        assert!(m.translation.is_none());
        let mut m = envelope();
        m.content = m.content.replace("revision=", "revision=G");
        normalize(&mut m, C, W);
        assert!(m.translation.is_none());
    }
    #[test]
    fn output_limit_counts_utf16_and_never_truncates() {
        assert!(encode(G, C, S, &revision("hello"), "en", &"😀".repeat(1000)).is_err());
        assert_ne!(revision("hello"), revision("edited"));
        assert_eq!(revision(""), "cbf29ce484222325");
    }
}
