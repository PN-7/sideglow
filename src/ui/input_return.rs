// SPDX-License-Identifier: MPL-2.0
//! Keep a requested game handoff pending until Sideglow has consumed the
//! closing key release and the corresponding physical keys are no longer down.
//! The native UI owns focus, sending, IME handling, and key-state sampling.

#[derive(Default)]
pub(super) struct ReturnGate {
    origin: Option<usize>,
}

#[derive(Default)]
pub(super) struct KeyState {
    pub enter_queued: bool,
    pub enter_physical: bool,
    pub escape_queued: bool,
    pub escape_physical: bool,
}

impl ReturnGate {
    /// Remember the original input context. Repeated requests must not move a
    /// pending handoff to a newly selected window or a different key sequence.
    pub(super) fn request(&mut self, origin: usize) {
        if origin != 0 && self.origin.is_none() {
            self.origin = Some(origin);
        }
    }

    pub(super) fn pending(&self) -> bool {
        self.origin.is_some()
    }

    pub(super) fn cancel(&mut self) {
        self.origin = None;
    }

    /// Return true exactly once for a ready handoff. An invalid or changed
    /// context cancels it; a later release cannot steal focus back.
    pub(super) fn take_ready(&mut self, current_origin: usize, keys: KeyState) -> bool {
        let Some(origin) = self.origin else {
            return false;
        };
        if current_origin == 0 || current_origin != origin {
            self.cancel();
            return false;
        }
        if keys.enter_queued || keys.enter_physical || keys.escape_queued || keys.escape_physical {
            return false;
        }
        // Clear before the caller enters User32, which may reenter the UI.
        self.cancel();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_both_queued_and_physical_enter_release() {
        let mut gate = ReturnGate::default();
        gate.request(11);
        assert!(!gate.take_ready(
            11,
            KeyState {
                enter_queued: true,
                enter_physical: true,
                ..KeyState::default()
            }
        ));
        // The OS can already see release while its key-up is still queued.
        assert!(!gate.take_ready(
            11,
            KeyState {
                enter_queued: true,
                ..KeyState::default()
            }
        ));
        // Conversely, a new press must not escape after an older queued up.
        assert!(!gate.take_ready(
            11,
            KeyState {
                enter_physical: true,
                ..KeyState::default()
            }
        ));
        assert!(gate.pending());
        assert!(gate.take_ready(11, KeyState::default()));
        assert!(!gate.pending());
    }

    #[test]
    fn overlapping_enter_and_escape_must_both_be_released() {
        let mut gate = ReturnGate::default();
        gate.request(11);
        gate.request(11);
        assert!(!gate.take_ready(
            11,
            KeyState {
                enter_queued: true,
                enter_physical: true,
                escape_queued: true,
                escape_physical: true,
            }
        ));
        assert!(!gate.take_ready(
            11,
            KeyState {
                escape_queued: true,
                ..KeyState::default()
            }
        ));
        assert!(!gate.take_ready(
            11,
            KeyState {
                escape_physical: true,
                ..KeyState::default()
            }
        ));
        assert!(gate.take_ready(11, KeyState::default()));
        assert!(!gate.take_ready(11, KeyState::default()));
    }

    #[test]
    fn changed_or_invalid_context_cancels_even_while_a_key_is_held() {
        for next_origin in [0, 22] {
            let mut gate = ReturnGate::default();
            gate.request(11);
            assert!(!gate.take_ready(
                next_origin,
                KeyState {
                    enter_physical: true,
                    ..KeyState::default()
                }
            ));
            assert!(!gate.pending());
            // Returning to the old window and releasing later cannot revive it.
            assert!(!gate.take_ready(11, KeyState::default()));
        }
        let mut gate = ReturnGate::default();
        gate.request(11);
        gate.cancel();
        assert!(!gate.pending());
        assert!(!gate.take_ready(11, KeyState::default()));
    }

    #[test]
    fn repeated_requests_preserve_the_original_context() {
        let mut gate = ReturnGate::default();
        gate.request(11);
        gate.request(22);
        gate.request(0);
        assert!(gate.take_ready(11, KeyState::default()));

        gate.request(11);
        gate.request(22);
        assert!(!gate.take_ready(22, KeyState::default()));
        assert!(!gate.pending());
    }

    #[test]
    fn already_released_handoff_is_one_shot_and_unrequested_handoff_is_inert() {
        let mut gate = ReturnGate::default();
        assert!(!gate.take_ready(11, KeyState::default()));
        gate.request(0);
        assert!(!gate.pending());
        assert!(!gate.take_ready(0, KeyState::default()));

        gate.request(11);
        assert!(gate.take_ready(11, KeyState::default()));
        assert!(!gate.pending());
        assert!(!gate.take_ready(11, KeyState::default()));

        gate.request(11);
        assert!(gate.take_ready(11, KeyState::default()));
    }
}
