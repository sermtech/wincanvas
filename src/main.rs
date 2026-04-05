#![windows_subsystem = "windows"]

mod canvas;
mod render;
mod search;
mod thumbnails;

use canvas::{aspect_thumb_rect, CanvasState, TITLE_H};
use render::RenderContext;
use search::SearchState;
use thumbnails::{
    enumerate_windows_v2, query_source_size, register_thumbnail, unregister_thumbnail,
    update_thumbnail, WindowInfo,
};

use std::cell::RefCell;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, RegisterHotKey, MOD_CONTROL, MOD_NOREPEAT, VK_BACK, VK_CONTROL, VK_DOWN,
    VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::RECT;

const HOTKEY_ID: i32 = 1;
const TIMER_ID: usize = 1;
const ANIM_TIMER_ID: usize = 2;
const ANIM_INTERVAL_MS: u32 = 16;

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
    hwnd: HWND,
    visible: bool,
    qpc_freq: i64,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = RefCell::new(None);
}

fn main() {
    unsafe {
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

    // Register thumbnails and query source sizes
    for w in &mut windows {
        w.thumbnail = register_thumbnail(state.hwnd, w.hwnd);
        if let Some(thumb) = w.thumbnail {
            let (sw, sh) = query_source_size(thumb);
            w.source_w = sw;
            w.source_h = sh;
        }
    }

    state.windows = windows;
    update_filter(state);
    update_all_thumbnails(state);
}

fn update_filter(state: &mut AppState) {
    state.filtered_indices.clear();
    for (i, w) in state.windows.iter().enumerate() {
        if state.search.matches(&w.title) {
            state.filtered_indices.push(i);
        }
    }
}

fn update_all_thumbnails(state: &AppState) {
    // Hide all thumbnails first (set invisible for non-filtered)
    for (i, w) in state.windows.iter().enumerate() {
        if let Some(thumb) = w.thumbnail {
            if !state.filtered_indices.contains(&i) {
                // Hide this thumbnail by setting a zero-size rect off-screen
                let hide_rect = RECT {
                    left: -1,
                    top: -1,
                    right: -1,
                    bottom: -1,
                };
                update_thumbnail(thumb, hide_rect);
            }
        }
    }

    // Update visible thumbnails with aspect-correct rects
    for (grid_idx, &win_idx) in state.filtered_indices.iter().enumerate() {
        let cell_rect = state.canvas.grid_rect(grid_idx);
        let w = &state.windows[win_idx];
        if let Some(thumb) = w.thumbnail {
            let thumb_rect = aspect_thumb_rect(cell_rect, w.source_w, w.source_h);
            update_thumbnail(thumb, thumb_rect);
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

fn state_search_active() -> bool {
    APP.with(|app| {
        if let Some(ref state) = *app.borrow() {
            state.search.is_active()
        } else {
            false
        }
    })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            APP.with(|app| {
                if let Some(ref state) = *app.borrow() {
                    if let Some(ref render) = state.render {
                        render.begin_draw();
                        render.draw_search_bar(&state.search.query, state.canvas.canvas_w);

                        // Draw cell borders, titles, selection, hover, badges
                        for (grid_idx, &win_idx) in state.filtered_indices.iter().enumerate() {
                            let cell_rect = state.canvas.grid_rect(grid_idx);
                            let winfo = &state.windows[win_idx];
                            let thumb_rect = aspect_thumb_rect(cell_rect, winfo.source_w, winfo.source_h);

                            if state.selected == Some(grid_idx) {
                                render.draw_selection_border(thumb_rect);
                            } else if state.hovered == Some(grid_idx) {
                                render.draw_hover_border(thumb_rect);
                            } else {
                                render.draw_cell_border(thumb_rect);
                            }

                            // Number badges for first 9 windows
                            if grid_idx < 9 {
                                render.draw_number_badge(thumb_rect, grid_idx + 1);
                            }

                            // Title below the aspect-correct thumbnail
                            let title_h = (TITLE_H * state.canvas.zoom) as i32;
                            let title_rect = RECT {
                                left: thumb_rect.left,
                                top: thumb_rect.bottom + 2,
                                right: thumb_rect.right,
                                bottom: thumb_rect.bottom + 2 + title_h,
                            };
                            let full = format!("[{}] {}", winfo.process_name, winfo.title);
                            let display_title = if full.len() > 45 {
                                format!("{}...", &full[..42])
                            } else {
                                full
                            };
                            render.draw_title(&display_title, title_rect);
                        }

                        render.end_draw();
                    }
                }
            });
            let _ = ValidateRect(Some(hwnd), None);
            LRESULT(0)
        }

        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            let mx = (lparam.0 & 0xFFFF) as i16 as i32;
            let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
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
                    state.canvas.is_panning = false;
                    // If we didn't pan (just a click), show context menu
                    if !state.did_pan {
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
                    let count = state.filtered_indices.len();
                    if let Some(grid_idx) = state.canvas.hit_test(mx, my, count) {
                        let win_idx = state.filtered_indices[grid_idx];
                        let target_hwnd = state.windows[win_idx].hwnd;
                        // Hide our window and activate the target
                        let _ = ShowWindow(hwnd, SW_HIDE);
                        state.visible = false;
                        let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(target_hwnd);
                    }
                }
            });
            LRESULT(0)
        }

        WM_CHAR => {
            let ch = char::from_u32(wparam.0 as u32);
            if let Some(c) = ch {
                if c >= ' ' && c != '\x7f' {
                    APP.with(|app| {
                        if let Some(ref mut state) = *app.borrow_mut() {
                            state.search.push(c);
                            update_filter(state);
                            update_all_thumbnails(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    });
                }
            }
            LRESULT(0)
        }

        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            if vk == VK_BACK.0 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        state.search.pop();
                        update_filter(state);
                        state.selected = clamp_selection(state.selected, state.filtered_indices.len());
                        update_all_thumbnails(state);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                });
            } else if vk == VK_ESCAPE.0 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        if state.search.is_active() {
                            state.search.clear();
                            update_filter(state);
                            state.selected = clamp_selection(state.selected, state.filtered_indices.len());
                            update_all_thumbnails(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        } else {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                            state.visible = false;
                        }
                    }
                });
            } else if vk == VK_RETURN.0 {
                // Enter: activate selected window
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        if let Some(sel) = state.selected {
                            if sel < state.filtered_indices.len() {
                                let win_idx = state.filtered_indices[sel];
                                let target_hwnd = state.windows[win_idx].hwnd;
                                let _ = ShowWindow(hwnd, SW_HIDE);
                                state.visible = false;
                                let _ = SetForegroundWindow(target_hwnd);
                            }
                        }
                    }
                });
            } else if vk == VK_TAB.0 || vk == VK_RIGHT.0 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
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
                    }
                });
            } else if vk == VK_LEFT.0 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
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
                    }
                });
            } else if vk == VK_DOWN.0 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        let count = state.filtered_indices.len();
                        let cols = state.canvas.cols();
                        if count > 0 {
                            let idx = match state.selected {
                                Some(s) => {
                                    let next = s + cols;
                                    if next < count { next } else { s % cols }
                                }
                                None => 0,
                            };
                            select_and_navigate(state, Some(idx), ctrl_held());
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                });
            } else if vk == VK_UP.0 {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        let count = state.filtered_indices.len();
                        let cols = state.canvas.cols();
                        if count > 0 {
                            let idx = match state.selected {
                                Some(s) if s >= cols => s - cols,
                                Some(s) => {
                                    let last_row_start = (count / cols) * cols;
                                    let target = last_row_start + s;
                                    if target < count { target } else if target >= cols { target - cols } else { s }
                                }
                                None => 0,
                            };
                            select_and_navigate(state, Some(idx), ctrl_held());
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                });
            } else if vk >= 0x31 && vk <= 0x39 && !state_search_active() {
                // Number keys 1-9: instant switch (only when not searching)
                let num = (vk - 0x30) as usize;
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        let idx = num - 1;
                        if idx < state.filtered_indices.len() {
                            let win_idx = state.filtered_indices[idx];
                            let target_hwnd = state.windows[win_idx].hwnd;
                            let _ = ShowWindow(hwnd, SW_HIDE);
                            state.visible = false;
                            let _ = SetForegroundWindow(target_hwnd);
                        }
                    }
                });
            }
            LRESULT(0)
        }

        WM_HOTKEY => {
            if wparam.0 as i32 == HOTKEY_ID {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        if state.visible {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                            state.visible = false;
                        } else {
                            let _ = ShowWindow(hwnd, SW_SHOW);
                            let _ = SetForegroundWindow(hwnd);
                            state.visible = true;
                            state.selected = None;
                            state.hovered = None;
                            state.search.clear();
                            refresh_windows(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                });
            }
            LRESULT(0)
        }

        WM_TIMER => {
            if wparam.0 == TIMER_ID {
                APP.with(|app| {
                    if let Some(ref mut state) = *app.borrow_mut() {
                        if state.visible {
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

        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
