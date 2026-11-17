// SPDX-License-Identifier: MPL-2.0
//! On-demand snapshots of user-visible applications. No polling or HWND is
//! persisted: the selected executable path remains valid across app restarts.
use super::*;
use std::collections::HashSet;

pub(super) struct Application {
    pub path: String,
    pub title: String,
}

pub(super) struct Applications {
    pub entries: Vec<Application>,
    pub inaccessible: usize,
    seen: HashSet<String>,
}

pub(super) fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

pub(super) unsafe fn scan() -> Result<Applications, String> {
    let mut result = Applications {
        entries: Vec::new(),
        inaccessible: 0,
        seen: HashSet::new(),
    };
    if EnumWindows(Some(visit), (&mut result as *mut Applications) as LPARAM) == 0 {
        return Err({
            let __ui_arg0 = &(GetLastError());
            crate::i18n::format(
                crate::i18n::Key::MBD488B39B156,
                &[("arg0", format!("{}", __ui_arg0))],
            )
        });
    }
    result.entries.sort_by(|left, right| {
        file_name(&left.path)
            .to_lowercase()
            .cmp(&file_name(&right.path).to_lowercase())
            .then_with(|| left.title.cmp(&right.title))
    });
    Ok(result)
}

pub(super) unsafe fn validate_running(path: &str) -> Result<(), String> {
    // Re-enumeration reopens the current process identity. It cannot accidentally
    // accept a reused HWND/PID or a cached window whose executable has changed.
    if scan()?
        .entries
        .iter()
        .any(|entry| entry.path.eq_ignore_ascii_case(path))
    {
        Ok(())
    } else {
        Err(crate::i18n::text(crate::i18n::Key::MBB5E942B337B))
    }
}

unsafe extern "system" fn visit(hwnd: HWND, context: LPARAM) -> BOOL {
    if IsWindowVisible(hwnd) == 0
        || hwnd == GetShellWindow()
        || GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0
    {
        return 1;
    }
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 || pid == GetCurrentProcessId() {
        return 1;
    }
    let title = text(hwnd)
        .chars()
        .filter(|character| !character.is_control())
        .take(200)
        .collect::<String>();
    if title.trim().is_empty() {
        return 1;
    }
    let result = &mut *(context as *mut Applications);
    if result.entries.len() >= 512 {
        return 1;
    }
    match process_path(hwnd) {
        Ok(path) if result.seen.insert(path.to_ascii_lowercase()) => {
            result.entries.push(Application { path, title });
        }
        Ok(_) => {}
        Err(_) => result.inaccessible += 1,
    }
    1
}
