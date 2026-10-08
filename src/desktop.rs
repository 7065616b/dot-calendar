//! Optional Explorer desktop attachment for the calendar canvas.
//!
//! Explorer's `Progman`, `WorkerW`, and `SHELLDLL_DefView` window classes are
//! implementation details, not a Windows widget API. We only use an existing
//! icon host. If it is unavailable or cross-process `SetParent` fails, the
//! calendar remains an ordinary window and the caller can keep using it.

use std::{cell::RefCell, collections::HashMap, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND, LPARAM, RECT},
    Graphics::Gdi::MapWindowPoints,
    UI::WindowsAndMessaging::{
        EnumChildWindows, FindWindowExW, FindWindowW, GetClassNameW, GetParent, GetWindowLongPtrW,
        GetWindowRect, IsWindow, SetParent, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE,
        GWL_STYLE, HWND_BOTTOM, HWND_TOP, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        WS_CHILD, WS_EX_APPWINDOW, WS_POPUP,
    },
};

#[derive(Clone, Copy)]
struct OriginalWindow {
    parent: HWND,
    host: HWND,
    style: isize,
    ex_style: isize,
}

thread_local! {
    // Window attachment and detachment happen on the UI thread that owns HWND.
    static ORIGINAL: RefCell<HashMap<usize, OriginalWindow>> = RefCell::new(HashMap::new());
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

unsafe fn has_class(hwnd: HWND, class: &str) -> bool {
    let mut buffer = [0u16; 64];
    let count = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    count > 0 && String::from_utf16_lossy(&buffer[..count as usize]) == class
}

unsafe extern "system" fn find_defview(hwnd: HWND, context: LPARAM) -> i32 {
    if has_class(hwnd, "SHELLDLL_DefView") {
        *(context as *mut HWND) = hwnd;
        0
    } else {
        1
    }
}

unsafe fn defview_within(candidate: HWND) -> HWND {
    if candidate.is_null() || IsWindow(candidate) == 0 {
        return null_mut();
    }
    let mut found: HWND = null_mut();
    EnumChildWindows(
        candidate,
        Some(find_defview),
        (&mut found as *mut HWND) as LPARAM,
    );
    found
}

/// Find the existing Explorer window that directly contains desktop icons.
/// This does not send Explorer undocumented messages or create shell windows.
unsafe fn icon_host() -> HWND {
    let progman = FindWindowW(wide("Progman").as_ptr(), null_mut());
    let mut view = defview_within(progman);
    if !view.is_null() {
        let parent = GetParent(view);
        if has_class(parent, "Progman") || has_class(parent, "WorkerW") {
            return parent;
        }
    }

    // On other Explorer layouts the icon view belongs to a top-level WorkerW.
    let mut worker: HWND = null_mut();
    loop {
        worker = FindWindowExW(null_mut(), worker, wide("WorkerW").as_ptr(), null_mut());
        if worker.is_null() {
            break;
        }
        view = defview_within(worker);
        if !view.is_null() {
            let parent = GetParent(view);
            if has_class(parent, "Progman") || has_class(parent, "WorkerW") {
                return parent;
            }
        }
    }
    null_mut()
}

unsafe fn set_long(hwnd: HWND, index: i32, value: isize) -> Result<(), String> {
    SetLastError(0);
    let previous = SetWindowLongPtrW(hwnd, index, value);
    if previous == 0 && GetLastError() != 0 {
        Err(format!("창 속성을 변경할 수 없습니다: {}", GetLastError()))
    } else {
        Ok(())
    }
}

/// Attach a small interactive calendar to the existing desktop icon host.
///
/// The child is placed above the icon view so it receives clicks. Desktop
/// icons under the widget rectangle will therefore be obscured; the UI should
/// let the user move or resize the calendar into free desktop space.
///
/// A successful attachment is deliberately optional. Explorer can restart,
/// and cross-process parenting can fail when DPI awareness modes differ.
/// In either case the caller should retain an ordinary-window fallback.
pub unsafe fn attach(hwnd: HWND) -> Result<(), String> {
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        return Err("달력 창을 찾을 수 없습니다.".into());
    }
    if is_attached(hwnd) {
        return Ok(());
    }
    let was_attached = ORIGINAL.with(|saved| saved.borrow().contains_key(&(hwnd as usize)));
    if was_attached {
        detach(hwnd);
    }

    let parent = icon_host();
    if parent.is_null() {
        return Err("Windows 바탕화면 아이콘 창을 찾지 못했습니다.".into());
    }
    let mut screen: RECT = std::mem::zeroed();
    if GetWindowRect(hwnd, &mut screen) == 0 {
        return Err("달력 창의 위치를 확인할 수 없습니다.".into());
    }
    let original = OriginalWindow {
        parent: GetParent(hwnd),
        host: parent,
        style: GetWindowLongPtrW(hwnd, GWL_STYLE),
        ex_style: GetWindowLongPtrW(hwnd, GWL_EXSTYLE),
    };
    let child_style = (original.style as u32 & !WS_POPUP) | WS_CHILD;
    set_long(hwnd, GWL_STYLE, child_style as isize)?;
    let child_ex_style = original.ex_style as u32 & !WS_EX_APPWINDOW;
    if let Err(error) = set_long(hwnd, GWL_EXSTYLE, child_ex_style as isize) {
        let _ = set_long(hwnd, GWL_STYLE, original.style);
        return Err(error);
    }

    SetLastError(0);
    let previous = SetParent(hwnd, parent);
    if previous.is_null() && GetLastError() != 0 {
        let error = GetLastError();
        let _ = set_long(hwnd, GWL_STYLE, original.style);
        let _ = set_long(hwnd, GWL_EXSTYLE, original.ex_style);
        let _ = SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            screen.left,
            screen.top,
            screen.right - screen.left,
            screen.bottom - screen.top,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        return Err(format!("달력을 바탕화면에 붙일 수 없습니다: {error}"));
    }

    // The original RECT is in screen coordinates. Child coordinates are
    // relative to Explorer's icon host and may differ on secondary monitors.
    let mut origin = windows_sys::Win32::Foundation::POINT {
        x: screen.left,
        y: screen.top,
    };
    MapWindowPoints(null_mut(), parent, &mut origin, 1);
    if SetWindowPos(
        hwnd,
        HWND_TOP,
        origin.x,
        origin.y,
        screen.right - screen.left,
        screen.bottom - screen.top,
        SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
    ) == 0
    {
        let error = GetLastError();
        let _ = SetParent(hwnd, original.parent);
        let _ = set_long(hwnd, GWL_STYLE, original.style);
        let _ = set_long(hwnd, GWL_EXSTYLE, original.ex_style);
        let _ = SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            screen.left,
            screen.top,
            screen.right - screen.left,
            screen.bottom - screen.top,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        return Err(format!(
            "바탕화면 달력의 위치를 지정할 수 없습니다: {error}"
        ));
    }
    ORIGINAL.with(|saved| {
        saved.borrow_mut().insert(hwnd as usize, original);
    });
    Ok(())
}

/// True only while the HWND still belongs to the Explorer host we selected.
pub unsafe fn is_attached(hwnd: HWND) -> bool {
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        return false;
    }
    ORIGINAL.with(|saved| {
        saved
            .borrow()
            .get(&(hwnd as usize))
            .is_some_and(|original| {
                let current = GetParent(hwnd);
                current == original.host
                    && IsWindow(current) != 0
                    && !defview_within(current).is_null()
            })
    })
}

/// Called at WM_DESTROY before Windows can recycle the HWND value.
pub fn forget_destroyed(hwnd: HWND) {
    ORIGINAL.with(|saved| {
        saved.borrow_mut().remove(&(hwnd as usize));
    });
}

/// Return an attached widget to its previous top-level style and screen spot.
/// The caller may also invoke this after Explorer changed its desktop layout.
pub unsafe fn detach(hwnd: HWND) {
    let original = ORIGINAL.with(|saved| saved.borrow_mut().remove(&(hwnd as usize)));
    let Some(original) = original else { return };
    if hwnd.is_null() || IsWindow(hwnd) == 0 {
        return;
    }
    let mut screen: RECT = std::mem::zeroed();
    if GetWindowRect(hwnd, &mut screen) == 0 {
        return;
    }
    SetLastError(0);
    let previous = SetParent(hwnd, original.parent);
    if previous.is_null() && GetLastError() != 0 {
        return;
    }
    let _ = set_long(hwnd, GWL_STYLE, original.style);
    let _ = set_long(hwnd, GWL_EXSTYLE, original.ex_style);
    let _ = SetWindowPos(
        hwnd,
        HWND_BOTTOM,
        screen.left,
        screen.top,
        screen.right - screen.left,
        screen.bottom - screen.top,
        SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
}
