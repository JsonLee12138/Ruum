//! Pure-Rust QR code matrix encoder and GPUI canvas renderer.
//!
//! Renders QR codes with an explicit 4-module opaque white quiet zone
//! (`#FFFFFF`) and opaque black modules (`#000000`) so mobile Alipay scanners
//! read reliably under both dark and light desktop themes.

use std::sync::Arc;

use gpui_kit::InteractiveElement as _;
use gpui_kit::{
    Bounds, Corners, Edges, Hsla, IntoElement, ParentElement as _, Pixels,
    Point, Rems, Rgba, Size, Styled as _, Window, canvas, div, quad,
};

/// Standard QR quiet zone width in modules on all four sides.
pub const QUIET_MODULES: usize = 4;

/// Binary QR matrix with helper accessors for 4-module quiet-zone rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrMatrix {
    width: usize,
    dark: Vec<bool>,
}

impl QrMatrix {
    /// Encode a non-empty payload (such as an Alipay QR URL) into a QR matrix.
    pub fn encode(payload: &str) -> Result<Self, String> {
        let trimmed = payload.trim();
        if trimmed.is_empty() {
            return Err("QR payload is empty".to_owned());
        }
        let code = qrcode::QrCode::new(trimmed.as_bytes())
            .map_err(|e| format!("failed to encode QR payload: {e}"))?;
        let width = code.width();
        let mut dark = Vec::with_capacity(width * width);
        for y in 0..width {
            for x in 0..width {
                dark.push(code[(x, y)] == qrcode::Color::Dark);
            }
        }
        Ok(Self { width, dark })
    }

    /// Data matrix dimension (excluding quiet zone) in modules.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Total rendered dimension including the 4-module quiet zone on each side.
    #[must_use]
    pub const fn total_modules(&self) -> usize {
        self.width + 2 * QUIET_MODULES
    }

    /// Return `true` if data module `(x, y)` is dark (`0..width`).
    #[must_use]
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.width {
            return false;
        }
        self.dark[y * self.width + x]
    }

    /// Return `true` if canvas module `(x, y)` in `0..total_modules` is dark,
    /// keeping the 4-module outer quiet zone strictly light (`false`).
    #[cfg(test)]
    #[must_use]
    pub fn is_dark_with_quiet_zone(&self, x: usize, y: usize) -> bool {
        let end = self.width + QUIET_MODULES;
        if x < QUIET_MODULES || y < QUIET_MODULES || x >= end || y >= end {
            return false;
        }
        self.is_dark(x - QUIET_MODULES, y - QUIET_MODULES)
    }
}

/// Render a high-contrast QR code inside a square `Rems`-sized GPUI `canvas`.
pub fn qr_canvas(
    matrix: Arc<QrMatrix>,
    size_rem: Rems,
    debug_id: &'static str,
) -> impl IntoElement {
    div()
        .id(debug_id)
        .debug_selector(move || debug_id.into())
        .w(size_rem)
        .h(size_rem)
        .flex_shrink_0()
        .child(
            canvas(
                |bounds, _, _| bounds.size,
                move |bounds, _, window, _| {
                    paint_qr(&matrix, bounds, window);
                },
            )
            .size_full(),
        )
}

fn paint_qr(matrix: &QrMatrix, bounds: Bounds<Pixels>, window: &mut Window) {
    let side = bounds.size.width.min(bounds.size.height);
    if side.as_f32() <= 0.0 {
        return;
    }
    let total = matrix.total_modules() as f32;
    let origin_x = bounds.origin.x + (bounds.size.width - side) * 0.5;
    let origin_y = bounds.origin.y + (bounds.size.height - side) * 0.5;
    let scale = window.scale_factor().max(1.0);
    let none = Edges::default();
    let solid = gpui_kit::BorderStyle::Solid;

    let white: Hsla = Rgba {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    }
    .into();
    let black: Hsla = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    }
    .into();

    // Paint the entire square (including the 4-module quiet zone) opaque white
    // so dark desktop themes never bleed behind finder patterns.
    let bg_bounds =
        snap_rect(origin_x, origin_y, origin_x + side, origin_y + side, scale);
    window.paint_quad(quad(
        bg_bounds,
        Corners::default(),
        white,
        none,
        white,
        solid,
    ));

    for my in 0..matrix.width() {
        let qy = (my + QUIET_MODULES) as f32;
        let y0 = origin_y + side * (qy / total);
        let y1 = origin_y + side * ((qy + 1.0) / total);
        for mx in 0..matrix.width() {
            if !matrix.is_dark(mx, my) {
                continue;
            }
            let qx = (mx + QUIET_MODULES) as f32;
            let x0 = origin_x + side * (qx / total);
            let x1 = origin_x + side * ((qx + 1.0) / total);
            let cell = snap_rect(x0, y0, x1, y1, scale);
            window.paint_quad(quad(
                cell,
                Corners::default(),
                black,
                none,
                black,
                solid,
            ));
        }
    }
}

fn snap_rect(
    left: Pixels,
    top: Pixels,
    right: Pixels,
    bottom: Pixels,
    scale: f32,
) -> Bounds<Pixels> {
    let snap = |val: Pixels| -> Pixels {
        Pixels::from((val.as_f32() * scale).round() / scale)
    };
    let l = snap(left);
    let t = snap(top);
    let r = snap(right);
    let b = snap(bottom);
    let zero = Pixels::from(0.0_f32);
    Bounds::new(
        Point::new(l, t),
        Size::new((r - l).max(zero), (b - t).max(zero)),
    )
}
