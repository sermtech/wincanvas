use windows::Win32::Foundation::RECT;

pub const CELL_PADDING: f64 = 20.0;
pub const SEARCH_BAR_H: f64 = 50.0;
pub const TITLE_H: f64 = 24.0;
pub const THUMB_INSET: i32 = 2;
const MIN_ROW_H: f64 = 100.0;
const MAX_ROW_H: f64 = 500.0;

#[derive(Clone, Debug)]
pub struct CellLayout {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub row: usize,
}

pub struct PanAnimation {
    pub start_x: f64,
    pub start_y: f64,
    pub target_x: f64,
    pub target_y: f64,
    pub start_zoom: f64,
    pub target_zoom: f64,
    pub start_ticks: i64,
    pub duration_ticks: i64,
    pub active: bool,
}

/// Critically damped spring response curve.
/// Settles smoothly without overshoot; feels like macOS/iOS animations.
/// omega controls snappiness (higher = faster settle).
fn spring_ease(t: f64) -> f64 {
    let omega = 6.0;
    1.0 - (1.0 + omega * t) * (-omega * t).exp()
}

pub struct CanvasState {
    pub pan_x: f64,
    pub pan_y: f64,
    pub zoom: f64,
    pub canvas_w: f64,
    pub canvas_h: f64,
    pub is_panning: bool,
    pub last_mouse_x: i32,
    pub last_mouse_y: i32,
    pub anim: PanAnimation,
    pub layout: Vec<CellLayout>,
    // Inertial panning state
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub last_pan_ticks: i64,
    pub inertia_active: bool,
}

impl CanvasState {
    pub fn new(w: f64, h: f64) -> Self {
        Self {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
            canvas_w: w,
            canvas_h: h,
            is_panning: false,
            last_mouse_x: 0,
            last_mouse_y: 0,
            anim: PanAnimation {
                start_x: 0.0,
                start_y: 0.0,
                target_x: 0.0,
                target_y: 0.0,
                start_zoom: 1.0,
                target_zoom: 1.0,
                start_ticks: 0,
                duration_ticks: 0,
                active: false,
            },
            layout: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            last_pan_ticks: 0,
            inertia_active: false,
        }
    }

    /// Compute flow layout in world coordinates (zoom-independent).
    /// Auto-scales row height so all windows fit on screen (Task View style).
    /// Falls back to MIN_ROW_H with scrolling if too many windows.
    /// source_sizes: (source_w, source_h) for each filtered window.
    pub fn compute_layout(&mut self, source_sizes: &[(i32, i32)]) {
        let count = source_sizes.len();
        self.layout.clear();
        if count == 0 {
            return;
        }

        let pad = CELL_PADDING;
        let title_h = TITLE_H;
        let max_row_w = self.canvas_w - 2.0 * pad;
        let viewport_h = self.canvas_h - SEARCH_BAR_H;

        // Binary search for the largest row_h where all windows fit on screen
        let row_h = optimal_row_height(source_sizes, max_row_w, viewport_h, pad, title_h);

        let slot_h = row_h + title_h + 2.0;

        // Compute each window's width at the chosen row height
        let widths: Vec<f64> = source_sizes
            .iter()
            .map(|&(sw, sh)| {
                if sw <= 0 || sh <= 0 {
                    row_h
                } else {
                    let w = row_h * (sw as f64 / sh as f64);
                    w.clamp(row_h * 0.5, row_h * 3.0)
                }
            })
            .collect();

        // Lay out in rows: pack left-to-right, wrap when full
        self.layout.resize(count, CellLayout { x: 0.0, y: 0.0, w: 0.0, h: 0.0, row: 0 });

        let mut x = 0.0;
        let mut y = 0.0;
        let mut row_idx = 0usize;
        let mut row_start = 0usize;

        for i in 0..count {
            if x + widths[i] > max_row_w && i > row_start {
                center_row(&mut self.layout, row_start, i, x - pad, max_row_w, pad);
                y += slot_h + pad;
                x = 0.0;
                row_idx += 1;
                row_start = i;
            }
            self.layout[i] = CellLayout {
                x,
                y,
                w: widths[i],
                h: row_h,
                row: row_idx,
            };
            x += widths[i] + pad;
        }
        center_row(&mut self.layout, row_start, count, x - pad, max_row_w, pad);

        // Compute content bounds and center vertically in viewport
        let content_h = y + slot_h; // last row y + one slot height
        let y_offset = if content_h < viewport_h {
            SEARCH_BAR_H + (viewport_h - content_h) / 2.0
        } else {
            SEARCH_BAR_H + pad
        };
        for item in &mut self.layout {
            item.y += y_offset;
        }
    }

    /// Look up pre-computed cell rect, applying zoom + pan transform.
    /// Layout is in world coords; this returns screen coords.
    pub fn cell_rect(&self, index: usize) -> RECT {
        let c = &self.layout[index];
        let x = c.x * self.zoom + self.pan_x;
        let y = c.y * self.zoom + self.pan_y;
        let w = c.w * self.zoom;
        let h = c.h * self.zoom;
        RECT {
            left: x.floor() as i32,
            top: y.floor() as i32,
            right: (x + w).ceil() as i32,
            bottom: (y + h).ceil() as i32,
        }
    }

    /// Title rect: just below the cell thumbnail.
    pub fn title_rect(&self, index: usize) -> RECT {
        let r = self.cell_rect(index);
        let title_h = (TITLE_H * self.zoom) as i32;
        RECT {
            left: r.left,
            top: r.bottom + 2,
            right: r.right,
            bottom: r.bottom + 2 + title_h,
        }
    }

    /// Inset the cell rect by THUMB_INSET for DWM thumbnail (border visibility).
    pub fn thumb_rect(&self, index: usize) -> RECT {
        let r = self.cell_rect(index);
        RECT {
            left: r.left + THUMB_INSET,
            top: r.top + THUMB_INSET,
            right: r.right - THUMB_INSET,
            bottom: r.bottom - THUMB_INSET,
        }
    }

    pub fn hit_test(&self, mx: i32, my: i32, count: usize) -> Option<usize> {
        for i in 0..count.min(self.layout.len()) {
            let r = self.cell_rect(i);
            if mx >= r.left && mx <= r.right && my >= r.top && my <= r.bottom {
                return Some(i);
            }
        }
        None
    }

    /// Find the insertion index for a drag-drop based on mouse position.
    pub fn drop_index(&self, mx: i32, my: i32, count: usize, _dragging: usize) -> Option<usize> {
        // Check each cell's row -- if mouse is in that row, find insertion point
        for i in 0..count.min(self.layout.len()) {
            let r = self.cell_rect(i);
            if my >= r.top && my <= r.bottom {
                let cx = (r.left + r.right) / 2;
                if mx <= cx {
                    return Some(i);
                }
            }
        }
        // Past the last item or below all rows -- append at end
        if count > 0 { Some(count - 1) } else { None }
    }

    /// Screen-space rect for the PIN toggle button (top-right of search bar).
    pub fn pin_button_rect(&self) -> RECT {
        let w = 60.0;
        let h = 30.0;
        let margin = 10.0;
        RECT {
            left: (self.canvas_w - w - margin) as i32,
            top: margin as i32,
            right: (self.canvas_w - margin) as i32,
            bottom: (margin + h) as i32,
        }
    }

    /// Animated scroll-wheel zoom toward cursor. Retargetable mid-flight.
    pub fn zoom_at_animated(&mut self, mx: i32, my: i32, delta: i16, freq: i64, now: i64) {
        // Resolve current animation state if mid-flight
        if self.anim.active {
            self.tick_animation(now);
        }
        let factor = if delta > 0 { 1.12 } else { 1.0 / 1.12 };
        let target_zoom = (self.zoom * factor).clamp(0.1, 5.0);
        let ratio = target_zoom / self.zoom;
        let target_pan_x = mx as f64 - ratio * (mx as f64 - self.pan_x);
        let target_pan_y = my as f64 - ratio * (my as f64 - self.pan_y);
        self.anim = PanAnimation {
            start_x: self.pan_x,
            start_y: self.pan_y,
            target_x: target_pan_x,
            target_y: target_pan_y,
            start_zoom: self.zoom,
            target_zoom,
            start_ticks: now,
            duration_ticks: freq * 150 / 1000, // 150ms -- snappy for scroll
            active: true,
        };
    }

    /// Pan with velocity tracking for inertia on release.
    pub fn pan_with_velocity(&mut self, dx: i32, dy: i32, freq: i64, now: i64) {
        self.pan_x += dx as f64;
        self.pan_y += dy as f64;
        let dt = (now - self.last_pan_ticks) as f64 / freq as f64;
        if dt > 0.001 && dt < 0.1 {
            let vx = dx as f64 / dt;
            let vy = dy as f64 / dt;
            // Exponential smoothing -- dampens jitter from irregular mouse events
            self.velocity_x = 0.6 * self.velocity_x + 0.4 * vx;
            self.velocity_y = 0.6 * self.velocity_y + 0.4 * vy;
        }
        self.last_pan_ticks = now;
    }

    /// Begin inertial coast after releasing pan. Returns true if inertia started.
    pub fn start_inertia(&mut self, now: i64) -> bool {
        let speed = (self.velocity_x * self.velocity_x + self.velocity_y * self.velocity_y).sqrt();
        if speed > 100.0 {
            self.inertia_active = true;
            self.last_pan_ticks = now;
            true
        } else {
            self.velocity_x = 0.0;
            self.velocity_y = 0.0;
            false
        }
    }

    /// Stop inertia (e.g. user clicked or started new pan).
    pub fn stop_inertia(&mut self) {
        self.inertia_active = false;
        self.velocity_x = 0.0;
        self.velocity_y = 0.0;
    }

    /// Tick inertial panning. Returns true if still coasting.
    pub fn tick_inertia(&mut self, now: i64, freq: i64) -> bool {
        if !self.inertia_active {
            return false;
        }
        let dt = (now - self.last_pan_ticks) as f64 / freq as f64;
        if dt <= 0.0 || dt > 0.1 {
            self.last_pan_ticks = now;
            return self.inertia_active;
        }
        self.last_pan_ticks = now;
        let friction = 5.0;
        let decay = (-friction * dt).exp();
        self.velocity_x *= decay;
        self.velocity_y *= decay;
        self.pan_x += self.velocity_x * dt;
        self.pan_y += self.velocity_y * dt;
        let speed = (self.velocity_x * self.velocity_x + self.velocity_y * self.velocity_y).sqrt();
        if speed < 15.0 {
            self.inertia_active = false;
            self.velocity_x = 0.0;
            self.velocity_y = 0.0;
        }
        self.inertia_active
    }

    pub fn center_on(&mut self, index: usize, freq: i64, now: i64) {
        if index >= self.layout.len() {
            return;
        }
        let c = &self.layout[index];
        // World-to-screen: screen = world * zoom + pan
        // We want world center to land at screen center:
        //   screen_cx = world_cx * zoom + target_pan_x
        let world_cx = c.x + c.w / 2.0;
        let world_cy = c.y + c.h / 2.0;

        let screen_cx = self.canvas_w / 2.0;
        let screen_cy = self.canvas_h / 2.0;

        let target_x = screen_cx - world_cx * self.zoom;
        let target_y = screen_cy - world_cy * self.zoom;
        self.animate_pan_to(target_x, target_y, freq, now);
    }

    pub fn scroll_into_view(&mut self, index: usize, freq: i64, now: i64) {
        if index >= self.layout.len() {
            return;
        }
        let r = self.cell_rect(index);
        let title_h = (TITLE_H * self.zoom).ceil() as i32;
        let margin = (CELL_PADDING * self.zoom).ceil() as i32;

        let cell_left = r.left;
        let cell_top = r.top;
        let cell_right = r.right;
        let cell_bottom = r.bottom + 2 + title_h;

        let cell_w = cell_right - cell_left;
        let cell_h = cell_bottom - cell_top;

        let view_left = margin;
        let view_top = SEARCH_BAR_H as i32 + margin;
        let view_right = self.canvas_w as i32 - margin;
        let view_bottom = self.canvas_h as i32 - margin;

        let view_w = view_right - view_left;
        let view_h = view_bottom - view_top;

        if cell_left >= view_left && cell_right <= view_right
            && cell_top >= view_top && cell_bottom <= view_bottom
        {
            return;
        }

        let dx = if cell_w > view_w {
            view_left - cell_left
        } else if cell_left < view_left {
            view_left - cell_left
        } else if cell_right > view_right {
            view_right - cell_right
        } else {
            0
        };

        let dy = if cell_h > view_h {
            view_top - cell_top
        } else if cell_top < view_top {
            view_top - cell_top
        } else if cell_bottom > view_bottom {
            view_bottom - cell_bottom
        } else {
            0
        };

        let target_x = self.pan_x + dx as f64;
        let target_y = self.pan_y + dy as f64;
        self.animate_pan_to(target_x, target_y, freq, now);
    }

    /// Navigate down: find closest window by horizontal center in next row.
    pub fn nav_down(&self, sel: usize) -> usize {
        if self.layout.is_empty() {
            return 0;
        }
        let current_row = self.layout[sel].row;
        let current_cx = self.layout[sel].x + self.layout[sel].w / 2.0;
        let target_row = current_row + 1;

        // Find candidates in target row
        let mut best: Option<(usize, f64)> = None;
        for (i, c) in self.layout.iter().enumerate() {
            if c.row == target_row {
                let cx = c.x + c.w / 2.0;
                let dist = (cx - current_cx).abs();
                if best.is_none() || dist < best.unwrap().1 {
                    best = Some((i, dist));
                }
            }
        }

        if let Some((idx, _)) = best {
            idx
        } else {
            // Wrap to row 0
            let mut best_wrap: Option<(usize, f64)> = None;
            for (i, c) in self.layout.iter().enumerate() {
                if c.row == 0 {
                    let cx = c.x + c.w / 2.0;
                    let dist = (cx - current_cx).abs();
                    if best_wrap.is_none() || dist < best_wrap.unwrap().1 {
                        best_wrap = Some((i, dist));
                    }
                }
            }
            best_wrap.map(|(idx, _)| idx).unwrap_or(0)
        }
    }

    /// Navigate up: find closest window by horizontal center in previous row.
    pub fn nav_up(&self, sel: usize) -> usize {
        if self.layout.is_empty() {
            return 0;
        }
        let current_row = self.layout[sel].row;
        let current_cx = self.layout[sel].x + self.layout[sel].w / 2.0;

        if current_row == 0 {
            // Wrap to last row
            let last_row = self.layout.last().map(|c| c.row).unwrap_or(0);
            let mut best: Option<(usize, f64)> = None;
            for (i, c) in self.layout.iter().enumerate() {
                if c.row == last_row {
                    let cx = c.x + c.w / 2.0;
                    let dist = (cx - current_cx).abs();
                    if best.is_none() || dist < best.unwrap().1 {
                        best = Some((i, dist));
                    }
                }
            }
            return best.map(|(idx, _)| idx).unwrap_or(0);
        }

        let target_row = current_row - 1;
        let mut best: Option<(usize, f64)> = None;
        for (i, c) in self.layout.iter().enumerate() {
            if c.row == target_row {
                let cx = c.x + c.w / 2.0;
                let dist = (cx - current_cx).abs();
                if best.is_none() || dist < best.unwrap().1 {
                    best = Some((i, dist));
                }
            }
        }
        best.map(|(idx, _)| idx).unwrap_or(0)
    }

    fn animate_pan_to(&mut self, target_x: f64, target_y: f64, freq: i64, now: i64) {
        self.anim = PanAnimation {
            start_x: self.pan_x,
            start_y: self.pan_y,
            target_x,
            target_y,
            start_zoom: self.zoom,
            target_zoom: self.zoom,
            start_ticks: now,
            duration_ticks: freq * 280 / 1000, // 280ms
            active: true,
        };
    }

    pub fn animate_zoom_pan_to(&mut self, target_zoom: f64, target_x: f64, target_y: f64, freq: i64, now: i64) {
        self.anim = PanAnimation {
            start_x: self.pan_x,
            start_y: self.pan_y,
            target_x,
            target_y,
            start_zoom: self.zoom,
            target_zoom,
            start_ticks: now,
            duration_ticks: freq * 350 / 1000, // 350ms
            active: true,
        };
    }

    /// Calculate target zoom and pan to center grid_idx on screen at the window's original client size.
    pub fn calc_pin_target(&self, grid_idx: usize, client_w: i32, client_h: i32) -> (f64, f64, f64) {
        let c = &self.layout[grid_idx];
        let inset = THUMB_INSET as f64 * 2.0;
        let zoom_w = (client_w as f64 + inset) / c.w;
        let zoom_h = (client_h as f64 + inset) / c.h;
        let mut target_zoom = zoom_w.min(zoom_h);
        // Cap so the cell fits on screen
        let max_zoom_w = self.canvas_w / c.w;
        let max_zoom_h = self.canvas_h / c.h;
        target_zoom = target_zoom.min(max_zoom_w).min(max_zoom_h);
        // Center cell on screen
        let cx = c.x + c.w / 2.0;
        let cy = c.y + c.h / 2.0;
        let pan_x = self.canvas_w / 2.0 - cx * target_zoom;
        let pan_y = self.canvas_h / 2.0 - cy * target_zoom;
        (target_zoom, pan_x, pan_y)
    }

    pub fn tick_animation(&mut self, now: i64) -> bool {
        if !self.anim.active {
            return false;
        }
        let elapsed = now - self.anim.start_ticks;
        if elapsed >= self.anim.duration_ticks {
            self.pan_x = self.anim.target_x;
            self.pan_y = self.anim.target_y;
            self.zoom = self.anim.target_zoom;
            self.anim.active = false;
            return true;
        }
        let t = elapsed as f64 / self.anim.duration_ticks as f64;
        let e = spring_ease(t);
        self.pan_x = self.anim.start_x + (self.anim.target_x - self.anim.start_x) * e;
        self.pan_y = self.anim.start_y + (self.anim.target_y - self.anim.start_y) * e;
        // Exponential zoom interpolation -- feels uniform because zoom is multiplicative
        if self.anim.start_zoom > 0.0 && self.anim.target_zoom > 0.0 {
            let ratio = self.anim.target_zoom / self.anim.start_zoom;
            self.zoom = self.anim.start_zoom * ratio.powf(e);
        } else {
            self.zoom = self.anim.start_zoom + (self.anim.target_zoom - self.anim.start_zoom) * e;
        }
        true
    }
}

/// Center a row of items horizontally within max_row_w.
fn center_row(layout: &mut [CellLayout], start: usize, end: usize, used_w: f64, max_row_w: f64, pad: f64) {
    if start >= end {
        return;
    }
    let offset = (max_row_w - used_w) / 2.0 + pad;
    for item in &mut layout[start..end] {
        item.x += offset;
    }
}

/// Find the largest row height where all windows fit in the viewport.
/// Falls back to MIN_ROW_H if too many windows to fit.
fn optimal_row_height(
    source_sizes: &[(i32, i32)],
    max_row_w: f64,
    viewport_h: f64,
    pad: f64,
    title_h: f64,
) -> f64 {
    let max_h = (viewport_h * 0.7).min(MAX_ROW_H).max(MIN_ROW_H);
    let mut lo = MIN_ROW_H;
    let mut hi = max_h;

    // 30 iterations of binary search = sub-pixel precision
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        let slot_h = mid + title_h + 2.0 + pad;
        let rows = count_rows(source_sizes, mid, max_row_w, pad);
        let total_h = rows as f64 * slot_h - pad;

        if total_h <= viewport_h {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    lo
}

/// Count how many rows the flow layout would produce at a given row height.
fn count_rows(source_sizes: &[(i32, i32)], row_h: f64, max_row_w: f64, pad: f64) -> usize {
    let mut x = 0.0;
    let mut rows = 1usize;
    let mut items_in_row = 0usize;

    for &(sw, sh) in source_sizes {
        let w = if sw > 0 && sh > 0 {
            (row_h * sw as f64 / sh as f64).clamp(row_h * 0.5, row_h * 3.0)
        } else {
            row_h
        };

        if x + w > max_row_w && items_in_row > 0 {
            rows += 1;
            x = 0.0;
            items_in_row = 0;
        }

        x += w + pad;
        items_in_row += 1;
    }

    rows
}
