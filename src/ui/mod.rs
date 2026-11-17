// SPDX-License-Identifier: MPL-2.0
//! Native Windows overlay. Network work remains in `transport`; this module owns HWNDs.
mod chat_text;
mod hotkey;
mod input_return;
mod scroll_math;
mod settings;
mod surface;
mod surface_math;
mod tab_bar;
mod ui_policy;

use ui_policy::Mode;

use crate::{
    channel_memory::{ChannelCache, LocalChannelState},
    config::{self, Settings},
    model::ChatStore,
    transport::{Client, ClientEvent},
};
use std::{
    collections::VecDeque,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::*},
    UI::{
        Controls::*,
        HiDpi::*,
        Input::{Ime::*, KeyboardAndMouse::*},
        WindowsAndMessaging::*,
    },
};

const WM_ACTIVATION_SETTLED: u32 = WM_APP + 1;
const WM_SEND_CHAT: u32 = WM_APP + 2;
const WM_CANCEL_INPUT: u32 = WM_APP + 3;
const WM_COMPLETE_INPUT: u32 = WM_APP + 4;
const TIMER: usize = 1;
const CLASS_MAIN: &str = "Sideglow.Main.v1";
const CLASS_BACKGROUND: &str = "Sideglow.Background.v1";
const CLASS_INPUT: &str = "Sideglow.Input.v1";
const CLASS_SCROLL: &str = "Sideglow.Scroll.v1";
const SCROLL_REPEAT_TIMER: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderAction {
    Toggle,
    Translation,
    Settings,
    Close,
    Channel(u64),
    AddChannel,
}

struct HeaderControls {
    toggle: RECT,
    translation: RECT,
    settings: RECT,
    close: RECT,
}

fn header_controls(width: i32, dpi: u32) -> HeaderControls {
    let rect = |left, right| RECT {
        left: width - scale(left, dpi),
        top: scale(4, dpi),
        right: width - scale(right, dpi),
        bottom: scale(30, dpi),
    };
    HeaderControls {
        translation: rect(288, 192),
        toggle: rect(184, 112),
        settings: rect(104, 48),
        close: rect(40, 12),
    }
}

fn contains_point(rect: &RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

fn header_action(controls: &HeaderControls, x: i32, y: i32) -> Option<HeaderAction> {
    if contains_point(&controls.toggle, x, y) {
        Some(HeaderAction::Toggle)
    } else if contains_point(&controls.translation, x, y) {
        Some(HeaderAction::Translation)
    } else if contains_point(&controls.settings, x, y) {
        Some(HeaderAction::Settings)
    } else if contains_point(&controls.close, x, y) {
        Some(HeaderAction::Close)
    } else {
        None
    }
}

unsafe fn all_header_action(app: *mut App, width: i32, x: i32, y: i32) -> Option<HeaderAction> {
    if let Some(action) = header_action(&header_controls(width, (*app).dpi), x, y) {
        return Some(action);
    }
    if (*app).settings.connection_mode != config::ConnectionMode::Rpc {
        return None;
    }
    match tab_bar::layout(width, (*app).dpi, (*app).settings.channel_tabs.len()).hit(x, y)? {
        tab_bar::Action::Add => Some(HeaderAction::AddChannel),
        tab_bar::Action::Select(index) => {
            let tabs = &(*app).settings.channel_tabs;
            tabs.get(index).map(|tab| HeaderAction::Channel(tab.id))
        }
    }
}

unsafe fn minimum_window_height(app: *mut App, width: i32) -> i32 {
    if (*app).settings.connection_mode == config::ConnectionMode::Rpc {
        tab_bar::minimum_height(width, (*app).dpi, (*app).settings.channel_tabs.len())
    } else {
        scale(200, (*app).dpi)
    }
}

unsafe fn ensure_window_height(app: *mut App) {
    let mut rect: RECT = zeroed();
    if GetWindowRect((*app).hwnd, &mut rect) == 0 {
        return;
    }
    let width = rect.right - rect.left;
    let mut height = minimum_window_height(app, width).max(rect.bottom - rect.top);
    let mut top = rect.top;
    let mut monitor: MONITORINFO = zeroed();
    monitor.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(
        MonitorFromWindow((*app).hwnd, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    ) != 0
    {
        height = height.min(monitor.rcWork.bottom - monitor.rcWork.top);
        top = top.clamp(
            monitor.rcWork.top,
            (monitor.rcWork.bottom - height).max(monitor.rcWork.top),
        );
    }
    if height != rect.bottom - rect.top || top != rect.top {
        SetWindowPos(
            (*app).hwnd,
            null_mut(),
            rect.left,
            top,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
        );
    }
}

#[derive(Clone, Copy, PartialEq)]
enum FocusKind {
    Input,
    Game,
}

#[derive(Clone, Copy)]
struct PendingFocus {
    kind: FocusKind,
    target: HWND,
    origin: HWND,
    deadline: Instant,
    request_accepted: bool,
}

pub(super) struct App {
    settings: Settings,
    locale_warning: Option<String>,
    appearance_preview: Option<AppearancePreview>,
    client: Client,
    chat: ChatStore,
    channel_cache: ChannelCache,
    hwnd: HWND,
    background: HWND,
    input: HWND,
    input_host: HWND,
    scroll_host: HWND,
    settings_hwnd: HWND,
    settings_active: bool,
    hotkey: Option<hotkey::Hotkey>,
    input_proc: isize,
    font: HFONT,
    small_font: HFONT,
    hint_font: HFONT,
    translation_font: HFONT,
    input_brush: HBRUSH,
    input_line_height: i32,
    mode: Mode,
    dpi: u32,
    status: String,
    status_is_diagnostic: bool,
    status_is_notice: bool,
    demo: bool,
    next_row_expiration: Option<Instant>,
    last_foreground: HWND,
    allowed_game: HWND,
    return_game: HWND,
    ime_active: bool,
    ime_return: bool,
    scroll: i32,
    max_scroll: i32,
    scroll_page: i32,
    scroll_drag_offset: Option<i32>,
    scroll_action: Option<scroll_math::Action>,
    previous_content_height: i32,
    preserve_history: bool,
    failed_texts: VecDeque<String>,
    pending_sends: VecDeque<String>,
    restored_draft: bool,
    closing: bool,
    surface: Option<surface::Surface>,
    rendering: bool,
    render_error_shown: bool,
    drag: Option<surface_math::Drag>,
    header_pressed: Option<HeaderAction>,
    pending_focus: Option<PendingFocus>,
    input_return: input_return::ReturnGate,
}

struct AppearancePreview {
    settings: Settings,
    sample: ChatStore,
}

impl App {
    fn appearance(&self) -> &Settings {
        self.appearance_preview
            .as_ref()
            .map_or(&self.settings, |p| &p.settings)
    }

    fn set_status(&mut self, status: String) {
        self.status = status;
        self.status_is_diagnostic = false;
        self.status_is_notice = false;
    }

    fn set_diagnostic_status(&mut self, status: String) {
        self.status = status;
        self.status_is_diagnostic = true;
        self.status_is_notice = false;
    }

    fn set_notice_status(&mut self, status: String) {
        self.status = status;
        self.status_is_diagnostic = false;
        self.status_is_notice = true;
    }

    fn visible_status(&self) -> Option<&str> {
        ui_policy::status_visible(
            self.appearance().show_diagnostics,
            self.status_is_diagnostic,
            self.status_is_notice,
            !self.status.is_empty(),
        )
        .then_some(self.status.as_str())
        .or(self.locale_warning.as_deref())
    }
}

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub(super) fn color(rgb: u32) -> u32 {
    ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)
}
pub(super) fn uncolor(value: u32) -> u32 {
    color(value)
}
pub(super) fn scale(value: i32, dpi: u32) -> i32 {
    (value as i64 * dpi as i64 / 96) as i32
}
pub(super) unsafe fn text(hwnd: HWND) -> String {
    let length = GetWindowTextLengthW(hwnd).max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    let read = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32).max(0) as usize;
    String::from_utf16_lossy(&buffer[..read])
}
pub(super) unsafe fn set_text(hwnd: HWND, value: &str) {
    SetWindowTextW(hwnd, wide(value).as_ptr());
}

/// Catalog failures must remain understandable even when the selected file is
/// absent or corrupt. The loader has already installed the Korean fallback.
pub fn language_warning(report: &crate::i18n::LoadReport) -> Option<String> {
    use crate::i18n::LoadWarning;
    if report.warnings.is_empty() {
        return None;
    }
    let reasons = report
        .warnings
        .iter()
        .map(|warning| match warning {
            LoadWarning::MissingFile => crate::i18n::text(crate::i18n::Key::M7FB84678DA93),
            LoadWarning::Malformed => crate::i18n::text(crate::i18n::Key::M127383F81A45),
            LoadWarning::WrongLocale => crate::i18n::text(crate::i18n::Key::M8D1A9121C2DF),
            LoadWarning::TooLarge => crate::i18n::text(crate::i18n::Key::M2066F7D98D84),
            LoadWarning::InvalidEntries(count) => crate::i18n::format(
                crate::i18n::Key::M68CE7FFC4B5C,
                &[("count", format!("{}", count))],
            ),
            LoadWarning::ReadFailure => crate::i18n::text(crate::i18n::Key::MC94F3791D480),
            LoadWarning::InvalidLanguage => crate::i18n::text(crate::i18n::Key::MFA7E5267C20B),
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some(
        if report
            .warnings
            .iter()
            .all(|warning| matches!(warning, LoadWarning::InvalidEntries(_)))
        {
            crate::i18n::format(
                crate::i18n::Key::M9B54097F4214,
                &[("reasons", reasons.to_string())],
            )
        } else {
            crate::i18n::format(
                crate::i18n::Key::MFE01312E3027,
                &[("reasons", reasons.to_string())],
            )
        },
    )
}

unsafe fn refresh_language(app: *mut App) {
    let title = crate::i18n::format(
        crate::i18n::Key::MC513DC53C4AA,
        &[("arg0", env!("CARGO_PKG_VERSION").to_owned())],
    );
    set_text((*app).hwnd, &title);
    SendMessageW(
        (*app).input,
        EM_SETCUEBANNER,
        1,
        crate::i18n::wide(crate::i18n::Key::MC1B04C62AFE2).as_ptr() as isize,
    );
}

pub fn run(
    mut settings: Settings,
    initial_error: Option<String>,
    locale_warning: Option<String>,
    demo: bool,
) -> Result<(), String> {
    settings.ensure_channel_tabs()?;
    let initial_error = initial_error.or_else(|| locale_warning.clone());
    let initial_notice = initial_error.is_none();
    unsafe {
        // Per-monitor coordinates are handled by WM_DPICHANGED; no rendering thread is spun up.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let saved = RECT {
            left: settings.x,
            top: settings.y,
            right: settings.x.saturating_add(settings.width),
            bottom: settings.y.saturating_add(settings.height),
        };
        let monitor = MonitorFromRect(&saved, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info: MONITORINFO = zeroed();
        monitor_info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut monitor_info) != 0 {
            let area = monitor_info.rcWork;
            settings.width = settings.width.max(320).min(area.right - area.left);
            settings.height = settings.height.max(180).min(area.bottom - area.top);
            settings.x = settings
                .x
                .clamp(area.left, (area.right - settings.width).max(area.left));
            settings.y = settings
                .y
                .clamp(area.top, (area.bottom - settings.height).max(area.top));
        }
        let instance = GetModuleHandleW(null());
        let class = wide(CLASS_MAIN);
        let background_class = wide(CLASS_BACKGROUND);
        let cursor = LoadCursorW(null_mut(), IDC_ARROW);
        let main_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: cursor,
            hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&main_class) == 0 {
            return Err({
                let __ui_arg0 = &(GetLastError());
                crate::i18n::format(
                    crate::i18n::Key::M5CCC06741B96,
                    &[("arg0", format!("{}", __ui_arg0))],
                )
            });
        }
        let bg_class = WNDCLASSW {
            lpfnWndProc: Some(background_proc),
            hInstance: instance,
            lpszClassName: background_class.as_ptr(),
            ..zeroed()
        };
        RegisterClassW(&bg_class);
        let input_class_name = wide(CLASS_INPUT);
        let input_class = WNDCLASSW {
            lpfnWndProc: Some(input_host_proc),
            hInstance: instance,
            hCursor: cursor,
            lpszClassName: input_class_name.as_ptr(),
            ..zeroed()
        };
        RegisterClassW(&input_class);
        let scroll_class_name = wide(CLASS_SCROLL);
        let scroll_class = WNDCLASSW {
            lpfnWndProc: Some(scroll_host_proc),
            hInstance: instance,
            hCursor: cursor,
            lpszClassName: scroll_class_name.as_ptr(),
            ..zeroed()
        };
        RegisterClassW(&scroll_class);
        settings::register(instance)?;
        let mut app = Box::new(App {
            settings,
            locale_warning,
            appearance_preview: None,
            client: Client::new(),
            chat: if demo {
                ChatStore::demo()
            } else {
                ChatStore::new()
            },
            channel_cache: ChannelCache::default(),
            hwnd: null_mut(),
            background: null_mut(),
            input: null_mut(),
            input_host: null_mut(),
            scroll_host: null_mut(),
            settings_hwnd: null_mut(),
            settings_active: false,
            hotkey: None,
            input_proc: 0,
            font: null_mut(),
            small_font: null_mut(),
            hint_font: null_mut(),
            translation_font: null_mut(),
            input_brush: null_mut(),
            input_line_height: 0,
            mode: Mode::Passive,
            dpi: 96,
            status: initial_error.unwrap_or_else(|| {
                if demo {
                    crate::i18n::text(crate::i18n::Key::M1D2B96617625)
                } else {
                    crate::i18n::text(crate::i18n::Key::MC30071E7BA61)
                }
            }),
            status_is_diagnostic: false,
            status_is_notice: initial_notice,
            demo,
            next_row_expiration: None,
            last_foreground: null_mut(),
            allowed_game: null_mut(),
            return_game: null_mut(),
            ime_active: false,
            ime_return: false,
            scroll: 0,
            max_scroll: 0,
            scroll_page: 0,
            scroll_drag_offset: None,
            scroll_action: None,
            previous_content_height: 0,
            preserve_history: false,
            failed_texts: VecDeque::new(),
            pending_sends: VecDeque::new(),
            restored_draft: false,
            closing: false,
            surface: None,
            rendering: false,
            render_error_shown: false,
            drag: None,
            header_pressed: None,
            pending_focus: None,
            input_return: input_return::ReturnGate::default(),
        });
        let app_ptr = &mut *app as *mut App;
        let s = &app.settings;
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_APPWINDOW | WS_EX_TRANSPARENT,
            class.as_ptr(),
            wide(&{
                let __ui_arg0 = &(env!("CARGO_PKG_VERSION"));
                crate::i18n::format(
                    crate::i18n::Key::MC513DC53C4AA,
                    &[("arg0", __ui_arg0.to_string())],
                )
            })
            .as_ptr(),
            WS_POPUP | WS_MINIMIZEBOX | WS_SYSMENU,
            s.x,
            s.y,
            s.width.max(340),
            s.height
                .max(minimum_window_height(app_ptr, s.width.max(340))),
            null_mut(),
            null_mut(),
            instance,
            app_ptr.cast(),
        );
        if hwnd.is_null() {
            return Err({
                let __ui_arg0 = &(GetLastError());
                crate::i18n::format(
                    crate::i18n::Key::M491750537531,
                    &[("arg0", format!("{}", __ui_arg0))],
                )
            });
        }
        app.hwnd = hwnd;
        app.dpi = GetDpiForWindow(hwnd).max(96);
        let mut current_monitor: MONITORINFO = zeroed();
        current_monitor.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(
            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
            &mut current_monitor,
        ) != 0
        {
            let area = current_monitor.rcWork;
            app.settings.width = app
                .settings
                .width
                .max(scale(340, app.dpi))
                .min(area.right - area.left);
            app.settings.height = app
                .settings
                .height
                .max(minimum_window_height(app_ptr, app.settings.width))
                .min(area.bottom - area.top);
            app.settings.x = app
                .settings
                .x
                .clamp(area.left, (area.right - app.settings.width).max(area.left));
            app.settings.y = app
                .settings
                .y
                .clamp(area.top, (area.bottom - app.settings.height).max(area.top));
            SetWindowPos(
                hwnd,
                null_mut(),
                app.settings.x,
                app.settings.y,
                app.settings.width,
                app.settings.height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        app.background = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            background_class.as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            app.settings.x,
            app.settings.y,
            app.settings.width,
            app.settings.height,
            null_mut(),
            null_mut(),
            instance,
            app_ptr.cast(),
        );
        if app.background.is_null() {
            DestroyWindow(hwnd);
            return Err(crate::i18n::text(crate::i18n::Key::M223DFA67BFAC));
        }
        app.input_host = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            wide(CLASS_INPUT).as_ptr(),
            wide(&crate::i18n::text(crate::i18n::Key::M8A2825D2098A)).as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            100,
            40,
            hwnd,
            null_mut(),
            instance,
            app_ptr.cast(),
        );
        if app.input_host.is_null() {
            DestroyWindow(hwnd);
            return Err(crate::i18n::text(crate::i18n::Key::M9C0299F24054));
        }
        // A no-activate owned popup accepts only scrollbar clicks. The main
        // input/passive overlay remains WS_EX_TRANSPARENT across the game.
        app.scroll_host = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            wide(CLASS_SCROLL).as_ptr(),
            wide(&crate::i18n::text(crate::i18n::Key::ME6F657D65FFD)).as_ptr(),
            WS_POPUP,
            0,
            0,
            22,
            120,
            hwnd,
            null_mut(),
            instance,
            app_ptr.cast(),
        );
        if app.scroll_host.is_null() {
            DestroyWindow(hwnd);
            return Err(crate::i18n::text(crate::i18n::Key::M0BE6331CCC1D));
        }
        app.input = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            0,
            0,
            10,
            10,
            app.input_host,
            101usize as HMENU,
            instance,
            null(),
        );
        if app.input.is_null() {
            DestroyWindow(hwnd);
            return Err(crate::i18n::text(crate::i18n::Key::M814BA95594B2));
        }
        // The input host paints one quiet border. Disable the EDIT's themed
        // non-client decoration, while retaining native text/IME behavior.
        let no_theme = wide("");
        SetWindowTheme(app.input, no_theme.as_ptr(), no_theme.as_ptr());
        SetWindowLongPtrW(app.input, GWLP_USERDATA, app_ptr as isize);
        app.input_proc =
            SetWindowLongPtrW(app.input, GWLP_WNDPROC, edit_proc as *const () as isize);
        SendMessageW(app.input, EM_SETLIMITTEXT, 2000, 0);
        SendMessageW(
            app.input,
            EM_SETCUEBANNER,
            1,
            crate::i18n::wide(crate::i18n::Key::MC1B04C62AFE2).as_ptr() as isize,
        );
        SendMessageW(
            app.input,
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            0,
        );
        rebuild_fonts(app_ptr);
        sync_background(app_ptr);
        ShowWindow(app.background, SW_SHOWNOACTIVATE);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        position_input(app_ptr);
        if !demo {
            app.client.configure(&app.settings);
        }
        if app.settings.overlay_enabled {
            match hotkey::Hotkey::install(hwnd) {
                Ok(manager) => app.hotkey = Some(manager),
                Err(error) => app.set_status(error),
            }
        }
        SetTimer(hwnd, TIMER, 100, None);
        if demo
            || app.settings.relay_url.trim().is_empty()
            || app.settings.target_exe.trim().is_empty()
        {
            set_mode(app_ptr, Mode::Manage);
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
        let mut msg: MSG = zeroed();
        let mut loop_error = None;
        loop {
            let result = GetMessageW(&mut msg, null_mut(), 0, 0);
            if result == 0 {
                break;
            }
            if result == -1 {
                loop_error = Some({
                    let __ui_arg0 = &(GetLastError());
                    crate::i18n::format(
                        crate::i18n::Key::M8B1A6259EA2E,
                        &[("arg0", format!("{}", __ui_arg0))],
                    )
                });
                break;
            }
            if !app.settings_hwnd.is_null()
                && IsWindowVisible(app.settings_hwnd) != 0
                && IsDialogMessageW(app.settings_hwnd, &msg) != 0
            {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        app.hotkey.take();
        if !app.settings_hwnd.is_null() && IsWindow(app.settings_hwnd) != 0 {
            DestroyWindow(app.settings_hwnd);
        }
        if IsWindow(app.background) != 0 {
            DestroyWindow(app.background);
        }
        if IsWindow(app.input_host) != 0 {
            DestroyWindow(app.input_host);
        }
        if IsWindow(app.scroll_host) != 0 {
            DestroyWindow(app.scroll_host);
        }
        if IsWindow(hwnd) != 0 {
            DestroyWindow(hwnd);
        }
        for object in [
            app.font as HGDIOBJ,
            app.small_font as HGDIOBJ,
            app.hint_font as HGDIOBJ,
            app.translation_font as HGDIOBJ,
            app.input_brush as HGDIOBJ,
        ] {
            if !object.is_null() {
                DeleteObject(object);
            }
        }
        if let Some(error) = loop_error {
            Err(error)
        } else {
            Ok(())
        }
    }
}

unsafe extern "system" fn input_host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if app.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_ACTIVATE => {
            if wp & 0xffff == WA_INACTIVE as usize {
                (*app).input_return.cancel();
            }
            PostMessageW((*app).hwnd, WM_ACTIVATION_SETTLED, 0, 0);
            0
        }
        WM_KEYUP | WM_SYSKEYUP if wp == VK_RETURN as usize || wp == VK_ESCAPE as usize => {
            if (*app).input_return.pending() {
                PostMessageW((*app).hwnd, WM_COMPLETE_INPUT, 0, 0);
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_SETFOCUS => {
            if (*app).pending_focus.is_none()
                && (*app).mode == Mode::Input
                && (*app).settings.overlay_enabled
                && !(*app).input.is_null()
            {
                SetFocus((*app).input);
            }
            0
        }
        WM_SIZE => {
            layout_input_edit(app);
            0
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let dc = wp as HDC;
            SetTextColor(dc, color((*app).appearance().text_color));
            SetBkColor(dc, color((*app).appearance().background_color));
            (*app).input_brush as isize
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut r: RECT = zeroed();
            GetClientRect(hwnd, &mut r);
            FillRect(dc, &r, (*app).input_brush);
            let bg = (*app).appearance().background_color;
            let fg = (*app).appearance().text_color;
            let mix =
                |shift: u32| -> u32 { (((bg >> shift) & 255) * 3 + ((fg >> shift) & 255)) / 4 };
            let border = CreateSolidBrush(color((mix(16) << 16) | (mix(8) << 8) | mix(0)));
            FrameRect(dc, &r, border);
            DeleteObject(border as _);
            EndPaint(hwnd, &paint);
            0
        }
        WM_LBUTTONDOWN => {
            if (*app).mode == Mode::Input && (*app).pending_focus.is_none() {
                SetFocus((*app).input);
            }
            0
        }
        WM_CLOSE => {
            return_to_game(app);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn scroll_geometry(hwnd: HWND, app: *mut App) -> scroll_math::Geometry {
    let mut r: RECT = zeroed();
    GetClientRect(hwnd, &mut r);
    scroll_math::Geometry::new(
        r.bottom,
        scale(18, (*app).dpi),
        scale(24, (*app).dpi),
        (*app).scroll_page,
        (*app).max_scroll,
        (*app).scroll,
    )
}

unsafe fn scroll_to(app: *mut App, next: i32) {
    let next = next.clamp(0, (*app).max_scroll);
    if next != (*app).scroll {
        (*app).scroll = next;
        InvalidateRect((*app).hwnd, null(), 0);
        InvalidateRect((*app).scroll_host, null(), 0);
    }
}

unsafe fn scroll_action(app: *mut App, action: scroll_math::Action) {
    let step = scale(42, (*app).dpi);
    let page = ((*app).scroll_page - step).max(step);
    let delta = match action {
        scroll_math::Action::Up => step,
        scroll_math::Action::Down => -step,
        scroll_math::Action::PageUp => page,
        scroll_math::Action::PageDown => -page,
        scroll_math::Action::Thumb => 0,
    };
    scroll_to(app, (*app).scroll.saturating_add(delta));
}

fn blend_rgb(background: u32, foreground: u32, amount: u32) -> u32 {
    let amount = amount.min(255);
    let mix = |shift: u32| -> u32 {
        (((background >> shift) & 255) * (255 - amount) + ((foreground >> shift) & 255) * amount)
            / 255
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

unsafe extern "system" fn scroll_host_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if app.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as _,
        WM_NCHITTEST
            if (*app).mode != Mode::Manage
                || GetForegroundWindow() != (*app).hwnd
                || GetAsyncKeyState(VK_RETURN as i32) < 0 =>
        {
            HTTRANSPARENT as _
        }
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut bounds: RECT = zeroed();
            GetClientRect(hwnd, &mut bounds);
            let geometry = scroll_geometry(hwnd, app);
            let background = (*app).appearance().background_color;
            let foreground = (*app).appearance().text_color;
            let brush = GetStockObject(DC_BRUSH) as HBRUSH;
            SetDCBrushColor(dc, color(background));
            FillRect(dc, &bounds, brush);
            let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
            let old_brush = SelectObject(dc, brush as _);
            let inset = scale(3, (*app).dpi).max(1);
            let track = RECT {
                left: inset,
                top: geometry.track_top,
                right: bounds.right - inset,
                bottom: geometry.track_bottom,
            };
            SetDCBrushColor(dc, color(blend_rgb(background, foreground, 12)));
            FillRect(dc, &track, brush);
            if geometry.max_scroll > 0 {
                SetDCBrushColor(
                    dc,
                    color(blend_rgb(
                        background,
                        foreground,
                        if (*app).scroll_drag_offset.is_some() {
                            160
                        } else {
                            88
                        },
                    )),
                );
                RoundRect(
                    dc,
                    inset,
                    geometry.thumb_top,
                    bounds.right - inset,
                    geometry.thumb_bottom,
                    scale(6, (*app).dpi),
                    scale(6, (*app).dpi),
                );
            }
            SetDCBrushColor(
                dc,
                color(blend_rgb(
                    background,
                    foreground,
                    if geometry.max_scroll > 0 { 152 } else { 48 },
                )),
            );
            // Fixed-size triangles stay usable when the chat font is very large.
            let center = bounds.right / 2;
            let half = scale(3, (*app).dpi).max(1);
            let rise = scale(3, (*app).dpi).max(1);
            for (middle, direction) in [
                (geometry.button / 2, -1),
                ((geometry.track_bottom + geometry.height) / 2, 1),
            ] {
                let points = [
                    POINT {
                        x: center,
                        y: middle + rise * direction,
                    },
                    POINT {
                        x: center - half,
                        y: middle - rise * direction,
                    },
                    POINT {
                        x: center + half,
                        y: middle - rise * direction,
                    },
                ];
                Polygon(dc, points.as_ptr(), points.len() as i32);
            }
            SelectObject(dc, old_brush);
            SelectObject(dc, old_pen);
            EndPaint(hwnd, &paint);
            0
        }
        WM_LBUTTONDOWN
            if (*app).mode == Mode::Manage
                && !(*app).settings_active
                && GetForegroundWindow() == (*app).hwnd
                && GetAsyncKeyState(VK_RETURN as i32) >= 0 =>
        {
            let y = ((lp as u32 >> 16) & 0xffff) as i16 as i32;
            let geometry = scroll_geometry(hwnd, app);
            if let Some(action) = geometry.hit(y) {
                SetCapture(hwnd);
                if action == scroll_math::Action::Thumb {
                    (*app).scroll_drag_offset = Some(y - geometry.thumb_top);
                } else {
                    (*app).scroll_action = Some(action);
                    scroll_action(app, action);
                    SetTimer(hwnd, SCROLL_REPEAT_TIMER, 350, None);
                }
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_MOUSEMOVE if GetCapture() == hwnd => {
            if let Some(offset) = (*app).scroll_drag_offset {
                let y = ((lp as u32 >> 16) & 0xffff) as i16 as i32;
                let next = scroll_geometry(hwnd, app).scroll_for_thumb(y - offset);
                scroll_to(app, next);
            }
            0
        }
        WM_TIMER if wp == SCROLL_REPEAT_TIMER => {
            if GetCapture() == hwnd {
                if let Some(action) = (*app).scroll_action {
                    // Stop page-repeat when the thumb reaches the held point.
                    let mut point: POINT = zeroed();
                    GetCursorPos(&mut point);
                    ScreenToClient(hwnd, &mut point);
                    let mut bounds: RECT = zeroed();
                    GetClientRect(hwnd, &mut bounds);
                    if point.x >= bounds.left
                        && point.x < bounds.right
                        && scroll_geometry(hwnd, app).hit(point.y) == Some(action)
                    {
                        scroll_action(app, action);
                    }
                    SetTimer(hwnd, SCROLL_REPEAT_TIMER, 70, None);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            0
        }
        WM_CAPTURECHANGED | WM_CANCELMODE | WM_DESTROY => {
            KillTimer(hwnd, SCROLL_REPEAT_TIMER);
            (*app).scroll_drag_offset = None;
            (*app).scroll_action = None;
            if msg == WM_CANCELMODE && GetCapture() == hwnd {
                ReleaseCapture();
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xffff) as i16 as i32;
            scroll_to(app, (*app).scroll + delta * scale(42, (*app).dpi) / 120);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn background_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    if msg == WM_PAINT {
        let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
        let mut paint: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut rect: RECT = zeroed();
        GetClientRect(hwnd, &mut rect);
        let rgb = if app.is_null() {
            0x111827
        } else {
            (*app).appearance().background_color
        };
        let brush = CreateSolidBrush(color(rgb));
        FillRect(dc, &rect, brush);
        DeleteObject(brush as _);
        EndPaint(hwnd, &paint);
        return 0;
    }
    if msg == WM_MOUSEACTIVATE {
        return MA_NOACTIVATE as _;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if app.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_PAINT => {
            paint_window(hwnd, app);
            0
        }
        WM_ERASEBKGND => 1,
        WM_NCCALCSIZE => 0,
        WM_TIMER => {
            tick(app);
            0
        }
        WM_HOTKEY if wp == hotkey::HOTKEY_ID as usize => {
            let game = (*app)
                .hotkey
                .as_ref()
                .and_then(|manager| manager.take_hotkey(wp, lp));
            if let Some(game) = game {
                (*app).return_game = game;
                // Keep activation inside the real Windows WM_HOTKEY handler.
                open_input(app);
            }
            0
        }
        hotkey::WM_REFRESH_SCOPE => {
            progress_input_return(app);
            refresh_game_scope(app);
            progress_focus(app);
            0
        }
        WM_ACTIVATION_SETTLED => {
            settle_activation(app);
            0
        }
        WM_SEND_CHAT => {
            send_chat(app);
            0
        }
        WM_CANCEL_INPUT => {
            return_to_game(app);
            0
        }
        WM_COMPLETE_INPUT => {
            progress_input_return(app);
            0
        }
        WM_ACTIVATE => {
            if wp & 0xffff == WA_INACTIVE as usize {
                (*app).input_return.cancel();
            }
            PostMessageW(hwnd, WM_ACTIVATION_SETTLED, 0, 0);
            0
        }
        WM_KEYUP | WM_SYSKEYUP if wp == VK_RETURN as usize || wp == VK_ESCAPE as usize => {
            if (*app).input_return.pending() {
                PostMessageW(hwnd, WM_COMPLETE_INPUT, 0, 0);
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_KEYDOWN => {
            if (*app).input_return.pending()
                && (wp == VK_RETURN as usize || wp == VK_ESCAPE as usize)
            {
                return 0;
            }
            if wp == VK_ESCAPE as usize {
                return_to_game(app);
                return 0;
            }
            if wp == VK_RETURN as usize && (*app).mode == Mode::Manage {
                open_input(app);
                return 0;
            }
            if wp == VK_SPACE as usize && (*app).mode == Mode::Manage {
                if lp & (1 << 30) == 0 {
                    toggle_overlay(app);
                }
                return 0;
            }
            if wp == VK_TAB as usize
                && (*app).mode == Mode::Manage
                && (*app).settings.connection_mode == config::ConnectionMode::Rpc
                && GetKeyState(VK_CONTROL as i32) < 0
            {
                let tabs = &(*app).settings.channel_tabs;
                if let Some(index) = tabs
                    .iter()
                    .position(|tab| tab.id == (*app).settings.active_channel_tab)
                {
                    let step = if GetKeyState(VK_SHIFT as i32) < 0 {
                        tabs.len() - 1
                    } else {
                        1
                    };
                    let id = tabs[(index + step) % tabs.len()].id;
                    switch_channel(app, id);
                }
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CHAR if wp == VK_SPACE as usize && (*app).mode == Mode::Manage => 0,
        WM_NCHITTEST if (*app).mode == Mode::Manage => HTCLIENT as _,
        WM_SETCURSOR if (*app).mode == Mode::Manage => {
            let mut point: POINT = zeroed();
            GetCursorPos(&mut point);
            ScreenToClient(hwnd, &mut point);
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            let edges = surface_math::hit_edges(
                point.x,
                point.y,
                rect.right,
                rect.bottom,
                scale(7, (*app).dpi),
            );
            let cursor = match edges {
                value
                    if value == (surface_math::LEFT | surface_math::TOP)
                        || value == (surface_math::RIGHT | surface_math::BOTTOM) =>
                {
                    IDC_SIZENWSE
                }
                value
                    if value == (surface_math::RIGHT | surface_math::TOP)
                        || value == (surface_math::LEFT | surface_math::BOTTOM) =>
                {
                    IDC_SIZENESW
                }
                value if value & (surface_math::LEFT | surface_math::RIGHT) != 0 => IDC_SIZEWE,
                value if value & (surface_math::TOP | surface_math::BOTTOM) != 0 => IDC_SIZENS,
                _ => IDC_ARROW,
            };
            SetCursor(LoadCursorW(null_mut(), cursor));
            1
        }
        WM_LBUTTONDOWN if (*app).mode == Mode::Manage => {
            let x = (lp as u32 & 0xffff) as i16 as i32;
            let y = ((lp as u32 >> 16) & 0xffff) as i16 as i32;
            let mut rect: RECT = zeroed();
            GetWindowRect(hwnd, &mut rect);
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let edges = surface_math::hit_edges(x, y, width, height, scale(7, (*app).dpi));
            if edges == 0 {
                if let Some(action) = all_header_action(app, width, x, y) {
                    (*app).header_pressed = Some(action);
                    SetCapture(hwnd);
                    return 0;
                }
            }
            let header = y < scale(34, (*app).dpi)
                && x < header_controls(width, (*app).dpi).toggle.left - scale(8, (*app).dpi);
            if edges != 0 || header {
                let mut pointer: POINT = zeroed();
                GetCursorPos(&mut pointer);
                (*app).drag = Some(surface_math::Drag {
                    rect: surface_math::Rect {
                        left: rect.left,
                        top: rect.top,
                        right: rect.right,
                        bottom: rect.bottom,
                    },
                    pointer_x: pointer.x,
                    pointer_y: pointer.y,
                    edges,
                });
                SetCapture(hwnd);
            }
            0
        }
        WM_MOUSEMOVE if (*app).drag.is_some() => {
            if let Some(drag) = (*app).drag {
                let mut pointer: POINT = zeroed();
                GetCursorPos(&mut pointer);
                let proposed = drag.update(
                    pointer.x,
                    pointer.y,
                    scale(340, (*app).dpi),
                    scale(200, (*app).dpi),
                );
                let min_height = minimum_window_height(app, proposed.right - proposed.left);
                let mut rect =
                    drag.update(pointer.x, pointer.y, scale(340, (*app).dpi), min_height);
                // A horizontal-only resize can wrap tabs into another row too.
                if rect.bottom - rect.top < min_height {
                    rect.bottom = rect.top + min_height;
                }
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
            0
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            (*app).header_pressed = None;
            finish_drag(app);
            if msg == WM_CANCELMODE && GetCapture() == hwnd {
                ReleaseCapture();
            }
            0
        }
        WM_GETMINMAXINFO => {
            let limits = &mut *(lp as *mut MINMAXINFO);
            limits.ptMinTrackSize.x = scale(340, (*app).dpi);
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            limits.ptMinTrackSize.y =
                minimum_window_height(app, rect.right.max(limits.ptMinTrackSize.x));
            0
        }
        WM_LBUTTONUP if (*app).mode == Mode::Manage => {
            if (*app).drag.is_some() {
                finish_drag(app);
                return 0;
            }
            let x = (lp as u32 & 0xffff) as i16 as i32;
            let y = ((lp as u32 >> 16) & 0xffff) as i16 as i32;
            let mut r: RECT = zeroed();
            GetClientRect(hwnd, &mut r);
            if let Some(pressed) = (*app).header_pressed.take() {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
                if all_header_action(app, r.right, x, y) == Some(pressed) {
                    match pressed {
                        HeaderAction::Close => {
                            PostMessageW(hwnd, WM_CLOSE, 0, 0);
                        }
                        HeaderAction::Settings => settings::show(app),
                        HeaderAction::Toggle => toggle_overlay(app),
                        HeaderAction::Translation => toggle_translation(app),
                        HeaderAction::Channel(id) => switch_channel(app, id),
                        HeaderAction::AddChannel => settings::show_add_channel(app),
                    }
                }
            } else if y > r.bottom - scale(42, (*app).dpi) {
                open_input(app);
            }
            0
        }
        WM_MOUSEWHEEL if (*app).mode != Mode::Passive => {
            let delta = ((wp >> 16) & 0xffff) as i16 as i32;
            scroll_to(app, (*app).scroll + delta * scale(42, (*app).dpi) / 120);
            0
        }
        WM_WINDOWPOSCHANGED => {
            if !(*app).background.is_null() {
                sync_background(app);
                position_input(app);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_SIZE => {
            if !(*app).background.is_null() {
                if wp == SIZE_MINIMIZED as usize {
                    ShowWindow((*app).background, SW_HIDE);
                    ShowWindow((*app).scroll_host, SW_HIDE);
                } else {
                    sync_background(app);
                    position_input(app);
                }
            }
            0
        }
        WM_EXITSIZEMOVE => {
            remember_rect(app);
            let _ = config::save(&(*app).settings);
            0
        }
        WM_DPICHANGED => {
            (*app).dpi = (wp & 0xffff) as u32;
            rebuild_fonts(app);
            let r = &*(lp as *const RECT);
            let width = (r.right - r.left).max(scale(340, (*app).dpi));
            SetWindowPos(
                hwnd,
                null_mut(),
                r.left,
                r.top,
                width,
                (r.bottom - r.top).max(minimum_window_height(app, width)),
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_CTLCOLOREDIT => {
            let dc = wp as HDC;
            SetTextColor(dc, color((*app).appearance().text_color));
            SetBkColor(dc, color((*app).appearance().background_color));
            (*app).input_brush as LRESULT
        }
        WM_CLOSE => {
            (*app).closing = true;
            (*app).input_return.cancel();
            (*app).pending_focus = None;
            (*app).hotkey.take();
            remember_rect(app);
            let _ = config::save(&(*app).settings);
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            (*app).closing = true;
            (*app).input_return.cancel();
            (*app).pending_focus = None;
            (*app).hotkey.take();
            KillTimer(hwnd, TIMER);
            if !(*app).settings_hwnd.is_null() {
                DestroyWindow((*app).settings_hwnd);
            }
            if !(*app).input_host.is_null() {
                DestroyWindow((*app).input_host);
                (*app).input_host = null_mut();
            }
            if !(*app).scroll_host.is_null() {
                DestroyWindow((*app).scroll_host);
                (*app).scroll_host = null_mut();
            }
            if !(*app).background.is_null() {
                DestroyWindow((*app).background);
                (*app).background = null_mut();
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if app.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_KILLFOCUS => {
            (*app).input_return.cancel();
        }
        WM_KEYDOWN | WM_SYSKEYDOWN
            if (*app).input_return.pending()
                && (wp == VK_RETURN as usize || wp == VK_ESCAPE as usize) =>
        {
            // Keep the foreground EDIT alive for release, but never enqueue a
            // second send/cancel while its initiating gesture is still held.
            return 0;
        }
        WM_MOUSEWHEEL if (*app).mode == Mode::Input => {
            let delta = ((wp >> 16) & 0xffff) as i16 as i32;
            scroll_to(app, (*app).scroll + delta * scale(42, (*app).dpi) / 120);
            return 0;
        }
        WM_KEYDOWN
            if (*app).mode == Mode::Input
                && !(*app).ime_active
                && (wp == VK_PRIOR as usize || wp == VK_NEXT as usize) =>
        {
            scroll_action(
                app,
                if wp == VK_PRIOR as usize {
                    scroll_math::Action::PageUp
                } else {
                    scroll_math::Action::PageDown
                },
            );
            return 0;
        }
        WM_IME_STARTCOMPOSITION => {
            (*app).ime_active = true;
        }
        WM_IME_ENDCOMPOSITION => {
            (*app).ime_active = false;
            if GetAsyncKeyState(VK_RETURN as i32) < 0 {
                (*app).ime_return = true;
            }
        }
        WM_KEYUP | WM_SYSKEYUP if wp == VK_RETURN as usize || wp == VK_ESCAPE as usize => {
            if wp == VK_RETURN as usize {
                (*app).ime_return = false;
            }
            if (*app).input_return.pending() {
                // The real release has now been read from our input queue.
                // A low-level hook notification would be too early to do this.
                PostMessageW((*app).hwnd, WM_COMPLETE_INPUT, 0, 0);
                return 0;
            }
        }
        WM_KEYDOWN if wp == VK_RETURN as usize => {
            let context = ImmGetContext(hwnd);
            let composing = !context.is_null()
                && ImmGetCompositionStringW(context, GCS_COMPSTR, null_mut(), 0) > 0;
            if !context.is_null() {
                ImmReleaseContext(hwnd, context);
            }
            if !(*app).ime_active && !composing && !(*app).ime_return {
                // Enter can also open input from management mode, without going
                // through the global hotkey guard. A held opening key must not
                // auto-repeat into sending a preserved or recovered draft.
                if lp as usize & (1 << 30) == 0 {
                    PostMessageW((*app).hwnd, WM_SEND_CHAT, 0, 0);
                }
                return 0;
            }
            (*app).ime_return = true;
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            if !(*app).ime_active {
                PostMessageW((*app).hwnd, WM_CANCEL_INPUT, 0, 0);
                return 0;
            }
        }
        WM_CHAR if (wp == 13 || wp == 27) && !(*app).ime_active => return 0,
        _ => {}
    }
    let previous: WNDPROC = Some(std::mem::transmute::<
        isize,
        unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    >((*app).input_proc));
    CallWindowProcW(previous, hwnd, msg, wp, lp)
}

pub(super) unsafe fn process_path(hwnd: HWND) -> Result<String, String> {
    if hwnd.is_null() {
        return Err(crate::i18n::text(crate::i18n::Key::M665956C6F14C));
    }
    let mut process_id = 0;
    GetWindowThreadProcessId(hwnd, &mut process_id);
    if process_id == GetCurrentProcessId() {
        return Err(crate::i18n::text(crate::i18n::Key::M853BBFAB94F9));
    }
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
    if process.is_null() {
        return Err(crate::i18n::text(crate::i18n::Key::M432C215A8783));
    }
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length);
    CloseHandle(process);
    if ok == 0 {
        return Err(crate::i18n::text(crate::i18n::Key::M1625AF1654DD));
    }
    Ok(String::from_utf16_lossy(&path[..length as usize]))
}

fn matches_target(path: &str, target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    if target.contains('\\') || target.contains('/') {
        path.eq_ignore_ascii_case(target)
    } else {
        path.rsplit(['\\', '/'])
            .next()
            .unwrap_or(path)
            .eq_ignore_ascii_case(target)
    }
}

unsafe fn refresh_game_scope(app: *mut App) {
    let foreground = GetForegroundWindow();
    if foreground != (*app).last_foreground {
        (*app).last_foreground = foreground;
        (*app).allowed_game = null_mut();
        if let Ok(path) = process_path(foreground) {
            if matches_target(&path, &(*app).settings.target_exe) {
                (*app).allowed_game = foreground;
                (*app).return_game = foreground;
            }
        }
    }
    let enabled = (*app).settings.overlay_enabled
        && (*app).mode == Mode::Passive
        && !(*app).settings_active
        && (*app).settings_hwnd.is_null()
        && (*app).pending_focus.is_none()
        && !(*app).closing;
    if let Some(manager) = (*app).hotkey.as_ref() {
        if let Err(error) = manager.sync_scope(
            (*app).allowed_game,
            (*app).settings.hotkey_vk,
            (*app).settings.hotkey_modifiers,
            enabled,
        ) {
            if (*app).status != error {
                (*app).set_status(error);
                InvalidateRect((*app).hwnd, null(), 0);
            }
        }
    }
}

unsafe fn tick(app: *mut App) {
    // Existing timer is only a fallback; WM_KEYUP normally completes at once.
    progress_input_return(app);
    progress_focus(app);
    refresh_game_scope(app);
    let events = (*app).client.drain();
    let mut redraw = false;
    for event in events {
        match event {
            ClientEvent::Batch(batch) => {
                if batch.reset && !batch.connected && !(*app).pending_sends.is_empty() {
                    while let Some(text) = (*app).pending_sends.pop_front() {
                        (*app).failed_texts.push_back(text);
                    }
                    (*app).set_status(crate::i18n::text(crate::i18n::Key::MC6C30F776212));
                }
                if batch.reset {
                    (*app).scroll = 0;
                    (*app).preserve_history = false;
                } else if (*app).scroll > 0 && (*app).mode != Mode::Passive {
                    (*app).preserve_history = true;
                }
                // A disconnected reset is only a connection transition. Keep
                // this tab's cached view until the authoritative snapshot arrives.
                let active = if batch.reset && !batch.connected && batch.events.is_empty() {
                    false
                } else {
                    (*app).chat.apply(&batch)
                };
                let visible_activity = (*app).settings.translation.show_received || batch.events.iter().any(|event| matches!(event, crate::model::ChatEvent::Upsert {message} if message.translation.is_none()));
                if active && visible_activity && (*app).settings.overlay_enabled {
                    show_display(app);
                }
                redraw = true;
            }
            ClientEvent::Status(status) => {
                (*app).set_status(status);
                redraw = true;
            }
            ClientEvent::Notice(status) => {
                (*app).set_notice_status(status);
                redraw = true;
            }
            ClientEvent::TranslationResult { message, status } => {
                if (*app).scroll > 0 && (*app).mode != Mode::Passive {
                    (*app).preserve_history = true;
                }
                if (*app).chat.acknowledge(&message)
                    && (*app).settings.overlay_enabled
                    && (*app).settings.translation.show_received
                {
                    show_display(app);
                }
                (*app).set_diagnostic_status(status);
                redraw = true;
            }
            ClientEvent::SendResult {
                text,
                result,
                confirmed,
                timing,
            } => {
                if let Some(index) = (*app)
                    .pending_sends
                    .iter()
                    .position(|pending| pending == &text)
                {
                    (*app).pending_sends.remove(index);
                }
                match result {
                    Ok(()) => {
                        if let Some(message) = confirmed {
                            if (*app).scroll > 0 && (*app).mode != Mode::Passive {
                                (*app).preserve_history = true;
                            }
                            // This message is already stored by Discord. A separate
                            // insertion path cannot overwrite a newer RPC edit/delete.
                            if (*app).chat.acknowledge(&message) && (*app).settings.overlay_enabled
                            {
                                show_display(app);
                            }
                        }
                        (*app).set_diagnostic_status(
                            timing
                                .as_ref()
                                .map(|value| value.summary())
                                .unwrap_or_else(|| {
                                    crate::i18n::text(crate::i18n::Key::M223B4A2C877B)
                                }),
                        );
                    }
                    Err(error) => {
                        (*app).set_status(crate::i18n::format(
                            crate::i18n::Key::M784AC1C91D4C,
                            &[("error", error.to_string())],
                        ));
                        if !(*app).failed_texts.contains(&text) {
                            (*app).failed_texts.push_back(text);
                        }
                        if (*app).settings.overlay_enabled {
                            show_display(app);
                        }
                    }
                }
                redraw = true;
            }
        }
    }
    if (*app)
        .next_row_expiration
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        // A paint schedules the next actual row expiry. Consume this deadline
        // before invalidating so a delayed paint does not request work each tick.
        (*app).next_row_expiration = None;
        redraw = true;
    }
    if redraw && display_visible(app) {
        InvalidateRect((*app).hwnd, null(), 0);
    }
}

unsafe fn request_focus(app: *mut App, kind: FocusKind, target: HWND) {
    if (*app).closing || (kind == FocusKind::Input && !(*app).settings.overlay_enabled) {
        return;
    }
    (*app).input_return.cancel();
    let pending = PendingFocus {
        kind,
        target,
        origin: GetForegroundWindow(),
        deadline: Instant::now() + Duration::from_millis(500),
        request_accepted: false,
    };
    // Store the transition before any User32 call can reenter a window procedure.
    // The EDIT stays disabled until its host is foreground and it actually has focus.
    (*app).pending_focus = Some(pending);
    EnableWindow((*app).input, 0);
    let accepted = SetForegroundWindow(target) != 0;
    if let Some(current) = &mut (*app).pending_focus {
        if current.deadline == pending.deadline && current.target == target {
            current.request_accepted = accepted;
        }
    }
    progress_focus(app);
}

unsafe fn progress_focus(app: *mut App) {
    let Some(pending) = (*app).pending_focus else {
        return;
    };
    if (*app).closing {
        (*app).pending_focus = None;
        return;
    }
    let foreground = GetForegroundWindow();
    if foreground == pending.target {
        match pending.kind {
            FocusKind::Input => {
                EnableWindow((*app).input, 1);
                SetFocus((*app).input);
                if GetFocus() == (*app).input {
                    (*app).pending_focus = None;
                    SendMessageW((*app).input, EM_SETSEL, usize::MAX, -1);
                    return;
                }
                EnableWindow((*app).input, 0);
            }
            FocusKind::Game => {
                (*app).pending_focus = None;
                EnableWindow((*app).input, 1);
                // Hiding an active owned popup during WM_ACTIVATE can reactivate
                // its owner. Wait until the game's activation is complete first.
                set_mode(app, Mode::Passive);
                return;
            }
        }
    }
    let user_selected_another_app = !foreground.is_null()
        && foreground != pending.origin
        && foreground != pending.target
        && foreground != (*app).hwnd
        && foreground != (*app).input_host
        && foreground != (*app).settings_hwnd;
    if user_selected_another_app {
        (*app).pending_focus = None;
        EnableWindow((*app).input, 1);
        set_mode(app, Mode::Passive);
        return;
    }
    if Instant::now() < pending.deadline {
        return;
    }
    (*app).pending_focus = None;
    EnableWindow((*app).input, 1);
    let reason = if pending.request_accepted {
        crate::i18n::text(crate::i18n::Key::M221FE1EEF024)
    } else {
        crate::i18n::text(crate::i18n::Key::M691B5FF77485)
    };
    match pending.kind {
        FocusKind::Input => {
            // Failed input activation must not make an already foreground
            // management window transparent and leave its toggle unreachable.
            set_mode(
                app,
                if foreground == (*app).hwnd {
                    Mode::Manage
                } else {
                    Mode::Passive
                },
            );
            (*app).set_status(crate::i18n::format(
                crate::i18n::Key::M74E125A923B9,
                &[("reason", reason.to_string())],
            ));
        }
        FocusKind::Game => {
            set_mode(app, Mode::Manage);
            SetForegroundWindow((*app).hwnd);
            (*app).set_status(crate::i18n::format(
                crate::i18n::Key::M09DA4657D626,
                &[("reason", reason.to_string())],
            ));
        }
    }
    show_display(app);
}

unsafe fn settle_activation(app: *mut App) {
    if (*app).closing {
        return;
    }
    if GetForegroundWindow() == (*app).hwnd
        && (*app)
            .pending_focus
            .is_some_and(|pending| pending.origin != (*app).hwnd)
    {
        // Alt+Tab back to the main window is an explicit management request.
        // Reconcile after WM_ACTIVATE so an intermediate activation notification
        // cannot hide the management UI or steal focus back to the pending target.
        (*app).pending_focus = None;
        EnableWindow((*app).input, 1);
    }
    progress_focus(app);
    if (*app).pending_focus.is_some() {
        return;
    }
    // This runs from a posted message, after Windows finishes WM_ACTIVATE.
    // Never show/hide/focus other HWNDs inside the activation callback itself.
    let foreground = GetForegroundWindow();
    if foreground == (*app).hwnd {
        set_mode(app, Mode::Manage);
        show_display(app);
    } else if foreground == (*app).input_host {
        if (*app).mode == Mode::Input {
            SetFocus((*app).input);
        }
    } else if foreground != (*app).settings_hwnd {
        set_mode(app, Mode::Passive);
    }
    refresh_game_scope(app);
}

unsafe fn open_input(app: *mut App) {
    if !(*app).settings.overlay_enabled || (*app).closing {
        return;
    }
    if (*app).settings_active {
        if !(*app).settings_hwnd.is_null() {
            SetForegroundWindow((*app).settings_hwnd);
        }
        return;
    }
    (*app).input_return.cancel();
    show_display(app);
    set_mode(app, Mode::Input);
    if text((*app).input).is_empty() {
        if let Some(failed) = (*app).failed_texts.pop_front() {
            set_text((*app).input, &failed);
            (*app).restored_draft = true;
            (*app).set_status(crate::i18n::text(crate::i18n::Key::MD4196ECAFEB5));
        }
    }
    request_focus(app, FocusKind::Input, (*app).input_host);
}

unsafe fn send_chat(app: *mut App) {
    if !(*app).settings.overlay_enabled
        || (*app).input_return.pending()
        || (*app).pending_focus.is_some()
        || (*app).mode != Mode::Input
        || GetForegroundWindow() != (*app).input_host
        || GetFocus() != (*app).input
    {
        return;
    }
    let value = text((*app).input);
    if value.trim().is_empty() {
        set_text((*app).input, "");
        (*app).restored_draft = false;
        return_to_game(app);
        return;
    }
    if (*app).failed_texts.len() + (*app).pending_sends.len() >= 32 {
        (*app).set_status(crate::i18n::text(crate::i18n::Key::M1D7AF075927B));
        InvalidateRect((*app).hwnd, null(), 0);
        return;
    }
    if (*app).demo {
        (*app).set_notice_status(crate::i18n::text(crate::i18n::Key::M07D1BBA54BB7));
        set_text((*app).input, "");
        (*app).restored_draft = false;
        return_to_game(app);
        return;
    }
    match (*app).client.send(value.clone()) {
        Ok(()) => {
            (*app).pending_sends.push_back(value);
            set_text((*app).input, "");
            (*app).restored_draft = false;
            (*app).set_notice_status(crate::i18n::text(crate::i18n::Key::M2911BD06FF2A));
            (*app).scroll = 0;
            return_to_game(app);
        }
        Err(error) => {
            (*app).set_status(error);
            InvalidateRect((*app).hwnd, null(), 0);
        }
    }
}

unsafe fn return_to_game(app: *mut App) {
    if (*app).closing || (*app).settings_active {
        (*app).input_return.cancel();
        return;
    }
    let source = GetForegroundWindow();
    if source != (*app).hwnd && source != (*app).input_host {
        (*app).input_return.cancel();
        return;
    }
    // A successful send is already queued. Only the foreground handoff waits;
    // neither network completion nor a fixed delay is part of this gate.
    (*app).pending_focus = None;
    (*app).input_return.request(source as usize);
    progress_input_return(app);
}

unsafe fn progress_input_return(app: *mut App) {
    if !(*app).input_return.pending() {
        return;
    }
    let foreground = GetForegroundWindow();
    let source = if !(*app).closing
        && !(*app).settings_active
        && (*app).mode != Mode::Passive
        && (foreground == (*app).hwnd || foreground == (*app).input_host)
    {
        foreground as usize
    } else {
        0
    };
    // Posted send messages can run before the physical release's queued
    // WM_KEYUP. Require both queue and hardware state, and both exit keys,
    // so overlapping Enter/Esc or an older release cannot hand off early.
    let keys = input_return::KeyState {
        enter_queued: GetKeyState(VK_RETURN as i32) < 0,
        enter_physical: GetAsyncKeyState(VK_RETURN as i32) < 0,
        escape_queued: GetKeyState(VK_ESCAPE as i32) < 0,
        escape_physical: GetAsyncKeyState(VK_ESCAPE as i32) < 0,
    };
    if !(*app).input_return.take_ready(source, keys) {
        return;
    }
    // The gate is cleared before any reentrant User32 activation callback.
    begin_game_return(app);
}

unsafe fn begin_game_return(app: *mut App) {
    let target = (*app).return_game;
    if !target.is_null() && IsWindow(target) != 0 {
        if IsIconic(target) != 0 {
            ShowWindow(target, SW_RESTORE);
        }
        // Cross-thread activation can finish after SetForegroundWindow returns.
        // Keep the active host visible until the timer/event confirms completion.
        request_focus(app, FocusKind::Game, target);
        return;
    }
    (*app).pending_focus = None;
    EnableWindow((*app).input, 1);
    set_mode(app, Mode::Manage);
    SetForegroundWindow((*app).hwnd);
    show_display(app);
}

unsafe fn set_mode(app: *mut App, mode: Mode) {
    let mode = ui_policy::effective_mode(
        (*app).settings.overlay_enabled,
        mode,
        (*app).settings_active,
    );
    if mode != (*app).mode || mode == Mode::Passive {
        (*app).input_return.cancel();
    }
    if mode != Mode::Manage {
        (*app).header_pressed = None;
        finish_drag(app);
        if GetCapture() == (*app).hwnd {
            ReleaseCapture();
        }
    }
    (*app).next_row_expiration = None;
    (*app).mode = mode;
    if mode == Mode::Manage {
        ensure_window_height(app);
    }
    if mode == Mode::Passive {
        (*app).scroll = 0;
        (*app).preserve_history = false;
    }
    let style = GetWindowLongPtrW((*app).hwnd, GWL_EXSTYLE) as u32;
    let style = if mode != Mode::Manage {
        style | WS_EX_TRANSPARENT
    } else {
        style & !WS_EX_TRANSPARENT
    };
    SetWindowLongPtrW((*app).hwnd, GWL_EXSTYLE, style as isize);
    if !(*app).input_host.is_null() {
        ShowWindow(
            (*app).input_host,
            if mode == Mode::Input {
                SW_SHOWNOACTIVATE
            } else {
                SW_HIDE
            },
        );
    }
    position_scroll(app);
    refresh_game_scope(app);
    update_alpha(app);
}

/// Windows makes every dialog owned by a TOPMOST window topmost as well.
/// Keep the full owner stack ordinary for the whole settings session, including
/// nested/native dialogs. Publish state before User32 can reenter our procedures.
pub(super) unsafe fn settings_session(app: *mut App, active: bool) {
    if (*app).settings_active == active {
        return;
    }
    (*app).settings_active = active;
    if (*app).closing {
        return;
    }
    if active {
        (*app).pending_focus = None;
        EnableWindow((*app).input, 1);
        set_mode(app, Mode::Passive);
    }
    SetWindowPos(
        (*app).hwnd,
        if active { HWND_NOTOPMOST } else { HWND_TOPMOST },
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
    // The input host is owned by main, so it follows the owner's topmost state.
    // The background is unowned and follows main through its explicit position.
    sync_background(app);
    refresh_game_scope(app);
}

unsafe fn display_visible(app: *mut App) -> bool {
    ((*app).settings_active && (*app).appearance_preview.is_some())
        || ui_policy::display_visible((*app).settings.overlay_enabled, (*app).mode)
}

/// Only visual settings enter this temporary state. Connection credentials,
/// translation execution and persistent preferences remain authoritative.
pub(super) unsafe fn preview_settings(app: *mut App, draft: &Settings) {
    if (*app).closing || !(*app).settings_active || config::validate_appearance(draft).is_err() {
        return;
    }
    let font_changed = (*app).appearance().font_family != draft.font_family
        || (*app).appearance().font_size != draft.font_size
        || (*app).appearance().translation_font_size != draft.translation_font_size;
    let background_changed = (*app).appearance().background_color != draft.background_color;
    if let Some(preview) = &mut (*app).appearance_preview {
        preview.settings.apply_appearance_from(draft);
    } else {
        let mut settings = Settings::default();
        settings.apply_appearance_from(draft);
        (*app).appearance_preview = Some(AppearancePreview {
            settings,
            sample: ChatStore::demo(),
        });
    }
    (*app).next_row_expiration = None;
    if font_changed {
        rebuild_fonts(app);
    } else if background_changed {
        rebuild_input_brush(app);
    }
    position_input(app);
    update_alpha(app);
    InvalidateRect((*app).background, null(), 1);
    InvalidateRect((*app).scroll_host, null(), 0);
    InvalidateRect((*app).input, null(), 1);
}

pub(super) unsafe fn clear_preview(app: *mut App) {
    let Some(previous) = (*app).appearance_preview.take() else {
        return;
    };
    if (*app).closing {
        return;
    }
    let font_changed = previous.settings.font_family != (*app).settings.font_family
        || previous.settings.font_size != (*app).settings.font_size
        || previous.settings.translation_font_size != (*app).settings.translation_font_size;
    if font_changed {
        rebuild_fonts(app);
    } else if previous.settings.background_color != (*app).settings.background_color {
        rebuild_input_brush(app);
    }
    (*app).next_row_expiration = None;
    position_input(app);
    update_alpha(app);
    InvalidateRect((*app).background, null(), 1);
    InvalidateRect((*app).scroll_host, null(), 0);
    InvalidateRect((*app).input, null(), 1);
}

unsafe fn toggle_translation(app: *mut App) {
    if (*app).mode != Mode::Manage || (*app).settings_active || (*app).closing {
        return;
    }
    if (*app).demo {
        (*app).set_status(crate::i18n::text(crate::i18n::Key::M212502A9B153));
        show_display(app);
        return;
    }
    remember_rect(app);
    let mut desired = (*app).settings.clone();
    desired.translation.enabled = !desired.translation.enabled;
    if !desired.translation.enabled {
        // Stopping paid work must not depend on a writable preferences file or
        // valid start-up credentials. Keep runtime OFF even if persistence fails.
        (*app).settings.translation.enabled = false;
        let _ = (*app).settings.sync_active_tab();
        (*app).client.configure(&(*app).settings);
        match config::save(&(*app).settings) {
            Ok(()) => (*app).set_notice_status(crate::i18n::text(crate::i18n::Key::M4A1B08A2EAD3)),
            Err(error) => (*app).set_status(crate::i18n::format(
                crate::i18n::Key::MB50E8E761950,
                &[("error", error.to_string())],
            )),
        }
        show_display(app);
        return;
    }
    let connection = config::validate_translation_connection(
        desired.connection_mode,
        &desired.channel_url,
        &desired.webhook_url,
    )
    .and_then(|()| {
        if desired.discord_app_id.trim().is_empty() {
            Err(crate::i18n::text(crate::i18n::Key::M8B3581BF7451))
        } else {
            Ok(())
        }
    });
    if let Err(error) = connection {
        (*app).set_status(crate::i18n::format(
            crate::i18n::Key::M34F37F06DA75,
            &[("error", error.to_string())],
        ));
        show_display(app);
        return;
    }
    if let Err(error) = desired
        .sync_active_tab()
        .and_then(|()| config::validate(&desired))
    {
        (*app).set_status(crate::i18n::format(
            crate::i18n::Key::M822F446B24ED,
            &[("error", error.to_string())],
        ));
        show_display(app);
        return;
    }
    if let Err(error) = config::save(&desired) {
        (*app).set_status(crate::i18n::format(
            crate::i18n::Key::MC53D526CDD44,
            &[("error", error.to_string())],
        ));
        show_display(app);
        return;
    }
    (*app).settings.translation.enabled = desired.translation.enabled;
    (*app).settings.channel_tabs = desired.channel_tabs;
    (*app).client.configure(&(*app).settings);
    (*app).set_notice_status(crate::i18n::text(crate::i18n::Key::M8FB2B9E3C493));
    show_display(app);
}

unsafe fn toggle_overlay(app: *mut App) {
    if (*app).mode != Mode::Manage || (*app).closing {
        return;
    }
    (*app).input_return.cancel();
    (*app).settings.overlay_enabled = !(*app).settings.overlay_enabled;
    let hotkey_error = if (*app).settings.overlay_enabled {
        match hotkey::Hotkey::install((*app).hwnd) {
            Ok(manager) => {
                (*app).hotkey = Some(manager);
                None
            }
            Err(error) => Some(error),
        }
    } else {
        // OFF removes registration and both hooks, including any held-opener
        // repeat guard. No keyboard interception remains while it is disabled.
        (*app).hotkey.take();
        None
    };
    // An explicit management action supersedes an unfinished activation request.
    // The EDIT text and recovered drafts are preserved across OFF/ON.
    (*app).pending_focus = None;
    EnableWindow((*app).input, 1);
    set_mode(app, Mode::Manage);
    if let Some(error) = hotkey_error {
        (*app).set_status(crate::i18n::format(
            crate::i18n::Key::M422FAAAE760A,
            &[("error", error.to_string())],
        ));
    } else if (*app).settings.overlay_enabled {
        (*app).set_notice_status(crate::i18n::text(crate::i18n::Key::MB9F19B31117D));
    } else {
        (*app).set_notice_status(crate::i18n::text(crate::i18n::Key::MDADA0D3D96CB));
    }
    remember_rect(app);
    if let Err(error) = config::save(&(*app).settings) {
        (*app).set_status(crate::i18n::format(
            crate::i18n::Key::M2B849710A3E7,
            &[("error", error.to_string())],
        ));
    }
    show_display(app);
}

unsafe fn show_display(app: *mut App) {
    update_alpha(app);
    InvalidateRect((*app).hwnd, null(), 0);
}

unsafe fn update_alpha(app: *mut App) {
    if !(*app).background.is_null() {
        let alpha = if !display_visible(app) {
            0
        } else {
            ((*app).appearance().background_opacity as u32 * 255 / 100) as u8
        };
        SetLayeredWindowAttributes((*app).background, 0, alpha, LWA_ALPHA);
    }
    // ULW may synchronously send a position notification. Avoid invalidating from
    // that notification while presenting, which would otherwise create a paint loop.
    if !(*app).rendering && !(*app).hwnd.is_null() {
        InvalidateRect((*app).hwnd, null(), 0);
    }
}

unsafe fn sync_background(app: *mut App) {
    if (*app).background.is_null() || (*app).hwnd.is_null() {
        return;
    }
    let mut r: RECT = zeroed();
    GetWindowRect((*app).hwnd, &mut r);
    // An unowned tool window is placed immediately behind the content HWND.
    SetWindowPos(
        (*app).background,
        (*app).hwnd,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    update_alpha(app);
}

unsafe fn position_input(app: *mut App) {
    if (*app).input_host.is_null() {
        return;
    }
    let mut r: RECT = zeroed();
    GetWindowRect((*app).hwnd, &mut r);
    let p = scale(12, (*app).dpi);
    let height = input_height(app);
    SetWindowPos(
        (*app).input_host,
        null_mut(),
        r.left + p,
        r.bottom - scale(12, (*app).dpi) - height,
        (r.right - r.left - 2 * p).max(30),
        height,
        // Topmost follows its main owner. Geometry updates must not promote
        // the settings stack back above the browser while it is being edited.
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
    );
    layout_input_edit(app);
    position_scroll(app);
}

unsafe fn chat_bounds(app: *mut App, height: i32) -> (i32, i32) {
    let top = if (*app).mode == Mode::Manage {
        if (*app).settings.connection_mode == config::ConnectionMode::Rpc {
            let mut rect: RECT = zeroed();
            GetClientRect((*app).hwnd, &mut rect);
            tab_bar::layout(rect.right, (*app).dpi, (*app).settings.channel_tabs.len()).bottom
        } else {
            scale(34, (*app).dpi)
        }
    } else {
        0
    };
    let rows =
        i32::from((*app).mode != Mode::Passive) + i32::from((*app).visible_status().is_some());
    let bottom = height
        - if (*app).mode == Mode::Input {
            input_height(app) + scale(24 + rows * 19, (*app).dpi)
        } else {
            scale(8 + rows * 19, (*app).dpi)
        };
    (top, bottom.max(top))
}

unsafe fn position_scroll(app: *mut App) {
    let hwnd = (*app).scroll_host;
    if hwnd.is_null() {
        return;
    }
    let active =
        ui_policy::scroll_controls_visible((*app).mode, (*app).settings_active, (*app).closing)
            && IsIconic((*app).hwnd) == 0;
    if !active {
        if GetCapture() == hwnd {
            ReleaseCapture();
        }
        KillTimer(hwnd, SCROLL_REPEAT_TIMER);
        (*app).scroll_drag_offset = None;
        (*app).scroll_action = None;
        ShowWindow(hwnd, SW_HIDE);
        return;
    }
    let mut rect: RECT = zeroed();
    GetWindowRect((*app).hwnd, &mut rect);
    let (top, bottom) = chat_bounds(app, rect.bottom - rect.top);
    let gutter = scale(14, (*app).dpi);
    let alpha = (96 + u32::from((*app).appearance().background_opacity) * 128 / 100) as u8;
    SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
    SetWindowPos(
        hwnd,
        null_mut(),
        rect.right - scale(12, (*app).dpi) - gutter,
        rect.top + top + scale(8, (*app).dpi),
        gutter,
        (bottom - top - scale(8, (*app).dpi)).max(1),
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
    );
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
}

unsafe fn layout_input_edit(app: *mut App) {
    if !(*app).input.is_null() {
        let mut client: RECT = zeroed();
        GetClientRect((*app).input_host, &mut client);
        let padding = scale(12, (*app).dpi);
        let height = ((*app).input_line_height + scale(2, (*app).dpi))
            .max(1)
            .min(client.bottom.max(1));
        SetWindowPos(
            (*app).input,
            null_mut(),
            padding,
            ((client.bottom - height) / 2).max(0),
            (client.right - padding * 2).max(10),
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        InvalidateRect((*app).input_host, null(), 0);
    }
}

unsafe fn input_height(app: *mut App) -> i32 {
    (*app).input_line_height.max(1) + scale(22, (*app).dpi)
}

unsafe fn finish_drag(app: *mut App) {
    if (*app).drag.take().is_some() {
        if GetCapture() == (*app).hwnd {
            ReleaseCapture();
        }
        if !(*app).closing {
            remember_rect(app);
            if let Err(error) = config::save(&(*app).settings) {
                (*app).set_status(error);
            }
        }
    }
}

unsafe fn remember_rect(app: *mut App) {
    if IsIconic((*app).hwnd) != 0 {
        return;
    }
    let mut r: RECT = zeroed();
    GetWindowRect((*app).hwnd, &mut r);
    (*app).settings.x = r.left;
    (*app).settings.y = r.top;
    (*app).settings.width = r.right - r.left;
    (*app).settings.height = r.bottom - r.top;
}

unsafe fn font(family: &str, points: u32, dpi: u32, weight: i32) -> HFONT {
    // Zero escapement/orientation alone does not neutralize an @ CJK face.
    let family = config::horizontal_font_family(family);
    CreateFontW(
        -(((points * dpi) / 72) as i32),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        ANTIALIASED_QUALITY as u32,
        DEFAULT_PITCH as u32 | FF_DONTCARE as u32,
        wide(family).as_ptr(),
    )
}

unsafe fn rebuild_fonts(app: *mut App) {
    for object in [
        (*app).font,
        (*app).small_font,
        (*app).hint_font,
        (*app).translation_font,
    ] {
        if !object.is_null() {
            DeleteObject(object as _);
        }
    }
    (*app).font = font(
        &(*app).appearance().font_family,
        (*app).appearance().font_size,
        (*app).dpi,
        FW_NORMAL as i32,
    );
    (*app).small_font = font("Segoe UI", 9, (*app).dpi, FW_NORMAL as i32);
    // The empty-channel instructions are UI copy. Use the same horizontal
    // UI family as the working header/footer, at the user's readable size.
    (*app).hint_font = font(
        "Segoe UI",
        (*app).appearance().font_size,
        (*app).dpi,
        FW_NORMAL as i32,
    );
    (*app).translation_font = font(
        &(*app).appearance().font_family,
        (*app).appearance().translation_font_size,
        (*app).dpi,
        FW_NORMAL as i32,
    );
    rebuild_input_brush(app);
    if !(*app).input.is_null() {
        SendMessageW((*app).input, WM_SETFONT, (*app).font as usize, 1);
        let dc = GetDC((*app).input);
        let previous = SelectObject(dc, (*app).font as _);
        let mut metrics: TEXTMETRICW = zeroed();
        (*app).input_line_height = if GetTextMetricsW(dc, &mut metrics) != 0 {
            metrics.tmHeight
        } else {
            ((*app).appearance().font_size * (*app).dpi / 72) as i32
        };
        SelectObject(dc, previous);
        ReleaseDC((*app).input, dc);
    }
}

unsafe fn rebuild_input_brush(app: *mut App) {
    if !(*app).input_brush.is_null() {
        DeleteObject((*app).input_brush as _);
    }
    (*app).input_brush = CreateSolidBrush(color((*app).appearance().background_color));
}

unsafe fn draw_text(dc: HDC, mask: HDC, value: &str, rect: &mut RECT, flags: u32) -> i32 {
    let value = wide(value);
    draw_wide_text(dc, mask, &value, rect, flags)
}

unsafe fn draw_key_text(
    dc: HDC,
    mask: HDC,
    key: crate::i18n::Key,
    rect: &mut RECT,
    flags: u32,
) -> i32 {
    // Prepared text is reused across paints; locale files are never consulted
    // by the renderer and fixed UI labels need no per-paint UTF-16 conversion.
    draw_wide_text(dc, mask, &crate::i18n::wide(key), rect, flags)
}

unsafe fn draw_wide_text(dc: HDC, mask: HDC, value: &[u16], rect: &mut RECT, flags: u32) -> i32 {
    let mut mask_rect = *rect;
    let result = DrawTextW(
        dc,
        value.as_ptr(),
        value.len() as i32 - 1,
        rect,
        flags | DT_NOPREFIX,
    );
    if flags & DT_CALCRECT == 0 {
        SelectObject(mask, GetCurrentObject(dc, OBJ_FONT as u32));
        SetTextColor(mask, 0x00ffffff);
        DrawTextW(
            mask,
            value.as_ptr(),
            value.len() as i32 - 1,
            &mut mask_rect,
            flags | DT_NOPREFIX,
        );
    }
    result
}

unsafe fn report_render_error(app: *mut App, error: String) {
    (*app).set_status(error.clone());
    if !(*app).render_error_shown {
        (*app).render_error_shown = true;
        MessageBoxW(
            (*app).hwnd,
            wide(&error).as_ptr(),
            wide(&crate::i18n::text(crate::i18n::Key::M81A908547A98)).as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

unsafe fn draw_toggle(dc: HDC, mask: HDC, rect: &RECT, dpi: u32, enabled: bool, label: &[u16]) {
    let left = rect.left + scale(3, dpi);
    let top = rect.top + scale(6, dpi);
    let width = scale(28, dpi);
    let height = scale(14, dpi);
    let knob_left = left + scale(if enabled { 15 } else { 2 }, dpi);
    for (target, track) in [
        (dc, color(if enabled { 0x429d83 } else { 0x566575 })),
        (mask, 0x00ffffff),
    ] {
        SaveDC(target);
        SelectObject(target, GetStockObject(NULL_PEN));
        SelectObject(target, GetStockObject(DC_BRUSH));
        SetDCBrushColor(target, track);
        RoundRect(
            target,
            left,
            top,
            left + width,
            top + height,
            height,
            height,
        );
        SetDCBrushColor(target, 0x00ffffff);
        Ellipse(
            target,
            knob_left,
            top + scale(2, dpi),
            knob_left + scale(10, dpi),
            top + scale(12, dpi),
        );
        RestoreDC(target, -1);
    }
    let mut label_rect = RECT {
        left: rect.left + scale(36, dpi),
        ..*rect
    };
    draw_wide_text(dc, mask, label, &mut label_rect, DT_SINGLELINE | DT_VCENTER);
}

unsafe fn paint_window(hwnd: HWND, app: *mut App) {
    let mut ps: PAINTSTRUCT = zeroed();
    BeginPaint(hwnd, &mut ps);
    if (*app).rendering {
        EndPaint(hwnd, &ps);
        return;
    }
    let mut client: RECT = zeroed();
    GetClientRect(hwnd, &mut client);
    let width = client.right;
    let height = client.bottom;
    if width <= 0 || height <= 0 {
        EndPaint(hwnd, &ps);
        return;
    }
    (*app).rendering = true;
    // Take ownership out of App before Win32 calls can reenter the window procedure.
    let mut layer = match (*app)
        .surface
        .take()
        .filter(|s| s.width == width && s.height == height)
    {
        Some(surface) => surface,
        None => match surface::Surface::new(width, height) {
            Ok(surface) => surface,
            Err(error) => {
                report_render_error(app, error);
                (*app).rendering = false;
                EndPaint(hwnd, &ps);
                return;
            }
        },
    };
    // Management has an opaque background so all drag/resize areas hit-test.
    // Passive/input modes draw only smooth glyphs; the background has its own alpha.
    layer.clear(if (*app).mode == Mode::Manage {
        Some((*app).appearance().background_color)
    } else {
        None
    });
    let dc = layer.dc();
    let mask = layer.mask_dc();
    let dpi = (*app).dpi;
    let p = scale(16, dpi);
    let interactive = (*app).mode != Mode::Passive;
    let previewing = (*app).settings_active && (*app).appearance_preview.is_some();
    // Game display/input start directly with messages. The compact management
    // strip exists only after Alt+Tab and carries actual controls, not branding.
    let (header_height, bottom) = chat_bounds(app, height);
    let content_top = header_height + scale(10, dpi);
    let text_right = width
        - if (*app).mode == Mode::Manage {
            scale(34, dpi)
        } else {
            p
        };
    if (*app).mode == Mode::Manage {
        SelectObject(dc, (*app).small_font as _);
        SetTextColor(dc, color(0x9aaabd));
        let mut title = RECT {
            left: p,
            top: scale(4, dpi),
            right: header_controls(width, dpi).translation.left - scale(8, dpi),
            bottom: scale(30, dpi),
        };
        if title.right - title.left >= scale(72, dpi) {
            draw_key_text(
                dc,
                mask,
                crate::i18n::Key::ME626A05AEAF0,
                &mut title,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
        SetTextColor(dc, color((*app).appearance().text_color));
        let controls = header_controls(width, dpi);
        SetTextColor(
            dc,
            color(if (*app).settings.translation.enabled {
                (*app).appearance().author_color
            } else {
                0x9aaabd
            }),
        );
        draw_toggle(
            dc,
            mask,
            &controls.translation,
            dpi,
            (*app).settings.translation.enabled,
            &crate::i18n::wide(if (*app).settings.translation.enabled {
                crate::i18n::Key::MD03C8DBE194D
            } else {
                crate::i18n::Key::M635FFA3E73F5
            }),
        );
        SetTextColor(dc, color((*app).appearance().text_color));
        draw_toggle(
            dc,
            mask,
            &controls.toggle,
            dpi,
            (*app).settings.overlay_enabled,
            &wide(if (*app).settings.overlay_enabled {
                "ON"
            } else {
                "OFF"
            }),
        );
        let mut settings_rect = controls.settings;
        draw_key_text(
            dc,
            mask,
            crate::i18n::Key::MB04B1CEE4046,
            &mut settings_rect,
            DT_SINGLELINE | DT_CENTER | DT_VCENTER,
        );
        let mut exit = controls.close;
        draw_text(
            dc,
            mask,
            "×",
            &mut exit,
            DT_SINGLELINE | DT_CENTER | DT_VCENTER,
        );
        if (*app).settings.connection_mode == config::ConnectionMode::Rpc {
            let tabs = &(*app).settings.channel_tabs;
            for cell in tab_bar::layout(width, dpi, tabs.len()).cells {
                let (label, selected) = match cell.action {
                    tab_bar::Action::Select(index) => {
                        let tab = &tabs[index];
                        (
                            tab.name.as_str(),
                            tab.id == (*app).settings.active_channel_tab,
                        )
                    }
                    tab_bar::Action::Add => ("+", false),
                };
                let background = blend_rgb(
                    (*app).appearance().background_color,
                    (*app).appearance().author_color,
                    if selected { 72 } else { 18 },
                );
                for (target, fill) in [(dc, color(background)), (mask, 0x00ffffff)] {
                    SaveDC(target);
                    SelectObject(target, GetStockObject(NULL_PEN));
                    SelectObject(target, GetStockObject(DC_BRUSH));
                    SetDCBrushColor(target, fill);
                    RoundRect(
                        target,
                        cell.left,
                        cell.top,
                        cell.right,
                        cell.bottom,
                        scale(7, dpi),
                        scale(7, dpi),
                    );
                    RestoreDC(target, -1);
                }
                SetTextColor(
                    dc,
                    color(if selected {
                        (*app).appearance().author_color
                    } else {
                        (*app).appearance().text_color
                    }),
                );
                let mut rect = RECT {
                    left: cell.left + scale(6, dpi),
                    top: cell.top,
                    right: cell.right - scale(6, dpi),
                    bottom: cell.bottom,
                };
                draw_text(
                    dc,
                    mask,
                    label,
                    &mut rect,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
            }
        }
    }
    let (mut rows, expiration) = if (*app).settings.overlay_enabled || previewing {
        let max_age =
            if (*app).mode == Mode::Passive && !previewing && (*app).appearance().hide_when_idle {
                Some(Duration::from_secs(
                    (*app).appearance().hide_after_secs as u64,
                ))
            } else {
                None
            };
        (*app).chat.rows_visible_with_deadline(
            (*app).settings.translation.show_received,
            Instant::now(),
            max_age,
        )
    } else {
        (Vec::new(), None)
    };
    if rows.is_empty() && previewing {
        if let Some(preview) = &(*app).appearance_preview {
            rows = preview.sample.rows();
        }
    }
    (*app).next_row_expiration = expiration;
    let line_gap = scale((*app).appearance().author_gap as i32, dpi);
    let row_gap = scale((*app).appearance().message_spacing as i32, dpi);
    let text_style = chat_text::TextStyle {
        letter_spacing_px: scale((*app).appearance().letter_spacing, dpi),
        line_gap_px: scale((*app).appearance().line_spacing as i32, dpi),
        outline: if (*app).appearance().text_outline {
            Some(chat_text::Outline {
                color: color((*app).appearance().outline_color),
                radius_px: scale((*app).appearance().outline_width as i32, dpi).max(1),
            })
        } else {
            None
        },
    };
    let mut layouts = Vec::with_capacity(rows.len());
    let message_time = crate::message_time::LocalTimeFormatter::current();
    let mut total = 0;
    // Each layout includes stroke padding above and below. Remove that double
    // padding between blocks so equal-size text keeps the configured line gap.
    let body_block_gap = text_style.line_gap_px.clamp(0, 256)
        - 2 * text_style
            .outline
            .map_or(0, |outline| outline.radius_px.clamp(0, 8));
    for row in rows {
        let mut body_blocks = Vec::with_capacity(row.translations.len() + 1);
        SelectObject(dc, (*app).font as _);
        if (*app).appearance().translation_style_enabled {
            body_blocks.push((
                chat_text::layout(dc, &row.original, (text_right - p).max(1), text_style),
                (*app).appearance().text_color,
            ));
            for (index, reply) in row.translations.iter().enumerate() {
                let translated = row.translated_replies.contains(&index);
                SelectObject(
                    dc,
                    if translated {
                        (*app).translation_font
                    } else {
                        (*app).font
                    } as _,
                );
                let reply_color = if translated {
                    (*app).appearance().translation_text_color
                } else {
                    (*app).appearance().text_color
                };
                body_blocks.push((
                    chat_text::layout(
                        dc,
                        &format!("↳ {reply}"),
                        (text_right - p).max(1),
                        text_style,
                    ),
                    reply_color,
                ));
            }
        } else {
            let mut body = row.original;
            for reply in &row.translations {
                body.push_str("\n↳ ");
                body.push_str(reply);
            }
            body_blocks.push((
                chat_text::layout(dc, &body, (text_right - p).max(1), text_style),
                (*app).appearance().text_color,
            ));
        }
        let body_height = body_blocks
            .iter()
            .map(|(layout, _)| layout.height())
            .sum::<i32>()
            + body_block_gap * (body_blocks.len() as i32 - 1);
        SelectObject(dc, (*app).small_font as _);
        let stamp = message_time.format(&row.timestamp);
        let author = if stamp.is_empty() {
            row.author
        } else {
            format!("{}   {}", row.author, stamp)
        };
        let author_layout =
            chat_text::single_line(dc, &author, (text_right - p).max(1), text_style);
        let row_h = author_layout.height() + line_gap + body_height + row_gap;
        total += row_h;
        layouts.push((author_layout, body_blocks, row_h));
    }
    let previous_scroll = ((*app).max_scroll, (*app).scroll_page, (*app).scroll);
    let available = (bottom - content_top).max(0);
    (*app).scroll_page = available;
    (*app).max_scroll = (total - available).max(0);
    if (*app).preserve_history && (*app).scroll > 0 {
        (*app).scroll += total - (*app).previous_content_height;
    }
    (*app).preserve_history = false;
    (*app).previous_content_height = total;
    (*app).scroll = (*app).scroll.clamp(0, (*app).max_scroll);
    if previous_scroll.1 != (*app).scroll_page {
        // A hidden diagnostic can reclaim a footer line without a WM_SIZE.
        // Reposition only when the page changes, avoiding a paint/position loop.
        position_scroll(app);
    }
    if previous_scroll != ((*app).max_scroll, (*app).scroll_page, (*app).scroll) {
        InvalidateRect((*app).scroll_host, null(), 0);
    }
    let mut y = scroll_math::content_origin(bottom, total, (*app).scroll);
    SaveDC(dc);
    SaveDC(mask);
    IntersectClipRect(dc, 0, header_height, text_right, bottom);
    IntersectClipRect(mask, 0, header_height, text_right, bottom);
    if layouts.is_empty() && interactive {
        SelectObject(
            dc,
            if (*app).settings.overlay_enabled {
                (*app).hint_font
            } else {
                (*app).small_font
            } as _,
        );
        SetTextColor(dc, color((*app).appearance().text_color));
        let mut r = RECT {
            left: p,
            top: header_height + scale(23, dpi),
            right: text_right,
            bottom,
        };
        draw_key_text(
            dc,
            mask,
            if (*app).settings.overlay_enabled {
                crate::i18n::Key::M04323CC72732
            } else {
                crate::i18n::Key::M11EC7AD997E2
            },
            &mut r,
            DT_WORDBREAK,
        );
    }
    let chat_clip = RECT {
        left: 0,
        top: header_height,
        right: text_right,
        bottom,
    };
    for (author_layout, body_blocks, row_h) in layouts {
        if y + row_h >= header_height && y < bottom {
            SetTextColor(dc, color((*app).appearance().author_color));
            chat_text::draw(&mut layer, &author_layout, p, y, &chat_clip);
            let mut body_y = y + author_layout.height() + line_gap;
            for (body_layout, text_color) in body_blocks {
                SetTextColor(dc, color(text_color));
                chat_text::draw(&mut layer, &body_layout, p, body_y, &chat_clip);
                body_y += body_layout.height() + body_block_gap;
            }
        }
        y += row_h;
    }
    RestoreDC(dc, -1);
    RestoreDC(mask, -1);
    SelectObject(dc, (*app).small_font as _);
    SetTextColor(dc, color(0x9aaabd));
    let mut r = RECT {
        left: p,
        top: bottom + scale(5, dpi),
        right: width - p,
        bottom: height - scale(5, dpi),
    };
    let scope_key = if (*app).mode == Mode::Manage && !(*app).settings.overlay_enabled {
        Some(crate::i18n::Key::M2B2C4A41E689)
    } else if (*app).mode == Mode::Input && (*app).restored_draft {
        Some(crate::i18n::Key::M67035067ACC3)
    } else if (*app).mode == Mode::Input {
        if (*app).settings.connection_mode == config::ConnectionMode::Rpc {
            None
        } else {
            Some(crate::i18n::Key::M0DCC0A7C7468)
        }
    } else if (*app).mode == Mode::Manage {
        Some(crate::i18n::Key::MBD90889046FA)
    } else if (*app).scroll > 0 {
        Some(crate::i18n::Key::M043DF5A80485)
    } else {
        None
    };
    if let Some(key) = scope_key {
        draw_key_text(dc, mask, key, &mut r, DT_SINGLELINE | DT_END_ELLIPSIS);
    } else if (*app).mode == Mode::Input {
        let channel = (*app).settings.active_tab().map_or_else(
            || crate::i18n::text(crate::i18n::Key::MC00FB6F89710),
            |tab| tab.name.clone(),
        );
        let scope = crate::i18n::format(crate::i18n::Key::MAB4C7A1A4931, &[("arg0", channel)]);
        draw_text(dc, mask, &scope, &mut r, DT_SINGLELINE | DT_END_ELLIPSIS);
    } else {
        draw_text(
            dc,
            mask,
            (*app).visible_status().unwrap_or(""),
            &mut r,
            DT_SINGLELINE | DT_END_ELLIPSIS,
        );
    }
    if (*app).mode != Mode::Passive && (*app).visible_status().is_some() {
        let mut r = RECT {
            left: p,
            top: bottom + scale(24, dpi),
            right: width - p,
            bottom: height,
        };
        draw_text(
            dc,
            mask,
            (*app).visible_status().unwrap_or(""),
            &mut r,
            DT_SINGLELINE | DT_END_ELLIPSIS,
        );
    }
    let result = layer.present(hwnd, if display_visible(app) { 255 } else { 0 });
    (*app).surface = Some(layer);
    if let Err(error) = result {
        report_render_error(app, error);
    }
    (*app).rendering = false;
    EndPaint(hwnd, &ps);
}

unsafe fn switch_channel(app: *mut App, id: u64) {
    if (*app).mode != Mode::Manage
        || (*app).settings_active
        || (*app).closing
        || id == (*app).settings.active_channel_tab
    {
        return;
    }
    let mut desired = (*app).settings.clone();
    let result = desired
        .select_channel_tab(id)
        .and_then(|()| commit_settings(app, desired));
    if let Err(error) = result {
        (*app).set_status(error);
    } else {
        let name = (*app).settings.active_tab().map_or_else(
            || crate::i18n::text(crate::i18n::Key::MC00FB6F89710),
            |tab| tab.name.clone(),
        );
        (*app).set_notice_status(crate::i18n::format(
            crate::i18n::Key::MD20E23E57283,
            &[("name", name.to_string())],
        ));
    }
    show_display(app);
}

pub(super) unsafe fn clear_channel_history(app: *mut App) {
    // Explicit logout clears received messages, not the user's unsent drafts.
    (*app).chat.clear();
    (*app).channel_cache.clear_history();
    (*app).scroll = 0;
    (*app).max_scroll = 0;
    (*app).previous_content_height = 0;
    (*app).preserve_history = false;
    (*app).next_row_expiration = None;
    position_scroll(app);
    show_display(app);
}

pub(super) unsafe fn commit_settings(app: *mut App, mut settings: Settings) -> Result<(), String> {
    remember_rect(app);
    settings.x = (*app).settings.x;
    settings.y = (*app).settings.y;
    settings.width = (*app).settings.width;
    settings.height = (*app).settings.height;
    // The settings dialog is modeless; its earlier draft must not undo a later
    // header toggle when the user returns to it and presses Save.
    settings.overlay_enabled = (*app).settings.overlay_enabled;
    settings.sync_active_tab()?;
    config::validate(&settings)?;
    let language_changed = settings.ui_language != (*app).settings.ui_language;
    // A language choice must not erase a pending connection/send error. Old
    // event messages retain their event language until the next event arrives.
    let replace_language_notice = language_changed
        && ((*app).status_is_notice
            || (*app).locale_warning.as_deref() == Some((*app).status.as_str()));
    let different_channel = settings.channel_url != (*app).settings.channel_url
        || settings.connection_mode != (*app).settings.connection_mode
        || settings.discord_app_id != (*app).settings.discord_app_id
        || settings.webhook_url != (*app).settings.webhook_url
        || settings.relay_url != (*app).settings.relay_url
        || settings.access_key != (*app).settings.access_key;
    let switching =
        different_channel || settings.active_channel_tab != (*app).settings.active_channel_tab;
    if switching && !(*app).pending_sends.is_empty() {
        return Err(crate::i18n::text(crate::i18n::Key::MF6DC7C8E8110));
    }
    let active_preserved = if settings.connection_mode == config::ConnectionMode::Rpc {
        settings.connection_mode == (*app).settings.connection_mode
            && settings.discord_app_id == (*app).settings.discord_app_id
            && settings.channel_tabs.iter().any(|tab| {
                tab.id == (*app).settings.active_channel_tab
                    && tab.channel_url == (*app).settings.channel_url
                    && tab.webhook_url == (*app).settings.webhook_url
            })
    } else {
        !switching
    };
    if (!active_preserved && (!text((*app).input).is_empty() || !(*app).failed_texts.is_empty()))
        || (*app).channel_cache.has_unsent_for_invalidated(&settings)
    {
        return Err(crate::i18n::text(crate::i18n::Key::MA3CA8449E896));
    }
    config::save(&settings)?;
    if switching {
        let state = LocalChannelState {
            chat: std::mem::take(&mut (*app).chat),
            draft: text((*app).input),
            failed_texts: std::mem::take(&mut (*app).failed_texts),
            restored_draft: (*app).restored_draft,
        };
        (*app).channel_cache.stash(&(*app).settings, state);
    }
    (*app).settings = settings;
    if language_changed {
        let report = crate::i18n::load_language(&(*app).settings.ui_language);
        (*app).locale_warning = language_warning(&report);
        refresh_language(app);
    }
    (*app).channel_cache.reconcile(&(*app).settings);
    (*app).allowed_game = null_mut();
    (*app).last_foreground = null_mut();
    if switching {
        let state = (*app)
            .channel_cache
            .take(&(*app).settings)
            .unwrap_or_else(|| LocalChannelState {
                chat: if (*app).demo {
                    ChatStore::demo()
                } else {
                    ChatStore::new()
                },
                ..LocalChannelState::default()
            });
        (*app).chat = state.chat;
        (*app).failed_texts = state.failed_texts;
        (*app).restored_draft = state.restored_draft;
        set_text((*app).input, &state.draft);
        (*app).scroll = 0;
        (*app).max_scroll = 0;
        (*app).previous_content_height = 0;
        (*app).preserve_history = false;
        (*app).next_row_expiration = None;
        (*app).set_notice_status(crate::i18n::text(crate::i18n::Key::MCE3285054301));
    }
    if !(*app).demo {
        (*app).client.configure(&(*app).settings);
    }
    if replace_language_notice {
        (*app).set_notice_status(crate::i18n::text(crate::i18n::Key::M70933366D5F6));
    }
    rebuild_fonts(app);
    ensure_window_height(app);
    position_input(app);
    show_display(app);
    InvalidateRect((*app).background, null(), 1);
    Ok(())
}
