// SPDX-License-Identifier: MPL-2.0
//! Per-PC translation preferences. This dialog does not start a worker,
//! decrypt a saved credential, make API calls, or persist a settings file.
use super::*;
use crate::api_provider::ApiProvider;
use crate::config::{TranslationSettings, TRANSLATION_LANGUAGES};
use crate::model_catalog;
use windows_sys::Win32::UI::Shell::ShellExecuteW;

const TRANSLATION_CLASS: &str = "Sideglow.Translation.v1";
const WIDTH: i32 = 660;
const PROVIDER_AREA_HEIGHT: i32 = 168;
const HEIGHT: i32 = 938 + PROVIDER_AREA_HEIGHT;
const ENABLED: i32 = 601;
const SHOW_RECEIVED: i32 = 602;
const API_KEY: i32 = 603;
const CLEAR_KEY: i32 = 604;
const KEY_STATUS: i32 = 605;
const MODEL: i32 = 606;
const LANGUAGE_A: i32 = 607;
const LANGUAGE_B: i32 = 608;
const GLOSSARY: i32 = 609;
const DETAIL: i32 = 610;
const APPLY: i32 = 611;
const DISMISS: i32 = 612;
const MODEL_PRESET: i32 = 613;
const MODEL_INFO: i32 = 614;
const FAST_MODE: i32 = 615;
const MODEL_DOCS: i32 = 616;
const PROVIDER_LABEL: i32 = 617;
const PROVIDER: i32 = 618;
const API_BASE_LABEL: i32 = 619;
const API_BASE_URL: i32 = 620;
const API_BASE_HELP: i32 = 621;
const FAST_HELP: i32 = 622;
const PROVIDERS: [ApiProvider; 4] = [
    ApiProvider::OpenAi,
    ApiProvider::OpenAiCompatible,
    ApiProvider::Anthropic,
    ApiProvider::Gemini,
];

struct TranslationDialog {
    hwnd: HWND,
    dpi: u32,
    font: HFONT,
    heading_font: HFONT,
    brush: HBRUSH,
    edit_brush: HBRUSH,
    controls: Vec<ControlLayout>,
    scroll_x: i32,
    scroll_y: i32,
    draft: TranslationSettings,
    connection_mode: ConnectionMode,
    channel_url: String,
    webhook_url: String,
    accepted: bool,
    filling_controls: bool,
    last_identity: Option<config::TranslationCredentialIdentity>,
}

pub(super) unsafe fn register(instance: HINSTANCE) -> Result<(), String> {
    let name = wide(TRANSLATION_CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    if RegisterClassW(&class) == 0 {
        Err(crate::i18n::text(crate::i18n::Key::M1AD8D1528355))
    } else {
        Ok(())
    }
}

pub(super) unsafe fn run(parent: *mut Dialog) -> Option<TranslationSettings> {
    let owner = (*parent).hwnd;
    let dpi = (*parent).dpi;
    let mut state = Box::new(TranslationDialog {
        hwnd: null_mut(),
        dpi,
        font: settings_font(dpi, false),
        heading_font: settings_font(dpi, true),
        brush: CreateSolidBrush(GetSysColor(COLOR_BTNFACE)),
        edit_brush: CreateSolidBrush(GetSysColor(COLOR_WINDOW)),
        controls: Vec::new(),
        scroll_x: 0,
        scroll_y: 0,
        draft: (*parent).draft.translation.clone(),
        connection_mode: (*parent).draft.connection_mode,
        channel_url: (*parent).draft.channel_url.clone(),
        webhook_url: (*parent).draft.webhook_url.clone(),
        accepted: false,
        filling_controls: true,
        last_identity: config::translation_credential_identity(
            (*parent).draft.translation.provider,
            &(*parent).draft.translation.api_base_url,
        )
        .ok(),
    });
    let ptr: *mut TranslationDialog = &mut *state;
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
        wide(TRANSLATION_CLASS).as_ptr(),
        wide(&crate::i18n::text(crate::i18n::Key::M9AB73DE1DBA5)).as_ptr(),
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
        super::error(parent, &crate::i18n::text(crate::i18n::Key::MD339180CF07E));
        release_objects(ptr);
        return None;
    }
    EnableWindow(owner, 0);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(GetDlgItem(hwnd, ENABLED));
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
    // Native messages may close the parent while this loop is dispatching.
    // The state stays owned here until WM_NCDESTROY has detached its pointer.
    let owner_alive = IsWindow(owner) != 0;
    if owner_alive {
        EnableWindow(owner, 1);
        SetForegroundWindow(owner);
        SetFocus(GetDlgItem(owner, TRANSLATION_SETTINGS));
    }
    release_objects(ptr);
    if owner_alive && state.accepted {
        Some(state.draft.clone())
    } else {
        None
    }
}

unsafe fn release_objects(s: *mut TranslationDialog) {
    DeleteObject((*s).font as _);
    DeleteObject((*s).heading_font as _);
    DeleteObject((*s).brush as _);
    DeleteObject((*s).edit_brush as _);
}

#[allow(clippy::too_many_arguments)]
unsafe fn item(
    s: *mut TranslationDialog,
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
    // Reserve two rows above the previous API-key section. Store the final
    // logical coordinates so DPI/scroll repositioning never adds this twice.
    let y = y + if y >= 160
        && !matches!(
            id,
            PROVIDER_LABEL | PROVIDER | API_BASE_LABEL | API_BASE_URL | API_BASE_HELP
        ) {
        PROVIDER_AREA_HEIGHT
    } else {
        0
    };
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

unsafe fn paragraph(s: *mut TranslationDialog, value: &str, y: i32, height: i32) {
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

unsafe fn checkbox(s: *mut TranslationDialog, value: &str, id: i32, y: i32, on: bool) {
    let hwnd = item(
        s,
        "BUTTON",
        value,
        id,
        22,
        y,
        616,
        24,
        WS_TABSTOP | BS_AUTOCHECKBOX as u32 | BS_NOTIFY as u32,
        false,
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

unsafe fn button(s: *mut TranslationDialog, value: &str, id: i32, x: i32, y: i32, width: i32) {
    item(
        s,
        "BUTTON",
        value,
        id,
        x,
        y,
        width,
        30,
        WS_TABSTOP
            | BS_NOTIFY as u32
            | if id == APPLY {
                BS_DEFPUSHBUTTON as u32
            } else {
                BS_PUSHBUTTON as u32
            },
        false,
    );
}

unsafe fn language_combo(s: *mut TranslationDialog, id: i32, x: i32, selected: &str) {
    let hwnd = item(
        s,
        "COMBOBOX",
        "",
        id,
        x,
        531,
        294,
        250,
        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        false,
    );
    let mut index = 0;
    for (i, (code, name)) in TRANSLATION_LANGUAGES.iter().enumerate() {
        let name = crate::i18n::text(*name);
        SendMessageW(
            hwnd,
            CB_ADDSTRING,
            0,
            wide(&format!("{name} ({code})")).as_ptr() as isize,
        );
        if *code == selected {
            index = i;
        }
    }
    SendMessageW(hwnd, CB_SETCURSEL, index, 0);
}

unsafe fn render(s: *mut TranslationDialog) {
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M9D5DF751BDF7),
        0,
        22,
        16,
        616,
        22,
        SS_NOPREFIX,
        true,
    );
    checkbox(
        s,
        &crate::i18n::text(crate::i18n::Key::MCCF7E0BBF4A1),
        ENABLED,
        45,
        (*s).draft.enabled,
    );
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::MFF8D762CE257),
        76,
        38,
    );
    checkbox(
        s,
        &crate::i18n::text(crate::i18n::Key::M0C33C940BCB5),
        SHOW_RECEIVED,
        122,
        (*s).draft.show_received,
    );
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::TranslationProviderLabel),
        PROVIDER_LABEL,
        22,
        160,
        616,
        20,
        SS_NOPREFIX,
        false,
    );
    let providers = item(
        s,
        "COMBOBOX",
        "",
        PROVIDER,
        22,
        183,
        616,
        160,
        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        false,
    );
    for provider in PROVIDERS {
        let label = if provider == ApiProvider::OpenAiCompatible {
            crate::i18n::text(crate::i18n::Key::TranslationProviderCompatible)
        } else {
            provider.label().to_owned()
        };
        SendMessageW(providers, CB_ADDSTRING, 0, wide(&label).as_ptr() as isize);
    }
    let selected = PROVIDERS
        .iter()
        .position(|provider| *provider == (*s).draft.provider)
        .unwrap_or(0);
    SendMessageW(providers, CB_SETCURSEL, selected, 0);
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::TranslationEndpointLabel),
        API_BASE_LABEL,
        22,
        224,
        616,
        20,
        SS_NOPREFIX,
        false,
    );
    let endpoint = {
        let draft = &(*s).draft;
        if draft.api_base_url.is_empty() {
            draft.provider.default_base_url().to_owned()
        } else {
            draft.api_base_url.clone()
        }
    };
    let endpoint_control = item(
        s,
        "EDIT",
        &endpoint,
        API_BASE_URL,
        22,
        247,
        616,
        27,
        WS_TABSTOP | ES_AUTOHSCROLL as u32,
        false,
    );
    SendMessageW(endpoint_control, EM_SETLIMITTEXT, 2048, 0);
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::TranslationEndpointHelp),
        API_BASE_HELP,
        22,
        282,
        616,
        34,
        SS_NOPREFIX,
        false,
    );
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M594D8A3C87F4),
        160,
        20,
    );
    let key = item(
        s,
        "EDIT",
        "",
        API_KEY,
        22,
        183,
        442,
        27,
        WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
        false,
    );
    SendMessageW(key, EM_SETLIMITTEXT, 1024, 0);
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M82BA8ED7B430),
        CLEAR_KEY,
        482,
        181,
        156,
    );
    item(
        s,
        "STATIC",
        "",
        KEY_STATUS,
        22,
        218,
        616,
        34,
        SS_NOPREFIX,
        false,
    );
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::MF4902C9D65A9),
        258,
        20,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::MA646BC684FC6),
        MODEL_DOCS,
        446,
        252,
        192,
    );
    item(
        s,
        "COMBOBOX",
        "",
        MODEL_PRESET,
        22,
        284,
        616,
        240,
        WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        false,
    );
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M7D5D527674C8),
        319,
        20,
    );
    let model = item(
        s,
        "EDIT",
        &(*s).draft.model.clone(),
        MODEL,
        22,
        342,
        616,
        27,
        WS_TABSTOP | ES_AUTOHSCROLL as u32,
        false,
    );
    SendMessageW(model, EM_SETLIMITTEXT, 200, 0);
    item(
        s,
        "STATIC",
        "",
        MODEL_INFO,
        22,
        379,
        616,
        40,
        SS_NOPREFIX,
        false,
    );
    checkbox(
        s,
        &crate::i18n::text(crate::i18n::Key::M22313339ACD2),
        FAST_MODE,
        430,
        (*s).draft.fast_mode,
    );
    item(
        s,
        "STATIC",
        "",
        FAST_HELP,
        22,
        461,
        616,
        38,
        SS_NOPREFIX,
        false,
    );
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::M98768A5599D4),
        0,
        22,
        508,
        294,
        20,
        SS_NOPREFIX,
        false,
    );
    item(
        s,
        "STATIC",
        &crate::i18n::text(crate::i18n::Key::MB4B3F9772FFC),
        0,
        344,
        508,
        294,
        20,
        SS_NOPREFIX,
        false,
    );
    language_combo(s, LANGUAGE_A, 22, &(*s).draft.language_a.clone());
    language_combo(s, LANGUAGE_B, 344, &(*s).draft.language_b.clone());
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::MC58F0E39CD49),
        570,
        36,
    );
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::MB35E75069865),
        615,
        20,
    );
    let glossary = item(
        s,
        "EDIT",
        &(*s).draft.glossary.clone(),
        GLOSSARY,
        22,
        638,
        616,
        95,
        WS_TABSTOP
            | WS_VSCROLL
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | ES_WANTRETURN as u32,
        false,
    );
    SendMessageW(glossary, EM_SETLIMITTEXT, 4000, 0);
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M90061BEBCE0E),
        744,
        38,
    );
    paragraph(
        s,
        &crate::i18n::text(crate::i18n::Key::M2E60091739C3),
        791,
        38,
    );
    item(
        s,
        "STATIC",
        "",
        DETAIL,
        22,
        841,
        616,
        38,
        SS_NOPREFIX,
        false,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::MBDE76B4A9F84),
        DISMISS,
        422,
        888,
        100,
    );
    button(
        s,
        &crate::i18n::text(crate::i18n::Key::M82DCCF41039E),
        APPLY,
        538,
        888,
        100,
    );
    provider_controls(s);
    (*s).filling_controls = false;
    destination_changed(s);
    ranges(s);
    position(s);
}

unsafe fn selected_provider(s: *mut TranslationDialog) -> Result<ApiProvider, String> {
    let index = SendMessageW(GetDlgItem((*s).hwnd, PROVIDER), CB_GETCURSEL, 0, 0);
    PROVIDERS
        .get(index as usize)
        .copied()
        .ok_or_else(|| crate::i18n::text(crate::i18n::Key::TranslationProviderInvalid))
}

unsafe fn selected_identity(
    s: *mut TranslationDialog,
) -> Result<config::TranslationCredentialIdentity, String> {
    config::translation_credential_identity(
        selected_provider(s)?,
        &text(GetDlgItem((*s).hwnd, API_BASE_URL)),
    )
}

unsafe fn provider_controls(s: *mut TranslationDialog) {
    let hwnd = (*s).hwnd;
    let Ok(provider) = selected_provider(s) else {
        return;
    };
    let openai = provider == ApiProvider::OpenAi;
    SendMessageW(
        GetDlgItem(hwnd, API_BASE_URL),
        EM_SETREADONLY,
        usize::from(provider != ApiProvider::OpenAiCompatible),
        0,
    );
    let presets = GetDlgItem(hwnd, MODEL_PRESET);
    SendMessageW(presets, CB_RESETCONTENT, 0, 0);
    if openai {
        for preset in model_catalog::PRESETS {
            SendMessageW(
                presets,
                CB_ADDSTRING,
                0,
                wide(&crate::i18n::text(preset.label)).as_ptr() as isize,
            );
        }
    }
    SendMessageW(
        presets,
        CB_ADDSTRING,
        0,
        wide(&crate::i18n::text(crate::i18n::Key::M7787D3CDAC90)).as_ptr() as isize,
    );
    EnableWindow(presets, i32::from(openai));
    EnableWindow(GetDlgItem(hwnd, FAST_MODE), i32::from(openai));
    set_text(
        GetDlgItem(hwnd, FAST_MODE),
        &crate::i18n::text(if openai {
            crate::i18n::Key::M22313339ACD2
        } else {
            crate::i18n::Key::TranslationFastOpenAiOnly
        }),
    );
    if !openai {
        SendMessageW(
            GetDlgItem(hwnd, FAST_MODE),
            BM_SETCHECK,
            BST_UNCHECKED as usize,
            0,
        );
    }
    set_text(
        GetDlgItem(hwnd, FAST_HELP),
        &crate::i18n::text(if openai {
            crate::i18n::Key::MB9C546B73864
        } else {
            crate::i18n::Key::TranslationFastUnavailable
        }),
    );
    EnableWindow(
        GetDlgItem(hwnd, MODEL_DOCS),
        i32::from(provider != ApiProvider::OpenAiCompatible),
    );
    model_hint(s);
    key_status(s);
}

unsafe fn change_provider(s: *mut TranslationDialog) {
    let Ok(provider) = selected_provider(s) else {
        return;
    };
    (*s).filling_controls = true;
    let endpoint = {
        let draft = &(*s).draft;
        if provider == draft.provider && !draft.api_base_url.is_empty() {
            draft.api_base_url.clone()
        } else {
            provider.default_base_url().to_owned()
        }
    };
    set_text(GetDlgItem((*s).hwnd, API_BASE_URL), &endpoint);
    // A key entered for one destination must not follow a provider switch.
    set_text(GetDlgItem((*s).hwnd, API_KEY), "");
    provider_controls(s);
    (*s).filling_controls = false;
    destination_changed(s);
}

unsafe fn destination_changed(s: *mut TranslationDialog) {
    if (*s).filling_controls {
        return;
    }
    let identity = selected_identity(s).ok();
    if identity != (*s).last_identity {
        set_text(GetDlgItem((*s).hwnd, API_KEY), "");
        (*s).last_identity = identity;
    }
    key_status(s);
}

unsafe fn model_hint(s: *mut TranslationDialog) {
    let hwnd = (*s).hwnd;
    let editor = GetDlgItem(hwnd, MODEL);
    if editor.is_null() {
        return;
    }
    let model = text(editor);
    let model = model.trim();
    if selected_provider(s).ok() != Some(ApiProvider::OpenAi) {
        SendMessageW(GetDlgItem(hwnd, MODEL_PRESET), CB_SETCURSEL, 0, 0);
        set_text(
            GetDlgItem(hwnd, MODEL_INFO),
            &crate::i18n::text(crate::i18n::Key::TranslationModelCustomHint),
        );
        return;
    }
    let index = model_catalog::PRESETS
        .iter()
        .position(|preset| preset.id == model)
        .unwrap_or(model_catalog::PRESETS.len());
    SendMessageW(GetDlgItem(hwnd, MODEL_PRESET), CB_SETCURSEL, index, 0);
    set_text(
        GetDlgItem(hwnd, MODEL_INFO),
        &crate::i18n::text(
            model_catalog::find(model)
                .map_or(crate::i18n::Key::M9D7800819FF3, |preset| preset.description),
        ),
    );
}

unsafe fn key_status(s: *mut TranslationDialog) {
    let draft = &(*s).draft;
    let status = match selected_identity(s) {
        Err(message) => message,
        Ok(identity) => {
            let mut candidate = draft.clone();
            candidate.provider = identity.provider;
            candidate.api_base_url = identity.api_base_url;
            if draft.protected_api_key.is_empty() {
                crate::i18n::text(crate::i18n::Key::M66D371B3ECFB)
            } else if config::validate_translation_key_binding(&candidate).is_err() {
                crate::i18n::text(crate::i18n::Key::TranslationKeyNewDestination)
            } else {
                crate::i18n::text(crate::i18n::Key::MEBADAF9F58B7)
            }
        }
    };
    set_text(GetDlgItem((*s).hwnd, KEY_STATUS), &status);
}

unsafe fn selected_language(s: *mut TranslationDialog, id: i32) -> Result<String, String> {
    let selected = SendMessageW(GetDlgItem((*s).hwnd, id), CB_GETCURSEL, 0, 0);
    TRANSLATION_LANGUAGES
        .get(selected as usize)
        .map(|(code, _)| (*code).to_string())
        .ok_or_else(|| crate::i18n::text(crate::i18n::Key::M0093A79DAA17))
}

unsafe fn apply(s: *mut TranslationDialog) -> Result<(), String> {
    let hwnd = (*s).hwnd;
    let mut draft = (*s).draft.clone();
    let identity = selected_identity(s)?;
    draft.provider = identity.provider;
    draft.api_base_url = identity.api_base_url;
    if config::validate_translation_key_binding(&draft).is_err() {
        draft.protected_api_key.clear();
        draft.credential_identity = None;
    }
    draft.enabled = checked(GetDlgItem(hwnd, ENABLED));
    draft.show_received = checked(GetDlgItem(hwnd, SHOW_RECEIVED));
    draft.model = text(GetDlgItem(hwnd, MODEL)).trim().into();
    draft.fast_mode = draft.provider == ApiProvider::OpenAi && checked(GetDlgItem(hwnd, FAST_MODE));
    draft.language_a = selected_language(s, LANGUAGE_A)?;
    draft.language_b = selected_language(s, LANGUAGE_B)?;
    draft.glossary = text(GetDlgItem(hwnd, GLOSSARY));
    if draft.enabled {
        config::validate_translation_connection(
            (*s).connection_mode,
            &(*s).channel_url,
            &(*s).webhook_url,
        )?;
    }
    let mut entered = text(GetDlgItem(hwnd, API_KEY));
    if !entered.trim().is_empty() {
        let saved = config::store_translation_key(&mut draft, &entered);
        // Keep the edit control intact on failure so the user can correct it,
        // but do not retain our transient plaintext copy beyond this call.
        entered.as_mut_vec().fill(0);
        saved?;
    }
    config::validate_translation(&draft)?;
    set_text(GetDlgItem(hwnd, API_KEY), "");
    (*s).draft = draft;
    (*s).accepted = true;
    DestroyWindow(hwnd);
    Ok(())
}

unsafe fn ranges(s: *mut TranslationDialog) {
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

unsafe fn position(s: *mut TranslationDialog) {
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

unsafe fn scroll_to_position(s: *mut TranslationDialog, vertical: bool, pos: i32) {
    if vertical {
        (*s).scroll_y = pos;
    } else {
        (*s).scroll_x = pos;
    }
    ranges(s);
    position(s);
    InvalidateRect((*s).hwnd, null(), 1);
}

unsafe fn scroll_page(s: *mut TranslationDialog, vertical: bool, command: u32) {
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

unsafe fn focus_visible(s: *mut TranslationDialog, hwnd: HWND) {
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

unsafe fn dpi_changed(s: *mut TranslationDialog, dpi: u32, rect: &RECT) {
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
        let s = create.lpCreateParams as *mut TranslationDialog;
        (*s).hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, s as isize);
    }
    let s = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TranslationDialog;
    if s.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_CREATE => {
            render(s);
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
        DM_GETDEFID => ((DC_HASDEFID as usize) << 16 | APPLY as usize) as isize,
        WM_COMMAND => {
            let id = (wp & 0xffff) as i32;
            let notification = (wp >> 16) as u32;
            if notification == EN_SETFOCUS
                || notification == BN_SETFOCUS
                || notification == CBN_SETFOCUS
            {
                focus_visible(s, lp as HWND);
                return 0;
            }
            if (*s).filling_controls {
                return 0;
            }
            if id == PROVIDER && notification == CBN_SELCHANGE {
                change_provider(s);
                return 0;
            }
            if id == API_BASE_URL && notification == EN_CHANGE {
                destination_changed(s);
                return 0;
            }
            if id == MODEL_PRESET && notification == CBN_SELCHANGE {
                if selected_provider(s).ok() != Some(ApiProvider::OpenAi) {
                    return 0;
                }
                let index = SendMessageW(GetDlgItem(hwnd, MODEL_PRESET), CB_GETCURSEL, 0, 0);
                if let Some(preset) = model_catalog::PRESETS.get(index as usize) {
                    set_text(GetDlgItem(hwnd, MODEL), preset.id);
                    model_hint(s);
                } else {
                    SetFocus(GetDlgItem(hwnd, MODEL));
                }
                return 0;
            }
            if id == MODEL && notification == EN_CHANGE {
                model_hint(s);
                return 0;
            }
            if notification != BN_CLICKED {
                return 0;
            }
            match id {
                APPLY | 1 => {
                    if let Err(message) = apply(s) {
                        let detail = GetDlgItem(hwnd, DETAIL);
                        set_text(detail, &message);
                        focus_visible(s, detail);
                    }
                }
                DISMISS | 2 => {
                    DestroyWindow(hwnd);
                }
                CLEAR_KEY => {
                    (*s).draft.protected_api_key.clear();
                    (*s).draft.credential_identity = None;
                    (*s).draft.enabled = false;
                    SendMessageW(
                        GetDlgItem(hwnd, ENABLED),
                        BM_SETCHECK,
                        BST_UNCHECKED as usize,
                        0,
                    );
                    set_text(GetDlgItem(hwnd, API_KEY), "");
                    key_status(s);
                    set_text(
                        GetDlgItem(hwnd, DETAIL),
                        &crate::i18n::text(crate::i18n::Key::M89026CD73FF8),
                    );
                }
                MODEL_DOCS => {
                    let url = match selected_provider(s) {
                        Ok(ApiProvider::OpenAi) => "https://developers.openai.com/api/docs/models",
                        Ok(ApiProvider::Anthropic) => {
                            "https://platform.claude.com/docs/en/models/overview"
                        }
                        Ok(ApiProvider::Gemini) => "https://aistudio.google.com/docs/models",
                        _ => return 0,
                    };
                    let result = ShellExecuteW(
                        hwnd,
                        wide("open").as_ptr(),
                        wide(url).as_ptr(),
                        null(),
                        null(),
                        SW_SHOWNORMAL,
                    );
                    if result as isize <= 32 && IsWindow(hwnd) != 0 {
                        set_text(
                            GetDlgItem(hwnd, DETAIL),
                            &crate::i18n::text(crate::i18n::Key::M8DB4E078B66A),
                        );
                    }
                }
                _ => {}
            }
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            SetTextColor(wp as HDC, GetSysColor(COLOR_BTNTEXT));
            SetBkColor(wp as HDC, GetSysColor(COLOR_BTNFACE));
            (*s).brush as isize
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            SetTextColor(wp as HDC, GetSysColor(COLOR_WINDOWTEXT));
            SetBkColor(wp as HDC, GetSysColor(COLOR_WINDOW));
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
            (*s).hwnd = null_mut();
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
