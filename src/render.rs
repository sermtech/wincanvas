use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F, D2D_SIZE_U};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_MEASURING_MODE, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

pub struct RenderContext {
    pub factory: ID2D1Factory,
    pub target: ID2D1HwndRenderTarget,
    pub bg_brush: ID2D1SolidColorBrush,
    pub text_brush: ID2D1SolidColorBrush,
    pub search_bg_brush: ID2D1SolidColorBrush,
    pub search_text_brush: ID2D1SolidColorBrush,
    pub highlight_brush: ID2D1SolidColorBrush,
    pub dwrite_factory: IDWriteFactory,
    pub title_format: IDWriteTextFormat,
    pub search_format: IDWriteTextFormat,
}

impl RenderContext {
    pub fn new(hwnd: HWND) -> Self {
        unsafe {
            let factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();

            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);

            let size = D2D_SIZE_U {
                width: (rc.right - rc.left) as u32,
                height: (rc.bottom - rc.top) as u32,
            };

            let rt_props = D2D1_RENDER_TARGET_PROPERTIES::default();
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: size,
                ..Default::default()
            };

            let target = factory
                .CreateHwndRenderTarget(&rt_props, &hwnd_props)
                .unwrap();

            let bg_brush = target
                .CreateSolidColorBrush(&color(0.15, 0.15, 0.25, 1.0), None)
                .unwrap();

            let text_brush = target
                .CreateSolidColorBrush(&color(0.9, 0.9, 0.9, 1.0), None)
                .unwrap();

            let search_bg_brush = target
                .CreateSolidColorBrush(&color(0.15, 0.15, 0.25, 0.9), None)
                .unwrap();

            let search_text_brush = target
                .CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None)
                .unwrap();

            let highlight_brush = target
                .CreateSolidColorBrush(&color(0.3, 0.3, 0.5, 0.5), None)
                .unwrap();

            let dwrite_factory: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();

            let font_name: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
            let locale: Vec<u16> = "en-us\0".encode_utf16().collect();

            let title_format = dwrite_factory
                .CreateTextFormat(
                    PCWSTR(font_name.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    12.0,
                    PCWSTR(locale.as_ptr()),
                )
                .unwrap();

            let _ = title_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = title_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

            let search_format = dwrite_factory
                .CreateTextFormat(
                    PCWSTR(font_name.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    18.0,
                    PCWSTR(locale.as_ptr()),
                )
                .unwrap();

            let _ = search_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

            Self {
                factory,
                target,
                bg_brush,
                text_brush,
                search_bg_brush,
                search_text_brush,
                highlight_brush,
                dwrite_factory,
                title_format,
                search_format,
            }
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        unsafe {
            let size = D2D_SIZE_U {
                width: w,
                height: h,
            };
            let _ = self.target.Resize(&size);
        }
    }

    pub fn begin_draw(&self) {
        unsafe {
            self.target.BeginDraw();
            self.target
                .Clear(Some(&color(0.102, 0.102, 0.180, 1.0)));
        }
    }

    pub fn end_draw(&self) {
        unsafe {
            let _ = self.target.EndDraw(None, None);
        }
    }

    pub fn draw_search_bar(&self, query: &str, canvas_w: f64) {
        unsafe {
            let bar_rect = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: canvas_w as f32,
                bottom: 50.0,
            };
            self.target
                .FillRectangle(&bar_rect, &self.search_bg_brush);

            let display = if query.is_empty() {
                "Type to search windows...".to_string()
            } else {
                format!("Search: {}", query)
            };
            let text: Vec<u16> = display.encode_utf16().collect();
            let text_rect = D2D_RECT_F {
                left: 20.0,
                top: 0.0,
                right: canvas_w as f32 - 20.0,
                bottom: 50.0,
            };

            let brush = if query.is_empty() {
                &self.text_brush
            } else {
                &self.search_text_brush
            };

            self.target.DrawText(
                &text,
                &self.search_format,
                &text_rect,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE(0),
            );
        }
    }

    pub fn draw_title(&self, title: &str, rect: RECT) {
        unsafe {
            let text: Vec<u16> = title.encode_utf16().collect();
            let d2d_rect = D2D_RECT_F {
                left: rect.left as f32,
                top: rect.top as f32,
                right: rect.right as f32,
                bottom: rect.bottom as f32,
            };
            self.target.DrawText(
                &text,
                &self.title_format,
                &d2d_rect,
                &self.text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE(0),
            );
        }
    }

    pub fn draw_cell_border(&self, rect: RECT) {
        unsafe {
            let d2d_rect = D2D_RECT_F {
                left: rect.left as f32 - 1.0,
                top: rect.top as f32 - 1.0,
                right: rect.right as f32 + 1.0,
                bottom: rect.bottom as f32 + 1.0,
            };
            self.target
                .DrawRectangle(&d2d_rect, &self.highlight_brush, 1.0, None);
        }
    }
}
