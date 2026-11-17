// SPDX-License-Identifier: MPL-2.0
use crate::api_provider::{self, ApiProvider};
use serde::{Deserialize, Serialize};
use std::{fmt, fs, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    #[default]
    Rpc,
    Relay,
}

pub const MAX_CHANNEL_TABS: usize = 8;
const MAX_TAB_NAME_UTF16: usize = 32;
const MAX_CONNECTION_URL_BYTES: usize = 2048;

/// A saved destination, not a live connection. Only the selected tab is projected
/// into Settings' legacy connection fields and passed to the single Client.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelTab {
    pub id: u64,
    pub name: String,
    pub channel_url: String,
    #[serde(skip)]
    pub webhook_url: String,
    #[serde(default)]
    pub translation_enabled: bool,
}

impl fmt::Debug for ChannelTab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelTab")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("channel_url", &self.channel_url)
            .field("webhook_url", &"[redacted]")
            .field("translation_enabled", &self.translation_enabled)
            .finish()
    }
}

pub fn validate_channel_tab_name(name: &str) -> Result<(), String> {
    if name.chars().any(char::is_control) {
        return Err(crate::i18n::text(crate::i18n::Key::M0C2AE1B2F385));
    }
    let name = name.trim();
    if name.is_empty() || name.encode_utf16().count() > MAX_TAB_NAME_UTF16 {
        return Err(crate::i18n::text(crate::i18n::Key::M5800D52C32C7));
    }
    Ok(())
}

/// A full, reviewable server/channel link for newly added or edited tabs.
/// Existing single-channel files may still contain a legacy bare channel ID.
pub fn canonical_channel_url(link: &str) -> Result<String, String> {
    let link = link.trim();
    if link.len() > MAX_CONNECTION_URL_BYTES {
        return Err(crate::i18n::text(crate::i18n::Key::M0F85B3808A97));
    }
    let channel = channel_id(link)?;
    let path = link
        .strip_prefix("https://")
        .and_then(|value| value.split_once('/').map(|(_, path)| path))
        .ok_or(crate::i18n::text(crate::i18n::Key::MC74DF18E2175))?;
    let guild = path
        .split('/')
        .nth(1)
        .filter(|id| snowflake(id))
        .ok_or(crate::i18n::text(crate::i18n::Key::M90DFF02BFC6E))?;
    Ok(format!("https://discord.com/channels/{guild}/{channel}"))
}

fn tab_connection(
    name: &str,
    channel: &str,
    webhook: &str,
) -> Result<(String, String, String), String> {
    validate_channel_tab_name(name)?;
    let channel = canonical_channel_url(channel)?;
    let webhook = webhook.trim();
    if webhook.len() > MAX_CONNECTION_URL_BYTES {
        return Err(crate::i18n::text(crate::i18n::Key::M50915D137897));
    }
    if !webhook.is_empty() {
        crate::discord_http::Webhook::parse(webhook)?;
    }
    Ok((name.trim().into(), channel, webhook.into()))
}

fn same_channel(a: &str, b: &str) -> bool {
    match (channel_id(a), channel_id(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.trim() == b.trim(),
    }
}

/// Per-PC translation preferences. The encrypted credential is deliberately
/// excluded from ordinary settings/profile serialization and is never opened
/// while reading the preferences file.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TranslationSettings {
    pub enabled: bool,
    pub show_received: bool,
    pub provider: ApiProvider,
    /// Empty selects the official provider default. Compatible services must
    /// provide their HTTPS base URL explicitly.
    pub api_base_url: String,
    pub model: String,
    pub fast_mode: bool,
    pub language_a: String,
    pub language_b: String,
    pub glossary: String,
    #[serde(skip)]
    pub protected_api_key: String,
    #[serde(skip)]
    pub credential_identity: Option<TranslationCredentialIdentity>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranslationCredentialIdentity {
    pub provider: ApiProvider,
    pub api_base_url: String,
}

impl Default for TranslationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            show_received: true,
            provider: ApiProvider::OpenAi,
            api_base_url: String::new(),
            model: "gpt-6-luna".into(),
            fast_mode: false,
            language_a: "ko".into(),
            language_b: "en".into(),
            glossary: String::new(),
            protected_api_key: String::new(),
            credential_identity: None,
        }
    }
}

impl fmt::Debug for TranslationSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranslationSettings")
            .field("enabled", &self.enabled)
            .field("show_received", &self.show_received)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("fast_mode", &self.fast_mode)
            .field("language_a", &self.language_a)
            .field("language_b", &self.language_b)
            .field("glossary", &"[redacted]")
            .field("protected_api_key", &"[redacted]")
            .finish()
    }
}

pub const TRANSLATION_LANGUAGES: &[(&str, crate::i18n::Key)] = &[
    ("ko", crate::i18n::Key::M5088E7A72444),
    ("en", crate::i18n::Key::M01DA1E06B01F),
    ("ja", crate::i18n::Key::MF3F5B013B672),
    ("zh", crate::i18n::Key::M9700B10FA0CB),
    ("es", crate::i18n::Key::M56E57041874C),
    ("fr", crate::i18n::Key::MF29674D12EC5),
    ("de", crate::i18n::Key::MFDB48A28A712),
    ("ru", crate::i18n::Key::M8AA3A6CFF485),
    ("vi", crate::i18n::Key::MCBE7405F8A73),
    ("th", crate::i18n::Key::MFADCF70EE620),
    ("id", crate::i18n::Key::MA3AAE6DE2846),
    ("pt", crate::i18n::Key::M161A8266FA8D),
];

pub fn validate_translation(s: &TranslationSettings) -> Result<(), String> {
    let endpoint = api_provider::resolve(s.provider, &s.api_base_url)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::ApiEndpointInvalid))?;
    endpoint
        .request_path(s.provider, &s.model)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M565741F305A1))?;
    if s.provider != ApiProvider::OpenAi && s.fast_mode {
        return Err(crate::i18n::text(
            crate::i18n::Key::TranslationFastUnavailable,
        ));
    }
    if s.language_a == s.language_b
        || ![&s.language_a, &s.language_b].iter().all(|value| {
            TRANSLATION_LANGUAGES
                .iter()
                .any(|(code, _)| *code == value.as_str())
        })
    {
        return Err(crate::i18n::text(crate::i18n::Key::M86E23FA862FD));
    }
    if s.glossary.encode_utf16().count() > 2000 || s.glossary.contains('\0') {
        return Err(crate::i18n::text(crate::i18n::Key::M6225E9CB067E));
    }
    // Validation is intentionally not decryption. A disabled user incurs no
    // DPAPI work, and an unusable credential cannot reset unrelated preferences.
    if s.protected_api_key.len() > 16 * 1024 {
        return Err(crate::i18n::text(crate::i18n::Key::M25ADE0826FB6));
    }
    if s.enabled && s.protected_api_key.is_empty() {
        return Err(crate::i18n::text(crate::i18n::Key::M7ABFDDED5A2D));
    }
    if s.enabled {
        validate_translation_key_binding(s)?;
    }
    Ok(())
}

/// Check only the connection prerequisites for starting shared translation.
/// This never opens credentials, starts a worker, or contacts Discord.
pub fn validate_translation_connection(
    mode: ConnectionMode,
    channel_url: &str,
    webhook_url: &str,
) -> Result<(), String> {
    if mode != ConnectionMode::Rpc {
        return Err(crate::i18n::text(crate::i18n::Key::MEFED3B07CA7F));
    }
    channel_id(channel_url).map_err(|_| crate::i18n::text(crate::i18n::Key::MF518A6B2B80A))?;
    if !channel_url.trim().starts_with("https://") {
        return Err(crate::i18n::text(crate::i18n::Key::M877B540CE348));
    }
    crate::discord_http::Webhook::parse(webhook_url)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M3D7FB2040B8A))?;
    Ok(())
}

pub fn translation_credential_identity(
    provider: ApiProvider,
    api_base_url: &str,
) -> Result<TranslationCredentialIdentity, String> {
    let endpoint = api_provider::resolve(provider, api_base_url)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::ApiEndpointInvalid))?;
    Ok(TranslationCredentialIdentity {
        provider,
        api_base_url: endpoint.canonical_url,
    })
}

fn legacy_openai_identity() -> TranslationCredentialIdentity {
    translation_credential_identity(ApiProvider::OpenAi, "")
        .expect("the compiled OpenAI default is a valid endpoint")
}

/// Check the saved destination without opening DPAPI. This is also checked
/// immediately before decrypting a worker's credential, not only in the UI.
pub fn validate_translation_key_binding(s: &TranslationSettings) -> Result<(), String> {
    if s.protected_api_key.is_empty() {
        return Ok(());
    }
    let expected = translation_credential_identity(s.provider, &s.api_base_url)?;
    let actual = match &s.credential_identity {
        Some(identity) => {
            translation_credential_identity(identity.provider, &identity.api_base_url)?
        }
        None => legacy_openai_identity(),
    };
    if expected != actual {
        return Err(crate::i18n::text(
            crate::i18n::Key::TranslationCredentialMismatch,
        ));
    }
    Ok(())
}

const TRANSLATION_KEY_PREFIX: &str = "SIDEGLOW_API_KEY_V1\n";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundTranslationKey {
    identity: TranslationCredentialIdentity,
    api_key: String,
}

impl Drop for BoundTranslationKey {
    fn drop(&mut self) {
        wipe_string(&mut self.api_key);
    }
}

fn wipe_string(value: &mut String) {
    // Zero bytes are valid UTF-8, so the String remains valid before clearing.
    unsafe { value.as_mut_vec().fill(0) };
    value.clear();
}

pub fn store_translation_key(s: &mut TranslationSettings, raw: &str) -> Result<(), String> {
    store_translation_key_with(s, raw, protect)
}

fn store_translation_key_with(
    s: &mut TranslationSettings,
    raw: &str,
    mut protect_key: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        s.protected_api_key.clear();
        s.credential_identity = None;
        return Ok(());
    }
    if raw.len() > 1024 || !raw.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(crate::i18n::text(crate::i18n::Key::M71B88C894AF5));
    }
    let identity = translation_credential_identity(s.provider, &s.api_base_url)?;
    let payload = BoundTranslationKey {
        identity: identity.clone(),
        api_key: raw.to_owned(),
    };
    let mut plaintext = serde_json::to_string(&payload)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MC10B0269BD1C))?;
    plaintext.insert_str(0, TRANSLATION_KEY_PREFIX);
    let protected = protect_key(&plaintext);
    wipe_string(&mut plaintext);
    let protected = protected.map_err(|_| crate::i18n::text(crate::i18n::Key::MC10B0269BD1C))?;
    // Publish the new key and its identity together only after encryption.
    s.protected_api_key = encode_hex(&protected);
    s.credential_identity = Some(identity);
    Ok(())
}

/// Call only when the translation runtime is actually being started. Opening
/// preferences and reading the settings file never decrypt a saved credential.
pub fn load_translation_key(s: &TranslationSettings) -> Result<String, String> {
    load_translation_key_with(s, unprotect)
}

fn load_translation_key_with(
    s: &TranslationSettings,
    mut unprotect_key: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<String, String> {
    if !s.enabled {
        return Err(crate::i18n::text(crate::i18n::Key::M74359D77C8FA));
    }
    if s.protected_api_key.is_empty() {
        return Err(crate::i18n::text(crate::i18n::Key::M4ADB29BEE609));
    }
    validate_translation_key_binding(s)?;
    let encrypted = decode_hex(&s.protected_api_key)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MCAA4AADBF8B7))?;
    let mut plaintext = unprotect_key(&encrypted)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M642C0E8AAFAE))?;
    let result = translation_key_from_plaintext(s, &plaintext);
    wipe_string(&mut plaintext);
    result
}

fn translation_key_from_plaintext(
    s: &TranslationSettings,
    plaintext: &str,
) -> Result<String, String> {
    let expected = translation_credential_identity(s.provider, &s.api_base_url)?;
    let key = if let Some(json) = plaintext.strip_prefix(TRANSLATION_KEY_PREFIX) {
        let mut payload: BoundTranslationKey = serde_json::from_str(json)
            .map_err(|_| crate::i18n::text(crate::i18n::Key::MCAA4AADBF8B7))?;
        if payload.identity != expected {
            return Err(crate::i18n::text(
                crate::i18n::Key::TranslationCredentialMismatch,
            ));
        }
        std::mem::take(&mut payload.api_key)
    } else {
        // Historical encrypted values contain only the raw OpenAI key. They
        // may never be reassigned to a compatible/custom or another provider.
        if expected != legacy_openai_identity() {
            return Err(crate::i18n::text(
                crate::i18n::Key::TranslationCredentialMismatch,
            ));
        }
        plaintext.to_owned()
    };
    if key.is_empty() || key.len() > 1024 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        let mut key = key;
        wipe_string(&mut key);
        return Err(crate::i18n::text(crate::i18n::Key::MCAA4AADBF8B7));
    }
    Ok(key)
}

// Keep source compatibility for older tests/callers while all providers now
// pass through the same destination-bound implementation.
pub fn store_openai_key(s: &mut TranslationSettings, raw: &str) -> Result<(), String> {
    store_translation_key(s, raw)
}
pub fn load_openai_key(s: &TranslationSettings) -> Result<String, String> {
    load_translation_key(s)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Program interface language; independent of the chat translation pair.
    pub ui_language: String,
    /// Personal outgoing-message hint, independent of UI language and shared
    /// translation execution. Connection profiles never export this choice.
    pub romaji_japanese_input: bool,
    pub translation: TranslationSettings,
    pub connection_mode: ConnectionMode,
    pub discord_app_id: String,
    #[serde(skip)]
    pub webhook_url: String,
    pub relay_url: String,
    pub channel_url: String,
    pub channel_tabs: Vec<ChannelTab>,
    /// Stable tab identity, never an unchecked vector index. Zero means an
    /// uninitialized legacy/default preference that still needs migration.
    pub active_channel_tab: u64,
    #[serde(skip)]
    pub access_key: String,
    pub background_color: u32,
    pub text_color: u32,
    pub author_color: u32,
    pub translation_style_enabled: bool,
    pub translation_font_size: u32,
    pub translation_text_color: u32,
    pub background_opacity: u8,
    pub font_family: String,
    pub font_size: u32,
    /// Extra spacing in logical pixels, scaled with the window DPI.
    pub letter_spacing: i32,
    pub line_spacing: u32,
    pub message_spacing: u32,
    pub author_gap: u32,
    pub text_outline: bool,
    pub outline_color: u32,
    pub outline_width: u32,
    pub show_diagnostics: bool,
    pub overlay_enabled: bool,
    pub hide_when_idle: bool,
    pub hide_after_secs: u32,
    pub hotkey_vk: u32,
    pub hotkey_modifiers: u32,
    pub target_exe: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_language: "ko".into(),
            romaji_japanese_input: false,
            translation: TranslationSettings::default(),
            connection_mode: ConnectionMode::Rpc,
            discord_app_id: String::new(),
            webhook_url: String::new(),
            relay_url: String::new(),
            channel_url: String::new(),
            channel_tabs: Vec::new(),
            active_channel_tab: 0,
            access_key: String::new(),
            background_color: 0x111923,
            text_color: 0xEFF4FB,
            author_color: 0x8fe5d7,
            translation_style_enabled: false,
            translation_font_size: 14,
            translation_text_color: 0xFFE29A,
            background_opacity: 78,
            font_family: "맑은 고딕".into(),
            font_size: 12,
            letter_spacing: 0,
            line_spacing: 4,
            message_spacing: 12,
            author_gap: 0,
            text_outline: true,
            outline_color: 0x000000,
            outline_width: 1,
            show_diagnostics: false,
            overlay_enabled: true,
            hide_when_idle: true,
            hide_after_secs: 20,
            hotkey_vk: 13,
            hotkey_modifiers: 0,
            target_exe: String::new(),
            x: 80,
            y: 200,
            width: 500,
            height: 340,
        }
    }
}

impl Settings {
    /// One-time migration of the legacy connection. Do this before presenting
    /// tabs; it does not connect, decrypt credentials or write preferences.
    pub fn ensure_channel_tabs(&mut self) -> Result<(), String> {
        if self.channel_tabs.is_empty() {
            if self.active_channel_tab != 0 {
                return Err(crate::i18n::text(crate::i18n::Key::M61A523FB01BB));
            }
            self.channel_tabs.push(ChannelTab {
                id: 1,
                name: "기본 채널".into(),
                channel_url: self.channel_url.clone(),
                webhook_url: self.webhook_url.clone(),
                translation_enabled: self.translation.enabled,
            });
            self.active_channel_tab = 1;
        }
        validate_channel_tabs(self)
    }

    pub fn active_tab(&self) -> Option<&ChannelTab> {
        self.channel_tabs
            .iter()
            .find(|tab| tab.id == self.active_channel_tab)
    }

    /// The active projection stays authoritative for older setup/import paths.
    /// Retargeting it cannot carry a previous channel's paid-translation consent.
    pub fn sync_active_tab(&mut self) -> Result<(), String> {
        self.ensure_channel_tabs()?;
        if self.connection_mode == ConnectionMode::Relay {
            // Relay remains the existing single-channel path. Its active fields
            // must not overwrite destinations remembered for Discord RPC tabs.
            return Ok(());
        }
        let index = self
            .channel_tabs
            .iter()
            .position(|tab| tab.id == self.active_channel_tab)
            .ok_or(crate::i18n::text(crate::i18n::Key::M1ACC27ABDF0C))?;
        let tab = &mut self.channel_tabs[index];
        if !same_channel(&tab.channel_url, &self.channel_url)
            || tab.webhook_url.trim() != self.webhook_url.trim()
        {
            self.translation.enabled = false;
        }
        tab.channel_url.clone_from(&self.channel_url);
        tab.webhook_url.clone_from(&self.webhook_url);
        tab.translation_enabled = self.translation.enabled;
        Ok(())
    }

    pub fn select_channel_tab(&mut self, id: u64) -> Result<(), String> {
        if self.connection_mode != ConnectionMode::Rpc {
            return Err(crate::i18n::text(crate::i18n::Key::M04CBDB515754));
        }
        let mut next = self.clone();
        next.sync_active_tab()?;
        let selected = next
            .channel_tabs
            .iter()
            .find(|tab| tab.id == id)
            .cloned()
            .ok_or(crate::i18n::text(crate::i18n::Key::M1ACC27ABDF0C))?;
        next.project_tab(&selected);
        validate_channel_tabs(&next)?;
        *self = next;
        Ok(())
    }

    fn project_tab(&mut self, tab: &ChannelTab) {
        self.active_channel_tab = tab.id;
        self.channel_url.clone_from(&tab.channel_url);
        self.webhook_url.clone_from(&tab.webhook_url);
        self.translation.enabled =
            tab.translation_enabled && !self.translation.protected_api_key.is_empty();
        if let Some(selected) = self.channel_tabs.iter_mut().find(|item| item.id == tab.id) {
            selected.translation_enabled = self.translation.enabled;
        }
    }

    /// Add a destination without selecting or connecting it. The UI chooses
    /// explicitly whether to activate the returned stable identity afterward.
    pub fn add_channel_tab(
        &mut self,
        name: &str,
        channel: &str,
        webhook: &str,
    ) -> Result<u64, String> {
        if self.connection_mode != ConnectionMode::Rpc {
            return Err(crate::i18n::text(crate::i18n::Key::M84A54C1BF453));
        }
        let (name, channel_url, webhook_url) = tab_connection(name, channel, webhook)?;
        let mut next = self.clone();
        next.sync_active_tab()?;
        if next.channel_tabs.len() >= MAX_CHANNEL_TABS {
            return Err(crate::i18n::format(
                crate::i18n::Key::M272E7CAD215A,
                &[("MAX_CHANNEL_TABS", format!("{}", MAX_CHANNEL_TABS))],
            ));
        }
        if next
            .channel_tabs
            .iter()
            .any(|tab| same_channel(&tab.channel_url, &channel_url))
        {
            return Err(crate::i18n::text(crate::i18n::Key::MBCA17E510C3F));
        }
        let id = next
            .channel_tabs
            .iter()
            .map(|tab| tab.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(crate::i18n::text(crate::i18n::Key::M691B528A59A5))?;
        next.channel_tabs.push(ChannelTab {
            id,
            name,
            channel_url,
            webhook_url,
            translation_enabled: false,
        });
        *self = next;
        Ok(id)
    }

    pub fn edit_channel_tab(
        &mut self,
        id: u64,
        name: &str,
        channel: &str,
        webhook: &str,
    ) -> Result<(), String> {
        if self.connection_mode != ConnectionMode::Rpc {
            return Err(crate::i18n::text(crate::i18n::Key::MEC79E5E7DF9C));
        }
        let (name, channel_url, webhook_url) = tab_connection(name, channel, webhook)?;
        let mut next = self.clone();
        next.sync_active_tab()?;
        if next
            .channel_tabs
            .iter()
            .any(|tab| tab.id != id && same_channel(&tab.channel_url, &channel_url))
        {
            return Err(crate::i18n::text(crate::i18n::Key::M39CCDB9FC2AC));
        }
        let tab = next
            .channel_tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .ok_or(crate::i18n::text(crate::i18n::Key::M4003D94C8B6E))?;
        if !same_channel(&tab.channel_url, &channel_url) || tab.webhook_url != webhook_url {
            tab.translation_enabled = false;
        }
        tab.name = name;
        tab.channel_url = channel_url;
        tab.webhook_url = webhook_url;
        let selected = (id == next.active_channel_tab).then(|| tab.clone());
        if let Some(tab) = selected {
            next.project_tab(&tab);
        }
        *self = next;
        Ok(())
    }

    pub fn remove_channel_tab(&mut self, id: u64) -> Result<(), String> {
        if self.connection_mode != ConnectionMode::Rpc {
            return Err(crate::i18n::text(crate::i18n::Key::M6138AB93F4A1));
        }
        let mut next = self.clone();
        next.sync_active_tab()?;
        if next.channel_tabs.len() <= 1 {
            return Err(crate::i18n::text(crate::i18n::Key::MECD7A2E0F9D5));
        }
        let index = next
            .channel_tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or(crate::i18n::text(crate::i18n::Key::MD67E5347CE6E))?;
        next.channel_tabs.remove(index);
        if id == next.active_channel_tab {
            let tab = next.channel_tabs[index.saturating_sub(1)].clone();
            next.project_tab(&tab);
        }
        *self = next;
        Ok(())
    }

    /// Visual previews copy only this explicit whitelist. Connection, API,
    /// hotkey, on/off and geometry state remain authoritative until Save.
    pub fn apply_appearance_from(&mut self, draft: &Self) {
        self.background_color = draft.background_color;
        self.text_color = draft.text_color;
        self.author_color = draft.author_color;
        self.translation_style_enabled = draft.translation_style_enabled;
        self.translation_font_size = draft.translation_font_size;
        self.translation_text_color = draft.translation_text_color;
        self.background_opacity = draft.background_opacity;
        self.font_family.clone_from(&draft.font_family);
        self.font_size = draft.font_size;
        self.letter_spacing = draft.letter_spacing;
        self.line_spacing = draft.line_spacing;
        self.message_spacing = draft.message_spacing;
        self.author_gap = draft.author_gap;
        self.text_outline = draft.text_outline;
        self.outline_color = draft.outline_color;
        self.outline_width = draft.outline_width;
        self.show_diagnostics = draft.show_diagnostics;
        self.hide_when_idle = draft.hide_when_idle;
        self.hide_after_secs = draft.hide_after_secs;
    }
}

// Debug output must never expose the bearer key.
impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("ui_language", &self.ui_language)
            .field("romaji_japanese_input", &self.romaji_japanese_input)
            .field("connection_mode", &self.connection_mode)
            .field("discord_app_id", &self.discord_app_id)
            .field("webhook_url", &"[redacted]")
            .field("relay_url", &self.relay_url)
            .field("channel_url", &self.channel_url)
            .field("active_channel_tab", &self.active_channel_tab)
            .field("channel_tabs", &self.channel_tabs)
            .field("access_key", &"[redacted]")
            .field("target_exe", &self.target_exe)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct SavedSettings {
    version: u32,
    settings: Settings,
    #[serde(default)]
    protected_access_key: String,
    #[serde(default)]
    protected_webhook_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    protected_openai_api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protected_translation_credential: Option<SavedTranslationCredential>,
    #[serde(default)]
    protected_channel_webhooks: Vec<SavedChannelWebhook>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedChannelWebhook {
    id: u64,
    protected_webhook_url: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedTranslationCredential {
    provider: ApiProvider,
    api_base_url: String,
    protected_api_key: String,
}

fn validate_channel_tabs(settings: &Settings) -> Result<(), String> {
    if settings.channel_tabs.len() > MAX_CHANNEL_TABS {
        return Err(crate::i18n::format(
            crate::i18n::Key::M272E7CAD215A,
            &[("MAX_CHANNEL_TABS", format!("{}", MAX_CHANNEL_TABS))],
        ));
    }
    if settings.channel_tabs.is_empty() {
        return if settings.active_channel_tab == 0 {
            Ok(())
        } else {
            Err(crate::i18n::text(crate::i18n::Key::M69176AB92FE3))
        };
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut channels = std::collections::BTreeSet::new();
    for tab in &settings.channel_tabs {
        if tab.id == 0 || !ids.insert(tab.id) {
            return Err(crate::i18n::text(crate::i18n::Key::MC401CD0299A5));
        }
        validate_channel_tab_name(&tab.name)?;
        if tab.channel_url.len() > MAX_CONNECTION_URL_BYTES
            || tab.webhook_url.len() > MAX_CONNECTION_URL_BYTES
        {
            return Err(crate::i18n::text(crate::i18n::Key::MB41FA1953D59));
        }
        // Keep dormant RPC drafts compatible with legacy relay mode. They are
        // checked before a later explicit switch to Discord direct connection.
        if settings.connection_mode == ConnectionMode::Rpc {
            if !tab.channel_url.trim().is_empty() {
                let channel = channel_id(&tab.channel_url)?;
                if !channels.insert(channel) {
                    return Err(crate::i18n::text(crate::i18n::Key::MA25E489C07E6));
                }
            }
            if !tab.webhook_url.trim().is_empty() {
                crate::discord_http::Webhook::parse(&tab.webhook_url)?;
            }
        }
    }
    if !ids.contains(&settings.active_channel_tab) {
        return Err(crate::i18n::text(crate::i18n::Key::M69176AB92FE3));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub secure: bool,
}

pub fn endpoint(input: &str) -> Result<Endpoint, String> {
    let url = input.trim().trim_end_matches('/');
    let (secure, authority) = if let Some(a) = url.strip_prefix("https://") {
        (true, a)
    } else if let Some(a) = url.strip_prefix("http://") {
        (false, a)
    } else {
        return Err(crate::i18n::text(crate::i18n::Key::MC81067F08243));
    };
    if authority.is_empty()
        || authority
            .chars()
            .any(|c| c.is_whitespace() || !c.is_ascii())
        || authority.contains(['/', '\\', '@', '?', '#', '%'])
    {
        return Err(crate::i18n::text(crate::i18n::Key::MB84E489A1B1C));
    }
    let (host, port_part) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or(crate::i18n::text(crate::i18n::Key::MF32015534E95))?;
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(crate::i18n::text(crate::i18n::Key::M60848A06C464));
        }
        let port = if tail.is_empty() {
            None
        } else {
            Some(
                tail.strip_prefix(':')
                    .ok_or(crate::i18n::text(crate::i18n::Key::MFFA1D30EC1F1))?,
            )
        };
        (host, port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        {
            return Err(crate::i18n::text(crate::i18n::Key::M9C9E028FD5A4));
        }
        (host, port)
    };
    let port = match port_part {
        Some(p) => p
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or(crate::i18n::text(crate::i18n::Key::M4DD3BABAEB56))?,
        None => {
            if secure {
                443
            } else {
                80
            }
        }
    };
    let local = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
    if !secure && !local {
        return Err(crate::i18n::text(crate::i18n::Key::MB0A1CF5E27E6));
    }
    Ok(Endpoint {
        host: host.to_ascii_lowercase(),
        port,
        secure,
    })
}

pub fn channel_id(input: &str) -> Result<String, String> {
    let value = input.trim();
    if snowflake(value) {
        return Ok(value.into());
    }
    let path = value
        .strip_prefix("https://")
        .ok_or(crate::i18n::text(crate::i18n::Key::M1A97CF71EC35))?;
    let (host, path) = path
        .split_once('/')
        .ok_or(crate::i18n::text(crate::i18n::Key::MFB676987A8D1))?;
    if ![
        "discord.com",
        "canary.discord.com",
        "ptb.discord.com",
        "discordapp.com",
    ]
    .contains(&host)
    {
        return Err(crate::i18n::text(crate::i18n::Key::M4A14D8EEDD7A));
    }
    let clean = path.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<_> = clean.trim_end_matches('/').split('/').collect();
    if !(parts.len() == 3 || parts.len() == 4)
        || parts[0] != "channels"
        || !snowflake(parts[1])
        || !snowflake(parts[2])
        || (parts.len() == 4 && !snowflake(parts[3]))
    {
        return Err(crate::i18n::text(crate::i18n::Key::M418A4C29DBFB));
    }
    Ok(parts[2].into())
}

pub(crate) fn snowflake(s: &str) -> bool {
    (16..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

/// Appearance validation is independent of incomplete connection/API drafts.
pub fn validate_appearance(s: &Settings) -> Result<(), String> {
    if s.background_opacity > 100
        || s.background_color > 0xFFFFFF
        || s.text_color > 0xFFFFFF
        || s.author_color > 0xFFFFFF
        || s.translation_text_color > 0xFFFFFF
        || s.outline_color > 0xFFFFFF
    {
        return Err(crate::i18n::text(crate::i18n::Key::M7EA765920BF6));
    }
    if !(8..=48).contains(&s.font_size)
        || !(8..=48).contains(&s.translation_font_size)
        || horizontal_font_family(&s.font_family).is_empty()
        || s.font_family.encode_utf16().count() > 31
        || s.font_family.contains('\0')
    {
        return Err(crate::i18n::text(crate::i18n::Key::M42FC44C27156));
    }
    if !(-2..=12).contains(&s.letter_spacing)
        || s.line_spacing > 24
        || s.message_spacing > 48
        || s.author_gap > 24
    {
        return Err(crate::i18n::text(crate::i18n::Key::MFBBE8C1BA1A5));
    }
    if !(1..=3).contains(&s.outline_width) {
        return Err(crate::i18n::text(crate::i18n::Key::MB5CBDDE78D0B));
    }
    if !(1..=3600).contains(&s.hide_after_secs) {
        return Err(crate::i18n::text(crate::i18n::Key::M08B74324FA88));
    }
    Ok(())
}

/// GDI names beginning with `@` select vertical CJK glyphs even when both
/// font angles are zero. Sideglow lays out horizontal text only. Keep the
/// chosen typeface, but use its horizontal counterpart at every entry point.
pub(crate) fn horizontal_font_family(family: &str) -> &str {
    family.trim().trim_start_matches('@').trim_start()
}

/// Locale filenames use a bounded identifier, never a caller-supplied path.
/// Custom packs may be added without rebuilding the application.
pub fn valid_ui_language(value: &str) -> bool {
    (2..=32).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        && !["CON", "PRN", "AUX", "NUL"]
            .iter()
            .any(|reserved| value.eq_ignore_ascii_case(reserved))
        && !(value.len() == 4
            && (value[..3].eq_ignore_ascii_case("COM") || value[..3].eq_ignore_ascii_case("LPT"))
            && matches!(value.as_bytes()[3], b'1'..=b'9'))
}

pub fn validate(s: &Settings) -> Result<(), String> {
    if !valid_ui_language(&s.ui_language) {
        return Err(crate::i18n::text(crate::i18n::Key::M1BF227715412));
    }
    validate_translation(&s.translation)?;
    validate_channel_tabs(s)?;
    if s.connection_mode == ConnectionMode::Relay && !s.relay_url.trim().is_empty() {
        endpoint(&s.relay_url)?;
    }
    if !s.channel_url.trim().is_empty() {
        channel_id(&s.channel_url)?;
    }
    if s.connection_mode == ConnectionMode::Relay
        && !s.access_key.is_empty()
        && (!(32..=256).contains(&s.access_key.len())
            || !s
                .access_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    {
        return Err(crate::i18n::text(crate::i18n::Key::MB642450F4141));
    }
    if s.connection_mode == ConnectionMode::Rpc {
        if !s.discord_app_id.trim().is_empty() && !snowflake(s.discord_app_id.trim()) {
            return Err(crate::i18n::text(crate::i18n::Key::M20D6BF2011A3));
        }
        if !s.webhook_url.trim().is_empty() {
            crate::discord_http::Webhook::parse(&s.webhook_url)?;
        }
    }
    validate_appearance(s)?;
    if s.hotkey_modifiers & !15 != 0
        || !((0x30..=0x39).contains(&s.hotkey_vk)
            || (0x41..=0x5A).contains(&s.hotkey_vk)
            || (0x70..=0x87).contains(&s.hotkey_vk)
            || [0x09, 0x0D, 0x20, 0x2D, 0x2E, 0x24, 0x23, 0x21, 0x22, 0xC0].contains(&s.hotkey_vk))
    {
        return Err(crate::i18n::text(crate::i18n::Key::M94725713A765));
    }
    if (s.hotkey_modifiers & 1 != 0 && [0x09, 0x73].contains(&s.hotkey_vk))
        || (s.hotkey_modifiers & 8 != 0 && s.hotkey_vk == 0x4C)
        || (s.hotkey_modifiers & 3 == 3 && s.hotkey_vk == 0x2E)
    {
        return Err(crate::i18n::text(crate::i18n::Key::M98CFBC5FA42A));
    }
    if !(280..=3840).contains(&s.width) || !(160..=2160).contains(&s.height) {
        return Err(crate::i18n::text(crate::i18n::Key::MB60D1F1FC6BB));
    }
    if s.target_exe.contains('\0') || s.target_exe.len() > 32768 {
        return Err(crate::i18n::text(crate::i18n::Key::ME0DB053FBAD4));
    }
    Ok(())
}

pub fn path() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or(crate::i18n::text(crate::i18n::Key::M579A58CE6321))?;
    // Settings and the RPC session use the same Sideglow directory.
    // Prior storage locations are not searched or migrated automatically.
    Ok(PathBuf::from(base).join("Sideglow").join("settings.json"))
}

pub fn load() -> Result<Settings, String> {
    let path = path()?;
    if !path.exists() {
        let mut settings = Settings::default();
        settings.ensure_channel_tabs()?;
        return Ok(settings);
    }
    let metadata = fs::metadata(&path).map_err(|e| {
        crate::i18n::format(crate::i18n::Key::MD7AFE7DB6262, &[("e", format!("{}", e))])
    })?;
    if metadata.len() > 128 * 1024 {
        return Err(crate::i18n::text(crate::i18n::Key::MF728B6765A26));
    }
    let bytes = fs::read(&path).map_err(|e| {
        crate::i18n::format(crate::i18n::Key::MD7AFE7DB6262, &[("e", format!("{}", e))])
    })?;
    let mut saved = decode_settings(&bytes)?;
    if saved.version != 1 {
        return Err(crate::i18n::text(crate::i18n::Key::M3948AF5B51F0));
    }
    if !saved.protected_access_key.is_empty() {
        saved.settings.access_key = unprotect(&decode_hex(&saved.protected_access_key)?)?;
    }
    if !saved.protected_webhook_url.is_empty() {
        saved.settings.webhook_url = unprotect(&decode_hex(&saved.protected_webhook_url)?)?;
    }
    restore_channel_webhooks(&mut saved, |encoded| unprotect(&decode_hex(encoded)?))?;
    saved.settings.sync_active_tab()?;
    validate(&saved.settings)?;
    Ok(saved.settings)
}

fn decode_settings(bytes: &[u8]) -> Result<SavedSettings, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M52F0BF5E75C2))?;
    let legacy = value
        .get("settings")
        .and_then(|v| v.get("connection_mode"))
        .is_none();
    let explicit_tabs = value
        .get("settings")
        .and_then(|settings| settings.get("channel_tabs"));
    if explicit_tabs
        .and_then(serde_json::Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        return Err(crate::i18n::text(crate::i18n::Key::M679610B25F3E));
    }
    let mut saved: SavedSettings = serde_json::from_value(value)
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M52F0BF5E75C2))?;
    saved.settings.font_family = horizontal_font_family(&saved.settings.font_family).to_owned();
    if legacy && (!saved.settings.relay_url.is_empty() || !saved.protected_access_key.is_empty()) {
        saved.settings.connection_mode = ConnectionMode::Relay;
    }
    // Keep the protected blob opaque here. Even an enabled setting decrypts
    // only when the runtime is deliberately started, never during file loading.
    if let Some(credential) = &saved.protected_translation_credential {
        saved
            .settings
            .translation
            .protected_api_key
            .clone_from(&credential.protected_api_key);
        saved.settings.translation.credential_identity = Some(TranslationCredentialIdentity {
            provider: credential.provider,
            api_base_url: credential.api_base_url.clone(),
        });
    } else if !saved.protected_openai_api_key.is_empty() {
        saved
            .settings
            .translation
            .protected_api_key
            .clone_from(&saved.protected_openai_api_key);
        saved.settings.translation.credential_identity = Some(legacy_openai_identity());
    }
    saved.settings.ensure_channel_tabs()?;
    validate_protected_channel_webhooks(&saved)?;
    Ok(saved)
}

fn validate_protected_channel_webhooks(saved: &SavedSettings) -> Result<(), String> {
    if saved.protected_channel_webhooks.len() > MAX_CHANNEL_TABS {
        return Err(crate::i18n::text(crate::i18n::Key::ME6B47166ED4B));
    }
    let mut ids = std::collections::BTreeSet::new();
    for record in &saved.protected_channel_webhooks {
        if !ids.insert(record.id)
            || !saved
                .settings
                .channel_tabs
                .iter()
                .any(|tab| tab.id == record.id)
            || record.protected_webhook_url.len() > 16 * 1024
        {
            return Err(crate::i18n::text(crate::i18n::Key::M0F408ACB551A));
        }
    }
    Ok(())
}

/// The injectable decoder lets identity/association tests run without invoking
/// Windows DPAPI. Only webhook blobs are opened; the AI key remains opaque.
fn restore_channel_webhooks(
    saved: &mut SavedSettings,
    mut decode: impl FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    validate_protected_channel_webhooks(saved)?;
    for record in &saved.protected_channel_webhooks {
        let webhook = if record.protected_webhook_url.is_empty() {
            String::new()
        } else {
            decode(&record.protected_webhook_url)?
        };
        let tab = saved
            .settings
            .channel_tabs
            .iter_mut()
            .find(|tab| tab.id == record.id)
            .ok_or(crate::i18n::text(crate::i18n::Key::M5941B6FBC220))?;
        tab.webhook_url = webhook;
    }
    // Legacy files had only the active encrypted URL. Attach it to the newly
    // migrated first tab before sync, so migration does not look like retargeting
    // and unexpectedly revoke that channel's existing translation preference.
    if !saved
        .protected_channel_webhooks
        .iter()
        .any(|record| record.id == saved.settings.active_channel_tab)
    {
        let active_webhook = saved.settings.webhook_url.clone();
        if let Some(tab) = saved
            .settings
            .channel_tabs
            .iter_mut()
            .find(|tab| tab.id == saved.settings.active_channel_tab)
        {
            tab.webhook_url = active_webhook;
        }
    }
    // New files keep the active legacy field as a downgrade-compatible copy.
    // A file with only per-tab encrypted fields can still restore its selection.
    if saved.protected_webhook_url.is_empty() && saved.settings.webhook_url.is_empty() {
        if let Some(tab) = saved.settings.active_tab() {
            saved.settings.webhook_url = tab.webhook_url.clone();
        }
    }
    Ok(())
}

fn encode_settings(
    settings: &Settings,
    mut encode: impl FnMut(&str) -> Result<String, String>,
) -> Result<Vec<u8>, String> {
    let mut settings = settings.clone();
    settings.font_family = horizontal_font_family(&settings.font_family).to_owned();
    settings.sync_active_tab()?;
    validate(&settings)?;
    let protected_access_key = if settings.access_key.is_empty() {
        String::new()
    } else {
        encode(&settings.access_key)?
    };
    let mut protected_channel_webhooks = Vec::with_capacity(settings.channel_tabs.len());
    for tab in &settings.channel_tabs {
        protected_channel_webhooks.push(SavedChannelWebhook {
            id: tab.id,
            protected_webhook_url: if tab.webhook_url.is_empty() {
                String::new()
            } else {
                encode(&tab.webhook_url)?
            },
        });
    }
    let protected_webhook_url = if settings.connection_mode == ConnectionMode::Relay {
        if settings.webhook_url.is_empty() {
            String::new()
        } else {
            encode(&settings.webhook_url)?
        }
    } else {
        protected_channel_webhooks
            .iter()
            .find(|record| record.id == settings.active_channel_tab)
            .map(|record| record.protected_webhook_url.clone())
            .ok_or(crate::i18n::text(crate::i18n::Key::M0992EE443A99))?
    };
    let protected_translation_credential = if settings.translation.protected_api_key.is_empty() {
        None
    } else {
        let identity = settings
            .translation
            .credential_identity
            .clone()
            .unwrap_or_else(legacy_openai_identity);
        Some(SavedTranslationCredential {
            provider: identity.provider,
            api_base_url: identity.api_base_url,
            protected_api_key: settings.translation.protected_api_key.clone(),
        })
    };
    let bytes = serde_json::to_vec_pretty(&SavedSettings {
        version: 1,
        settings,
        protected_access_key,
        protected_webhook_url,
        protected_openai_api_key: String::new(),
        protected_translation_credential,
        protected_channel_webhooks,
    })
    .map_err(|_| crate::i18n::text(crate::i18n::Key::MC032F698D740))?;
    if bytes.len() > 128 * 1024 {
        return Err(crate::i18n::text(crate::i18n::Key::M1831B90192A8));
    }
    Ok(bytes)
}

pub fn save(settings: &Settings) -> Result<(), String> {
    let bytes = encode_settings(settings, |value| Ok(encode_hex(&protect(value)?)))?;
    let path = path()?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| {
        crate::i18n::format(crate::i18n::Key::MB736A133029F, &[("e", format!("{}", e))])
    })?;
    let tmp = path.with_extension("json.tmp");
    use std::io::Write;
    let mut file = fs::File::create(&tmp).map_err(|e| {
        crate::i18n::format(crate::i18n::Key::MAA01324E635B, &[("e", format!("{}", e))])
    })?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| {
            crate::i18n::format(crate::i18n::Key::M1A65F975F15B, &[("e", format!("{}", e))])
        })?;
    drop(file);
    replace_file(&tmp, &path)
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(crate) fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return Err(crate::i18n::text(crate::i18n::Key::MD2A237168173));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| crate::i18n::text(crate::i18n::Key::MD2A237168173))
        })
        .collect()
}

#[cfg(windows)]
pub(crate) fn replace_file(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            a.as_ptr(),
            b.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err({
            let __ui_arg0 = &(std::io::Error::last_os_error());
            crate::i18n::format(
                crate::i18n::Key::MD3B3F2651B9D,
                &[("arg0", format!("{}", __ui_arg0))],
            )
        });
    }
    Ok(())
}
#[cfg(not(windows))]
pub(crate) fn replace_file(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|e| e.to_string())
}

#[cfg(windows)]
pub(crate) fn protect(text: &str) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB},
    };
    let data = CRYPT_INTEGER_BLOB {
        cbData: text.len() as u32,
        pbData: text.as_ptr() as *mut u8,
    };
    let mut out: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CryptProtectData(
            &data,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(crate::i18n::text(crate::i18n::Key::MBE475C503733));
    }
    let result = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as *mut _);
    }
    Ok(result)
}

#[cfg(windows)]
pub(crate) fn unprotect(bytes: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let data = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut out: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CryptUnprotectData(
            &data,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(crate::i18n::text(crate::i18n::Key::M39237ABBC4C3));
    }
    let mut raw = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        LocalFree(out.pbData as *mut _);
    }
    let result = String::from_utf8(raw.clone())
        .map_err(|_| crate::i18n::text(crate::i18n::Key::M5B66BAEA0DA4));
    raw.fill(0);
    result
}

#[cfg(not(windows))]
pub(crate) fn protect(_: &str) -> Result<Vec<u8>, String> {
    Err(crate::i18n::text(crate::i18n::Key::MA396EC0E4069))
}
#[cfg(not(windows))]
pub(crate) fn unprotect(_: &[u8]) -> Result<String, String> {
    Err(crate::i18n::text(crate::i18n::Key::ME9D2B8849EAA))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_channel(number: u64) -> String {
        format!(
            "https://discord.com/channels/100000000000000001/{}",
            200000000000000000 + number
        )
    }

    fn tab_webhook(number: u64) -> String {
        format!(
            "https://discord.com/api/webhooks/{}/fictional_secret_ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            300000000000000000 + number
        )
    }

    fn tab_settings() -> Settings {
        let mut settings = Settings {
            discord_app_id: "100000000000000009".into(),
            channel_url: tab_channel(1),
            webhook_url: tab_webhook(1),
            target_exe: "ExampleGame.exe".into(),
            font_size: 18,
            ..Settings::default()
        };
        settings.translation.protected_api_key = "opaque-existing-key".into();
        settings.translation.enabled = true;
        settings.ensure_channel_tabs().unwrap();
        settings
    }

    #[test]
    fn legacy_connection_migrates_to_one_tab_without_changing_its_execution_choice() {
        let input = serde_json::json!({
            "version": 1,
            "settings": {
                "connection_mode": "rpc",
                "channel_url": tab_channel(1),
                "discord_app_id": "100000000000000009",
                "font_size": 18,
                "translation": {"enabled": true}
            },
            "protected_webhook_url": "opaque-webhook-blob",
            "protected_openai_api_key": "not-opened-by-loading"
        });
        let mut saved = decode_settings(&serde_json::to_vec(&input).unwrap()).unwrap();
        assert_eq!(saved.settings.channel_tabs.len(), 1);
        assert_eq!(saved.settings.active_channel_tab, 1);
        assert!(saved.settings.translation.enabled);
        assert!(saved.settings.active_tab().unwrap().translation_enabled);
        // Simulate the existing active credential decoder. Tab migration itself
        // must not mistake this restoration for a destination edit.
        saved.settings.webhook_url = tab_webhook(1);
        restore_channel_webhooks(&mut saved, |_| panic!("no per-tab blobs in legacy file"))
            .unwrap();
        saved.settings.sync_active_tab().unwrap();
        assert!(saved.settings.translation.enabled);
        assert_eq!(
            saved.settings.active_tab().unwrap().webhook_url,
            tab_webhook(1)
        );
        assert_eq!(
            saved.settings.translation.protected_api_key,
            "not-opened-by-loading"
        );
        assert_eq!(saved.settings.font_size, 18);
    }

    #[test]
    fn tab_switching_projects_only_destination_and_remembers_each_translation_choice() {
        let mut settings = tab_settings();
        let second = settings
            .add_channel_tab(" 두 번째 ", &tab_channel(2), &tab_webhook(2))
            .unwrap();
        assert_eq!(settings.active_channel_tab, 1);
        assert_eq!(settings.channel_url, tab_channel(1));
        assert!(settings.translation.enabled);
        assert!(!settings.channel_tabs[1].translation_enabled);
        assert_eq!(settings.channel_tabs[1].name, "두 번째");
        settings.select_channel_tab(second).unwrap();
        assert_eq!(settings.channel_url, tab_channel(2));
        assert_eq!(settings.webhook_url, tab_webhook(2));
        assert!(!settings.translation.enabled);
        settings.translation.enabled = true;
        settings.select_channel_tab(1).unwrap();
        assert_eq!(settings.channel_url, tab_channel(1));
        assert!(settings.translation.enabled);
        assert!(settings.channel_tabs[1].translation_enabled);
        settings.select_channel_tab(second).unwrap();
        assert!(settings.translation.enabled);
        assert_eq!(settings.discord_app_id, "100000000000000009");
        assert_eq!(
            settings.translation.protected_api_key,
            "opaque-existing-key"
        );
        assert_eq!(settings.target_exe, "ExampleGame.exe");
        assert_eq!(settings.font_size, 18);
        // Removing the shared API key never silently restores a paid ON state.
        settings.translation.enabled = false;
        settings.translation.protected_api_key.clear();
        settings.select_channel_tab(1).unwrap();
        assert!(!settings.translation.enabled);
        assert!(!settings.active_tab().unwrap().translation_enabled);
    }

    #[test]
    fn retargeting_disables_translation_and_deletion_uses_stable_id() {
        let mut settings = tab_settings();
        settings
            .edit_channel_tab(1, "새 이름", &tab_channel(1), &tab_webhook(1))
            .unwrap();
        assert!(settings.translation.enabled);
        settings
            .edit_channel_tab(1, "새 이름", &tab_channel(1), &tab_webhook(2))
            .unwrap();
        assert!(!settings.translation.enabled);
        assert_eq!(settings.webhook_url, tab_webhook(2));
        settings.translation.enabled = true;
        settings.channel_url = tab_channel(3); // old setup/import projection path
        settings.sync_active_tab().unwrap();
        assert!(!settings.translation.enabled);
        let second = settings.add_channel_tab("둘", &tab_channel(2), "").unwrap();
        let third = settings
            .add_channel_tab("셋", &tab_channel(4), &tab_webhook(4))
            .unwrap();
        settings.select_channel_tab(third).unwrap();
        settings.remove_channel_tab(second).unwrap();
        assert_eq!(settings.active_channel_tab, third);
        settings.remove_channel_tab(third).unwrap();
        assert_eq!(settings.active_channel_tab, 1);
        assert_eq!(settings.channel_url, tab_channel(3));
        assert_eq!(settings.webhook_url, tab_webhook(2));
        assert!(settings.remove_channel_tab(1).is_err());
        assert!(settings.select_channel_tab(999).is_err());
        assert_eq!(settings.channel_tabs.len(), 1);
    }

    #[test]
    fn tabs_bound_names_destinations_count_and_duplicate_channels() {
        let mut settings = tab_settings();
        for name in ["", "   ", "x\ny", "\tname", "x\0y", &"가".repeat(33)] {
            assert!(settings.add_channel_tab(name, &tab_channel(2), "").is_err());
        }
        assert!(settings
            .add_channel_tab("중복", &tab_channel(1), "")
            .is_err());
        for channel in [
            "200000000000000002",
            "https://discord.com/channels/@me/200000000000000002",
            "https://example.com/channels/100000000000000001/200000000000000002",
        ] {
            assert!(settings.add_channel_tab("둘", channel, "").is_err());
        }
        assert!(settings
            .add_channel_tab("둘", &tab_channel(2), "https://example.com/webhook")
            .is_err());
        let full_message = format!("{}/400000000000000001?x=1#anchor", tab_channel(2));
        assert_eq!(
            canonical_channel_url(&full_message).unwrap(),
            tab_channel(2)
        );
        for number in 2..=MAX_CHANNEL_TABS as u64 {
            settings
                .add_channel_tab(&format!("채널 {number}"), &tab_channel(number), "")
                .unwrap();
        }
        assert_eq!(settings.channel_tabs.len(), MAX_CHANNEL_TABS);
        assert!(settings
            .add_channel_tab("초과", &tab_channel(99), "")
            .is_err());
        let original = settings.channel_url.clone();
        assert!(settings
            .edit_channel_tab(1, "중복", &tab_channel(2), "")
            .is_err());
        assert_eq!(settings.channel_url, original);
        assert!(settings.translation.enabled);
    }

    #[test]
    fn all_tab_webhooks_are_secret_and_restored_by_id_not_record_order() {
        let mut settings = tab_settings();
        let second = settings
            .add_channel_tab("둘", &tab_channel(2), &tab_webhook(2))
            .unwrap();
        settings.select_channel_tab(second).unwrap();
        assert!(!serde_json::to_string(&settings)
            .unwrap()
            .contains("fictional_secret"));
        assert!(!format!("{settings:?}").contains("fictional_secret"));
        assert!(!format!("{:?}", settings.channel_tabs).contains("fictional_secret"));
        let mut vault = std::collections::BTreeMap::new();
        let bytes = encode_settings(&settings, |value| {
            let blob = format!("opaque-{}", vault.len());
            vault.insert(blob.clone(), value.to_owned());
            Ok(blob)
        })
        .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("fictional_secret"));
        let mut saved = decode_settings(&bytes).unwrap();
        saved.protected_channel_webhooks.reverse();
        saved.settings.webhook_url = vault[&saved.protected_webhook_url].clone();
        restore_channel_webhooks(&mut saved, |blob| Ok(vault[blob].clone())).unwrap();
        saved.settings.sync_active_tab().unwrap();
        assert_eq!(saved.settings.active_channel_tab, second);
        assert_eq!(saved.settings.channel_tabs[0].webhook_url, tab_webhook(1));
        assert_eq!(saved.settings.channel_tabs[1].webhook_url, tab_webhook(2));
        assert_eq!(saved.settings.webhook_url, tab_webhook(2));
        assert!(!saved.settings.translation.enabled);
        assert!(saved.settings.channel_tabs[0].translation_enabled);
        assert_eq!(
            saved.settings.translation.protected_api_key,
            "opaque-existing-key"
        );
    }

    #[test]
    fn malformed_saved_tab_ids_empty_lists_and_secret_records_are_rejected() {
        let settings = tab_settings();
        let base: serde_json::Value =
            serde_json::from_slice(&encode_settings(&settings, |_| Ok("opaque".into())).unwrap())
                .unwrap();
        for (field, value) in [
            ("channel_tabs", serde_json::json!([])),
            ("active_channel_tab", serde_json::json!(0)),
            ("active_channel_tab", serde_json::json!(999)),
        ] {
            let mut invalid = base.clone();
            invalid["settings"][field] = value;
            assert!(decode_settings(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let mut invalid = base.clone();
        let first = invalid["settings"]["channel_tabs"][0].clone();
        invalid["settings"]["channel_tabs"]
            .as_array_mut()
            .unwrap()
            .push(first);
        assert!(decode_settings(&serde_json::to_vec(&invalid).unwrap()).is_err());
        for records in [
            serde_json::json!([{"id":999,"protected_webhook_url":"opaque"}]),
            serde_json::json!([{"id":1,"protected_webhook_url":"a"},{"id":1,"protected_webhook_url":"b"}]),
            serde_json::json!([{"id":1,"protected_webhook_url":"x".repeat(16*1024+1)}]),
        ] {
            let mut invalid = base.clone();
            invalid["protected_channel_webhooks"] = records;
            assert!(decode_settings(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }

    #[test]
    fn relay_preferences_do_not_overwrite_saved_rpc_tabs() {
        let mut settings = tab_settings();
        settings.add_channel_tab("둘", &tab_channel(2), "").unwrap();
        settings.connection_mode = ConnectionMode::Relay;
        settings.channel_url = tab_channel(9);
        settings.webhook_url = "dormant RPC draft".into();
        settings.translation.enabled = false;
        settings.sync_active_tab().unwrap();
        assert_eq!(settings.channel_tabs[0].channel_url, tab_channel(1));
        assert_eq!(settings.channel_tabs[0].webhook_url, tab_webhook(1));
        assert!(settings.select_channel_tab(2).is_err());
        assert!(settings.remove_channel_tab(2).is_err());
        let mut vault = std::collections::BTreeMap::new();
        let bytes = encode_settings(&settings, |value| {
            let blob = format!("opaque-{}", vault.len());
            vault.insert(blob.clone(), value.to_owned());
            Ok(blob)
        })
        .unwrap();
        let mut saved = decode_settings(&bytes).unwrap();
        saved.settings.webhook_url = vault[&saved.protected_webhook_url].clone();
        restore_channel_webhooks(&mut saved, |blob| Ok(vault[blob].clone())).unwrap();
        saved.settings.sync_active_tab().unwrap();
        assert_eq!(saved.settings.channel_url, tab_channel(9));
        assert_eq!(saved.settings.webhook_url, "dormant RPC draft");
        assert_eq!(saved.settings.channel_tabs[0].channel_url, tab_channel(1));
        assert_eq!(saved.settings.channel_tabs[0].webhook_url, tab_webhook(1));
    }

    #[test]
    fn channel_link_is_not_a_webhook() {
        assert_eq!(
            channel_id("https://discord.com/channels/123456789012345678/234567890123456789")
                .unwrap(),
            "234567890123456789"
        );
        assert!(channel_id("https://discord.com/api/webhooks/123/SECRET").is_err());
        assert!(channel_id(
            "https://discord.com.evil.test/channels/123456789012345678/234567890123456789"
        )
        .is_err());
        assert!(channel_id("https://discord.com/channels/@me/234567890123456789").is_err());
    }
    #[test]
    fn plaintext_transport_is_local_only() {
        assert!(endpoint("http://127.0.0.1:8787").is_ok());
        assert!(endpoint("http://[::1]:8787/").is_ok());
        assert!(endpoint("http://192.168.1.5:8787").is_err());
        assert!(endpoint("https://user:password@chat.example.com").is_err());
        assert!(endpoint("https://chat.example.com\r\nX-Evil:yes").is_err());
        assert!(endpoint("https://chat.example.com:443").is_ok());
        assert!(endpoint("https://chat.example.com:0").is_err());
    }
    #[test]
    fn serializing_and_debugging_never_emit_plain_key() {
        let s = Settings {
            access_key: "sensitive_key_01234567890123456789".into(),
            webhook_url: "https://discord.com/api/webhooks/123456789012345678/sensitive_key_01234567890123456789".into(),
            ..Default::default()
        };
        assert!(!serde_json::to_string(&s).unwrap().contains("sensitive_key"));
        assert!(!format!("{s:?}").contains("sensitive_key"));
        assert!(validate(&s).is_ok());
    }
    #[test]
    fn hostile_settings_rejected() {
        let mut s = Settings {
            connection_mode: ConnectionMode::Relay,
            access_key: "key\r\nInjected: 1".into(),
            ..Default::default()
        };
        assert!(validate(&s).is_err());
        s = Settings::default();
        s.hide_after_secs = 0;
        assert!(validate(&s).is_err());
        s = Settings::default();
        s.hotkey_vk = 0;
        assert!(validate(&s).is_err());
        s = Settings::default();
        s.hotkey_vk = 0x09;
        s.hotkey_modifiers = 1;
        assert!(validate(&s).is_err());
        s = Settings::default();
        s.hotkey_vk = 0x73;
        s.hotkey_modifiers = 1;
        assert!(validate(&s).is_err());
    }
    #[test]
    fn protected_blob_hex_round_trip() {
        assert_eq!(
            decode_hex(&encode_hex(&[0, 255, 16, 128])).unwrap(),
            vec![0, 255, 16, 128]
        );
        assert!(decode_hex("xx").is_err());
        assert!(decode_hex("ä").is_err());
    }

    #[test]
    fn existing_settings_preserve_preferences_and_disabled_state_round_trips() {
        // A 0.1.1 settings file has no overlay_enabled field. Upgrading must
        // retain its game/font/connection settings instead of resetting them.
        let old = serde_json::json!({
            "version": 1,
            "settings": {
                "target_exe": r"C:\Games\Example\game.exe",
                "font_family": "맑은 고딕",
                "font_size": 18,
                "background_opacity": 42,
                "hide_when_idle": false,
                "relay_url": "https://chat.example.com"
            },
            "protected_access_key": "0123abcd"
        });
        let mut saved = decode_settings(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(saved.settings.connection_mode, ConnectionMode::Relay);
        assert!(saved.settings.overlay_enabled);
        assert_eq!(saved.settings.target_exe, r"C:\Games\Example\game.exe");
        assert_eq!(saved.settings.font_size, 18);
        assert_eq!(saved.settings.background_opacity, 42);
        assert!(!saved.settings.hide_when_idle);
        assert_eq!(saved.protected_access_key, "0123abcd");

        saved.settings.overlay_enabled = false;
        let restored: SavedSettings =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert!(!restored.settings.overlay_enabled);
        assert_eq!(restored.settings.target_exe, saved.settings.target_exe);
        assert_eq!(restored.settings.relay_url, "https://chat.example.com");
        assert_eq!(restored.protected_access_key, saved.protected_access_key);
    }

    #[test]
    fn horizontal_chat_fonts_preserve_typefaces_and_reject_empty_vertical_names() {
        for (input, expected) in [
            ("@メイリオ", "メイリオ"),
            ("@Yu Gothic", "Yu Gothic"),
            (" @맑은 고딕 ", "맑은 고딕"),
            ("Meiryo", "Meiryo"),
            ("Yu Gothic UI", "Yu Gothic UI"),
            ("Font @ Symbol", "Font @ Symbol"),
        ] {
            assert_eq!(horizontal_font_family(input), expected);
        }
        for invalid in ["", " ", "@", "@@", " @ "] {
            let settings = Settings {
                font_family: invalid.to_owned(),
                ..Settings::default()
            };
            assert!(validate_appearance(&settings).is_err());
        }
    }

    #[test]
    fn saved_vertical_font_is_migrated_without_resetting_preferences() {
        let old = serde_json::json!({
            "version": 1,
            "settings": {
                "font_family": "@メイリオ",
                "font_size": 18,
                "ui_language": "ja",
                "background_opacity": 42,
                "target_exe": r"C:\Games\Example\game.exe"
            },
            "protected_openai_api_key": "0123abcd"
        });
        let mut saved = decode_settings(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(saved.settings.font_family, "メイリオ");
        assert_eq!(saved.settings.font_size, 18);
        assert_eq!(saved.settings.ui_language, "ja");
        assert_eq!(saved.settings.background_opacity, 42);
        assert_eq!(saved.settings.target_exe, r"C:\Games\Example\game.exe");
        assert_eq!(saved.settings.translation.protected_api_key, "0123abcd");
        // Saving also corrects a programmatically supplied vertical face while
        // leaving the caller's settings and protected API blob untouched.
        saved.settings.font_family = "@Yu Gothic".into();
        let encoded = encode_settings(&saved.settings, |_| {
            panic!("Font-only migration must not open or encrypt credentials")
        })
        .unwrap();
        let restored = decode_settings(&encoded).unwrap();
        assert_eq!(restored.settings.font_family, "Yu Gothic");
        assert_eq!(saved.settings.font_family, "@Yu Gothic");
        assert_eq!(restored.settings.translation.protected_api_key, "0123abcd");
    }

    #[test]
    fn readability_settings_upgrade_with_defaults_and_preserve_customization() {
        let old = serde_json::json!({
            "version": 1,
            "settings": {"font_size": 18, "background_opacity": 42}
        });
        let mut saved = decode_settings(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(saved.settings.letter_spacing, 0);
        assert_eq!(saved.settings.line_spacing, 4);
        assert_eq!(saved.settings.message_spacing, 12);
        assert_eq!(saved.settings.author_gap, 0);
        assert_eq!(saved.settings.author_color, 0x8fe5d7);
        assert!(!saved.settings.translation_style_enabled);
        assert_eq!(saved.settings.translation_font_size, 14);
        assert_eq!(saved.settings.translation_text_color, 0xFFE29A);
        assert!(!saved.settings.show_diagnostics);
        assert!(saved.settings.text_outline);
        assert_eq!(saved.settings.outline_color, 0);
        assert_eq!(saved.settings.outline_width, 1);
        assert_eq!(saved.settings.font_size, 18);
        assert_eq!(saved.settings.background_opacity, 42);

        saved.settings.letter_spacing = -2;
        saved.settings.line_spacing = 24;
        saved.settings.message_spacing = 48;
        saved.settings.author_gap = 24;
        saved.settings.author_color = 0x112233;
        saved.settings.translation_style_enabled = true;
        saved.settings.translation_font_size = 20;
        saved.settings.translation_text_color = 0xFEDCBA;
        saved.settings.show_diagnostics = true;
        saved.settings.text_outline = false;
        saved.settings.outline_color = 0x123456;
        saved.settings.outline_width = 3;
        assert!(validate(&saved.settings).is_ok());
        let restored = decode_settings(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(restored.settings.letter_spacing, -2);
        assert_eq!(restored.settings.line_spacing, 24);
        assert_eq!(restored.settings.message_spacing, 48);
        assert_eq!(restored.settings.author_gap, 24);
        assert_eq!(restored.settings.author_color, 0x112233);
        assert!(restored.settings.translation_style_enabled);
        assert_eq!(restored.settings.translation_font_size, 20);
        assert_eq!(restored.settings.translation_text_color, 0xFEDCBA);
        assert!(restored.settings.show_diagnostics);
        assert!(!restored.settings.text_outline);
        assert_eq!(restored.settings.outline_color, 0x123456);
        assert_eq!(restored.settings.outline_width, 3);
    }

    #[test]
    fn readability_settings_reject_out_of_range_spacing_and_outline() {
        for invalid in [
            serde_json::json!({"letter_spacing": -3}),
            serde_json::json!({"letter_spacing": 13}),
            serde_json::json!({"line_spacing": 25}),
            serde_json::json!({"message_spacing": 49}),
            serde_json::json!({"author_gap": 25}),
            serde_json::json!({"author_color": 0x1000000}),
            serde_json::json!({"translation_font_size": 7}),
            serde_json::json!({"translation_font_size": 49}),
            serde_json::json!({"translation_text_color": 0x1000000}),
            serde_json::json!({"outline_color": 0x1000000}),
            serde_json::json!({"outline_width": 0}),
            serde_json::json!({"outline_width": 4}),
        ] {
            let settings: Settings = serde_json::from_value(invalid).unwrap();
            assert!(validate(&settings).is_err());
        }
        let boundaries = Settings {
            letter_spacing: 12,
            line_spacing: 0,
            message_spacing: 0,
            outline_color: 0xFFFFFF,
            outline_width: 1,
            ..Default::default()
        };
        assert!(validate(&boundaries).is_ok());
    }

    #[test]
    fn ui_language_defaults_and_round_trips_independently_of_chat_languages() {
        let legacy = decode_settings(
            br#"{"version":1,"settings":{"translation":{"language_a":"ja","language_b":"en"}}}"#,
        )
        .unwrap();
        assert_eq!(legacy.settings.ui_language, "ko");
        assert_eq!(legacy.settings.translation.language_a, "ja");
        assert_eq!(legacy.settings.translation.language_b, "en");

        let mut settings = legacy.settings;
        settings.ui_language = "fr-CA".into();
        let bytes = encode_settings(&settings, |_| panic!("no credentials needed")).unwrap();
        let restored = decode_settings(&bytes).unwrap().settings;
        assert_eq!(restored.ui_language, "fr-CA");
        assert_eq!(restored.translation.language_a, "ja");
        assert_eq!(restored.translation.language_b, "en");
        assert!(!restored.translation.enabled);
    }

    #[test]
    fn romaji_input_defaults_off_and_round_trips_without_translation_credentials() {
        let legacy = decode_settings(
            br#"{"version":1,"settings":{"ui_language":"en","translation":{"language_a":"ko","language_b":"ja"}}}"#,
        )
        .unwrap();
        assert!(!Settings::default().romaji_japanese_input);
        assert!(!legacy.settings.romaji_japanese_input);
        for enabled in [true, false] {
            let mut settings = legacy.settings.clone();
            settings.romaji_japanese_input = enabled;
            let bytes = encode_settings(&settings, |_| panic!("no credentials needed")).unwrap();
            let restored = decode_settings(&bytes).unwrap().settings;
            assert_eq!(restored.romaji_japanese_input, enabled);
            assert_eq!(restored.ui_language, "en");
            assert_eq!(restored.translation.language_a, "ko");
            assert_eq!(restored.translation.language_b, "ja");
            assert!(!restored.translation.enabled);
            assert!(restored.translation.protected_api_key.is_empty());
        }
        assert!(
            decode_settings(br#"{"version":1,"settings":{"romaji_japanese_input":"true"}}"#,)
                .is_err()
        );
    }

    #[test]
    fn connection_codes_never_share_or_overwrite_the_personal_romaji_choice() {
        let mut shared = tab_settings();
        shared.romaji_japanese_input = false;
        let code = crate::connection_profile::export_code(&shared).unwrap();
        shared.romaji_japanese_input = true;
        assert_eq!(
            crate::connection_profile::export_code(&shared).unwrap(),
            code
        );
        let profile = crate::connection_profile::import_code(&code).unwrap();
        for enabled in [true, false] {
            let mut local = Settings {
                ui_language: "ja".into(),
                romaji_japanese_input: enabled,
                ..Settings::default()
            };
            local.translation.language_a = "en".into();
            local.translation.language_b = "ko".into();
            let imported = profile.apply_to(&local);
            assert_eq!(imported.romaji_japanese_input, enabled);
            assert_eq!(imported.ui_language, "ja");
            assert_eq!(imported.translation.language_a, "en");
            assert_eq!(imported.translation.language_b, "ko");
            assert_eq!(local.romaji_japanese_input, enabled);
        }
    }

    #[test]
    fn ui_language_ids_accept_custom_packs_without_accepting_paths_or_devices() {
        for id in ["ko", "en", "ja", "pt_BR", "zh-Hant", "custom2"] {
            assert!(valid_ui_language(id), "{id}");
            assert!(validate(&Settings {
                ui_language: id.into(),
                ..Settings::default()
            })
            .is_ok());
        }
        for id in [
            "",
            "k",
            "1en",
            "../en",
            "en.json",
            "en/US",
            "en\\US",
            "en\0",
            "日本語",
            "CON",
            "nul",
            "COM1",
            "lpt9",
            "with space",
            "A12345678901234567890123456789012",
        ] {
            assert!(!valid_ui_language(id), "{id}");
            assert!(validate(&Settings {
                ui_language: id.into(),
                ..Settings::default()
            })
            .is_err());
        }
        assert!(valid_ui_language("A1234567890123456789012345678901"));
    }

    #[test]
    fn appearance_preview_isolated_from_connection_api_input_and_geometry() {
        let mut current = Settings::default();
        let mut draft = Settings {
            ui_language: "ja".into(),
            romaji_japanese_input: true,
            author_color: 0x123456,
            author_gap: 4,
            translation_style_enabled: true,
            translation_font_size: 24,
            translation_text_color: 0xFEDCBA,
            show_diagnostics: true,
            font_family: "Example Font".into(),
            font_size: 20,
            background_opacity: 0,
            channel_url: "incomplete connection draft".into(),
            webhook_url: "never preview".into(),
            overlay_enabled: false,
            hotkey_vk: 65,
            x: 999,
            ..Settings::default()
        };
        draft.translation.enabled = true;
        draft.channel_tabs.push(ChannelTab {
            id: 999,
            name: "not previewed".into(),
            channel_url: "not previewed".into(),
            webhook_url: "not previewed".into(),
            translation_enabled: true,
        });
        draft.active_channel_tab = 999;
        draft.translation.fast_mode = true;
        draft.translation.model = "incomplete model draft !".into();
        assert!(validate_appearance(&draft).is_ok());
        assert!(validate(&draft).is_err());
        current.apply_appearance_from(&draft);
        assert_eq!(current.author_color, 0x123456);
        assert_eq!(current.author_gap, 4);
        assert!(current.translation_style_enabled);
        assert_eq!(current.translation_font_size, 24);
        assert_eq!(current.translation_text_color, 0xFEDCBA);
        assert!(current.show_diagnostics);
        assert_eq!(current.font_family, "Example Font");
        assert_eq!(current.background_opacity, 0);
        assert_eq!(current.ui_language, "ko");
        assert!(!current.romaji_japanese_input);
        assert!(current.channel_url.is_empty());
        assert!(current.webhook_url.is_empty());
        assert!(current.channel_tabs.is_empty());
        assert_eq!(current.active_channel_tab, 0);
        assert!(current.overlay_enabled);
        assert_eq!(current.hotkey_vk, 13);
        assert_eq!(current.x, 80);
        assert!(!current.translation.enabled);
        assert!(!current.translation.fast_mode);
        assert_eq!(current.translation.model, "gpt-6-luna");
    }

    #[test]
    fn inactive_connection_draft_does_not_block_active_mode() {
        let mut s = Settings {
            relay_url: "incomplete".into(),
            access_key: "draft".into(),
            ..Default::default()
        };
        assert!(validate(&s).is_ok());
        s.connection_mode = ConnectionMode::Relay;
        assert!(validate(&s).is_err());
        s.relay_url.clear();
        s.access_key.clear();
        s.webhook_url = "incomplete".into();
        assert!(validate(&s).is_ok());
        s.connection_mode = ConnectionMode::Rpc;
        assert!(validate(&s).is_err());
    }

    #[test]
    fn legacy_preferences_upgrade_without_starting_translation() {
        let old = serde_json::json!({
            "version": 1,
            "settings": {
                "connection_mode": "rpc",
                "font_size": 18,
                "target_exe": r"C:\Games\Existing.exe"
            }
        });
        let saved = decode_settings(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(saved.settings.font_size, 18);
        assert_eq!(saved.settings.target_exe, r"C:\Games\Existing.exe");
        assert_eq!(saved.settings.translation, TranslationSettings::default());
        assert!(!saved.settings.translation.enabled);
        assert!(!saved.settings.translation.fast_mode);
        assert!(saved.settings.translation.show_received);
        assert!(validate(&saved.settings).is_ok());
        let mut fast = saved.settings.translation.clone();
        fast.fast_mode = true;
        let restored: TranslationSettings =
            serde_json::from_str(&serde_json::to_string(&fast).unwrap()).unwrap();
        assert!(restored.fast_mode);
    }

    #[test]
    fn protected_openai_key_is_preserved_opaquely_and_never_serialized_in_settings() {
        // Intentionally not a real DPAPI blob. Loading must not touch DPAPI,
        // even for an enabled preference; the runtime reports a bad key later.
        let input = serde_json::json!({
            "version": 1,
            "settings": {"translation": {"enabled": true}},
            "protected_openai_api_key": "01abcdff"
        });
        let saved = decode_settings(&serde_json::to_vec(&input).unwrap()).unwrap();
        assert_eq!(saved.settings.translation.protected_api_key, "01abcdff");
        assert!(validate(&saved.settings).is_ok());
        let settings_json = serde_json::to_string(&saved.settings).unwrap();
        assert!(!settings_json.contains("01abcdff"));
        assert!(!settings_json.contains("protected_api_key"));
        assert!(!format!("{:?}", saved.settings.translation).contains("01abcdff"));
        let round_trip = decode_settings(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(
            round_trip.settings.translation.protected_api_key,
            "01abcdff"
        );
    }

    #[test]
    fn disabled_translation_does_not_open_or_decode_key() {
        let s = TranslationSettings {
            protected_api_key: "not-even-hex".into(),
            ..Default::default()
        };
        assert!(validate_translation(&s).is_ok());
        assert_eq!(
            load_openai_key(&s).unwrap_err(),
            "이 PC의 공용 번역이 꺼져 있습니다."
        );
    }

    #[test]
    fn translation_start_requires_direct_server_channel_and_valid_webhook() {
        const CHANNEL: &str = "https://discord.com/channels/111111111111111111/222222222222222222";
        const WEBHOOK: &str = "https://discord.com/api/webhooks/123456789012345678/ABCDEFGHIJKLMNOPQRSTUVWXYZ_1234567890";
        // These are syntax checks with deliberately fictional IDs/tokens.
        // No stored API key, credential decryption, or network is involved.
        assert!(validate_translation_connection(ConnectionMode::Rpc, CHANNEL, WEBHOOK).is_ok());
        assert!(validate_translation_connection(ConnectionMode::Relay, CHANNEL, WEBHOOK).is_err());
        for channel in [
            "",
            "222222222222222222",
            "http://discord.com/channels/111111111111111111/222222222222222222",
            "https://example.com/channels/111111111111111111/222222222222222222",
            "https://discord.com/channels/@me/222222222222222222",
            "https://discord.com/channels/missing/222222222222222222",
        ] {
            assert!(
                validate_translation_connection(ConnectionMode::Rpc, channel, WEBHOOK).is_err()
            );
        }
        for webhook in [
            "",
            "not-a-webhook",
            "https://discord.com/api/webhooks/123456789012345678/short",
            "https://example.com/api/webhooks/123456789012345678/ABCDEFGHIJKLMNOPQRSTUVWXYZ_1234567890",
        ] {
            assert!(validate_translation_connection(ConnectionMode::Rpc, CHANNEL, webhook).is_err());
        }
    }

    #[test]
    fn translation_configuration_rejects_missing_credentials_and_unbounded_input() {
        let mut s = TranslationSettings {
            enabled: true,
            ..Default::default()
        };
        assert!(validate_translation(&s).is_err());
        s.protected_api_key = "00".into();
        assert!(validate_translation(&s).is_ok());
        s.language_b.clone_from(&s.language_a);
        assert!(validate_translation(&s).is_err());
        s.language_b = "en".into();
        s.model = "model\r\nAuthorization: injected".into();
        assert!(validate_translation(&s).is_err());
        s.model = "gpt-6-luna".into();
        s.glossary = "가".repeat(2001);
        assert!(validate_translation(&s).is_err());
        s.glossary = "가".repeat(2000);
        assert!(validate_translation(&s).is_ok());
    }

    #[test]
    fn malformed_new_openai_key_cannot_replace_saved_key() {
        let mut s = TranslationSettings {
            protected_api_key: "previous-protected-key".into(),
            ..Default::default()
        };
        assert!(store_openai_key(&mut s, "sk-01234567890123456789\r\nInjected: x").is_err());
        assert_eq!(s.protected_api_key, "previous-protected-key");
        store_openai_key(&mut s, "").unwrap();
        assert!(s.protected_api_key.is_empty());
    }

    fn mock_protect_key(value: &str) -> Result<Vec<u8>, String> {
        Ok(value.as_bytes().to_vec())
    }

    fn mock_unprotect_key(value: &[u8]) -> Result<String, String> {
        String::from_utf8(value.to_vec()).map_err(|_| "mock decode failed".into())
    }

    #[test]
    fn provider_credentials_round_trip_with_destination_and_without_plaintext() {
        for provider in [
            ApiProvider::OpenAi,
            ApiProvider::OpenAiCompatible,
            ApiProvider::Anthropic,
            ApiProvider::Gemini,
        ] {
            let mut settings = Settings::default();
            settings.translation.provider = provider;
            if provider == ApiProvider::OpenAiCompatible {
                settings.translation.api_base_url = "https://translate.example.test/v1".into();
            }
            store_translation_key_with(
                &mut settings.translation,
                "fictional-token",
                mock_protect_key,
            )
            .unwrap();
            settings.translation.enabled = true;
            let public = serde_json::to_string(&settings).unwrap();
            assert!(!public.contains("protected_api_key"));
            assert!(!public.contains("credential_identity"));
            let bytes = encode_settings(&settings, |_| panic!("no Discord secrets")).unwrap();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(json.get("protected_openai_api_key").is_none());
            assert!(json.get("protected_translation_credential").is_some());
            assert!(!String::from_utf8_lossy(&bytes).contains("fictional-token"));
            let restored = decode_settings(&bytes).unwrap().settings;
            assert_eq!(restored.translation.provider, provider);
            assert_eq!(
                restored.translation.credential_identity,
                settings.translation.credential_identity
            );
            assert_eq!(
                load_translation_key_with(&restored.translation, mock_unprotect_key).unwrap(),
                "fictional-token"
            );
        }
    }

    #[test]
    fn destination_change_rejects_old_key_before_decryption_but_model_change_keeps_it() {
        let mut settings = TranslationSettings {
            enabled: true,
            provider: ApiProvider::OpenAiCompatible,
            api_base_url: "https://translate.example.test/v1".into(),
            ..Default::default()
        };
        store_translation_key_with(&mut settings, "short", mock_protect_key).unwrap();
        settings.api_base_url = "https://TRANSLATE.example.test:443/v1/".into();
        settings.model = "organization/model:latest".into();
        assert!(validate_translation(&settings).is_ok());
        assert_eq!(
            load_translation_key_with(&settings, mock_unprotect_key).unwrap(),
            "short"
        );
        for (provider, address) in [
            (
                ApiProvider::OpenAiCompatible,
                "https://other.example.test/v1",
            ),
            (
                ApiProvider::OpenAiCompatible,
                "https://translate.example.test/v2",
            ),
            (ApiProvider::Anthropic, ""),
        ] {
            let mut changed = settings.clone();
            changed.provider = provider;
            changed.api_base_url = address.into();
            assert!(validate_translation(&changed).is_err());
            assert!(load_translation_key_with(&changed, |_| panic!(
                "destination mismatch must be checked before decryption"
            ))
            .is_err());
        }
    }

    #[test]
    fn encrypted_scope_still_rejects_a_relabelled_credential_record() {
        let mut settings = TranslationSettings {
            enabled: true,
            provider: ApiProvider::OpenAiCompatible,
            api_base_url: "https://original.example.test/v1".into(),
            ..Default::default()
        };
        store_translation_key_with(&mut settings, "fictional-token", mock_protect_key).unwrap();
        settings.api_base_url = "https://different.example.test/v1".into();
        settings.credential_identity = Some(
            translation_credential_identity(settings.provider, &settings.api_base_url).unwrap(),
        );
        assert!(validate_translation_key_binding(&settings).is_ok());
        assert!(load_translation_key_with(&settings, mock_unprotect_key).is_err());
    }

    #[test]
    fn legacy_plaintext_key_is_only_usable_at_official_openai() {
        let mut settings = TranslationSettings {
            enabled: true,
            protected_api_key: encode_hex(b"fictional-legacy-key"),
            ..Default::default()
        };
        assert_eq!(
            load_translation_key_with(&settings, mock_unprotect_key).unwrap(),
            "fictional-legacy-key"
        );
        settings.provider = ApiProvider::OpenAiCompatible;
        settings.api_base_url = "https://api.openai.com/v1".into();
        settings.credential_identity = Some(
            translation_credential_identity(settings.provider, &settings.api_base_url).unwrap(),
        );
        assert!(load_translation_key_with(&settings, mock_unprotect_key).is_err());
    }

    #[test]
    fn failed_key_replacement_preserves_ciphertext_and_destination_binding() {
        let mut settings = TranslationSettings::default();
        store_translation_key_with(&mut settings, "old-key", mock_protect_key).unwrap();
        let original = settings.clone();
        assert!(
            store_translation_key_with(&mut settings, "new-key", |_| Err(
                "simulated DPAPI failure".into()
            ))
            .is_err()
        );
        assert_eq!(settings, original);
        assert!(
            store_translation_key_with(&mut settings, "header\r\ninjection", |_| panic!(
                "invalid key must not reach DPAPI"
            ))
            .is_err()
        );
        assert_eq!(settings, original);
        store_translation_key_with(&mut settings, "", |_| panic!("clear needs no DPAPI")).unwrap();
        assert!(settings.protected_api_key.is_empty());
        assert!(settings.credential_identity.is_none());
    }

    #[test]
    fn non_openai_profiles_allow_custom_models_but_never_openai_fast_mode() {
        let mut settings = TranslationSettings {
            provider: ApiProvider::OpenAiCompatible,
            api_base_url: "https://translate.example.test/v1".into(),
            model: "organization/model@revision+variant~1".into(),
            ..Default::default()
        };
        assert!(validate_translation(&settings).is_ok());
        assert!(settings.protected_api_key.is_empty());
        settings.fast_mode = true;
        assert!(validate_translation(&settings).is_err());
        settings.fast_mode = false;
        settings.model = "x".repeat(201);
        assert!(validate_translation(&settings).is_err());
    }
}
