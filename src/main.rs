#![windows_subsystem = "windows"]

mod canvas;
mod render;
mod search;
mod thumbnails;

use canvas::CanvasState;
use render::RenderContext;
use search::SearchState;
use thumbnails::{
    enumerate_windows_v2, register_thumbnail, unregister_thumbnail, update_thumbnail, WindowInfo,
};

use std::cell::RefCell;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, MOD_CONTROL, MOD_NOREPEAT, VK_BACK, VK_ESCAPE, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::RECT;

const HOTKEY_ID: i32 = 1;
const TIMER_ID: usize = 1;
struct AppState {
    canvas: CanvasState,
    search: SearchState,
    render: Option<RenderContext>,
    windows: Vec<WindowInfo>,
    filtered_indices: Vec<usize>,
    hwnd: HWND,
    visible: bool,
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
        let canvas = CanvasState::new(screen_w as f64, screen_h as f64);
        let search = SearchState::new();
        let render = RenderContext::new(hwnd);

        let mut state = AppState {
            canvas,
            search,
            render: Some(render),
            windows: Vec::new(),
            filtered_indices: Vec::new(),
            hwnd: hwnd,
            visible: true,
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

    // Register thumbnails
    for w in &mut windows {
        w.thumbnail = register_thumbnail(state.hwnd, w.hwnd);
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

    // Update visible thumbnails
    for (grid_idx, &win_idx) in state.filtered_indices.iter().enumerate() {
        let rect = state.canvas.grid_rect(grid_idx);
        if let Some(thumb) = state.windows[win_idx].thumbnail {
            update_thumbnail(thumb, rect);
        }
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

                        // Draw cell borders and titles
                        for (grid_idx, &win_idx) in state.filtered_indices.iter().enumerate() {
                            let thumb_rect = state.canvas.grid_rect(grid_idx);
                            render.draw_cell_border(thumb_rect);

                            let title_rect = state.canvas.title_rect(grid_idx);
                            let title = &state.windows[win_idx].title;
                            // Truncate title for display
                            let display_title = if title.len() > 40 {
                                format!("{}...", &title[..37])
                            } else {
                                title.clone()
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
                    state.canvas.zoom_at(mx, my, delta);
                    update_all_thumbnails(state);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            });
            LRESULT(0)
        }

        WM_RBUTTONDOWN => {
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    state.canvas.is_panning = true;
                    state.canvas.last_mouse_x = (lparam.0 & 0xFFFF) as i16 as i32;
                    state.canvas.last_mouse_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                }
            });
            LRESULT(0)
        }

        WM_RBUTTONUP => {
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    state.canvas.is_panning = false;
                }
            });
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            APP.with(|app| {
                if let Some(ref mut state) = *app.borrow_mut() {
                    if state.canvas.is_panning {
                        let mx = (lparam.0 & 0xFFFF) as i16 as i32;
                        let my = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                        let dx = mx - state.canvas.last_mouse_x;
                        let dy = my - state.canvas.last_mouse_y;
                        state.canvas.pan(dx, dy);
                        state.canvas.last_mouse_x = mx;
                        state.canvas.last_mouse_y = my;
                        update_all_thumbnails(state);
                        let _ = InvalidateRect(Some(hwnd), None, false);
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
                            update_all_thumbnails(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        } else {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                            state.visible = false;
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

        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
