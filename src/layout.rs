//! Bay tiling.

use eframe::egui::{Rect, pos2, vec2};

use crate::settings::LayoutMode;

/// Compute bay rectangles inside `area`.
pub fn compute(area: Rect, n: usize, focused: usize, mode: LayoutMode, gap: f32) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![area];
    }
    let split_h = |r: Rect, k: usize| -> Vec<Rect> {
        let w = (r.width() - gap * (k - 1) as f32) / k as f32;
        (0..k).map(|i| Rect::from_min_size(pos2(r.left() + i as f32 * (w + gap), r.top()), vec2(w, r.height()))).collect()
    };
    let split_v = |r: Rect, k: usize| -> Vec<Rect> {
        let h = (r.height() - gap * (k - 1) as f32) / k as f32;
        (0..k).map(|i| Rect::from_min_size(pos2(r.left(), r.top() + i as f32 * (h + gap)), vec2(r.width(), h))).collect()
    };
    match mode {
        LayoutMode::Columns => split_h(area, n),
        LayoutMode::Rows => split_v(area, n),
        LayoutMode::Focus => {
            let main_w = (area.width() - gap) * 0.64;
            let main = Rect::from_min_size(area.min, vec2(main_w, area.height()));
            let side = Rect::from_min_max(pos2(main.right() + gap, area.top()), area.max);
            let mut stacked = split_v(side, n - 1).into_iter();
            (0..n).map(|i| if i == focused.min(n - 1) { main } else { stacked.next().unwrap_or(side) }).collect()
        }
        LayoutMode::Auto => {
            if n == 3 {
                let cols = split_h(area, 2);
                let mut out = vec![cols[0]];
                out.extend(split_v(cols[1], 2));
                return out;
            }
            let cols = (n as f32).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            let row_rects = split_v(area, rows);
            let mut out = Vec::with_capacity(n);
            for (r, row) in row_rects.into_iter().enumerate() {
                let remaining = n - r * cols;
                out.extend(split_h(row, remaining.min(cols)));
            }
            out
        }
    }
}
