// SPDX-License-Identifier: MPL-2.0
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    use sideglow::{config, i18n, ui};
    let demo = std::env::args().any(|a| a == "--demo");
    let (settings, error) = match config::load() {
        Ok(s) => (s, None),
        Err(e) => (config::Settings::default(), Some(e)),
    };
    let locale_warning = ui::language_warning(&i18n::load_language(&settings.ui_language));
    let Some(_instance) = single_instance() else {
        return;
    };
    if let Err(e) = ui::run(settings, error, locale_warning, demo) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let text: Vec<u16> = e.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "Sideglow".encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(windows)]
struct Instance(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
#[cfg(windows)]
fn single_instance() -> Option<Instance> {
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS},
        System::Threading::CreateMutexW,
        UI::WindowsAndMessaging::{
            FindWindowW, MessageBoxW, SetForegroundWindow, ShowWindow, MB_ICONERROR, MB_OK,
            SW_RESTORE,
        },
    };
    // Keep the same lock as older releases to prevent simultaneous instances
    // from registering competing hotkeys.
    let name: Vec<u16> = "Local\\WhisperOverlay.Desktop.v1"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        let last_error = GetLastError();
        if handle.is_null() {
            let message: Vec<u16> = sideglow::i18n::text(sideglow::i18n::Key::M77E780666A99)
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let title: Vec<u16> = "Sideglow".encode_utf16().chain(Some(0)).collect();
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
            return None;
        }
        let guard = Instance(handle);
        if last_error == ERROR_ALREADY_EXISTS {
            for class_name in ["Sideglow.Main.v1", "WhisperOverlay.Main.v1"] {
                let class: Vec<u16> = class_name.encode_utf16().chain(Some(0)).collect();
                let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
                if !hwnd.is_null() {
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                    break;
                }
            }
            return None;
        }
        Some(guard)
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "Sideglow is a native Windows 10/11 application. Run cargo test for portable core tests."
    );
}
