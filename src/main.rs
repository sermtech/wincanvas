#![windows_subsystem = "windows"]

mod canvas;
mod render;
mod search;
mod thumbnails;

use canvas::CanvasState;
use render::RenderContext;
use search::SearchState;
use thumbnails::{
    enumerate_windows_v2, register_and_measure_thumbnail, unregister_thumbnail, update_thumbnail,
    WindowInfo,
};

use std::cell::RefCell;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, InvalidateRect, SetWindowRgn,
    ValidateRect, RGN_DIFF,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT, VK_BACK,
    VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F1, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB,
    VK_UP,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
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
    is_cloaked: bool,
    /// If we switched desktops to reach the target, save the original desktop GUID.
    saved_desktop: Option<windows::core::GUID>,
    saved_zoom: f64,
    saved_pan_x: f64,
    saved_pan_y: f64,
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
    pin_zoom_pending: bool,
    hwnd: HWND,
    visible: bool,
    qpc_freq: i64,
    vdm: Option<IVirtualDesktopManager>,
    last_desktop_id: Option<windows::core::GUID>,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = RefCell::new(None);
}

fn main() {
    unsafe {
        // Must be the very first Win32 call -- enables physical pixel coordinates everywhere
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).unwrap();

        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );

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
            pin_zoom_pending: false,
            hwnd: hwnd,
            visible: true,
            qpc_freq,
            vdm: windows::Win32::System::Com::CoCreateInstance(
                &VirtualDesktopManager, None, windows::Win32::System::Com::CLSCTX_ALL,
            ).ok(),
            last_desktop_id: None,
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

    // Register thumbnails and query source/client sizes (including cross-desktop windows)
    for w in &mut windows {
        register_and_measure_thumbnail(state.hwnd, w);
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

/// Punch a rectangular hole in the overlay's window region so clicks reach the window behind.
fn apply_region_hole(hwnd: HWND, hole: &RECT) {
    unsafe {
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let full = CreateRectRgn(0, 0, sw, sh);
        let cut = CreateRectRgn(hole.left, hole.top, hole.right, hole.bottom);
        CombineRgn(Some(full), Some(full), Some(cut), RGN_DIFF);
        SetWindowRgn(hwnd, Some(full), true);
        // SetWindowRgn takes ownership of `full`; only delete `cut`
        let _ = DeleteObject(cut.into());
    }
}

/// Remove the window region, restoring the full overlay surface.
fn clear_region_hole(hwnd: HWND) {
    unsafe {
        SetWindowRgn(hwnd, None, true);
    }
}

/// After the zoom-in animation completes, position the real window and punch the hole.
fn apply_pin_hole(state: &mut AppState) {
    if let Some(ref focus) = state.pin_focus {
        let grid_idx = focus.grid_idx;
        let target = focus.target_hwnd;

        // Guard: target may have been closed during the animation
        if !unsafe { IsWindow(Some(target)).as_bool() } {
            let hwnd = state.hwnd;
            state.pin_focus = None;
            clear_region_hole(hwnd);
            update_all_thumbnails(state);
            return;
        }

        // Guard: layout may have changed (belt-and-suspenders)
        if grid_idx >= state.canvas.layout.len() || grid_idx >= state.filtered_indices.len() {
            return;
        }

        // Guard: verify index still maps to the same window after potential list rebuild
        let win_idx = state.filtered_indices[grid_idx];
        if win_idx >= state.windows.len() || state.windows[win_idx].hwnd != target {
            return;
        }

        let tr = state.canvas.thumb_rect(grid_idx);

        if focus.is_cloaked {
            // Cross-desktop window: keep the DWM thumbnail visible as a live preview.
            // Do NOT call SetWindowPos/SetForegroundWindow (would switch desktops).
            // No hole punch needed -- the thumbnail stays on the overlay.
            return;
        }

        // Current-desktop window: swap real window in behind the hole
        let (px, py, pw, ph) = compute_client_aligned_rect(target, &tr);

        unsafe {
            let _ = SetWindowPos(target, Some(HWND_TOP), px, py, pw, ph,
                SWP_NOACTIVATE | SWP_FRAMECHANGED);

            // DPI correction: if the client area landed at the wrong position, nudge
            let mut actual_cp = POINT { x: 0, y: 0 };
            let _ = ClientToScreen(target, &mut actual_cp);
            let dx = tr.left - actual_cp.x;
            let dy = tr.top - actual_cp.y;
            if dx != 0 || dy != 0 {
                let _ = SetWindowPos(target, Some(HWND_TOP), px + dx, py + dy, pw, ph,
                    SWP_NOACTIVATE | SWP_FRAMECHANGED);
            }

            let _ = SetForegroundWindow(target);
        }

        apply_region_hole(state.hwnd, &tr);
        if let Some(thumb) = state.windows[win_idx].thumbnail {
            let hide = RECT { left: -1, top: -1, right: -1, bottom: -1 };
            update_thumbnail(thumb, hide, 0, 0, 0);
        }
    }
}

/// Lightweight repositioning of the pinned window + hole during pan (no debug logging).
fn update_pin_position(state: &mut AppState) {
    if let Some(ref focus) = state.pin_focus {
        if focus.is_cloaked {
            return; // Thumbnail-only preview; no hole or window repositioning
        }
        let grid_idx = focus.grid_idx;
        let target = focus.target_hwnd;
        if grid_idx >= state.canvas.layout.len() || grid_idx >= state.filtered_indices.len() {
            return;
        }
        let tr = state.canvas.thumb_rect(grid_idx);
        let (px, py, pw, ph) = compute_client_aligned_rect(target, &tr);
        unsafe {
            let _ = SetWindowPos(target, Some(HWND_TOP), px, py, pw, ph, SWP_NOACTIVATE);
        }
        apply_region_hole(state.hwnd, &tr);
    }
}

/// Get the current virtual desktop GUID by probing non-cloaked windows from our list.
/// Shell_TrayWnd is pinned to ALL desktops so GetWindowDesktopId returns "Element not found".
/// Instead, find a regular non-cloaked window and query its desktop GUID.
/// Falls back to `cached` (last known GUID) when all windows are cloaked (e.g. empty desktop).
fn get_current_desktop_id(
    vdm: &IVirtualDesktopManager,
    windows: &[WindowInfo],
    cached: &mut Option<windows::core::GUID>,
) -> Option<windows::core::GUID> {
    unsafe {
        // Try non-cloaked windows from our enumerated list
        for w in windows {
            if w.cloaked {
                continue;
            }
            // Check IsWindowOnCurrentVirtualDesktop first as a sanity check
            match vdm.IsWindowOnCurrentVirtualDesktop(w.hwnd) {
                Ok(on_current) if on_current.as_bool() => {
                    if let Ok(guid) = vdm.GetWindowDesktopId(w.hwnd) {
                        if guid != windows::core::GUID::zeroed() {
                            *cached = Some(guid);
                            return Some(guid);
                        }
                    }
                }
                _ => {}
            }
        }
        // Fallback: try any non-cloaked window without the IsWindowOnCurrentVirtualDesktop check
        for w in windows {
            if w.cloaked {
                continue;
            }
            if let Ok(guid) = vdm.GetWindowDesktopId(w.hwnd) {
                if guid != windows::core::GUID::zeroed() {
                    *cached = Some(guid);
                    return Some(guid);
                }
            }
        }
        // All windows cloaked or no windows -- use last known desktop
        *cached
    }
}

/// Move the canvas to whichever virtual desktop the user is currently on.
fn ensure_canvas_on_current_desktop(state: &mut AppState) {
    if let Some(ref vdm) = state.vdm {
        if let Some(id) = get_current_desktop_id(vdm, &state.windows, &mut state.last_desktop_id) {
            unsafe {
                let _ = vdm.MoveWindowToDesktop(state.hwnd, &id);
            }
        }
    }
}

/// Move our canvas to the target window's virtual desktop so the system switches there.
/// Used for non-pin activation of cross-desktop windows (click, Enter, number keys).
/// The overlay is hidden immediately after, so we don't need to switch back.
fn activate_cross_desktop(state: &mut AppState, target: HWND) {
    if let Some(ref vdm) = state.vdm {
        if let Ok(target_desktop) = unsafe { vdm.GetWindowDesktopId(target) } {
            unsafe {
                let _ = vdm.MoveWindowToDesktop(state.hwnd, &target_desktop);
            }
        }
    }
}

fn enter_pin_focus(state: &mut AppState, grid_idx: usize) {
    // Exit any existing pin focus first (clears hole and animates back if needed)
    exit_pin_focus(state);

    // Cancel any in-flight animation or inertia so they don't fight the zoom-in
    state.canvas.stop_inertia();
    state.canvas.anim.active = false;

    let win_idx = state.filtered_indices[grid_idx];
    let target = state.windows[win_idx].hwnd;

    // Save original placement
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { let _ = GetWindowPlacement(target, &mut placement); }

    let mut is_cloaked = state.windows[win_idx].cloaked;
    let mut saved_desktop: Option<windows::core::GUID> = None;

    // For cloaked (cross-desktop) windows: try to bring the window to this desktop,
    // or switch our canvas to the target's desktop if the API denies cross-process moves.
    if is_cloaked {
        if let Some(ref vdm) = state.vdm {
            let our_desktop = get_current_desktop_id(vdm, &state.windows, &mut state.last_desktop_id);
            let mut uncloaked = false;

            // Plan A: move target to our desktop (works for own-process windows)
            if let Some(our_id) = our_desktop {
                if unsafe { vdm.MoveWindowToDesktop(target, &our_id) }.is_ok() {
                    uncloaked = true;
                }
            }

            // Plan B: move our canvas to target's desktop (own-process, always works)
            if !uncloaked {
                if let Ok(target_desktop) = unsafe { vdm.GetWindowDesktopId(target) } {
                    if unsafe { vdm.MoveWindowToDesktop(state.hwnd, &target_desktop) }.is_ok() {
                        // Reclaim topmost + foreground to trigger desktop switch
                        unsafe {
                            let _ = SetWindowPos(state.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                            let _ = SetForegroundWindow(state.hwnd);
                        }
                        saved_desktop = our_desktop;
                        uncloaked = true;
                    }
                }
            }

            if uncloaked {
                state.windows[win_idx].cloaked = false;
                is_cloaked = false;
                // Unregister old thumbnail before re-registering
                if let Some(thumb) = state.windows[win_idx].thumbnail {
                    unregister_thumbnail(thumb);
                    state.windows[win_idx].thumbnail = None;
                }
                // Re-measure now that window is on our desktop
                let prev_sw = state.windows[win_idx].source_w;
                let prev_sh = state.windows[win_idx].source_h;
                let prev_cw = state.windows[win_idx].client_w;
                let prev_ch = state.windows[win_idx].client_h;
                register_and_measure_thumbnail(state.hwnd, &mut state.windows[win_idx]);
                if state.windows[win_idx].source_w <= 0 { state.windows[win_idx].source_w = prev_sw; }
                if state.windows[win_idx].source_h <= 0 { state.windows[win_idx].source_h = prev_sh; }
                if state.windows[win_idx].client_w <= 0 { state.windows[win_idx].client_w = prev_cw; }
                if state.windows[win_idx].client_h <= 0 { state.windows[win_idx].client_h = prev_ch; }
            }
        }
    }

    // For current-desktop windows: restore if minimized, remove TOPMOST
    let mut was_topmost = false;
    if !is_cloaked {
        if placement.showCmd == SW_SHOWMINIMIZED.0 as u32 {
            unsafe { let _ = ShowWindow(target, SW_RESTORE); }
        }
        let ex_style = unsafe { GetWindowLongW(target, GWL_EXSTYLE) } as u32;
        was_topmost = ex_style & WS_EX_TOPMOST.0 != 0;
        if was_topmost {
            unsafe {
                let _ = SetWindowPos(target, Some(HWND_NOTOPMOST), 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    // Register hotkeys now so F1/Escape work during the animation
    unsafe {
        let _ = RegisterHotKey(Some(state.hwnd), PIN_F1_HOTKEY_ID, MOD_NOREPEAT, VK_F1.0 as u32);
        let _ = RegisterHotKey(Some(state.hwnd), PIN_ESC_HOTKEY_ID, MOD_NOREPEAT, VK_ESCAPE.0 as u32);
    }

    // Calculate target zoom+pan to center this cell at the window's original client size.
    // Use source dimensions as fallback if client dimensions are unavailable (e.g. minimized).
    let w = &state.windows[win_idx];
    let client_w = if w.client_w > 0 { w.client_w } else { w.source_w };
    let client_h = if w.client_h > 0 { w.client_h } else { w.source_h };
    if client_w <= 0 || client_h <= 0 {
        // No valid size available; cannot compute zoom target -- abort
        return;
    }
    let (target_zoom, target_pan_x, target_pan_y) =
        state.canvas.calc_pin_target(grid_idx, client_w, client_h);

    // Save current canvas view for restoration on exit
    let saved_zoom = state.canvas.zoom;
    let saved_pan_x = state.canvas.pan_x;
    let saved_pan_y = state.canvas.pan_y;

    // Start zoom+pan animation -- hole is applied when it completes
    let now = unsafe { let mut t: i64 = 0; let _ = QueryPerformanceCounter(&mut t); t };
    state.canvas.animate_zoom_pan_to(target_zoom, target_pan_x, target_pan_y, state.qpc_freq, now);
    unsafe { let _ = SetTimer(Some(state.hwnd), ANIM_TIMER_ID, ANIM_INTERVAL_MS, None); }

    state.pin_zoom_pending = true;
    state.pin_focus = Some(PinFocusState {
        grid_idx,
        target_hwnd: target,
        saved_placement: placement,
        was_topmost,
        is_cloaked,
        saved_desktop,
        saved_zoom,
        saved_pan_x,
        saved_pan_y,
    });
}

fn exit_pin_focus(state: &mut AppState) {
    if let Some(focus) = state.pin_focus.take() {
        state.pin_zoom_pending = false;
        // Remove hole and restore full overlay
        clear_region_hole(state.hwnd);
        unsafe {
            if !focus.is_cloaked && IsWindow(Some(focus.target_hwnd)).as_bool() {
                let _ = SetWindowPlacement(focus.target_hwnd, &focus.saved_placement);
                if focus.was_topmost {
                    let _ = SetWindowPos(focus.target_hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
            // If we switched desktops, switch back
            if let Some(original_desktop) = focus.saved_desktop {
                if let Some(ref vdm) = state.vdm {
                    let _ = vdm.MoveWindowToDesktop(state.hwnd, &original_desktop);
                }
            }
            let _ = UnregisterHotKey(Some(state.hwnd), PIN_F1_HOTKEY_ID);
            let _ = UnregisterHotKey(Some(state.hwnd), PIN_ESC_HOTKEY_ID);
            let _ = SetForegroundWindow(state.hwnd);
        }
        // Re-show DWM thumbnails so they're visible during zoom-out animation
        update_all_thumbnails(state);
        // Animate zoom+pan back to original canvas view
        let now = unsafe { let mut t: i64 = 0; let _ = QueryPerformanceCounter(&mut t); t };
        state.canvas.animate_zoom_pan_to(
            focus.saved_zoom, focus.saved_pan_x, focus.saved_pan_y,
            state.qpc_freq, now,
        );
        unsafe { let _ = SetTimer(Some(state.hwnd), ANIM_TIMER_ID, ANIM_INTERVAL_MS, None); }
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
                            let display_title = if full.chars().count() > 45 {
                                let truncated: String = full.chars().take(42).collect();
                                format!("{}...", truncated)
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
                    // Pin focus active (or animating in): ignore scroll on overlay
                    if state.pin_focus.is_some() || state.pin_zoom_pending {
                        return;
                    }
                    let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
                    let mut now: i64 = 0;
                    let _ = QueryPerformanceCounter(&mut now);
                    state.canvas.stop_inertia();
                    state.canvas.zoom_at_animated(mx, my, delta, state.qpc_freq, now);
                    let _ = SetTimer(Some(hwnd), ANIM_TIMER_ID, ANIM_INTERVAL_MS, None);
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
                    // Animating zoom-in: ignore right-click pan
                    if state.pin_zoom_pending {
                        return;
                    }
                    if state.pin_focus.is_none() {
                        state.canvas.anim.active = false;
                        state.canvas.stop_inertia();
                        let _ = KillTimer(Some(hwnd), ANIM_TIMER_ID);
                    }
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
                    if state.pin_zoom_pending {
                        return;
                    }
                    state.canvas.is_panning = false;
                    if !state.did_pan && !state.pin_mode && state.pin_focus.is_none() {
                        let count = state.filtered_indices.len();
                        if let Some(grid_idx) = state.canvas.hit_test(mx, my, count) {
                            let win_idx = state.filtered_indices[grid_idx];
                            show_context_menu(hwnd, mx, my, win_idx, state);
                        }
                        state.canvas.stop_inertia();
                    } else if state.did_pan && state.pin_focus.is_none() {
                        // Start inertial coast
                        let mut now: i64 = 0;
                        let _ = QueryPerformanceCounter(&mut now);
                        if state.canvas.start_inertia(now) {
                            let _ = SetTimer(Some(hwnd), ANIM_TIMER_ID, ANIM_INTERVAL_MS, None);
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
                        let mut now: i64 = 0;
                        let _ = QueryPerformanceCounter(&mut now);
                        state.canvas.pan_with_velocity(dx, dy, state.qpc_freq, now);
                        state.canvas.last_mouse_x = mx;
                        state.canvas.last_mouse_y = my;
                        update_all_thumbnails(state);
                        if state.pin_focus.is_some() {
                            update_pin_position(state);
                        }
                        let _ = InvalidateRect(Some(hwnd), None, false);
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
                        // (clicks pass through the region hole to the real window).
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
                            let win_idx = state.filtered_indices[grid_idx];
                            let target_hwnd = state.windows[win_idx].hwnd;
                            let is_cloaked = state.windows[win_idx].cloaked;
                            if is_cloaked {
                                // Move canvas to target's desktop so the switch happens
                                activate_cross_desktop(state, target_hwnd);
                            }
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
                    // Pin focus: clicks pass through the region hole
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
            if vk == VK_F1.0 {
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
                                    if state.windows[win_idx].cloaked {
                                        activate_cross_desktop(state, target_hwnd);
                                    }
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
                                if state.windows[win_idx].cloaked {
                                    activate_cross_desktop(state, target_hwnd);
                                }
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
                                ensure_canvas_on_current_desktop(state);
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
                        let anim_going = state.canvas.tick_animation(now);
                        let inertia_going = state.canvas.tick_inertia(now, state.qpc_freq);
                        update_all_thumbnails(state);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        // Pin hole fires when zoom animation ends, independent of inertia
                        if !anim_going && state.pin_zoom_pending {
                            state.pin_zoom_pending = false;
                            apply_pin_hole(state);
                        }
                        if !anim_going && !inertia_going {
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
                    // Resize invalidates thumbnail positions -- exit pin focus
                    exit_pin_focus(state);
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

        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
