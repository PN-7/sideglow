// SPDX-License-Identifier: MPL-2.0
//! Small, offline model picker. This is a curated list, not an account-specific
//! availability check. Custom model IDs remain editable in settings.
//!
//! Verified against the official model pages on 2026-11-12:
//! https://developers.openai.com/api/docs/models/gpt-6-luna
//! https://developers.openai.com/api/docs/models/gpt-6-sol
//! https://developers.openai.com/api/docs/models/gpt-6.1-sol
//! https://developers.openai.com/api/docs/models/gpt-6-astra
//! Do not imply that a larger model is faster or better at this user's chat.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPreset {
    pub id: &'static str,
    pub label: crate::i18n::Key,
    pub description: crate::i18n::Key,
}

pub const PRESETS: &[ModelPreset] = &[
    ModelPreset {
        id: "gpt-6-luna",
        label: crate::i18n::Key::M831DD6330F7D,
        description: crate::i18n::Key::MBE4C011D67D5,
    },
    ModelPreset {
        id: "gpt-6-sol",
        label: crate::i18n::Key::MC68516362328,
        description: crate::i18n::Key::MBB8CA1A0CADE,
    },
    ModelPreset {
        id: "gpt-6.1-sol",
        label: crate::i18n::Key::M6229F207D00C,
        description: crate::i18n::Key::M5FE9CE096C79,
    },
    ModelPreset {
        id: "gpt-6-astra",
        label: crate::i18n::Key::M869558212C7D,
        description: crate::i18n::Key::MEE614696A8AE,
    },
];

pub fn find(model: &str) -> Option<&'static ModelPreset> {
    PRESETS.iter().find(|preset| preset.id == model)
}

/// Only apply verified parameters to exact, known model IDs. A custom ID may
/// refer to a different model family or snapshot with different capabilities.
#[cfg(any(windows, test))]
pub(crate) fn reasoning_effort(model: &str) -> Option<&'static str> {
    match model {
        "gpt-6-luna" | "gpt-6-sol" => Some("none"),
        "gpt-6.1-sol" | "gpt-6-astra" => Some("low"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_parameters_are_explicit_and_custom_ids_remain_unknown() {
        assert_eq!(reasoning_effort("gpt-6-luna"), Some("none"));
        assert_eq!(reasoning_effort("gpt-6-sol"), Some("none"));
        assert_eq!(reasoning_effort("gpt-6.1-sol"), Some("low"));
        assert_eq!(reasoning_effort("gpt-6-astra"), Some("low"));
        for model in ["gpt-6-luna-custom", "gpt-6.1-sol-future", "custom-model"] {
            assert!(find(model).is_none());
            assert_eq!(reasoning_effort(model), None);
        }
        for preset in PRESETS {
            assert_eq!(find(preset.id), Some(preset));
            assert!(reasoning_effort(preset.id).is_some());
        }
    }
}
