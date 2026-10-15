use super::*;

const SEARCH: i32 = 101;
const RESULTS: i32 = 102;
const CLOSE: i32 = 103;
const CLOSE_TIMER: usize = 24;
const COMMANDS: &[PaletteCommand] = &[
    PaletteCommand {
        id: MONTH_TAB,
        category: "보기",
        title: "월간 달력",
        keywords: "calendar month 월간",
        shortcut: "",
    },
    PaletteCommand {
        id: DAY_TAB,
        category: "보기",
        title: "선택한 날짜의 일정",
        keywords: "day agenda 하루",
        shortcut: "",
    },
    PaletteCommand {
        id: DONE_TAB,
        category: "보기",
        title: "완료한 일정",
        keywords: "completed done 완료",
        shortcut: "",
    },
    PaletteCommand {
        id: TODAY,
        category: "이동",
        title: "오늘로 이동",
        keywords: "today 오늘",
        shortcut: "",
    },
    PaletteCommand {
        id: ADD,
        category: "일정",
        title: "새 메모 작성",
        keywords: "new add note 메모 추가",
        shortcut: "Enter",
    },
    PaletteCommand {
        id: NEW_DETAILS,
        category: "일정",
        title: "시간·반복 일정 만들기",
        keywords: "new event repeat 일정",
        shortcut: "",
    },
    PaletteCommand {
        id: VIEW,
        category: "일정",
        title: "선택한 메모 전체 보기",
        keywords: "view memo 전체",
        shortcut: "Space",
    },
    PaletteCommand {
        id: SETTINGS,
        category: "앱",
        title: "설정 열기",
        keywords: "settings theme 설정",
        shortcut: "",
    },
    PaletteCommand {
        id: DOT,
        category: "앱",
        title: "AI 연결 안내",
        keywords: "ai dot connect 연결",
        shortcut: "",
    },
    PaletteCommand {
        id: SHOWCASE,
        category: "앱",
        title: "네이티브 컴포넌트 체험",
        keywords: "showcase demo component 데모",
        shortcut: "",
    },
];

struct State {
    owner: HWND,
    root: HWND,
    panel: HWND,
    backdrop: HWND,
    search: HWND,
    list: HWND,
    dpi: u32,
    fonts: Fonts,
    theme: Theme,
    matches: Vec<usize>,
    accepted: Option<usize>,
    closing: bool,
    composing: bool,
    done: bool,
    previous_focus: HWND,
    width: i32,
    height: i32,
    original: Vec<u32>,
    blurred: Vec<u32>,
    blur_width: i32,
    blur_height: i32,
    glass: Vec<u32>,
}
impl State {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
}

unsafe extern "system" fn search_proc(
    h: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let state = data as *const RefCell<State>;
    match msg {
        WM_IME_STARTCOMPOSITION => (*state).borrow_mut().composing = true,
        WM_IME_ENDCOMPOSITION => (*state).borrow_mut().composing = false,
        WM_NCDESTROY => {
            RemoveWindowSubclass(h, Some(search_proc), 25);
        }
        _ => {}
    }
    DefSubclassProc(h, msg, wp, lp)
}

/// One cached, separable 12-DIP blur of the application's own client surface.
/// No live backdrop effect, nested scrolling blur or animation-frame raster work.
fn blur(pixels: &[u32], width: usize, height: usize, radius: usize) -> Vec<u32> {
    let mut current = pixels.to_vec();
    let mut work = vec![0u32; pixels.len()];
    if width == 0 || height == 0 {
        return current;
    }
    for horizontal in [true, false, true, false, true, false] {
        let (lines, length) = if horizontal {
            (height, width)
        } else {
            (width, height)
        };
        let r = (radius / 2).max(1);
        let divisor = (r * 2 + 1) as u64;
        for line in 0..lines {
            let at = |position: usize| {
                if horizontal {
                    line * width + position
                } else {
                    position * width + line
                }
            };
            let mut sums = [0u64; 3];
            for n in -(r as isize)..=r as isize {
                let value = current[at(n.clamp(0, length as isize - 1) as usize)];
                for (i, sum) in sums.iter_mut().enumerate() {
                    *sum += ((value >> (i * 8)) & 255) as u64;
                }
            }
            for position in 0..length {
                work[at(position)] = 0xff000000
                    | ((sums[0] / divisor) as u32)
                    | (((sums[1] / divisor) as u32) << 8)
                    | (((sums[2] / divisor) as u32) << 16);
                let old = current[at(position.saturating_sub(r))];
                let new = current[at((position + r + 1).min(length - 1))];
                for (i, sum) in sums.iter_mut().enumerate() {
                    *sum = *sum - ((old >> (i * 8)) & 255) as u64 + ((new >> (i * 8)) & 255) as u64;
                }
            }
        }
        std::mem::swap(&mut current, &mut work);
    }
    current
}
unsafe fn bitmap(dc: HDC, pixels: &[u32], width: i32, height: i32) {
    if pixels.len() != (width * height) as usize {
        return;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        },
        ..zeroed()
    };
    SetDIBitsToDevice(
        dc,
        0,
        0,
        width as u32,
        height as u32,
        0,
        0,
        0,
        height as u32,
        pixels.as_ptr() as _,
        &info,
        DIB_RGB_COLORS,
    );
}
unsafe fn stretch_bitmap(
    dc: HDC,
    pixels: &[u32],
    width: i32,
    height: i32,
    target_width: i32,
    target_height: i32,
) {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        },
        ..zeroed()
    };
    SetStretchBltMode(dc, HALFTONE);
    SetBrushOrgEx(dc, 0, 0, null_mut());
    StretchDIBits(
        dc,
        0,
        0,
        target_width,
        target_height,
        0,
        0,
        width,
        height,
        pixels.as_ptr() as _,
        &info,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
}
unsafe fn capture(owner: HWND, width: i32, height: i32) -> Vec<u32> {
    let dc = GetDC(owner);
    let mem = CreateCompatibleDC(dc);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        },
        ..zeroed()
    };
    let mut bits = null_mut();
    let image = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    let mut pixels = vec![0xff0b0909; (width * height) as usize];
    if !mem.is_null() && !image.is_null() && !bits.is_null() {
        let previous = SelectObject(mem, image);
        BitBlt(mem, 0, 0, width, height, dc, 0, 0, SRCCOPY);
        let count = pixels.len();
        pixels.copy_from_slice(std::slice::from_raw_parts(bits as *const u32, count));
        SelectObject(mem, previous);
    }
    if !image.is_null() {
        DeleteObject(image);
    }
    if !mem.is_null() {
        DeleteDC(mem);
    }
    ReleaseDC(owner, dc);
    pixels
}

unsafe fn update(raw: *const RefCell<State>) {
    let (search, list) = {
        let s = (*raw).borrow();
        (s.search, s.list)
    };
    let matches = search_commands(COMMANDS, &label(search));
    (*raw).borrow_mut().matches = matches.clone();
    SendMessageW(list, WM_SETREDRAW, 0, 0);
    SendMessageW(list, LB_RESETCONTENT, 0, 0);
    for index in matches {
        let c = &COMMANDS[index];
        let name = format!("{} · {} {}", c.category, c.title, c.shortcut);
        if SendMessageW(list, LB_ADDSTRING, 0, w(&name).as_ptr() as isize) < 0 {
            (*raw).borrow_mut().matches.clear();
            SendMessageW(list, LB_RESETCONTENT, 0, 0);
            break;
        }
    }
    SendMessageW(list, LB_SETCURSEL, 0, 0);
    SendMessageW(list, WM_SETREDRAW, 1, 0);
    InvalidateRect(list, null(), 1);
    InvalidateRect((*raw).borrow().panel, null(), 0);
}
unsafe fn start_close(raw: *const RefCell<State>, accept: bool) {
    let (root, panel, backdrop, owner, focus) = {
        let mut s = (*raw).borrow_mut();
        if s.closing {
            return;
        }
        if accept {
            let selected = SendMessageW(s.list, LB_GETCURSEL, 0, 0);
            s.accepted = usize::try_from(selected)
                .ok()
                .and_then(|i| s.matches.get(i))
                .copied();
            if s.accepted.is_none() {
                return;
            }
        }
        s.closing = true;
        (s.root, s.panel, s.backdrop, s.owner, s.previous_focus)
    };
    EnableWindow(owner, 1);
    SetForegroundWindow(GetAncestor(owner, GA_ROOT));
    if IsWindow(focus) != 0 && IsWindowEnabled(focus) != 0 {
        SetFocus(focus);
    } else {
        SetFocus(owner);
    }
    animate(root, backdrop, Motion::backdrop(false), Finish::Restore);
    animate(root, panel, Motion::dialog(false), Finish::Restore);
    if reduced_motion() {
        (*raw).borrow_mut().done = true;
    } else {
        SetTimer(root, CLOSE_TIMER, DURATION_NORMAL + 24, None);
    }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        SetWindowLongPtrW(
            h,
            GWLP_USERDATA,
            (*(lp as *const CREATESTRUCTW)).lpCreateParams as isize,
        );
    }
    let raw = GetWindowLongPtrW(h, GWLP_USERDATA) as *const RefCell<State>;
    if raw.is_null() {
        return DefWindowProcW(h, msg, wp, lp);
    }
    match msg {
        WM_CLOSE => {
            start_close(raw, false);
            return 0;
        }
        WM_TIMER if wp == UI_TIMER => {
            settle(h, false);
            return 0;
        }
        WM_TIMER if wp == CLOSE_TIMER => {
            KillTimer(h, CLOSE_TIMER);
            (*raw).borrow_mut().done = true;
            return 0;
        }
        WM_COMMAND => {
            let id = (wp & 0xffff) as i32;
            let code = (wp >> 16) as u32;
            if id == SEARCH && code == EN_CHANGE {
                update(raw);
            }
            if id == CLOSE {
                start_close(raw, false);
            }
            if id == RESULTS && code == LBN_DBLCLK {
                start_close(raw, true);
            }
            return 0;
        }
        WM_LBUTTONDOWN => {
            if h == (*raw).borrow().backdrop {
                start_close(raw, false);
            }
            return 0;
        }
        WM_SETTINGCHANGE => {
            let (root, owner) = {
                let s = (*raw).borrow();
                (s.root, s.owner)
            };
            settle(root, true);
            let dark = (*app_ptr(owner)).borrow().prefs.dark;
            (*raw).borrow_mut().theme = Theme::new(dark);
            RedrawWindow(root, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
            return 0;
        }
        WM_DPICHANGED => {
            // Reopen at the new monitor's DPI; closing keeps any inline draft.
            start_close(raw, false);
            return 0;
        }
        WM_ERASEBKGND => return 1,
        _ => {}
    }
    let Ok(s) = (*raw).try_borrow() else {
        return DefWindowProcW(h, msg, wp, lp);
    };
    let t = s.theme;
    let p = |v| s.px(v);
    match msg {
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let dc = wp as HDC;
            SetTextColor(dc, t.ink);
            SetBkColor(dc, t.elevated);
            SetDCBrushColor(dc, t.elevated);
            GetStockObject(DC_BRUSH) as isize
        }
        WM_DRAWITEM => {
            let item = &*(lp as *const DRAWITEMSTRUCT);
            if item.CtlID != RESULTS as u32 {
                return 0;
            }
            let Some(&index) = s.matches.get(item.itemID as usize) else {
                return 1;
            };
            let c = &COMMANDS[index];
            let r = item.rcItem;
            let selected = item.itemState & ODS_SELECTED != 0;
            let bg = if selected {
                if t.high_contrast {
                    t.focus
                } else {
                    blend(t.focus, t.elevated, 0.2)
                }
            } else {
                t.elevated
            };
            let ink = if selected && t.high_contrast {
                GetSysColor(COLOR_HIGHLIGHTTEXT)
            } else {
                t.ink
            };
            fill(item.hDC, r, bg);
            text(
                item.hDC,
                if selected { "›" } else { "·" },
                rect(r.left + p(10), r.top + p(12), p(ICON_DIP), p(ICON_DIP)),
                s.fonts.strong,
                ink,
                DT_CENTER,
            );
            text(
                item.hDC,
                c.title,
                rect(
                    r.left + p(40),
                    r.top + p(2),
                    r.right - r.left - p(130),
                    p(25),
                ),
                s.fonts.body,
                ink,
                DT_LEFT,
            );
            text(
                item.hDC,
                c.category,
                rect(r.left + p(40), r.top + p(24), p(200), p(18)),
                s.fonts.small,
                if selected && t.high_contrast {
                    ink
                } else {
                    t.secondary
                },
                DT_LEFT,
            );
            if !c.shortcut.is_empty() {
                let badge_width = p(SHORTCUT_DIP.max(52));
                let badge = rect(
                    r.right - badge_width - p(14),
                    r.top + p(10),
                    badge_width,
                    p(24),
                );
                round(item.hDC, badge, t.surface, p(6));
                text(
                    item.hDC,
                    c.shortcut,
                    badge,
                    s.fonts.small,
                    t.secondary,
                    DT_CENTER,
                );
            }
            1
        }
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(h, &mut ps);
            let mut r = zeroed();
            GetClientRect(h, &mut r);
            if h == s.root {
                bitmap(dc, &s.original, s.width, s.height);
            } else if h == s.backdrop {
                if t.high_contrast {
                    fill(dc, r, t.canvas);
                } else {
                    stretch_bitmap(
                        dc,
                        &s.blurred,
                        s.blur_width,
                        s.blur_height,
                        s.width,
                        s.height,
                    );
                }
            } else {
                if t.high_contrast || t.canvas != BG_CANVAS {
                    fill(dc, r, t.surface);
                } else {
                    bitmap(dc, &s.glass, r.right, r.bottom);
                }
                line(dc, r, t.border);
                text(
                    dc,
                    "명령 검색",
                    rect(p(24), p(13), p(280), p(24)),
                    s.fonts.strong,
                    t.ink,
                    DT_LEFT,
                );
                text(
                    dc,
                    "Esc",
                    rect(r.right - p(57), p(14), p(SHORTCUT_DIP), p(24)),
                    s.fonts.small,
                    t.secondary,
                    DT_CENTER,
                );
                line(dc, rect(p(20), p(49), r.right - p(40), p(38)), t.focus);
                if s.matches.is_empty() {
                    text(
                        dc,
                        "일치하는 명령이 없습니다.",
                        rect(p(24), p(112), r.right - p(48), p(44)),
                        s.fonts.body,
                        t.secondary,
                        DT_CENTER,
                    );
                }
                text(
                    dc,
                    "↑ ↓ 선택     Enter 실행     Esc 닫기",
                    rect(p(24), r.bottom - p(33), r.right - p(48), p(22)),
                    s.fonts.small,
                    t.secondary,
                    DT_LEFT,
                );
            }
            EndPaint(h, &ps);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

pub(super) unsafe fn show(owner: HWND) {
    let app = app_ptr(owner);
    if app.is_null() || (*app).borrow().ui.palette_open {
        return;
    }
    let previous_focus = GetFocus();
    let dpi = GetDpiForWindow(owner).max(96);
    let p = |v| v * dpi as i32 / 96;
    let mut client = zeroed();
    GetClientRect(owner, &mut client);
    let width = client.right.max(p(320));
    let height = client.bottom.max(p(300));
    let mut origin = POINT { x: 0, y: 0 };
    ClientToScreen(owner, &mut origin);
    let dark = (*app).borrow().prefs.dark;
    let theme = Theme::new(dark);
    let original = capture(owner, width, height);
    // The blur has no fine detail: cache at quarter resolution and let GDI
    // resample once. This also keeps palette opening responsive in debug builds.
    let blur_width = ((width + 3) / 4).max(1);
    let blur_height = ((height + 3) / 4).max(1);
    let small: Vec<_> = (0..blur_height)
        .flat_map(|y| {
            let original = &original;
            (0..blur_width).map(move |x| {
                original[((y * 4).min(height - 1) * width + (x * 4).min(width - 1)) as usize]
            })
        })
        .collect();
    let mut blurred = if theme.high_contrast {
        vec![0xff000000; small.len()]
    } else {
        blur(
            &small,
            blur_width as usize,
            blur_height as usize,
            (p(BLUR_DIP) / 4).max(1) as usize,
        )
    };
    for pixel in &mut blurred {
        let color = (((*pixel) & 255) << 16) | ((*pixel) & 0xff00) | (((*pixel) >> 16) & 255);
        let dim = blend(theme.canvas, color, 0.64);
        *pixel = 0xff000000 | ((dim & 255) << 16) | (dim & 0xff00) | ((dim >> 16) & 255);
    }
    let panel_width = p(560).min(width - p(32));
    let panel_height = p(428).min(height - p(32));
    let panel_bounds = rect(
        (width - panel_width) / 2,
        (height - panel_height) / 2,
        panel_width,
        panel_height,
    );
    if !theme.high_contrast {
        let shadow = SHADOW_ELEVATION;
        let mut sr = panel_bounds;
        InflateRect(&mut sr, p(shadow.spread), p(shadow.spread));
        OffsetRect(&mut sr, p(shadow.x), p(shadow.y));
        let sigma = (p(shadow.blur) as f32 / 2.0).max(1.0);
        for (i, pixel) in blurred.iter_mut().enumerate() {
            let x = (i as i32 % blur_width) * width / blur_width;
            let y = (i as i32 / blur_width) * height / blur_height;
            let dx = (sr.left - x).max(x - sr.right).max(0) as f32;
            let dy = (sr.top - y).max(y - sr.bottom).max(0) as f32;
            let alpha = shadow.alpha * (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp();
            let color = ((*pixel & 255) << 16) | (*pixel & 0xff00) | ((*pixel >> 16) & 255);
            let shaded = blend(shadow.color, color, alpha);
            *pixel =
                0xff000000 | ((shaded & 255) << 16) | (shaded & 0xff00) | ((shaded >> 16) & 255);
        }
    }
    let glass = (0..panel_height)
        .flat_map(|y| {
            let blurred = &blurred;
            (0..panel_width).map(move |x| {
                let pixel = blurred[(((y + panel_bounds.top) * blur_height / height) * blur_width
                    + (x + panel_bounds.left) * blur_width / width)
                    as usize];
                let color = ((pixel & 255) << 16) | (pixel & 0xff00) | ((pixel >> 16) & 255);
                let tinted = blend(BG_GLASS.0, color, BG_GLASS.1);
                0xff000000 | ((tinted & 255) << 16) | (tinted & 0xff00) | ((tinted >> 16) & 255)
            })
        })
        .collect();
    let state = Box::new(RefCell::new(State {
        owner,
        root: null_mut(),
        panel: null_mut(),
        backdrop: null_mut(),
        search: null_mut(),
        list: null_mut(),
        dpi,
        fonts: Fonts::new(dpi),
        theme,
        matches: vec![],
        accepted: None,
        closing: false,
        composing: false,
        done: false,
        previous_focus,
        width,
        height,
        original,
        blurred,
        blur_width,
        blur_height,
        glass,
    }));
    let raw = &*state as *const RefCell<State>;
    // Mark this before focus moves, so opening/canceling never commits a draft.
    (*app).borrow_mut().ui.palette_open = true;
    let root = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        w("DotCalendarPalette").as_ptr(),
        w("명령 검색 · AI 캘린더").as_ptr(),
        WS_POPUP | WS_CLIPCHILDREN,
        origin.x,
        origin.y,
        width,
        height,
        owner,
        null_mut(),
        GetModuleHandleW(null()),
        raw as _,
    );
    if root.is_null() {
        (*app).borrow_mut().ui.palette_open = false;
        return;
    }
    state.borrow_mut().root = root;
    let backdrop = CreateWindowExW(
        0,
        w("DotCalendarPalette").as_ptr(),
        w("명령 검색 배경").as_ptr(),
        WS_CHILD | WS_VISIBLE,
        0,
        0,
        width,
        height,
        root,
        null_mut(),
        GetModuleHandleW(null()),
        raw as _,
    );
    state.borrow_mut().backdrop = backdrop;
    let panel = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        w("DotCalendarPalette").as_ptr(),
        w("명령 검색 패널").as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
        panel_bounds.left,
        panel_bounds.top,
        panel_width,
        panel_height,
        root,
        null_mut(),
        GetModuleHandleW(null()),
        raw as _,
    );
    state.borrow_mut().panel = panel;
    SetWindowRgn(
        panel,
        CreateRoundRectRgn(0, 0, panel_width + 1, panel_height + 1, p(14), p(14)),
        1,
    );
    let font = state.borrow().fonts.body;
    let caption = control(
        panel,
        "STATIC",
        "명령 검색",
        0,
        104,
        state.borrow().fonts.strong,
    );
    MoveWindow(caption, p(24), p(13), p(280), p(24), 1);
    let search = control(
        panel,
        "EDIT",
        "",
        WS_TABSTOP | ES_AUTOHSCROLL as u32,
        SEARCH,
        font,
    );
    SetWindowSubclass(search, Some(search_proc), 25, raw as usize);
    let list = control(
        panel,
        "LISTBOX",
        "명령 목록",
        WS_TABSTOP
            | WS_VSCROLL
            | LBS_OWNERDRAWFIXED as u32
            | LBS_HASSTRINGS as u32
            | LBS_NOTIFY as u32
            | LBS_NOINTEGRALHEIGHT as u32,
        RESULTS,
        font,
    );
    let close = control(
        panel,
        "BUTTON",
        "닫기",
        WS_TABSTOP | BS_FLAT as u32,
        CLOSE,
        state.borrow().fonts.small,
    );
    {
        let mut s = state.borrow_mut();
        s.search = search;
        s.list = list;
    }
    SendMessageW(
        search,
        EM_SETCUEBANNER,
        1,
        w("명령을 검색하세요…").as_ptr() as isize,
    );
    SendMessageW(search, EM_SETLIMITTEXT, 256, 0);
    MoveWindow(search, p(29), p(57), panel_width - p(58), p(24), 1);
    MoveWindow(
        list,
        p(20),
        p(100),
        panel_width - p(40),
        panel_height - p(144),
        1,
    );
    MoveWindow(close, panel_width - p(110), p(13), p(48), p(25), 1);
    SendMessageW(list, LB_SETITEMHEIGHT, 0, p(46) as isize);
    SetWindowTheme(search, w("").as_ptr(), w("").as_ptr());
    SetWindowTheme(list, w("").as_ptr(), w("").as_ptr());
    update(raw);
    (*app).borrow_mut().active_form = root;
    EnableWindow(owner, 0);
    ShowWindow(root, SW_SHOW);
    SetForegroundWindow(root);
    SetFocus(search);
    animate(root, backdrop, Motion::backdrop(true), Finish::Restore);
    animate(root, panel, Motion::dialog(true), Finish::Restore);
    let signal = (*app).borrow().store.change_event_handle();
    let mut msg = zeroed();
    while !state.borrow().done && IsWindow(root) != 0 && IsWindow(owner) != 0 {
        let wait =
            MsgWaitForMultipleObjectsEx(1, &signal, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        if wait == WAIT_FAILED {
            break;
        }
        if wait == WAIT_OBJECT_0 {
            SendMessageW(owner, RELOAD, 0, 0);
        }
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT {
                PostQuitMessage(msg.wParam as i32);
                state.borrow_mut().done = true;
                break;
            }
            if msg.message == WM_KEYDOWN && !state.borrow().closing {
                let key = msg.wParam as u16;
                if key == VK_ESCAPE || (key == b'K' as u16 && GetKeyState(VK_CONTROL as i32) < 0) {
                    if key != VK_ESCAPE && !palette_shortcut(key, true, msg.lParam & (1 << 30) != 0)
                    {
                        continue;
                    }
                    start_close(raw, false);
                    continue;
                }
                if state.borrow().composing {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                    continue;
                }
                if key == VK_RETURN {
                    start_close(raw, true);
                    continue;
                }
                if key == VK_UP || key == VK_DOWN {
                    let count = state.borrow().matches.len();
                    let current = usize::try_from(SendMessageW(list, LB_GETCURSEL, 0, 0)).ok();
                    if let Some(index) =
                        wrap_selection(current, if key == VK_UP { -1 } else { 1 }, count)
                    {
                        SendMessageW(list, LB_SETCURSEL, index, 0);
                    }
                    continue;
                }
            }
            if !state.borrow().closing && IsDialogMessageW(panel, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    cleanup(root);
    if IsWindow(root) != 0 {
        DestroyWindow(root);
    }
    if IsWindow(owner) == 0 {
        return;
    }
    EnableWindow(owner, 1);
    {
        let mut a = (*app).borrow_mut();
        a.active_form = null_mut();
        a.ui.palette_open = false;
    }
    let accepted = state.borrow().accepted;
    if let Some(index) = accepted {
        let c = &COMMANDS[index];
        if c.id == TODAY {
            super::command(owner, MONTH_TAB);
        }
        if !super::command(owner, c.id) {
            super::super::command(owner, c.id);
        }
        if IsWindow(owner) != 0 {
            notify(owner, &format!("{} 명령을 실행했습니다.", c.title));
        }
    }
}

pub(super) unsafe fn register() -> Result<(), String> {
    let class = w("DotCalendarPalette");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: GetModuleHandleW(null()),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    if RegisterClassW(&wc) == 0 {
        Err("명령 검색 창을 등록하지 못했습니다.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blur_preserves_constant_pixels_and_small_image_boundaries() {
        for (w, h) in [(1, 1), (2, 5), (31, 7)] {
            let input = vec![0xff102030; w * h];
            assert_eq!(blur(&input, w, h, 12), input);
        }
        let input = vec![0xff000000, 0xffffffff, 0xff000000];
        let result = blur(&input, 3, 1, 12);
        assert_eq!(result.len(), 3);
        assert!(result
            .iter()
            .all(|v| v & 0x00ffffff != 0 && v & 0x00ffffff != 0xffffff));
    }
    #[test]
    fn palette_has_no_automatic_destructive_or_remote_action() {
        assert!(COMMANDS
            .iter()
            .all(|c| ![DELETE, RESTORE, DISCONNECT, SYNC, CONNECT, QUIT].contains(&c.id)));
        assert_eq!(
            search_commands(COMMANDS, "showcase")
                .iter()
                .map(|&i| COMMANDS[i].id)
                .collect::<Vec<_>>(),
            vec![SHOWCASE]
        );
    }
}
