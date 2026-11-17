// SPDX-License-Identifier: MPL-2.0
//! Window visibility rules shared by both native overlay layers.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Passive,
    Manage,
    Input,
}

/// A settings session must not leave a click-blocking management overlay above
/// the applications the user is consulting, even if main is activated again.
pub fn effective_mode(enabled: bool, requested: Mode, settings_active: bool) -> Mode {
    if settings_active || (requested == Mode::Input && !enabled) {
        Mode::Passive
    } else {
        requested
    }
}

/// Expiration removes individual chat rows, never the overlay window. Management
/// remains reachable through Alt+Tab even when chat display is switched off.
pub fn display_visible(enabled: bool, mode: Mode) -> bool {
    mode == Mode::Manage || enabled
}

/// Routine connection/ready/waiting notices belong in settings only. Timing is
/// opt-in; actual errors remain visible without parsing or filtering their text.
pub fn status_visible(
    show_diagnostics: bool,
    diagnostic: bool,
    notice: bool,
    has_status: bool,
) -> bool {
    has_status && !notice && (show_diagnostics || !diagnostic)
}

pub fn scroll_controls_visible(mode: Mode, settings_active: bool, closing: bool) -> bool {
    mode == Mode::Manage && !settings_active && !closing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_diagnostics_leave_errors_visible() {
        assert!(!status_visible(false, true, false, true));
        assert!(status_visible(true, true, false, true));
        assert!(status_visible(false, false, false, true));
        assert!(!status_visible(true, false, false, false));
    }

    #[test]
    fn routine_notices_never_reserve_an_overlay_footer() {
        for show_diagnostics in [false, true] {
            for diagnostic in [false, true] {
                assert!(!status_visible(show_diagnostics, diagnostic, true, true));
            }
            assert!(status_visible(show_diagnostics, false, false, true));
        }
    }

    #[test]
    fn scrollbar_mouse_targets_exist_only_in_management() {
        assert!(scroll_controls_visible(Mode::Manage, false, false));
        assert!(!scroll_controls_visible(Mode::Input, false, false));
        assert!(!scroll_controls_visible(Mode::Passive, false, false));
        assert!(!scroll_controls_visible(Mode::Manage, true, false));
        assert!(!scroll_controls_visible(Mode::Manage, false, true));
    }

    #[test]
    fn only_explicit_switch_hides_passive_and_input_windows() {
        for mode in [Mode::Passive, Mode::Input] {
            assert!(!display_visible(false, mode));
            assert!(display_visible(true, mode));
        }
    }

    #[test]
    fn management_is_reachable_when_display_is_disabled() {
        for enabled in [false, true] {
            assert!(display_visible(enabled, Mode::Manage));
        }
    }

    #[test]
    fn settings_session_blocks_input_and_management_until_it_closes() {
        for enabled in [false, true] {
            for requested in [Mode::Passive, Mode::Manage, Mode::Input] {
                assert!(effective_mode(enabled, requested, true) == Mode::Passive);
            }
            assert!(effective_mode(enabled, Mode::Manage, false) == Mode::Manage);
        }
        assert!(effective_mode(true, Mode::Input, false) == Mode::Input);
        assert!(effective_mode(false, Mode::Input, false) == Mode::Passive);
    }
}
