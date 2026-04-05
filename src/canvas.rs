use windows::Win32::Foundation::RECT;

pub const BASE_CELL_W: f64 = 300.0;
pub const BASE_CELL_H: f64 = 200.0;
pub const CELL_PADDING: f64 = 20.0;
pub const SEARCH_BAR_H: f64 = 50.0;
pub const TITLE_H: f64 = 24.0;

pub struct CanvasState {
    pub pan_x: f64,
    pub pan_y: f64,
    pub zoom: f64,
    pub canvas_w: f64,
    pub canvas_h: f64,
    pub is_panning: bool,
    pub last_mouse_x: i32,
    pub last_mouse_y: i32,
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
        }
    }

    pub fn cell_w(&self) -> f64 {
        BASE_CELL_W * self.zoom
    }

    pub fn cell_h(&self) -> f64 {
        (BASE_CELL_H + TITLE_H) * self.zoom
    }

    pub fn cols(&self) -> usize {
        let cw = self.cell_w() + CELL_PADDING * self.zoom;
        let cols = (self.canvas_w / cw).floor() as usize;
        if cols < 1 { 1 } else { cols }
    }

    pub fn grid_rect(&self, index: usize) -> RECT {
        let cols = self.cols();
        let col = index % cols;
        let row = index / cols;
        let cw = self.cell_w();
        let ch = self.cell_h();
        let pad = CELL_PADDING * self.zoom;

        let total_row_w = cols as f64 * (cw + pad) - pad;
        let start_x = (self.canvas_w - total_row_w) / 2.0;

        let x = start_x + col as f64 * (cw + pad) + self.pan_x;
        let y = SEARCH_BAR_H + row as f64 * (ch + pad) + self.pan_y;
        let thumb_h = BASE_CELL_H * self.zoom;

        RECT {
            left: x as i32,
            top: y as i32,
            right: (x + cw) as i32,
            bottom: (y + thumb_h) as i32,
        }
    }

    pub fn title_rect(&self, index: usize) -> RECT {
        let thumb = self.grid_rect(index);
        let title_h = (TITLE_H * self.zoom) as i32;
        RECT {
            left: thumb.left,
            top: thumb.bottom + 2,
            right: thumb.right,
            bottom: thumb.bottom + 2 + title_h,
        }
    }

    pub fn hit_test(&self, mx: i32, my: i32, count: usize) -> Option<usize> {
        for i in 0..count {
            let r = self.grid_rect(i);
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

    pub fn center_on(&mut self, index: usize) {
        // Calculate where the cell center WOULD be at pan=(0,0), then set pan
        // so that point lands at screen center
        let cols = self.cols();
        let col = index % cols;
        let row = index / cols;
        let cw = self.cell_w();
        let ch = self.cell_h();
        let pad = CELL_PADDING * self.zoom;
        let thumb_h = BASE_CELL_H * self.zoom;

        let total_row_w = cols as f64 * (cw + pad) - pad;
        let start_x = (self.canvas_w - total_row_w) / 2.0;

        // Cell center without pan
        let cell_cx = start_x + col as f64 * (cw + pad) + cw / 2.0;
        let cell_cy = SEARCH_BAR_H + row as f64 * (ch + pad) + thumb_h / 2.0;

        // Screen center
        let screen_cx = self.canvas_w / 2.0;
        let screen_cy = self.canvas_h / 2.0;

        // Pan so cell center = screen center
        self.pan_x = screen_cx - cell_cx;
        self.pan_y = screen_cy - cell_cy;
    }
}
