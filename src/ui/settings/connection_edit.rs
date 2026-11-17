// SPDX-License-Identifier: MPL-2.0
//! Small draft-only editor for one channel tab. No login, HTTP, persistence or
//! translation starts here. The caller applies the returned values deliberately.
use super::*;
use crate::connection_profile::Profile;

const EDIT_CLASS: &str = "Sideglow.ConnectionEdit.v1";
const WIDTH: i32 = 660;
const HEIGHT: i32 = 526;
const NAME: i32 = 701;
const CHANNEL_URL: i32 = 702;
const WEBHOOK_URL: i32 = 703;
const APP_ID: i32 = 704;
const DETAIL: i32 = 705;
const APPLY: i32 = 706;
const DISMISS: i32 = 707;

// No Debug implementation: the webhook is a sending credential.
#[derive(Clone)]
pub(super) struct EditValues {
    pub name: String,
    pub discord_app_id: String,
    pub channel_url: String,
    pub webhook_url: String,
}

struct Editor {
    hwnd: HWND,
    dpi: u32,
    font: HFONT,
    heading_font: HFONT,
    brush: HBRUSH,
    edit_brush: HBRUSH,
    controls: Vec<ControlLayout>,
    scroll_x: i32,
    scroll_y: i32,
    values: EditValues,
    adding: bool,
    accepted: bool,
}

pub(super) unsafe fn register(instance: HINSTANCE) -> Result<(), String> {
    let name = wide(EDIT_CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    if RegisterClassW(&class) == 0 {
        Err(crate::i18n::text(crate::i18n::Key::M6CD2DDCCD20E))
    } else {
        Ok(())
    }
}

pub(super) unsafe fn run(
    owner: HWND,
    dpi: u32,
    initial: EditValues,
    adding: bool,
) -> Option<EditValues> {
    let mut state = Box::new(Editor {
        hwnd: null_mut(),
        dpi,
        font: settings_font(dpi, false),
        heading_font: settings_font(dpi, true),
        brush: CreateSolidBrush(GetSysColor(COLOR_BTNFACE)),
        edit_brush: CreateSolidBrush(GetSysColor(COLOR_WINDOW)),
        controls: Vec::new(),
        scroll_x: 0,
        scroll_y: 0,
        values: initial,
        adding,
        accepted: false,
    });
    let ptr: *mut Editor = &mut *state;
    let mut bounds: RECT = zeroed();
    let mut work: RECT = zeroed();
    GetWindowRect(owner, &mut bounds);
    let monitor = MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut info) != 0 {
        work = info.rcWork;
    } else {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
    }
    let width = scale(WIDTH + 38, dpi).min((work.right - work.left).max(1));
    let height = scale(HEIGHT + 48, dpi).min((work.bottom - work.top).max(1));
    let x = ((bounds.left + bounds.right - width) / 2)
        .clamp(work.left, (work.right - width).max(work.left));
    let y = ((bounds.top + bounds.bottom - height) / 2)
        .clamp(work.top, (work.bottom - height).max(work.top));
    let hwnd = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        wide(EDIT_CLASS).as_ptr(),
        wide(&if adding {
            crate::i18n::text(crate::i18n::Key::M279BB15C76F3)
        } else {
            crate::i18n::text(crate::i18n::Key::M7DA3519EB79A)
        })
        .as_ptr(),
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
        release(ptr);
        MessageBoxW(
            owner,
            wide(&crate::i18n::text(crate::i18n::Key::M77E97C6FF50F)).as_ptr(),
            wide("Sideglow").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
        return None;
    }
    EnableWindow(owner, 0);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(GetDlgItem(hwnd, if adding { NAME } else { CHANNEL_URL }));
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
    let owner_alive = IsWindow(owner) != 0;
    if owner_alive {
        EnableWindow(owner, 1);
        SetForegroundWindow(owner);
    }
    let result = (state.accepted && owner_alive).then(|| state.values.clone());
    release(ptr);
    result
}

unsafe fn release(s: *mut Editor) {
    for object in [
        (*s).font as HGDIOBJ,
        (*s).heading_font as HGDIOBJ,
        (*s).brush as HGDIOBJ,
        (*s).edit_brush as HGDIOBJ,
    ] {
        DeleteObject(object);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn item(
    s: *mut Editor,
    class: &str,
    value: &str,
    id: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
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
        scale(width, (*s).dpi),
        scale(height, (*s).dpi),
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
        width,
        height,
        heading,
    });
    hwnd
}

unsafe fn paragraph(s: *mut Editor, value: &str, y: i32, height: i32) {
    item(
        s,
        "STATIC",
        value,
        0,
        22,
        y,
        616,
        height,
        SS_NOPREFIX,
        false,
    );
}

unsafe fn entry(s: *mut Editor, value: &str, id: i32, y: i32, secret: bool) {
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
        match id {
            NAME => 32,
            APP_ID => 20,
            _ => 2048,
        },
        0,
    );
}

unsafe fn create_controls(s: *mut Editor) {
    item(
        s,
        "STATIC",
        &if (*s).adding {
            crate::i18n::text(crate::i18n::Key::M08B3FAF7DA65)
        } else {
            crate::i18n::text(crate::i18n::Key::M8BC71065828D)
        },
        0,
        22,
        18,
        616,
        28,
        SS_NOPREFIX,
        true,
    );
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::MD11D216CD950),
        58,
        42,
    );
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M23064DAC6E44),
        110,
        22,
    );
    entry(s, &(*s).values.name.clone(), NAME, 136, false);
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M3554FCDE2C66),
        178,
        22,
    );
    entry(s, &(*s).values.channel_url.clone(), CHANNEL_URL, 204, false);
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M8CDFC006144A),
        246,
        22,
    );
    entry(s, &(*s).values.webhook_url.clone(), WEBHOOK_URL, 272, true);
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M9F82FD4B1581),
        314,
        22,
    );
    entry(s, &(*s).values.discord_app_id.clone(), APP_ID, 340, false);
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M5C70B229804F),
        380,
        42,
    );
    item(
        s,
        "STATIC",
        "",
        DETAIL,
        22,
        432,
        616,
        36,
        SS_NOPREFIX,
        false,
    );
    item(
        s,
        "BUTTON",
        &crate::i18n::text(crate::i18n::Key::MBDE76B4A9F84),
        DISMISS,
        394,
        482,
        112,
        32,
        WS_TABSTOP | BS_PUSHBUTTON as u32 | BS_NOTIFY as u32,
        false,
    );
    item(
        s,
        "BUTTON",
        &if (*s).adding {
            crate::i18n::text(crate::i18n::Key::MFDC9F0D961E8)
        } else {
            crate::i18n::text(crate::i18n::Key::M8F85233280A1)
        },
        APPLY,
        522,
        482,
        116,
        32,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32 | BS_NOTIFY as u32,
        false,
    );
    ranges(s);
    position(s);
}

unsafe fn apply(s: *mut Editor) -> Result<(), String> {
    let hwnd = (*s).hwnd;
    let name = text(GetDlgItem(hwnd, NAME)).trim().to_owned();
    config::validate_channel_tab_name(&name)?;
    let app_id = text(GetDlgItem(hwnd, APP_ID));
    let channel_url = text(GetDlgItem(hwnd, CHANNEL_URL));
    let webhook_url = text(GetDlgItem(hwnd, WEBHOOK_URL));
    let profile = Profile::from_manual(&app_id, &channel_url, &webhook_url)?;
    (*s).values = EditValues {
        name,
        discord_app_id: profile.discord_app_id,
        channel_url: profile.channel_url,
        webhook_url: profile.webhook_url,
    };
    (*s).accepted = true;
    DestroyWindow(hwnd);
    Ok(())
}

unsafe fn ranges(s: *mut Editor) {
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

unsafe fn position(s: *mut Editor) {
    for layout in (*s).controls.clone() {
        SetWindowPos(
            layout.hwnd,
            null_mut(),
            scale(layout.x, (*s).dpi) - (*s).scroll_x,
            scale(layout.y, (*s).dpi) - (*s).scroll_y,
            scale(layout.width, (*s).dpi),
            scale(layout.height, (*s).dpi),
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
        );
    }
}

unsafe fn scroll_to(s: *mut Editor, vertical: bool, value: i32) {
    if vertical {
        (*s).scroll_y = value;
    } else {
        (*s).scroll_x = value;
    }
    ranges(s);
    position(s);
    InvalidateRect((*s).hwnd, null(), 1);
}

unsafe fn scroll(s: *mut Editor, vertical: bool, command: u32) {
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

unsafe fn focus_visible(s: *mut Editor, hwnd: HWND) {
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
    scroll_to(s, false, (*s).scroll_x + dx);
    scroll_to(s, true, (*s).scroll_y + dy);
}

unsafe fn dpi_changed(s: *mut Editor, dpi: u32, rect: &RECT) {
    let old_dpi = (*s).dpi.max(1);
    (*s).dpi = dpi.max(1);
    (*s).scroll_x = ((*s).scroll_x as i64 * (*s).dpi as i64 / old_dpi as i64) as i32;
    (*s).scroll_y = ((*s).scroll_y as i64 * (*s).dpi as i64 / old_dpi as i64) as i32;
    let old_font = (*s).font;
    let old_heading = (*s).heading_font;
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
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
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
        let s = create.lpCreateParams as *mut Editor;
        (*s).hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, s as isize);
    }
    let s = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor;
    if s.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_CREATE => {
            create_controls(s);
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
        DM_GETDEFID => ((DC_HASDEFID as usize) << 16 | APPLY as usize) as isize,
        WM_COMMAND => {
            let id = (wp & 0xffff) as i32;
            let notification = (wp >> 16) as u32;
            if notification == EN_SETFOCUS || notification == BN_SETFOCUS {
                focus_visible(s, lp as HWND);
                return 0;
            }
            if notification != BN_CLICKED {
                return 0;
            }
            match id {
                APPLY | 1 => {
                    if let Err(error) = apply(s) {
                        set_text(GetDlgItem(hwnd, DETAIL), &error);
                        focus_visible(s, GetDlgItem(hwnd, DETAIL));
                    }
                }
                DISMISS | 2 => {
                    DestroyWindow(hwnd);
                }
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
        WM_CTLCOLOREDIT => {
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
            DestroyWindow(hwnd);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
