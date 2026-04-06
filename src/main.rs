#![windows_subsystem = "windows"]

mod canvas;
mod render;
mod search;
mod thumbnails;

use canvas::CanvasState;
use render::RenderContext;
use search::SearchState;
use thumbnails::{
    enumerate_windows_v2, query_client_area_size, query_source_size, register_thumbnail,
    unregister_thumbnail, update_thumbnail, WindowInfo,
};

use std::cell::RefCell;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT, VK_BACK,
    VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::RECT;

const HOTKEY_ID: i32 = 1;
const PIN_F1_HOTKEY_ID: i32 = 2;
const PIN_ESC_HOTKEY_ID: i32 = 3;
const TIMER_ID: usize = 1;
const ANIM_TIMER_ID: usize = 2;
const ANIM_INTERVAL_MS: u32 = 16;

struct PinFocusState {
    grid_idx: usize,
    target_hwnd: HWND,
    saved_placement: WINDOWPLACEMENT,
    was_topmost: bool,
}

struct DragState {
    grid_idx: usize,
    start_x: i32,
    start_y: i32,
    active: bool,
}

struct AppState {
    canvas: CanvasState,
    search: SearchState,
    render: Option<RenderContext>,
    windows: Vec<WindowInfo>,
    filtered_indices: Vec<usize>,
    selected: Option<usize>,
    hovered: Option<usize>,
    right_click_start: Option<(i32, i32)>,
    did_pan: bool,
    drag: Option<DragState>,
    custom_order: Vec<isize>,
    pin_mode: bool,
    pin_focus: Option<PinFocusState>,
    hwnd: HWND,
    visible: bool,
    qpc_freq: i64,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = RefCell::new(None);
}

fn main() {
    unsafe {
        // Must be the very first Win32 call -- enables physical pixel coordinates everywhere
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).unwrap();

        let hinstance = GetModuleHandleW(None).unwrap();

        let class_name: Vec<u16> = "WinCanvasClass\0".encode_utf16().collect();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(std::ptr::null_mut()),
            ..Default::default()
        };

        RegisterClassExW(&wc);

        let title: Vec<u16> = "WinCanvas\0".encode_utf16().collect();

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            screen_w,
            screen_h,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .unwrap();

        // Register global hotkey: Ctrl+Space
        let _ = RegisterHotKey(
            Some(hwnd),
            HOTKEY_ID,
            MOD_CONTROL | MOD_NOREPEAT,
            VK_SPACE.0 as u32,
        );

        // Timer for re-enumeration every 2 seconds
        let _ = SetTimer(Some(hwnd), TIMER_ID, 2000, None);

        // Initialize app state
        let mut qpc_freq: i64 = 0;
        let _ = QueryPerformanceFrequency(&mut qpc_freq);
        let canvas = CanvasState::new(screen_w as f64, screen_h as f64);
        let search = SearchState::new();
        let render = RenderContext::new(hwnd);

        let mut state = AppState {
            canvas,
            search,
            render: Some(render),
            windows: Vec::new(),
            filtered_indices: Vec::new(),
            selected: None,
            hovered: None,
            right_click_start: None,
            did_pan: false,
            drag: None,
            custom_order: Vec::new(),
            pin_mode: false,
            pin_focus: None,
            hwnd: hwnd,
            visible: true,
            qpc_freq,
        };

        refresh_windows(&mut state);

        APP.with(|app| {
            *app.borrow_mut() = Some(state);
        });

        // Force to foreground and paint
        let _ = SetForegroundWindow(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = InvalidateRect(Some(hwnd), None, false);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn recompute_layout(state: &mut AppState) {
    let sizes: Vec<(i32, i32)> = state.filtered_indices.iter()
        .map(|&i| {
            let w = &state.windows[i];
            if w.client_w > 0 && w.client_h > 0 {
                (w.client_w, w.client_h)
            } else {
                (w.source_w, w.source_h)
            }
        })
        .collect();
    state.canvas.compute_layout(&sizes);
}

fn refresh_windows(state: &mut AppState) {
    // Unregister old thumbnails
    for w in &state.windows {
        if let Some(thumb) = w.thumbnail {
            unregister_thumbnail(thumb);
        }
    }

    // Enumerate new windows
    let mut windows = enumerate_windows_v2(state.hwnd);

    // Sort by z-order (EnumWindows already returns in z-order, most recent first)
    // No extra work needed -- EnumWindows gives us top-to-bottom z-order

    // Register thumbnails and query source/client sizes
    for w in &mut windows {
        w.thumbnail = register_thumbnail(state.hwnd, w.hwnd);
        if let Some(thumb) = w.thumbnail {
            let (sw, sh) = query_source_size(thumb);
            w.source_w = sw;
            w.source_h = sh;
        }
        let (cw, ch) = query_client_area_size(w.hwnd);
        w.client_w = cw;
        w.client_h = ch;
    }

    state.windows = windows;
    update_filter(state);
    recompute_layout(state);
    update_all_thumbnails(state);
}

fn update_filter(state: &mut AppState) {
    state.filtered_indices.clear();
    for (i, w) in state.windows.iter().enumerate() {
        if state.search.matches(&w.title) {
            state.filtered_indices.push(i);
        }
    }
    if !state.custom_order.is_empty() {
        apply_custom_order(state);
    }
}

fn save_custom_order(state: &mut AppState) {
    state.custom_order = state.filtered_indices.iter()
        .map(|&i| state.windows[i].hwnd.0 as isize)
        .collect();
}

fn apply_custom_order(state: &mut AppState) {
    let order = &state.custom_order;
    state.filtered_indices.sort_by_key(|&i| {
        let hwnd_val = state.windows[i].hwnd.0 as isize;
        order.iter().position(|&h| h == hwnd_val).unwrap_or(usize::MAX)
    });
}

fn update_all_thumbnails(state: &AppState) {
    // Hide all thumbnails first (set invisible for non-filtered)
    for (i, w) in state.windows.iter().enumerate() {
        if let Some(thumb) = w.thumbnail {
            if !state.filtered_indices.contains(&i) {
                let hide_rect = RECT {
                    left: -1,
                    top: -1,
                    right: -1,
                    bottom: -1,
                };
                update_thumbnail(thumb, hide_rect, 0, 0, 0);
            }
        }
    }

    // Update visible thumbnails -- flow layout gives aspect-correct rects directly
    for (grid_idx, &win_idx) in state.filtered_indices.iter().enumerate() {
        if grid_idx >= state.canvas.layout.len() {
            break;
        }
        let w = &state.windows[win_idx];
        if let Some(thumb) = w.thumbnail {
            let tr = state.canvas.thumb_rect(grid_idx);
            let (cw, ch) = if w.client_w > 0 && w.client_h > 0 {
                (w.client_w, w.client_h)
            } else {
                (w.source_w, w.source_h)
            };
            let opacity = 255u8;
            update_thumbnail(thumb, tr, cw, ch, opacity);
        }
    }
}

const CMD_CLOSE: u32 = 1001;
const CMD_MINIMIZE: u32 = 1002;
const CMD_MAXIMIZE: u32 = 1003;
const CMD_RESTORE: u32 = 1004;

fn show_context_menu(hwnd: HWND, x: i32, y: i32, win_idx: usize, _state: &AppState) {
    unsafe {
        let menu = CreatePopupMenu().unwrap();
        let close_text: Vec<u16> = "Close\0".encode_utf16().collect();
        let min_text: Vec<u16> = "Minimize\0".encode_utf16().collect();
        let max_text: Vec<u16> = "Maximize\0".encode_utf16().collect();
        let restore_text: Vec<u16> = "Restore\0".encode_utf16().collect();

        let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(0), CMD_RESTORE as usize, PCWSTR(restore_text.as_ptr()));
        let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(0), CMD_MINIMIZE as usize, PCWSTR(min_text.as_ptr()));
        let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(0), CMD_MAXIMIZE as usize, PCWSTR(max_text.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(std::ptr::null()));
        let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(0), CMD_CLOSE as usize, PCWSTR(close_text.as_ptr()));

        // Store the target window index for WM_COMMAND
        CONTEXT_MENU_TARGET.with(|t| *t.borrow_mut() = Some(win_idx));

        // Convert client coords to screen coords
        let mut pt = windows::Win32::Foundation::POINT { x, y };
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);

        let _ = TrackPopupMenu(menu, TRACK_POPUP_MENU_FLAGS(0), pt.x, pt.y, Some(0), hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

thread_local! {
    static CONTEXT_MENU_TARGET: RefCell<Option<usize>> = RefCell::new(None);
}

fn select_and_navigate(state: &mut AppState, idx: Option<usize>, center: bool) {
    state.selected = idx;
    if let Some(i) = idx {
        let mut now: i64 = 0;
        unsafe { let _ = QueryPerformanceCounter(&mut now); }
        if center {
            state.canvas.center_on(i, state.qpc_freq, now);
        } else {
            state.canvas.scroll_into_view(i, state.qpc_freq, now);
        }
        if state.canvas.anim.active {
            unsafe { let _ = SetTimer(Some(state.hwnd), ANIM_TIMER_ID, ANIM_INTERVAL_MS, None); }
        }
        update_all_thumbnails(state);
    }
}

/// Compute the window rect so the target's client area aligns with `client_target` on screen.
fn compute_client_aligned_rect(hwnd: HWND, client_target: &RECT) -> (i32, i32, i32, i32) {
    unsafe {
        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        let mut cp = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(hwnd, &mut cp);
        let mut cr = RECT::default();
        let _ = GetClientRect(hwnd, &mut cr);

        let bl = cp.x - wr.left;
        let bt = cp.y - wr.top;
        let br = wr.right - (cp.x + cr.right);
        let bb = wr.bottom - (cp.y + cr.bottom);

        let x = client_target.left - bl;
        let y = client_target.top - bt;
        let w = (client_target.right - client_target.left) + bl + br;
        let h = (client_target.bottom - client_target.top) + bt + bb;
        (x, y, w, h)
    }
}

fn enter_pin_focus(state: &mut AppState, grid_idx: usize) {
    // Exit any existing pin focus first
    exit_pin_focus(state);

    let win_idx = state.filtered_indices[grid_idx];
    let target = state.windows[win_idx].hwnd;

    // Save original placement
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { let _ = GetWindowPlacement(target, &mut placement); }

    // Check if target is TOPMOST and remove it if so
    let ex_style = unsafe { GetWindowLongW(target, GWL_EXSTYLE) } as u32;
    let was_topmost = ex_style & WS_EX_TOPMOST.0 != 0;
    if was_topmost {
        unsafe {
            let _ = SetWindowPos(
                target, Some(HWND_NOTOPMOST), 0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    // Align target's client area with thumb_rect
    let tr = state.canvas.thumb_rect(grid_idx);
    let (px, py, pw, ph) = compute_client_aligned_rect(target, &tr);

    unsafe {
        let _ = SetWindowPos(
            target, Some(HWND_TOP), px, py, pw, ph,
            SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_FRAMECHANGED,
        );

        // Give keyboard focus to target
        let _ = SetForegroundWindow(target);

        // Register F1 and Escape as global hotkeys for exiting pin focus
        let _ = RegisterHotKey(Some(state.hwnd), PIN_F1_HOTKEY_ID, MOD_NOREPEAT, 0x70);
        let _ = RegisterHotKey(Some(state.hwnd), PIN_ESC_HOTKEY_ID, MOD_NOREPEAT, VK_ESCAPE.0 as u32);
    }

    state.pin_focus = Some(PinFocusState {
        grid_idx,
        target_hwnd: target,
        saved_placement: placement,
        was_topmost,
    });
}

fn exit_pin_focus(state: &mut AppState) {
    if let Some(focus) = state.pin_focus.take() {
        unsafe {
            // Restore original position/size
            if IsWindow(Some(focus.target_hwnd)).as_bool() {
                let _ = SetWindowPlacement(focus.target_hwnd, &focus.saved_placement);
                // Restore TOPMOST if it was originally set
                if focus.was_topmost {
                    let _ = SetWindowPos(
                        focus.target_hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
            // Unregister pin hotkeys
            let _ = UnregisterHotKey(Some(state.hwnd), PIN_F1_HOTKEY_ID);
            let _ = UnregisterHotKey(Some(state.hwnd), PIN_ESC_HOTKEY_ID);
            // Refocus overlay
            let _ = SetForegroundWindow(state.hwnd);
        }
        update_all_thumbnails(state);
    }
}

fn ctrl_held() -> bool {
    unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 }
}

fn clamp_selection(sel: Option<usize>, count: usize) -> Option<usize> {
    match sel {
        Some(_) if count == 0 => None,
        Some(s) if s >= count => Some(count - 1),
        other => other,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            APP.with(|app| {
                if let Some(ref state) = *app.borrow() {
                    if let Some(ref render) = state.render {
                        render.begin_draw();
                        render.draw_search_bar(&state.search.query, state.canvas.canvas_w);

                        // Pin button (always visible)
                        let pin_rect = state.canvas.pin_button_rect();
                        render.draw_pin_button(pin_rect, state.pin_mode);

                        // Draw cell borders, titles, selection, hover, badges
                        for (grid_idx, &win_idx) in state.filtered_indices.iter().enumerate() {
                            if grid_idx >= state.canvas.layout.len() {
                                break;
                            }
                            let cr = state.canvas.cell_rect(grid_idx);
                            let winfo = &state.windows[win_idx];

                            if state.pin_mode && state.pin_focus.as_ref().map(|f| f.grid_idx) == Some(grid_idx) {
                                render.draw_pin_focus_border(cr);
                            } else if state.selected == Some(grid_idx) {
                                render.draw_selection_border(cr);
                            } else if state.hovered == Some(grid_idx) {
                                render.draw_hover_border(cr);
                            } else {
                                render.draw_cell_border(cr);
                            }

                            // Number badges for first 9 windows
                            if grid_idx < 9 {
                                render.draw_number_badge(cr, grid_idx + 1);
                            }

                            // Title below the cell
                            let tr = state.canvas.title_rect(grid_idx);
                            let full = format!("[{}] {}", winfo.process_name, winfo.title);
                            let display_title = if full.len() > 45 {
                                format!("{}...", &full[..42])
                            } else {
                                full
                            };
                            render.draw_title(&display_title, tr);
                        }

                        render.end_draw();
                    }
                }
            });
            let _ = ValidateRect(Some(hwnd), None);
            LRESULT(0)
        }

        WM_MOUSEWHEEL => {
            let mx = (lparam.0 & 0xFFFF) as i16 as i32;
            let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    // Pin focus active: ignore scroll on overlay (target gets it via focus)
                    if state.pin_focus.is_some() {
                        return;
                    }
                    let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
                    state.canvas.anim.active = false;
                    let _ = KillTimer(Some(hwnd), ANIM_TIMER_ID);
                    state.canvas.zoom_at(mx, my, delta);
                    update_all_thumbnails(state);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            });
            LRESULT(0)
        }

        WM_RBUTTONDOWN => {
            let mx = (lparam.0 & 0xFFFF) as i16 as i32;
            let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    // Pin focus active: ignore (right-clicks pass through via HTTRANSPARENT)
                    if state.pin_focus.is_some() {
                        return;
                    }
                    state.canvas.anim.active = false;
                    let _ = KillTimer(Some(hwnd), ANIM_TIMER_ID);
                    state.canvas.is_panning = true;
                    state.canvas.last_mouse_x = mx;
                    state.canvas.last_mouse_y = my;
                    state.right_click_start = Some((mx, my));
                    state.did_pan = false;
                }
            });
            LRESULT(0)
        }

        WM_RBUTTONUP => {
            let mx = (lparam.0 & 0xFFFF) as i16 as i32;
            let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    if state.pin_focus.is_some() {
                        return;
                    }
                    state.canvas.is_panning = false;
                    if !state.did_pan && !state.pin_mode {
                        let count = state.filtered_indices.len();
                        if let Some(grid_idx) = state.canvas.hit_test(mx, my, count) {
                            let win_idx = state.filtered_indices[grid_idx];
                            show_context_menu(hwnd, mx, my, win_idx, state);
                        }
                    }
                    state.right_click_start = None;
                }
            });
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            let mx = (lparam.0 & 0xFFFF) as i16 as i32;
            let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    if state.canvas.is_panning {
                        let dx = mx - state.canvas.last_mouse_x;
                        let dy = my - state.canvas.last_mouse_y;
                        if dx.abs() > 3 || dy.abs() > 3 {
                            state.did_pan = true;
                        }
                        state.canvas.pan(dx, dy);
                        state.canvas.last_mouse_x = mx;
                        state.canvas.last_mouse_y = my;
                        update_all_thumbnails(state);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    } else if state.pin_focus.is_some() {
                        // Pin focus active: mouse moves pass through via HTTRANSPARENT
                    } else if state.drag.is_some() {
                        let drag = state.drag.as_mut().unwrap();
                        if !drag.active {
                            let dx = mx - drag.start_x;
                            let dy = my - drag.start_y;
                            if dx.abs() > 5 || dy.abs() > 5 {
                                drag.active = true;
                            }
                        }
                        if drag.active {
                            let count = state.filtered_indices.len();
                            let new_hover = state.canvas.hit_test(mx, my, count);
                            if new_hover != state.hovered {
                                state.hovered = new_hover;
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        }
                    } else {
                        // Hover tracking
                        let count = state.filtered_indices.len();
                        let new_hover = state.canvas.hit_test(mx, my, count);
                        if new_hover != state.hovered {
                            state.hovered = new_hover;
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                }
            });
            LRESULT(0)
        }

        WM_MBUTTONDOWN => {
            // Middle-click: close the target window
            let mx = (lparam.0 & 0xFFFF) as i16 as i32;
            let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    let count = state.filtered_indices.len();
                    if let Some(grid_idx) = state.canvas.hit_test(mx, my, count) {
                        let win_idx = state.filtered_indices[grid_idx];
                        let target_hwnd = state.windows[win_idx].hwnd;
                        let _ = PostMessageW(Some(target_hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                        // Refresh after a short delay (timer will catch it)
                    }
                }
            });
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            let mx = (lparam.0 & 0xFFFF) as i16 as i32;
            let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    // Check pin button click first
                    let pin_r = state.canvas.pin_button_rect();
                    if mx >= pin_r.left && mx <= pin_r.right && my >= pin_r.top && my <= pin_r.bottom {
                        exit_pin_focus(state);
                        state.pin_mode = !state.pin_mode;
                        update_all_thumbnails(state);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        return;
                    }

                    let count = state.filtered_indices.len();

                    if state.pin_mode {
                        // Clicks on focused window's thumb_rect never reach here
                        // (HTTRANSPARENT passes them to the real window).
                        // Clicks outside go here -- focus a new window or unfocus.
                        if let Some(grid_idx) = state.canvas.hit_test(mx, my, count) {
                            let already_focused = state.pin_focus.as_ref().map(|f| f.grid_idx) == Some(grid_idx);
                            if !already_focused {
                                enter_pin_focus(state, grid_idx);
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        } else {
                            exit_pin_focus(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        return;
                    }

                    // Normal mode
                    if let Some(grid_idx) = state.canvas.hit_test(mx, my, count) {
                        if ctrl_held() {
                            // Ctrl+click: start potential drag
                            state.selected = Some(grid_idx);
                            state.drag = Some(DragState {
                                grid_idx,
                                start_x: mx,
                                start_y: my,
                                active: false,
                            });
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        } else {
                            // Plain click: activate the target window
                            let win_idx = state.filtered_indices[grid_idx];
                            let target_hwnd = state.windows[win_idx].hwnd;
                            let _ = ShowWindow(hwnd, SW_HIDE);
                            state.visible = false;
                            let _ = SetForegroundWindow(target_hwnd);
                        }
                    }
                }
            });
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            let mx = (lparam.0 & 0xFFFF) as i16 as i32;
            let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    // Pin focus: clicks pass through via HTTRANSPARENT
                    if state.pin_focus.is_some() {
                        return;
                    }
                    if let Some(drag) = state.drag.take() {
                        if drag.active {
                            let count = state.filtered_indices.len();
                            if let Some(drop_idx) = state.canvas.drop_index(mx, my, count, drag.grid_idx) {
                                if drop_idx != drag.grid_idx {
                                    let removed = state.filtered_indices.remove(drag.grid_idx);
                                    let insert_at = if drop_idx > drag.grid_idx { drop_idx - 1 } else { drop_idx };
                                    state.filtered_indices.insert(insert_at, removed);
                                    state.selected = Some(insert_at);
                                    save_custom_order(state);
                                    recompute_layout(state);
                                    update_all_thumbnails(state);
                                }
                            }
                        }
                        state.hovered = None;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
            });
            LRESULT(0)
        }

        WM_CHAR => {
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    // Pin focus: keyboard goes to target via SetForegroundWindow
                    if state.pin_focus.is_some() {
                        return;
                    }
                    // Normal: search input
                    let ch = char::from_u32(wparam.0 as u32);
                    if let Some(c) = ch {
                        if c >= ' ' && c != '\x7f' {
                            state.search.push(c);
                            update_filter(state);
                            recompute_layout(state);
                            update_all_thumbnails(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                }
            });
            LRESULT(0)
        }

        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            // F1: toggle pin mode (only when overlay has focus, i.e. not in pin focus)
            if vk == 0x70 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        exit_pin_focus(state);
                        state.pin_mode = !state.pin_mode;
                        update_all_thumbnails(state);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                });
            } else if vk == VK_ESCAPE.0 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        if state.pin_mode {
                            exit_pin_focus(state);
                            state.pin_mode = false;
                            update_all_thumbnails(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        } else if state.search.is_active() {
                            state.search.clear();
                            update_filter(state);
                            recompute_layout(state);
                            state.selected = clamp_selection(state.selected, state.filtered_indices.len());
                            update_all_thumbnails(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        } else {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                            state.visible = false;
                        }
                    }
                });
            } else if vk == VK_BACK.0 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        if state.pin_focus.is_some() {
                            return;
                        }
                        state.search.pop();
                        update_filter(state);
                        recompute_layout(state);
                        state.selected = clamp_selection(state.selected, state.filtered_indices.len());
                        update_all_thumbnails(state);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                });
            } else {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        // Pin focus: keyboard goes to target naturally
                        if state.pin_focus.is_some() {
                            return;
                        }
                        if vk == VK_RETURN.0 {
                            if let Some(sel) = state.selected {
                                if sel < state.filtered_indices.len() {
                                    let win_idx = state.filtered_indices[sel];
                                    let target_hwnd = state.windows[win_idx].hwnd;
                                    let _ = ShowWindow(hwnd, SW_HIDE);
                                    state.visible = false;
                                    let _ = SetForegroundWindow(target_hwnd);
                                }
                            }
                        } else if vk == VK_TAB.0 || vk == VK_RIGHT.0 {
                            let count = state.filtered_indices.len();
                            if count > 0 {
                                let idx = match state.selected {
                                    Some(s) if s + 1 < count => s + 1,
                                    Some(_) => 0,
                                    None => 0,
                                };
                                select_and_navigate(state, Some(idx), ctrl_held());
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        } else if vk == VK_LEFT.0 {
                            let count = state.filtered_indices.len();
                            if count > 0 {
                                let idx = match state.selected {
                                    Some(0) => count - 1,
                                    Some(s) => s - 1,
                                    None => 0,
                                };
                                select_and_navigate(state, Some(idx), ctrl_held());
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        } else if vk == VK_DOWN.0 {
                            let count = state.filtered_indices.len();
                            if count > 0 {
                                let idx = match state.selected {
                                    Some(s) => state.canvas.nav_down(s),
                                    None => 0,
                                };
                                select_and_navigate(state, Some(idx), ctrl_held());
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        } else if vk == VK_UP.0 {
                            let count = state.filtered_indices.len();
                            if count > 0 {
                                let idx = match state.selected {
                                    Some(s) => state.canvas.nav_up(s),
                                    None => 0,
                                };
                                select_and_navigate(state, Some(idx), ctrl_held());
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        } else if vk >= 0x31 && vk <= 0x39 && !state.search.is_active() {
                            let num = (vk - 0x30) as usize;
                            let idx = num - 1;
                            if idx < state.filtered_indices.len() {
                                let win_idx = state.filtered_indices[idx];
                                let target_hwnd = state.windows[win_idx].hwnd;
                                let _ = ShowWindow(hwnd, SW_HIDE);
                                state.visible = false;
                                let _ = SetForegroundWindow(target_hwnd);
                            }
                        }
                    }
                });
            }
            LRESULT(0)
        }

        WM_HOTKEY => {
            let id = wparam.0 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    match id {
                        HOTKEY_ID => {
                            // Ctrl+Space: toggle overlay visibility
                            if state.visible {
                                exit_pin_focus(state);
                                let _ = ShowWindow(hwnd, SW_HIDE);
                                state.visible = false;
                            } else {
                                let _ = ShowWindow(hwnd, SW_SHOW);
                                let _ = SetForegroundWindow(hwnd);
                                state.visible = true;
                                state.selected = None;
                                state.hovered = None;
                                state.pin_mode = false;
                                state.search.clear();
                                refresh_windows(state);
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        }
                        PIN_F1_HOTKEY_ID | PIN_ESC_HOTKEY_ID => {
                            // F1 or Escape while pin-focused: exit pin focus
                            exit_pin_focus(state);
                            if id == PIN_ESC_HOTKEY_ID {
                                state.pin_mode = false;
                            }
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        _ => {}
                    }
                }
            });
            LRESULT(0)
        }

        WM_KEYUP => {
            // Keyboard goes to target via SetForegroundWindow during pin focus
            LRESULT(0)
        }

        WM_TIMER => {
            if wparam.0 == TIMER_ID {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        if state.pin_focus.is_some() {
                            // Don't refresh while pin-focused (it destroys all thumbnails).
                            // Just validate the target is still alive.
                            let dead = {
                                let f = state.pin_focus.as_ref().unwrap();
                                !IsWindow(Some(f.target_hwnd)).as_bool()
                            };
                            if dead {
                                exit_pin_focus(state);
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        } else if state.visible {
                            refresh_windows(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                });
            } else if wparam.0 == ANIM_TIMER_ID {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        let mut now: i64 = 0;
                        let _ = QueryPerformanceCounter(&mut now);
                        let still_going = state.canvas.tick_animation(now);
                        update_all_thumbnails(state);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        if !still_going {
                            let _ = KillTimer(Some(hwnd), ANIM_TIMER_ID);
                        }
                    }
                });
            }
            LRESULT(0)
        }

        WM_SIZE => {
            let w = (lparam.0 & 0xFFFF) as u16 as u32;
            let h = ((lparam.0 >> 16) & 0xFFFF) as u16 as u32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    state.canvas.canvas_w = w as f64;
                    state.canvas.canvas_h = h as f64;
                    if let Some(ref mut render) = state.render {
                        render.resize(w, h);
                    }
                    recompute_layout(state);
                    update_all_thumbnails(state);
                }
            });
            LRESULT(0)
        }

        WM_COMMAND => {
            let cmd = (wparam.0 & 0xFFFF) as u32;
            CONTEXT_MENU_TARGET.with(|t| {
                if let Some(win_idx) = *t.borrow() {
                    APP.with(|app| {
                        if let Some(ref state) = *app.borrow() {
                            if win_idx < state.windows.len() {
                                let target = state.windows[win_idx].hwnd;
                                match cmd {
                                    CMD_CLOSE => { let _ = PostMessageW(Some(target), WM_CLOSE, WPARAM(0), LPARAM(0)); }
                                    CMD_MINIMIZE => { let _ = ShowWindow(target, SW_MINIMIZE); }
                                    CMD_MAXIMIZE => { let _ = ShowWindow(target, SW_MAXIMIZE); }
                                    CMD_RESTORE => { let _ = ShowWindow(target, SW_RESTORE); }
                                    _ => {}
                                }
                            }
                        }
                    });
                }
            });
            LRESULT(0)
        }

        WM_NCHITTEST => {
            // Pin focus: return HTTRANSPARENT over the focused thumbnail region
            // so real hardware clicks pass through to the target window behind.
            let transparent = APP.with(|app| {
                let state = app.borrow();
                if let Some(ref state) = *state {
                    if let Some(ref focus) = state.pin_focus {
                        let x = (lparam.0 & 0xFFFF) as i16 as i32;
                        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                        let tr = state.canvas.thumb_rect(focus.grid_idx);
                        if x >= tr.left && x < tr.right && y >= tr.top && y < tr.bottom {
                            return true;
                        }
                    }
                }
                false
            });
            if transparent {
                return LRESULT(-1); // HTTRANSPARENT
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
