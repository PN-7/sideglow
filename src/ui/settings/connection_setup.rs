// SPDX-License-Identifier: MPL-2.0
//! Guided native setup. Website preparation and local login are separate pages.
//! The modal message loop continues dispatching the main UI's network-event timer.
use super::*;
use crate::{connection_profile, discord_auth::REDIRECT_URI};
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};
use windows_sys::Win32::{
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
    },
    UI::Shell::ShellExecuteW,
};

const SETUP_CLASS: &str = "Sideglow.ConnectionSetup.v1";
const TIMER_ID: usize = 19;
const WIDTH: i32 = 660;
const HEIGHT: i32 = 606;
const BACK: i32 = 301;
const NEXT: i32 = 302;
const OWNER_FLOW: i32 = 303;
const FRIEND_FLOW: i32 = 304;
const PORTAL: i32 = 305;
const OAUTH_PAGE: i32 = 306;
const COPY_CALLBACK: i32 = 307;
const OPEN_DISCORD: i32 = 308;
const LOGIN: i32 = 309;
const VERIFY_SEND: i32 = 310;
const STOP_CONNECTING: i32 = 311;
const IMPORT_PROFILE: i32 = 312;
const EXPORT_PROFILE: i32 = 313;
const IMPORT_CODE: i32 = 314;
const EXPORT_CODE: i32 = 315;
const APP_ID_EDIT: i32 = 320;
const CHANNEL_EDIT: i32 = 321;
const WEBHOOK_EDIT: i32 = 322;
const CALLBACK_EDIT: i32 = 323;
const PORTAL_CONFIRMED: i32 = 324;
const DETAIL: i32 = 325;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Choose,
    App,
    OAuth,
    Receive,
    Send,
    Friend,
}

struct Setup {
    app: *mut App,
    hwnd: HWND,
    dpi: u32,
    font: HFONT,
    heading_font: HFONT,
    brush: HBRUSH,
    edit_brush: HBRUSH,
    controls: Vec<ControlLayout>,
    draft: Settings,
    page: Page,
    scroll_x: i32,
    scroll_y: i32,
    rebuilding: bool,
    portal_confirmed: bool,
    waiting: bool,
    watch_status: bool,
    last_status: String,
    detail: String,
    previously_ready: bool,
    local_detail: bool,
    retry_after: Option<Instant>,
}

pub(super) unsafe fn register(instance: HINSTANCE) -> Result<(), String> {
    let name = wide(SETUP_CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    if RegisterClassW(&class) == 0 {
        return Err(crate::i18n::text(crate::i18n::Key::M0E53B3AB7C7A));
    }
    Ok(())
}

pub(super) unsafe fn run(parent: *mut Dialog) {
    let app = (*parent).app;
    let owner = (*parent).hwnd;
    let dpi = (*parent).dpi;
    let mut state = Box::new(Setup {
        app,
        hwnd: null_mut(),
        dpi,
        font: settings_font(dpi, false),
        heading_font: settings_font(dpi, true),
        brush: CreateSolidBrush(GetSysColor(COLOR_BTNFACE)),
        edit_brush: CreateSolidBrush(GetSysColor(COLOR_WINDOW)),
        controls: Vec::new(),
        draft: (*parent).draft.clone(),
        page: Page::Choose,
        scroll_x: 0,
        scroll_y: 0,
        rebuilding: false,
        portal_confirmed: false,
        waiting: false,
        watch_status: false,
        last_status: String::new(),
        detail: String::new(),
        previously_ready: false,
        local_detail: false,
        retry_after: None,
    });
    let ptr: *mut Setup = &mut *state;
    let mut bounds: RECT = zeroed();
    let mut work: RECT = zeroed();
    GetWindowRect(owner, &mut bounds);
    SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
    let width = scale(WIDTH + 38, dpi).min(work.right - work.left);
    let height = scale(HEIGHT + 48, dpi).min(work.bottom - work.top);
    let x = ((bounds.left + bounds.right - width) / 2)
        .clamp(work.left, (work.right - width).max(work.left));
    let y = ((bounds.top + bounds.bottom - height) / 2)
        .clamp(work.top, (work.bottom - height).max(work.top));
    let hwnd = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        wide(SETUP_CLASS).as_ptr(),
        wide(&crate::i18n::text(crate::i18n::Key::M1701C89AFB69)).as_ptr(),
        WS_OVERLAPPED
            | WS_CAPTION
            | WS_SYSMENU
            | WS_THICKFRAME
            | WS_VSCROLL
            | WS_HSCROLL
            | WS_CLIPCHILDREN,
        x,
        y,
        width,
        height,
        owner,
        null_mut(),
        GetModuleHandleW(null()),
        ptr.cast(),
    );
    if hwnd.is_null() {
        super::error(parent, &crate::i18n::text(crate::i18n::Key::M819E0BDC90AA));
        release_objects(ptr);
        return;
    }
    EnableWindow(owner, 0);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(GetDlgItem(hwnd, OWNER_FLOW));
    let mut message: MSG = zeroed();
    while IsWindow(hwnd) != 0 {
        let result = GetMessageW(&mut message, null_mut(), 0, 0);
        if result == 0 {
            PostQuitMessage(message.wParam as i32);
            break;
        }
        if result < 0 {
            break;
        }
        if IsDialogMessageW(hwnd, &message) == 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    if IsWindow(hwnd) != 0 {
        DestroyWindow(hwnd);
    }
    // The parent/main close path can run inside this modal loop. Never dereference
    // the parent's allocation after its HWND has been destroyed.
    if IsWindow(owner) != 0 {
        (*parent)
            .draft
            .discord_app_id
            .clone_from(&state.draft.discord_app_id);
        (*parent)
            .draft
            .channel_url
            .clone_from(&state.draft.channel_url);
        (*parent)
            .draft
            .webhook_url
            .clone_from(&state.draft.webhook_url);
        (*parent).draft.connection_mode = ConnectionMode::Rpc;
        (*parent).draft.translation.enabled = state.draft.translation.enabled;
        (*parent)
            .draft
            .channel_tabs
            .clone_from(&state.draft.channel_tabs);
        (*parent).draft.active_channel_tab = state.draft.active_channel_tab;
        SendMessageW(GetDlgItem(owner, CONNECTION_MODE), CB_SETCURSEL, 0, 0);
        EnableWindow(owner, 1);
        SetForegroundWindow(owner);
        SetFocus(GetDlgItem(owner, CONNECT_DISCORD));
    }
    release_objects(ptr);
}

unsafe fn release_objects(s: *mut Setup) {
    DeleteObject((*s).font as _);
    DeleteObject((*s).heading_font as _);
    DeleteObject((*s).brush as _);
    DeleteObject((*s).edit_brush as _);
}

#[allow(clippy::too_many_arguments)]
unsafe fn item(
    s: *mut Setup,
    class: &str,
    value: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    style: u32,
    heading: bool,
) -> HWND {
    let hwnd = CreateWindowExW(
        if class == "EDIT" { WS_EX_CLIENTEDGE } else { 0 },
        wide(class).as_ptr(),
        wide(value).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        scale(x, (*s).dpi),
        scale(y, (*s).dpi),
        scale(w, (*s).dpi),
        scale(h, (*s).dpi),
        (*s).hwnd,
        id as usize as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(
        hwnd,
        WM_SETFONT,
        if heading {
            (*s).heading_font
        } else {
            (*s).font
        } as usize,
        0,
    );
    (*s).controls.push(ControlLayout {
        hwnd,
        x,
        y,
        width: w,
        height: h,
        heading,
    });
    hwnd
}
unsafe fn paragraph(s: *mut Setup, value: &str, y: i32, h: i32) {
    item(s, "STATIC", value, 0, 22, y, 616, h, SS_NOPREFIX, false);
}
unsafe fn action(s: *mut Setup, value: &str, id: i32, x: i32, y: i32, w: i32) {
    item(
        s,
        "BUTTON",
        value,
        id,
        x,
        y,
        w,
        32,
        WS_TABSTOP | BS_NOTIFY as u32 | BS_PUSHBUTTON as u32,
        false,
    );
}
unsafe fn entry(s: *mut Setup, value: &str, id: i32, y: i32, secret: bool) {
    let hwnd = item(
        s,
        "EDIT",
        value,
        id,
        22,
        y,
        616,
        28,
        WS_TABSTOP | ES_AUTOHSCROLL as u32 | if secret { ES_PASSWORD as u32 } else { 0 },
        false,
    );
    SendMessageW(
        hwnd,
        EM_SETLIMITTEXT,
        if id == APP_ID_EDIT { 20 } else { 2048 },
        0,
    );
}
unsafe fn detail_box(s: *mut Setup, y: i32, h: i32) {
    item(
        s,
        "EDIT",
        &(*s).detail.clone(),
        DETAIL,
        22,
        y,
        616,
        h,
        WS_TABSTOP | WS_VSCROLL | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_READONLY as u32,
        false,
    );
}

unsafe fn render(s: *mut Setup) {
    (*s).rebuilding = true;
    let old = std::mem::take(&mut (*s).controls);
    for control in old {
        DestroyWindow(control.hwnd);
    }
    let title = match (*s).page {
        Page::Choose => crate::i18n::text(crate::i18n::Key::MA51E93BEA17F),
        Page::App => crate::i18n::text(crate::i18n::Key::M4DE692E74430),
        Page::OAuth => crate::i18n::text(crate::i18n::Key::MA8D006B8FC6E),
        Page::Receive => crate::i18n::text(crate::i18n::Key::MB37A2DD4F983),
        Page::Send => crate::i18n::text(crate::i18n::Key::M623B2A14F372),
        Page::Friend => crate::i18n::text(crate::i18n::Key::MFAD980A5F169),
    };
    item(s, "STATIC", &title, 0, 22, 18, 616, 28, SS_NOPREFIX, true);
    match (*s).page {
        Page::Choose => {
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M78B3880E4028),
                60,
                54,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::ME31876C52DD9),
                OWNER_FLOW,
                22,
                139,
                616,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M340F4E5EAC84),
                184,
                58,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::MFAD980A5F169),
                FRIEND_FLOW,
                22,
                272,
                616,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MB70376F35171),
                317,
                58,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MFC96DBAB3774),
                418,
                64,
            );
            if (*(*s).app).demo {
                paragraph(
                    s,
                    &crate::i18n::text(crate::i18n::Key::M521CA9454BA1),
                    500,
                    40,
                );
            }
        }
        Page::App => {
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MC82BC412F2F6),
                60,
                54,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M09FD299F92EA),
                PORTAL,
                22,
                128,
                260,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M50B589A7E97E),
                179,
                92,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M0704943039D9),
                294,
                24,
            );
            entry(
                s,
                &(*s).draft.discord_app_id.clone(),
                APP_ID_EDIT,
                323,
                false,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MA7BE8B236B2E),
                371,
                56,
            );
            detail_box(s, 453, 70);
        }
        Page::OAuth => {
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MD2E877776113),
                60,
                54,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::MACA013A66DC8),
                OAUTH_PAGE,
                22,
                128,
                326,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M8225FF86A7B7),
                177,
                56,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M75F344EB0DA9),
                249,
                24,
            );
            item(
                s,
                "EDIT",
                REDIRECT_URI,
                CALLBACK_EDIT,
                22,
                280,
                468,
                28,
                WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_READONLY as u32,
                false,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M46FD386D65AB),
                COPY_CALLBACK,
                502,
                278,
                136,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MDB1287926E5B),
                330,
                56,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M9B2E297BD156),
                402,
                56,
            );
            let confirm = item(
                s,
                "BUTTON",
                &crate::i18n::text(crate::i18n::Key::M19622B4A6547),
                PORTAL_CONFIRMED,
                22,
                474,
                616,
                32,
                WS_TABSTOP | BS_AUTOCHECKBOX as u32 | BS_NOTIFY as u32,
                false,
            );
            SendMessageW(
                confirm,
                BM_SETCHECK,
                if (*s).portal_confirmed {
                    BST_CHECKED as usize
                } else {
                    BST_UNCHECKED as usize
                },
                0,
            );
            detail_box(s, 514, 35);
        }
        Page::Receive => {
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M91B2387C67A7),
                60,
                54,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::MFFE480DB0D31),
                OPEN_DISCORD,
                22,
                128,
                220,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M36F6B21BB57F),
                177,
                24,
            );
            entry(s, &(*s).draft.channel_url.clone(), CHANNEL_EDIT, 208, false);
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MC3ACDEEA56D2),
                255,
                56,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M96BD500F8587),
                LOGIN,
                22,
                327,
                402,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M5AC3E33AFE8C),
                STOP_CONNECTING,
                438,
                327,
                200,
            );
            detail_box(s, 378, 142);
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M33199F35F4BA),
                531,
                22,
            );
        }
        Page::Send => {
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MB15469518816),
                60,
                54,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::MFFE480DB0D31),
                OPEN_DISCORD,
                22,
                128,
                220,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M182C06196861),
                177,
                56,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M32FB0E2E194C),
                249,
                24,
            );
            entry(s, &(*s).draft.webhook_url.clone(), WEBHOOK_EDIT, 280, true);
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M8894BB4DE3C1),
                VERIFY_SEND,
                22,
                327,
                352,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M5AC3E33AFE8C),
                STOP_CONNECTING,
                388,
                327,
                250,
            );
            detail_box(s, 378, 106);
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M6FC56A96BE5B),
                EXPORT_CODE,
                22,
                501,
                306,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::MAFEF9BC1D47C),
                EXPORT_PROFILE,
                342,
                501,
                296,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M69D630A9B827),
                539,
                22,
            );
        }
        Page::Friend => {
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M00B9B93501CF),
                60,
                36,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M6FB105A2D7E2),
                IMPORT_CODE,
                22,
                106,
                306,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M40CFB3FB2B5C),
                IMPORT_PROFILE,
                342,
                106,
                296,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::MF77EE839D140),
                154,
                22,
            );
            entry(
                s,
                &(*s).draft.discord_app_id.clone(),
                APP_ID_EDIT,
                180,
                false,
            );
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M3554FCDE2C66),
                219,
                22,
            );
            entry(s, &(*s).draft.channel_url.clone(), CHANNEL_EDIT, 245, false);
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M8CDFC006144A),
                284,
                22,
            );
            entry(s, &(*s).draft.webhook_url.clone(), WEBHOOK_EDIT, 310, true);
            paragraph(
                s,
                &crate::i18n::text(crate::i18n::Key::M7419380E656C),
                356,
                22,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M124C96D131AF),
                LOGIN,
                22,
                378,
                374,
            );
            action(
                s,
                &crate::i18n::text(crate::i18n::Key::M5AC3E33AFE8C),
                STOP_CONNECTING,
                410,
                378,
                228,
            );
            detail_box(s, 430, 106);
        }
    }
    if (*s).page != Page::Choose {
        action(
            s,
            &crate::i18n::text(crate::i18n::Key::M36B75802D60D),
            BACK,
            22,
            568,
            100,
        );
    }
    if matches!(
        (*s).page,
        Page::App | Page::OAuth | Page::Receive | Page::Send | Page::Friend
    ) {
        let next = match (*s).page {
            Page::App => crate::i18n::text(crate::i18n::Key::M602010975E59),
            Page::OAuth => crate::i18n::text(crate::i18n::Key::M9E3B9CF17B04),
            Page::Receive => crate::i18n::text(crate::i18n::Key::MA7F1B96BA38C),
            _ => crate::i18n::text(crate::i18n::Key::M74F1B559839D),
        };
        action(s, &next, NEXT, 312, 568, 200);
    }
    action(
        s,
        &crate::i18n::text(crate::i18n::Key::MABDC65F17FDE),
        2,
        526,
        568,
        112,
    );
    (*s).rebuilding = false;
    ranges(s);
    position(s);
    update_buttons(s);
    InvalidateRect((*s).hwnd, null(), 1);
}

unsafe fn capture(s: *mut Setup) {
    if (*s).rebuilding {
        return;
    }
    let before = (
        (*s).draft.discord_app_id.clone(),
        (*s).draft.channel_url.clone(),
        (*s).draft.webhook_url.clone(),
    );
    match (*s).page {
        Page::App => {
            (*s).draft.discord_app_id = text(GetDlgItem((*s).hwnd, APP_ID_EDIT)).trim().into()
        }
        Page::OAuth => (*s).portal_confirmed = checked(GetDlgItem((*s).hwnd, PORTAL_CONFIRMED)),
        Page::Receive => {
            (*s).draft.channel_url = text(GetDlgItem((*s).hwnd, CHANNEL_EDIT)).trim().into()
        }
        Page::Send => {
            (*s).draft.webhook_url = text(GetDlgItem((*s).hwnd, WEBHOOK_EDIT)).trim().into()
        }
        Page::Friend => {
            (*s).draft.discord_app_id = text(GetDlgItem((*s).hwnd, APP_ID_EDIT)).trim().into();
            (*s).draft.channel_url = text(GetDlgItem((*s).hwnd, CHANNEL_EDIT)).trim().into();
            (*s).draft.webhook_url = text(GetDlgItem((*s).hwnd, WEBHOOK_EDIT)).trim().into();
        }
        _ => {}
    }
    if before.0 != (*s).draft.discord_app_id
        || before.1 != (*s).draft.channel_url
        || before.2 != (*s).draft.webhook_url
    {
        (*s).draft.translation.enabled = false;
    }
    if before.0 != (*s).draft.discord_app_id {
        for tab in &mut (*s).draft.channel_tabs {
            tab.translation_enabled = false;
        }
    }
}
unsafe fn go(s: *mut Setup, page: Page) {
    capture(s);
    cancel_pending(s);
    (*s).page = page;
    (*s).scroll_x = 0;
    (*s).scroll_y = 0;
    (*s).watch_status = false;
    (*s).previously_ready = false;
    (*s).local_detail = false;
    (*s).detail.clear();
    (*s).last_status.clear();
    render(s);
    let focus = match page {
        Page::App => APP_ID_EDIT,
        Page::OAuth => OAUTH_PAGE,
        Page::Receive => CHANNEL_EDIT,
        Page::Send => WEBHOOK_EDIT,
        Page::Friend => IMPORT_CODE,
        Page::Choose => OWNER_FLOW,
    };
    SetFocus(GetDlgItem((*s).hwnd, focus));
    poll(s);
}
unsafe fn same_receiver(s: *mut Setup) -> bool {
    let saved = &(*(*s).app).settings;
    saved.connection_mode == ConnectionMode::Rpc
        && saved.discord_app_id.trim() == (*s).draft.discord_app_id.trim()
        && config::channel_id(&saved.channel_url)
            .ok()
            .is_some_and(|id| {
                config::channel_id(&(*s).draft.channel_url).ok().as_deref() == Some(id.as_str())
            })
}
unsafe fn receive_ready(s: *mut Setup) -> bool {
    same_receiver(s) && (*(*s).app).client.is_discord_receive_ready()
}
unsafe fn send_ready(s: *mut Setup) -> bool {
    receive_ready(s)
        && !(*s).draft.webhook_url.trim().is_empty()
        && (*(*s).app).settings.webhook_url.trim() == (*s).draft.webhook_url.trim()
        && (*(*s).app).client.is_discord_send_ready()
}
unsafe fn friend_ready(s: *mut Setup) -> bool {
    receive_ready(s)
        && if (*s).draft.webhook_url.trim().is_empty() {
            (*(*s).app).settings.webhook_url.trim().is_empty()
        } else {
            send_ready(s)
        }
}
unsafe fn update_buttons(s: *mut Setup) {
    capture(s);
    let live = !(*(*s).app).demo;
    let cooling = (*s).retry_after.is_some_and(|at| Instant::now() < at);
    let idle = !(*s).waiting && !cooling;
    let app_valid = config::snowflake((*s).draft.discord_app_id.trim());
    let channel_valid = config::channel_id(&(*s).draft.channel_url).is_ok();
    let next = match (*s).page {
        Page::App => app_valid,
        Page::OAuth => (*s).portal_confirmed,
        Page::Receive => receive_ready(s) && idle,
        Page::Send => send_ready(s) && idle,
        Page::Friend => friend_ready(s) && idle,
        Page::Choose => false,
    };
    EnableWindow(GetDlgItem((*s).hwnd, NEXT), i32::from(next));
    EnableWindow(
        GetDlgItem((*s).hwnd, LOGIN),
        i32::from(
            live && idle
                && app_valid
                && channel_valid
                && ((*s).page != Page::Friend
                    || connection_profile::Profile::from_manual(
                        &(*s).draft.discord_app_id,
                        &(*s).draft.channel_url,
                        &(*s).draft.webhook_url,
                    )
                    .is_ok()),
        ),
    );
    EnableWindow(
        GetDlgItem((*s).hwnd, VERIFY_SEND),
        i32::from(
            live && idle && app_valid && channel_valid && !(*s).draft.webhook_url.trim().is_empty(),
        ),
    );
    EnableWindow(
        GetDlgItem((*s).hwnd, EXPORT_PROFILE),
        i32::from(live && idle && send_ready(s)),
    );
    EnableWindow(
        GetDlgItem((*s).hwnd, EXPORT_CODE),
        i32::from(
            idle && connection_profile::Profile::from_manual(
                &(*s).draft.discord_app_id,
                &(*s).draft.channel_url,
                &(*s).draft.webhook_url,
            )
            .is_ok(),
        ),
    );
    EnableWindow(
        GetDlgItem((*s).hwnd, STOP_CONNECTING),
        i32::from((*s).waiting),
    );
    for id in [
        APP_ID_EDIT,
        CHANNEL_EDIT,
        WEBHOOK_EDIT,
        IMPORT_PROFILE,
        IMPORT_CODE,
    ] {
        EnableWindow(GetDlgItem((*s).hwnd, id), i32::from(!(*s).waiting));
    }
}
unsafe fn say(s: *mut Setup, value: &str, watch: bool) {
    (*s).detail = value.into();
    (*s).watch_status = watch;
    (*s).local_detail = !watch;
    set_text(GetDlgItem((*s).hwnd, DETAIL), value);
}
unsafe fn poll(s: *mut Setup) {
    if (*s).rebuilding {
        return;
    }
    capture(s);
    let app = (*s).app;
    let matching_credentials = same_receiver(s)
        && (!matches!((*s).page, Page::Send | Page::Friend)
            || (*app).settings.webhook_url.trim() == (*s).draft.webhook_url.trim());
    let ready = match (*s).page {
        Page::Receive => receive_ready(s),
        Page::Send => send_ready(s),
        Page::Friend => friend_ready(s),
        _ => false,
    };
    if (*s).previously_ready != ready && matching_credentials {
        (*s).local_detail = false;
    }
    if (*s).previously_ready && !ready && matching_credentials {
        (*s).watch_status = true;
        (*s).last_status.clear();
    }
    (*s).previously_ready = ready;
    if (*s).waiting && !(*app).client.is_discord_connecting() {
        (*s).waiting = false;
    }
    let status = if (*s).local_detail {
        None
    } else if (*app).demo {
        Some(crate::i18n::text(crate::i18n::Key::MFB0832DBC345))
    } else if (*s).page == Page::Receive && receive_ready(s) {
        Some(crate::i18n::text(crate::i18n::Key::MD7BF1EA883BF))
    } else if (*s).page == Page::Friend
        && friend_ready(s)
        && (*s).draft.webhook_url.trim().is_empty()
    {
        Some(crate::i18n::text(crate::i18n::Key::MF2509618E72D))
    } else if matches!((*s).page, Page::Send | Page::Friend) && send_ready(s) {
        Some(crate::i18n::text(crate::i18n::Key::M5B97F23F1B66))
    } else if (*s).watch_status {
        Some((*app).status.clone())
    } else {
        None
    };
    if let Some(status) = status {
        if status != (*s).last_status {
            (*s).last_status = status.clone();
            (*s).detail = status.clone();
            set_text(GetDlgItem((*s).hwnd, DETAIL), &status);
        }
    }
    update_buttons(s);
}

unsafe fn connection_settings(s: *mut Setup, read_only: bool) -> Result<Settings, String> {
    let mut settings = (*(*s).app).settings.clone();
    // The wizard changes connection state deliberately, but must retain the
    // caller's draft tab collection/selection. Global API and appearance drafts
    // stay outside this explicit login/connection commit.
    settings.channel_tabs.clone_from(&(*s).draft.channel_tabs);
    settings.active_channel_tab = (*s).draft.active_channel_tab;
    settings.connection_mode = ConnectionMode::Rpc;
    settings.discord_app_id = (*s).draft.discord_app_id.trim().into();
    settings.channel_url = (*s).draft.channel_url.trim().into();
    settings.webhook_url = if read_only {
        String::new()
    } else {
        (*s).draft.webhook_url.trim().into()
    };
    settings.translation.enabled = (*s).draft.translation.enabled;
    let old = &(*(*s).app).settings;
    if old.discord_app_id != settings.discord_app_id {
        for tab in &mut settings.channel_tabs {
            tab.translation_enabled = false;
        }
    }
    if old.connection_mode != settings.connection_mode
        || old.discord_app_id != settings.discord_app_id
        || old.channel_url != settings.channel_url
        || old.webhook_url != settings.webhook_url
    {
        settings.translation.enabled = false;
    }
    settings.sync_active_tab()?;
    Ok(settings)
}
unsafe fn begin_connection(s: *mut Setup, login: bool) -> Result<(), String> {
    if (*(*s).app).demo {
        return Err(demo_connection_notice());
    }
    if (*s).waiting || (*s).retry_after.is_some_and(|at| Instant::now() < at) {
        return Err(crate::i18n::text(crate::i18n::Key::M3888CEA603AD));
    }
    capture(s);
    if (*s).page == Page::Friend {
        let profile = connection_profile::Profile::from_manual(
            &(*s).draft.discord_app_id,
            &(*s).draft.channel_url,
            &(*s).draft.webhook_url,
        )?;
        (*s).draft.discord_app_id = profile.discord_app_id;
        (*s).draft.channel_url = profile.channel_url;
        (*s).draft.webhook_url = profile.webhook_url;
        fill_manual_fields(s);
    }
    let settings = connection_settings(s, (*s).page == Page::Receive)?;
    config::validate(&settings)?;
    let app = (*s).app;
    commit_settings(app, settings)?;
    (*s).draft.translation.enabled = (*app).settings.translation.enabled;
    (*s).draft
        .channel_tabs
        .clone_from(&(*app).settings.channel_tabs);
    (*s).draft.active_channel_tab = (*app).settings.active_channel_tab;
    if login {
        (*app).client.login_discord(&(*app).settings)?;
    } else {
        (*app).client.reconnect_discord(&(*app).settings)?;
    }
    (*s).waiting = true;
    (*s).last_status = (*app).status.clone();
    say(
        s,
        &if login {
            crate::i18n::text(crate::i18n::Key::M3566A434712E)
        } else {
            crate::i18n::text(crate::i18n::Key::M0F2588998B59)
        },
        true,
    );
    update_buttons(s);
    Ok(())
}
unsafe fn cancel_pending(s: *mut Setup) {
    if (*s).waiting {
        (*(*s).app).client.cancel_discord_login();
        (*s).waiting = false;
        (*s).watch_status = false;
        (*s).retry_after = Some(Instant::now() + Duration::from_millis(2500));
    }
}
unsafe fn next_page(s: *mut Setup) -> Result<(), String> {
    capture(s);
    match (*s).page {
        Page::App => {
            if !config::snowflake((*s).draft.discord_app_id.trim()) {
                return Err(crate::i18n::text(crate::i18n::Key::M5FBDD237CD7F));
            }
            go(s, Page::OAuth);
        }
        Page::OAuth => {
            if !(*s).portal_confirmed {
                return Err(crate::i18n::text(crate::i18n::Key::MCAEACC49D78A));
            }
            go(s, Page::Receive);
        }
        Page::Receive => {
            if !receive_ready(s) || (*s).waiting {
                return Err(crate::i18n::text(crate::i18n::Key::MD6A19C4BB50A));
            }
            go(s, Page::Send);
        }
        Page::Send | Page::Friend => {
            let ready = if (*s).page == Page::Friend {
                friend_ready(s)
            } else {
                send_ready(s)
            };
            if !ready || (*s).waiting {
                return Err(crate::i18n::text(crate::i18n::Key::M32309C5AB60A));
            }
            DestroyWindow((*s).hwnd);
        }
        Page::Choose => {}
    }
    Ok(())
}

unsafe fn open_site(s: *mut Setup, url: &str) -> Result<(), String> {
    // Only fixed Discord URLs are passed from the button dispatcher.
    let result = ShellExecuteW(
        (*s).hwnd,
        wide("open").as_ptr(),
        wide(url).as_ptr(),
        null(),
        null(),
        SW_SHOWNORMAL,
    );
    if result as usize <= 32 {
        return Err(crate::i18n::text(crate::i18n::Key::M6AABCC228901));
    }
    Ok(())
}
unsafe fn copy_callback(s: *mut Setup) -> Result<(), String> {
    let value = wide(REDIRECT_URI);
    let memory = GlobalAlloc(GMEM_MOVEABLE, value.len() * size_of::<u16>());
    if memory.is_null() {
        return Err(crate::i18n::text(crate::i18n::Key::M9996C178CEC7));
    }
    let destination = GlobalLock(memory);
    if destination.is_null() {
        GlobalFree(memory);
        return Err(crate::i18n::text(crate::i18n::Key::M998BFE422E69));
    }
    std::ptr::copy_nonoverlapping(value.as_ptr(), destination.cast::<u16>(), value.len());
    GlobalUnlock(memory);
    if OpenClipboard((*s).hwnd) == 0 {
        GlobalFree(memory);
        return Err(crate::i18n::text(crate::i18n::Key::M23D75EDEB313));
    }
    let ok = EmptyClipboard() != 0 && !SetClipboardData(13, memory).is_null(); // CF_UNICODETEXT
    CloseClipboard();
    if !ok {
        GlobalFree(memory);
        return Err(crate::i18n::text(crate::i18n::Key::MB05DA440268A));
    }
    say(
        s,
        &crate::i18n::text(crate::i18n::Key::MC34815B3A7C5),
        false,
    );
    Ok(())
}
unsafe fn choose_file(s: *mut Setup, save: bool) -> Result<Option<PathBuf>, String> {
    let mut filename = vec![0u16; 32768];
    if save {
        let default = wide(connection_profile::DEFAULT_FILE_NAME);
        filename[..default.len()].copy_from_slice(&default);
    }
    let filter = wide(&format!(
        "{}\0*.sideglow.json;*.whisper.json\0{}\0*.json\0",
        crate::i18n::text(crate::i18n::Key::M16276EE3E2F9),
        crate::i18n::text(crate::i18n::Key::M0E605A7DFA26)
    ));
    let title = wide(&if save {
        crate::i18n::text(crate::i18n::Key::MD4839F6F6441)
    } else {
        crate::i18n::text(crate::i18n::Key::MF965918188A8)
    });
    let extension = wide("sideglow.json");
    let mut dialog: OPENFILENAMEW = zeroed();
    dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = (*s).hwnd;
    dialog.lpstrFile = filename.as_mut_ptr();
    dialog.nMaxFile = filename.len() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.nFilterIndex = 1;
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.Flags = OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };
    let chosen = if save {
        GetSaveFileNameW(&mut dialog)
    } else {
        GetOpenFileNameW(&mut dialog)
    };
    if chosen == 0 {
        return if CommDlgExtendedError() == 0 {
            Ok(None)
        } else {
            Err(crate::i18n::text(crate::i18n::Key::M06ED8E7FE2B8))
        };
    }
    let end = filename
        .iter()
        .position(|v| *v == 0)
        .unwrap_or(filename.len());
    Ok(Some(PathBuf::from(OsString::from_wide(&filename[..end]))))
}
unsafe fn import_profile(s: *mut Setup) -> Result<(), String> {
    if (*s).waiting {
        return Err(crate::i18n::text(crate::i18n::Key::M6987D76520A1));
    }
    let owner = (*s).hwnd;
    let Some(path) = choose_file(s, false)? else {
        return Ok(());
    };
    if IsWindow(owner) == 0 {
        return Ok(());
    }
    let profile = connection_profile::import(&path)?;
    (*s).draft = super::imported_connection_settings(&(*s).draft, &profile)?;
    fill_manual_fields(s);
    say(
        s,
        &crate::i18n::text(crate::i18n::Key::M0C09DB5F92BF),
        false,
    );
    update_buttons(s);
    Ok(())
}
unsafe fn exchange_code(s: *mut Setup, purpose: share_code::Purpose) -> Result<(), String> {
    if (*s).waiting {
        return Err(crate::i18n::text(crate::i18n::Key::MB3F55CD1C685));
    }
    capture(s);
    let owner = (*s).hwnd;
    let result = share_code::run(owner, (*s).dpi, &(*s).draft, purpose);
    if IsWindow(owner) == 0 {
        return Ok(());
    }
    if let Some(profile) = result {
        (*s).draft = super::imported_connection_settings(&(*s).draft, &profile)?;
        fill_manual_fields(s);
        (*s).portal_confirmed = false;
        (*s).last_status.clear();
        say(
            s,
            &crate::i18n::text(crate::i18n::Key::M40AB0BF1E806),
            false,
        );
    }
    update_buttons(s);
    SetFocus(GetDlgItem(
        owner,
        if purpose == share_code::Purpose::Share {
            EXPORT_CODE
        } else {
            IMPORT_CODE
        },
    ));
    Ok(())
}
unsafe fn fill_manual_fields(s: *mut Setup) {
    (*s).rebuilding = true;
    for (id, value) in [
        (APP_ID_EDIT, &(*s).draft.discord_app_id),
        (CHANNEL_EDIT, &(*s).draft.channel_url),
        (WEBHOOK_EDIT, &(*s).draft.webhook_url),
    ] {
        set_text(GetDlgItem((*s).hwnd, id), value);
    }
    (*s).rebuilding = false;
}
unsafe fn export_profile(s: *mut Setup) -> Result<(), String> {
    if (*(*s).app).demo {
        return Err(crate::i18n::text(crate::i18n::Key::ME6652D753F5A));
    }
    capture(s);
    if !send_ready(s) || (*s).waiting {
        return Err(crate::i18n::text(crate::i18n::Key::M5365BD8BE18C));
    }
    let result = MessageBoxW(
        (*s).hwnd,
        wide(&crate::i18n::text(crate::i18n::Key::M7CEAFCD95FA8)).as_ptr(),
        wide(&crate::i18n::text(crate::i18n::Key::M8479F9512561)).as_ptr(),
        MB_YESNO | MB_ICONINFORMATION | MB_DEFBUTTON2,
    );
    if result != IDYES {
        return Ok(());
    }
    let Some(path) = choose_file(s, true)? else {
        return Ok(());
    };
    // File dialogs keep pumping messages; recheck the connection after the user
    // picks a path instead of relying on the state before the dialog opened.
    if !send_ready(s) || (*s).waiting {
        return Err(crate::i18n::text(crate::i18n::Key::ME314C5C1CD8C));
    }
    connection_profile::export(&connection_settings(s, false)?, &path)?;
    say(
        s,
        &crate::i18n::text(crate::i18n::Key::MA8BEC3E9A2F4),
        false,
    );
    Ok(())
}

unsafe fn ranges(s: *mut Setup) {
    let mut rect: RECT = zeroed();
    GetClientRect((*s).hwnd, &mut rect);
    for (bar, content, page, old) in [
        (SB_HORZ, scale(WIDTH, (*s).dpi), rect.right, (*s).scroll_x),
        (SB_VERT, scale(HEIGHT, (*s).dpi), rect.bottom, (*s).scroll_y),
    ] {
        let pos = old.clamp(0, (content - page).max(0));
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
}
unsafe fn position(s: *mut Setup) {
    for layout in (*s).controls.clone() {
        SetWindowPos(
            layout.hwnd,
            null_mut(),
            scale(layout.x, (*s).dpi) - (*s).scroll_x,
            scale(layout.y, (*s).dpi) - (*s).scroll_y,
            scale(layout.width, (*s).dpi),
            scale(layout.height, (*s).dpi),
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}
unsafe fn scroll_to_position(s: *mut Setup, vertical: bool, pos: i32) {
    if vertical {
        (*s).scroll_y = pos;
    } else {
        (*s).scroll_x = pos;
    }
    ranges(s);
    position(s);
    InvalidateRect((*s).hwnd, null(), 1);
}
unsafe fn scroll_page(s: *mut Setup, vertical: bool, command: u32) {
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
    scroll_to_position(s, vertical, next);
}
unsafe fn focus_visible(s: *mut Setup, hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    let mut rect: RECT = zeroed();
    let mut client: RECT = zeroed();
    GetWindowRect(hwnd, &mut rect);
    MapWindowPoints(null_mut(), (*s).hwnd, (&mut rect as *mut RECT).cast(), 2);
    GetClientRect((*s).hwnd, &mut client);
    let margin = scale(8, (*s).dpi);
    let dx = if rect.left < margin {
        rect.left - margin
    } else {
        (rect.right - client.right + margin).max(0)
    };
    let dy = if rect.top < margin {
        rect.top - margin
    } else {
        (rect.bottom - client.bottom + margin).max(0)
    };
    scroll_to_position(s, false, (*s).scroll_x + dx);
    scroll_to_position(s, true, (*s).scroll_y + dy);
}
unsafe fn dpi_changed(s: *mut Setup, dpi: u32, rect: &RECT) {
    let old_font = (*s).font;
    let old_heading = (*s).heading_font;
    let old_dpi = (*s).dpi.max(1);
    (*s).dpi = dpi.max(1);
    (*s).scroll_x = ((*s).scroll_x as i64 * (*s).dpi as i64 / old_dpi as i64) as i32;
    (*s).scroll_y = ((*s).scroll_y as i64 * (*s).dpi as i64 / old_dpi as i64) as i32;
    let font = settings_font((*s).dpi, false);
    let heading = settings_font((*s).dpi, true);
    if !font.is_null() {
        (*s).font = font;
    }
    if !heading.is_null() {
        (*s).heading_font = heading;
    }
    for control in (*s).controls.clone() {
        SendMessageW(
            control.hwnd,
            WM_SETFONT,
            if control.heading {
                (*s).heading_font
            } else {
                (*s).font
            } as usize,
            0,
        );
    }
    if old_font != (*s).font {
        DeleteObject(old_font as _);
    }
    if old_heading != (*s).heading_font {
        DeleteObject(old_heading as _);
    }
    SetWindowPos(
        (*s).hwnd,
        null_mut(),
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    ranges(s);
    position(s);
    RedrawWindow(
        (*s).hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
    );
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        let s = create.lpCreateParams as *mut Setup;
        (*s).hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, s as isize);
    }
    let s = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Setup;
    if s.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_CREATE => {
            render(s);
            SetTimer(hwnd, TIMER_ID, 250, None);
            0
        }
        WM_SIZE => {
            ranges(s);
            position(s);
            0
        }
        WM_DPICHANGED => {
            dpi_changed(s, (wp & 0xffff) as u32, &*(lp as *const RECT));
            0
        }
        WM_TIMER if wp == TIMER_ID => {
            poll(s);
            0
        }
        WM_VSCROLL => {
            scroll_page(s, true, (wp & 0xffff) as u32);
            0
        }
        WM_HSCROLL => {
            scroll_page(s, false, (wp & 0xffff) as u32);
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xffff) as i16;
            scroll_page(
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
        DM_GETDEFID => ((DC_HASDEFID as usize) << 16 | NEXT as usize) as isize,
        WM_COMMAND => {
            if (*s).rebuilding {
                return 0;
            }
            let id = (wp & 0xffff) as i32;
            let notification = (wp >> 16) as u32;
            if notification == EN_SETFOCUS || notification == BN_SETFOCUS {
                focus_visible(s, lp as HWND);
                return 0;
            }
            if notification == EN_CHANGE {
                let before_app = (*s).draft.discord_app_id.clone();
                let before_channel = (*s).draft.channel_url.clone();
                let before_webhook = (*s).draft.webhook_url.clone();
                capture(s);
                if id == APP_ID_EDIT && before_app != (*s).draft.discord_app_id {
                    (*s).portal_confirmed = false;
                }
                if id == CHANNEL_EDIT && before_channel != (*s).draft.channel_url {
                    (*s).last_status.clear();
                    say(
                        s,
                        &crate::i18n::text(crate::i18n::Key::M6C3F349AFDB7),
                        false,
                    );
                }
                if id == WEBHOOK_EDIT && before_webhook != (*s).draft.webhook_url {
                    (*s).last_status.clear();
                    say(
                        s,
                        &if (*s).page == Page::Friend {
                            crate::i18n::text(crate::i18n::Key::M30554EA2CB3D)
                        } else {
                            crate::i18n::text(crate::i18n::Key::MF1213B5DD527)
                        },
                        false,
                    );
                }
                update_buttons(s);
                return 0;
            }
            if notification != BN_CLICKED {
                return 0;
            }
            let result = match id {
                2 => {
                    capture(s);
                    cancel_pending(s);
                    DestroyWindow(hwnd);
                    Ok(())
                }
                OWNER_FLOW => {
                    go(s, Page::App);
                    Ok(())
                }
                FRIEND_FLOW => {
                    go(s, Page::Friend);
                    Ok(())
                }
                BACK => {
                    let page = match (*s).page {
                        Page::App | Page::Friend => Page::Choose,
                        Page::OAuth => Page::App,
                        Page::Receive => Page::OAuth,
                        Page::Send => Page::Receive,
                        Page::Choose => Page::Choose,
                    };
                    go(s, page);
                    Ok(())
                }
                NEXT | 1 => next_page(s),
                PORTAL => open_site(s, "https://discord.com/developers/applications"),
                OAUTH_PAGE => open_site(
                    s,
                    "https://discord.com/developers/applications/select/oauth2",
                ),
                OPEN_DISCORD => open_site(s, "https://discord.com/channels/@me"),
                COPY_CALLBACK => copy_callback(s),
                LOGIN => begin_connection(s, true),
                VERIFY_SEND => begin_connection(s, false),
                STOP_CONNECTING => {
                    cancel_pending(s);
                    say(
                        s,
                        &crate::i18n::text(crate::i18n::Key::M5614CBA416A9),
                        false,
                    );
                    update_buttons(s);
                    Ok(())
                }
                IMPORT_PROFILE => import_profile(s),
                EXPORT_PROFILE => export_profile(s),
                IMPORT_CODE => exchange_code(s, share_code::Purpose::Import),
                EXPORT_CODE => exchange_code(s, share_code::Purpose::Share),
                PORTAL_CONFIRMED => {
                    capture(s);
                    update_buttons(s);
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                say(s, &e, false);
                update_buttons(s);
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
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            FillRect(wp as HDC, &rect, (*s).brush);
            1
        }
        WM_CLOSE => {
            capture(s);
            cancel_pending(s);
            DestroyWindow(hwnd);
            0
        }
        WM_NCDESTROY => {
            KillTimer(hwnd, TIMER_ID);
            cancel_pending(s);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            (*s).hwnd = null_mut();
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
