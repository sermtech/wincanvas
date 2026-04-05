# WinCanvas

Infinite canvas window manager for Windows. Shows live thumbnails of all open windows on a pannable, zoomable canvas.

## Requirements

- Windows 10/11 with Desktop Window Manager (DWM) enabled (default)
- Rust toolchain (for building from source)

## Quick Start

- **start.bat** - Launch the app
- **kill.bat** - Stop the app

## Building from Source

```
cargo build --release
```

The binary will be at `target\release\wincanvas.exe`.

## Controls

| Action | Input |
|--------|-------|
| Toggle show/hide | Ctrl+Space (global hotkey, works from any app) |
| Pan the canvas | Right-click drag |
| Zoom in/out | Mouse wheel (zooms toward cursor) |
| Switch to a window | Left-click its thumbnail |
| Search by window title | Start typing |
| Delete search text | Backspace |
| Clear search | Escape |
| Hide canvas | Escape (when search is empty) |

## How It Works

- Uses the DWM Thumbnail API for live, hardware-accelerated window previews
- Direct2D for rendering the background, search bar, and window titles
- Windows are re-enumerated every 2 seconds to pick up new/closed windows
- Filtered windows (system trays, tooltips, zero-title, tool windows) are hidden automatically

## Architecture

```
src/
  main.rs         - Win32 window, message loop, global hotkey, timer
  canvas.rs       - Pan/zoom state, grid layout, hit testing
  thumbnails.rs   - Window enumeration (EnumWindows), DWM thumbnail lifecycle
  search.rs       - Case-insensitive title filtering
  render.rs       - Direct2D/DirectWrite rendering
```
