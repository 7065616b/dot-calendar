//! Native component layer. Store mutations still belong to the existing command
//! handlers. All motion is finite DirectComposition transform/opacity animation.
use super::*;
use crate::{
    composition::{Compositor, Finish},
    ui_design::*,
};
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Accessibility::NotifyWinEvent;

mod palette;

pub(super) const OPEN_PALETTE: i32 = 51;
const MONTH_TAB: i32 = 52;
const DAY_TAB: i32 = 53;
const DONE_TAB: i32 = 54;
pub(super) const SHOWCASE: i32 = 55;
const DEMO_PRIMARY: i32 = 56;
const DEMO_SECONDARY: i32 = 57;
const DEMO_GHOST: i32 = 58;
const DEMO_TOAST: i32 = 59;
const EVENT_LIST: i32 = 60;
const EVENT_MEMO: i32 = 61;
const ANIMATE_BUTTON: u32 = WM_APP + 21;
const UI_TIMER: usize = 21;
const UI_EXPIRY: usize = 22;
const TOAST_CLOSE: u32 = WM_APP + 22;
pub(super) const VIEW_REFRESH: u32 = WM_APP + 23;
#[cfg(debug_assertions)]
const DIAGNOSTIC: u32 = WM_APP + 24;
const SOURCE_DESTROYED: u32 = WM_APP + 26;
const HOVER_PROP: &str = "DotCalendar.NativeHover";
const KEY_FOCUS_PROP: &str = "DotCalendar.KeyboardFocus";

thread_local! {
    static ENGINES: RefCell<HashMap<isize, Engine>> = RefCell::new(HashMap::new());
    #[cfg(debug_assertions)]
    static TEST_SETTINGS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

struct Engine {
    compositor: Option<Compositor>,
    failures: u32,
    submitted: u64,
    completed: u64,
    pending_hover: Vec<(HWND, HWND, bool)>,
    last_error: Option<String>,
}
impl Engine {
    unsafe fn new(host: HWND) -> Self {
        let result = Compositor::new(host);
        if let Err(error) = &result {
            eprintln!("Native motion uses static rendering: {error}");
        }
        Self {
            compositor: None,
            failures: u32::from(result.is_err()),
            submitted: 0,
            completed: 0,
            pending_hover: vec![],
            last_error: result.as_ref().err().cloned(),
        }
        .with_compositor(result.ok())
    }
    fn with_compositor(mut self, compositor: Option<Compositor>) -> Self {
        self.compositor = compositor;
        self
    }
}

pub(super) unsafe fn contrast_mode() -> bool {
    #[cfg(debug_assertions)]
    if TEST_SETTINGS.with(|s| s.get() & 2 != 0) {
        return true;
    }
    high_contrast()
}
unsafe fn reduced_motion() -> bool {
    #[cfg(debug_assertions)]
    if TEST_SETTINGS.with(|s| s.get() & 1 != 0) {
        return true;
    }
    let mut enabled: i32 = 1;
    SystemParametersInfoW(
        SPI_GETCLIENTAREAANIMATION,
        0,
        &mut enabled as *mut _ as _,
        0,
    );
    enabled == 0 || contrast_mode()
}

#[derive(Clone, Copy)]
struct Theme {
    canvas: u32,
    surface: u32,
    elevated: u32,
    ink: u32,
    secondary: u32,
    muted: u32,
    border: u32,
    focus: u32,
    high_contrast: bool,
}
impl Theme {
    unsafe fn new(dark: bool) -> Self {
        if contrast_mode() {
            return Self {
                canvas: GetSysColor(COLOR_WINDOW),
                surface: GetSysColor(COLOR_WINDOW),
                elevated: GetSysColor(COLOR_BTNFACE),
                ink: GetSysColor(COLOR_WINDOWTEXT),
                secondary: GetSysColor(COLOR_WINDOWTEXT),
                muted: GetSysColor(COLOR_GRAYTEXT),
                border: GetSysColor(COLOR_WINDOWTEXT),
                focus: GetSysColor(COLOR_HIGHLIGHT),
                high_contrast: true,
            };
        }
        if dark {
            Self {
                canvas: BG_CANVAS,
                surface: BG_SURFACE,
                elevated: BG_ELEVATED,
                ink: TEXT_PRIMARY,
                secondary: TEXT_SECONDARY,
                muted: TEXT_MUTED,
                border: blend(BORDER_SUBTLE.0, BG_SURFACE, BORDER_SUBTLE.1),
                focus: BORDER_FOCUS,
                high_contrast: false,
            }
        } else {
            Self {
                canvas: rgb(246, 246, 248),
                surface: rgb(255, 255, 255),
                elevated: rgb(234, 235, 239),
                ink: rgb(27, 28, 32),
                secondary: rgb(85, 87, 98),
                muted: rgb(105, 107, 118),
                border: rgb(216, 218, 224),
                focus: BORDER_FOCUS,
                high_contrast: false,
            }
        }
    }
}

pub(super) struct UiState {
    pub view: View,
    list: HWND,
    memo: HWND,
    rows: Vec<String>,
    toasts: Vec<Toast>,
    serial: u64,
    pub palette_open: bool,
    pill: HWND,
}
impl UiState {
    pub fn new() -> Self {
        Self {
            view: View::Calendar,
            list: null_mut(),
            memo: null_mut(),
            rows: vec![],
            toasts: vec![],
            serial: 0,
            palette_open: false,
            pill: null_mut(),
        }
    }
}
struct Toast {
    hwnd: HWND,
    trigger: HWND,
    expires: Instant,
}

unsafe fn app_ptr(host: HWND) -> *const RefCell<App> {
    GetWindowLongPtrW(host, GWLP_USERDATA) as _
}

/// The engine is removed from the registry during Win32 calls because painting
/// and destruction can synchronously reenter a native window procedure.
unsafe fn with_engine(host: HWND, action: impl FnOnce(&mut Engine)) {
    let mut engine = ENGINES
        .with(|map| map.borrow_mut().remove(&(host as isize)))
        .unwrap_or_else(|| Engine::new(host));
    action(&mut engine);
    KillTimer(host, UI_TIMER);
    if let Some(delay) = engine
        .compositor
        .as_ref()
        .and_then(Compositor::next_deadline_ms)
    {
        SetTimer(host, UI_TIMER, delay.max(USER_TIMER_MINIMUM), None);
    }
    ENGINES.with(|map| {
        map.borrow_mut().insert(host as isize, engine);
    });
}

unsafe fn animate(host: HWND, source: HWND, motion: Motion, finish: Finish) {
    if reduced_motion() {
        // Windows cannot present a 0.01ms frame. Apply the final state directly.
        let _logical_duration = REDUCED_DURATION_MS;
        if finish == Finish::Destroy {
            DestroyWindow(source);
        }
        return;
    }
    let dpi = GetDpiForWindow(host).max(96);
    with_engine(host, |engine| {
        if let Some(compositor) = engine.compositor.as_mut() {
            #[cfg(debug_assertions)]
            let injected_failure = TEST_SETTINGS.with(|s| s.get() & 4 != 0);
            #[cfg(not(debug_assertions))]
            let injected_failure = false;
            let result = if injected_failure {
                Err("QA simulated composition unavailable".into())
            } else {
                compositor.start(source, motion, dpi, finish)
            };
            match result {
                Ok(()) => {
                    engine.submitted += 1;
                    return;
                }
                Err(error) => {
                    engine.failures += 1;
                    if engine.last_error.as_ref() != Some(&error) {
                        eprintln!("Native motion uses static rendering: {error}");
                    }
                    engine.last_error = Some(error);
                }
            }
        }
        if finish == Finish::Destroy {
            DestroyWindow(source);
        }
    });
    InvalidateRect(host, null(), 0);
}

unsafe fn settle(host: HWND, all: bool) {
    with_engine(host, |engine| {
        if let Some(compositor) = engine.compositor.as_mut() {
            compositor.finish_due(all);
            engine.completed = compositor.completed;
        }
        engine.pending_hover.retain(|&(button, sprite, hover)| {
            if IsWindow(sprite) != 0 {
                return true;
            }
            if IsWindow(button) != 0 {
                SetPropW(
                    button,
                    w(HOVER_PROP).as_ptr(),
                    if hover { 1usize as HANDLE } else { null_mut() },
                );
                InvalidateRect(button, null(), 0);
            }
            false
        });
    });
}

pub(super) unsafe fn cleanup(host: HWND) {
    KillTimer(host, UI_TIMER);
    KillTimer(host, UI_EXPIRY);
    let engine = ENGINES.with(|map| map.borrow_mut().remove(&(host as isize)));
    drop(engine);
}

struct ButtonState {
    host: HWND,
    hovering: bool,
    pressed: bool,
}
unsafe fn install_button(host: HWND, button: HWND) {
    // Keep the standard push-button accessibility provider. Painting a
    // BS_OWNERDRAW control alone exposes a generic Pane on this Windows host.
    let style = GetWindowLongPtrW(button, GWL_STYLE);
    if style & BS_TYPEMASK as isize == BS_OWNERDRAW as isize {
        SendMessageW(button, BM_SETSTYLE, BS_PUSHBUTTON as usize, 1);
    }
    let state = Box::new(ButtonState {
        host,
        hovering: false,
        pressed: false,
    });
    let raw = Box::into_raw(state);
    if SetWindowSubclass(button, Some(button_proc), 21, raw as usize) == 0 {
        drop(Box::from_raw(raw));
    }
}
unsafe extern "system" fn button_proc(
    h: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let state = data as *mut ButtonState;
    // Preserve native button/radio semantics while painting their shared style.
    if msg == WM_PAINT {
        let mut ps = zeroed();
        let dc = BeginPaint(h, &mut ps);
        let mut bounds = zeroed();
        GetClientRect(h, &mut bounds);
        {
            let native_state = SendMessageW(h, BM_GETSTATE, 0, 0) as u32;
            let item = DRAWITEMSTRUCT {
                CtlType: ODT_BUTTON,
                CtlID: GetDlgCtrlID(h) as u32,
                hwndItem: h,
                hDC: dc,
                rcItem: bounds,
                itemState: (if GetFocus() == h { ODS_FOCUS } else { 0 })
                    | (if native_state & BST_PUSHED != 0 {
                        ODS_SELECTED
                    } else {
                        0
                    })
                    | (if IsWindowEnabled(h) == 0 {
                        ODS_DISABLED
                    } else {
                        0
                    }),
                ..zeroed()
            };
            SendMessageW(
                GetParent(h),
                WM_DRAWITEM,
                item.CtlID as usize,
                &item as *const _ as isize,
            );
        }
        EndPaint(h, &ps);
        return 0;
    }
    match msg {
        WM_MOUSEMOVE if !(*state).hovering => {
            (*state).hovering = true;
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: h,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut track);
            PostMessageW((*state).host, ANIMATE_BUTTON, h as usize, 2);
        }
        WM_MOUSELEAVE => {
            (*state).hovering = false;
            PostMessageW((*state).host, ANIMATE_BUTTON, h as usize, 3);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            RemovePropW(h, w(KEY_FOCUS_PROP).as_ptr());
            (*state).pressed = true;
            PostMessageW((*state).host, ANIMATE_BUTTON, h as usize, 1);
        }
        WM_KEYDOWN if wp as u16 == VK_SPACE && lp & (1 << 30) == 0 => {
            (*state).pressed = true;
            PostMessageW((*state).host, ANIMATE_BUTTON, h as usize, 1);
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED | WM_KILLFOCUS => {
            if (*state).pressed {
                (*state).pressed = false;
                PostMessageW((*state).host, ANIMATE_BUTTON, h as usize, 0);
            }
        }
        WM_KEYUP if wp as u16 == VK_SPACE => {
            if (*state).pressed {
                (*state).pressed = false;
                PostMessageW((*state).host, ANIMATE_BUTTON, h as usize, 0);
            }
        }
        WM_SETFOCUS => {
            if GetKeyState(VK_LBUTTON as i32) >= 0 {
                SetPropW(h, w(KEY_FOCUS_PROP).as_ptr(), 1usize as HANDLE);
            }
        }
        WM_KEYDOWN if [VK_LEFT, VK_RIGHT, VK_HOME, VK_END].contains(&(wp as u16)) => {
            let id = GetDlgCtrlID(h);
            if (MONTH_TAB..=SHOWCASE).contains(&id) {
                let next = match wp as u16 {
                    VK_HOME => MONTH_TAB,
                    VK_END => SHOWCASE,
                    VK_LEFT => MONTH_TAB + (id - MONTH_TAB + 3) % 4,
                    _ => MONTH_TAB + (id - MONTH_TAB + 1) % 4,
                };
                SetFocus(GetDlgItem((*state).host, next));
                PostMessageW((*state).host, WM_COMMAND, next as usize, 0);
                return 0;
            }
        }
        WM_NCDESTROY => {
            let stamp = crate::composition::source_stamp(h);
            if stamp != 0 {
                PostMessageW((*state).host, SOURCE_DESTROYED, h as usize, stamp as isize);
            }
            RemoveWindowSubclass(h, Some(button_proc), 21);
            RemovePropW(h, w(HOVER_PROP).as_ptr());
            RemovePropW(h, w(KEY_FOCUS_PROP).as_ptr());
            drop(Box::from_raw(data as *mut ButtonState));
            return DefSubclassProc(h, msg, wp, lp);
        }
        _ => {}
    }
    DefSubclassProc(h, msg, wp, lp)
}

pub(super) unsafe fn init(a: &mut App) {
    a.ui = UiState::new();
    for (id, title) in [
        (OPEN_PALETTE, "명령 검색  Ctrl K"),
        (MONTH_TAB, "월간"),
        (DAY_TAB, "하루"),
        (DONE_TAB, "완료"),
        (SHOWCASE, "컴포넌트"),
        (DEMO_PRIMARY, "Primary"),
        (DEMO_SECONDARY, "Secondary"),
        (DEMO_GHOST, "Ghost"),
        (DEMO_TOAST, "알림 쌓기"),
    ] {
        let style = if (MONTH_TAB..=SHOWCASE).contains(&id) {
            BS_AUTORADIOBUTTON as u32
                | BS_PUSHLIKE as u32
                | if id == MONTH_TAB { WS_GROUP } else { 0 }
        } else {
            BS_OWNERDRAW as u32 | WS_GROUP
        };
        let h = control(
            a.hwnd,
            "BUTTON",
            title,
            style | WS_TABSTOP,
            id,
            a.fonts.body,
        );
        a.buttons.push((id, h));
    }
    for &(id, h) in &a.buttons {
        if id != SHOW_GOOGLE {
            install_button(a.hwnd, h);
        }
    }
    a.ui.list = control(
        a.hwnd,
        "LISTBOX",
        "일정 목록",
        WS_TABSTOP | WS_VSCROLL | LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32,
        EVENT_LIST,
        a.fonts.body,
    );
    a.ui.memo = control(
        a.hwnd,
        "EDIT",
        "",
        WS_TABSTOP | WS_VSCROLL | ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32,
        EVENT_MEMO,
        a.fonts.body,
    );
    SetWindowTheme(a.ui.list, w("").as_ptr(), w("").as_ptr());
    SendMessageW(a.ui.list, LB_SETITEMHEIGHT, 0, a.px(38) as isize);
}

pub(super) fn custom_id(id: i32) -> bool {
    (OPEN_PALETTE..=DEMO_TOAST).contains(&id)
}

pub(super) unsafe fn layout(a: &App) {
    let r = a.client();
    let p = |v| a.px(v);
    for (index, id) in [MONTH_TAB, DAY_TAB, DONE_TAB, SHOWCASE].iter().enumerate() {
        let h = GetDlgItem(a.hwnd, *id);
        MoveWindow(h, p(16 + index as i32 * 76), p(44), p(76), p(34), 1);
        let style = GetWindowLongPtrW(h, GWL_STYLE);
        SetWindowLongPtrW(
            h,
            GWL_STYLE,
            if a.ui.view.index() == index {
                style | WS_TABSTOP as isize
            } else {
                style & !(WS_TABSTOP as isize)
            },
        );
        SendMessageW(
            h,
            BM_SETCHECK,
            if a.ui.view.index() == index {
                BST_CHECKED as usize
            } else {
                BST_UNCHECKED as usize
            },
            0,
        );
    }
    MoveWindow(
        GetDlgItem(a.hwnd, OPEN_PALETTE),
        p(490),
        p(44),
        p(150),
        p(34),
        1,
    );
    ShowWindow(GetDlgItem(a.hwnd, AGENDA), SW_HIDE);
    let show_list = matches!(a.ui.view, View::Day | View::Completed);
    for h in [a.ui.list, a.ui.memo] {
        ShowWindow(h, if show_list { SW_SHOW } else { SW_HIDE });
        SendMessageW(h, WM_SETFONT, a.fonts.body as usize, 0);
    }
    let content = rect(p(24), p(148), r.right - p(48), r.bottom - p(200));
    let list_width = (content.right - content.left) * 47 / 100;
    MoveWindow(
        a.ui.list,
        content.left,
        content.top,
        list_width,
        content.bottom - content.top,
        1,
    );
    MoveWindow(
        a.ui.memo,
        content.left + list_width + p(24),
        content.top,
        content.right - content.left - list_width - p(24),
        content.bottom - content.top,
        1,
    );
    let demo = a.ui.view == View::Showcase;
    for (index, id) in [DEMO_PRIMARY, DEMO_SECONDARY, DEMO_GHOST, DEMO_TOAST]
        .iter()
        .enumerate()
    {
        let h = GetDlgItem(a.hwnd, *id);
        ShowWindow(h, if demo { SW_SHOW } else { SW_HIDE });
        let (x, y, width) = if index < 3 {
            (42 + index as i32 * 128, 250, 116)
        } else {
            (42, 394, 170)
        };
        MoveWindow(h, p(x), p(y), p(width), p(40), 1);
    }
    for (index, toast) in a.ui.toasts.iter().enumerate() {
        let raw = GetWindowLongPtrW(toast.hwnd, GWLP_USERDATA) as *mut Surface;
        if !raw.is_null() && (*raw).dpi != a.dpi {
            (*raw).dpi = a.dpi;
            (*raw).fonts = Fonts::new(a.dpi);
            let close = GetDlgItem(toast.hwnd, 1);
            SendMessageW(close, WM_SETFONT, (*raw).fonts.small as usize, 1);
            MoveWindow(close, p(300), p(20), p(38), p(28), 1);
            SetWindowRgn(
                toast.hwnd,
                CreateRoundRectRgn(0, 0, p(344) + 1, p(68) + 1, p(12), p(12)),
                1,
            );
        }
        MoveWindow(
            toast.hwnd,
            r.right - p(368),
            r.bottom - p(104 + index as i32 * 78),
            p(344),
            p(68),
            1,
        );
        // MoveWindow can preserve old client pixels. A DPI/font or theme change
        // must repaint the complete toast, not only its newly exposed region.
        RedrawWindow(
            toast.hwnd,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
    }
}

pub(super) unsafe fn refresh_view(a: &mut App) {
    if !matches!(a.ui.view, View::Day | View::Completed) {
        return;
    }
    let rows: Vec<_> = if a.ui.view == View::Day {
        a.cells
            .iter()
            .find(|c| c.date == a.selected)
            .map(|c| agenda_rows(c, &a.events))
            .unwrap_or_default()
    } else {
        let mut events: Vec<_> = a
            .events
            .iter()
            .filter(|e| e.completed && (a.prefs.show_google || !google::is_imported_event(e)))
            .collect();
        events.sort_unstable_by_key(|e| (&e.date, &e.time, &e.id));
        events
            .into_iter()
            .map(|e| (e.id.clone(), format!("✓ {}  {}", e.date, e.title)))
            .collect()
    };
    let selected = a
        .selected_id
        .as_ref()
        .and_then(|id| rows.iter().position(|(row, _)| row == id))
        .unwrap_or(0);
    set_agenda_items(a.ui.list, &rows, selected);
    a.ui.rows = rows.into_iter().map(|(id, _)| id).collect();
    select_row(a);
    InvalidateRect(a.hwnd, null(), 0);
}
unsafe fn select_row(a: &mut App) {
    let index = SendMessageW(a.ui.list, LB_GETCURSEL, 0, 0);
    a.selected_id = usize::try_from(index)
        .ok()
        .and_then(|i| a.ui.rows.get(i))
        .cloned();
    let content = a
        .selected_id
        .as_ref()
        .and_then(|id| a.events.iter().find(|e| &e.id == id))
        .map(memo_content)
        .unwrap_or_else(|| "이 보기에는 일정이 없습니다.".into());
    SetWindowTextW(a.ui.memo, w(&content).as_ptr());
    a.redraw_buttons();
}

pub(super) unsafe fn paint_header(a: &App, dc: HDC) {
    let t = Theme::new(a.prefs.dark);
    let r = a.client();
    let p = |v| a.px(v);
    fill(dc, rect(0, 0, r.right, p(84)), t.surface);
    text(
        dc,
        &format!("{}년 {:02}월", a.month.year, a.month.month),
        rect(p(24), p(9), p(232), p(26)),
        a.fonts.strong,
        t.ink,
        DT_LEFT,
    );
    round(dc, rect(p(16), p(44), p(304), p(34)), t.canvas, p(12));
}

pub(super) unsafe fn paint_content(a: &App, dc: HDC) -> bool {
    if a.ui.view == View::Calendar {
        return false;
    }
    let t = Theme::new(a.prefs.dark);
    let p = |v| a.px(v);
    let r = a.client();
    if a.ui.view == View::Showcase {
        text(
            dc,
            "작은 동작, 분명한 피드백",
            rect(p(32), p(102), r.right - p(64), p(30)),
            a.fonts.strong,
            t.ink,
            DT_LEFT,
        );
        text(
            dc,
            "마우스와 키보드로 네이티브 컴포넌트를 직접 사용해 보세요.",
            rect(p(32), p(139), r.right - p(64), p(24)),
            a.fonts.body,
            t.secondary,
            DT_LEFT,
        );
        for (y, title, body) in [
            (
                188,
                "01  Tactile buttons",
                "누르기 · Space · Tab으로 초점을 확인하세요.",
            ),
            (
                332,
                "02  Stacked toast",
                "알림을 추가하고 Escape 또는 닫기 버튼으로 정리하세요.",
            ),
        ] {
            round(
                dc,
                rect(p(28), p(y), r.right - p(56), p(126)),
                t.border,
                p(18),
            );
            round(
                dc,
                rect(p(29), p(y + 1), r.right - p(58), p(124)),
                t.surface,
                p(18),
            );
            text(
                dc,
                title,
                rect(p(42), p(y + 10), p(360), p(25)),
                a.fonts.strong,
                t.ink,
                DT_LEFT,
            );
            text(
                dc,
                body,
                rect(p(42), p(y + 36), r.right - p(90), p(22)),
                a.fonts.small,
                t.secondary,
                DT_LEFT,
            );
        }
        text(
            dc,
            "상단의 보기 탭은 ← / →, 명령 검색은 Ctrl+K로 사용할 수 있습니다.",
            rect(p(32), p(469), r.right - p(64), p(22)),
            a.fonts.small,
            t.secondary,
            DT_LEFT,
        );
        let static_fallback = ENGINES.with(|map| {
            map.borrow()
                .get(&(a.hwnd as isize))
                .is_some_and(|e| e.last_error.is_some())
        });
        if static_fallback || reduced_motion() {
            text(
                dc,
                if static_fallback {
                    "이 환경에서는 일부 동작 효과를 생략합니다."
                } else {
                    "Windows의 동작 줄이기 설정을 적용하고 있습니다."
                },
                rect(p(32), p(495), r.right - p(64), p(22)),
                a.fonts.small,
                t.secondary,
                DT_LEFT,
            );
        }
    } else {
        let title = if a.ui.view == View::Day {
            format!("{} · 하루 일정", a.selected)
        } else {
            "완료한 일정".into()
        };
        text(
            dc,
            &title,
            rect(p(24), p(103), r.right - p(180), p(30)),
            a.fonts.strong,
            t.ink,
            DT_LEFT,
        );
        text(
            dc,
            &format!("{:>4}개", a.ui.rows.len()),
            rect(r.right - p(128), p(103), p(104), p(30)),
            a.fonts.body,
            t.secondary,
            DT_RIGHT,
        );
        if a.ui.rows.is_empty() {
            text(
                dc,
                "선택한 보기의 일정이 없습니다.",
                rect(p(28), p(153), p(320), p(28)),
                a.fonts.body,
                t.secondary,
                DT_LEFT,
            );
        }
    }
    true
}

pub(super) unsafe fn draw_button(a: &App, item: &DRAWITEMSTRUCT) {
    let t = Theme::new(a.prefs.dark);
    let p = |v| a.px(v);
    let id = item.CtlID as i32;
    let role = if id == ADD || id == DEMO_PRIMARY {
        ButtonRole::Primary
    } else if id == MENU
        || id == OPEN_PALETTE
        || id == DEMO_GHOST
        || (MONTH_TAB..=SHOWCASE).contains(&id)
    {
        ButtonRole::Ghost
    } else {
        ButtonRole::Secondary
    };
    let selected = (MONTH_TAB..=SHOWCASE).contains(&id)
        && a.ui.view.index() == (id - MONTH_TAB) as usize
        && IsWindow(a.ui.pill) == 0;
    let hover = !GetPropW(item.hwndItem, w(HOVER_PROP).as_ptr()).is_null();
    let base = if selected {
        t.elevated
    } else {
        match role {
            ButtonRole::Primary => t.focus,
            ButtonRole::Secondary => t.elevated,
            ButtonRole::Ghost => t.surface,
        }
    };
    let background = if hover && !t.high_contrast {
        blend(0xffffff, base, 0.055)
    } else {
        base
    };
    fill(
        item.hDC,
        item.rcItem,
        if (MONTH_TAB..=SHOWCASE).contains(&id) {
            t.canvas
        } else {
            t.surface
        },
    );
    let mut inside = item.rcItem;
    InflateRect(&mut inside, -p(4), -p(4));
    if item.itemState & ODS_SELECTED != 0 && reduced_motion() {
        let inset_x = ((inside.right - inside.left) as f32 * 0.015).round() as i32;
        let inset_y = ((inside.bottom - inside.top) as f32 * 0.015).round() as i32;
        InflateRect(&mut inside, -inset_x, -inset_y);
    }
    round(
        item.hDC,
        inside,
        if selected || role != ButtonRole::Ghost {
            t.border
        } else {
            background
        },
        p(12),
    );
    InflateRect(&mut inside, -p(1), -p(1));
    round(item.hDC, inside, background, p(10));
    if role != ButtonRole::Ghost && !t.high_contrast {
        fill(
            item.hDC,
            rect(
                inside.left + p(6),
                inside.top,
                inside.right - inside.left - p(12),
                p(1),
            ),
            blend(
                SHADOW_INSET_HIGHLIGHT.0,
                background,
                SHADOW_INSET_HIGHLIGHT.1,
            ),
        );
    }
    let foreground = if item.itemState & ODS_DISABLED != 0 {
        t.muted
    } else if role == ButtonRole::Primary {
        if t.high_contrast {
            GetSysColor(COLOR_HIGHLIGHTTEXT)
        } else {
            0xffffff
        }
    } else {
        t.ink
    };
    text(
        item.hDC,
        &label(item.hwndItem),
        inside,
        a.fonts.body,
        foreground,
        DT_CENTER,
    );
    if item.itemState & ODS_FOCUS != 0
        && !GetPropW(item.hwndItem, w(KEY_FOCUS_PROP).as_ptr()).is_null()
    {
        let mut focus = item.rcItem;
        // The painted body is inset 4 DIP: a 2 DIP ring leaves a 2 DIP gap.
        for _ in 0..p(2).max(1) {
            line(item.hDC, focus, t.focus);
            InflateRect(&mut focus, -1, -1);
        }
    }
}

struct Surface {
    color: u32,
    text: String,
    dark: bool,
    dpi: u32,
    fonts: Fonts,
    toast: bool,
    owner: HWND,
}
unsafe extern "system" fn surface_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            h,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let raw = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Surface;
    if raw.is_null() {
        return DefWindowProcW(h, msg, wp, lp);
    }
    let s = &*raw;
    let p = |v| v * s.dpi as i32 / 96;
    // Re-evaluate accessibility colors at paint time, including a toast that
    // was created before Windows changed its high-contrast preference.
    let theme = Theme::new(s.dark);
    let color = if s.toast { theme.elevated } else { s.color };
    match msg {
        WM_ERASEBKGND => 1,
        WM_DRAWITEM if s.toast => {
            let item = &*(lp as *const DRAWITEMSTRUCT);
            let mut r = item.rcItem;
            fill(item.hDC, r, color);
            InflateRect(&mut r, -p(4), -p(4));
            let hover = !GetPropW(item.hwndItem, w(HOVER_PROP).as_ptr()).is_null();
            if hover {
                round(item.hDC, r, blend(0xffffff, color, 0.055), p(8));
            }
            text(item.hDC, "×", r, s.fonts.strong, theme.secondary, DT_CENTER);
            if item.itemState & ODS_FOCUS != 0 {
                let mut focus = item.rcItem;
                InflateRect(&mut focus, -p(1), -p(1));
                for _ in 0..p(2).max(1) {
                    line(item.hDC, focus, theme.focus);
                    InflateRect(&mut focus, -1, -1);
                }
            }
            1
        }
        WM_NCHITTEST if !s.toast => HTTRANSPARENT as isize,
        WM_COMMAND if wp & 0xffff == 1 => {
            PostMessageW(s.owner, TOAST_CLOSE, h as usize, 0);
            0
        }
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(h, &mut ps);
            let mut r = zeroed();
            GetClientRect(h, &mut r);
            fill(dc, r, color);
            if s.toast {
                line(dc, r, theme.border);
                text(
                    dc,
                    "✓",
                    rect(p(16), p(23), p(ICON_DIP), p(ICON_DIP)),
                    s.fonts.body,
                    theme.focus,
                    DT_CENTER,
                );
                text(
                    dc,
                    &s.text,
                    rect(p(48), p(10), r.right - p(92), r.bottom - p(20)),
                    s.fonts.body,
                    theme.ink,
                    DT_LEFT | DT_END_ELLIPSIS,
                );
            }
            EndPaint(h, &ps);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(h, GWLP_USERDATA, 0);
            drop(Box::from_raw(raw));
            DefWindowProcW(h, msg, wp, lp)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
unsafe fn surface(owner: HWND, bounds: RECT, color: u32, text_value: &str, toast: bool) -> HWND {
    let dpi = GetDpiForWindow(owner).max(96);
    let ptr = app_ptr(owner);
    let dark = if ptr.is_null() {
        true
    } else {
        (*ptr).borrow().prefs.dark
    };
    let state = Box::new(Surface {
        color,
        text: text_value.into(),
        dark,
        dpi,
        fonts: Fonts::new(dpi),
        toast,
        owner,
    });
    let raw = Box::into_raw(state);
    let h = CreateWindowExW(
        if toast {
            WS_EX_CONTROLPARENT
        } else {
            WS_EX_TRANSPARENT
        },
        w("DotCalendarSurface").as_ptr(),
        w(text_value).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
        bounds.left,
        bounds.top,
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
        owner,
        null_mut(),
        GetModuleHandleW(null()),
        raw as _,
    );
    if h.is_null() {
        drop(Box::from_raw(raw));
        return h;
    }
    if toast {
        SetWindowRgn(
            h,
            CreateRoundRectRgn(
                0,
                0,
                bounds.right - bounds.left + 1,
                bounds.bottom - bounds.top + 1,
                12 * dpi as i32 / 96,
                12 * dpi as i32 / 96,
            ),
            1,
        );
        let close = control(
            h,
            "BUTTON",
            "알림 닫기",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            1,
            (*raw).fonts.small,
        );
        install_button(owner, close);
        MoveWindow(
            close,
            bounds.right - bounds.left - 44 * dpi as i32 / 96,
            20 * dpi as i32 / 96,
            38 * dpi as i32 / 96,
            28 * dpi as i32 / 96,
            1,
        );
    }
    h
}

unsafe fn hover(host: HWND, button: HWND, on: bool) {
    if reduced_motion() {
        SetPropW(
            button,
            w(HOVER_PROP).as_ptr(),
            if on { 1usize as HANDLE } else { null_mut() },
        );
        InvalidateRect(button, null(), 0);
        return;
    }
    // Fade a translucent highlight; only the compositor changes its opacity.
    // Hit geometry and the accessible BUTTON remain fixed throughout.
    with_engine(host, |engine| {
        engine.pending_hover.retain(|&(source, sprite, _)| {
            if source != button {
                return true;
            }
            if let Some(c) = engine.compositor.as_mut() {
                c.cancel(sprite);
            }
            if IsWindow(sprite) != 0 {
                DestroyWindow(sprite);
            }
            false
        });
    });
    let mut r = zeroed();
    GetWindowRect(button, &mut r);
    MapWindowPoints(null_mut(), host, &mut r as *mut RECT as *mut POINT, 2);
    let dpi = GetDpiForWindow(host).max(96) as i32;
    InflateRect(&mut r, -5 * dpi / 96, -5 * dpi / 96);
    let h = surface(host, r, 0xffffff, "", false);
    if h.is_null() {
        return;
    }
    let radius = 10 * dpi / 96;
    SetWindowRgn(
        h,
        CreateRoundRectRgn(
            0,
            0,
            r.right - r.left + 1,
            r.bottom - r.top + 1,
            radius,
            radius,
        ),
        1,
    );
    let motion = Motion {
        from: Pose {
            opacity: if on { 0.0 } else { 0.055 },
            ..Pose::IDENTITY
        },
        to: Pose {
            opacity: if on { 0.055 } else { 0.0 },
            ..Pose::IDENTITY
        },
        milliseconds: DURATION_FAST,
        curve: EASE_OUT_EXPO,
    };
    RemovePropW(button, w(HOVER_PROP).as_ptr());
    InvalidateRect(button, null(), 0);
    animate(host, h, motion, Finish::Destroy);
    with_engine(host, |engine| engine.pending_hover.push((button, h, on)));
    settle(host, false);
}

unsafe fn notify(host: HWND, message: &str) {
    let ptr = app_ptr(host);
    if ptr.is_null() {
        return;
    }
    let trigger = GetFocus();
    let (bounds, color) = {
        let a = (*ptr).borrow();
        let r = a.client();
        (
            rect(
                r.right - a.px(368),
                r.bottom - a.px(104),
                a.px(344),
                a.px(68),
            ),
            Theme::new(a.prefs.dark).elevated,
        )
    };
    let oldest = {
        let mut a = (*ptr).borrow_mut();
        if a.ui.toasts.len() >= 3 {
            a.ui.toasts.pop()
        } else {
            None
        }
    };
    if let Some(old) = oldest {
        with_engine(host, |e| {
            if let Some(c) = e.compositor.as_mut() {
                c.cancel(old.hwnd);
            }
        });
        DestroyWindow(old.hwnd);
    }
    let h = surface(host, bounds, color, message, true);
    if h.is_null() {
        return;
    }
    {
        let mut a = (*ptr).borrow_mut();
        a.ui.toasts.insert(
            0,
            Toast {
                hwnd: h,
                trigger,
                expires: Instant::now() + Duration::from_secs(6),
            },
        );
        layout(&a);
    }
    animate(host, h, Motion::toast(true), Finish::Restore);
    SetTimer(host, UI_EXPIRY, 1000, None);
    NotifyWinEvent(EVENT_SYSTEM_ALERT, h, OBJID_CLIENT, CHILDID_SELF as i32);
}
unsafe fn dismiss(host: HWND, target: HWND, restore_focus: bool) {
    let ptr = app_ptr(host);
    let toast = {
        let mut a = (*ptr).borrow_mut();
        let index =
            a.ui.toasts
                .iter()
                .position(|t| target.is_null() || t.hwnd == target);
        index.map(|i| a.ui.toasts.remove(i))
    };
    if let Some(toast) = toast {
        if restore_focus && IsWindow(toast.trigger) != 0 && IsWindowEnabled(toast.trigger) != 0 {
            SetFocus(toast.trigger);
        } else if restore_focus {
            SetFocus(GetDlgItem(host, OPEN_PALETTE));
        }
        animate(host, toast.hwnd, Motion::toast(false), Finish::Destroy);
    }
    if (*ptr).borrow().ui.toasts.is_empty() {
        KillTimer(host, UI_EXPIRY);
    }
}

pub(super) unsafe fn command(host: HWND, id: i32) -> bool {
    if !custom_id(id) {
        return false;
    }
    if id == OPEN_PALETTE {
        palette::show(host);
        return true;
    }
    if (MONTH_TAB..=SHOWCASE).contains(&id) {
        if !commit_inline(host) {
            return true;
        }
        let ptr = app_ptr(host);
        let previous = (*ptr).borrow().ui.view;
        let view = match id {
            MONTH_TAB => View::Calendar,
            DAY_TAB => View::Day,
            DONE_TAB => View::Completed,
            _ => View::Showcase,
        };
        {
            let mut a = (*ptr).borrow_mut();
            a.ui.view = view;
            refresh_view(&mut a);
            a.layout();
            a.redraw();
        }
        if previous != view && !reduced_motion() {
            let old_pill = (*ptr).borrow().ui.pill;
            if IsWindow(old_pill) != 0 {
                with_engine(host, |e| {
                    if let Some(c) = e.compositor.as_mut() {
                        c.cancel(old_pill);
                    }
                });
                DestroyWindow(old_pill);
            }
            let h = GetDlgItem(host, id);
            let mut r = zeroed();
            GetWindowRect(h, &mut r);
            MapWindowPoints(null_mut(), host, &mut r as *mut RECT as *mut POINT, 2);
            let dpi = GetDpiForWindow(host).max(96) as i32;
            InflateRect(&mut r, -5 * dpi / 96, -5 * dpi / 96);
            let pill = surface(host, r, rgb(220, 220, 255), "", false);
            if !pill.is_null() {
                SetWindowRgn(
                    pill,
                    CreateRoundRectRgn(
                        0,
                        0,
                        r.right - r.left + 1,
                        r.bottom - r.top + 1,
                        10 * dpi / 96,
                        10 * dpi / 96,
                    ),
                    1,
                );
                (*ptr).borrow_mut().ui.pill = pill;
                (*ptr).borrow().redraw_buttons();
                let mut motion =
                    Motion::slide((previous.index() as f32 - view.index() as f32) * 76.0);
                motion.from.opacity = 0.09;
                motion.to.opacity = 0.09;
                // Only a translucent pill moves; accessible tab text and hit
                // rectangles retain their exact pixel positions throughout.
                animate(host, pill, motion, Finish::Destroy);
            }
        }
        return true;
    }
    let message = match id {
        DEMO_PRIMARY => "Primary 버튼을 눌렀습니다.",
        DEMO_SECONDARY => "Secondary 버튼을 눌렀습니다.",
        DEMO_GHOST => "Ghost 버튼을 눌렀습니다.",
        _ => "새 알림이 도착했습니다.",
    };
    (*app_ptr(host)).borrow_mut().ui.serial += 1;
    notify(host, message);
    true
}

pub(super) unsafe fn prepare_existing(host: HWND, id: i32) {
    if ![ADD, EDIT].contains(&id) {
        return;
    }
    let mut a = (*app_ptr(host)).borrow_mut();
    if a.ui.view == View::Calendar {
        return;
    }
    if id == EDIT && a.ui.view == View::Completed {
        if let Some(date) = a
            .selected_id
            .as_ref()
            .and_then(|id| a.events.iter().find(|e| &e.id == id))
            .and_then(|e| Date::parse(&e.date).ok())
        {
            a.selected = date;
            a.month = Date { day: 1, ..date };
        }
    }
    a.ui.view = View::Calendar;
    a.rebuild_cells();
    a.layout();
    a.redraw_buttons();
}

pub(super) unsafe fn before_message(
    host: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> Option<LRESULT> {
    match msg {
        SOURCE_DESTROYED => {
            if ENGINES.with(|map| map.borrow().contains_key(&(host as isize))) {
                with_engine(host, |engine| {
                    if let Some(c) = engine.compositor.as_mut() {
                        c.forget(wp as HWND, lp as usize);
                    }
                });
            }
            Some(0)
        }
        ANIMATE_BUTTON => {
            let h = wp as HWND;
            if IsWindow(h) != 0 {
                if lp >= 2 {
                    hover(host, h, lp == 2);
                } else {
                    animate(
                        host,
                        h,
                        Motion::press(lp == 1),
                        if lp == 1 {
                            Finish::Hold
                        } else {
                            Finish::Restore
                        },
                    );
                }
            }
            Some(0)
        }
        WM_TIMER if wp == UI_TIMER => {
            settle(host, false);
            (*app_ptr(host)).borrow().redraw_buttons();
            Some(0)
        }
        WM_TIMER if wp == UI_EXPIRY => {
            let expired: Vec<_> = (*app_ptr(host))
                .borrow()
                .ui
                .toasts
                .iter()
                .filter(|t| t.expires <= Instant::now())
                .map(|t| t.hwnd)
                .collect();
            for h in expired {
                dismiss(host, h, false);
            }
            Some(0)
        }
        TOAST_CLOSE => {
            dismiss(host, wp as HWND, true);
            Some(0)
        }
        VIEW_REFRESH => {
            refresh_view(&mut (*app_ptr(host)).borrow_mut());
            Some(0)
        }
        WM_COMMAND if wp & 0xffff == EVENT_LIST as usize && wp >> 16 == LBN_SELCHANGE as usize => {
            select_row(&mut (*app_ptr(host)).borrow_mut());
            Some(0)
        }
        WM_COMMAND if wp & 0xffff == EVENT_LIST as usize && wp >> 16 == LBN_DBLCLK as usize => {
            super::command(host, VIEW);
            Some(0)
        }
        WM_SETTINGCHANGE => {
            settle(host, true);
            let a = (*app_ptr(host)).borrow();
            let theme = Theme::new(a.prefs.dark);
            for toast in &a.ui.toasts {
                let raw = GetWindowLongPtrW(toast.hwnd, GWLP_USERDATA) as *mut Surface;
                if !raw.is_null() {
                    (*raw).dark = a.prefs.dark;
                    (*raw).color = theme.elevated;
                    RedrawWindow(
                        toast.hwnd,
                        null(),
                        null_mut(),
                        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                    );
                }
            }
            a.redraw();
            a.redraw_buttons();
            None
        }
        WM_DPICHANGED => {
            settle(host, true);
            None
        }
        #[cfg(debug_assertions)]
        DIAGNOSTIC => {
            if wp == 20 && (96..=384).contains(&lp) {
                (*app_ptr(host)).borrow_mut().update_dpi(lp as u32);
                let a = (*app_ptr(host)).borrow();
                a.layout();
                a.redraw();
                return Some(1);
            }
            if wp == 10 {
                TEST_SETTINGS.with(|s| s.set(lp as u32));
                PostMessageW(host, WM_SETTINGCHANGE, 0, 0);
                return Some(1);
            }
            let result = ENGINES.with(|map| {
                map.borrow()
                    .get(&(host as isize))
                    .map(|e| match wp {
                        0 => e.compositor.as_ref().map_or(0, |c| c.count()) as isize,
                        1 => e.submitted as isize,
                        2 => e.completed as isize,
                        3 => e.failures as isize,
                        4 => isize::from(
                            e.compositor
                                .as_ref()
                                .and_then(Compositor::next_deadline_ms)
                                .is_some(),
                        ),
                        _ => 0,
                    })
                    .unwrap_or(0)
            });
            Some(result)
        }
        _ => None,
    }
}

pub(super) unsafe fn pretranslate(msg: &MSG, host: HWND) -> bool {
    if host.is_null() || IsWindowEnabled(host) == 0 {
        return false;
    }
    if msg.message == WM_KEYDOWN {
        if msg.wParam as u16 == VK_RETURN {
            let mut class = [0u16; 32];
            let n = GetClassNameW(msg.hwnd, class.as_mut_ptr(), 32);
            if String::from_utf16_lossy(&class[..n.max(0) as usize]).eq_ignore_ascii_case("Button")
            {
                SendMessageW(msg.hwnd, BM_CLICK, 0, 0);
                return true;
            }
        }
        if msg.wParam as u16 == b'K' as u16 && GetKeyState(VK_CONTROL as i32) < 0 {
            if palette_shortcut(msg.wParam as u16, true, msg.lParam & (1 << 30) != 0) {
                PostMessageW(host, WM_COMMAND, OPEN_PALETTE as usize, 0);
            }
            return true;
        }
        if msg.wParam as u16 == VK_ESCAPE && !(*app_ptr(host)).borrow().ui.toasts.is_empty() {
            dismiss(host, null_mut(), true);
            return true;
        }
        if msg.wParam as u16 == VK_TAB {
            let current = GetFocus();
            let next =
                GetNextDlgTabItem(host, current, i32::from(GetKeyState(VK_SHIFT as i32) < 0));
            if !next.is_null() {
                SetFocus(next);
                return true;
            }
        }
    }
    false
}

pub(super) unsafe fn register() -> Result<(), String> {
    {
        let class = w("DotCalendarSurface");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(surface_proc),
            hInstance: GetModuleHandleW(null()),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("네이티브 UI 클래스를 등록하지 못했습니다.".into());
        }
    }
    palette::register()
}
