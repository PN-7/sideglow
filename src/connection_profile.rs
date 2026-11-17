// SPDX-License-Identifier: MPL-2.0
//! Portable connection information for a trusted friend.
//!
//! Files intentionally contain the incoming-webhook URL in plaintext so another
//! Windows account can use it. Share codes contain the same sending credential
//! in reversible base64url, not encryption or an authenticated signature. OAuth
//! tokens, relay keys, AI credentials/preferences, game paths and UI preferences
//! are never exported. Import only parses data; it does not connect or log in.

use crate::config::{self, ConnectionMode, Settings};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_FILE_NAME: &str = "Sideglow-connection.sideglow.json";
const MAX_BYTES: usize = 16 * 1024;
const FORMAT: &str = "Sideglow connection";
const LEGACY_FORMAT: &str = "Whisper connection";
const VERSION: u32 = 1;
/// Maximum pasted UTF-8 input size, including optional ASCII line wrapping.
/// The UI must bound its input before handing it to the portable parser.
pub const MAX_CODE_TEXT_BYTES: usize = 32 * 1024;
const CODE_PREFIX: &str = "SIDEGLOW1:";
const LEGACY_CODE_PREFIX: &str = "WHISPER1:";
const MAX_CODE_PAYLOAD_BYTES: usize = (MAX_BYTES * 4).div_ceil(3);
const MAX_CODE_COMPACT_BYTES: usize = CODE_PREFIX.len() + MAX_CODE_PAYLOAD_BYTES + 1 + 8;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// Deliberately no Debug implementation: webhook_url is a credential.
#[derive(Clone)]
pub struct Profile {
    pub discord_app_id: String,
    pub channel_url: String,
    pub webhook_url: String,
}

impl Profile {
    /// Changes the selected connection and its three shared values. Translation
    /// execution is disabled so a newly imported channel cannot use an old key
    /// without an explicit local choice. Other translation preferences survive.
    /// This operation neither persists settings nor starts a connection.
    pub fn apply_to(&self, current: &Settings) -> Settings {
        let mut settings = current.clone();
        settings.connection_mode = ConnectionMode::Rpc;
        if settings.discord_app_id != self.discord_app_id {
            for tab in &mut settings.channel_tabs {
                tab.translation_enabled = false;
            }
        }
        settings.discord_app_id.clone_from(&self.discord_app_id);
        settings.channel_url.clone_from(&self.channel_url);
        settings.webhook_url.clone_from(&self.webhook_url);
        settings.translation.enabled = false;
        settings
    }

    /// Parse values pasted directly by a friend. This performs local format
    /// checks only: no file, login, persistence or network access is involved.
    /// An empty webhook deliberately creates a receive-only connection.
    pub fn from_manual(app_id: &str, channel: &str, webhook: &str) -> Result<Self, String> {
        let app_id = app_id.trim();
        let channel = channel.trim();
        let webhook = webhook.trim();
        if !config::snowflake(app_id) {
            return Err(crate::i18n::text(crate::i18n::Key::M92D39636321D));
        }
        if channel.len() > 2048 || webhook.len() > 2048 {
            return Err(crate::i18n::text(crate::i18n::Key::M75A2DC5EC175));
        }
        let channel_url = config::canonical_channel_url(channel)?;
        if !webhook.is_empty() {
            crate::discord_http::Webhook::parse(webhook)?;
        }
        Ok(Self {
            discord_app_id: app_id.into(),
            channel_url,
            webhook_url: webhook.into(),
        })
    }

    fn validated(app_id: &str, channel: &str, webhook: &str) -> Result<Self, String> {
        let profile = Self::from_manual(app_id, channel, webhook)?;
        if profile.webhook_url.is_empty() {
            return Err(crate::i18n::text(crate::i18n::Key::ME431F8809DA0));
        }
        Ok(profile)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedProfile {
    format: String,
    version: u32,
    discord_app_id: String,
    channel_url: String,
    webhook_url: String,
}

/// Export a copyable connection code without file or network access.
///
/// V1 wire format: `SIDEGLOW1:<base64url>.<crc32>`. The payload is the compact
/// UTF-8 JSON representation of `SavedProfile`, encoded with the RFC 4648 URL
/// alphabet, without padding and with zero unused tail bits. The checksum is
/// eight lowercase hexadecimal digits of CRC-32/ISO-HDLC over the decoded JSON
/// bytes (reflected polynomial 0xEDB88320, initial/final XOR 0xFFFFFFFF).
/// It catches copying damage only: anybody can decode, edit and re-checksum a
/// code. Treat the entire code as a webhook sending credential, not a password
/// vault or proof of who supplied it. Empty webhooks allow receive-only use.
pub fn export_code(settings: &Settings) -> Result<String, String> {
    if settings.connection_mode != ConnectionMode::Rpc {
        return Err(crate::i18n::text(crate::i18n::Key::M4D17124C7C5F));
    }
    let profile = Profile::from_manual(
        &settings.discord_app_id,
        &settings.channel_url,
        &settings.webhook_url,
    )?;
    let bytes = serde_json::to_vec(&SavedProfile {
        format: FORMAT.into(),
        version: VERSION,
        discord_app_id: profile.discord_app_id,
        channel_url: profile.channel_url,
        webhook_url: profile.webhook_url,
    })
    .map_err(|_| crate::i18n::text(crate::i18n::Key::M17383705F4AB))?;
    if bytes.len() > MAX_BYTES {
        return Err(code_size_error());
    }
    Ok(format!(
        "{CODE_PREFIX}{}.{:08x}",
        code_base64_encode(&bytes),
        code_crc32(&bytes)
    ))
}

/// Parse an opaque code locally. Surrounding and line-wrapping ASCII whitespace
/// is ignored, within `MAX_CODE_TEXT_BYTES`; Unicode whitespace and base64
/// padding are deliberately rejected. Lengths are checked before allocating
/// normalized text or decoded bytes. JSON rejects unknown and duplicate fields.
/// As with manual entry, an empty webhook is allowed. Legacy `WHISPER1:` codes
/// are accepted with their original payload format; prefix and format names
/// must match. JSON file import continues to require a nonempty webhook.
pub fn import_code(text: &str) -> Result<Profile, String> {
    if text.len() > MAX_CODE_TEXT_BYTES {
        return Err(code_size_error());
    }
    let compact_len = text.bytes().filter(|b| !code_whitespace(*b)).count();
    if compact_len > MAX_CODE_COMPACT_BYTES {
        return Err(code_size_error());
    }
    if !text.is_ascii() {
        return Err(invalid_code());
    }
    let mut compact = String::with_capacity(compact_len);
    for byte in text.bytes().filter(|b| !code_whitespace(*b)) {
        compact.push(char::from(byte));
    }
    let (rest, expected_format) = compact
        .strip_prefix(CODE_PREFIX)
        .map(|rest| (rest, FORMAT))
        .or_else(|| {
            compact
                .strip_prefix(LEGACY_CODE_PREFIX)
                .map(|rest| (rest, LEGACY_FORMAT))
        })
        .ok_or_else(code_version_error)?;
    let (encoded, checksum) = rest.split_once('.').ok_or_else(invalid_code)?;
    if checksum.len() != 8 || !checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid_code());
    }
    let checksum = u32::from_str_radix(checksum, 16).map_err(|_| invalid_code())?;
    let bytes = code_base64_decode(encoded).ok_or_else(invalid_code)?;
    if code_crc32(&bytes) != checksum {
        return Err(crate::i18n::text(crate::i18n::Key::M92BC4E8FFBEC));
    }
    let saved: SavedProfile = serde_json::from_slice(&bytes).map_err(|_| invalid_code())?;
    if saved.format != expected_format || saved.version != VERSION {
        return Err(code_version_error());
    }
    Profile::from_manual(
        &saved.discord_app_id,
        &saved.channel_url,
        &saved.webhook_url,
    )
}

fn code_size_error() -> String {
    crate::i18n::text(crate::i18n::Key::MC89FD9083A35)
}

fn invalid_code() -> String {
    crate::i18n::text(crate::i18n::Key::M497EEEDE332C)
}

fn code_version_error() -> String {
    crate::i18n::text(crate::i18n::Key::M839260134529)
}

fn code_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t'..=b'\r')
}

fn code_base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        }
    }
    out
}

fn code_base64_digit(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

fn code_base64_decode(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() > MAX_CODE_PAYLOAD_BYTES || text.len() % 4 == 1 {
        return None;
    }
    let decoded_len = (text.len() / 4) * 3
        + match text.len() % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        };
    if decoded_len > MAX_BYTES {
        return None;
    }
    let mut out = Vec::with_capacity(decoded_len);
    for chunk in text.as_bytes().chunks(4) {
        // A one-byte final chunk was rejected above, before this allocation.
        let a = code_base64_digit(chunk[0])?;
        let b = code_base64_digit(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk.len() == 2 {
            if b & 0x0f != 0 {
                return None;
            }
            continue;
        }
        let c = code_base64_digit(chunk[2])?;
        out.push((b << 4) | (c >> 2));
        if chunk.len() == 3 {
            if c & 0x03 != 0 {
                return None;
            }
            continue;
        }
        let d = code_base64_digit(chunk[3])?;
        out.push((c << 6) | d);
    }
    Some(out)
}

fn code_crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb8_8320 } else { 0 };
        }
    }
    !crc
}

fn encode(settings: &Settings) -> Result<Vec<u8>, String> {
    if settings.connection_mode != ConnectionMode::Rpc {
        return Err(crate::i18n::text(crate::i18n::Key::M4D17124C7C5F));
    }
    let profile = Profile::validated(
        &settings.discord_app_id,
        &settings.channel_url,
        &settings.webhook_url,
    )?;
    let mut bytes = serde_json::to_vec_pretty(&SavedProfile {
        format: FORMAT.into(),
        version: VERSION,
        discord_app_id: profile.discord_app_id,
        channel_url: profile.channel_url,
        webhook_url: profile.webhook_url,
    })
    .map_err(|_| crate::i18n::text(crate::i18n::Key::M8792B43CD18F))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_BYTES {
        return Err(crate::i18n::text(crate::i18n::Key::ME142CB8EC4A4));
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<Profile, String> {
    if bytes.len() > MAX_BYTES {
        return Err(crate::i18n::text(crate::i18n::Key::M44EA416CF5A4));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let saved: SavedProfile = serde_json::from_slice(bytes)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M2CFC878697CC))?;
    if !matches!(saved.format.as_str(), FORMAT | LEGACY_FORMAT) || saved.version != VERSION {
        return Err(crate::i18n::text(crate::i18n::Key::M08198FE0D31F));
    }
    Profile::validated(
        &saved.discord_app_id,
        &saved.channel_url,
        &saved.webhook_url,
    )
}

/// Parse a selected friend profile without opening URLs or starting any network
/// activity. Reads at most 16 KiB + one sentinel byte, even if the file changes.
pub fn import(path: &Path) -> Result<Profile, String> {
    let file = File::open(path).map_err(|_| crate::i18n::text(crate::i18n::Key::M90A11DA7049C))?;
    let metadata = file
        .metadata()
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MBF79849F5258))?;
    if !metadata.is_file() {
        return Err(crate::i18n::text(crate::i18n::Key::MECA18B03793F));
    }
    if metadata.len() > MAX_BYTES as u64 {
        return Err(crate::i18n::text(crate::i18n::Key::M44EA416CF5A4));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M488F5FC5F9E4))?;
    decode(&bytes)
}

/// Atomically write a portable plaintext profile to a path already selected by
/// the user. The native Save dialog must handle overwrite confirmation.
pub fn export(settings: &Settings, path: &Path) -> Result<(), String> {
    let bytes = encode(settings)?;
    if path.file_name().is_none() {
        return Err(crate::i18n::text(crate::i18n::Key::M2BB320E34B98));
    }
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = TempOutput::create(parent)?;
    temp.file
        .as_mut()
        .unwrap()
        .write_all(&bytes)
        .and_then(|_| temp.file.as_ref().unwrap().sync_all())
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M170287E65276))?;
    drop(temp.file.take());
    config::replace_file(&temp.path, path)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MB03AA2F77361))
}

struct TempOutput {
    path: PathBuf,
    file: Option<File>,
}
impl TempOutput {
    fn create(parent: &Path) -> Result<Self, String> {
        for _ in 0..8 {
            let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = parent.join(format!(
                ".sideglow-connection-{}-{stamp:x}-{nonce:x}.tmp",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(crate::i18n::text(crate::i18n::Key::MC5985F01738E)),
            }
        }
        Err(crate::i18n::text(crate::i18n::Key::MA3BC38802326))
    }
}
impl Drop for TempOutput {
    fn drop(&mut self) {
        // Close before cleanup on Windows. After a successful rename this path
        // no longer exists; on failure, attempt to remove the incomplete temp.
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const WEBHOOK: &str = "https://discord.com/api/webhooks/345678901234567890/EXAMPLE_ONLY_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890";
    // Fixed legacy v1 fixtures, independent of the current exporter. All IDs
    // and webhook credentials here are examples, never live connection data.
    const LEGACY_PROFILE_JSON: &[u8] = br#"{"format":"Whisper connection","version":1,"discord_app_id":"123456789012345678","channel_url":"https://discord.com/channels/234567890123456789/345678901234567890","webhook_url":"https://discord.com/api/webhooks/345678901234567890/EXAMPLE_ONLY_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890"}"#;
    const LEGACY_SHARE_CODE: &str = "WHISPER1:eyJmb3JtYXQiOiJXaGlzcGVyIGNvbm5lY3Rpb24iLCJ2ZXJzaW9uIjoxLCJkaXNjb3JkX2FwcF9pZCI6IjEyMzQ1Njc4OTAxMjM0NTY3OCIsImNoYW5uZWxfdXJsIjoiaHR0cHM6Ly9kaXNjb3JkLmNvbS9jaGFubmVscy8yMzQ1Njc4OTAxMjM0NTY3ODkvMzQ1Njc4OTAxMjM0NTY3ODkwIiwid2ViaG9va191cmwiOiJodHRwczovL2Rpc2NvcmQuY29tL2FwaS93ZWJob29rcy8zNDU2Nzg5MDEyMzQ1Njc4OTAvRVhBTVBMRV9PTkxZX0FCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaMTIzNDU2Nzg5MCJ9.b6ad657b";

    fn settings() -> Settings {
        Settings {
            discord_app_id: "123456789012345678".into(),
            channel_url: "https://discord.com/channels/234567890123456789/345678901234567890"
                .into(),
            webhook_url: WEBHOOK.into(),
            relay_url: "https://old-relay.example.test".into(),
            access_key: "RELAY_KEY_MUST_NOT_BE_EXPORTED_0123456789".into(),
            target_exe: r"C:\Private\game.exe".into(),
            translation: config::TranslationSettings {
                enabled: true,
                protected_api_key: "PROTECTED_OPENAI_KEY_MUST_NOT_BE_EXPORTED".into(),
                glossary: "PRIVATE_GAME_GLOSSARY_MUST_NOT_BE_EXPORTED".into(),
                ..config::TranslationSettings::default()
            },
            ..Settings::default()
        }
    }

    #[test]
    fn exports_only_shared_connection_values_and_keeps_webhook_portable() {
        let encoded = encode(&settings()).unwrap();
        let text = String::from_utf8(encoded.clone()).unwrap();
        let parsed: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(parsed.as_object().unwrap().len(), 5);
        assert_eq!(parsed["format"], FORMAT);
        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["webhook_url"], WEBHOOK);
        assert!(!text.contains("RELAY_KEY_MUST_NOT_BE_EXPORTED"));
        assert!(!text.contains("Private"));
        assert!(!text.contains("old-relay"));
        assert!(!text.contains("OPENAI_KEY"));
        assert!(!text.contains("PRIVATE_GAME_GLOSSARY"));
        assert!(!text.contains("translation"));
        assert!(!text.contains("gpt-"));
        assert_eq!(
            decode(&encoded).unwrap().discord_app_id,
            settings().discord_app_id
        );
    }

    #[test]
    fn importing_connection_does_not_replace_personal_or_relay_preferences() {
        let profile = decode(&encode(&settings()).unwrap()).unwrap();
        let old = Settings {
            connection_mode: ConnectionMode::Relay,
            font_size: 24,
            background_opacity: 33,
            overlay_enabled: false,
            target_exe: r"D:\Games\my-game.exe".into(),
            relay_url: "https://my-relay.example.test".into(),
            access_key: "KEEP_THIS_LOCAL_RELAY_KEY_0123456789".into(),
            ..Settings::default()
        };
        let new = profile.apply_to(&old);
        assert_eq!(new.connection_mode, ConnectionMode::Rpc);
        assert_eq!(new.webhook_url, WEBHOOK);
        assert_eq!(new.channel_url, settings().channel_url);
        assert_eq!(new.target_exe, old.target_exe);
        assert_eq!(new.access_key, old.access_key);
        assert_eq!(new.relay_url, old.relay_url);
        assert_eq!(new.font_size, 24);
        assert_eq!(new.background_opacity, 33);
        assert!(!new.overlay_enabled);
        assert_eq!(old.connection_mode, ConnectionMode::Relay);
    }

    #[test]
    fn imported_channel_cannot_start_translation_with_existing_local_key() {
        let profile = decode(&encode(&settings()).unwrap()).unwrap();
        let mut old = settings();
        old.translation.show_received = false;
        old.translation.language_b = "ja".into();
        old.ensure_channel_tabs().unwrap();
        old.add_channel_tab(
            "두 번째",
            "https://discord.com/channels/234567890123456789/456789012345678901",
            WEBHOOK,
        )
        .unwrap();
        for tab in &mut old.channel_tabs {
            tab.translation_enabled = true;
        }
        old.discord_app_id = "777777777777777777".into();
        let new = profile.apply_to(&old);
        assert!(old.translation.enabled);
        assert!(!new.translation.enabled);
        assert!(new.channel_tabs.iter().all(|tab| !tab.translation_enabled));
        assert_eq!(
            new.translation.protected_api_key,
            old.translation.protected_api_key
        );
        assert_eq!(new.translation.glossary, old.translation.glossary);
        assert_eq!(new.translation.language_b, "ja");
        assert!(!new.translation.show_received);
    }

    #[test]
    fn rejects_malformed_versions_extra_fields_and_untrusted_urls_without_echo() {
        let base: Value = serde_json::from_slice(&encode(&settings()).unwrap()).unwrap();
        for (field, replacement) in [
            ("format", json!("not Sideglow")),
            ("version", json!(2)),
            ("oauth_token", json!("SECRET_FROM_UNTRUSTED_FILE")),
            ("discord_app_id", json!("123456789012345678&evil")),
            (
                "channel_url",
                json!("https://evil.test/channels/234567890123456789/345678901234567890"),
            ),
            (
                "channel_url",
                json!("https://discord.com/channels/@me/345678901234567890"),
            ),
            (
                "webhook_url",
                json!(WEBHOOK.replace("discord.com", "discord.com.evil.test")),
            ),
            ("webhook_url", json!(format!("{WEBHOOK}/../messages"))),
            ("webhook_url", json!("SECRET_FROM_UNTRUSTED_FILE")),
        ] {
            let mut document = base.clone();
            document[field] = replacement;
            assert!(decode(&serde_json::to_vec(&document).unwrap()).is_err());
        }
        let mut malicious = base.clone();
        malicious["webhook_url"] = json!("https://evil.test/SECRET_FROM_UNTRUSTED_FILE");
        let error = decode(&serde_json::to_vec(&malicious).unwrap())
            .err()
            .unwrap();
        assert!(!error.contains("SECRET_FROM_UNTRUSTED_FILE"));
        assert!(!error.contains("evil.test"));
        assert!(decode(b"{not json}").is_err());
    }

    #[test]
    fn normalizes_full_channel_links_and_rejects_bare_ids_for_sharing() {
        let mut s = settings();
        s.channel_url.push_str("/456789012345678901?x=ignored");
        assert_eq!(
            decode(&encode(&s).unwrap()).unwrap().channel_url,
            settings().channel_url
        );
        s.channel_url = "345678901234567890".into();
        assert!(encode(&s).is_err());
    }

    #[test]
    fn pasted_links_use_a_saved_app_id_and_allow_receive_only_without_a_file() {
        let s = settings();
        let received = Profile::from_manual(
            &format!(" {} ", s.discord_app_id),
            &format!("{}/456789012345678901?ignored=1", s.channel_url),
            "   ",
        )
        .unwrap();
        assert_eq!(received.discord_app_id, s.discord_app_id);
        assert_eq!(received.channel_url, s.channel_url);
        assert!(received.webhook_url.is_empty());
        let sending = Profile::from_manual(&s.discord_app_id, &s.channel_url, WEBHOOK).unwrap();
        assert_eq!(sending.webhook_url, WEBHOOK);
        assert!(Profile::from_manual("", &s.channel_url, WEBHOOK).is_err());
        assert!(Profile::from_manual(&s.discord_app_id, "345678901234567890", WEBHOOK).is_err());
        let error =
            Profile::from_manual(&s.discord_app_id, &s.channel_url, "PRIVATE_INVALID_SECRET")
                .err()
                .unwrap();
        assert!(!error.contains("PRIVATE_INVALID_SECRET"));
        // The optional file format continues to require a sending credential.
        let mut for_export = s;
        for_export.webhook_url.clear();
        assert!(encode(&for_export).is_err());
    }

    fn code_for_payload(bytes: &[u8]) -> String {
        format!(
            "{CODE_PREFIX}{}.{:08x}",
            code_base64_encode(bytes),
            code_crc32(bytes)
        )
    }

    fn code_payload(code: &str) -> Vec<u8> {
        let (encoded, _) = code
            .strip_prefix(CODE_PREFIX)
            .unwrap()
            .split_once('.')
            .unwrap();
        code_base64_decode(encoded).unwrap()
    }

    #[test]
    fn code_roundtrip_normalizes_links_and_supports_receive_only() {
        let mut s = settings();
        s.channel_url = format!(
            " {}/456789012345678901?ignored=1 ",
            s.channel_url.replace("discord.com", "ptb.discord.com")
        );
        s.discord_app_id = format!(" {} ", s.discord_app_id);
        s.webhook_url = format!(" {WEBHOOK} ");
        let code = export_code(&s).unwrap();
        assert!(code.starts_with(CODE_PREFIX));
        assert!(code.len() < MAX_CODE_TEXT_BYTES);
        assert!(code.is_ascii());
        assert!(!code.bytes().any(|b| b.is_ascii_whitespace()));
        assert!(!code.contains('='));
        let profile = import_code(&code).unwrap();
        assert_eq!(profile.discord_app_id, settings().discord_app_id);
        assert_eq!(profile.channel_url, settings().channel_url);
        assert_eq!(profile.webhook_url, WEBHOOK);
        assert_eq!(export_code(&s).unwrap(), code);

        s.webhook_url.clear();
        let read_only = export_code(&s).unwrap();
        assert!(import_code(&read_only).unwrap().webhook_url.is_empty());
        // Existing JSON-file semantics are intentionally unchanged.
        assert!(encode(&s).is_err());
        assert!(decode(&code_payload(&read_only)).is_err());
    }

    #[test]
    fn legacy_json_and_code_preserve_local_credentials_and_preferences() {
        let directory = TestDirectory::new();
        let legacy_path = directory.0.join("Whisper-connection.whisper.json");
        fs::write(&legacy_path, LEGACY_PROFILE_JSON).unwrap();
        let profiles = [
            import(&legacy_path).unwrap(),
            import_code(LEGACY_SHARE_CODE).unwrap(),
        ];
        let mut local = settings();
        local.discord_app_id = "777777777777777777".into();
        local.ui_language = "ja".into();
        local.font_family = "Yu Gothic UI".into();
        local.font_size = 23;
        local.background_opacity = 39;
        local.translation.language_a = "de".into();
        local.translation.language_b = "ja".into();
        local.translation.show_received = false;
        local.translation.model = "LOCAL_MODEL_PREFERENCE".into();
        local.webhook_url = WEBHOOK.replace("EXAMPLE_ONLY", "LOCAL_ONLY");
        local.ensure_channel_tabs().unwrap();
        local
            .add_channel_tab(
                "Keep my other tab",
                "https://discord.com/channels/234567890123456789/456789012345678901",
                WEBHOOK,
            )
            .unwrap();
        for tab in &mut local.channel_tabs {
            tab.translation_enabled = true;
        }
        local.translation.enabled = true;
        local.connection_mode = ConnectionMode::Relay;
        let before = serde_json::to_value(&local).unwrap();

        for profile in profiles {
            assert_eq!(profile.discord_app_id, settings().discord_app_id);
            assert_eq!(profile.channel_url, settings().channel_url);
            assert_eq!(profile.webhook_url, WEBHOOK);
            let imported = profile.apply_to(&local);
            let mut expected = before.clone();
            expected["connection_mode"] = json!("rpc");
            expected["discord_app_id"] = json!(profile.discord_app_id);
            expected["channel_url"] = json!(profile.channel_url);
            expected["translation"]["enabled"] = json!(false);
            for tab in expected["channel_tabs"].as_array_mut().unwrap() {
                tab["translation_enabled"] = json!(false);
            }
            assert_eq!(serde_json::to_value(&imported).unwrap(), expected);
            // These credentials are deliberately omitted from Settings JSON.
            assert_eq!(imported.webhook_url, WEBHOOK);
            assert_eq!(imported.access_key, local.access_key);
            assert_eq!(
                imported.translation.protected_api_key,
                local.translation.protected_api_key
            );
            for (old_tab, new_tab) in local.channel_tabs.iter().zip(&imported.channel_tabs) {
                assert_eq!(new_tab.webhook_url, old_tab.webhook_url);
            }
            let exported: Value = serde_json::from_slice(&encode(&imported).unwrap()).unwrap();
            assert_eq!(exported["format"], "Sideglow connection");
            assert!(export_code(&imported).unwrap().starts_with("SIDEGLOW1:"));
        }
        assert_eq!(serde_json::to_value(&local).unwrap(), before);
        assert_eq!(fs::read(&legacy_path).unwrap(), LEGACY_PROFILE_JSON);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn new_and_legacy_codes_keep_exact_prefix_format_and_version_pairs() {
        let current = export_code(&settings()).unwrap();
        assert!(import_code(&current.replacen(CODE_PREFIX, LEGACY_CODE_PREFIX, 1)).is_err());
        assert!(
            import_code(&LEGACY_SHARE_CODE.replacen(LEGACY_CODE_PREFIX, CODE_PREFIX, 1)).is_err()
        );
        for prefix in ["WHISPER0:", "WHISPER2:", "WHISPER01:", "whisper1:"] {
            assert!(
                import_code(&LEGACY_SHARE_CODE.replacen(LEGACY_CODE_PREFIX, prefix, 1)).is_err()
            );
        }
        let mut legacy: Value = serde_json::from_slice(LEGACY_PROFILE_JSON).unwrap();
        for version in [json!(0), json!(2), json!("1")] {
            legacy["version"] = version;
            let bytes = serde_json::to_vec(&legacy).unwrap();
            let code = code_for_payload(&bytes).replacen(CODE_PREFIX, LEGACY_CODE_PREFIX, 1);
            assert!(import_code(&code).is_err());
            assert!(decode(&bytes).is_err());
        }
    }

    #[test]
    fn code_export_contains_only_the_three_connection_fields() {
        let mut s = settings();
        let code = export_code(&s).unwrap();
        let bytes = code_payload(&code);
        let document: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            document,
            json!({
                "format": FORMAT,
                "version": VERSION,
                "discord_app_id": s.discord_app_id,
                "channel_url": s.channel_url,
                "webhook_url": WEBHOOK,
            })
        );
        let plain = String::from_utf8(bytes).unwrap();
        for private in [
            "RELAY_KEY_MUST_NOT_BE_EXPORTED",
            "PROTECTED_OPENAI_KEY_MUST_NOT_BE_EXPORTED",
            "PRIVATE_GAME_GLOSSARY_MUST_NOT_BE_EXPORTED",
            "Private",
            "old-relay",
            "translation",
            "ui_language",
            "oauth",
            "channel_tabs",
        ] {
            assert!(!plain.contains(private));
        }
        s.ui_language = "ja".into();
        s.translation.enabled = false;
        s.translation.protected_api_key = "A_DIFFERENT_PRIVATE_KEY".into();
        s.translation.model = "PRIVATE_CUSTOM_MODEL".into();
        s.translation.glossary.clear();
        s.relay_url.clear();
        s.access_key.clear();
        s.target_exe.clear();
        s.font_size = 32;
        s.channel_tabs.push(config::ChannelTab {
            id: 99,
            name: "PRIVATE_TAB_NAME".into(),
            channel_url: "PRIVATE_OTHER_CHANNEL".into(),
            webhook_url: "PRIVATE_OTHER_WEBHOOK".into(),
            translation_enabled: true,
        });
        // Changing any of these private fields does not even change the code.
        assert_eq!(export_code(&s).unwrap(), code);
    }

    #[test]
    fn code_accepts_ascii_line_wrapping_but_rejects_unicode_and_control_injection() {
        let code = export_code(&settings()).unwrap();
        let mut wrapped = String::from(" \t\r\n\x0b\x0c");
        for chunk in code.as_bytes().chunks(17) {
            wrapped.push_str(std::str::from_utf8(chunk).unwrap());
            wrapped.push_str("\r\n\t ");
        }
        assert_eq!(import_code(&wrapped).unwrap().webhook_url, WEBHOOK);
        for bad in [
            format!("\u{00a0}{code}"),
            format!("{code}\u{200b}"),
            format!("{code}\0"),
            format!("{code}\x1b"),
        ] {
            assert!(import_code(&bad).is_err());
        }
    }

    #[test]
    fn code_base64_and_crc_match_standard_vectors_and_reject_noncanonical_tail_bits() {
        for (plain, encoded) in [
            (&b"f"[..], "Zg"),
            (&b"fo"[..], "Zm8"),
            (&b"foo"[..], "Zm9v"),
            (&b"foob"[..], "Zm9vYg"),
            (&b"fooba"[..], "Zm9vYmE"),
            (&b"foobar"[..], "Zm9vYmFy"),
            (&[0xfb, 0xef, 0xff][..], "--__"),
        ] {
            assert_eq!(code_base64_encode(plain), encoded);
            assert_eq!(code_base64_decode(encoded).unwrap(), plain);
        }
        assert_eq!(code_crc32(b""), 0);
        assert_eq!(code_crc32(b"123456789"), 0xcbf4_3926);
        for malformed in ["", "A", "AAAAA", "Zg=", "Zg==", "Zg+", "/w", "Zh", "Zm9"] {
            assert!(code_base64_decode(malformed).is_none());
        }
        for length in 1..=512 {
            let bytes: Vec<_> = (0..length).map(|n| (n * 73) as u8).collect();
            assert_eq!(
                code_base64_decode(&code_base64_encode(&bytes)).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn code_detects_copy_damage_and_every_truncated_prefix() {
        let code = export_code(&settings()).unwrap();
        for length in 0..code.len() {
            assert!(import_code(&code[..length]).is_err());
        }
        let mut changed_payload = code.clone();
        changed_payload.replace_range(CODE_PREFIX.len()..CODE_PREFIX.len() + 1, "A");
        let mut changed_checksum = code.clone();
        let replacement = if code.ends_with('0') { "1" } else { "0" };
        changed_checksum.replace_range(code.len() - 1.., replacement);
        for bad in [changed_payload, changed_checksum] {
            let error = import_code(&bad).err().unwrap();
            assert!(!error.contains(WEBHOOK));
            assert!(!error.contains(&bad));
            assert!(!error.contains("EXAMPLE_ONLY"));
        }
        let (payload, checksum) = code.rsplit_once('.').unwrap();
        // Hexadecimal letter case does not change the checksum value.
        assert!(import_code(&format!("{payload}.{}", checksum.to_uppercase())).is_ok());
    }

    #[test]
    fn code_rejects_unknown_versions_fields_duplicate_fields_and_invalid_json() {
        let bytes = code_payload(&export_code(&settings()).unwrap());
        let base: Value = serde_json::from_slice(&bytes).unwrap();
        for (field, replacement) in [
            ("version", json!(0)),
            ("version", json!(2)),
            ("version", json!("1")),
            ("format", json!("SECRET_UNTRUSTED_FORMAT")),
            ("oauth_token", json!("SECRET_UNTRUSTED_TOKEN")),
            ("ui_language", json!("ja")),
            ("translation", json!({"enabled": true})),
            ("channel_url", Value::Null),
        ] {
            let mut changed = base.clone();
            changed[field] = replacement;
            let error = import_code(&code_for_payload(&serde_json::to_vec(&changed).unwrap()))
                .err()
                .unwrap();
            assert!(!error.contains("SECRET_UNTRUSTED"));
        }
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove("webhook_url");
        assert!(import_code(&code_for_payload(&serde_json::to_vec(&missing).unwrap())).is_err());
        let original = std::str::from_utf8(&bytes).unwrap();
        let webhook_json = serde_json::to_string(WEBHOOK).unwrap();
        for (field, value) in [
            ("version", "1"),
            ("webhook_url", webhook_json.as_str()),
            ("webhook_\\u0075rl", webhook_json.as_str()),
        ] {
            let duplicate = format!("{{\"{field}\":{value},{}", &original[1..]);
            assert!(import_code(&code_for_payload(duplicate.as_bytes())).is_err());
        }
        for malformed in [&b"{bad}"[..], &b"[]"[..], &b"{} {}"[..], &[0xff][..]] {
            assert!(import_code(&code_for_payload(malformed)).is_err());
        }
        let code = export_code(&settings()).unwrap();
        for bad in [
            code.replace(CODE_PREFIX, "SIDEGLOW2:"),
            code.replace(CODE_PREFIX, "SIDEGLOW01:"),
            code.replace(CODE_PREFIX, "sideglow1:"),
            format!("{code}.00000000"),
            format!("{code}x"),
            "SIDEGLOW1:Zg.abcdefgh".into(),
            "SIDEGLOW1:.00000000".into(),
        ] {
            assert!(import_code(&bad).is_err());
        }
    }

    #[test]
    fn code_rejects_untrusted_destinations_even_with_a_valid_checksum() {
        let base: Value =
            serde_json::from_slice(&code_payload(&export_code(&settings()).unwrap())).unwrap();
        for (field, replacement) in [
            (
                "discord_app_id",
                "123456789012345678&SECRET_UNTRUSTED".into(),
            ),
            (
                "channel_url",
                settings().channel_url.replace("https:", "http:"),
            ),
            (
                "channel_url",
                settings()
                    .channel_url
                    .replace("discord.com", "discord.com.evil.test"),
            ),
            (
                "channel_url",
                settings()
                    .channel_url
                    .replace("discord.com", "discord.com@evil.test"),
            ),
            (
                "channel_url",
                "https://discord.com/channels/@me/345678901234567890".into(),
            ),
            ("channel_url", "file:///SECRET_UNTRUSTED".into()),
            ("channel_url", "345678901234567890".into()),
            ("webhook_url", WEBHOOK.replace("https:", "http:")),
            (
                "webhook_url",
                WEBHOOK.replace("discord.com", "discord.com.evil.test"),
            ),
            (
                "webhook_url",
                WEBHOOK.replace("discord.com", "discord.com@evil.test"),
            ),
            ("webhook_url", format!("{WEBHOOK}/../messages")),
            ("webhook_url", format!("{WEBHOOK}?secret=SECRET_UNTRUSTED")),
            ("webhook_url", "javascript:SECRET_UNTRUSTED".into()),
            ("webhook_url", format!("{WEBHOOK}\0SECRET_UNTRUSTED")),
        ] {
            let mut changed = base.clone();
            changed[field] = json!(replacement);
            let error = import_code(&code_for_payload(&serde_json::to_vec(&changed).unwrap()))
                .err()
                .unwrap();
            assert!(!error.contains("SECRET_UNTRUSTED"));
            assert!(!error.contains("evil.test"));
            assert!(!error.contains("EXAMPLE_ONLY"));
        }
    }

    #[test]
    fn code_bounds_raw_text_payload_and_connection_fields() {
        assert!(import_code(&" ".repeat(MAX_CODE_TEXT_BYTES + 1)).is_err());
        let code = export_code(&settings()).unwrap();
        let bounded = format!("{}{code}", " ".repeat(MAX_CODE_TEXT_BYTES - code.len()));
        assert!(import_code(&bounded).is_ok());
        assert!(import_code(&"A".repeat(MAX_CODE_COMPACT_BYTES + 1)).is_err());
        assert!(code_base64_decode(&"A".repeat(MAX_CODE_PAYLOAD_BYTES + 1)).is_none());
        assert!(import_code(&code_for_payload(&vec![b' '; MAX_BYTES + 1])).is_err());
        let maximum = vec![b'a'; MAX_BYTES];
        assert_eq!(
            code_base64_decode(&code_base64_encode(&maximum)).unwrap(),
            maximum
        );
        let mut s = settings();
        s.channel_url.push_str(&format!("?{}", "a".repeat(2048)));
        assert!(export_code(&s).is_err());
        let mut s = settings();
        s.webhook_url.push_str(&"a".repeat(2048));
        assert!(export_code(&s).is_err());
        let mut s = settings();
        s.connection_mode = ConnectionMode::Relay;
        assert!(export_code(&s).is_err());
    }

    #[test]
    fn code_rejects_equivalent_but_noncanonical_base64_with_correct_checksum() {
        let mut bytes = code_payload(&export_code(&settings()).unwrap());
        for tail_bytes in [1, 2] {
            while bytes.len() % 3 != tail_bytes {
                bytes.push(b' ');
            }
            let canonical = code_for_payload(&bytes);
            assert!(import_code(&canonical).is_ok());
            let (body, checksum) = canonical.rsplit_once('.').unwrap();
            let last = *body.as_bytes().last().unwrap();
            let value = code_base64_digit(last).unwrap();
            assert_eq!(value & if tail_bytes == 1 { 15 } else { 3 }, 0);
            const TABLE: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let noncanonical = format!(
                "{}{}.{checksum}",
                &body[..body.len() - 1],
                TABLE[(value | 1) as usize] as char
            );
            assert!(import_code(&noncanonical).is_err());
        }
    }

    #[test]
    fn code_application_preserves_personal_settings_and_disables_old_translation_consent() {
        let profile = import_code(&export_code(&settings()).unwrap()).unwrap();
        let mut old = settings();
        old.ui_language = "ja".into();
        old.discord_app_id = "777777777777777777".into();
        old.font_size = 32;
        old.target_exe = "PRIVATE_LOCAL_GAME".into();
        old.translation.language_a = "de".into();
        old.translation.language_b = "ja".into();
        old.ensure_channel_tabs().unwrap();
        old.add_channel_tab(
            "나의 다른 탭",
            "https://discord.com/channels/234567890123456789/456789012345678901",
            WEBHOOK,
        )
        .unwrap();
        for tab in &mut old.channel_tabs {
            tab.translation_enabled = true;
        }
        old.translation.enabled = true;
        old.connection_mode = ConnectionMode::Relay;
        let imported = profile.apply_to(&old);
        let mut expected = serde_json::to_value(&old).unwrap();
        expected["connection_mode"] = json!("rpc");
        expected["discord_app_id"] = json!(profile.discord_app_id);
        expected["channel_url"] = json!(profile.channel_url);
        expected["translation"]["enabled"] = json!(false);
        for tab in expected["channel_tabs"].as_array_mut().unwrap() {
            tab["translation_enabled"] = json!(false);
        }
        assert_eq!(serde_json::to_value(&imported).unwrap(), expected);
        assert_eq!(imported.webhook_url, WEBHOOK);
        assert_eq!(imported.access_key, old.access_key);
        assert_eq!(
            imported.translation.protected_api_key,
            old.translation.protected_api_key
        );
        assert_eq!(imported.active_channel_tab, old.active_channel_tab);
        for (before, after) in old.channel_tabs.iter().zip(&imported.channel_tabs) {
            assert_eq!(before.webhook_url, after.webhook_url);
        }
        assert!(old.translation.enabled);
        assert!(old.channel_tabs.iter().all(|tab| tab.translation_enabled));
        assert_eq!(old.connection_mode, ConnectionMode::Relay);
    }

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sideglow-profile-test-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn file_roundtrip_overwrite_and_failed_export_preserve_existing_data() {
        let directory = TestDirectory::new();
        let path = directory.0.join(DEFAULT_FILE_NAME);
        fs::write(&path, b"old profile").unwrap();
        export(&settings(), &path).unwrap();
        assert_eq!(import(&path).unwrap().webhook_url, WEBHOOK);
        let before = fs::read(&path).unwrap();
        let mut bad = settings();
        bad.webhook_url.clear();
        assert!(export(&bad, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let blocked = directory.0.join("blocked.sideglow.json");
        fs::create_dir(&blocked).unwrap();
        assert!(export(&settings(), &blocked).is_err());
        // Only the prior valid file and the blocking directory remain. The
        // failed replacement's same-directory plaintext temporary file is gone.
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
    }

    #[test]
    fn oversized_file_is_rejected_before_parsing() {
        assert!(decode(&vec![b' '; MAX_BYTES + 1]).is_err());
        let directory = TestDirectory::new();
        let path = directory.0.join(DEFAULT_FILE_NAME);
        fs::write(&path, vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(import(&path).is_err());
    }
}
