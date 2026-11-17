// SPDX-License-Identifier: MPL-2.0
//! UI language catalogs loaded only on startup or an explicit language change.
//! The render path indexes a prepared table; it never reads or parses a file.

mod keys;
pub use keys::Key;

use serde::{de, Deserialize};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    fs::{self, File},
    io::{self, Read},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock, RwLockReadGuard},
};

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_MESSAGE_ENTRIES: usize = 4096;
const MAX_LANGUAGE_FILES: usize = 64;
const MAX_TEMPLATE_PARTS: usize = 256;

pub(super) struct Definition {
    pub id: &'static str,
    pub text: &'static str,
    /// Placeholder occurrences, including repeats. Values are preformatted by
    /// the Rust caller before substitution; locale files cannot choose formats.
    pub fields: &'static [&'static str],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageChoice {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadWarning {
    MissingFile,
    Malformed,
    WrongLocale,
    TooLarge,
    InvalidEntries(usize),
    ReadFailure,
    InvalidLanguage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadReport {
    /// A validated filename identifier, or empty when the identifier is invalid.
    pub requested: String,
    pub effective: String,
    /// No filesystem path, file contents, or parser error is exposed here.
    pub warnings: Vec<LoadWarning>,
}

enum Part {
    Text(Range<usize>),
    Brace(char),
    Field(Range<usize>),
}

struct Template {
    parts: Vec<Part>,
}

struct Entry {
    text: Cow<'static, str>,
    template: Option<Template>,
    wide: OnceLock<Arc<[u16]>>,
}

impl Entry {
    fn default(definition: &Definition) -> Self {
        Self {
            text: Cow::Borrowed(definition.text),
            template: (!definition.fields.is_empty())
                .then(|| parse_template(definition.text).ok())
                .flatten(),
            wide: OnceLock::new(),
        }
    }

    fn translated(text: String, definition: &Definition) -> Option<Self> {
        if text.len() > MAX_MESSAGE_BYTES
            || text.trim().is_empty()
            || text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return None;
        }
        let template = if definition.fields.is_empty() {
            // Static UI text may contain braces, JSON examples, or paths. It
            // does not become a format string just because a translator edits it.
            None
        } else {
            let template = parse_template(&text).ok()?;
            let mut actual = BTreeMap::new();
            for part in &template.parts {
                if let Part::Field(range) = part {
                    *actual.entry(&text[range.clone()]).or_insert(0usize) += 1;
                }
            }
            let mut expected = BTreeMap::new();
            for &field in definition.fields {
                *expected.entry(field).or_insert(0usize) += 1;
            }
            if actual != expected {
                return None;
            }
            Some(template)
        };
        Some(Self {
            text: Cow::Owned(text),
            template,
            wide: OnceLock::new(),
        })
    }

    fn format(&self, args: &[(&str, String)]) -> String {
        let Some(template) = &self.template else {
            return self.text.to_string();
        };
        let mut output = String::with_capacity(self.text.len());
        for part in &template.parts {
            match part {
                Part::Text(range) => output.push_str(&self.text[range.clone()]),
                Part::Brace(brace) => output.push(*brace),
                Part::Field(range) => {
                    let name = &self.text[range.clone()];
                    if let Some((_, value)) = args.iter().find(|(key, _)| *key == name) {
                        // Append once. Braces or placeholder-looking user text
                        // inside an argument are never interpreted a second time.
                        output.push_str(value);
                    } else {
                        // An internal call-site mismatch stays readable and
                        // cannot panic or erase the surrounding error context.
                        output.push('{');
                        output.push_str(name);
                        output.push('}');
                    }
                }
            }
        }
        output
    }

    fn wide(&self) -> Arc<[u16]> {
        self.wide
            .get_or_init(|| self.text.encode_utf16().chain(Some(0)).collect())
            .clone()
    }
}

fn parse_template(text: &str) -> Result<Template, ()> {
    fn push(parts: &mut Vec<Part>, part: Part) -> Result<(), ()> {
        if parts.len() == MAX_TEMPLATE_PARTS {
            return Err(());
        }
        parts.push(part);
        Ok(())
    }

    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut position = 0;
    let mut literal = 0;
    while position < bytes.len() {
        if !matches!(bytes[position], b'{' | b'}') {
            position += 1;
            continue;
        }
        if literal < position {
            push(&mut parts, Part::Text(literal..position))?;
        }
        let brace = bytes[position];
        if bytes.get(position + 1) == Some(&brace) {
            push(&mut parts, Part::Brace(char::from(brace)))?;
            position += 2;
        } else if brace == b'{' {
            let start = position + 1;
            let end = bytes[start..]
                .iter()
                .position(|&value| value == b'}')
                .map(|offset| start + offset)
                .ok_or(())?;
            if start == end
                || !bytes[start..end]
                    .iter()
                    .all(|value| value.is_ascii_alphanumeric() || *value == b'_')
            {
                return Err(());
            }
            push(&mut parts, Part::Field(start..end))?;
            position = end + 1;
        } else {
            return Err(());
        }
        literal = position;
    }
    if literal < bytes.len() {
        push(&mut parts, Part::Text(literal..bytes.len()))?;
    }
    Ok(Template { parts })
}

struct Catalog {
    language: String,
    entries: Vec<Arc<Entry>>,
}

static DEFAULTS: OnceLock<Vec<Arc<Entry>>> = OnceLock::new();
static ACTIVE: OnceLock<RwLock<Catalog>> = OnceLock::new();

fn default_entries() -> &'static [Arc<Entry>] {
    DEFAULTS.get_or_init(|| {
        keys::DEFINITIONS
            .iter()
            .map(|definition| Arc::new(Entry::default(definition)))
            .collect()
    })
}

fn active() -> &'static RwLock<Catalog> {
    ACTIVE.get_or_init(|| {
        RwLock::new(Catalog {
            language: "ko".into(),
            entries: default_entries().to_vec(),
        })
    })
}

fn read_catalog(catalog: &RwLock<Catalog>) -> RwLockReadGuard<'_, Catalog> {
    catalog.read().unwrap_or_else(|error| error.into_inner())
}

fn replace_catalog(catalog: &RwLock<Catalog>, replacement: Catalog) {
    *catalog.write().unwrap_or_else(|error| error.into_inner()) = replacement;
}

fn entry(key: Key) -> Arc<Entry> {
    let catalog = read_catalog(active());
    catalog.entries[key as usize].clone()
}

pub fn text(key: Key) -> String {
    entry(key).text.to_string()
}

pub fn format(key: Key, args: &[(&str, String)]) -> String {
    entry(key).format(args)
}

/// Cache null-terminated Windows text only for labels actually requested.
/// Returned Arcs remain valid while a caller makes a Win32 call; changing the
/// language releases unused catalog entries instead of leaking old strings.
pub fn wide(key: Key) -> Arc<[u16]> {
    entry(key).wide()
}

pub fn current_language() -> String {
    read_catalog(active()).language.clone()
}

fn locale_directory() -> io::Result<PathBuf> {
    std::env::current_exe()?
        .parent()
        .map(|parent| parent.join("locales"))
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

pub fn load_language(id: &str) -> LoadReport {
    let (catalog, report) = match locale_directory() {
        Ok(directory) => load_from_directory(id, &directory, keys::DEFINITIONS, default_entries()),
        Err(_) => fallback(id, LoadWarning::ReadFailure, default_entries()),
    };
    replace_catalog(active(), catalog);
    report
}

pub fn available_languages() -> Vec<LanguageChoice> {
    choices_from_directory(locale_directory().ok().as_deref())
}

fn choices_from_directory(directory: Option<&Path>) -> Vec<LanguageChoice> {
    let mut choices = vec![
        LanguageChoice {
            id: "ko".into(),
            label: "한국어".into(),
        },
        LanguageChoice {
            id: "en".into(),
            label: "English".into(),
        },
        LanguageChoice {
            id: "ja".into(),
            label: "日本語".into(),
        },
    ];
    let mut seen: HashSet<String> = choices.iter().map(|choice| choice.id.clone()).collect();
    let mut extra = Vec::new();
    if let Some(entries) = directory.and_then(|path| fs::read_dir(path).ok()) {
        // Enumerate names only, with a fixed bound. Never parse every language
        // merely to show the picker or run a watcher/polling worker.
        for entry in entries.take(MAX_LANGUAGE_FILES).flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if crate::config::valid_ui_language(id) && seen.insert(id.to_owned()) {
                extra.push(LanguageChoice {
                    id: id.to_owned(),
                    label: id.to_owned(),
                });
            }
        }
    }
    extra.sort_by(|left, right| left.id.cmp(&right.id));
    choices.extend(extra.into_iter().take(MAX_LANGUAGE_FILES - choices.len()));
    choices
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    locale: String,
    name: String,
    messages: UniqueMessages,
}

struct UniqueMessages(BTreeMap<String, String>);

impl<'de> Deserialize<'de> for UniqueMessages {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct MessageVisitor;
        impl<'de> de::Visitor<'de> for MessageVisitor {
            type Value = UniqueMessages;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded object of unique string messages")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut messages = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if messages.len() >= MAX_MESSAGE_ENTRIES || key.len() > 128 {
                        return Err(de::Error::custom("message limit exceeded"));
                    }
                    if messages.contains_key(&key) {
                        return Err(de::Error::custom("duplicate message key"));
                    }
                    messages.insert(key, map.next_value::<String>()?);
                }
                Ok(UniqueMessages(messages))
            }
        }
        deserializer.deserialize_map(MessageVisitor)
    }
}

fn fallback(id: &str, warning: LoadWarning, defaults: &[Arc<Entry>]) -> (Catalog, LoadReport) {
    let valid = crate::config::valid_ui_language(id);
    let warnings = if !valid {
        vec![LoadWarning::InvalidLanguage]
    } else if id == "ko" && warning == LoadWarning::MissingFile {
        Vec::new()
    } else {
        vec![warning]
    };
    (
        Catalog {
            language: "ko".into(),
            entries: defaults.to_vec(),
        },
        LoadReport {
            requested: if valid { id.to_owned() } else { String::new() },
            effective: "ko".into(),
            warnings,
        },
    )
}

fn load_from_directory(
    id: &str,
    directory: &Path,
    definitions: &[Definition],
    defaults: &[Arc<Entry>],
) -> (Catalog, LoadReport) {
    if !crate::config::valid_ui_language(id) {
        return fallback(id, LoadWarning::InvalidLanguage, defaults);
    }
    let result = (|| -> Result<Vec<u8>, LoadWarning> {
        let file = File::open(directory.join(format!("{id}.json"))).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                LoadWarning::MissingFile
            } else {
                LoadWarning::ReadFailure
            }
        })?;
        let metadata = file.metadata().map_err(|_| LoadWarning::ReadFailure)?;
        if !metadata.is_file() {
            return Err(LoadWarning::ReadFailure);
        }
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(LoadWarning::TooLarge);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| LoadWarning::ReadFailure)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(LoadWarning::TooLarge);
        }
        Ok(bytes)
    })();
    match result {
        Ok(bytes) => parse_catalog(id, &bytes, definitions, defaults),
        Err(warning) => fallback(id, warning, defaults),
    }
}

fn parse_catalog(
    id: &str,
    bytes: &[u8],
    definitions: &[Definition],
    defaults: &[Arc<Entry>],
) -> (Catalog, LoadReport) {
    if !crate::config::valid_ui_language(id) {
        return fallback(id, LoadWarning::InvalidLanguage, defaults);
    }
    if bytes.len() > MAX_FILE_BYTES {
        return fallback(id, LoadWarning::TooLarge, defaults);
    }
    // UTF-8 BOMs from text editors are harmless; the rest is strict JSON.
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let pack: RawCatalog = match serde_json::from_slice(bytes) {
        Ok(pack) => pack,
        Err(_) => return fallback(id, LoadWarning::Malformed, defaults),
    };
    if pack.locale != id {
        return fallback(id, LoadWarning::WrongLocale, defaults);
    }
    if pack.name.trim().is_empty()
        || pack.name.len() > 128
        || pack.name.chars().any(char::is_control)
    {
        return fallback(id, LoadWarning::Malformed, defaults);
    }
    let positions: HashMap<&str, usize> = definitions
        .iter()
        .enumerate()
        .map(|(index, definition)| (definition.id, index))
        .collect();
    let mut entries = defaults.to_vec();
    let mut invalid = 0;
    for (key, value) in pack.messages.0 {
        let Some(&index) = positions.get(key.as_str()) else {
            invalid += 1;
            continue;
        };
        if let Some(entry) = Entry::translated(value, &definitions[index]) {
            entries[index] = Arc::new(entry);
        } else {
            invalid += 1;
        }
    }
    (
        Catalog {
            language: id.to_owned(),
            entries,
        },
        LoadReport {
            requested: id.to_owned(),
            effective: id.to_owned(),
            warnings: if invalid == 0 {
                Vec::new()
            } else {
                vec![LoadWarning::InvalidEntries(invalid)]
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const TEST_DEFINITIONS: &[Definition] = &[
        Definition {
            id: "settings.title",
            text: "설정",
            fields: &[],
        },
        Definition {
            id: "send.error",
            text: "{user}: {error} ({user})",
            fields: &["user", "error", "user"],
        },
        Definition {
            id: "static.example",
            text: "JSON 예시: {value}",
            fields: &[],
        },
    ];

    fn defaults() -> Vec<Arc<Entry>> {
        TEST_DEFINITIONS
            .iter()
            .map(|definition| Arc::new(Entry::default(definition)))
            .collect()
    }

    fn parse(value: &str) -> (Catalog, LoadReport) {
        parse_catalog("en", value.as_bytes(), TEST_DEFINITIONS, &defaults())
    }

    struct TempDirectory(PathBuf);
    impl TempDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "sideglow-i18n-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn partial_packs_keep_missing_defaults_and_reject_invalid_entries_individually() {
        let (catalog, report) = parse(
            r#"{"locale":"en","name":"English","messages":{"settings.title":"Settings","send.error":"Missing {error}","unused":"Extra"}}"#,
        );
        assert_eq!(report.effective, "en");
        assert_eq!(report.warnings, vec![LoadWarning::InvalidEntries(2)]);
        assert_eq!(catalog.entries[0].text, "Settings");
        assert_eq!(catalog.entries[1].text, TEST_DEFINITIONS[1].text);
        assert_eq!(catalog.entries[2].text, TEST_DEFINITIONS[2].text);
    }

    #[test]
    fn reordered_repeated_fields_escape_braces_and_never_reinterpret_argument_text() {
        let (catalog, report) = parse(
            r#"{"locale":"en","name":"English","messages":{"send.error":"{{Notice}} {error}: {user}/{user}"}}"#,
        );
        assert!(report.warnings.is_empty());
        assert_eq!(
            catalog.entries[1].format(&[("user", "A{error}".into()), ("error", "{user}".into())]),
            "{Notice} {user}: A{error}/A{error}"
        );
        // Missing caller arguments remain visible instead of panicking.
        assert_eq!(
            catalog.entries[1].format(&[]),
            "{Notice} {error}: {user}/{user}"
        );
    }

    #[test]
    fn placeholder_multiplicity_names_and_syntax_must_match_exactly() {
        for bad in [
            "{user}: {error}",
            "{user}: {error} {user} {extra}",
            "{user}: {error} {user:.2}",
            "{user}: {error} {user",
            "{user}: {error} {user}}",
            "{user}: {error} {}",
        ] {
            assert!(
                Entry::translated(bad.into(), &TEST_DEFINITIONS[1]).is_none(),
                "{bad}"
            );
        }
    }

    #[test]
    fn static_braces_are_literal_and_utf16_is_cached_with_one_terminator() {
        let entry = Entry::translated("例 {value} {{x}} 😌".into(), &TEST_DEFINITIONS[2]).unwrap();
        assert_eq!(
            entry.format(&[("value", "changed".into())]),
            "例 {value} {{x}} 😌"
        );
        let first = entry.wide();
        let second = entry.wide();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&first[..first.len() - 1]).unwrap(),
            entry.text
        );
        assert_eq!(first.iter().filter(|&&value| value == 0).count(), 1);
    }

    #[test]
    fn duplicate_json_keys_and_wrong_schema_fail_without_exposing_contents() {
        for bad in [
            r#"{"locale":"en","locale":"en","name":"secret","messages":{}}"#,
            r#"{"locale":"en","name":"English","messages":{"settings.title":"A","settings.title":"B"}}"#,
            r#"{"locale":"en","name":"English","messages":{"settings.title":"A","settings.\u0074itle":"B"}}"#,
            r#"{"locale":"en","name":"English","messages":{"settings.title":9}}"#,
            r#"{"locale":"en","name":"English","messages":{},"unknown":"secret"}"#,
            r#"{"locale":"en","messages":{}}"#,
        ] {
            let (catalog, report) = parse(bad);
            assert_eq!(catalog.language, "ko");
            assert_eq!(report.warnings, vec![LoadWarning::Malformed]);
            assert!(!format!("{report:?}").contains("secret"));
        }
    }

    #[test]
    fn empty_nul_control_or_oversized_messages_fall_back() {
        for bad in [
            String::new(),
            "  ".into(),
            "bad\0text".into(),
            "bad\u{1b}text".into(),
            "x".repeat(MAX_MESSAGE_BYTES + 1),
        ] {
            assert!(Entry::translated(bad, &TEST_DEFINITIONS[0]).is_none());
        }
        assert!(Entry::translated("First\r\nSecond\tvalue".into(), &TEST_DEFINITIONS[0]).is_some());
    }

    #[test]
    fn invalid_locale_ids_never_become_paths_or_reflected_warnings() {
        let directory = TempDirectory::new();
        for id in [
            "../secret",
            "en/../../secret",
            "NUL",
            "CON",
            "en.json",
            "en\0",
        ] {
            let (_, report) = load_from_directory(id, &directory.0, TEST_DEFINITIONS, &defaults());
            assert!(report.requested.is_empty());
            assert_eq!(report.warnings, vec![LoadWarning::InvalidLanguage]);
        }
    }

    #[test]
    fn missing_korean_pack_is_silent_but_missing_selected_language_is_reported() {
        let directory = TempDirectory::new();
        let (_, report) = load_from_directory("ko", &directory.0, TEST_DEFINITIONS, &defaults());
        assert!(report.warnings.is_empty());
        let (_, report) = load_from_directory("ja", &directory.0, TEST_DEFINITIONS, &defaults());
        assert_eq!(report.requested, "ja");
        assert_eq!(report.effective, "ko");
        assert_eq!(report.warnings, vec![LoadWarning::MissingFile]);
    }

    #[test]
    fn loader_bounds_file_size_and_detects_wrong_locale_and_utf8() {
        let directory = TempDirectory::new();
        fs::write(directory.0.join("en.json"), vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
        let (_, report) = load_from_directory("en", &directory.0, TEST_DEFINITIONS, &defaults());
        assert_eq!(report.warnings, vec![LoadWarning::TooLarge]);
        assert_eq!(
            parse(r#"{"locale":"ja","name":"Japanese","messages":{}}"#)
                .1
                .warnings,
            vec![LoadWarning::WrongLocale]
        );
        let (_, report) = parse_catalog("en", &[0xff], TEST_DEFINITIONS, &defaults());
        assert_eq!(report.warnings, vec![LoadWarning::Malformed]);
        let (_, report) = parse_catalog(
            "en",
            b"\xef\xbb\xbf{\"locale\":\"en\",\"name\":\"English\",\"messages\":{}}",
            TEST_DEFINITIONS,
            &defaults(),
        );
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn old_catalog_and_wide_callers_remain_valid_without_retaining_old_tables() {
        let base = defaults();
        let (catalog, _) = parse_catalog(
            "en",
            br#"{"locale":"en","name":"English","messages":{"settings.title":"Settings"}}"#,
            TEST_DEFINITIONS,
            &base,
        );
        let old = Arc::downgrade(&catalog.entries[0]);
        let wide = catalog.entries[0].wide();
        assert!(Arc::ptr_eq(&catalog.entries[1], &base[1]));
        drop(catalog);
        assert!(old.upgrade().is_none());
        assert_eq!(
            String::from_utf16(&wide[..wide.len() - 1]).unwrap(),
            "Settings"
        );
    }

    #[test]
    fn language_picker_reads_names_only_and_ignores_nonfiles_and_unsafe_ids() {
        let directory = TempDirectory::new();
        fs::write(directory.0.join("fr-CA.json"), b"not parsed by the picker").unwrap();
        fs::write(directory.0.join("en.json"), b"also not parsed").unwrap();
        fs::write(directory.0.join("NUL.json"), b"rejected name").unwrap();
        fs::write(directory.0.join("other.txt"), b"ignored extension").unwrap();
        fs::create_dir(directory.0.join("de.json")).unwrap();
        let choices = choices_from_directory(Some(&directory.0));
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.id.as_str())
                .collect::<Vec<_>>(),
            ["ko", "en", "ja", "fr-CA"]
        );
        assert_eq!(choices.last().unwrap().label, "fr-CA");
    }

    #[test]
    fn generated_defaults_have_unique_ids_and_consistent_placeholders() {
        let mut ids = HashSet::new();
        for definition in keys::DEFINITIONS {
            assert!(ids.insert(definition.id), "{}", definition.id);
            assert!(
                Entry::translated(definition.text.into(), definition).is_some(),
                "{}",
                definition.id
            );
        }
    }

    #[test]
    fn packaged_languages_cover_every_generated_key_without_any_fallback() {
        let base: Vec<_> = keys::DEFINITIONS
            .iter()
            .map(|definition| Arc::new(Entry::default(definition)))
            .collect();
        for (id, bytes) in [
            ("ko", &include_bytes!("../../locales/ko.json")[..]),
            ("en", &include_bytes!("../../locales/en.json")[..]),
            ("ja", &include_bytes!("../../locales/ja.json")[..]),
        ] {
            let pack: RawCatalog = serde_json::from_slice(bytes).unwrap();
            assert_eq!(pack.locale, id);
            assert_eq!(pack.messages.0.len(), keys::DEFINITIONS.len(), "{id}");
            for definition in keys::DEFINITIONS {
                assert!(
                    pack.messages.0.contains_key(definition.id),
                    "{id}: {}",
                    definition.id
                );
            }
            let (catalog, report) = parse_catalog(id, bytes, keys::DEFINITIONS, &base);
            assert_eq!(catalog.language, id);
            assert!(report.warnings.is_empty(), "{id}: {:?}", report.warnings);
            for (index, definition) in keys::DEFINITIONS.iter().enumerate() {
                assert_eq!(catalog.entries[index].text, pack.messages.0[definition.id]);
                // Matching text could otherwise conceal a missing/invalid
                // translation that quietly reused the Korean default entry.
                assert!(
                    !Arc::ptr_eq(&catalog.entries[index], &base[index]),
                    "{id}: {}",
                    definition.id
                );
            }
        }
    }

    #[test]
    fn template_part_and_message_count_limits_bound_custom_pack_allocations() {
        let excessive = format!(
            "{}{{user}} {{error}} {{user}}",
            "{{}}".repeat(MAX_TEMPLATE_PARTS)
        );
        assert!(excessive.len() < MAX_MESSAGE_BYTES);
        assert!(Entry::translated(excessive, &TEST_DEFINITIONS[1]).is_none());

        let messages: BTreeMap<_, _> = (0..=MAX_MESSAGE_ENTRIES)
            .map(|index| (format!("unused.{index}"), "message"))
            .collect();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "locale": "en", "name": "English", "messages": messages,
        }))
        .unwrap();
        assert!(bytes.len() < MAX_FILE_BYTES);
        let (_, report) = parse_catalog("en", &bytes, TEST_DEFINITIONS, &defaults());
        assert_eq!(report.warnings, vec![LoadWarning::Malformed]);
    }

    #[test]
    fn concurrent_cached_readers_survive_replacement_without_retaining_old_entries() {
        use std::{sync::Barrier, thread};

        let first =
            br#"{"locale":"en","name":"English","messages":{"settings.title":"Settings A"}}"#;
        let second =
            br#"{"locale":"en","name":"English","messages":{"settings.title":"Settings B"}}"#;
        let base = defaults();
        let (initial, _) = parse_catalog("en", first, TEST_DEFINITIONS, &base);
        let mut old_entries = vec![Arc::downgrade(&initial.entries[0])];
        let state = Arc::new(RwLock::new(initial));
        let start = Arc::new(Barrier::new(5));
        thread::scope(|scope| {
            for _ in 0..4 {
                let state = state.clone();
                let start = start.clone();
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..512 {
                        // Match the public lookup: retain just this entry and
                        // release the catalog read lock before encoding/drawing.
                        let entry = read_catalog(&state).entries[0].clone();
                        let wide = entry.wide();
                        assert!(Arc::ptr_eq(&wide, &entry.wide()));
                        let text = String::from_utf16(&wide[..wide.len() - 1]).unwrap();
                        assert!(matches!(text.as_str(), "Settings A" | "Settings B"));
                        assert_eq!(wide.last(), Some(&0));
                    }
                });
            }
            start.wait();
            for index in 0..128 {
                let bytes = if index % 2 == 0 { second } else { first };
                let (replacement, report) = parse_catalog("en", bytes, TEST_DEFINITIONS, &base);
                assert!(report.warnings.is_empty());
                old_entries.push(Arc::downgrade(&replacement.entries[0]));
                replace_catalog(&state, replacement);
            }
        });
        drop(state);
        assert!(old_entries.iter().all(|entry| entry.upgrade().is_none()));
        assert!(base.iter().all(|entry| Arc::strong_count(entry) == 1));
    }

    #[test]
    fn poisoned_catalog_lock_can_be_read_and_replaced_without_panicking() {
        let base = defaults();
        let (initial, _) = parse_catalog(
            "en",
            br#"{"locale":"en","name":"English","messages":{"settings.title":"Settings"}}"#,
            TEST_DEFINITIONS,
            &base,
        );
        let previous = Arc::downgrade(&initial.entries[0]);
        let catalog = RwLock::new(initial);
        let interrupted = std::panic::catch_unwind(|| {
            let _guard = catalog.write().unwrap();
            panic!("simulated interrupted catalog update");
        });
        assert!(interrupted.is_err());
        assert!(catalog.is_poisoned());
        assert_eq!(read_catalog(&catalog).entries[0].text, "Settings");

        let bytes =
            r#"{"locale":"ja","name":"日本語","messages":{"settings.title":"設定"}}"#.as_bytes();
        let (replacement, report) = parse_catalog("ja", bytes, TEST_DEFINITIONS, &base);
        assert!(report.warnings.is_empty());
        replace_catalog(&catalog, replacement);
        assert_eq!(read_catalog(&catalog).language, "ja");
        let entry = read_catalog(&catalog).entries[0].clone();
        let wide = entry.wide();
        assert_eq!(String::from_utf16(&wide[..wide.len() - 1]).unwrap(), "設定");
        assert!(previous.upgrade().is_none());
    }
}
