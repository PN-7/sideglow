// SPDX-License-Identifier: MPL-2.0
//! Scoped, genuine Windows hotkeys for user-requested foreground activation.
//!
//! Detecting a key in WH_KEYBOARD_LL and posting our own WM_APP does not grant
//! SetForegroundWindow permission. A real RegisterHotKey -> WM_HOTKEY does.
//! The parent MUST call its activation code synchronously from WM_HOTKEY.
//!
//! This module never synthesizes keyboard input or posts a fake WM_HOTKEY.
//! Its low-level hook is only a scope/repeat guard; it never requests activation.

use std::{
    cell::Cell,
    marker::PhantomData,
    ptr::{null, null_mut},
    rc::Rc,
    sync::atomic::{AtomicPtr, Ordering},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT, VK_F12},
        WindowsAndMessaging::*,
    },
};

pub(super) const HOTKEY_ID: i32 = 0x574F;
pub(super) const WM_REFRESH_SCOPE: u32 = WM_APP + 20;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Binding {
    game: HWND,
    key: u32,
    modifiers: u32,
}

struct State {
    owner: HWND,
    keyboard_hook: Cell<HHOOK>,
    foreground_hook: Cell<HWINEVENTHOOK>,
    requested: Cell<Option<Binding>>,
    registered: Cell<Option<Binding>>,
    failed: Cell<Option<(Binding, u32)>>,
    trigger_down: Cell<bool>,
    swallow_key: Cell<u32>,
}

// Both hooks execute on the UI thread that installed them. Cell provides
// interior mutability without keeping an exclusive Rust reference across a
// User32 call that may dispatch a nested callback. The public handle is !Send.
static HOOK_STATE: AtomicPtr<State> = AtomicPtr::new(null_mut());

pub(super) struct Hotkey {
    state: Box<State>,
    _ui_thread_only: PhantomData<Rc<()>>,
}

impl Hotkey {
    pub(super) fn install(owner: HWND) -> Result<Self, String> {
        let state = Box::new(State {
            owner,
            keyboard_hook: Cell::new(null_mut()),
            foreground_hook: Cell::new(null_mut()),
            requested: Cell::new(None),
            registered: Cell::new(None),
            failed: Cell::new(None),
            trigger_down: Cell::new(false),
            swallow_key: Cell::new(0),
        });
        let pointer = (&*state as *const State).cast_mut();
        if HOOK_STATE
            .compare_exchange(null_mut(), pointer, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(crate::i18n::text(crate::i18n::Key::MA8CEAFBAFDCC));
        }
        let manager = Self {
            state,
            _ui_thread_only: PhantomData,
        };
        unsafe {
            let keyboard = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(scope_guard),
                GetModuleHandleW(null()),
                0,
            );
            if keyboard.is_null() {
                return Err({
                    let __ui_arg0 = &(GetLastError());
                    crate::i18n::format(
                        crate::i18n::Key::M98F527BF9AD0,
                        &[("arg0", format!("{}", __ui_arg0))],
                    )
                });
            }
            manager.state.keyboard_hook.set(keyboard);
            let foreground = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(foreground_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if foreground.is_null() {
                return Err({
                    let __ui_arg0 = &(GetLastError());
                    crate::i18n::format(
                        crate::i18n::Key::M428A51A0F3BB,
                        &[("arg0", format!("{}", __ui_arg0))],
                    )
                });
            }
            manager.state.foreground_hook.set(foreground);
        }
        Ok(manager)
    }

    /// Reconcile after a foreground notification, mode/settings change, and
    /// periodically as a fallback. The caller resolves the game's executable
    /// outside all keyboard hooks. A null/mismatched game never registers a key.
    pub(super) fn sync_scope(
        &self,
        game: HWND,
        key: u32,
        modifiers: u32,
        enabled: bool,
    ) -> Result<(), String> {
        let state = &self.state;
        let wanted = if enabled && !game.is_null() && unsafe { GetForegroundWindow() } == game {
            Some(Binding {
                game,
                key,
                modifiers: modifiers & 15,
            })
        } else {
            None
        };
        if state.requested.get() != wanted {
            state.disarm();
            state.requested.set(wanted);
            state.failed.set(None);
            state.trigger_down.set(false);
        }
        let Some(binding) = wanted else {
            state.disarm();
            return Ok(());
        };
        if key == VK_F12 as u32 {
            state.disarm();
            return Err(crate::i18n::text(crate::i18n::Key::M419420169317));
        }
        if let Some((failed_binding, code)) = state.failed.get() {
            if binding == failed_binding {
                return Err(registration_error(code));
            }
        }
        if state.registered.get() == Some(binding) {
            return Ok(());
        }
        state.disarm();
        unsafe {
            if RegisterHotKey(
                state.owner,
                HOTKEY_ID,
                binding.modifiers | MOD_NOREPEAT,
                key,
            ) == 0
            {
                let code = GetLastError();
                state.failed.set(Some((binding, code)));
                return Err(registration_error(code));
            }
        }
        state.registered.set(Some(binding));
        Ok(())
    }

    /// Call ONLY from the real WM_HOTKEY handler. Returns the game to restore
    /// after input, and releases the global registration before EDIT gets focus.
    /// The first key-down was delivered to Windows, not swallowed by our hook.
    pub(super) fn take_hotkey(&self, wp: WPARAM, lp: LPARAM) -> Option<HWND> {
        if wp != HOTKEY_ID as usize {
            return None;
        }
        let state = &self.state;
        let binding = state.registered.get()?;
        let key = ((lp as usize >> 16) & 0xffff) as u32;
        let modifiers = (lp as usize & 15) as u32;
        if key != binding.key
            || modifiers != binding.modifiers
            || unsafe { GetForegroundWindow() } != binding.game
        {
            state.disarm();
            return None;
        }
        // When the opening key is held, prevent its auto-repeat from immediately
        // sending a restored draft after we unregister and focus EDIT. The guard
        // permits the physical key-up to reach Windows: the original key-down
        // was permitted too, so suppressing release could leave key state down.
        if state.trigger_down.get() {
            state.swallow_key.set(binding.key);
        }
        state.disarm();
        state.requested.set(None);
        state.failed.set(None);
        Some(binding.game)
    }
}

impl State {
    fn disarm(&self) {
        if self.registered.replace(None).is_some() {
            unsafe { UnregisterHotKey(self.owner, HOTKEY_ID) };
        }
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        // Detach the callback pointer before unregistering anything or freeing
        // State. All resources were installed on this same UI thread.
        HOOK_STATE.store(null_mut(), Ordering::Release);
        self.state.disarm();
        unsafe {
            let keyboard = self.state.keyboard_hook.replace(null_mut());
            if !keyboard.is_null() {
                UnhookWindowsHookEx(keyboard);
            }
            let foreground = self.state.foreground_hook.replace(null_mut());
            if !foreground.is_null() {
                UnhookWinEvent(foreground);
            }
        }
    }
}

fn registration_error(code: u32) -> String {
    crate::i18n::format(
        crate::i18n::Key::ME20A981C449B,
        &[("code", format!("{}", code))],
    )
}

unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let pointer = HOOK_STATE.load(Ordering::Acquire);
    if pointer.is_null() {
        return;
    }
    let state = &*pointer;
    if state
        .registered
        .get()
        .is_some_and(|binding| GetForegroundWindow() != binding.game)
    {
        state.disarm();
    }
    // Executable lookup and registration reconciliation belong in the normal
    // window message handler, not in the event callback.
    PostMessageW(state.owner, WM_REFRESH_SCOPE, 0, 0);
}

unsafe extern "system" fn scope_guard(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(null_mut(), code, wp, lp);
    }
    let pointer = HOOK_STATE.load(Ordering::Acquire);
    if pointer.is_null() {
        return CallNextHookEx(null_mut(), code, wp, lp);
    }
    let state = &*pointer;
    let key = &*(lp as *const KBDLLHOOKSTRUCT);
    let down = wp == WM_KEYDOWN as usize || wp == WM_SYSKEYDOWN as usize;
    let up = wp == WM_KEYUP as usize || wp == WM_SYSKEYUP as usize;

    if key.flags & LLKHF_INJECTED == 0 && state.swallow_key.get() == key.vkCode {
        if up {
            state.swallow_key.set(0);
            state.trigger_down.set(false);
            return CallNextHookEx(null_mut(), code, wp, lp);
        }
        if down {
            return 1;
        }
    }

    if let Some(binding) = state.registered.get() {
        // Foreground event delivery is asynchronous. Cancel stale scope at the
        // keyboard edge too, before returning this input to the normal path.
        // No synthetic key replay, process lookup, network IO, or UI activation.
        if GetForegroundWindow() != binding.game {
            state.disarm();
            state.trigger_down.set(false);
            PostMessageW(state.owner, WM_REFRESH_SCOPE, 0, 0);
        } else if key.vkCode == binding.key {
            if key.flags & LLKHF_INJECTED != 0 {
                state.disarm();
                PostMessageW(state.owner, WM_REFRESH_SCOPE, 0, 0);
            } else if down {
                state.trigger_down.set(true);
            } else if up {
                state.trigger_down.set(false);
            }
        }
    }
    // The genuine opening key must continue to Windows so it can generate the
    // registered WM_HOTKEY and confer foreground activation permission.
    CallNextHookEx(null_mut(), code, wp, lp)
}
