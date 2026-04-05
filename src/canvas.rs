use windows::Win32::Foundation::RECT;

pub const BASE_ROW_H: f64 = 200.0;
pub const CELL_PADDING: f64 = 20.0;
pub const SEARCH_BAR_H: f64 = 50.0;
pub const TITLE_H: f64 = 24.0;
pub const THUMB_INSET: i32 = 2;

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
    pub start_ticks: i64,
    pub duration_ticks: i64,
    pub active: bool,
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
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
                start_ticks: 0,
                duration_ticks: 0,
                active: false,
            },
            layout: Vec::new(),
        }
    }

    /// Compute flow layout in world coordinates (zoom-independent).
    /// Positions are stable across zoom changes -- zoom is applied in cell_rect().
    /// source_sizes: (source_w, source_h) for each filtered window.
    pub fn compute_layout(&mut self, source_sizes: &[(i32, i32)]) {
        let count = source_sizes.len();
        self.layout.clear();
        if count == 0 {
            return;
        }

        let row_h = BASE_ROW_H;
        let pad = CELL_PADDING;
        let title_h = TITLE_H;
        let slot_h = row_h + title_h + 2.0; // thumbnail + gap + title
        let max_row_w = self.canvas_w - 2.0 * pad;

        // Compute each window's width at target row height
        let widths: Vec<f64> = source_sizes
            .iter()
            .map(|&(sw, sh)| {
                if sw <= 0 || sh <= 0 {
                    row_h // fallback: square
                } else {
                    let w = row_h * (sw as f64 / sh as f64);
                    w.clamp(row_h * 0.5, row_h * 3.0)
                }
            })
            .collect();

        // Lay out in rows: pack left-to-right, wrap when full
        self.layout.resize(count, CellLayout { x: 0.0, y: 0.0, w: 0.0, h: 0.0, row: 0 });

        let mut x = 0.0;
        let mut y = SEARCH_BAR_H;
        let mut row_idx = 0usize;
        let mut row_start = 0usize;

        for i in 0..count {
            if x + widths[i] > max_row_w && i > row_start {
                // Center the completed row
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
        // Center last row
        center_row(&mut self.layout, row_start, count, x - pad, max_row_w, pad);
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

    pub fn zoom_at(&mut self, mx: i32, my: i32, delta: i16) {
        let old_zoom = self.zoom;
        let factor = if delta > 0 { 1.1 } else { 1.0 / 1.1 };
        self.zoom = (self.zoom * factor).clamp(0.1, 5.0);
        let ratio = self.zoom / old_zoom;
        self.pan_x = mx as f64 - ratio * (mx as f64 - self.pan_x);
        self.pan_y = my as f64 - ratio * (my as f64 - self.pan_y);
    }

    pub fn pan(&mut self, dx: i32, dy: i32) {
        self.pan_x += dx as f64;
        self.pan_y += dy as f64;
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
            start_ticks: now,
            duration_ticks: freq * 200 / 1000, // 200ms
            active: true,
        };
    }

    pub fn tick_animation(&mut self, now: i64) -> bool {
        if !self.anim.active {
            return false;
        }
        let elapsed = now - self.anim.start_ticks;
        if elapsed >= self.anim.duration_ticks {
            self.pan_x = self.anim.target_x;
            self.pan_y = self.anim.target_y;
            self.anim.active = false;
            return true;
        }
        let t = elapsed as f64 / self.anim.duration_ticks as f64;
        let e = ease_out_cubic(t);
        self.pan_x = self.anim.start_x + (self.anim.target_x - self.anim.start_x) * e;
        self.pan_y = self.anim.start_y + (self.anim.target_y - self.anim.start_y) * e;
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
