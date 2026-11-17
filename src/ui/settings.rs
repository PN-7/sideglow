// SPDX-License-Identifier: MPL-2.0
//! Native settings window. Visual drafts preview without persistence; connection,
//! API and input changes remain unapplied until Save succeeds.
mod connection_edit;
mod connection_setup;
mod running_apps;
mod share_code;
mod translation;

use super::*;
use crate::config::ConnectionMode;
use windows_sys::Win32::System::SystemServices::{SS_NOPREFIX, SS_PATHELLIPSIS};
use windows_sys::Win32::UI::Controls::Dialogs::*;

const CLASS: &str = "Sideglow.Settings.v1";
const RELAY: i32 = 201;
const CHANNEL: i32 = 202;
const KEY: i32 = 203;
const TARGET: i32 = 204;
const REFRESH_APPS: i32 = 205;
const HOTKEY: i32 = 206;
const CTRL: i32 = 207;
const ALT: i32 = 208;
const SHIFT: i32 = 209;
const WIN: i32 = 210;
const FONT: i32 = 211;
const FONT_LABEL: i32 = 212;
const FOREGROUND: i32 = 213;
const BACKGROUND: i32 = 214;
const OPACITY: i32 = 215;
const AUTOHIDE: i32 = 216;
const SECONDS: i32 = 217;
const STATUS: i32 = 218;
const SAVE: i32 = 219;
const CANCEL: i32 = 220;
const TARGET_PATH: i32 = 221;
const CONNECTION_MODE: i32 = 222;
const CONNECT_DISCORD: i32 = 225;
const LOGOUT_DISCORD: i32 = 226;
const RELAY_LABEL: i32 = 229;
const KEY_LABEL: i32 = 230;
const CONNECTION_HELP: i32 = 231;
const CONNECTION_STATUS: i32 = 232;
const CHANNEL_LABEL: i32 = 233;
const SEND_TIMING: i32 = 234;
const TRANSLATION_SETTINGS: i32 = 235;
const TRANSLATION_SUMMARY: i32 = 236;
const TRANSLATION_TIMING: i32 = 237;
const LETTER_SPACING: i32 = 238;
const LINE_SPACING: i32 = 239;
const MESSAGE_SPACING: i32 = 240;
const TEXT_OUTLINE: i32 = 241;
const OUTLINE_COLOR: i32 = 242;
const OUTLINE_WIDTH: i32 = 243;
const AUTHOR_COLOR: i32 = 244;
const AUTHOR_GAP: i32 = 245;
const SHOW_DIAGNOSTICS: i32 = 246;
const EDIT_CONNECTION: i32 = 247;
const CHANNEL_TABS_LABEL: i32 = 248;
const CHANNEL_TABS: i32 = 249;
const ADD_CHANNEL_TAB: i32 = 250;
const EDIT_CHANNEL_TAB: i32 = 251;
const REMOVE_CHANNEL_TAB: i32 = 252;
const TRANSLATION_STYLE: i32 = 253;
const TRANSLATION_SIZE: i32 = 254;
const TRANSLATION_COLOR: i32 = 255;
const UI_LANGUAGE: i32 = 256;
const UI_LANGUAGE_LABEL: i32 = 257;
const UI_LANGUAGE_HELP: i32 = 258;
const SHARE_CONNECTION: i32 = 259;
const ROMAJI_JAPANESE_INPUT: i32 = 260;
const SETTINGS_TIMER: usize = 2;
const PREVIEW_TIMER: usize = 3;
const CONTENT_WIDTH: i32 = 650;
const LANGUAGE_AREA_HEIGHT: i32 = 84;
const ROMAJI_AREA_HEIGHT: i32 = 72;
const CONTENT_HEIGHT: i32 = 1133 + LANGUAGE_AREA_HEIGHT + ROMAJI_AREA_HEIGHT;
const RPC_COMPACT_BY: i32 = 0;
const FIRST_GAME_CONTROL_Y: i32 = 352;
fn demo_connection_notice() -> String {
    crate::i18n::text(crate::i18n::Key::M0DC262DEDDFA)
}

#[derive(Clone, Copy)]
struct ControlLayout {
    hwnd: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    heading: bool,
}

struct TargetChoice {
    label: String,
    path: String,
    running: bool,
}

struct Dialog {
    app: *mut App,
    draft: Settings,
    hwnd: HWND,
    dpi: u32,
    font: HFONT,
    heading_font: HFONT,
    brush: HBRUSH,
    edit_brush: HBRUSH,
    custom_colors: [u32; 16],
    languages: Vec<crate::i18n::LanguageChoice>,
    keys: Vec<(String, u32)>,
    targets: Vec<TargetChoice>,
    filling_targets: bool,
    filling_tabs: bool,
    target_requires_running: bool,
    created: bool,
    scroll_x: i32,
    scroll_y: i32,
    controls: Vec<ControlLayout>,
    last_connection_status: String,
    watch_connection_status: bool,
    setup_active: bool,
}

// Settings are an opaque native form. Their font and colors must not inherit
// the transparent overlay's color-key rendering restrictions. Use the system
// message font and colors so buttons, check boxes and edit controls agree with
// the surface they are drawn on, including Windows high-contrast settings.
unsafe fn settings_font(dpi: u32, heading: bool) -> HFONT {
    let mut metrics: NONCLIENTMETRICSW = zeroed();
    metrics.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
    if SystemParametersInfoForDpi(
        SPI_GETNONCLIENTMETRICS,
        metrics.cbSize,
        (&mut metrics as *mut NONCLIENTMETRICSW).cast(),
        0,
        dpi,
    ) != 0
    {
        // Honor Windows font smoothing instead of forcing aliased glyphs.
        metrics.lfMessageFont.lfQuality = DEFAULT_QUALITY;
        if heading {
            metrics.lfMessageFont.lfWeight = FW_SEMIBOLD as i32;
        }
        let result = CreateFontIndirectW(&metrics.lfMessageFont);
        if !result.is_null() {
            return result;
        }
    }
    CreateFontW(
        -((10 * dpi / 72) as i32),
        0,
        0,
        0,
        if heading { FW_SEMIBOLD } else { FW_NORMAL } as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        DEFAULT_QUALITY as u32,
        DEFAULT_PITCH as u32 | FF_DONTCARE as u32,
        wide("Segoe UI").as_ptr(),
    )
}

pub(super) unsafe fn register(instance: HINSTANCE) -> Result<(), String> {
    let name = wide(CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    if RegisterClassW(&class) == 0 {
        Err({
            let __ui_arg0 = &(GetLastError());
            crate::i18n::format(
                crate::i18n::Key::M5D3735A8FFB4,
                &[("arg0", format!("{}", __ui_arg0))],
            )
        })
    } else {
        connection_edit::register(instance)?;
        connection_setup::register(instance)?;
        share_code::register(instance)?;
        translation::register(instance)
    }
}

pub(super) unsafe fn show(app: *mut App) {
    if !(*app).settings_hwnd.is_null() {
        ShowWindow((*app).settings_hwnd, SW_SHOW);
        SetForegroundWindow((*app).settings_hwnd);
        return;
    }
    let dpi = (*app).dpi;
    let mut keys = vec![("Enter".into(), VK_RETURN as u32)];
    for n in 0..24 {
        // Windows reserves F12 for the debugger; RegisterHotKey cannot own it.
        if n == 11 {
            continue;
        }
        keys.push((format!("F{}", n + 1), VK_F1 as u32 + n));
    }
    for c in b'A'..=b'Z' {
        keys.push(((c as char).to_string(), c as u32));
    }
    for c in b'0'..=b'9' {
        keys.push(((c as char).to_string(), c as u32));
    }
    keys.extend([
        ("Space".into(), VK_SPACE as u32),
        ("Tab".into(), VK_TAB as u32),
        ("Insert".into(), VK_INSERT as u32),
        ("Home".into(), VK_HOME as u32),
        ("End".into(), VK_END as u32),
        ("Delete".into(), VK_DELETE as u32),
        ("Page Up".into(), VK_PRIOR as u32),
        ("Page Down".into(), VK_NEXT as u32),
        ("`".into(), 0xc0),
    ]);
    if !keys.iter().any(|(_, vk)| *vk == (*app).settings.hotkey_vk) {
        keys.push((
            format!("Key {}", (*app).settings.hotkey_vk),
            (*app).settings.hotkey_vk,
        ));
    }
    // Discovery reads filenames only; opening settings does not parse every
    // installed catalog. Keep the saved choice visible if its file is missing.
    let mut languages = crate::i18n::available_languages();
    if !languages
        .iter()
        .any(|choice| choice.id == (*app).settings.ui_language)
    {
        languages.push(crate::i18n::LanguageChoice {
            id: (*app).settings.ui_language.clone(),
            label: (*app).settings.ui_language.clone(),
        });
    }
    let state = Box::new(Dialog {
        app,
        draft: (*app).settings.clone(),
        hwnd: null_mut(),
        dpi,
        font: settings_font(dpi, false),
        heading_font: settings_font(dpi, true),
        brush: CreateSolidBrush(GetSysColor(COLOR_BTNFACE)),
        edit_brush: CreateSolidBrush(GetSysColor(COLOR_WINDOW)),
        custom_colors: [0x00ffffff; 16],
        languages,
        keys,
        targets: Vec::new(),
        filling_targets: false,
        filling_tabs: false,
        target_requires_running: false,
        created: false,
        scroll_x: 0,
        scroll_y: 0,
        controls: Vec::new(),
        last_connection_status: String::new(),
        watch_connection_status: false,
        setup_active: false,
    });
    let ptr = Box::into_raw(state);
    let mut owner: RECT = zeroed();
    GetWindowRect((*app).hwnd, &mut owner);
    let mut work: RECT = zeroed();
    SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
    let width = scale(690, dpi).min(work.right - work.left);
    let height = scale(
        CONTENT_HEIGHT + 43
            - if (*app).settings.connection_mode == ConnectionMode::Rpc {
                RPC_COMPACT_BY
            } else {
                0
            },
        dpi,
    )
    .min(work.bottom - work.top);
    // Prefer placing the form beside the actual chat so live edits are visible.
    // Small work areas still retain native scrolling and ordinary window order.
    let gap = scale(16, dpi);
    let x = if owner.right + gap + width <= work.right {
        owner.right + gap
    } else if owner.left - gap - width >= work.left {
        owner.left - gap - width
    } else if owner.left - work.left >= work.right - owner.right {
        work.left
    } else {
        (work.right - width).max(work.left)
    };
    let y = ((owner.top + owner.bottom - height) / 2)
        .clamp(work.top, (work.bottom - height).max(work.top));
    settings_session(app, true);
    let hwnd = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        wide(CLASS).as_ptr(),
        wide(&{
            let __ui_arg0 = &(env!("CARGO_PKG_VERSION"));
            crate::i18n::format(
                crate::i18n::Key::M14FC17723CA1,
                &[("arg0", __ui_arg0.to_string())],
            )
        })
        .as_ptr(),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VSCROLL | WS_HSCROLL | WS_CLIPCHILDREN,
        x,
        y,
        width,
        height,
        (*app).hwnd,
        null_mut(),
        GetModuleHandleW(null()),
        ptr.cast(),
    );
    if hwnd.is_null() {
        // While CreateWindowExW is running, the caller retains ownership. WM_NCDESTROY
        // does not free the allocation until creation has returned successfully.
        cleanup(ptr);
        settings_session(app, false);
        set_mode(app, Mode::Manage);
        (*app).set_status(crate::i18n::text(crate::i18n::Key::MCB5FCD07DAD3));
        return;
    }
    (*ptr).created = true;
    (*app).settings_hwnd = hwnd;
    refresh_preview(ptr);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(GetDlgItem(hwnd, UI_LANGUAGE));
}

/// The main tab bar uses the same draft editor as the settings window. Opening
/// this path never saves preferences or starts another Discord subscription.
pub(super) unsafe fn show_add_channel(app: *mut App) {
    show(app);
    let hwnd = (*app).settings_hwnd;
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        return;
    }
    let s = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog;
    if !s.is_null() && !(*s).setup_active {
        launch_channel_editor(s, true);
    }
}

unsafe fn cleanup(state: *mut Dialog) {
    let state = Box::from_raw(state);
    DeleteObject(state.font as _);
    DeleteObject(state.heading_font as _);
    DeleteObject(state.brush as _);
    DeleteObject(state.edit_brush as _);
}

#[allow(clippy::too_many_arguments)]
unsafe fn control(
    state: *mut Dialog,
    class: &str,
    label: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    style: u32,
) -> HWND {
    let dpi = (*state).dpi;
    let hwnd = CreateWindowExW(
        if class == "EDIT" { WS_EX_CLIENTEDGE } else { 0 },
        wide(class).as_ptr(),
        wide(label).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        scale(x, dpi),
        scale(y + language_area_offset(id), dpi),
        scale(w, dpi),
        scale(h, dpi),
        (*state).hwnd,
        id as usize as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(hwnd, WM_SETFONT, (*state).font as usize, 1);
    (*state).controls.push(ControlLayout {
        hwnd,
        x,
        y,
        width: w,
        height: h,
        heading: false,
    });
    hwnd
}
unsafe fn label(s: *mut Dialog, value: &str, x: i32, y: i32, w: i32) {
    control(s, "STATIC", value, 0, x, y, w, 20, 0);
}
unsafe fn heading(s: *mut Dialog, value: &str, x: i32, y: i32, w: i32) {
    let hwnd = control(s, "STATIC", value, 0, x, y, w, 20, 0);
    SendMessageW(hwnd, WM_SETFONT, (*s).heading_font as usize, 1);
    if let Some(layout) = (*s).controls.last_mut() {
        layout.heading = true;
    }
}
unsafe fn edit(s: *mut Dialog, value: &str, id: i32, x: i32, y: i32, w: i32, flags: u32) {
    control(
        s,
        "EDIT",
        value,
        id,
        x,
        y,
        w,
        26,
        WS_TABSTOP | ES_AUTOHSCROLL as u32 | flags,
    );
}
unsafe fn button(s: *mut Dialog, value: &str, id: i32, x: i32, y: i32, w: i32) {
    control(
        s,
        "BUTTON",
        value,
        id,
        x,
        y,
        w,
        29,
        WS_TABSTOP
            | BS_NOTIFY as u32
            | if id == SAVE {
                BS_DEFPUSHBUTTON as u32
            } else {
                BS_PUSHBUTTON as u32
            },
    );
}
unsafe fn checked(hwnd: HWND) -> bool {
    SendMessageW(hwnd, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
}
unsafe fn check(s: *mut Dialog, value: &str, id: i32, x: i32, y: i32, w: i32, on: bool) {
    let hwnd = control(
        s,
        "BUTTON",
        value,
        id,
        x,
        y,
        w,
        24,
        WS_TABSTOP | BS_AUTOCHECKBOX as u32 | BS_NOTIFY as u32,
    );
    SendMessageW(
        hwnd,
        BM_SETCHECK,
        if on {
            BST_CHECKED as usize
        } else {
            BST_UNCHECKED as usize
        },
        0,
    );
}

unsafe fn create_controls(s: *mut Dialog) {
    control(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M8A1424E8A673),
        UI_LANGUAGE_LABEL,
        22,
        21,
        210,
        20,
        SS_NOPREFIX,
    );
    let language = control(
        s,
        "COMBOBOX",
        "",
        UI_LANGUAGE,
        242,
        16,
        230,
        160,
        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    let languages = &(*s).languages;
    for choice in languages {
        SendMessageW(
            language,
            CB_ADDSTRING,
            0,
            wide(&choice.label).as_ptr() as isize,
        );
    }
    let selected = languages
        .iter()
        .position(|choice| choice.id == (*s).draft.ui_language)
        .unwrap_or(0);
    SendMessageW(language, CB_SETCURSEL, selected, 0);
    control(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::MB3602B9DC25C),
        UI_LANGUAGE_HELP,
        22,
        50,
        610,
        32,
        SS_NOPREFIX,
    );
    heading(
        s,
        &crate::i18n::text(crate::i18n::Key::MC16E93E89D46),
        22,
        16,
        610,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::M8FA4F010EB1F),
        22,
        45,
        92,
    );
    let mode = control(
        s,
        "COMBOBOX",
        "",
        CONNECTION_MODE,
        122,
        39,
        320,
        160,
        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    for name in [
        crate::i18n::text(crate::i18n::Key::M2FADA47649BD),
        crate::i18n::text(crate::i18n::Key::M947679E1CEDD),
    ] {
        SendMessageW(mode, CB_ADDSTRING, 0, wide(&name).as_ptr() as isize);
    }
    SendMessageW(
        mode,
        CB_SETCURSEL,
        usize::from((*s).draft.connection_mode == ConnectionMode::Relay),
        0,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M6DC5E83BFA0D),
        SHARE_CONNECTION,
        460,
        39,
        172,
    );

    // Editing channel destinations is a local draft operation. The separate
    // wizard remains available for first-time app setup and explicit login.
    control(
        s,
        "STATIC",
        "",
        CONNECTION_STATUS,
        22,
        82,
        610,
        42,
        SS_NOPREFIX,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M530F1DA5A94C),
        EDIT_CONNECTION,
        22,
        135,
        212,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::MB9A439BAF89D),
        CONNECT_DISCORD,
        246,
        135,
        202,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M91ED92D8A342),
        LOGOUT_DISCORD,
        460,
        135,
        172,
    );
    control(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M104BE950533F),
        CHANNEL_TABS_LABEL,
        22,
        178,
        610,
        20,
        0,
    );
    let tabs = control(
        s,
        "COMBOBOX",
        "",
        CHANNEL_TABS,
        22,
        201,
        310,
        260,
        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    SendMessageW(tabs, CB_SETDROPPEDWIDTH, scale(610, (*s).dpi) as usize, 0);
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M9915606DA6BD),
        ADD_CHANNEL_TAB,
        345,
        199,
        84,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M2FE89F7C1EB6),
        EDIT_CHANNEL_TAB,
        441,
        199,
        90,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::MFB2D8AAF0C86),
        REMOVE_CHANNEL_TAB,
        543,
        199,
        89,
    );
    control(
        s,
        "STATIC",
        "",
        CONNECTION_HELP,
        22,
        242,
        610,
        42,
        SS_NOPREFIX,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M8C062CE572A0),
        SEND_TIMING,
        22,
        291,
        190,
    );

    control(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M1F193F147AA1),
        CHANNEL_LABEL,
        22,
        82,
        610,
        20,
        0,
    );
    edit(s, &(*s).draft.channel_url.clone(), CHANNEL, 22, 103, 610, 0);
    control(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::MDE5F9F3C26A7),
        RELAY_LABEL,
        22,
        138,
        610,
        20,
        0,
    );
    edit(s, &(*s).draft.relay_url.clone(), RELAY, 22, 159, 610, 0);
    control(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M3308D2EDCA0B),
        KEY_LABEL,
        22,
        194,
        610,
        20,
        0,
    );
    edit(
        s,
        &(*s).draft.access_key.clone(),
        KEY,
        22,
        215,
        610,
        ES_PASSWORD as u32,
    );
    for (id, limit) in [(CHANNEL, 2048), (RELAY, 2048), (KEY, 512)] {
        SendMessageW(GetDlgItem((*s).hwnd, id), EM_SETLIMITTEXT, limit, 0);
    }

    // A fixed connection panel keeps the rest of the form in place while
    // switching modes. Hidden edits stay alive, retaining unsaved drafts.
    const BELOW_CONNECTION: i32 = 136;
    heading(
        s,
        &crate::i18n::text(crate::i18n::Key::MBAE52A7EFFDB),
        22,
        216 + BELOW_CONNECTION,
        610,
    );
    let target = control(
        s,
        "COMBOBOX",
        "",
        TARGET,
        22,
        243 + BELOW_CONNECTION,
        444,
        280,
        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    SendMessageW(target, CB_SETDROPPEDWIDTH, scale(610, (*s).dpi) as usize, 0);
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M00F606C9ABBD),
        REFRESH_APPS,
        477,
        241 + BELOW_CONNECTION,
        155,
    );
    control(
        s,
        "STATIC",
        "",
        TARGET_PATH,
        22,
        274 + BELOW_CONNECTION,
        610,
        24,
        SS_PATHELLIPSIS | SS_NOPREFIX,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::M57564A1C185A),
        22,
        306 + BELOW_CONNECTION,
        110,
    );
    let combo = control(
        s,
        "COMBOBOX",
        "",
        HOTKEY,
        22,
        327 + BELOW_CONNECTION,
        140,
        300,
        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
    );
    let mut selected = 0;
    for (i, (name, vk)) in (*s).keys.iter().enumerate() {
        SendMessageW(combo, CB_ADDSTRING, 0, wide(name).as_ptr() as isize);
        if *vk == (*s).draft.hotkey_vk {
            selected = i;
        }
    }
    SendMessageW(combo, CB_SETCURSEL, selected, 0);
    let mods = (*s).draft.hotkey_modifiers;
    check(
        s,
        "Ctrl",
        CTRL,
        184,
        328 + BELOW_CONNECTION,
        85,
        mods & MOD_CONTROL != 0,
    );
    check(
        s,
        "Alt",
        ALT,
        281,
        328 + BELOW_CONNECTION,
        76,
        mods & MOD_ALT != 0,
    );
    check(
        s,
        "Shift",
        SHIFT,
        371,
        328 + BELOW_CONNECTION,
        85,
        mods & MOD_SHIFT != 0,
    );
    check(
        s,
        "Win",
        WIN,
        474,
        328 + BELOW_CONNECTION,
        85,
        mods & MOD_WIN != 0,
    );
    heading(
        s,
        &crate::i18n::text(crate::i18n::Key::M83071C5A1EBF),
        22,
        373 + BELOW_CONNECTION,
        610,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M85769C7811B7),
        FONT,
        22,
        400 + BELOW_CONNECTION,
        146,
    );
    control(
        s,
        "STATIC",
        "",
        FONT_LABEL,
        181,
        405 + BELOW_CONNECTION,
        451,
        23,
        0,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::MB048BB55A1CB),
        FOREGROUND,
        22,
        440 + BELOW_CONNECTION,
        146,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M34BFEE9B7A21),
        AUTHOR_COLOR,
        181,
        440 + BELOW_CONNECTION,
        146,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::MF377E2785F66),
        BACKGROUND,
        340,
        440 + BELOW_CONNECTION,
        146,
    );
    check(
        s,
        &crate::i18n::text(crate::i18n::Key::M6ED963619B15),
        TRANSLATION_STYLE,
        22,
        482 + BELOW_CONNECTION,
        180,
        (*s).draft.translation_style_enabled,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::MA613B43D5060),
        218,
        487 + BELOW_CONNECTION,
        82,
    );
    edit(
        s,
        &(*s).draft.translation_font_size.to_string(),
        TRANSLATION_SIZE,
        310,
        482 + BELOW_CONNECTION,
        65,
        ES_NUMBER as u32,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M19305BDEF18C),
        TRANSLATION_COLOR,
        402,
        480 + BELOW_CONNECTION,
        230,
    );
    const BELOW_STYLE: i32 = BELOW_CONNECTION + 44;
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::MBAC094565460),
        22,
        490 + BELOW_STYLE,
        150,
    );
    edit(
        s,
        &(*s).draft.background_opacity.to_string(),
        OPACITY,
        181,
        485 + BELOW_STYLE,
        65,
        ES_NUMBER as u32,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::ME9674A63B6B9),
        282,
        490 + BELOW_STYLE,
        210,
    );
    edit(
        s,
        &(*s).draft.author_gap.to_string(),
        AUTHOR_GAP,
        557,
        485 + BELOW_STYLE,
        75,
        ES_NUMBER as u32,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::MAD76610B25F4),
        22,
        535 + BELOW_STYLE,
        67,
    );
    edit(
        s,
        &(*s).draft.letter_spacing.to_string(),
        LETTER_SPACING,
        89,
        530 + BELOW_STYLE,
        65,
        0,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::MC02AD4396258),
        181,
        535 + BELOW_STYLE,
        95,
    );
    edit(
        s,
        &(*s).draft.line_spacing.to_string(),
        LINE_SPACING,
        282,
        530 + BELOW_STYLE,
        65,
        ES_NUMBER as u32,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::MC5AEA894F6B2),
        370,
        535 + BELOW_STYLE,
        175,
    );
    edit(
        s,
        &(*s).draft.message_spacing.to_string(),
        MESSAGE_SPACING,
        557,
        530 + BELOW_STYLE,
        75,
        ES_NUMBER as u32,
    );
    check(
        s,
        &crate::i18n::text(crate::i18n::Key::M1817FBA45E94),
        TEXT_OUTLINE,
        22,
        574 + BELOW_STYLE,
        180,
        (*s).draft.text_outline,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M338CDAE7CEA5),
        OUTLINE_COLOR,
        218,
        573 + BELOW_STYLE,
        180,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::M12A7D4F650A8),
        420,
        578 + BELOW_STYLE,
        120,
    );
    edit(
        s,
        &(*s).draft.outline_width.to_string(),
        OUTLINE_WIDTH,
        557,
        573 + BELOW_STYLE,
        75,
        ES_NUMBER as u32,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::M0F81D451B505),
        22,
        614 + BELOW_STYLE,
        610,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::M11860B043841),
        22,
        638 + BELOW_STYLE,
        610,
    );
    for id in [
        OPACITY,
        AUTHOR_GAP,
        LETTER_SPACING,
        LINE_SPACING,
        MESSAGE_SPACING,
        OUTLINE_WIDTH,
        TRANSLATION_SIZE,
        SECONDS,
    ] {
        let control = GetDlgItem((*s).hwnd, id);
        if !control.is_null() {
            SendMessageW(control, EM_SETLIMITTEXT, 4, 0);
        }
    }
    const BELOW_READABILITY: i32 = BELOW_STYLE + 190;
    check(
        s,
        &crate::i18n::text(crate::i18n::Key::MA5AB20DF65A8),
        AUTOHIDE,
        22,
        486 + BELOW_READABILITY,
        240,
        (*s).draft.hide_when_idle,
    );
    edit(
        s,
        &(*s).draft.hide_after_secs.to_string(),
        SECONDS,
        281,
        485 + BELOW_READABILITY,
        73,
        ES_NUMBER as u32,
    );
    SendMessageW(GetDlgItem((*s).hwnd, SECONDS), EM_SETLIMITTEXT, 4, 0);
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::M38E63657372B),
        367,
        490 + BELOW_READABILITY,
        265,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::M9B9FD14D2469),
        22,
        523 + BELOW_READABILITY,
        610,
    );
    control(
        s,
        "STATIC",
        "",
        TRANSLATION_SUMMARY,
        414,
        557 + BELOW_READABILITY,
        218,
        34,
        SS_NOPREFIX,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M3AE51915D669),
        TRANSLATION_SETTINGS,
        22,
        552 + BELOW_READABILITY,
        180,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::MD93415BC698D),
        TRANSLATION_TIMING,
        218,
        552 + BELOW_READABILITY,
        180,
    );
    check(
        s,
        &crate::i18n::text(crate::i18n::Key::RomajiJapaneseInput),
        ROMAJI_JAPANESE_INPUT,
        22,
        598 + BELOW_READABILITY,
        610,
        (*s).draft.romaji_japanese_input,
    );
    label(
        s,
        &crate::i18n::text(crate::i18n::Key::RomajiJapaneseInputHelp),
        42,
        626 + BELOW_READABILITY,
        590,
    );
    check(
        s,
        &crate::i18n::text(crate::i18n::Key::M0BEBB182B358),
        SHOW_DIAGNOSTICS,
        22,
        598 + BELOW_READABILITY + ROMAJI_AREA_HEIGHT,
        610,
        (*s).draft.show_diagnostics,
    );
    control(
        s,
        "STATIC",
        "",
        STATUS,
        22,
        640 + BELOW_READABILITY + ROMAJI_AREA_HEIGHT,
        610,
        42,
        SS_NOPREFIX,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::MBDE76B4A9F84),
        CANCEL,
        423,
        1073 + ROMAJI_AREA_HEIGHT,
        98,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M2DB8261CF113),
        SAVE,
        534,
        1073 + ROMAJI_AREA_HEIGHT,
        98,
    );
    outline_controls(s);
    translation_style_controls(s);
    refresh_channel_tabs(s);
    translation_summary(s);
    font_label(s);
    refresh_apps(s);
    connection_controls(s);
    if let Some(warning) = &(*(*s).app).locale_warning {
        error(s, warning);
    }
    scroll_ranges(s);
    SetTimer((*s).hwnd, SETTINGS_TIMER, 500, None);
}

unsafe fn selected_connection_mode(s: *mut Dialog) -> Result<ConnectionMode, String> {
    match SendMessageW(GetDlgItem((*s).hwnd, CONNECTION_MODE), CB_GETCURSEL, 0, 0) {
        0 => Ok(ConnectionMode::Rpc),
        1 => Ok(ConnectionMode::Relay),
        _ => Err(crate::i18n::text(crate::i18n::Key::M962A4677D9E9)),
    }
}

unsafe fn selected_ui_language(s: *mut Dialog) -> Result<String, String> {
    let index = SendMessageW(GetDlgItem((*s).hwnd, UI_LANGUAGE), CB_GETCURSEL, 0, 0);
    let languages = &(*s).languages;
    usize::try_from(index)
        .ok()
        .and_then(|index| languages.get(index))
        .map(|choice| choice.id.clone())
        .ok_or_else(|| crate::i18n::text(crate::i18n::Key::M1BF227715412))
}

unsafe fn refresh_channel_tabs(s: *mut Dialog) {
    if let Err(message) = (*s).draft.ensure_channel_tabs() {
        error(s, &message);
        return;
    }
    let combo = GetDlgItem((*s).hwnd, CHANNEL_TABS);
    (*s).filling_tabs = true;
    SendMessageW(combo, CB_RESETCONTENT, 0, 0);
    let active = (*s).draft.active_channel_tab;
    let tabs = (*s).draft.channel_tabs.clone();
    let mut selected = 0;
    for (index, tab) in tabs.iter().enumerate() {
        let caption = if tab.webhook_url.trim().is_empty() {
            {
                let __ui_arg0 = &(tab.name);
                crate::i18n::format(
                    crate::i18n::Key::M8C70033162AD,
                    &[("arg0", __ui_arg0.to_string())],
                )
            }
        } else {
            tab.name.clone()
        };
        SendMessageW(combo, CB_ADDSTRING, 0, wide(&caption).as_ptr() as isize);
        if tab.id == active {
            selected = index;
        }
    }
    SendMessageW(combo, CB_SETCURSEL, selected, 0);
    (*s).filling_tabs = false;
    EnableWindow(
        GetDlgItem((*s).hwnd, ADD_CHANNEL_TAB),
        i32::from(tabs.len() < config::MAX_CHANNEL_TABS),
    );
    EnableWindow(
        GetDlgItem((*s).hwnd, REMOVE_CHANNEL_TAB),
        i32::from(tabs.len() > 1),
    );
}

unsafe fn channel_draft_changed(s: *mut Dialog, message: &str) {
    set_text(GetDlgItem((*s).hwnd, CHANNEL), &(*s).draft.channel_url);
    refresh_channel_tabs(s);
    translation_summary(s);
    (*s).last_connection_status.clear();
    refresh_connection_status(s);
    error(s, message);
}

unsafe fn choose_channel_tab(s: *mut Dialog) {
    if (*s).filling_tabs || (*s).setup_active {
        return;
    }
    let index = SendMessageW(GetDlgItem((*s).hwnd, CHANNEL_TABS), CB_GETCURSEL, 0, 0);
    let tabs = &(*s).draft.channel_tabs;
    let Some(id) = usize::try_from(index)
        .ok()
        .and_then(|index| tabs.get(index))
        .map(|tab| tab.id)
    else {
        return;
    };
    if let Err(message) = (*s).draft.select_channel_tab(id) {
        refresh_channel_tabs(s);
        error(s, &message);
        return;
    }
    channel_draft_changed(s, &crate::i18n::text(crate::i18n::Key::M6FAE73FA3E4F));
}

fn edited_channel_settings(
    current: &Settings,
    values: &connection_edit::EditValues,
    adding: bool,
) -> Result<Settings, String> {
    let mut draft = current.clone();
    if adding {
        let id = draft.add_channel_tab(&values.name, &values.channel_url, &values.webhook_url)?;
        draft.select_channel_tab(id)?;
    } else {
        draft.edit_channel_tab(
            draft.active_channel_tab,
            &values.name,
            &values.channel_url,
            &values.webhook_url,
        )?;
    }
    if draft.discord_app_id != values.discord_app_id {
        draft.discord_app_id.clone_from(&values.discord_app_id);
        // The public app ID is shared across every tab. Changing the receiving
        // app must not silently reuse any old paid-translation execution choice.
        for tab in &mut draft.channel_tabs {
            tab.translation_enabled = false;
        }
        draft.translation.enabled = false;
    }
    draft.sync_active_tab()?;
    Ok(draft)
}

/// A profile replaces only the selected draft destination, retaining its stable
/// tab ID/name and the remaining tabs. The same duplicate-channel guard used by
/// the manual editor runs before accepting a pasted/file profile.
fn imported_connection_settings(
    current: &Settings,
    profile: &crate::connection_profile::Profile,
) -> Result<Settings, String> {
    let mut base = current.clone();
    base.connection_mode = ConnectionMode::Rpc;
    base.ensure_channel_tabs()?;
    let values = connection_edit::EditValues {
        name: base
            .active_tab()
            .map(|tab| tab.name.clone())
            .ok_or_else(|| crate::i18n::text(crate::i18n::Key::MCD533964FA8A))?,
        discord_app_id: profile.discord_app_id.clone(),
        channel_url: profile.channel_url.clone(),
        webhook_url: profile.webhook_url.clone(),
    };
    let edited = edited_channel_settings(&base, &values, false)?;
    let mut draft = profile.apply_to(&edited);
    draft.sync_active_tab()?;
    Ok(draft)
}

unsafe fn launch_share_code(s: *mut Dialog) {
    if (*s).setup_active || (*s).draft.connection_mode != ConnectionMode::Rpc {
        return;
    }
    let owner = (*s).hwnd;
    (*s).setup_active = true;
    let result = share_code::run(owner, (*s).dpi, &(*s).draft, share_code::Purpose::Manage);
    // Closing the main app can destroy this modeless owner inside the modal loop.
    if IsWindow(owner) == 0 {
        return;
    }
    (*s).setup_active = false;
    if let Some(profile) = result {
        match imported_connection_settings(&(*s).draft, &profile) {
            Ok(draft) => {
                (*s).draft = draft;
                channel_draft_changed(s, &crate::i18n::text(crate::i18n::Key::MCFFD0A2E4BDD));
            }
            Err(message) => error(s, &message),
        }
    }
    SetFocus(GetDlgItem(owner, SHARE_CONNECTION));
}

unsafe fn launch_channel_editor(s: *mut Dialog, adding: bool) {
    if (*s).setup_active {
        return;
    }
    if (*s).draft.connection_mode != ConnectionMode::Rpc {
        error(s, &crate::i18n::text(crate::i18n::Key::MB45747DBA7FC));
        return;
    }
    if adding && (*s).draft.channel_tabs.len() >= config::MAX_CHANNEL_TABS {
        error(s, &{
            let __ui_arg0 = &(config::MAX_CHANNEL_TABS);
            crate::i18n::format(
                crate::i18n::Key::M9FF60E08BCC6,
                &[("arg0", format!("{}", __ui_arg0))],
            )
        });
        return;
    }
    let owner = (*s).hwnd;
    let mut initial = connection_edit::EditValues {
        name: if adding {
            {
                let __ui_arg0 = &((*s).draft.channel_tabs.len() + 1);
                crate::i18n::format(
                    crate::i18n::Key::M0C43D0916102,
                    &[("arg0", format!("{}", __ui_arg0))],
                )
            }
        } else {
            (*s).draft
                .active_tab()
                .map(|tab| tab.name.clone())
                .unwrap_or_else(|| crate::i18n::text(crate::i18n::Key::MAC3536707B34))
        },
        discord_app_id: (*s).draft.discord_app_id.clone(),
        channel_url: if adding {
            String::new()
        } else {
            (*s).draft.channel_url.clone()
        },
        webhook_url: if adding {
            String::new()
        } else {
            (*s).draft.webhook_url.clone()
        },
    };
    (*s).setup_active = true;
    loop {
        let result = connection_edit::run(owner, (*s).dpi, initial, adding);
        if IsWindow(owner) == 0 {
            return;
        }
        let Some(values) = result else {
            (*s).setup_active = false;
            return;
        };
        match edited_channel_settings(&(*s).draft, &values, adding) {
            Ok(draft) => {
                (*s).draft = draft;
                (*s).setup_active = false;
                channel_draft_changed(
                    s,
                    &if adding {
                        crate::i18n::text(crate::i18n::Key::M97CF9257C035)
                    } else {
                        crate::i18n::text(crate::i18n::Key::M87907E09A4BE)
                    },
                );
                SetFocus(GetDlgItem(owner, CHANNEL_TABS));
                return;
            }
            Err(message) => {
                error(s, &message);
                MessageBoxW(
                    owner,
                    wide(&message).as_ptr(),
                    wide(&crate::i18n::text(crate::i18n::Key::MDBAADF2FE3D6)).as_ptr(),
                    MB_OK | MB_ICONINFORMATION,
                );
                if IsWindow(owner) == 0 {
                    return;
                }
                // Keep pasted values when a duplicate destination needs fixing.
                initial = values;
            }
        }
    }
}

unsafe fn remove_channel_tab(s: *mut Dialog) {
    let id = (*s).draft.active_channel_tab;
    match (*s).draft.remove_channel_tab(id) {
        Ok(()) => channel_draft_changed(s, &crate::i18n::text(crate::i18n::Key::M8989636C0C47)),
        Err(message) => error(s, &message),
    }
}

unsafe fn connection_controls(s: *mut Dialog) {
    let rpc = (*s).draft.connection_mode == ConnectionMode::Rpc;
    let demo = (*(*s).app).demo;
    for (ids, visible) in [
        (
            &[
                CONNECT_DISCORD,
                SHARE_CONNECTION,
                EDIT_CONNECTION,
                LOGOUT_DISCORD,
                CONNECTION_STATUS,
                SEND_TIMING,
                CHANNEL_TABS_LABEL,
                CHANNEL_TABS,
                ADD_CHANNEL_TAB,
                EDIT_CHANNEL_TAB,
                REMOVE_CHANNEL_TAB,
            ][..],
            rpc,
        ),
        (
            &[CHANNEL_LABEL, CHANNEL, RELAY_LABEL, RELAY, KEY_LABEL, KEY][..],
            !rpc,
        ),
    ] {
        for id in ids {
            ShowWindow(
                GetDlgItem((*s).hwnd, *id),
                if visible { SW_SHOW } else { SW_HIDE },
            );
        }
    }
    EnableWindow(GetDlgItem((*s).hwnd, LOGOUT_DISCORD), i32::from(!demo));
    set_text(
        GetDlgItem((*s).hwnd, CONNECTION_HELP),
        &if demo {
            crate::i18n::text(crate::i18n::Key::M50C58CD5995A)
        } else if rpc {
            crate::i18n::text(crate::i18n::Key::M8C4CCC555ECE)
        } else {
            crate::i18n::text(crate::i18n::Key::M35593D22C0EA)
        },
    );
    if demo {
        set_text(
            GetDlgItem((*s).hwnd, CONNECTION_HELP),
            &crate::i18n::text(crate::i18n::Key::M7235425E9393),
        );
    }
    (*s).last_connection_status.clear();
    refresh_connection_status(s);
    scroll_ranges(s);
    position_controls(s);
    InvalidateRect((*s).hwnd, null(), 1);
}

unsafe fn refresh_connection_status(s: *mut Dialog) {
    let app = (*s).app;
    EnableWindow(
        GetDlgItem((*s).hwnd, TRANSLATION_TIMING),
        i32::from((*app).client.last_translation_timing().is_some()),
    );
    if (*s).draft.connection_mode != ConnectionMode::Rpc {
        return;
    }
    EnableWindow(
        GetDlgItem((*s).hwnd, SEND_TIMING),
        i32::from((*app).client.last_send_timing().is_some()),
    );
    let live_status = if (*app).demo {
        crate::i18n::text(crate::i18n::Key::M409981F70820)
    } else if (*app).client.is_discord_send_ready() {
        crate::i18n::text(crate::i18n::Key::MEFBB2021C25A)
    } else if (*app).client.is_discord_receive_ready() {
        if (*app).settings.webhook_url.trim().is_empty() {
            crate::i18n::text(crate::i18n::Key::MF496CB414D3E)
        } else {
            crate::i18n::text(crate::i18n::Key::MA57A9FC9D81D)
        }
    } else if (*app).settings.discord_app_id.trim().is_empty() {
        crate::i18n::text(crate::i18n::Key::M478A6890206A)
    } else {
        crate::i18n::text(crate::i18n::Key::M549223754739)
    };
    let saved = &(*app).settings;
    let draft = &(*s).draft;
    let edited = saved.connection_mode != draft.connection_mode
        || saved.discord_app_id != draft.discord_app_id
        || saved.active_channel_tab != draft.active_channel_tab
        || saved.channel_url != draft.channel_url
        || saved.webhook_url != draft.webhook_url
        || saved.channel_tabs != draft.channel_tabs;
    let name = draft
        .active_tab()
        .map(|tab| tab.name.clone())
        .unwrap_or_else(|| crate::i18n::text(crate::i18n::Key::MAC3536707B34));
    let status = format!(
        "{live_status}\r\n{}: {name}{}",
        if edited {
            crate::i18n::text(crate::i18n::Key::MF6A649171417)
        } else {
            crate::i18n::text(crate::i18n::Key::MBB4C89EE8F4D)
        },
        if edited {
            crate::i18n::text(crate::i18n::Key::M65CA0EEC2256)
        } else {
            String::new()
        },
    );
    if status != (*s).last_connection_status {
        set_text(GetDlgItem((*s).hwnd, CONNECTION_STATUS), &status);
        (*s).last_connection_status = status;
    }
}

unsafe fn launch_connection_setup(s: *mut Dialog) {
    if (*s).setup_active {
        return;
    }
    let hwnd = (*s).hwnd;
    (*s).setup_active = true;
    connection_setup::run(s);
    // The nested modal loop may also process the main app's close request.
    if IsWindow(hwnd) == 0 {
        return;
    }
    (*s).setup_active = false;
    set_text(GetDlgItem(hwnd, CHANNEL), &(*s).draft.channel_url);
    refresh_channel_tabs(s);
    translation_summary(s);
    connection_controls(s);
}

unsafe fn translation_summary(s: *mut Dialog) {
    let t = &(*s).draft.translation;
    set_text(GetDlgItem((*s).hwnd, TRANSLATION_SUMMARY), &{
        let __ui_arg0 = &(if t.enabled { "ON" } else { "OFF" });
        let __ui_arg1 = &(if t.show_received { "ON" } else { "OFF" });
        crate::i18n::format(
            crate::i18n::Key::M4620D22C4D60,
            &[
                ("arg0", __ui_arg0.to_string()),
                ("arg1", __ui_arg1.to_string()),
            ],
        )
    });
}

unsafe fn launch_translation_settings(s: *mut Dialog) {
    if (*s).setup_active {
        return;
    }
    let hwnd = (*s).hwnd;
    (*s).setup_active = true;
    let result = translation::run(s);
    if IsWindow(hwnd) == 0 {
        return;
    }
    (*s).setup_active = false;
    if let Some(translation) = result {
        (*s).draft.translation = translation;
        translation_summary(s);
        error(s, &crate::i18n::text(crate::i18n::Key::MC0EF3132F389));
    }
}

unsafe fn logout_discord(s: *mut Dialog) -> Result<(), String> {
    let app = (*s).app;
    if (*app).demo {
        return Err(demo_connection_notice());
    }
    (*app).client.logout_discord(&(*app).settings)?;
    super::clear_channel_history(app);
    error(s, &crate::i18n::text(crate::i18n::Key::M3F28B8899BBC));
    refresh_connection_status(s);
    Ok(())
}

unsafe fn target_path_label(s: *mut Dialog) {
    let path = &(*s).draft.target_exe;
    let label = if path.is_empty() {
        crate::i18n::text(crate::i18n::Key::MEEAD8D8D047E)
    } else {
        crate::i18n::format(
            crate::i18n::Key::MF8936C9637BD,
            &[("path", path.to_string())],
        )
    };
    set_text(GetDlgItem((*s).hwnd, TARGET_PATH), &label);
}

unsafe fn refresh_apps(s: *mut Dialog) {
    let applications = match running_apps::scan() {
        Ok(applications) => applications,
        Err(message) => {
            error(s, &message);
            target_path_label(s);
            return;
        }
    };
    let current = (*s).draft.target_exe.clone();
    let count = applications.entries.len();
    let mut choices = vec![TargetChoice {
        label: crate::i18n::text(crate::i18n::Key::MF4A57F9A0910),
        path: String::new(),
        running: false,
    }];
    if !current.is_empty()
        && !applications
            .entries
            .iter()
            .any(|entry| entry.path.eq_ignore_ascii_case(&current))
    {
        choices.push(TargetChoice {
            label: {
                let __ui_arg0 = &(running_apps::file_name(&current));
                crate::i18n::format(
                    crate::i18n::Key::M9FF1550E5071,
                    &[("arg0", __ui_arg0.to_string())],
                )
            },
            path: current.clone(),
            running: false,
        });
    }
    choices.extend(applications.entries.into_iter().map(|entry| TargetChoice {
        label: format!("{} · {}", running_apps::file_name(&entry.path), entry.title),
        path: entry.path,
        running: true,
    }));
    let selected = choices
        .iter()
        .position(|choice| choice.path.eq_ignore_ascii_case(&current))
        .unwrap_or(0);
    // Updating a native control may dispatch nested notifications. Publish the
    // new snapshot before doing so, and ignore selection notifications until done.
    (*s).filling_targets = true;
    (*s).targets = choices;
    let combo = GetDlgItem((*s).hwnd, TARGET);
    SendMessageW(combo, WM_SETREDRAW, 0, 0);
    SendMessageW(combo, CB_RESETCONTENT, 0, 0);
    for index in 0..(*s).targets.len() {
        let label = wide(&(&(*s).targets)[index].label);
        SendMessageW(combo, CB_ADDSTRING, 0, label.as_ptr() as isize);
    }
    SendMessageW(combo, CB_SETCURSEL, selected, 0);
    SendMessageW(combo, WM_SETREDRAW, 1, 0);
    (*s).filling_targets = false;
    InvalidateRect(combo, null(), 1);
    target_path_label(s);
    let note = if applications.inaccessible > 0 {
        crate::i18n::format(
            crate::i18n::Key::M07C95B657BA0,
            &[("count", format!("{}", count))],
        )
    } else if count == 0 {
        crate::i18n::text(crate::i18n::Key::MFD9477D97AAC)
    } else {
        crate::i18n::format(
            crate::i18n::Key::M31B4F5E1B77D,
            &[("count", format!("{}", count))],
        )
    };
    error(s, &note);
}

unsafe fn choose_target(s: *mut Dialog) {
    if (*s).filling_targets {
        return;
    }
    let combo = GetDlgItem((*s).hwnd, TARGET);
    let index = SendMessageW(combo, CB_GETCURSEL, 0, 0);
    let targets: &[TargetChoice] = &(*s).targets;
    let Some(choice) = targets.get(index as usize) else {
        return;
    };
    let path = choice.path.clone();
    let running = choice.running;
    if running {
        if let Err(message) = running_apps::validate_running(&path) {
            // Keep the old draft intact if the selected app has already exited.
            let previous = (*s)
                .targets
                .iter()
                .position(|entry| entry.path.eq_ignore_ascii_case(&(*s).draft.target_exe));
            SendMessageW(combo, CB_SETCURSEL, previous.unwrap_or(0), 0);
            error(s, &message);
            return;
        }
    }
    if !path.eq_ignore_ascii_case(&(*s).draft.target_exe) {
        (*s).target_requires_running = running;
    } else if running {
        (*s).target_requires_running = true;
    }
    let cleared = path.is_empty();
    (*s).draft.target_exe = path;
    target_path_label(s);
    error(
        s,
        &if cleared {
            crate::i18n::text(crate::i18n::Key::MC37E21517F55)
        } else if running {
            crate::i18n::text(crate::i18n::Key::M1C69007B61F0)
        } else {
            crate::i18n::text(crate::i18n::Key::M85F0C1FD74DE)
        },
    );
}

unsafe fn scroll_ranges(s: *mut Dialog) {
    let mut client: RECT = zeroed();
    GetClientRect((*s).hwnd, &mut client);
    let before = ((*s).scroll_x, (*s).scroll_y);
    for (bar, content, page, pos) in [
        (
            SB_HORZ,
            scale(CONTENT_WIDTH, (*s).dpi),
            client.right,
            (*s).scroll_x,
        ),
        (
            SB_VERT,
            scale(
                CONTENT_HEIGHT
                    - if (*s).draft.connection_mode == ConnectionMode::Rpc {
                        RPC_COMPACT_BY
                    } else {
                        0
                    },
                (*s).dpi,
            ),
            client.bottom,
            (*s).scroll_y,
        ),
    ] {
        let pos = pos.clamp(0, (content - page).max(0));
        if bar == SB_HORZ {
            (*s).scroll_x = pos;
        } else {
            (*s).scroll_y = pos;
        }
        let info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: content - 1,
            nPage: page.max(0) as u32,
            nPos: pos,
            nTrackPos: 0,
        };
        SetScrollInfo((*s).hwnd, bar, &info, 1);
    }
    if before != ((*s).scroll_x, (*s).scroll_y) {
        position_controls(s);
    }
}

unsafe fn layout_y(s: *mut Dialog, layout: &ControlLayout) -> i32 {
    let id = GetDlgCtrlID(layout.hwnd);
    let offset = language_area_offset(id);
    if id == CONNECTION_HELP {
        return if (*s).draft.connection_mode == ConnectionMode::Rpc {
            242 + offset
        } else {
            260 + offset
        };
    }
    if layout.y >= FIRST_GAME_CONTROL_Y && (*s).draft.connection_mode == ConnectionMode::Rpc {
        layout.y - RPC_COMPACT_BY + offset
    } else {
        layout.y + offset
    }
}

fn language_area_offset(id: i32) -> i32 {
    if matches!(id, UI_LANGUAGE | UI_LANGUAGE_LABEL | UI_LANGUAGE_HELP) {
        0
    } else {
        LANGUAGE_AREA_HEIGHT
    }
}

unsafe fn position_controls(s: *mut Dialog) {
    // Store unscaled coordinates instead of rebuilding controls on a DPI
    // change. Text, selections, password masks and unsaved edits survive.
    let controls = (*s).controls.clone();
    if controls.is_empty() {
        return;
    }
    let mut positions = BeginDeferWindowPos(controls.len() as i32);
    for layout in &controls {
        if positions.is_null() {
            break;
        }
        positions = DeferWindowPos(
            positions,
            layout.hwnd,
            null_mut(),
            scale(layout.x, (*s).dpi) - (*s).scroll_x,
            scale(layout_y(s, layout), (*s).dpi) - (*s).scroll_y,
            scale(layout.width, (*s).dpi),
            scale(layout.height, (*s).dpi),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
    if !positions.is_null() && EndDeferWindowPos(positions) != 0 {
        return;
    }
    // Batched positioning is only an optimization; retain a complete layout
    // even when Windows cannot allocate the temporary position list.
    for layout in &controls {
        SetWindowPos(
            layout.hwnd,
            null_mut(),
            scale(layout.x, (*s).dpi) - (*s).scroll_x,
            scale(layout_y(s, layout), (*s).dpi) - (*s).scroll_y,
            scale(layout.width, (*s).dpi),
            scale(layout.height, (*s).dpi),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

unsafe fn change_dpi(s: *mut Dialog, dpi: u32, suggested: &RECT) {
    let previous_dpi = (*s).dpi.max(1);
    (*s).dpi = dpi.max(1);
    (*s).scroll_x = ((*s).scroll_x as i64 * (*s).dpi as i64 / previous_dpi as i64) as i32;
    (*s).scroll_y = ((*s).scroll_y as i64 * (*s).dpi as i64 / previous_dpi as i64) as i32;
    let previous_font = (*s).font;
    let previous_heading = (*s).heading_font;
    let font = settings_font((*s).dpi, false);
    let heading_font = settings_font((*s).dpi, true);
    if !font.is_null() {
        (*s).font = font;
    }
    if !heading_font.is_null() {
        (*s).heading_font = heading_font;
    }
    let controls = (*s).controls.clone();
    for layout in controls {
        let font = if layout.heading {
            (*s).heading_font
        } else {
            (*s).font
        };
        SendMessageW(layout.hwnd, WM_SETFONT, font as usize, 0);
    }
    if (*s).font != previous_font {
        DeleteObject(previous_font as _);
    }
    if (*s).heading_font != previous_heading {
        DeleteObject(previous_heading as _);
    }
    SetWindowPos(
        (*s).hwnd,
        null_mut(),
        suggested.left,
        suggested.top,
        suggested.right - suggested.left,
        suggested.bottom - suggested.top,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    scroll_ranges(s);
    position_controls(s);
    SendMessageW(
        GetDlgItem((*s).hwnd, TARGET),
        CB_SETDROPPEDWIDTH,
        scale(610, (*s).dpi) as usize,
        0,
    );
    RedrawWindow(
        (*s).hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
    );
}

unsafe fn ensure_visible(s: *mut Dialog, control: HWND) {
    if control.is_null() || IsWindowVisible(control) == 0 {
        return;
    }
    let mut bounds: RECT = zeroed();
    let mut client: RECT = zeroed();
    GetWindowRect(control, &mut bounds);
    MapWindowPoints(null_mut(), (*s).hwnd, (&mut bounds as *mut RECT).cast(), 2);
    GetClientRect((*s).hwnd, &mut client);
    let margin = scale(10, (*s).dpi);
    let dx = if bounds.left < margin {
        bounds.left - margin
    } else {
        (bounds.right - client.right + margin).max(0)
    };
    let dy = if bounds.top < margin {
        bounds.top - margin
    } else {
        (bounds.bottom - client.bottom + margin).max(0)
    };
    scroll_to(s, false, (*s).scroll_x + dx);
    scroll_to(s, true, (*s).scroll_y + dy);
}

unsafe fn scroll(s: *mut Dialog, vertical: bool, command: u32) {
    let bar = if vertical { SB_VERT } else { SB_HORZ };
    let mut info: SCROLLINFO = zeroed();
    info.cbSize = size_of::<SCROLLINFO>() as u32;
    info.fMask = SIF_ALL;
    GetScrollInfo((*s).hwnd, bar, &mut info);
    let old = if vertical {
        (*s).scroll_y
    } else {
        (*s).scroll_x
    };
    let step = scale(35, (*s).dpi);
    let next = match command {
        x if x == SB_LINEUP as u32 => old - step,
        x if x == SB_LINEDOWN as u32 => old + step,
        x if x == SB_PAGEUP as u32 => old - info.nPage as i32,
        x if x == SB_PAGEDOWN as u32 => old + info.nPage as i32,
        x if x == SB_THUMBTRACK as u32 || x == SB_THUMBPOSITION as u32 => info.nTrackPos,
        x if x == SB_TOP as u32 => 0,
        x if x == SB_BOTTOM as u32 => info.nMax,
        _ => old,
    };
    scroll_to(s, vertical, next);
}

unsafe fn scroll_to(s: *mut Dialog, vertical: bool, next: i32) {
    let bar = if vertical { SB_VERT } else { SB_HORZ };
    let mut info: SCROLLINFO = zeroed();
    info.cbSize = size_of::<SCROLLINFO>() as u32;
    info.fMask = SIF_RANGE | SIF_PAGE;
    GetScrollInfo((*s).hwnd, bar, &mut info);
    let old = if vertical {
        (*s).scroll_y
    } else {
        (*s).scroll_x
    };
    let next = next.clamp(0, (info.nMax - info.nPage as i32 + 1).max(0));
    if next == old {
        return;
    }
    if vertical {
        (*s).scroll_y = next;
    } else {
        (*s).scroll_x = next;
    }
    SetScrollPos((*s).hwnd, bar, next, 1);
    // Move every child, including edits currently outside the viewport.
    // ScrollWindowEx only moves intersecting children and can leave controls
    // unreachable when the work area is shorter than the settings form.
    position_controls(s);
    InvalidateRect((*s).hwnd, null(), 1);
}

unsafe fn font_label(s: *mut Dialog) {
    set_text(
        GetDlgItem((*s).hwnd, FONT_LABEL),
        &format!("{}  ·  {} pt", (*s).draft.font_family, (*s).draft.font_size),
    );
}
unsafe fn error(s: *mut Dialog, value: &str) {
    (*s).watch_connection_status = false;
    set_text(GetDlgItem((*s).hwnd, STATUS), value);
}

unsafe fn choose_font(s: *mut Dialog) {
    let owner = (*s).hwnd;
    let mut log: LOGFONTW = zeroed();
    log.lfHeight = -(((*s).draft.font_size * (*s).dpi / 72) as i32);
    log.lfCharSet = DEFAULT_CHARSET;
    log.lfEscapement = 0;
    log.lfOrientation = 0;
    let face = wide(config::horizontal_font_family(&(*s).draft.font_family));
    for (slot, value) in log.lfFaceName.iter_mut().zip(face.iter()) {
        *slot = *value;
    }
    let mut chooser: CHOOSEFONTW = zeroed();
    chooser.lStructSize = size_of::<CHOOSEFONTW>() as u32;
    chooser.hwndOwner = owner;
    chooser.lpLogFont = &mut log;
    chooser.iPointSize = (*s).draft.font_size as i32 * 10;
    chooser.Flags = CF_SCREENFONTS | CF_INITTOLOGFONTSTRUCT | CF_LIMITSIZE | CF_NOVERTFONTS;
    chooser.nSizeMin = 8;
    chooser.nSizeMax = 48;
    let chosen = ChooseFontW(&mut chooser) != 0;
    // Native chooser message loops can also dispatch the main app's close.
    if IsWindow(owner) == 0 {
        return;
    }
    if chosen {
        let length = log
            .lfFaceName
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(log.lfFaceName.len());
        let chosen_family = String::from_utf16_lossy(&log.lfFaceName[..length]);
        (*s).draft.font_family = config::horizontal_font_family(&chosen_family).to_owned();
        (*s).draft.font_size = (chooser.iPointSize / 10).clamp(8, 48) as u32;
        font_label(s);
        refresh_preview(s);
    }
}

unsafe fn outline_controls(s: *mut Dialog) {
    let enabled = checked(GetDlgItem((*s).hwnd, TEXT_OUTLINE));
    for id in [OUTLINE_COLOR, OUTLINE_WIDTH] {
        EnableWindow(GetDlgItem((*s).hwnd, id), i32::from(enabled));
    }
}

unsafe fn translation_style_controls(s: *mut Dialog) {
    let enabled = checked(GetDlgItem((*s).hwnd, TRANSLATION_STYLE));
    for id in [TRANSLATION_SIZE, TRANSLATION_COLOR] {
        EnableWindow(GetDlgItem((*s).hwnd, id), i32::from(enabled));
    }
}

unsafe fn choose_color(s: *mut Dialog, target: i32) {
    let owner = (*s).hwnd;
    let mut custom_colors = (*s).custom_colors;
    let current = match target {
        BACKGROUND => (*s).draft.background_color,
        OUTLINE_COLOR => (*s).draft.outline_color,
        AUTHOR_COLOR => (*s).draft.author_color,
        TRANSLATION_COLOR => (*s).draft.translation_text_color,
        _ => (*s).draft.text_color,
    };
    let mut chooser: CHOOSECOLORW = zeroed();
    chooser.lStructSize = size_of::<CHOOSECOLORW>() as u32;
    chooser.hwndOwner = owner;
    chooser.rgbResult = color(current);
    chooser.lpCustColors = custom_colors.as_mut_ptr();
    chooser.Flags = CC_FULLOPEN | CC_RGBINIT;
    let chosen = ChooseColorW(&mut chooser) != 0;
    if IsWindow(owner) == 0 {
        return;
    }
    (*s).custom_colors = custom_colors;
    if chosen {
        match target {
            BACKGROUND => (*s).draft.background_color = uncolor(chooser.rgbResult),
            OUTLINE_COLOR => (*s).draft.outline_color = uncolor(chooser.rgbResult),
            AUTHOR_COLOR => (*s).draft.author_color = uncolor(chooser.rgbResult),
            TRANSLATION_COLOR => (*s).draft.translation_text_color = uncolor(chooser.rgbResult),
            _ => (*s).draft.text_color = uncolor(chooser.rgbResult),
        }
        error(s, &{
            let __ui_arg0 = &((*s).draft.text_color);
            let __ui_arg1 = &((*s).draft.translation_text_color);
            let __ui_arg2 = &((*s).draft.author_color);
            let __ui_arg3 = &((*s).draft.background_color);
            let __ui_arg4 = &((*s).draft.outline_color);
            crate::i18n::format(
                crate::i18n::Key::M09FE7B396E8F,
                &[
                    ("arg0", format!("{:06X}", __ui_arg0)),
                    ("arg1", format!("{:06X}", __ui_arg1)),
                    ("arg2", format!("{:06X}", __ui_arg2)),
                    ("arg3", format!("{:06X}", __ui_arg3)),
                    ("arg4", format!("{:06X}", __ui_arg4)),
                ],
            )
        });
        refresh_preview(s);
    }
}

fn visual_number<T: std::str::FromStr + PartialOrd + Copy>(
    value: &str,
    previous: T,
    low: T,
    high: T,
    strict: bool,
    message: &str,
) -> Result<T, String> {
    match value
        .trim()
        .parse::<T>()
        .ok()
        .filter(|n| *n >= low && *n <= high)
    {
        Some(value) => Ok(value),
        None if !strict => Ok(previous),
        None => Err(message.into()),
    }
}

unsafe fn appearance_controls(s: *mut Dialog, strict: bool) -> Result<Settings, String> {
    let hwnd = (*s).hwnd;
    let mut draft = (*s).draft.clone();
    draft.translation_style_enabled = checked(GetDlgItem(hwnd, TRANSLATION_STYLE));
    draft.translation_font_size = visual_number(
        &text(GetDlgItem(hwnd, TRANSLATION_SIZE)),
        draft.translation_font_size,
        8,
        48,
        strict && draft.translation_style_enabled,
        &crate::i18n::text(crate::i18n::Key::ME2D28A75D4AA),
    )?;
    draft.background_opacity = visual_number(
        &text(GetDlgItem(hwnd, OPACITY)),
        draft.background_opacity,
        0,
        100,
        strict,
        &crate::i18n::text(crate::i18n::Key::MF9A0722B5537),
    )?;
    draft.letter_spacing = visual_number(
        &text(GetDlgItem(hwnd, LETTER_SPACING)),
        draft.letter_spacing,
        -2,
        12,
        strict,
        &crate::i18n::text(crate::i18n::Key::MF4489A07FC5B),
    )?;
    draft.line_spacing = visual_number(
        &text(GetDlgItem(hwnd, LINE_SPACING)),
        draft.line_spacing,
        0,
        24,
        strict,
        &crate::i18n::text(crate::i18n::Key::ME6A311025F35),
    )?;
    draft.message_spacing = visual_number(
        &text(GetDlgItem(hwnd, MESSAGE_SPACING)),
        draft.message_spacing,
        0,
        48,
        strict,
        &crate::i18n::text(crate::i18n::Key::MED4F3262C870),
    )?;
    draft.author_gap = visual_number(
        &text(GetDlgItem(hwnd, AUTHOR_GAP)),
        draft.author_gap,
        0,
        24,
        strict,
        &crate::i18n::text(crate::i18n::Key::M7FB2F4585C41),
    )?;
    draft.outline_width = visual_number(
        &text(GetDlgItem(hwnd, OUTLINE_WIDTH)),
        draft.outline_width,
        1,
        3,
        strict,
        &crate::i18n::text(crate::i18n::Key::MED9861B904A8),
    )?;
    draft.hide_after_secs = visual_number(
        &text(GetDlgItem(hwnd, SECONDS)),
        draft.hide_after_secs,
        1,
        3600,
        strict,
        &crate::i18n::text(crate::i18n::Key::MB59BF4A5B52E),
    )?;
    draft.text_outline = checked(GetDlgItem(hwnd, TEXT_OUTLINE));
    draft.hide_when_idle = checked(GetDlgItem(hwnd, AUTOHIDE));
    draft.show_diagnostics = checked(GetDlgItem(hwnd, SHOW_DIAGNOSTICS));
    config::validate_appearance(&draft)?;
    Ok(draft)
}

unsafe fn refresh_preview(s: *mut Dialog) {
    if !(*s).created || (*(*s).app).closing {
        return;
    }
    KillTimer((*s).hwnd, PREVIEW_TIMER);
    if let Ok(draft) = appearance_controls(s, false) {
        (*s).draft.apply_appearance_from(&draft);
        preview_settings((*s).app, &draft);
    }
}

unsafe fn save(s: *mut Dialog) -> Result<(), String> {
    let hwnd = (*s).hwnd;
    // Do not update this draft field on combo changes: a separate explicit
    // connection-login action must not commit an unsaved UI-language choice.
    (*s).draft.ui_language = selected_ui_language(s)?;
    (*s).draft.connection_mode = selected_connection_mode(s)?;
    (*s).draft.relay_url = text(GetDlgItem(hwnd, RELAY)).trim().into();
    if (*s).draft.connection_mode == ConnectionMode::Relay {
        (*s).draft.channel_url = text(GetDlgItem(hwnd, CHANNEL)).trim().into();
    }
    (*s).draft.access_key = text(GetDlgItem(hwnd, KEY)).trim().into();
    let target_path: &str = &(*s).draft.target_exe;
    if (*s).target_requires_running && !target_path.is_empty() {
        running_apps::validate_running(target_path)?;
    }
    let appearance = appearance_controls(s, true)?;
    (*s).draft.apply_appearance_from(&appearance);
    let key = SendMessageW(GetDlgItem(hwnd, HOTKEY), CB_GETCURSEL, 0, 0);
    if key < 0 {
        return Err(crate::i18n::text(crate::i18n::Key::MB522F222DF45));
    }
    let keys: &[(String, u32)] = &(*s).keys;
    (*s).draft.hotkey_vk = keys
        .get(key as usize)
        .ok_or(crate::i18n::text(crate::i18n::Key::MB522F222DF45))?
        .1;
    let mut mods = 0;
    for (id, flag) in [
        (CTRL, MOD_CONTROL),
        (ALT, MOD_ALT),
        (SHIFT, MOD_SHIFT),
        (WIN, MOD_WIN),
    ] {
        if checked(GetDlgItem(hwnd, id)) {
            mods |= flag;
        }
    }
    (*s).draft.hotkey_modifiers = mods;
    if (*s).draft.translation.enabled && (*s).draft.connection_mode != ConnectionMode::Rpc {
        return Err(crate::i18n::text(crate::i18n::Key::M9347AAF40465));
    }
    // Read this personal input choice only into the candidate being saved.
    // A failed Save must not leave it in the connection-login draft, and
    // appearance previews/Cancel must not change the active input policy.
    let mut candidate = (*s).draft.clone();
    candidate.romaji_japanese_input = checked(GetDlgItem(hwnd, ROMAJI_JAPANESE_INPUT));
    commit_settings((*s).app, candidate)
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        let s = create.lpCreateParams as *mut Dialog;
        (*s).hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, s as isize);
    }
    let s = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog;
    if s.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_CREATE => {
            create_controls(s);
            0
        }
        WM_SIZE => {
            scroll_ranges(s);
            0
        }
        WM_DPICHANGED => {
            change_dpi(s, (wp & 0xffff) as u32, &*(lp as *const RECT));
            0
        }
        WM_TIMER if wp == SETTINGS_TIMER => {
            refresh_connection_status(s);
            0
        }
        WM_TIMER if wp == PREVIEW_TIMER => {
            refresh_preview(s);
            0
        }
        WM_VSCROLL => {
            scroll(s, true, (wp & 0xffff) as u32);
            0
        }
        WM_HSCROLL => {
            scroll(s, false, (wp & 0xffff) as u32);
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xffff) as i16;
            scroll(
                s,
                true,
                if delta > 0 {
                    SB_LINEUP as u32
                } else {
                    SB_LINEDOWN as u32
                },
            );
            0
        }
        DM_GETDEFID => ((DC_HASDEFID as usize) << 16 | SAVE as usize) as isize,
        WM_COMMAND => {
            let id = (wp & 0xffff) as i32;
            let notification = (wp >> 16) as u32;
            if notification == EN_SETFOCUS
                || notification == CBN_SETFOCUS
                || notification == BN_SETFOCUS
            {
                ensure_visible(s, lp as HWND);
                return 0;
            }
            if id == CONNECTION_MODE && notification == CBN_SELCHANGE {
                match selected_connection_mode(s) {
                    Ok(mode) => {
                        (*s).draft.connection_mode = mode;
                        (*s).watch_connection_status = false;
                        connection_controls(s);
                        error(s, &crate::i18n::text(crate::i18n::Key::M254ACE9F6555));
                    }
                    Err(message) => error(s, &message),
                }
                return 0;
            }
            if id == TARGET && notification == CBN_SELCHANGE {
                choose_target(s);
                return 0;
            }
            if id == CHANNEL_TABS && notification == CBN_SELCHANGE {
                choose_channel_tab(s);
                return 0;
            }
            if notification == EN_CHANGE
                && matches!(
                    id,
                    OPACITY
                        | LETTER_SPACING
                        | LINE_SPACING
                        | MESSAGE_SPACING
                        | AUTHOR_GAP
                        | OUTLINE_WIDTH
                        | TRANSLATION_SIZE
                        | SECONDS
                )
            {
                if (*s).created {
                    SetTimer(hwnd, PREVIEW_TIMER, 120, None);
                }
                return 0;
            }
            // Buttons opt into focus notifications for keyboard scrolling;
            // only a click (including Enter/Space activation) invokes actions.
            if notification != BN_CLICKED {
                return 0;
            }
            match id {
                SAVE | 1 => match save(s) {
                    Ok(()) => {
                        DestroyWindow(hwnd);
                    }
                    Err(message) => error(s, &message),
                },
                CANCEL | 2 => {
                    DestroyWindow(hwnd);
                }
                CONNECT_DISCORD => launch_connection_setup(s),
                SHARE_CONNECTION => launch_share_code(s),
                EDIT_CONNECTION | EDIT_CHANNEL_TAB => launch_channel_editor(s, false),
                ADD_CHANNEL_TAB => launch_channel_editor(s, true),
                REMOVE_CHANNEL_TAB => remove_channel_tab(s),
                TRANSLATION_SETTINGS => launch_translation_settings(s),
                SEND_TIMING => {
                    if let Some(timing) = (*(*s).app).client.last_send_timing() {
                        MessageBoxW(
                            hwnd,
                            wide(&timing.details()).as_ptr(),
                            wide(&crate::i18n::text(crate::i18n::Key::M7055131BB267)).as_ptr(),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                TRANSLATION_TIMING => {
                    if let Some(timing) = (*(*s).app).client.last_translation_timing() {
                        MessageBoxW(
                            hwnd,
                            wide(&timing.details()).as_ptr(),
                            wide(&crate::i18n::text(crate::i18n::Key::M0AD9CBFE4D99)).as_ptr(),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                LOGOUT_DISCORD => {
                    if let Err(message) = logout_discord(s) {
                        error(s, &message);
                        set_text(GetDlgItem(hwnd, CONNECTION_STATUS), &message);
                    }
                }
                FONT => choose_font(s),
                FOREGROUND | AUTHOR_COLOR | BACKGROUND | OUTLINE_COLOR | TRANSLATION_COLOR => {
                    choose_color(s, id)
                }
                TEXT_OUTLINE => {
                    outline_controls(s);
                    refresh_preview(s);
                }
                TRANSLATION_STYLE => {
                    translation_style_controls(s);
                    refresh_preview(s);
                }
                AUTOHIDE | SHOW_DIAGNOSTICS => refresh_preview(s),
                REFRESH_APPS => refresh_apps(s),
                _ => {}
            }
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let dc = wp as HDC;
            SetTextColor(dc, GetSysColor(COLOR_BTNTEXT));
            SetBkColor(dc, GetSysColor(COLOR_BTNFACE));
            (*s).brush as isize
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let dc = wp as HDC;
            SetTextColor(dc, GetSysColor(COLOR_WINDOWTEXT));
            SetBkColor(dc, GetSysColor(COLOR_WINDOW));
            (*s).edit_brush as isize
        }
        WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            DeleteObject((*s).brush as _);
            DeleteObject((*s).edit_brush as _);
            (*s).brush = CreateSolidBrush(GetSysColor(COLOR_BTNFACE));
            (*s).edit_brush = CreateSolidBrush(GetSysColor(COLOR_WINDOW));
            RedrawWindow(
                hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_ERASEBKGND => {
            let mut r: RECT = zeroed();
            GetClientRect(hwnd, &mut r);
            FillRect(wp as HDC, &r, (*s).brush);
            1
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_NCDESTROY => {
            KillTimer(hwnd, SETTINGS_TIMER);
            KillTimer(hwnd, PREVIEW_TIMER);
            let app = (*s).app;
            (*app).settings_hwnd = null_mut();
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let created = (*s).created;
            let result = DefWindowProcW(hwnd, msg, wp, lp);
            if created {
                clear_preview(app);
                cleanup(s);
                settings_session(app, false);
                if !(*app).closing && IsWindow((*app).hwnd) != 0 {
                    set_mode(app, Mode::Manage);
                    SetForegroundWindow((*app).hwnd);
                }
            }
            result
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
