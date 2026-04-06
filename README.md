# WinCanvas

Infinite canvas window manager for Windows. Shows live DWM thumbnails of all open windows on a pannable, zoomable canvas -- including windows on other virtual desktops.

![demo](assets/demo.gif)

![screenshot](assets/screenshot.png)

## Requirements

- Windows 10 / 11 with DWM enabled (default)
- Rust toolchain

## Build

```
cargo build --release
```

Binary: `target\release\wincanvas.exe`

## Controls

| Action | Input |
|---|---|
| Toggle canvas | Ctrl+Space (global hotkey) |
| Pan | Right-click drag |
| Zoom | Scroll wheel |
| Switch to window | Left-click thumbnail |
| Search | Type to filter by title |
| Clear search / hide | Escape |
| Pin / focus window | F1 |
| Exit pin mode | Escape or F1 |

## Architecture

```
src/
  main.rs       Win32 message loop, hotkeys, VDM desktop switching, pin mode
  canvas.rs     Pan/zoom/inertia, grid layout, hit testing
  thumbnails.rs Window enumeration (EnumWindows), DWM thumbnail lifecycle
  render.rs     Direct2D / DirectWrite rendering
  search.rs     Case-insensitive title filtering
```

Uses the DWM Thumbnail API for live hardware-accelerated previews, Direct2D for chrome, and IVirtualDesktopManager for cross-desktop window navigation.
