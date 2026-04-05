use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, GWL_EXSTYLE, GWL_STYLE, WS_EX_TOOLWINDOW, WS_CHILD,
};

// DWM_TNP flags
const DWM_TNP_RECTDESTINATION: u32 = 0x1;
const DWM_TNP_VISIBLE: u32 = 0x8;
const DWM_TNP_SOURCECLIENTAREAONLY: u32 = 0x10;
const DWM_TNP_OPACITY: u32 = 0x4;

pub struct WindowInfo {
    pub hwnd: HWND,
    pub title: String,
    pub process_name: String,
    pub pid: u32,
    pub thumbnail: Option<isize>,
    pub source_w: i32,
    pub source_h: i32,
}

fn get_process_name(pid: u32) -> String {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if let Ok(handle) = handle {
            let mut buf = [0u16; 260];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0), windows::core::PWSTR(buf.as_mut_ptr()), &mut size);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
            if ok.is_ok() {
                let path = String::from_utf16_lossy(&buf[..size as usize]);
                if let Some(name) = path.rsplit('\\').next() {
                    return name.trim_end_matches(".exe").to_string();
                }
            }
        }
        "unknown".to_string()
    }
}

struct EnumData {
    windows: Vec<WindowInfo>,
    our_hwnd: HWND,
    our_pid: u32,
}

pub fn enumerate_windows_v2(our_hwnd: HWND) -> Vec<WindowInfo> {
    let our_pid = unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(our_hwnd, Some(&mut pid));
        pid
    };
    let mut data = EnumData {
        windows: Vec::new(),
        our_hwnd,
        our_pid,
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_callback_v2),
            LPARAM(&mut data as *mut EnumData as isize),
        );
    }
    data.windows
}

unsafe extern "system" fn enum_callback_v2(
    hwnd: HWND,
    lparam: LPARAM,
) -> windows::core::BOOL {
    let data = &mut *(lparam.0 as *mut EnumData);

    if hwnd == data.our_hwnd {
        return windows::core::BOOL(1);
    }

    if !IsWindowVisible(hwnd).as_bool() {
        return windows::core::BOOL(1);
    }

    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    if style & WS_CHILD.0 != 0 {
        return windows::core::BOOL(1);
    }

    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return windows::core::BOOL(1);
    }

    let len = GetWindowTextLengthW(hwnd);
    if len == 0 {
        return windows::core::BOOL(1);
    }

    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == data.our_pid {
        return windows::core::BOOL(1);
    }

    let mut buf = vec![0u16; (len + 1) as usize];
    GetWindowTextW(hwnd, &mut buf);
    let title = String::from_utf16_lossy(&buf[..len as usize]);

    let process_name = get_process_name(pid);
    data.windows.push(WindowInfo {
        hwnd,
        title,
        process_name,
        pid,
        thumbnail: None,
        source_w: 0,
        source_h: 0,
    });

    windows::core::BOOL(1)
}

pub fn register_thumbnail(dest: HWND, source: HWND) -> Option<isize> {
    unsafe {
        match DwmRegisterThumbnail(dest, source) {
            Ok(handle) => Some(handle),
            Err(_) => None,
        }
    }
}

pub fn query_source_size(thumb: isize) -> (i32, i32) {
    unsafe {
        match DwmQueryThumbnailSourceSize(thumb) {
            Ok(size) if size.cx > 0 && size.cy > 0 => (size.cx, size.cy),
            _ => (0, 0),
        }
    }
}

pub fn update_thumbnail(thumb: isize, dest_rect: RECT) {
    let props = DWM_THUMBNAIL_PROPERTIES {
        dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
        rcDestination: dest_rect,
        fVisible: windows::core::BOOL(1),
        opacity: 255,
        fSourceClientAreaOnly: windows::core::BOOL(1),
        ..Default::default()
    };
    unsafe {
        let _ = DwmUpdateThumbnailProperties(thumb, &props);
    }
}

pub fn unregister_thumbnail(thumb: isize) {
    unsafe {
        let _ = DwmUnregisterThumbnail(thumb);
    }
}
