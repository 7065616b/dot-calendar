use crate::{
    calendar_info, desktop, google,
    model::{for_each_occurrence, Date, Event},
    preferences::{self, Preferences},
    store::Store,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    ffi::c_void,
    mem::zeroed,
    ptr::{null, null_mut},
    rc::Rc,
    sync::{
        atomic::{AtomicIsize, Ordering},
        Arc, Mutex,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    Storage::Xps::*,
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetLocalTime, Threading::*},
    UI::{
        Controls::Dialogs::*, Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*,
        WindowsAndMessaging::*,
    },
};

const RELOAD: u32 = WM_APP + 1;
const SYNC_DONE: u32 = WM_APP + 2;
const TRAY: u32 = WM_APP + 3;
const REBUILD_WIDGET: u32 = WM_APP + 4;
const SHOW_WIDGET: u32 = WM_APP + 5;
const INLINE_COMMIT: u32 = WM_APP + 6;
const INLINE_CANCEL: u32 = WM_APP + 7;
const INLINE_EDIT_ID: i32 = 70;
const INLINE_SAVE_ID: i32 = 71;
const INLINE_DELETE_ID: i32 = 72;
const INLINE_CANCEL_ID: i32 = 73;
const TBM_GETPOS: u32 = WM_USER;
static CONTROLLER_WINDOW: AtomicIsize = AtomicIsize::new(0);
const PREV: i32 = 10;
const NEXT: i32 = 11;
const TODAY: i32 = 12;
const ADD: i32 = 13;
const SETTINGS: i32 = 14;
const SYNC: i32 = 15;
const MENU: i32 = 16;
const QUIT: i32 = 30;
const SHOW: i32 = 31;
const CONNECT: i32 = 32;
const BACKUP: i32 = 33;
const RESTORE: i32 = 34;
const PRINT: i32 = 35;
const LOCK: i32 = 36;
const DETACH: i32 = 37;
const DISCONNECT: i32 = 38;
const HIDE: i32 = 39;
const EDIT: i32 = 40;
const DELETE: i32 = 41;
const COMPLETE: i32 = 42;
const ABOUT: i32 = 43;
const DETAILS: i32 = 44;
const SHOW_GOOGLE: i32 = 45;
const VIEW: i32 = 46;
const AGENDA: i32 = 47;
const PALETTE: [(&str, &str); 6] = [
    ("기본", ""),
    ("파랑", "#DCEBFA"),
    ("초록", "#DCF1E3"),
    ("노랑", "#FFF0BF"),
    ("분홍", "#F9DDE5"),
    ("보라", "#E9E0FA"),
];
fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn rect(x: i32, y: i32, width: i32, height: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}
fn has(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}
fn event_row_rect(cell: RECT, row: usize, dpi: u32) -> RECT {
    let px = |v: i32| v * dpi as i32 / 96;
    rect(
        cell.left + px(6),
        cell.top + px(35) + row as i32 * px(23),
        cell.right - cell.left - px(12),
        px(21),
    )
}
fn event_row_hit(cell: RECT, x: i32, y: i32, count: usize, dpi: u32) -> Option<usize> {
    (0..count).find(|&row| has(event_row_rect(cell, row, dpi), x, y))
}
fn weekday(d: Date) -> u32 {
    ((d.ordinal() + 1) % 7) as u32
}
fn shift(d: Date, delta: i32) -> Date {
    Date::from_ordinal(d.ordinal().saturating_add(delta).clamp(0, 3_652_058)).unwrap()
}
fn month_shift(d: Date, n: i32) -> Date {
    let i = ((d.year - 1) * 12 + d.month as i32 - 1 + n).clamp(0, 9999 * 12 - 1);
    Date {
        year: i / 12 + 1,
        month: (i % 12 + 1) as u32,
        day: 1,
    }
}
fn color(s: &str) -> Option<u32> {
    let v = u32::from_str_radix(s.strip_prefix('#')?, 16).ok()?;
    Some((v & 255) << 16 | (v & 0xff00) | (v >> 16 & 255))
}
fn alpha_to_percent(alpha: u8) -> u32 {
    ((alpha as u32 * 100 + 127) / 255).clamp(40, 100)
}
fn percent_to_alpha(percent: u32) -> u8 {
    ((percent * 255 + 50) / 100) as u8
}
fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}
fn note_parts(value: &str) -> Option<(&str, &str)> {
    let value = value.trim_start();
    if value.is_empty() {
        return None;
    }
    let (title, notes) = value.split_once('\n').unwrap_or((value, ""));
    Some((title.trim(), notes))
}
unsafe fn fill(dc: HDC, r: RECT, c: u32) {
    SetDCBrushColor(dc, c);
    FillRect(dc, &r, GetStockObject(DC_BRUSH));
}
unsafe fn line(dc: HDC, r: RECT, c: u32) {
    SetDCBrushColor(dc, c);
    FrameRect(dc, &r, GetStockObject(DC_BRUSH));
}
unsafe fn text(dc: HDC, s: &str, mut r: RECT, font: HFONT, c: u32, flags: u32) {
    let old = SelectObject(dc, font);
    SetBkMode(dc, TRANSPARENT as i32);
    SetTextColor(dc, c);
    DrawTextW(
        dc,
        w(s).as_ptr(),
        -1,
        &mut r,
        DT_NOPREFIX | DT_SINGLELINE | DT_VCENTER | flags,
    );
    SelectObject(dc, old);
}
unsafe fn round(dc: HDC, r: RECT, c: u32, radius: i32) {
    SetDCBrushColor(dc, c);
    let old = SelectObject(dc, GetStockObject(DC_BRUSH));
    let pen = SelectObject(dc, GetStockObject(NULL_PEN));
    RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(dc, old);
    SelectObject(dc, pen);
}
unsafe fn label(h: HWND) -> String {
    let n = GetWindowTextLengthW(h);
    let mut b = vec![0; n as usize + 1];
    GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32);
    String::from_utf16_lossy(&b[..n as usize])
}
struct Fonts {
    body: HFONT,
    small: HFONT,
    strong: HFONT,
}
impl Fonts {
    unsafe fn new(dpi: u32) -> Self {
        unsafe fn f(n: i32, weight: i32, dpi: u32) -> HFONT {
            CreateFontW(
                -n * dpi as i32 / 96,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                w("Segoe UI").as_ptr(),
            )
        }
        Self {
            body: f(13, 400, dpi),
            small: f(12, 400, dpi),
            strong: f(15, 600, dpi),
        }
    }
}
impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for f in [self.body, self.small, self.strong] {
                DeleteObject(f);
            }
        }
    }
}
struct Cell {
    date: Date,
    date_key: String,
    lunar: String,
    holiday: String,
    events: Vec<usize>,
}

fn agenda_rows(cell: &Cell, events: &[Event]) -> Vec<(String, String)> {
    cell.events
        .iter()
        .map(|&index| {
            let event = &events[index];
            (
                event.id.clone(),
                format!(
                    "{}  {}",
                    event.time.as_deref().unwrap_or("종일"),
                    event.title
                ),
            )
        })
        .collect()
}

fn agenda_heading(date: Date, count: usize) -> String {
    format!(
        "{}년 {}월 {}일 · 일정 {}개",
        date.year, date.month, date.day, count
    )
}

unsafe fn set_agenda_items(hwnd: HWND, rows: &[(String, String)], selected: usize) {
    SendMessageW(hwnd, WM_SETREDRAW, 0, 0);
    SendMessageW(hwnd, LB_RESETCONTENT, 0, 0);
    for (_, title) in rows {
        if SendMessageW(hwnd, LB_ADDSTRING, 0, w(title).as_ptr() as isize) < 0 {
            // A partial native list must never select a different source ID.
            SendMessageW(hwnd, LB_RESETCONTENT, 0, 0);
            break;
        }
    }
    SendMessageW(hwnd, LB_SETCURSEL, selected, 0);
    SendMessageW(hwnd, WM_SETREDRAW, 1, 0);
    InvalidateRect(hwnd, null(), 1);
}

fn day_event<'a>(cells: &[Cell], events: &'a [Event], date: Date, id: &str) -> Option<&'a Event> {
    cells
        .iter()
        .find(|cell| cell.date == date)?
        .events
        .iter()
        .map(|&index| &events[index])
        .find(|event| event.id == id)
}

fn memo_content(event: &Event) -> String {
    let mut content = event.title.clone();
    if !event.notes.is_empty() {
        content.push_str("\r\n\r\n");
        content.push_str(&normalize_newlines(&event.notes).replace('\n', "\r\n"));
    }
    content
}

fn populate_cell_events(
    cells: &mut [Cell],
    events: &[Event],
    start: Date,
    end: Date,
    show_google: bool,
) {
    for cell in cells.iter_mut() {
        cell.events.clear();
    }
    // Sort source indices once; each day's entries inherit the same display order.
    let mut order: Vec<_> = (0..events.len())
        .filter(|&i| show_google || !google::is_imported_event(&events[i]))
        .collect();
    order.sort_unstable_by_key(|&i| (&events[i].time, &events[i].title, &events[i].id));
    let start_day = start.ordinal();
    for index in order {
        for_each_occurrence(
            std::slice::from_ref(&events[index]),
            start,
            end,
            |date, _| {
                cells[(date.ordinal() - start_day) as usize]
                    .events
                    .push(index);
            },
        );
    }
}

fn next_reminder(
    events: &[Event],
    notified: &HashMap<String, i64>,
    today: Date,
    now: i64,
) -> Option<(String, String, i64)> {
    let mut next: Option<(i64, Date, &Event, String)> = None;
    let end = shift(today, 8);
    for event in events {
        if event.completed {
            continue;
        }
        let (Some(minutes), Some(time)) = (event.reminder_minutes, event.time.as_deref()) else {
            continue;
        };
        let hh = time.get(..2).and_then(|v| v.parse().ok()).unwrap_or(0);
        let mm = time.get(3..5).and_then(|v| v.parse().ok()).unwrap_or(0);
        for_each_occurrence(std::slice::from_ref(event), today, end, |date, _| {
            let due = wall_seconds(date, hh, mm) - minutes as i64 * 60;
            if due < now - 60
                || next
                    .as_ref()
                    .is_some_and(|(prior_due, prior_date, prior, _)| {
                        (due, date, &event.time, &event.title, &event.id)
                            >= (
                                *prior_due,
                                *prior_date,
                                &prior.time,
                                &prior.title,
                                &prior.id,
                            )
                    })
            {
                return;
            }
            let key = format!("{}:{}:{}", event.id, date, time);
            if !notified.contains_key(&key) {
                next = Some((due, date, event, key));
            }
        });
    }
    next.map(|(due, _, event, key)| {
        (
            key,
            format!(
                "{} · {}",
                event.time.as_deref().unwrap_or_default(),
                event.title
            ),
            due,
        )
    })
}

struct InlineEditor {
    hwnd: HWND,
    buttons: [HWND; 3],
    date: Date,
    original: Option<Event>,
}

struct InlineKeys {
    parent: HWND,
    composing: bool,
    buttons: [HWND; 3],
}

struct App {
    hwnd: HWND,
    controller: HWND,
    store: Store,
    dpi: u32,
    fonts: Fonts,
    prefs: Preferences,
    month: Date,
    selected: Date,
    selected_id: Option<String>,
    inline: Option<InlineEditor>,
    cells: Vec<Cell>,
    grid_key: Option<(Date, bool, bool)>,
    events: Vec<Event>,
    buttons: Vec<(i32, HWND)>,
    status: String,
    sync_result: Arc<Mutex<Option<Result<String, String>>>>,
    busy: bool,
    notified: HashMap<String, i64>,
    pending: Option<(String, String, i64)>,
    taskbar_created: u32,
    quitting: bool,
}
impl App {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    fn colors(&self) -> (u32, u32, u32, u32) {
        if self.prefs.dark {
            (0x003a332b, 0x00f5eee6, 0x00c8bcb0, 0x0062564d)
        } else {
            (0x00f2ece4, 0x00332a22, 0x008a7562, 0x00d6c8b8)
        }
    }
    unsafe fn client(&self) -> RECT {
        let mut r = zeroed();
        GetClientRect(self.hwnd, &mut r);
        r
    }
    unsafe fn grid(&self) -> RECT {
        let r = self.client();
        rect(
            self.px(16),
            self.px(84),
            r.right - self.px(32),
            r.bottom - self.px(116),
        )
    }
    fn cell_rect(g: RECT, i: usize) -> RECT {
        let col = (i % 7) as i32;
        let row = (i / 7) as i32;
        RECT {
            left: g.left + (g.right - g.left) * col / 7,
            top: g.top + (g.bottom - g.top) * row / 6,
            right: g.left + (g.right - g.left) * (col + 1) / 7,
            bottom: g.top + (g.bottom - g.top) * (row + 1) / 6,
        }
    }
    fn cell_capacity(&self, r: RECT) -> usize {
        ((r.bottom - r.top - self.px(38)) / self.px(23)).max(0) as usize
    }
    unsafe fn hit(&self, x: i32, y: i32) -> Option<(usize, Option<usize>)> {
        let grid = self.grid();
        for i in 0..self.cells.len() {
            let mut r = Self::cell_rect(grid, i);
            if has(r, x, y) {
                let capacity = self.cell_capacity(r);
                r.right -= 1;
                r.bottom -= 1;
                let e = event_row_hit(r, x, y, capacity.min(self.cells[i].events.len()), self.dpi);
                return Some((i, e));
            }
        }
        None
    }
    unsafe fn init(&mut self, h: HWND) {
        self.hwnd = h;
        self.dpi = GetDpiForWindow(h).max(96);
        self.fonts = Fonts::new(self.dpi);
        self.buttons.clear();
        for (id, title) in [
            (PREV, "‹"),
            (NEXT, "›"),
            (TODAY, "오늘"),
            (ADD, "+ 메모"),
            (DELETE, "삭제"),
            (SYNC, "동기화"),
            (SETTINGS, "설정"),
            (MENU, "···"),
        ] {
            self.buttons.push((
                id,
                control(
                    h,
                    "BUTTON",
                    title,
                    WS_TABSTOP | BS_OWNERDRAW as u32,
                    id,
                    self.fonts.body,
                ),
            ));
        }
        self.buttons.push((
            SHOW_GOOGLE,
            control(
                h,
                "BUTTON",
                "Google 일정 표시",
                WS_TABSTOP | BS_CHECKBOX as u32,
                SHOW_GOOGLE,
                self.fonts.body,
            ),
        ));
        // The themed checkbox ignores WM_CTLCOLOR text colors on a dark surface.
        SetWindowTheme(
            self.buttons.last().unwrap().1,
            [0u16].as_ptr(),
            [0u16].as_ptr(),
        );
        for (id, title) in [(VIEW, "전체 메모"), (AGENDA, "하루 일정")] {
            self.buttons.push((
                id,
                control(
                    h,
                    "BUTTON",
                    title,
                    WS_TABSTOP | BS_OWNERDRAW as u32,
                    id,
                    self.fonts.body,
                ),
            ));
        }
        self.layout();
        if !self.refresh() {
            self.arm_reminders();
        }
        self.arm_sync();
    }
    unsafe fn arm_sync(&self) {
        KillTimer(self.hwnd, 3);
        if google::is_connected() {
            SetTimer(self.hwnd, 3, self.prefs.sync_minutes * 60_000, None);
        }
    }
    unsafe fn layout(&self) {
        let r = self.client();
        let mut x = r.right - self.px(20);
        for (id, h) in self.buttons.iter().rev() {
            if *id == SHOW_GOOGLE {
                MoveWindow(
                    *h,
                    r.right - self.px(178),
                    self.px(40),
                    self.px(158),
                    self.px(19),
                    1,
                );
                continue;
            }
            if *id == VIEW || *id == AGENDA {
                MoveWindow(
                    *h,
                    self.px(if *id == VIEW { 260 } else { 358 }),
                    self.px(40),
                    self.px(90),
                    self.px(19),
                    1,
                );
                continue;
            }
            let ww = match *id {
                MENU | PREV | NEXT => 36,
                TODAY | DELETE => 50,
                ADD => 74,
                SYNC => 68,
                _ => 64,
            };
            x -= self.px(ww);
            MoveWindow(*h, x, self.px(8), self.px(ww), self.px(28), 1);
            x -= self.px(6);
        }
        if let Some(editor) = &self.inline {
            if let Some(i) = self.cells.iter().position(|cell| cell.date == editor.date) {
                let panel = self.inline_panel_rect(i);
                let r = self.inline_text_rect(panel);
                MoveWindow(
                    editor.hwnd,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    1,
                );
                for (index, button) in editor.buttons.iter().enumerate() {
                    let r = self.inline_button_rect(panel, index);
                    MoveWindow(
                        *button,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        1,
                    );
                }
            }
        }
        self.redraw();
        self.redraw_buttons();
    }
    unsafe fn inline_panel_rect(&self, i: usize) -> RECT {
        let grid = self.grid();
        let cell = Self::cell_rect(grid, i);
        let width = self.px(268);
        let height = self.px(250);
        let left = (cell.left + self.px(8)).clamp(grid.left, grid.right - width);
        let top = (cell.top + self.px(8)).clamp(grid.top, grid.bottom - height);
        rect(left, top, width, height)
    }
    fn inline_text_rect(&self, panel: RECT) -> RECT {
        rect(
            panel.left + self.px(8),
            panel.top + self.px(36),
            panel.right - panel.left - self.px(16),
            panel.bottom - panel.top - self.px(78),
        )
    }
    fn inline_button_rect(&self, panel: RECT, index: usize) -> RECT {
        let width = self.px(76);
        rect(
            panel.right - self.px(8) - (3 - index) as i32 * (width + self.px(4)),
            panel.bottom - self.px(35),
            width,
            self.px(27),
        )
    }
    unsafe fn redraw(&self) {
        InvalidateRect(self.hwnd, null(), 0);
    }
    unsafe fn redraw_selection(&self, previous: Date) {
        let grid = self.grid();
        for (i, cell) in self.cells.iter().enumerate() {
            if cell.date == previous || cell.date == self.selected {
                InvalidateRect(self.hwnd, &Self::cell_rect(grid, i), 0);
            }
        }
        for (id, button) in &self.buttons {
            if *id == DELETE || *id == VIEW {
                let enabled = self.selected_id.is_some() as i32;
                if IsWindowEnabled(*button) != enabled {
                    EnableWindow(*button, enabled);
                }
            }
        }
    }
    unsafe fn redraw_buttons(&self) {
        for (id, h) in &self.buttons {
            if *id == DELETE || *id == VIEW {
                EnableWindow(*h, self.selected_id.is_some() as i32);
            } else if *id == SHOW_GOOGLE {
                SendMessageW(
                    *h,
                    BM_SETCHECK,
                    if self.prefs.show_google {
                        BST_CHECKED
                    } else {
                        BST_UNCHECKED
                    } as usize,
                    0,
                );
            }
            InvalidateRect(*h, null(), 0);
        }
    }
    unsafe fn refresh(&mut self) -> bool {
        match self.store.list_events(None) {
            Ok(e) => {
                if self.status.starts_with("불러오기 실패: ") {
                    self.status.clear();
                    self.redraw();
                }
                if e == self.events && !self.cells.is_empty() {
                    return false;
                }
                self.events = e;
            }
            Err(e) => {
                self.status = format!("불러오기 실패: {e}");
                self.redraw();
                return false;
            }
        }
        self.rebuild_cells();
        if !self.events.iter().any(|e| {
            Some(&e.id) == self.selected_id.as_ref()
                && (self.prefs.show_google || !google::is_imported_event(e))
        }) {
            self.selected_id = None;
        }
        self.arm_reminders();
        self.redraw_buttons();
        true
    }
    unsafe fn rebuild_cells(&mut self) {
        let start = shift(self.month, -(weekday(self.month) as i32));
        let end = shift(start, 41);
        let key = (self.month, self.prefs.lunar, self.prefs.holidays);
        if self.grid_key != Some(key) {
            let mut holidays = BTreeMap::new();
            if self.prefs.holidays {
                for year in start.year..=end.year {
                    holidays.extend(calendar_info::holidays(year));
                }
            }
            self.cells = (0..42)
                .map(|i| {
                    let date = shift(start, i);
                    let date_key = date.to_string();
                    Cell {
                        date,
                        lunar: if self.prefs.lunar {
                            calendar_info::lunar_label(date)
                        } else {
                            String::new()
                        },
                        holiday: holidays.remove(&date_key).unwrap_or_default(),
                        date_key,
                        events: Vec::new(),
                    }
                })
                .collect();
            self.grid_key = Some(key);
        }
        populate_cell_events(
            &mut self.cells,
            &self.events,
            start,
            end,
            self.prefs.show_google,
        );
        self.redraw();
    }
    unsafe fn arm_reminders(&mut self) {
        KillTimer(self.hwnd, 1);
        KillTimer(self.hwnd, 2);
        let mut st: SYSTEMTIME = zeroed();
        GetLocalTime(&mut st);
        let s = st.wHour as u32 * 3600 + st.wMinute as u32 * 60 + st.wSecond as u32;
        SetTimer(self.hwnd, 1, (86400 - s) * 1000 + 250, None);
        let today = Date {
            year: st.wYear as i32,
            month: st.wMonth as u32,
            day: st.wDay as u32,
        };
        let now = wall_seconds(today, st.wHour as u32, st.wMinute as u32) + st.wSecond as i64;
        self.notified.retain(|_, due| *due >= now - 86400);
        self.pending = next_reminder(&self.events, &self.notified, today, now);
        if let Some((_, _, due)) = &self.pending {
            SetTimer(
                self.hwnd,
                2,
                ((due - now).max(1) * 1000).min(0x7fffffff) as u32,
                None,
            );
        }
    }
    unsafe fn paint(&self, dc: HDC) {
        let r = self.client();
        let (bg, ink, muted, border) = self.colors();
        fill(dc, r, bg);
        let header = rect(0, 0, r.right, self.px(60));
        if RectVisible(dc, &header) != 0 {
            fill(
                dc,
                rect(0, 0, r.right, self.px(38)),
                if self.prefs.dark {
                    0x00363029
                } else {
                    0x00eae3da
                },
            );
            fill(
                dc,
                rect(0, self.px(38), r.right, self.px(22)),
                if self.prefs.dark {
                    0x003e362d
                } else {
                    0x00f5f0ea
                },
            );
            text(
                dc,
                &format!("{}년 {}월", self.month.year, self.month.month),
                rect(self.px(22), self.px(8), self.px(265), self.px(28)),
                self.fonts.strong,
                ink,
                DT_LEFT,
            );
            text(
                dc,
                if self.prefs.locked {
                    "위치 고정됨"
                } else {
                    "헤더를 끌어서 이동"
                },
                rect(self.px(24), self.px(40), self.px(220), self.px(19)),
                self.fonts.small,
                muted,
                DT_LEFT,
            );
        }
        let grid = self.grid();
        let today = Date::today();
        for (i, name) in [
            "일요일",
            "월요일",
            "화요일",
            "수요일",
            "목요일",
            "금요일",
            "토요일",
        ]
        .iter()
        .enumerate()
        {
            let cr = Self::cell_rect(grid, i);
            let weekday_rect = rect(
                cr.left + self.px(1),
                self.px(62),
                cr.right - cr.left - self.px(2),
                self.px(20),
            );
            if RectVisible(dc, &weekday_rect) == 0 {
                continue;
            }
            text(
                dc,
                name,
                weekday_rect,
                self.fonts.small,
                if self.prefs.dark && i == 0 {
                    0x00bcd9f2
                } else if self.prefs.dark && i == 6 {
                    0x00e6d9c8
                } else if self.prefs.dark {
                    ink
                } else if i == 0 {
                    0x00616bcb
                } else if i == 6 {
                    0x00b27a4b
                } else {
                    muted
                },
                DT_CENTER,
            );
        }
        for (i, cell) in self.cells.iter().enumerate() {
            let mut cr = Self::cell_rect(grid, i);
            if RectVisible(dc, &cr) == 0 {
                continue;
            }
            let capacity = self.cell_capacity(cr);
            let more = cell.events.len() > capacity;
            cr.right -= 1;
            cr.bottom -= 1;
            let selected = cell.date == self.selected;
            let tint = self
                .prefs
                .day_colors
                .get(&cell.date_key)
                .and_then(|v| color(v));
            let cell_bg = tint.unwrap_or(
                match (self.prefs.dark, selected, i % 7 == 0 || i % 7 == 6) {
                    (true, true, _) => 0x00604a36,
                    (true, false, true) => 0x00454240,
                    (true, false, false) => 0x003e372d,
                    (false, true, _) => 0x00f9e8d5,
                    (false, false, _) => 0x00fbf8f4,
                },
            );
            fill(dc, cr, cell_bg);
            line(dc, cr, if selected { 0x00d88448 } else { border });
            let cell_ink = if tint.is_some() { 0x00443527 } else { ink };
            let holiday_ink = if self.prefs.dark && tint.is_none() {
                0x00bcd9f2
            } else {
                0x00616bcb
            };
            let pad = self.px(10);
            let date_r = rect(cr.left + pad, cr.top + self.px(5), self.px(26), self.px(25));
            if cell.date == today {
                round(dc, date_r, 0x00d47a39, self.px(9));
            }
            text(
                dc,
                &cell.date.day.to_string(),
                date_r,
                self.fonts.strong,
                if cell.date == today {
                    0x00ffffff
                } else if cell.date.month != self.month.month {
                    muted
                } else if !cell.holiday.is_empty() || i % 7 == 0 {
                    holiday_ink
                } else if i % 7 == 6 {
                    if self.prefs.dark && tint.is_none() {
                        0x00e6d9c8
                    } else {
                        0x00b27a4b
                    }
                } else {
                    cell_ink
                },
                DT_CENTER,
            );
            let subtitle = if !cell.holiday.is_empty() {
                &cell.holiday
            } else {
                &cell.lunar
            };
            text(
                dc,
                subtitle,
                rect(
                    cr.left + self.px(41),
                    cr.top + self.px(7),
                    cr.right - cr.left - self.px(if more { 84 } else { 50 }),
                    self.px(22),
                ),
                self.fonts.small,
                if !cell.holiday.is_empty() {
                    holiday_ink
                } else {
                    muted
                },
                DT_RIGHT | DT_END_ELLIPSIS,
            );
            for (n, &index) in cell.events.iter().take(capacity).enumerate() {
                let e = &self.events[index];
                let er = event_row_rect(cr, n, self.dpi);
                let explicit = e.color.as_deref().and_then(color);
                let selected_event = selected && self.selected_id.as_deref() == Some(e.id.as_str());
                let block = explicit.unwrap_or(match (self.prefs.dark, selected_event) {
                    (true, true) => 0x00715940,
                    (true, false) => 0x00554b3d,
                    (false, true) => 0x00e9d2bb,
                    (false, false) => 0x00f1e5d9,
                });
                round(dc, er, block, self.px(5));
                if selected_event {
                    fill(
                        dc,
                        rect(
                            er.left,
                            er.top + self.px(3),
                            self.px(3),
                            er.bottom - er.top - self.px(6),
                        ),
                        0x00d47a39,
                    );
                }
                let t;
                let display = if e.time.is_none() && e.recurrence.is_none() {
                    e.title.as_str()
                } else {
                    t = format!(
                        "{}{}{}{}",
                        e.time.as_deref().unwrap_or_default(),
                        if e.time.is_some() { " " } else { "" },
                        if e.recurrence.is_some() { "↻ " } else { "" },
                        e.title
                    );
                    &t
                };
                text(
                    dc,
                    display,
                    rect(
                        er.left + self.px(8),
                        er.top,
                        er.right - er.left - self.px(12),
                        er.bottom - er.top,
                    ),
                    self.fonts.body,
                    if explicit.is_some() { 0x00443527 } else { ink },
                    DT_LEFT | DT_END_ELLIPSIS,
                );
            }
            if more {
                text(
                    dc,
                    &format!("+{}", cell.events.len() - capacity),
                    rect(
                        cr.right - self.px(36),
                        cr.top + self.px(7),
                        self.px(30),
                        self.px(22),
                    ),
                    self.fonts.small,
                    muted,
                    DT_RIGHT,
                );
            }
        }
        let footer = if self.status.starts_with("메모 저장 실패: ") {
            &self.status
        } else if self.inline.is_some() {
            "Ctrl+Enter / 바깥 클릭: 저장 · 삭제: 반복 전체·Google 동기화에 적용 · Esc: 취소"
        } else if !self.status.is_empty() {
            &self.status
        } else {
            "클릭: 선택 · 전체 메모 / Space: 내용 보기 · 더블클릭 / F2: 수정 · Delete: 삭제"
        };
        let footer_rect = rect(
            grid.left,
            r.bottom - self.px(28),
            grid.right - grid.left,
            self.px(22),
        );
        if RectVisible(dc, &footer_rect) != 0 {
            text(
                dc,
                footer,
                footer_rect,
                self.fonts.small,
                muted,
                DT_LEFT | DT_END_ELLIPSIS,
            );
        }
        if let Some(editor) = &self.inline {
            if let Some(i) = self.cells.iter().position(|cell| cell.date == editor.date) {
                let panel = self.inline_panel_rect(i);
                if RectVisible(dc, &panel) != 0 {
                    fill(dc, panel, 0x00ffffff);
                    line(dc, panel, 0x00c9bdb2);
                    let heading = format!(
                        "{}년 {}월 {}일",
                        editor.date.year, editor.date.month, editor.date.day
                    );
                    text(
                        dc,
                        &heading,
                        rect(
                            panel.left + self.px(10),
                            panel.top + self.px(5),
                            panel.right - panel.left - self.px(20),
                            self.px(26),
                        ),
                        self.fonts.body,
                        0x00332a22,
                        DT_LEFT,
                    );
                }
            }
        }
    }
    unsafe fn persist(&mut self) {
        if IsIconic(self.hwnd) == 0 {
            let mut r = zeroed();
            GetWindowRect(self.hwnd, &mut r);
            self.prefs.position = Some([r.left, r.top, r.right - r.left, r.bottom - r.top]);
        }
        if let Err(e) = self.prefs.save() {
            self.status = format!("설정 저장 실패: {e}");
        }
    }
}

unsafe fn control(
    parent: HWND,
    class: &str,
    title: &str,
    style: u32,
    id: i32,
    font: HFONT,
) -> HWND {
    let h = CreateWindowExW(
        0,
        w(class).as_ptr(),
        w(title).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        0,
        0,
        10,
        10,
        parent,
        id as usize as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(h, WM_SETFONT, font as usize, 0);
    h
}
unsafe fn tray(hwnd: HWND, action: u32, message: Option<&str>) {
    let mut n: NOTIFYICONDATAW = zeroed();
    n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    n.hWnd = hwnd;
    n.uID = 1;
    n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    n.uCallbackMessage = TRAY;
    n.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
    let title = w("Dot Calendar");
    n.szTip[..title.len()].copy_from_slice(&title);
    if let Some(message) = message {
        n.uFlags |= NIF_INFO;
        n.dwInfoFlags = NIIF_INFO;
        let m = w(message);
        let len = m.len().min(255);
        n.szInfo[..len].copy_from_slice(&m[..len]);
        n.szInfoTitle[..title.len()].copy_from_slice(&title);
    }
    Shell_NotifyIconW(action, &n);
}
fn wall_seconds(d: Date, h: u32, m: u32) -> i64 {
    use chrono::NaiveDate;
    NaiveDate::from_ymd_opt(d.year, d.month, d.day)
        .and_then(|d| d.and_hms_opt(h, m, 0))
        .map(|d| d.and_utc().timestamp())
        .unwrap_or(0)
}
unsafe fn message(parent: HWND, title: &str, body: &str, flags: u32) -> i32 {
    MessageBoxW(parent, w(body).as_ptr(), w(title).as_ptr(), flags)
}
pub fn show_error(s: &str) {
    unsafe {
        message(null_mut(), "Dot Calendar", s, MB_OK | MB_ICONERROR);
    }
}

unsafe fn popup(hwnd: HWND, cell: Option<usize>) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    let menu = CreatePopupMenu();
    {
        let a = (*ptr).borrow();
        if cell.is_some() {
            AppendMenuW(menu, MF_STRING, ADD as usize, w("+ 메모").as_ptr());
            AppendMenuW(
                menu,
                MF_STRING,
                AGENDA as usize,
                w("이 날짜의 전체 일정").as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            if a.selected_id.is_some() {
                for (id, t) in [
                    (VIEW, "전체 메모 보기"),
                    (EDIT, "메모 수정"),
                    (DETAILS, "세부 설정"),
                    (COMPLETE, "완료 / 미완료"),
                    (DELETE, "선택 메모 삭제"),
                ] {
                    AppendMenuW(menu, MF_STRING, id as usize, w(t).as_ptr());
                }
            }
            let colors = CreatePopupMenu();
            for (n, (name, _)) in PALETTE.iter().enumerate() {
                AppendMenuW(colors, MF_STRING, 2000 + n, w(name).as_ptr());
            }
            AppendMenuW(menu, MF_POPUP, colors as usize, w("날짜 배경색").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
        }
        for (id, t) in [
            (SETTINGS, "설정"),
            (CONNECT, "Google 계정 연결"),
            (DISCONNECT, "이 PC의 Google 연결 해제"),
            (SYNC, "지금 동기화"),
            (BACKUP, "백업 내보내기"),
            (RESTORE, "백업 가져오기"),
            (PRINT, "이 달 인쇄"),
            (
                LOCK,
                if a.prefs.locked {
                    "위치 잠금 해제"
                } else {
                    "위치 잠금"
                },
            ),
            (
                DETACH,
                if desktop::is_attached(hwnd) {
                    "일반 창으로 보기"
                } else {
                    "바탕화면에 고정"
                },
            ),
            (SHOW, "달력 표시"),
            (HIDE, "달력 숨기기"),
            (ABOUT, "도움말 / 공휴일 안내"),
            (QUIT, "종료"),
        ] {
            AppendMenuW(menu, MF_STRING, id as usize, w(t).as_ptr());
        }
    }
    let mut p = zeroed();
    GetCursorPos(&mut p);
    SetForegroundWindow(hwnd);
    let id = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        p.x,
        p.y,
        0,
        hwnd,
        null(),
    );
    DestroyMenu(menu);
    PostMessageW(hwnd, WM_NULL, 0, 0);
    if (2000..2000 + PALETTE.len() as i32).contains(&id) {
        let mut a = (*ptr).borrow_mut();
        let key = a.selected.to_string();
        let v = PALETTE[(id - 2000) as usize].1;
        if v.is_empty() {
            a.prefs.day_colors.remove(&key);
        } else {
            a.prefs.day_colors.insert(key, v.into());
        }
        a.persist();
        a.redraw();
    } else if id > 0 {
        command(hwnd, id);
    }
}

unsafe extern "system" fn inline_edit_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    let keys = &mut *(data as *mut InlineKeys);
    match msg {
        WM_IME_STARTCOMPOSITION => keys.composing = true,
        WM_IME_ENDCOMPOSITION => keys.composing = false,
        WM_KILLFOCUS if !keys.buttons.contains(&(wp as HWND)) => {
            PostMessageW(keys.parent, INLINE_COMMIT, hwnd as usize, 0);
        }
        WM_KEYDOWN if !keys.composing && wp as u16 == VK_ESCAPE => {
            PostMessageW(keys.parent, INLINE_CANCEL, hwnd as usize, 0);
            return 0;
        }
        WM_KEYDOWN
            if !keys.composing && wp as u16 == VK_RETURN && GetKeyState(VK_CONTROL as i32) < 0 =>
        {
            PostMessageW(keys.parent, INLINE_COMMIT, hwnd as usize, 0);
            return 0;
        }
        WM_CHAR
            if !keys.composing
                && (wp == 10 || wp == VK_RETURN as usize)
                && GetKeyState(VK_CONTROL as i32) < 0 =>
        {
            return 0;
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(inline_edit_proc), id);
            let result = DefSubclassProc(hwnd, msg, wp, lp);
            drop(Box::from_raw(data as *mut InlineKeys));
            return result;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

unsafe fn finish_inline(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    let editor = {
        let mut a = (*ptr).borrow_mut();
        if a.status.starts_with("메모 저장 실패: ") {
            a.status.clear();
            a.redraw();
        }
        a.inline.take()
    };
    if let Some(editor) = editor {
        let focused = GetFocus() == editor.hwnd || editor.buttons.contains(&GetFocus());
        DestroyWindow(editor.hwnd);
        for button in editor.buttons {
            DestroyWindow(button);
        }
        if focused {
            SetFocus(hwnd);
        }
    }
    (*ptr).borrow().redraw();
}

pub(crate) fn save_inline_note(
    store: &Store,
    date: Date,
    original: Option<&Event>,
    value: &str,
) -> Result<(), String> {
    let raw = normalize_newlines(value);
    if let Some((title, notes)) = note_parts(&raw) {
        if let Some(original) = original {
            let mut replacement = original.clone();
            replacement.title = title.into();
            if normalize_newlines(&original.notes) != notes {
                replacement.notes = notes.into();
            }
            if replacement == *original {
                Ok(())
            } else {
                match store.replace_if_unchanged(original, &replacement)? {
                    true => Ok(()),
                    false => Err("다른 곳에서 일정이 변경되었습니다. 내용을 복사하거나 Esc로 취소한 뒤 다시 편집해 주세요.".into()),
                }
            }
        } else {
            store
                .create_event_with_details(
                    &date.to_string(),
                    None,
                    title,
                    notes,
                    None,
                    false,
                    None,
                    None,
                    None,
                )
                .map(|_| ())
        }
    } else if let Some(original) = original {
        if store.delete_if_unchanged(original)? {
            Ok(())
        } else {
            Err(
                "다른 곳에서 일정이 변경되었습니다. Esc로 취소한 뒤 최신 내용을 확인해 주세요."
                    .into(),
            )
        }
    } else {
        Ok(())
    }
}

/// Returns false and retains the editable text if validation or a concurrent write fails.
unsafe fn save_inline_value(hwnd: HWND, override_value: Option<&str>) -> bool {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    if (*ptr).borrow().inline.is_none() {
        return true;
    }
    let result = {
        let a = (*ptr).borrow();
        let editor = a.inline.as_ref().unwrap();
        let label_value;
        let value = if let Some(value) = override_value {
            value
        } else {
            label_value = label(editor.hwnd);
            &label_value
        };
        save_inline_note(&a.store, editor.date, editor.original.as_ref(), value)
    };
    match result {
        Ok(()) => {
            finish_inline(hwnd);
            let mut a = (*ptr).borrow_mut();
            a.refresh();
            true
        }
        Err(error) => {
            let mut a = (*ptr).borrow_mut();
            a.status = format!("메모 저장 실패: {error}");
            a.redraw();
            let edit = a.inline.as_ref().map(|editor| editor.hwnd);
            drop(a);
            if let Some(edit) = edit {
                SetFocus(edit);
            }
            false
        }
    }
}
unsafe fn commit_inline(hwnd: HWND) -> bool {
    save_inline_value(hwnd, None)
}

unsafe fn begin_inline(hwnd: HWND, existing: bool) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    if !commit_inline(hwnd) {
        return;
    }
    let (date, original, panel, font) = {
        let a = (*ptr).borrow();
        let date = a.selected;
        let original = if existing {
            a.events
                .iter()
                .find(|e| Some(&e.id) == a.selected_id.as_ref())
                .cloned()
        } else {
            None
        };
        if existing && original.is_none() {
            return;
        }
        let Some(i) = a.cells.iter().position(|cell| cell.date == date) else {
            return;
        };
        (date, original, a.inline_panel_rect(i), a.fonts.body)
    };
    let initial = original
        .as_ref()
        .map(|event| {
            if event.notes.is_empty() {
                event.title.clone()
            } else {
                format!(
                    "{}\r\n{}",
                    event.title,
                    normalize_newlines(&event.notes).replace('\n', "\r\n")
                )
            }
        })
        .unwrap_or_default();
    let area = (*ptr).borrow().inline_text_rect(panel);
    let edit = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w("EDIT").as_ptr(),
        w(&initial).as_ptr(),
        WS_CHILD
            | WS_VISIBLE
            | WS_TABSTOP
            | WS_VSCROLL
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | ES_WANTRETURN as u32
            | ES_NOHIDESEL as u32,
        area.left,
        area.top,
        area.right - area.left,
        area.bottom - area.top,
        hwnd,
        INLINE_EDIT_ID as usize as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    if edit.is_null() {
        (*ptr).borrow_mut().status = "메모 입력창을 열지 못했습니다.".into();
        (*ptr).borrow().redraw();
        return;
    }
    SendMessageW(edit, WM_SETFONT, font as usize, 1);
    SendMessageW(edit, EM_LIMITTEXT, 9000, 0);
    let mut buttons: [HWND; 3] = [null_mut(); 3];
    for (index, (id, caption)) in [
        (INLINE_SAVE_ID, "저장"),
        (INLINE_DELETE_ID, "삭제"),
        (INLINE_CANCEL_ID, "취소"),
    ]
    .iter()
    .enumerate()
    {
        let r = (*ptr).borrow().inline_button_rect(panel, index);
        let button = control(
            hwnd,
            "BUTTON",
            caption,
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            *id,
            font,
        );
        if button.is_null() {
            for button in buttons {
                if !button.is_null() {
                    DestroyWindow(button);
                }
            }
            DestroyWindow(edit);
            (*ptr).borrow_mut().status = "메모 버튼을 열지 못했습니다.".into();
            (*ptr).borrow().redraw();
            return;
        }
        MoveWindow(button, r.left, r.top, r.right - r.left, r.bottom - r.top, 1);
        buttons[index] = button;
    }
    let keys = Box::into_raw(Box::new(InlineKeys {
        parent: hwnd,
        composing: false,
        buttons,
    }));
    if SetWindowSubclass(edit, Some(inline_edit_proc), 1, keys as usize) == 0 {
        drop(Box::from_raw(keys));
        for button in buttons {
            DestroyWindow(button);
        }
        DestroyWindow(edit);
        (*ptr).borrow_mut().status = "메모 키 입력을 설정하지 못했습니다.".into();
        (*ptr).borrow().redraw();
        return;
    }
    (*ptr).borrow_mut().inline = Some(InlineEditor {
        hwnd: edit,
        buttons,
        date,
        original,
    });
    (*ptr).borrow().redraw();
    SetFocus(edit);
    SendMessageW(edit, EM_SETSEL, usize::MAX, -1);
}

unsafe fn apply_preferences(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    let (desktop, opacity) = {
        let a = (*ptr).borrow();
        (a.prefs.desktop, a.prefs.opacity)
    };
    // An empty inherited value must not turn a desktop widget into a normal window.
    let windowed = std::env::var_os("DOT_CALENDAR_WINDOWED").is_some_and(|value| value == "1");
    if desktop && !windowed {
        if !desktop::is_attached(hwnd) {
            if let Err(e) = desktop::attach(hwnd) {
                (*ptr).borrow_mut().status = format!("바탕화면 연결 실패 · 일반 창으로 표시: {e}");
            }
        }
    } else if desktop::is_attached(hwnd) {
        desktop::detach(hwnd);
    }
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    // Keep layered composition even at alpha 255: Explorer-hosted children
    // regressed to a black rectangle when the v0.3.3 optimization removed it.
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
    SetLayeredWindowAttributes(hwnd, 0, opacity, LWA_ALPHA);
    (*ptr).borrow().arm_sync();
    {
        let mut a = (*ptr).borrow_mut();
        if a.grid_key != Some((a.month, a.prefs.lunar, a.prefs.holidays)) {
            a.rebuild_cells();
        } else {
            a.redraw();
        }
    }
    (*ptr).borrow().redraw_buttons();
}

unsafe fn view_selected(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    let selection = {
        let app = (*ptr).borrow();
        app.events
            .iter()
            .find(|event| Some(&event.id) == app.selected_id.as_ref())
            .map(|event| (app.selected, memo_content(event)))
    };
    if let Some((date, content)) = selection {
        form(
            hwnd,
            "전체 메모",
            vec![field(
                &format!(
                    "{}년 {}월 {}일 · 내용을 선택해 복사할 수 있습니다",
                    date.year, date.month, date.day
                ),
                content,
                Kind::ReadOnly,
                rect(24, 55, 450, 290),
            )],
            420,
            None,
            Box::new(|_| Ok(())),
        );
    }
}

unsafe fn view_day(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    let (date, rows, selected) = {
        let mut app = (*ptr).borrow_mut();
        app.refresh();
        let Some(cell) = app.cells.iter().find(|cell| cell.date == app.selected) else {
            return;
        };
        let rows = agenda_rows(cell, &app.events);
        let selected = rows
            .iter()
            .position(|(id, _)| Some(id) == app.selected_id.as_ref())
            .unwrap_or(0);
        (cell.date, Rc::new(RefCell::new(rows)), selected)
    };
    let heading = agenda_heading(date, rows.borrow().len());
    let choices = Rc::clone(&rows);
    let edit = form(
        hwnd,
        "하루 일정 · 전체 보기",
        vec![
            field(
                &heading,
                selected.to_string(),
                Kind::Agenda { date, rows },
                rect(24, 55, 450, 165),
            ),
            field(
                "선택한 일정의 전체 메모 · 선택해 복사할 수 있습니다",
                "",
                Kind::ReadOnly,
                rect(24, 256, 450, 190),
            ),
        ],
        520,
        None,
        Box::new(move |values| {
            if IsWindow(hwnd) == 0 {
                return Err("달력 창이 닫혔습니다.".into());
            }
            let selected = values[0].parse::<usize>().ok();
            let id = selected
                .and_then(|i| choices.borrow().get(i).map(|(id, _)| id.clone()))
                .ok_or("수정할 일정을 선택해 주세요.")?;
            let mut app = (*ptr).borrow_mut();
            app.refresh();
            if day_event(&app.cells, &app.events, date, &id).is_none() {
                return Err(
                    "일정이 변경되거나 삭제되었습니다. 목록을 닫고 다시 열어 주세요.".into(),
                );
            }
            let previous = app.selected;
            app.selected = date;
            app.selected_id = Some(id);
            app.redraw_selection(previous);
            Ok(())
        }),
    );
    if edit && IsWindow(hwnd) != 0 {
        command(hwnd, EDIT);
    }
}

unsafe fn command(hwnd: HWND, id: i32) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    if id != ADD
        && id != EDIT
        && id != DETAILS
        && id != INLINE_SAVE_ID
        && id != INLINE_CANCEL_ID
        && id != INLINE_DELETE_ID
        && !commit_inline(hwnd)
    {
        return;
    }
    match id {
        VIEW => view_selected(hwnd),
        AGENDA => view_day(hwnd),
        INLINE_SAVE_ID => {
            commit_inline(hwnd);
        }
        INLINE_DELETE_ID => {
            save_inline_value(hwnd, Some(""));
        }
        INLINE_CANCEL_ID => finish_inline(hwnd),
        PREV | NEXT | TODAY => {
            if !commit_inline(hwnd) {
                return;
            }
            let mut a = (*ptr).borrow_mut();
            a.month = if id == TODAY {
                Date {
                    day: 1,
                    ..Date::today()
                }
            } else {
                month_shift(a.month, if id == NEXT { 1 } else { -1 })
            };
            a.selected = if id == TODAY { Date::today() } else { a.month };
            a.selected_id = None;
            a.rebuild_cells();
        }
        ADD | EDIT => begin_inline(hwnd, id == EDIT),
        SHOW_GOOGLE => {
            let mut a = (*ptr).borrow_mut();
            let old = a.prefs.show_google;
            a.prefs.show_google = !old;
            if let Err(error) = a.prefs.save() {
                a.prefs.show_google = old;
                a.status = format!("설정 저장 실패: {error}");
            } else {
                if !a.prefs.show_google
                    && a.events.iter().any(|e| {
                        Some(&e.id) == a.selected_id.as_ref() && google::is_imported_event(e)
                    })
                {
                    a.selected_id = None;
                }
                a.status = if a.prefs.show_google {
                    "Google 일정 표시 · 최신 일정은 동기화 버튼으로 가져오세요."
                } else {
                    "Google 일정 숨김 · 일정은 보존되며 동기화는 계속됩니다."
                }
                .into();
                a.rebuild_cells();
            }
            a.redraw_buttons();
            a.redraw();
        }
        DETAILS => {
            if !commit_inline(hwnd) {
                return;
            }
            let (date, event) = {
                let a = (*ptr).borrow();
                (
                    a.selected,
                    a.events
                        .iter()
                        .find(|e| Some(&e.id) == a.selected_id.as_ref())
                        .cloned(),
                )
            };
            if event.is_some() {
                edit_event(hwnd, date, event);
                (*ptr).borrow_mut().refresh();
            }
        }
        COMPLETE => {
            let change = {
                let a = (*ptr).borrow();
                a.events
                    .iter()
                    .find(|e| Some(&e.id) == a.selected_id.as_ref())
                    .map(|e| (e.id.clone(), !e.completed))
            };
            if let Some((id, completed)) = change {
                let result =
                    (*ptr)
                        .borrow()
                        .store
                        .patch_details(&id, Some(completed), None, None, None);
                if let Err(e) = result {
                    show_error(&e);
                }
                (*ptr).borrow_mut().refresh();
            }
        }
        DELETE => {
            let e = {
                let a = (*ptr).borrow();
                a.events
                    .iter()
                    .find(|e| Some(&e.id) == a.selected_id.as_ref())
                    .cloned()
            };
            if let Some(e) = e {
                let suffix = if e.recurrence.is_some() {
                    "\n반복 일정 전체가 삭제됩니다."
                } else {
                    ""
                };
                if message(
                    hwnd,
                    "일정 삭제",
                    &format!("‘{}’ 메모를 삭제할까요?{}\nGoogle과 동기화한 일정이면 Google에도 삭제가 반영됩니다.", e.title, suffix),
                    MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
                ) == IDYES
                {
                    let result = (*ptr).borrow().store.delete_event_expected(&e.id, Some(&e));
                    if let Err(e) = result {
                        show_error(&e);
                    }
                    (*ptr).borrow_mut().refresh();
                }
            }
        }
        SETTINGS => {
            if (*ptr).borrow().busy {
                message(
                    hwnd,
                    "설정",
                    "Google 연결 또는 동기화가 끝난 후 설정을 변경해 주세요.",
                    MB_OK,
                );
                return;
            }
            settings(hwnd);
            apply_preferences(hwnd);
        }
        ABOUT => {
            message(hwnd,concat!("Dot Calendar ", env!("CARGO_PKG_VERSION")),&format!("날짜 칸에서 더블클릭, F2 또는 Enter로 메모를 작성합니다.\nCtrl+Enter 또는 바깥 클릭으로 저장하고 Esc로 취소합니다.\n우클릭의 세부 설정에서 날짜·시간·반복·색상·알림을 수정합니다.\n완료 상태는 닷 또는 우클릭 메뉴에서 관리합니다.\n설정 저장 후 메뉴에서 Google 계정에 연결하세요.\n\n{}\n\n알림은 달력이 실행 중일 때 표시됩니다.",calendar_info::HOLIDAY_DATA_NOTE),MB_OK|MB_ICONINFORMATION);
        }
        MENU => popup(hwnd, None),
        CONNECT | SYNC => start_sync(hwnd, id == CONNECT),
        DISCONNECT => {
            if (*ptr).borrow().busy {
                message(
                    hwnd,
                    "Google 연결",
                    "현재 연결 또는 동기화 작업이 끝난 후 해제해 주세요.",
                    MB_OK,
                );
            } else {
                let result = google::forget_connection();
                let mut a = (*ptr).borrow_mut();
                a.status = result.unwrap_or_else(|e| e);
                a.arm_sync();
                a.redraw();
            }
        }
        LOCK => {
            let mut a = (*ptr).borrow_mut();
            a.prefs.locked = !a.prefs.locked;
            a.persist();
            a.redraw();
        }
        DETACH => {
            let attached = desktop::is_attached(hwnd);
            (*ptr).borrow_mut().prefs.desktop = !attached;
            apply_preferences(hwnd);
            (*ptr).borrow_mut().persist();
        }
        SHOW => {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            if !desktop::is_attached(hwnd) {
                SetForegroundWindow(hwnd);
            }
        }
        HIDE => {
            if !commit_inline(hwnd) {
                return;
            }
            (*ptr).borrow_mut().persist();
            ShowWindow(hwnd, SW_HIDE);
        }
        QUIT => {
            (*ptr).borrow_mut().quitting = true;
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        BACKUP | RESTORE => backup(hwnd, id == RESTORE),
        PRINT => print_month(hwnd),
        _ => {}
    }
}

unsafe fn start_sync(hwnd: HWND, connect: bool) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    if (*ptr).borrow().busy {
        return;
    }
    let config = match google::load_config() {
        Ok(c) if !c.client_id.trim().is_empty() => c,
        _ => {
            message(
                hwnd,
                "Google 연결",
                "먼저 설정에서 Google OAuth 클라이언트 ID를 입력해 주세요.",
                MB_OK | MB_ICONINFORMATION,
            );
            return;
        }
    };
    if !connect && !google::is_connected() {
        message(
            hwnd,
            "Google 연결",
            "메뉴의 ‘Google 계정 연결’에서 로그인한 뒤 동기화할 수 있습니다.",
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    }
    let output = {
        let mut a = (*ptr).borrow_mut();
        a.busy = true;
        a.status = if connect {
            "브라우저에서 Google 로그인을 완료해 주세요…"
        } else {
            "Google Calendar 동기화 중…"
        }
        .into();
        a.redraw();
        a.sync_result.clone()
    };
    // The controller survives a desktop child being destroyed by Explorer.
    let handle = (*ptr).borrow().controller as usize;
    std::thread::spawn(move || {
        let result = if connect {
            google::connect(&config).and_then(|_| {
                Store::open().and_then(|s| google::sync(&s, &config)).map_err(|error| {
                    format!("계정 연결 완료 · 첫 동기화 실패: {error}. 동기화 버튼으로 다시 시도해 주세요.")
                })
            })
        } else {
            Store::open().and_then(|s| google::sync(&s, &config))
        };
        if let Ok(mut slot) = output.lock() {
            *slot = Some(result);
        }
        unsafe {
            PostMessageW(handle as HWND, SYNC_DONE, 0, 0);
        }
    });
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Keep resize hit testing, but let the calendar paint the entire window.
    // Explorer otherwise draws WS_THICKFRAME as a pale border around the child.
    if msg == WM_NCCALCSIZE || msg == WM_NCPAINT {
        return 0;
    }
    if msg == WM_NCACTIVATE {
        return DefWindowProcW(hwnd, msg, wp, -1);
    }
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_CLOSE {
        if !commit_inline(hwnd) {
            return 0;
        }
        let quitting = (*ptr).borrow().quitting;
        (*ptr).borrow_mut().persist();
        if quitting {
            DestroyWindow(hwnd);
        } else {
            ShowWindow(hwnd, SW_HIDE);
        }
        return 0;
    }
    if msg == WM_DESTROY {
        // Explorer destroys child controls too. Never read a stale EDIT handle as
        // an empty draft after rebuilding: that could delete its original note.
        if let Ok(mut app) = (*ptr).try_borrow_mut() {
            app.inline = None;
        }
        desktop::forget_destroyed(hwnd);
        let controller = CONTROLLER_WINDOW.load(Ordering::Relaxed) as HWND;
        let quitting = (*ptr).try_borrow().is_ok_and(|app| app.quitting);
        if quitting {
            if !controller.is_null() {
                DestroyWindow(controller);
            }
        } else if !controller.is_null() {
            PostMessageW(controller, REBUILD_WIDGET, 0, 0);
        }
        return 0;
    }
    if msg == INLINE_COMMIT || msg == INLINE_CANCEL {
        let is_current = (*ptr)
            .borrow()
            .inline
            .as_ref()
            .is_some_and(|edit| edit.hwnd as usize == wp);
        if is_current {
            if msg == INLINE_COMMIT {
                commit_inline(hwnd);
            } else {
                finish_inline(hwnd);
            }
        }
        return 0;
    }
    if msg == WM_ACTIVATE && (wp & 0xffff) == WA_INACTIVE as usize {
        if let Some(editor) = (*ptr).borrow().inline.as_ref() {
            PostMessageW(hwnd, INLINE_COMMIT, editor.hwnd as usize, 0);
        }
    }
    if msg == WM_COMMAND && (wp >> 16) == BN_CLICKED as usize {
        command(hwnd, (wp & 0xffff) as i32);
        return 0;
    }
    if msg == WM_CONTEXTMENU {
        if !commit_inline(hwnd) {
            return 0;
        }
        let mut p = POINT {
            x: (lp as u16 as i16) as i32,
            y: ((lp >> 16) as u16 as i16) as i32,
        };
        ScreenToClient(hwnd, &mut p);
        let hit = (*ptr).borrow().hit(p.x, p.y);
        if let Some((i, e)) = hit {
            let mut a = (*ptr).borrow_mut();
            let previous = a.selected;
            a.selected = a.cells[i].date;
            a.selected_id = e.and_then(|n| {
                a.cells[i]
                    .events
                    .get(n)
                    .map(|&index| a.events[index].id.clone())
            });
            a.redraw_selection(previous);
        }
        popup(hwnd, hit.map(|h| h.0));
        return 0;
    }
    if msg == WM_LBUTTONDOWN || msg == WM_LBUTTONDBLCLK {
        // The painted calendar is not a focusable child control. Commit before
        // changing the selection so a click outside the EDIT really saves it.
        if !commit_inline(hwnd) {
            return 0;
        }
        SetFocus(hwnd);
        let x = (lp as u16 as i16) as i32;
        let y = ((lp >> 16) as u16 as i16) as i32;
        let hit = (*ptr).borrow().hit(x, y);
        if let Some((i, e)) = hit {
            let (mut action, mut more) = (0, false);
            {
                let mut a = (*ptr).borrow_mut();
                let previous = a.selected;
                a.selected = a.cells[i].date;
                let cr = App::cell_rect(a.grid(), i);
                let capacity = a.cell_capacity(cr);
                if a.cells[i].events.len() > capacity
                    && x >= cr.right - a.px(40)
                    && y < cr.top + a.px(31)
                {
                    a.selected_id = None;
                    more = true;
                } else if let Some(n) = e {
                    a.selected_id = Some(a.events[a.cells[i].events[n]].id.clone());
                    if msg == WM_LBUTTONDBLCLK {
                        action = EDIT;
                    }
                } else {
                    a.selected_id = None;
                    if msg == WM_LBUTTONDBLCLK {
                        action = ADD;
                    }
                }
                a.redraw_selection(previous);
            }
            if more {
                command(hwnd, AGENDA);
            } else if action != 0 {
                command(hwnd, action);
            }
            return 0;
        }
        let drag = {
            let a = (*ptr).borrow();
            !a.prefs.locked && y < a.px(60)
        };
        if drag {
            ReleaseCapture();
            SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
        }
        return 0;
    }
    if msg == WM_KEYDOWN {
        let key = wp as u16;
        if key == VK_RETURN && GetKeyState(VK_CONTROL as i32) < 0 {
            command(hwnd, AGENDA);
            return 0;
        }
        if key == VK_SPACE {
            command(hwnd, VIEW);
            return 0;
        }
        if key == VK_RETURN || key == VK_F2 {
            let id = if (*ptr).borrow().selected_id.is_some() {
                EDIT
            } else {
                ADD
            };
            command(hwnd, id);
            return 0;
        }
        if key == VK_DELETE {
            command(hwnd, DELETE);
            return 0;
        }
        if key == VK_LEFT || key == VK_RIGHT || key == VK_UP || key == VK_DOWN {
            let mut a = (*ptr).borrow_mut();
            let previous = a.selected;
            let previous_month = a.month;
            a.selected = shift(
                a.selected,
                match key {
                    VK_LEFT => -1,
                    VK_RIGHT => 1,
                    VK_UP => -7,
                    _ => 7,
                },
            );
            a.month = Date {
                day: 1,
                ..a.selected
            };
            a.selected_id = None;
            if a.month != previous_month {
                a.rebuild_cells();
            } else {
                a.redraw_selection(previous);
            }
            return 0;
        }
    }
    if msg == WM_TIMER && wp == 3 {
        if !(*ptr).borrow().busy {
            start_sync(hwnd, false);
        }
        return 0;
    }
    if msg == WM_NCHITTEST {
        let a = (*ptr).borrow();
        if !a.prefs.locked {
            let mut r = zeroed();
            GetWindowRect(hwnd, &mut r);
            let x = (lp as u16 as i16) as i32;
            let y = ((lp >> 16) as u16 as i16) as i32;
            let b = a.px(7);
            if x >= r.right - b && y >= r.bottom - b {
                return HTBOTTOMRIGHT as isize;
            }
            if x <= r.left + b && y >= r.bottom - b {
                return HTBOTTOMLEFT as isize;
            }
            if y >= r.bottom - b {
                return HTBOTTOM as isize;
            }
            if x >= r.right - b {
                return HTRIGHT as isize;
            }
            if x <= r.left + b {
                return HTLEFT as isize;
            }
        }
        return HTCLIENT as isize;
    }
    if msg == WM_EXITSIZEMOVE {
        (*ptr).borrow_mut().persist();
        return 0;
    }
    if msg == WM_DPICHANGED {
        let dpi = (wp & 0xffff) as u32;
        {
            let mut a = (*ptr).borrow_mut();
            a.dpi = dpi;
            a.fonts = Fonts::new(dpi);
            for (_, h) in &a.buttons {
                SendMessageW(*h, WM_SETFONT, a.fonts.body as usize, 0);
            }
            if let Some(editor) = &a.inline {
                SendMessageW(editor.hwnd, WM_SETFONT, a.fonts.body as usize, 1);
            }
        }
        if !desktop::is_attached(hwnd) {
            let r = &*(lp as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        (*ptr).borrow().layout();
        return 0;
    }
    let Ok(mut a) = (*ptr).try_borrow_mut() else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let (bg, ink, _, _) = a.colors();
            let dc = wp as HDC;
            SetTextColor(dc, ink);
            SetBkColor(dc, bg);
            SetDCBrushColor(dc, bg);
            GetStockObject(DC_BRUSH) as LRESULT
        }
        WM_CREATE => {
            a.init(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let r = ps.rcPaint;
            let width = r.right - r.left;
            let height = r.bottom - r.top;
            if width > 0 && height > 0 {
                let mem = CreateCompatibleDC(dc);
                let bitmap = CreateCompatibleBitmap(dc, width, height);
                if !mem.is_null() && !bitmap.is_null() {
                    let old = SelectObject(mem, bitmap);
                    SetViewportOrgEx(mem, -r.left, -r.top, null_mut());
                    a.paint(mem);
                    BitBlt(
                        dc, r.left, r.top, width, height, mem, r.left, r.top, SRCCOPY,
                    );
                    SelectObject(mem, old);
                } else {
                    a.paint(dc);
                }
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                if !mem.is_null() {
                    DeleteDC(mem);
                }
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_SIZE => {
            a.layout();
            0
        }
        WM_GETMINMAXINFO => {
            let m = &mut *(lp as *mut MINMAXINFO);
            m.ptMinTrackSize.x = a.px(840);
            m.ptMinTrackSize.y = a.px(530);
            0
        }
        WM_DRAWITEM => {
            let item = &*(lp as *const DRAWITEMSTRUCT);
            let (bg, ink, muted, border) = a.colors();
            let pressed = item.itemState & ODS_SELECTED != 0;
            let focused = item.itemState & ODS_FOCUS != 0;
            let primary = item.CtlID == ADD as u32;
            let button_bg = if primary {
                if pressed {
                    0x00b8662d
                } else {
                    0x00d47a39
                }
            } else if a.prefs.dark {
                if pressed {
                    0x005d5149
                } else {
                    0x00453c35
                }
            } else if pressed {
                0x00d9d1c8
            } else {
                0x00f9f7f5
            };
            fill(item.hDC, item.rcItem, bg);
            round(
                item.hDC,
                item.rcItem,
                if focused { 0x00d47a39 } else { border },
                a.px(17),
            );
            let inset = a.px(if focused { 2 } else { 1 });
            let inner = RECT {
                left: item.rcItem.left + inset,
                top: item.rcItem.top + inset,
                right: item.rcItem.right - inset,
                bottom: item.rcItem.bottom - inset,
            };
            round(item.hDC, inner, button_bg, a.px(16));
            text(
                item.hDC,
                &label(item.hwndItem),
                item.rcItem,
                a.fonts.body,
                if item.itemState & ODS_DISABLED != 0 {
                    muted
                } else if primary {
                    0xffffff
                } else {
                    ink
                },
                DT_CENTER,
            );
            1
        }
        WM_TIMER => {
            if wp == 2 {
                KillTimer(hwnd, 2);
                if let Some((key, body, due)) = a.pending.take() {
                    a.notified.insert(key, due);
                    tray(a.controller, NIM_MODIFY, Some(&body));
                }
            }
            a.arm_reminders();
            if wp == 1 {
                a.redraw();
            }
            0
        }
        WM_TIMECHANGE | WM_POWERBROADCAST => {
            a.arm_reminders();
            a.redraw();
            1
        }
        RELOAD => {
            a.refresh();
            0
        }
        _ => {
            drop(a);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}

enum Kind {
    Text,
    Multiline,
    ReadOnly,
    Password,
    Choice(Vec<String>),
    Agenda { date: Date, rows: AgendaRows },
    Check,
    Slider,
}
type AgendaRows = Rc<RefCell<Vec<(String, String)>>>;
struct Field {
    title: String,
    value: String,
    kind: Kind,
    r: RECT,
    hwnd: HWND,
}
fn field(title: &str, value: impl Into<String>, kind: Kind, r: RECT) -> Field {
    Field {
        title: title.into(),
        value: value.into(),
        kind,
        r,
        hwnd: null_mut(),
    }
}
type SaveForm = Box<dyn Fn(&[String]) -> Result<(), String>>;

struct Form {
    owner: HWND,
    dpi: u32,
    fonts: Fonts,
    fields: Vec<Field>,
    height: i32,
    done: bool,
    committed: bool,
    opacity_preview: Option<(HWND, u8)>,
    save: SaveForm,
}
impl Form {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    unsafe fn init(&mut self, h: HWND) {
        self.dpi = GetDpiForWindow(h).max(96);
        self.fonts = Fonts::new(self.dpi);
        for (i, f) in self.fields.iter_mut().enumerate() {
            let (class, style) = match &f.kind {
                Kind::Choice(_) => ("COMBOBOX", CBS_DROPDOWNLIST as u32 | WS_VSCROLL),
                Kind::Agenda { .. } => (
                    "LISTBOX",
                    WS_BORDER | WS_VSCROLL | LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32,
                ),
                Kind::Check => ("BUTTON", BS_AUTOCHECKBOX as u32),
                Kind::Slider => ("msctls_trackbar32", TBS_NOTICKS | TBS_FIXEDLENGTH),
                Kind::Multiline => (
                    "EDIT",
                    WS_BORDER
                        | ES_MULTILINE as u32
                        | ES_AUTOVSCROLL as u32
                        | ES_WANTRETURN as u32
                        | WS_VSCROLL,
                ),
                Kind::ReadOnly => (
                    "EDIT",
                    WS_BORDER
                        | ES_MULTILINE as u32
                        | ES_AUTOVSCROLL as u32
                        | ES_READONLY as u32
                        | WS_VSCROLL,
                ),
                Kind::Password => (
                    "EDIT",
                    WS_BORDER | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
                ),
                _ => ("EDIT", WS_BORDER | ES_AUTOHSCROLL as u32),
            };
            f.hwnd = control(
                h,
                class,
                if matches!(f.kind, Kind::Check) {
                    &f.title
                } else {
                    &f.value
                },
                style | WS_TABSTOP,
                200 + i as i32,
                self.fonts.body,
            );
            match &f.kind {
                Kind::Slider => {
                    SendMessageW(f.hwnd, TBM_SETRANGEMIN, 0, 40);
                    SendMessageW(f.hwnd, TBM_SETRANGEMAX, 0, 100);
                    SendMessageW(f.hwnd, TBM_SETTHUMBLENGTH, (28 * self.dpi / 96) as usize, 0);
                    SendMessageW(f.hwnd, TBM_SETPAGESIZE, 0, 10);
                    SendMessageW(f.hwnd, TBM_SETPOS, 1, f.value.parse().unwrap_or(100));
                }
                Kind::Choice(values) => {
                    for v in values {
                        SendMessageW(f.hwnd, CB_ADDSTRING, 0, w(v).as_ptr() as isize);
                    }
                    SendMessageW(f.hwnd, CB_SETCURSEL, f.value.parse().unwrap_or(0), 0);
                }
                Kind::Agenda { rows, .. } => {
                    set_agenda_items(f.hwnd, &rows.borrow(), f.value.parse().unwrap_or(0));
                }
                Kind::Check => {
                    SendMessageW(
                        f.hwnd,
                        BM_SETCHECK,
                        if f.value == "1" {
                            BST_CHECKED
                        } else {
                            BST_UNCHECKED
                        } as usize,
                        0,
                    );
                }
                _ => {
                    SendMessageW(
                        f.hwnd,
                        EM_SETLIMITTEXT,
                        match f.kind {
                            Kind::Multiline => 4096,
                            Kind::ReadOnly => 8192,
                            _ => 512,
                        },
                        0,
                    );
                }
            }
            let scale = |n| n * self.dpi as i32 / 96;
            MoveWindow(
                f.hwnd,
                scale(f.r.left),
                scale(f.r.top),
                scale(f.r.right - f.r.left),
                scale(if matches!(f.kind, Kind::Choice(_)) {
                    210
                } else {
                    f.r.bottom - f.r.top
                }),
                1,
            );
            if matches!(f.kind, Kind::Agenda { .. }) {
                SendMessageW(f.hwnd, LB_SETTOPINDEX, f.value.parse().unwrap_or(0), 0);
            }
        }
        let buttons: &[(i32, &str, i32)] = if self
            .fields
            .iter()
            .any(|field| matches!(field.kind, Kind::Agenda { .. }))
        {
            &[(1, "메모 수정", 302), (2, "닫기", 394)]
        } else if self
            .fields
            .iter()
            .any(|field| matches!(field.kind, Kind::ReadOnly))
        {
            &[(2, "닫기", 394)]
        } else {
            &[(1, "저장", 394), (2, "취소", 302)]
        };
        for &(id, t, x) in buttons {
            let b = control(
                h,
                "BUTTON",
                t,
                WS_TABSTOP | if id == 1 { BS_DEFPUSHBUTTON as u32 } else { 0 },
                id,
                self.fonts.body,
            );
            MoveWindow(
                b,
                self.px(x),
                self.px(self.height - 52),
                self.px(80),
                self.px(32),
                1,
            );
        }
        self.update_agenda(h, false);
    }
    unsafe fn update_agenda(&mut self, h: HWND, reload: bool) {
        let Some(index) = self
            .fields
            .iter()
            .position(|f| matches!(f.kind, Kind::Agenda { .. }))
        else {
            return;
        };
        let list = &self.fields[index];
        let Kind::Agenda { date, rows } = &list.kind else {
            return;
        };
        let (date, rows, list_hwnd) = (*date, Rc::clone(rows), list.hwnd);
        // Keep the actual widget: an owned top-level form may belong to
        // Explorer's root window when the widget is a desktop child.
        let ptr = GetWindowLongPtrW(self.owner, GWLP_USERDATA) as *const RefCell<App>;
        if ptr.is_null() {
            return;
        }
        if reload {
            let mut app = (*ptr).borrow_mut();
            app.refresh();
            let fresh = app
                .cells
                .iter()
                .find(|cell| cell.date == date)
                .map(|cell| agenda_rows(cell, &app.events))
                .unwrap_or_default();
            let selected = SendMessageW(list_hwnd, LB_GETCURSEL, 0, 0) as usize;
            let mut current = rows.borrow_mut();
            if *current != fresh {
                let selected = current
                    .get(selected)
                    .and_then(|(id, _)| fresh.iter().position(|(fresh_id, _)| fresh_id == id))
                    .unwrap_or(0);
                *current = fresh;
                set_agenda_items(list_hwnd, &current, selected);
                self.fields[index].title = agenda_heading(date, current.len());
                InvalidateRect(h, null(), 0);
            }
        }
        let rows = rows.borrow();
        let selected = SendMessageW(list_hwnd, LB_GETCURSEL, 0, 0);
        let count = SendMessageW(list_hwnd, LB_GETCOUNT, 0, 0);
        let content = if count == rows.len() as isize {
            let app = (*ptr).borrow();
            rows.get(selected as usize)
                .and_then(|(id, _)| day_event(&app.cells, &app.events, date, id).map(memo_content))
        } else {
            None
        };
        EnableWindow(GetDlgItem(h, 1), content.is_some() as i32);
        if let Some(preview) = self
            .fields
            .iter()
            .find(|f| matches!(f.kind, Kind::ReadOnly))
        {
            let empty = if rows.is_empty() {
                "이 날짜에는 표시할 일정이 없습니다."
            } else if count != rows.len() as isize {
                "목록을 불러올 수 없습니다. 창을 닫고 다시 열어 주세요."
            } else {
                "일정이 변경되거나 삭제되었습니다. 목록을 닫고 다시 열어 주세요."
            };
            SetWindowTextW(
                preview.hwnd,
                w(content.as_deref().unwrap_or(empty)).as_ptr(),
            );
        }
    }
    unsafe fn values(&self) -> Vec<String> {
        self.fields
            .iter()
            .map(|f| match f.kind {
                Kind::Slider => SendMessageW(f.hwnd, TBM_GETPOS, 0, 0).to_string(),
                Kind::Choice(_) => SendMessageW(f.hwnd, CB_GETCURSEL, 0, 0).to_string(),
                Kind::Agenda { .. } => SendMessageW(f.hwnd, LB_GETCURSEL, 0, 0).to_string(),
                Kind::Check => if SendMessageW(f.hwnd, BM_GETCHECK, 0, 0) == BST_CHECKED as isize {
                    "1"
                } else {
                    "0"
                }
                .into(),
                _ => label(f.hwnd),
            })
            .collect()
    }
}
unsafe extern "system" fn form_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<Form>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_DESTROY {
        if let Ok(mut form) = (*ptr).try_borrow_mut() {
            form.done = true;
        }
        let controller = CONTROLLER_WINDOW.load(Ordering::Relaxed) as HWND;
        if !controller.is_null() {
            PostMessageW(controller, WM_NULL, 0, 0);
        }
        return 0;
    }
    if msg == WM_NOTIFY && lp != 0 {
        let draw = &*(lp as *const NMCUSTOMDRAW);
        if draw.hdr.code == NM_CUSTOMDRAW {
            let slider = (*ptr).try_borrow().ok().and_then(|f| {
                f.fields
                    .iter()
                    .find(|field| matches!(field.kind, Kind::Slider))
                    .map(|field| (field.hwnd, f.dpi))
            });
            if let Some((slider, dpi)) = slider.filter(|(h, _)| *h == draw.hdr.hwndFrom) {
                if draw.dwDrawStage == CDDS_PREPAINT {
                    // Draw once across the control; item painting clips a circular
                    // thumb to the native thin thumb's bounds and leaves stale rails.
                    let px = |n: i32| n * dpi as i32 / 96;
                    let mut area = zeroed();
                    GetClientRect(slider, &mut area);
                    fill(draw.hdc, area, 0x00f8f7f5);
                    let radius = px(13);
                    let left = radius + px(2);
                    let right = area.right - left;
                    let cy = area.bottom / 2;
                    let percent = SendMessageW(slider, TBM_GETPOS, 0, 0).clamp(40, 100) as i32;
                    let cx = left + (right - left) * (percent - 40) / 60;
                    let thickness = px(4).max(2);
                    round(
                        draw.hdc,
                        rect(left, cy - thickness / 2, right - left, thickness),
                        0x00dedbd7,
                        thickness,
                    );
                    round(
                        draw.hdc,
                        rect(left, cy - thickness / 2, cx - left, thickness),
                        0x00d47800,
                        thickness,
                    );
                    round(
                        draw.hdc,
                        rect(cx - radius, cy - radius, radius * 2, radius * 2),
                        0x00d6d2cd,
                        radius * 2,
                    );
                    let inner = radius - px(1).max(1);
                    round(
                        draw.hdc,
                        rect(cx - inner, cy - inner, inner * 2, inner * 2),
                        0x00ffffff,
                        inner * 2,
                    );
                    let center = px(8);
                    round(
                        draw.hdc,
                        rect(cx - center, cy - center, center * 2, center * 2),
                        0x00d47800,
                        center * 2,
                    );
                    if GetFocus() == slider {
                        let focus = rect(1, 1, area.right - 2, area.bottom - 2);
                        DrawFocusRect(draw.hdc, &focus);
                    }
                    return CDRF_SKIPDEFAULT as isize;
                }
            }
        }
    }
    if msg == WM_CLOSE || msg == WM_COMMAND && (wp & 0xffff) == 2 {
        (*ptr).borrow_mut().done = true;
        DestroyWindow(hwnd);
        return 0;
    }
    if msg == WM_COMMAND && (wp & 0xffff) == 200 {
        let is_agenda = (*ptr).try_borrow().is_ok_and(|f| {
            f.fields
                .first()
                .is_some_and(|field| matches!(field.kind, Kind::Agenda { .. }))
        });
        if is_agenda {
            if (wp >> 16) == LBN_SELCHANGE as usize {
                (*ptr).borrow_mut().update_agenda(hwnd, false);
            } else if (wp >> 16) == LBN_DBLCLK as usize && IsWindowEnabled(GetDlgItem(hwnd, 1)) != 0
            {
                PostMessageW(hwnd, WM_COMMAND, 1, 0);
            }
            return 0;
        }
    }
    if msg == WM_HSCROLL {
        let preview = {
            (*ptr).try_borrow().ok().and_then(|f| {
                f.opacity_preview.and_then(|(owner, _)| {
                    f.fields
                        .iter()
                        .any(|field| matches!(field.kind, Kind::Slider) && field.hwnd == lp as HWND)
                        .then(|| (owner, SendMessageW(lp as HWND, TBM_GETPOS, 0, 0) as u32))
                })
            })
        };
        if let Some((owner, percent)) = preview {
            if IsWindow(owner) != 0 {
                SetLayeredWindowAttributes(owner, 0, percent_to_alpha(percent), LWA_ALPHA);
            }
            InvalidateRect(lp as HWND, null(), 0);
            InvalidateRect(hwnd, null(), 0);
            return 0;
        }
    }
    if msg == WM_COMMAND && (wp & 0xffff) == 1 && (wp >> 16) == 0 {
        if IsWindowEnabled(GetDlgItem(hwnd, 1)) == 0 {
            return 0;
        }
        let result = {
            let f = (*ptr).borrow();
            (f.save)(&f.values())
        };
        if let Err(e) = result {
            message(hwnd, "저장할 수 없습니다", &e, MB_OK | MB_ICONWARNING);
        } else {
            let mut f = (*ptr).borrow_mut();
            f.committed = true;
            f.done = true;
            drop(f);
            DestroyWindow(hwnd);
        }
        return 0;
    }
    let Ok(mut f) = (*ptr).try_borrow_mut() else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        WM_CREATE => {
            f.init(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut r = zeroed();
            GetClientRect(hwnd, &mut r);
            fill(dc, r, 0x00f8f7f5);
            for field in &f.fields {
                if !matches!(field.kind, Kind::Check) {
                    text(
                        dc,
                        &field.title,
                        rect(
                            f.px(field.r.left),
                            f.px(field.r.top - 24),
                            f.px(field.r.right - field.r.left),
                            f.px(22),
                        ),
                        f.fonts.small,
                        0x007e7165,
                        DT_LEFT,
                    );
                }
            }
            if let Some(slider) = f
                .fields
                .iter()
                .find(|field| matches!(field.kind, Kind::Slider))
            {
                let percent = SendMessageW(slider.hwnd, TBM_GETPOS, 0, 0) as u32;
                let old_pen = SelectObject(dc, GetStockObject(DC_PEN));
                let old_brush = SelectObject(dc, GetStockObject(HOLLOW_BRUSH));
                SetDCPenColor(dc, 0x00332920);
                let (cx, cy, radius) = (f.px(36), f.px(64), f.px(5));
                Ellipse(dc, cx - radius, cy - radius, cx + radius, cy + radius);
                for (x1, y1, x2, y2) in [
                    (0, -9, 0, -12),
                    (0, 9, 0, 12),
                    (-9, 0, -12, 0),
                    (9, 0, 12, 0),
                    (-7, -7, -9, -9),
                    (7, -7, 9, -9),
                    (-7, 7, -9, 9),
                    (7, 7, 9, 9),
                ] {
                    MoveToEx(dc, cx + f.px(x1), cy + f.px(y1), null_mut());
                    LineTo(dc, cx + f.px(x2), cy + f.px(y2));
                }
                SelectObject(dc, old_pen);
                SelectObject(dc, old_brush);
                text(
                    dc,
                    &format!("{}%", percent),
                    rect(f.px(408), f.px(48), f.px(67), f.px(32)),
                    f.fonts.body,
                    0x007a7167,
                    DT_RIGHT,
                );
            }
            EndPaint(hwnd, &ps);
            0
        }
        _ => {
            drop(f);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}
unsafe fn form(
    parent: HWND,
    title: &str,
    fields: Vec<Field>,
    height: i32,
    preview_opacity: Option<u8>,
    save: SaveForm,
) -> bool {
    if fields
        .iter()
        .any(|field| matches!(field.kind, Kind::Slider))
    {
        let common = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        };
        if InitCommonControlsEx(&common) == 0 {
            show_error("불투명도 조절 컨트롤을 열 수 없습니다.");
            return false;
        }
    }
    let dpi = GetDpiForWindow(parent).max(96);
    let state = Box::new(RefCell::new(Form {
        owner: parent,
        dpi,
        fonts: Fonts::new(dpi),
        fields,
        height,
        done: false,
        committed: false,
        opacity_preview: preview_opacity.map(|opacity| (parent, opacity)),
        save,
    }));
    let mut pr = zeroed();
    GetWindowRect(parent, &mut pr);
    let mut area = rect(0, 0, 500 * dpi as i32 / 96, height * dpi as i32 / 96);
    AdjustWindowRectExForDpi(
        &mut area,
        WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
        0,
        WS_EX_DLGMODALFRAME,
        dpi,
    );
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromWindow(parent, MONITOR_DEFAULTTONEAREST), &mut mi);
    let width = area.right - area.left;
    let hh = area.bottom - area.top;
    let x = (pr.left + 40).clamp(
        mi.rcWork.left,
        (mi.rcWork.right - width).max(mi.rcWork.left),
    );
    let y = (pr.top + 50).clamp(mi.rcWork.top, (mi.rcWork.bottom - hh).max(mi.rcWork.top));
    let h = CreateWindowExW(
        WS_EX_DLGMODALFRAME | WS_EX_APPWINDOW,
        w("DotCalendarForm").as_ptr(),
        w(title).as_ptr(),
        WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
        x,
        y,
        width,
        hh,
        parent,
        null_mut(),
        GetModuleHandleW(null()),
        (&*state) as *const _ as *const c_void,
    );
    if h.is_null() {
        show_error("편집 창을 열 수 없습니다.");
        return false;
    }
    EnableWindow(parent, 0);
    ShowWindow(h, SW_SHOW);
    SetForegroundWindow(h);
    let input = state.borrow().fields[0].hwnd;
    SetFocus(input);
    let signal = if state
        .borrow()
        .fields
        .iter()
        .any(|f| matches!(f.kind, Kind::Agenda { .. }))
    {
        let app = GetWindowLongPtrW(parent, GWLP_USERDATA) as *const RefCell<App>;
        Some((*app).borrow().store.change_event_handle())
    } else {
        None
    };
    let mut msg = zeroed();
    loop {
        if state.borrow().done || IsWindow(h) == 0 {
            break;
        }
        let ret = if let Some(signal) = signal {
            // Use the existing change event while this nested loop owns input.
            // No timer or extra thread is needed to keep Dot updates visible.
            let wait =
                MsgWaitForMultipleObjectsEx(1, &signal, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if wait == WAIT_OBJECT_0 {
                state.borrow_mut().update_agenda(h, true);
                continue;
            }
            if wait == WAIT_FAILED {
                break;
            }
            if PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) == 0 {
                continue;
            }
            if msg.message == WM_QUIT {
                0
            } else {
                1
            }
        } else {
            GetMessageW(&mut msg, null_mut(), 0, 0)
        };
        if ret <= 0 {
            if ret == 0 {
                PostQuitMessage(msg.wParam as i32);
            }
            break;
        }
        if msg.message == WM_KEYDOWN && msg.wParam == VK_ESCAPE as usize {
            SendMessageW(h, WM_CLOSE, 0, 0);
            continue;
        }
        if msg.message == WM_KEYDOWN
            && msg.wParam == VK_RETURN as usize
            && GetKeyState(VK_CONTROL as i32) < 0
        {
            SendMessageW(h, WM_COMMAND, 1, 0);
            continue;
        }
        if IsDialogMessageW(h, &msg) == 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    if IsWindow(h) != 0 {
        DestroyWindow(h);
    }
    let restore_opacity = {
        let f = state.borrow();
        if f.committed {
            None
        } else {
            f.opacity_preview
        }
    };
    if let Some((owner, original)) = restore_opacity {
        if IsWindow(owner) != 0 {
            SetLayeredWindowAttributes(owner, 0, original, LWA_ALPHA);
        }
    }
    if IsWindow(parent) != 0 {
        EnableWindow(parent, 1);
        SetForegroundWindow(parent);
    }
    let committed = state.borrow().committed;
    committed
}

unsafe fn edit_event(parent: HWND, date: Date, event: Option<Event>) {
    let repeat = ["", "daily", "weekly", "monthly", "yearly"];
    let palette = PALETTE.iter().map(|p| p.0.to_string()).collect();
    let fields = vec![
        field(
            "일정 제목",
            event.as_ref().map(|e| e.title.clone()).unwrap_or_default(),
            Kind::Text,
            rect(24, 46, 452, 32),
        ),
        field(
            "날짜 · YYYY-MM-DD",
            event
                .as_ref()
                .map(|e| e.date.clone())
                .unwrap_or(date.to_string()),
            Kind::Text,
            rect(24, 110, 218, 32),
        ),
        field(
            "시간 · HH:MM (비우면 종일)",
            event
                .as_ref()
                .and_then(|e| e.time.clone())
                .unwrap_or_default(),
            Kind::Text,
            rect(258, 110, 218, 32),
        ),
        field(
            "메모",
            event.as_ref().map(|e| e.notes.clone()).unwrap_or_default(),
            Kind::Multiline,
            rect(24, 174, 452, 100),
        ),
        field(
            "반복 · 수정/완료/삭제는 전체 반복에 적용",
            event
                .as_ref()
                .and_then(|e| e.recurrence.as_deref())
                .and_then(|r| repeat.iter().position(|v| *v == r))
                .unwrap_or(0)
                .to_string(),
            Kind::Choice(vec![
                "반복 안 함".into(),
                "매일".into(),
                "매주".into(),
                "매월 · 없는 날짜는 건너뜀".into(),
                "매년".into(),
            ]),
            rect(24, 314, 286, 32),
        ),
        field(
            "색상",
            event
                .as_ref()
                .and_then(|e| e.color.as_deref())
                .and_then(|r| PALETTE.iter().position(|v| v.1 == r))
                .unwrap_or(0)
                .to_string(),
            Kind::Choice(palette),
            rect(326, 314, 150, 32),
        ),
        field(
            "알림 · 몇 분 전 (빈칸: 끔, 시간 필수)",
            event
                .as_ref()
                .and_then(|e| e.reminder_minutes)
                .map(|n| n.to_string())
                .unwrap_or_default(),
            Kind::Text,
            rect(24, 382, 286, 32),
        ),
        field(
            "완료한 일정",
            if event.as_ref().is_some_and(|e| e.completed) {
                "1"
            } else {
                "0"
            },
            Kind::Check,
            rect(326, 382, 150, 32),
        ),
    ];
    form(
        parent,
        if event.is_some() {
            "일정 수정"
        } else {
            "새 일정"
        },
        fields,
        490,
        None,
        Box::new(move |v| {
            let store = Store::open()?;
            let time = if v[2].trim().is_empty() {
                None
            } else {
                Some(v[2].trim())
            };
            let reminder = if v[6].trim().is_empty() {
                None
            } else {
                Some(
                    v[6].trim()
                        .parse::<u32>()
                        .map_err(|_| "알림은 0 이상의 분으로 입력해 주세요.")?,
                )
            };
            if reminder.is_some() && time.is_none() {
                return Err("알림을 받으려면 시간을 입력해 주세요.".into());
            }
            if reminder.is_some_and(|m| m > 10080) {
                return Err("알림은 최대 7일(10080분) 전까지 설정할 수 있습니다.".into());
            }
            let recurrence = repeat[v[4].parse::<usize>().unwrap_or(0).min(4)];
            let c = PALETTE[v[5].parse::<usize>().unwrap_or(0).min(5)].1;
            let c = if c.is_empty() { None } else { Some(c) };
            let recurrence = if recurrence.is_empty() {
                None
            } else {
                Some(recurrence)
            };
            if let Some(e) = &event {
                let mut replacement = e.clone();
                replacement.date = v[1].trim().into();
                replacement.time = time.map(str::to_owned);
                replacement.title = v[0].trim().into();
                replacement.notes = v[3].clone();
                replacement.completed = v[7] == "1";
                replacement.color = c.map(str::to_owned);
                replacement.recurrence = recurrence.map(str::to_owned);
                replacement.reminder_minutes = reminder;
                if !store.replace_if_unchanged(e, &replacement)? {
                    return Err("편집 중 다른 곳에서 일정이 변경되었습니다. 내용을 복사해 두고, 편집 창을 닫았다가 다시 열어 주세요.".into());
                }
            } else {
                store.create_event_with_details(
                    v[1].trim(),
                    time,
                    &v[0],
                    &v[3],
                    None,
                    v[7] == "1",
                    c,
                    recurrence,
                    reminder,
                )?;
            }
            Ok(())
        }),
    );
}

unsafe fn settings(parent: HWND) {
    let ptr = GetWindowLongPtrW(parent, GWLP_USERDATA) as *const RefCell<App>;
    let prefs = (*ptr).borrow().prefs.clone();
    let g = google::load_config().unwrap_or_default();
    let fields = vec![
        field(
            "불투명도 · 드래그하거나 방향키로 조절",
            alpha_to_percent(prefs.opacity).to_string(),
            Kind::Slider,
            rect(54, 48, 345, 32),
        ),
        field(
            "테마",
            if prefs.dark { "1" } else { "0" },
            Kind::Choice(vec!["라이트".into(), "다크".into()]),
            rect(24, 118, 218, 32),
        ),
        field(
            "동기화 간격 · 1 ~ 60분",
            prefs.sync_minutes.to_string(),
            Kind::Text,
            rect(258, 118, 218, 32),
        ),
        field(
            "바탕화면 고정",
            if prefs.desktop { "1" } else { "0" },
            Kind::Check,
            rect(24, 174, 220, 28),
        ),
        field(
            "Windows 로그인 시 시작",
            if prefs.auto_start { "1" } else { "0" },
            Kind::Check,
            rect(254, 174, 230, 28),
        ),
        field(
            "한국 음력 표시",
            if prefs.lunar { "1" } else { "0" },
            Kind::Check,
            rect(24, 211, 220, 28),
        ),
        field(
            "한국 공휴일 표시",
            if prefs.holidays { "1" } else { "0" },
            Kind::Check,
            rect(254, 211, 220, 28),
        ),
        field(
            "Google OAuth 클라이언트 ID · 데스크톱 앱",
            g.client_id,
            Kind::Text,
            rect(24, 278, 452, 32),
        ),
        field(
            "Google 클라이언트 보안 비밀번호 · 해당할 때 입력",
            g.client_secret.unwrap_or_default(),
            Kind::Password,
            rect(24, 345, 452, 32),
        ),
        field(
            "Google Calendar ID · 기본 캘린더는 primary",
            if g.calendar_id.is_empty() {
                "primary".into()
            } else {
                g.calendar_id
            },
            Kind::Text,
            rect(24, 412, 452, 32),
        ),
    ];
    form(
        parent,
        "달력 설정 · 저장 후 메뉴에서 Google 계정 연결",
        fields,
        510,
        Some(prefs.opacity),
        Box::new(move |v| {
            let opacity = v[0]
                .trim()
                .parse::<u32>()
                .map_err(|_| "불투명도 값을 읽을 수 없습니다.")?;
            if !(40..=100).contains(&opacity) {
                return Err("불투명도는 40~100% 범위입니다.".into());
            }
            let minutes = v[2]
                .trim()
                .parse::<u32>()
                .map_err(|_| "동기화 간격은 1~60분입니다.")?;
            if !(1..=60).contains(&minutes) {
                return Err("동기화 간격은 1~60분입니다.".into());
            }
            let mut p = prefs.clone();
            p.opacity = if opacity == alpha_to_percent(prefs.opacity) {
                prefs.opacity
            } else {
                percent_to_alpha(opacity)
            };
            p.dark = v[1] == "1";
            p.sync_minutes = minutes;
            p.desktop = v[3] == "1";
            p.auto_start = v[4] == "1";
            p.lunar = v[5] == "1";
            p.holidays = v[6] == "1";
            if !v[7].trim().is_empty() {
                google::save_config(&google::GoogleConfig {
                    client_id: v[7].trim().into(),
                    client_secret: if v[8].trim().is_empty() {
                        None
                    } else {
                        Some(v[8].trim().into())
                    },
                    calendar_id: if v[9].trim().is_empty() {
                        "primary".into()
                    } else {
                        v[9].trim().into()
                    },
                })?;
            }
            if p.auto_start != prefs.auto_start {
                preferences::set_startup(p.auto_start)?;
            }
            p.save()?;
            (*ptr).borrow_mut().prefs = p;
            Ok(())
        }),
    );
}

unsafe fn file_dialog(parent: HWND, open: bool) -> Option<std::path::PathBuf> {
    let mut buffer = [0u16; 32768];
    if !open {
        let name = w("dot-calendar-backup.json");
        buffer[..name.len()].copy_from_slice(&name);
    }
    let filter = w("Dot Calendar backup (*.json)\0*.json\0\0");
    let mut ofn: OPENFILENAMEW = zeroed();
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = parent;
    ofn.lpstrFile = buffer.as_mut_ptr();
    ofn.nMaxFile = buffer.len() as u32;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.Flags = OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if open {
            OFN_FILEMUSTEXIST
        } else {
            OFN_OVERWRITEPROMPT
        };
    let ext = w("json");
    ofn.lpstrDefExt = ext.as_ptr();
    let ok = if open {
        GetOpenFileNameW(&mut ofn)
    } else {
        GetSaveFileNameW(&mut ofn)
    };
    if ok == 0 {
        None
    } else {
        Some(std::path::PathBuf::from(String::from_utf16_lossy(
            &buffer[..buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len())],
        )))
    }
}
unsafe fn backup(hwnd: HWND, restore: bool) {
    let Some(path) = file_dialog(hwnd, restore) else {
        return;
    };
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    let result = (|| -> Result<(), String> {
        let store = Store::open()?;
        if restore {
            if message(hwnd,"백업 가져오기","현재 일정을 백업 내용으로 교체합니다. 현재 데이터는 복원 전 백업으로 보관합니다. 계속할까요?",MB_YESNO|MB_ICONWARNING|MB_DEFBUTTON2)!=IDYES{return Ok(());}
            let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
            if metadata.len() > 4 * 1024 * 1024 {
                return Err("백업 파일은 4 MiB 이하여야 합니다.".into());
            }
            let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let name = format!(
                "before-restore-{}.json",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            );
            std::fs::write(preferences::directory().join(name), store.export_json()?)
                .map_err(|e| e.to_string())?;
            store.restore_json(&data)?;
        } else {
            std::fs::write(path, store.export_json()?).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        show_error(&e);
    } else {
        (*ptr).borrow_mut().refresh();
    }
}
unsafe fn print_month(hwnd: HWND) {
    let mut pd: PRINTDLGW = zeroed();
    pd.lStructSize = std::mem::size_of::<PRINTDLGW>() as u32;
    pd.hwndOwner = hwnd;
    pd.Flags = PD_RETURNDC | PD_NOPAGENUMS | PD_NOSELECTION;
    pd.nCopies = 1;
    if PrintDlgW(&mut pd) == 0 {
        return;
    }
    let title = w("Dot Calendar");
    let info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: title.as_ptr(),
        ..zeroed()
    };
    if StartDocW(pd.hDC, &info) > 0 {
        if StartPage(pd.hDC) > 0 {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
            let a = (*ptr).borrow();
            let r = a.client();
            let ww = GetDeviceCaps(pd.hDC, HORZRES as i32);
            let hh = GetDeviceCaps(pd.hDC, VERTRES as i32);
            let scale = (ww as f64 / r.right as f64).min(hh as f64 / r.bottom as f64);
            SetMapMode(pd.hDC, MM_ANISOTROPIC);
            SetWindowExtEx(pd.hDC, r.right, r.bottom, null_mut());
            SetViewportExtEx(
                pd.hDC,
                (r.right as f64 * scale) as i32,
                (r.bottom as f64 * scale) as i32,
                null_mut(),
            );
            a.paint(pd.hDC);
            EndPage(pd.hDC);
        }
        EndDoc(pd.hDC);
    }
    DeleteDC(pd.hDC);
    if !pd.hDevMode.is_null() {
        GlobalFree(pd.hDevMode);
    }
    if !pd.hDevNames.is_null() {
        GlobalFree(pd.hDevNames);
    }
}

unsafe fn create_widget(state: &RefCell<App>) -> Result<HWND, String> {
    let dpi = GetDpiForSystem().max(96);
    let scale = |v: i32| v * dpi as i32 / 96;
    let mut work: RECT = zeroed();
    SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut _ as *mut c_void, 0);
    let mut position = [
        work.left + scale(170),
        work.top + scale(55),
        scale(1100).min(work.right - work.left - scale(200)),
        scale(730).min(work.bottom - work.top - scale(90)),
    ];
    let saved_position = state.borrow().prefs.position;
    if let Some(p) = saved_position {
        if !MonitorFromRect(&rect(p[0], p[1], p[2], p[3]), MONITOR_DEFAULTTONULL).is_null() {
            position = p;
        }
    }
    position[2] = position[2].max(scale(840));
    position[3] = position[3].max(scale(530));
    let hwnd = CreateWindowExW(
        // Establish the layered surface before desktop::attach can show the child.
        WS_EX_APPWINDOW | WS_EX_LAYERED,
        w("DotCalendarWidget").as_ptr(),
        w("Dot Calendar").as_ptr(),
        WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
        position[0],
        position[1],
        position[2],
        position[3],
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        state as *const _ as *const c_void,
    );
    if hwnd.is_null() {
        return Err(format!(
            "달력 창 생성 실패: {}",
            std::io::Error::last_os_error()
        ));
    }
    let corner = DWMWCP_ROUND;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        &corner as *const _ as *const c_void,
        4,
    );
    apply_preferences(hwnd);
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    UpdateWindow(hwnd);
    Ok(hwnd)
}

unsafe extern "system" fn controller_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<App>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_DESTROY {
        CONTROLLER_WINDOW.store(0, Ordering::Relaxed);
        if (*ptr).try_borrow().is_ok_and(|app| app.quitting) {
            tray(hwnd, NIM_DELETE, None);
            PostQuitMessage(0);
        }
        return 0;
    }
    if msg == SYNC_DONE {
        let Ok(mut app) = (*ptr).try_borrow_mut() else {
            SetTimer(hwnd, 5, 100, None);
            return 0;
        };
        let output = app.sync_result.lock().ok().and_then(|mut slot| slot.take());
        app.busy = false;
        if let Some(output) = output {
            app.status = output.unwrap_or_else(|error| format!("Google: {error}"));
        }
        if !app.hwnd.is_null() && IsWindow(app.hwnd) != 0 {
            app.arm_sync();
            app.refresh();
            app.redraw();
        } else {
            PostMessageW(hwnd, REBUILD_WIDGET, 0, 0);
        }
        return 0;
    }
    if msg == TRAY {
        let widget = (*ptr)
            .try_borrow()
            .map(|app| app.hwnd)
            .unwrap_or(null_mut());
        if !widget.is_null() && IsWindow(widget) != 0 {
            match lp as u32 {
                WM_LBUTTONDBLCLK => command(widget, SHOW),
                WM_RBUTTONUP | WM_CONTEXTMENU => popup(widget, None),
                _ => {}
            }
        } else {
            PostMessageW(hwnd, REBUILD_WIDGET, 0, 0);
        }
        return 0;
    }
    if msg == SHOW_WIDGET {
        let widget = (*ptr)
            .try_borrow()
            .map(|app| app.hwnd)
            .unwrap_or(null_mut());
        if !widget.is_null() && IsWindow(widget) != 0 {
            command(widget, SHOW);
        } else {
            PostMessageW(hwnd, REBUILD_WIDGET, 0, 0);
        }
        return 0;
    }
    if msg == REBUILD_WIDGET {
        let widget = (*ptr)
            .try_borrow()
            .map(|app| app.hwnd)
            .unwrap_or(null_mut());
        if !widget.is_null() && IsWindow(widget) != 0 {
            let desktop_requested = (*ptr).try_borrow().is_ok_and(|app| app.prefs.desktop);
            if desktop_requested && !desktop::is_attached(widget) {
                apply_preferences(widget);
            }
        } else {
            // Explorer can still be rebuilding its desktop when it destroys the child.
            // Delay one attempt, then keep the normal-window fallback if attachment fails.
            SetTimer(hwnd, 4, 750, None);
        }
        return 0;
    }
    if msg == WM_TIMER && wp == 4 {
        KillTimer(hwnd, 4);
        let widget = match (*ptr).try_borrow() {
            Ok(app) => app.hwnd,
            Err(_) => {
                SetTimer(hwnd, 4, 250, None);
                return 0;
            }
        };
        if widget.is_null() || IsWindow(widget) == 0 {
            {
                let Ok(mut app) = (*ptr).try_borrow_mut() else {
                    SetTimer(hwnd, 4, 250, None);
                    return 0;
                };
                app.hwnd = null_mut();
                app.buttons.clear();
            }
            if let Err(error) = create_widget(&*ptr) {
                show_error(&error);
                (*ptr).borrow_mut().quitting = true;
                DestroyWindow(hwnd);
            }
        }
        return 0;
    }
    if msg == WM_TIMER && wp == 5 {
        KillTimer(hwnd, 5);
        PostMessageW(hwnd, SYNC_DONE, 0, 0);
        return 0;
    }
    let taskbar_created = (*ptr)
        .try_borrow()
        .map(|app| app.taskbar_created)
        .unwrap_or(0);
    if taskbar_created != 0 && msg == taskbar_created {
        tray(hwnd, NIM_ADD, None);
        PostMessageW(hwnd, REBUILD_WIDGET, 0, 0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

pub fn run() -> Result<(), String> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let store = Store::open()?;
        let signal = store.change_event_handle();
        let mutex = CreateMutexW(
            null(),
            0,
            w(&format!("{}.UI", store.change_event_name())).as_ptr(),
        );
        if mutex.is_null() {
            return Err("달력을 시작할 수 없습니다.".into());
        }
        // Own the mutex for the lifetime of this UI. GetLastError after a Rust
        // temporary is dropped is not a reliable instance-existence check.
        let instance_wait = WaitForSingleObject(mutex, 0);
        if instance_wait == WAIT_TIMEOUT {
            let h = FindWindowW(
                w("DotCalendarController").as_ptr(),
                w(store.change_event_name()).as_ptr(),
            );
            if !h.is_null() {
                PostMessageW(h, SHOW_WIDGET, 0, 0);
            }
            CloseHandle(mutex);
            return Ok(());
        }
        if instance_wait != WAIT_OBJECT_0 && instance_wait != WAIT_ABANDONED {
            CloseHandle(mutex);
            return Err("달력 실행 잠금을 얻을 수 없습니다.".into());
        }
        let instance = GetModuleHandleW(null());
        for (class, proc) in [
            (
                "DotCalendarController",
                Some(controller_proc as unsafe extern "system" fn(_, _, _, _) -> _),
            ),
            (
                "DotCalendarWidget",
                Some(window_proc as unsafe extern "system" fn(_, _, _, _) -> _),
            ),
            (
                "DotCalendarForm",
                Some(form_proc as unsafe extern "system" fn(_, _, _, _) -> _),
            ),
        ] {
            let name = w(class);
            let wc = WNDCLASSW {
                style: CS_DBLCLKS,
                lpfnWndProc: proc,
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                lpszClassName: name.as_ptr(),
                ..zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                ReleaseMutex(mutex);
                CloseHandle(mutex);
                return Err("달력 창 등록에 실패했습니다.".into());
            }
        }
        let prefs = Preferences::load();
        let dpi = GetDpiForSystem().max(96);
        let today = Date::today();
        let state = Box::new(RefCell::new(App {
            hwnd: null_mut(),
            controller: null_mut(),
            store,
            dpi,
            fonts: Fonts::new(dpi),
            prefs,
            month: Date { day: 1, ..today },
            selected: today,
            selected_id: None,
            inline: None,
            cells: vec![],
            grid_key: None,
            events: vec![],
            buttons: vec![],
            status: String::new(),
            sync_result: Arc::new(Mutex::new(None)),
            busy: false,
            notified: HashMap::new(),
            pending: None,
            taskbar_created: RegisterWindowMessageW(w("TaskbarCreated").as_ptr()),
            quitting: false,
        }));
        let controller_name = w(state.borrow().store.change_event_name());
        let controller = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w("DotCalendarController").as_ptr(),
            controller_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            (&*state) as *const _ as *const c_void,
        );
        if controller.is_null() {
            ReleaseMutex(mutex);
            CloseHandle(mutex);
            return Err(format!(
                "달력 컨트롤러 생성 실패: {}",
                std::io::Error::last_os_error()
            ));
        }
        CONTROLLER_WINDOW.store(controller as isize, Ordering::Relaxed);
        state.borrow_mut().controller = controller;
        tray(controller, NIM_ADD, None);
        if let Err(error) = create_widget(&state) {
            state.borrow_mut().quitting = true;
            DestroyWindow(controller);
            ReleaseMutex(mutex);
            CloseHandle(mutex);
            return Err(error);
        }
        let mut msg: MSG = zeroed();
        'events: loop {
            let ret =
                MsgWaitForMultipleObjectsEx(1, &signal, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if ret == WAIT_FAILED {
                break;
            }
            if ret == WAIT_OBJECT_0 {
                let widget = state.borrow().hwnd;
                if !widget.is_null() && IsWindow(widget) != 0 {
                    SendMessageW(widget, RELOAD, 0, 0);
                } else {
                    PostMessageW(controller, REBUILD_WIDGET, 0, 0);
                }
            }
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    break 'events;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        ReleaseMutex(mutex);
        CloseHandle(mutex);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_hit_respects_block_edges_and_gaps() {
        let cell = rect(10, 20, 120, 180);
        let first = event_row_rect(cell, 0, 96);
        let second = event_row_rect(cell, 1, 96);
        assert_eq!(event_row_hit(cell, first.left, first.top, 2, 96), Some(0));
        assert_eq!(event_row_hit(cell, second.left, second.top, 2, 96), Some(1));
        assert_eq!(event_row_hit(cell, first.left - 1, first.top, 2, 96), None);
        assert_eq!(event_row_hit(cell, first.right, first.top, 2, 96), None);
        assert_eq!(event_row_hit(cell, first.left, first.bottom, 2, 96), None);
        assert_eq!(event_row_hit(cell, second.left, second.top, 1, 96), None);
    }

    #[test]
    fn agenda_has_no_hundred_item_cap_and_resolves_stable_ids_after_reload() {
        let date = Date::parse("2026-10-08").unwrap();
        let mut events: Vec<Event> = (0..150)
            .rev()
            .map(|n| {
                serde_json::from_value(serde_json::json!({
                    "id": format!("event-{n:03}"), "date": date.to_string(),
                    "title": format!("일정 {n:03}"), "notes": format!("본문 {n:03}\n마지막 줄")
                }))
                .unwrap()
            })
            .collect();
        let mut cells = vec![Cell {
            date,
            date_key: date.to_string(),
            lunar: String::new(),
            holiday: String::new(),
            events: vec![],
        }];
        populate_cell_events(&mut cells, &events, date, date, true);
        let rows = agenda_rows(&cells[0], &events);
        assert_eq!(rows.len(), 150);
        assert_eq!(rows[100], ("event-100".into(), "종일  일정 100".into()));
        assert_eq!(rows[149].0, "event-149");
        let id = &rows[149].0;
        events.reverse();
        populate_cell_events(&mut cells, &events, date, date, true);
        assert_eq!(
            memo_content(day_event(&cells, &events, date, id).unwrap()),
            "일정 149\r\n\r\n본문 149\r\n마지막 줄"
        );
        assert!(day_event(&cells, &events, shift(date, 1), id).is_none());
        events.retain(|event| event.id != *id);
        populate_cell_events(&mut cells, &events, date, date, true);
        assert!(day_event(&cells, &events, date, id).is_none());
        assert_eq!(agenda_rows(&cells[0], &events).len(), 149);
    }

    #[test]
    fn blank_note_and_leading_empty_lines() {
        for raw in ["", " \t\n", "\r\n\r\n", "\u{3000}"] {
            assert_eq!(note_parts(&normalize_newlines(raw)), None);
        }
        let raw = normalize_newlines("\r\n  \r\n  남은 메모  \r\n둘째 줄\r\n\r\n마지막 줄");
        assert_eq!(
            note_parts(&raw),
            Some(("남은 메모", "둘째 줄\n\n마지막 줄"))
        );
        assert_eq!(
            note_parts("제목\n\n  본문  \n"),
            Some(("제목", "\n  본문  \n"))
        );
    }

    #[test]
    fn google_visibility_filters_recurrences_without_changing_sources() {
        let events: Vec<Event> = serde_json::from_value(serde_json::json!([
            {"id":"local", "date":"2026-10-08", "title":"직접 메모", "notes":""},
            {"id":"google", "date":"2026-10-08", "title":"구글 반복", "notes":"", "recurrence":"daily", "request_id":format!("google-{}", "a".repeat(64))},
            {"id":"dot", "date":"2026-10-09", "title":"닷 메모", "notes":"", "request_id":"dot-123"}
        ])).unwrap();
        let original = events.clone();
        let start = Date::parse("2026-10-08").unwrap();
        let end = shift(start, 2);
        let mut cells: Vec<_> = (0..3)
            .map(|n| Cell {
                date: shift(start, n),
                date_key: String::new(),
                lunar: String::new(),
                holiday: String::new(),
                events: vec![],
            })
            .collect();
        populate_cell_events(&mut cells, &events, start, end, true);
        let shown: Vec<_> = cells.iter().map(|c| c.events.clone()).collect();
        assert_eq!(shown.iter().map(Vec::len).sum::<usize>(), 5);
        populate_cell_events(&mut cells, &events, start, end, false);
        assert!(day_event(&cells, &events, shift(start, 1), "google").is_none());
        assert_eq!(
            cells.iter().map(|c| c.events.clone()).collect::<Vec<_>>(),
            vec![vec![0], vec![2], vec![]]
        );
        populate_cell_events(&mut cells, &events, start, end, true);
        assert_eq!(
            day_event(&cells, &events, shift(start, 1), "google")
                .unwrap()
                .date,
            "2026-10-08"
        );
        assert_eq!(
            cells.iter().map(|c| c.events.clone()).collect::<Vec<_>>(),
            shown
        );
        assert_eq!(events, original);
    }

    #[test]
    fn opacity_percent_round_trip() {
        assert_eq!(alpha_to_percent(235), 92);
        for percent in 40..=100 {
            assert_eq!(alpha_to_percent(percent_to_alpha(percent)), percent);
        }
    }

    #[test]
    fn streamed_cells_and_reminders_match_sorted_occurrences() {
        use crate::model::occurrence_indices;
        let events: Vec<Event> = serde_json::from_value(serde_json::json!([
            {"id":"later", "date":"2026-09-30", "time":"10:00", "title":"Later", "notes":"", "recurrence":"daily", "reminder_minutes":60},
            {"id":"earlier", "date":"2026-09-29", "time":"09:00", "title":"Earlier", "notes":"", "recurrence":"daily", "reminder_minutes":0},
            {"id":"weekly", "date":"2026-09-30", "time":"08:00", "title":"Weekly", "notes":"", "recurrence":"weekly", "reminder_minutes":0},
            {"id":"done", "date":"2026-09-30", "time":"07:00", "title":"Done", "notes":"", "completed":true, "reminder_minutes":0},
            {"id":"silent", "date":"2026-09-30", "time":"06:00", "title":"Silent", "notes":"", "recurrence":"daily"},
            {"id":"all-day", "date":"2026-09-30", "title":"All day", "notes":"", "recurrence":"daily", "reminder_minutes":0}
        ])).unwrap();
        let start = Date::parse("2026-09-27").unwrap();
        let end = shift(start, 41);
        let mut cells: Vec<_> = (0..42)
            .map(|n| Cell {
                date: shift(start, n),
                date_key: String::new(),
                lunar: String::new(),
                holiday: String::new(),
                events: vec![usize::MAX],
            })
            .collect();
        populate_cell_events(&mut cells, &events, start, end, true);
        let actual: Vec<_> = cells
            .iter()
            .flat_map(|cell| cell.events.iter().map(|&i| (cell.date, i)))
            .collect();
        assert_eq!(actual, occurrence_indices(&events, start, end));

        let today = Date::parse("2026-09-30").unwrap();
        let mut notified = HashMap::new();
        for now in [
            wall_seconds(today, 7, 0),
            wall_seconds(today, 9, 0),
            wall_seconds(today, 10, 0),
        ] {
            for _ in 0..3 {
                let expected = occurrence_indices(&events, today, shift(today, 8))
                    .into_iter()
                    .filter_map(|(date, i)| {
                        let e = &events[i];
                        let (Some(minutes), Some(time)) = (e.reminder_minutes, e.time.as_deref())
                        else {
                            return None;
                        };
                        let due = wall_seconds(
                            date,
                            time[..2].parse().unwrap(),
                            time[3..].parse().unwrap(),
                        ) - minutes as i64 * 60;
                        let key = format!("{}:{}:{}", e.id, date, time);
                        (!e.completed && due >= now - 60 && !notified.contains_key(&key))
                            .then(|| (key, format!("{} · {}", time, e.title), due))
                    })
                    .min_by_key(|v| v.2);
                let actual = next_reminder(&events, &notified, today, now);
                assert_eq!(actual, expected);
                if let Some((key, _, due)) = actual {
                    notified.insert(key, due);
                }
            }
        }
        assert!(next_reminder(&events[4..], &notified, today, wall_seconds(today, 0, 0)).is_none());
    }

    #[test]
    fn grid_and_recurrence_dates() {
        assert_eq!(weekday(Date::parse("2026-10-01").unwrap()), 4);
        assert_eq!(
            shift(Date::parse("2026-03-01").unwrap(), -1).to_string(),
            "2026-02-28"
        );
        assert_eq!(
            month_shift(Date::parse("2026-12-01").unwrap(), 1).to_string(),
            "2027-01-01"
        );
        assert_eq!(
            shift(Date::parse("0001-01-01").unwrap(), -1).to_string(),
            "0001-01-01"
        );
    }
}
