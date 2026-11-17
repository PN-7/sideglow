// SPDX-License-Identifier: MPL-2.0
//! Local connection-code exchange. No login, network request or preferences write.
//! Only an explicit Copy action changes the clipboard; accepting returns a draft.
use super::*;
use crate::connection_profile::{self, Profile, MAX_CODE_TEXT_BYTES};
use windows_sys::Win32::{
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
        },
        Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
    },
    UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
};

const CODE_CLASS: &str = "Sideglow.ShareCode.v1";
const WIDTH: i32 = 660;
const HEIGHT: i32 = 542;
const CODE: i32 = 801;
const COPY_CURRENT: i32 = 802;
const PASTE: i32 = 803;
const REVEAL: i32 = 804;
const PREVIEW: i32 = 805;
const DETAIL: i32 = 806;
const APPLY: i32 = 807;
const DISMISS: i32 = 808;
const CODE_SUBCLASS: usize = 1;
const UNICODE_TEXT: u32 = 13;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Purpose {
    Manage,
    Share,
    Import,
}
impl Purpose {
    fn can_import(self) -> bool {
        self != Self::Share
    }
    fn can_share(self) -> bool {
        self != Self::Import
    }
}

// No Debug: both the code and source settings can contain credentials.
struct CodeDialog {
    hwnd: HWND,
    dpi: u32,
    font: HFONT,
    heading_font: HFONT,
    brush: HBRUSH,
    edit_brush: HBRUSH,
    controls: Vec<ControlLayout>,
    scroll_x: i32,
    scroll_y: i32,
    source: Settings,
    purpose: Purpose,
    rebuilding: bool,
    parsed: Option<Profile>,
    accepted: bool,
}

pub(super) unsafe fn register(instance: HINSTANCE) -> Result<(), String> {
    let name = wide(CODE_CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    if RegisterClassW(&class) == 0 {
        Err(crate::i18n::text(crate::i18n::Key::MC6F4366E19CA))
    } else {
        Ok(())
    }
}

pub(super) unsafe fn run(
    owner: HWND,
    dpi: u32,
    source: &Settings,
    purpose: Purpose,
) -> Option<Profile> {
    let mut state = Box::new(CodeDialog {
        hwnd: null_mut(),
        dpi,
        font: settings_font(dpi, false),
        heading_font: settings_font(dpi, true),
        brush: CreateSolidBrush(GetSysColor(COLOR_BTNFACE)),
        edit_brush: CreateSolidBrush(GetSysColor(COLOR_WINDOW)),
        controls: Vec::new(),
        scroll_x: 0,
        scroll_y: 0,
        source: source.clone(),
        purpose,
        rebuilding: true,
        parsed: None,
        accepted: false,
    });
    let ptr: *mut CodeDialog = &mut *state;
    let mut bounds: RECT = zeroed();
    GetWindowRect(owner, &mut bounds);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    let mut work: RECT = zeroed();
    if GetMonitorInfoW(
        MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
        &mut info,
    ) != 0
    {
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
        wide(CODE_CLASS).as_ptr(),
        wide(&crate::i18n::text(crate::i18n::Key::M9EEF99491D4C)).as_ptr(),
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
            wide(&crate::i18n::text(crate::i18n::Key::M8F63B198FEF9)).as_ptr(),
            wide("Sideglow").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
        return None;
    }
    EnableWindow(owner, 0);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(GetDlgItem(
        hwnd,
        if purpose == Purpose::Import {
            PASTE
        } else {
            COPY_CURRENT
        },
    ));
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
    let result = if state.accepted && owner_alive {
        state.parsed.take()
    } else {
        None
    };
    release(ptr);
    result
}

unsafe fn release(s: *mut CodeDialog) {
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
    s: *mut CodeDialog,
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
unsafe fn action(
    s: *mut CodeDialog,
    value: &str,
    id: i32,
    x: i32,
    y: i32,
    width: i32,
    default: bool,
) {
    item(
        s,
        "BUTTON",
        value,
        id,
        x,
        y,
        width,
        32,
        WS_TABSTOP
            | BS_NOTIFY as u32
            | if default {
                BS_DEFPUSHBUTTON as u32
            } else {
                BS_PUSHBUTTON as u32
            },
        false,
    );
}

unsafe fn create_controls(s: *mut CodeDialog) -> bool {
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M9EEF99491D4C),
        0,
        22,
        18,
        616,
        28,
        SS_NOPREFIX,
        true,
    );
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M2A38FACE774A),
        0,
        22,
        58,
        616,
        58,
        SS_NOPREFIX,
        false,
    );
    if (*s).purpose.can_share() {
        action(
            s,
            &crate::i18n::text(crate::i18n::Key::M5FA3445E8237),
            COPY_CURRENT,
            22,
            126,
            if (*s).purpose == Purpose::Share {
                616
            } else {
                350
            },
            false,
        );
    }
    if (*s).purpose.can_import() {
        action(
            s,
            &crate::i18n::text(crate::i18n::Key::M0051BC203E6D),
            PASTE,
            if (*s).purpose == Purpose::Import {
                22
            } else {
                386
            },
            126,
            if (*s).purpose == Purpose::Import {
                616
            } else {
                252
            },
            false,
        );
    }
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::MA1E066C723CA),
        0,
        22,
        174,
        616,
        20,
        SS_NOPREFIX,
        false,
    );
    let code = item(
        s,
        "EDIT",
        "",
        CODE,
        22,
        200,
        616,
        30,
        WS_TABSTOP
            | ES_AUTOHSCROLL as u32
            | ES_PASSWORD as u32
            | if (*s).purpose == Purpose::Share {
                ES_READONLY as u32
            } else {
                0
            },
        false,
    );
    SendMessageW(code, EM_SETLIMITTEXT, MAX_CODE_TEXT_BYTES, 0);
    if code.is_null() || SetWindowSubclass(code, Some(code_proc), CODE_SUBCLASS, s as usize) == 0 {
        return false;
    }
    item(
        s,
        "BUTTON",
        &crate::i18n::text(crate::i18n::Key::M48E433B779D0),
        REVEAL,
        22,
        242,
        190,
        28,
        WS_TABSTOP | BS_AUTOCHECKBOX as u32 | BS_NOTIFY as u32,
        false,
    );
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M26FF1955D72F),
        0,
        22,
        286,
        616,
        22,
        SS_NOPREFIX,
        true,
    );
    item(
        s,
        "EDIT",
        &crate::i18n::text(crate::i18n::Key::M757C5BBC83CD),
        PREVIEW,
        22,
        314,
        616,
        82,
        WS_TABSTOP | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_READONLY as u32 | WS_VSCROLL,
        false,
    );
    item(
        s,
        "STATIC",
        "",
        DETAIL,
        22,
        408,
        616,
        64,
        SS_NOPREFIX,
        false,
    );
    if (*s).purpose.can_import() {
        action(
            s,
            &crate::i18n::text(crate::i18n::Key::MB3AA57E1FBB0),
            APPLY,
            342,
            492,
            176,
            true,
        );
        EnableWindow(GetDlgItem((*s).hwnd, APPLY), 0);
    }
    action(
        s,
        &if (*s).purpose == Purpose::Share {
            crate::i18n::text(crate::i18n::Key::MABDC65F17FDE)
        } else {
            crate::i18n::text(crate::i18n::Key::MBDE76B4A9F84)
        },
        DISMISS,
        530,
        492,
        108,
        (*s).purpose == Purpose::Share,
    );
    (*s).rebuilding = false;
    ranges(s);
    position(s);
    !(*s).controls.iter().any(|control| control.hwnd.is_null())
}

unsafe fn read_code(s: *mut CodeDialog) -> Result<String, String> {
    let edit = GetDlgItem((*s).hwnd, CODE);
    let length = GetWindowTextLengthW(edit);
    if length < 0 || length as usize > MAX_CODE_TEXT_BYTES {
        return Err(crate::i18n::text(crate::i18n::Key::M4A762B3366A3));
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let read = GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32).max(0) as usize;
    let value = String::from_utf16(&buffer[..read])
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MF8C125898C6D))?;
    if value.len() > MAX_CODE_TEXT_BYTES {
        Err(crate::i18n::text(crate::i18n::Key::M4A762B3366A3))
    } else {
        Ok(value)
    }
}

unsafe fn detail(s: *mut CodeDialog, value: &str, reveal: bool) {
    set_text(GetDlgItem((*s).hwnd, DETAIL), value);
    if reveal {
        focus_visible(s, GetDlgItem((*s).hwnd, DETAIL));
    }
}

unsafe fn update_preview(s: *mut CodeDialog) {
    (*s).parsed = None;
    if (*s).purpose.can_import() {
        EnableWindow(GetDlgItem((*s).hwnd, APPLY), 0);
    }
    let value = match read_code(s) {
        Ok(value) => value,
        Err(message) => {
            set_text(GetDlgItem((*s).hwnd, PREVIEW), "");
            detail(s, &message, false);
            return;
        }
    };
    if value.trim().is_empty() {
        set_text(
            GetDlgItem((*s).hwnd, PREVIEW),
            &crate::i18n::text(crate::i18n::Key::M757C5BBC83CD),
        );
        detail(s, "", false);
        return;
    }
    match connection_profile::import_code(&value) {
        Ok(profile) => {
            let summary = {
                let __ui_arg0 = &(profile.discord_app_id);
                let __ui_arg1 = &(profile.channel_url);
                let __ui_arg2 = &(if profile.webhook_url.is_empty() {
                    crate::i18n::text(crate::i18n::Key::ME4924B1EBED6)
                } else {
                    crate::i18n::text(crate::i18n::Key::M1AE9C3BD6E82)
                });
                crate::i18n::format(
                    crate::i18n::Key::MC45EEAB37613,
                    &[
                        ("arg0", __ui_arg0.to_string()),
                        ("arg1", __ui_arg1.to_string()),
                        ("arg2", __ui_arg2.to_string()),
                    ],
                )
            };
            set_text(GetDlgItem((*s).hwnd, PREVIEW), &summary);
            (*s).parsed = Some(profile);
            if (*s).purpose.can_import() {
                EnableWindow(GetDlgItem((*s).hwnd, APPLY), 1);
                detail(
                    s,
                    &crate::i18n::text(crate::i18n::Key::MC6C7DD81A097),
                    false,
                );
            } else {
                detail(
                    s,
                    &crate::i18n::text(crate::i18n::Key::M0EC16D73955A),
                    false,
                );
            }
        }
        Err(message) => {
            set_text(
                GetDlgItem((*s).hwnd, PREVIEW),
                &crate::i18n::text(crate::i18n::Key::M1668219C96C4),
            );
            detail(s, &message, false);
        }
    }
}

unsafe fn copy_current(s: *mut CodeDialog) -> Result<(), String> {
    if !(*s).purpose.can_share() {
        return Ok(());
    }
    let code = connection_profile::export_code(&(*s).source)?;
    write_clipboard((*s).hwnd, &code)?;
    set_text(GetDlgItem((*s).hwnd, CODE), &code);
    detail(s, &crate::i18n::text(crate::i18n::Key::MFBBDD090CECB), true);
    Ok(())
}

unsafe fn paste(s: *mut CodeDialog) -> Result<(), String> {
    if !(*s).purpose.can_import() {
        return Ok(());
    }
    let copied = read_clipboard((*s).hwnd)?;
    // A code is atomic, not prose. Preserve the codec's accepted line wrapping
    // while presenting one masked native edit line; never silently truncate it.
    let value: String = copied
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != '\u{000b}')
        .collect();
    if value.is_empty() {
        return Err(crate::i18n::text(crate::i18n::Key::MBA1577C88781));
    }
    set_text(GetDlgItem((*s).hwnd, CODE), &value);
    SetFocus(GetDlgItem((*s).hwnd, CODE));
    SendMessageW(GetDlgItem((*s).hwnd, CODE), EM_SETSEL, 0, -1);
    focus_visible(s, GetDlgItem((*s).hwnd, PREVIEW));
    Ok(())
}

unsafe fn apply(s: *mut CodeDialog) -> Result<(), String> {
    if !(*s).purpose.can_import() {
        return Ok(());
    }
    let profile = connection_profile::import_code(&read_code(s)?)?;
    // Check the active-tab identity and duplicate destinations before closing.
    // The caller repeats this against its own current draft after the modal loop.
    super::imported_connection_settings(&(*s).source, &profile)?;
    (*s).parsed = Some(profile);
    (*s).accepted = true;
    DestroyWindow((*s).hwnd);
    Ok(())
}

struct Clipboard;
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
struct LockedMemory(HGLOBAL);
impl Drop for LockedMemory {
    fn drop(&mut self) {
        unsafe {
            GlobalUnlock(self.0);
        }
    }
}

unsafe fn open_clipboard(owner: HWND) -> Result<Clipboard, String> {
    if OpenClipboard(owner) == 0 {
        Err(crate::i18n::text(crate::i18n::Key::MF65DA5ED2514))
    } else {
        Ok(Clipboard)
    }
}

unsafe fn read_clipboard(owner: HWND) -> Result<String, String> {
    let _clipboard = open_clipboard(owner)?;
    let memory = GetClipboardData(UNICODE_TEXT);
    if memory.is_null() {
        return Err(crate::i18n::text(crate::i18n::Key::M57A38CA2CABC));
    }
    let bytes = GlobalSize(memory);
    let max_bytes = (MAX_CODE_TEXT_BYTES + 1) * size_of::<u16>();
    if bytes < size_of::<u16>() || bytes % size_of::<u16>() != 0 || bytes > max_bytes {
        return Err(crate::i18n::text(crate::i18n::Key::MC8B5871038BF));
    }
    let data = GlobalLock(memory).cast::<u16>();
    if data.is_null() {
        return Err(crate::i18n::text(crate::i18n::Key::MCEF8357096F4));
    }
    let _memory = LockedMemory(memory);
    let units = std::slice::from_raw_parts(data, bytes / size_of::<u16>());
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| crate::i18n::text(crate::i18n::Key::M06C1C803A4C6))?;
    let value = String::from_utf16(&units[..end])
        .map_err(|_| crate::i18n::text(crate::i18n::Key::MCB7C4C6E1653))?;
    if value.len() > MAX_CODE_TEXT_BYTES {
        return Err(crate::i18n::text(crate::i18n::Key::M65DC9BA034A7));
    }
    Ok(value)
}

unsafe fn write_clipboard(owner: HWND, text: &str) -> Result<(), String> {
    if text.len() > MAX_CODE_TEXT_BYTES || text.contains('\0') {
        return Err(crate::i18n::text(crate::i18n::Key::M453FF6AC4182));
    }
    let value = wide(text);
    let memory = GlobalAlloc(GMEM_MOVEABLE, value.len() * size_of::<u16>());
    if memory.is_null() {
        return Err(crate::i18n::text(crate::i18n::Key::MD5B2CBDA74DD));
    }
    let destination = GlobalLock(memory).cast::<u16>();
    if destination.is_null() {
        GlobalFree(memory);
        return Err(crate::i18n::text(crate::i18n::Key::M5BF584606A90));
    }
    std::ptr::copy_nonoverlapping(value.as_ptr(), destination, value.len());
    GlobalUnlock(memory);
    let clipboard = match open_clipboard(owner) {
        Ok(clipboard) => clipboard,
        Err(message) => {
            GlobalFree(memory);
            return Err(message);
        }
    };
    let copied = EmptyClipboard() != 0 && !SetClipboardData(UNICODE_TEXT, memory).is_null();
    drop(clipboard);
    if !copied {
        GlobalFree(memory);
        return Err(crate::i18n::text(crate::i18n::Key::M57036D7DD67C));
    }
    // Successful SetClipboardData transfers ownership to Windows.
    Ok(())
}

unsafe extern "system" fn code_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(code_proc), id);
        return DefSubclassProc(hwnd, msg, wp, lp);
    }
    if msg == WM_PASTE {
        let s = data as *mut CodeDialog;
        if !s.is_null() {
            if let Err(message) = paste(s) {
                detail(s, &message, true);
            }
        }
        return 0;
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

unsafe fn ranges(s: *mut CodeDialog) {
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

unsafe fn position(s: *mut CodeDialog) {
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

unsafe fn scroll_to(s: *mut CodeDialog, vertical: bool, value: i32) {
    if vertical {
        (*s).scroll_y = value;
    } else {
        (*s).scroll_x = value;
    }
    ranges(s);
    position(s);
    InvalidateRect((*s).hwnd, null(), 1);
}

unsafe fn scroll(s: *mut CodeDialog, vertical: bool, command: u32) {
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

unsafe fn focus_visible(s: *mut CodeDialog, hwnd: HWND) {
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

unsafe fn dpi_changed(s: *mut CodeDialog, dpi: u32, rect: &RECT) {
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
        let s = create.lpCreateParams as *mut CodeDialog;
        (*s).hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, s as isize);
    }
    let s = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut CodeDialog;
    if s.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_CREATE => {
            if create_controls(s) {
                0
            } else {
                -1
            }
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
        DM_GETDEFID => {
            ((DC_HASDEFID as usize) << 16
                | if (*s).purpose.can_import() {
                    APPLY
                } else {
                    DISMISS
                } as usize) as isize
        }
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
            if id == CODE && notification == EN_CHANGE {
                update_preview(s);
                return 0;
            }
            if id == CODE && notification == EN_MAXTEXT {
                (*s).parsed = None;
                if (*s).purpose.can_import() {
                    EnableWindow(GetDlgItem(hwnd, APPLY), 0);
                }
                detail(s, &crate::i18n::text(crate::i18n::Key::MB229C5ED1579), true);
                return 0;
            }
            if notification != BN_CLICKED {
                return 0;
            }
            let result = match id {
                COPY_CURRENT => copy_current(s),
                PASTE => paste(s),
                REVEAL => {
                    SendMessageW(
                        GetDlgItem(hwnd, CODE),
                        EM_SETPASSWORDCHAR,
                        if checked(GetDlgItem(hwnd, REVEAL)) {
                            0
                        } else {
                            0x25cf
                        },
                        0,
                    );
                    InvalidateRect(GetDlgItem(hwnd, CODE), null(), 1);
                    Ok(())
                }
                APPLY | 1 => apply(s),
                DISMISS | 2 => {
                    DestroyWindow(hwnd);
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(message) = result {
                detail(s, &message, true);
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
