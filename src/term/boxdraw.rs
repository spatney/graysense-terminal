//! Procedurally drawn box-drawing, block, braille and powerline glyphs.
//!
//! Fonts rarely tile these perfectly at arbitrary line heights; drawing them as
//! geometry keeps TUI borders (like Copilot's input box) seamless and crisp.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// Arm weights for U+2500..=U+257F as [up, right, down, left]:
/// 0 = none, 1 = light, 2 = heavy, 3 = double. `X` marks glyphs handled specially.
const X: [u8; 4] = [9, 9, 9, 9];
#[rustfmt::skip]
const ARMS: [[u8; 4]; 128] = [
    [0,1,0,1],[0,2,0,2],[1,0,1,0],[2,0,2,0], X, X, X, X, X, X, X, X,                       // 2500-250B
    [0,1,1,0],[0,2,1,0],[0,1,2,0],[0,2,2,0],                                                // 250C-250F
    [0,0,1,1],[0,0,1,2],[0,0,2,1],[0,0,2,2],                                                // 2510-2513
    [1,1,0,0],[1,2,0,0],[2,1,0,0],[2,2,0,0],                                                // 2514-2517
    [1,0,0,1],[1,0,0,2],[2,0,0,1],[2,0,0,2],                                                // 2518-251B
    [1,1,1,0],[1,2,1,0],[2,1,1,0],[1,1,2,0],[2,1,2,0],[2,2,1,0],[1,2,2,0],[2,2,2,0],        // 251C-2523
    [1,0,1,1],[1,0,1,2],[2,0,1,1],[1,0,2,1],[2,0,2,1],[2,0,1,2],[1,0,2,2],[2,0,2,2],        // 2524-252B
    [0,1,1,1],[0,1,1,2],[0,2,1,1],[0,2,1,2],[0,1,2,1],[0,1,2,2],[0,2,2,1],[0,2,2,2],        // 252C-2533
    [1,1,0,1],[1,1,0,2],[1,2,0,1],[1,2,0,2],[2,1,0,1],[2,1,0,2],[2,2,0,1],[2,2,0,2],        // 2534-253B
    [1,1,1,1],[1,1,1,2],[1,2,1,1],[1,2,1,2],[2,1,1,1],[1,1,2,1],[2,1,2,1],[2,1,1,2],        // 253C-2543
    [2,2,1,1],[1,1,2,2],[1,2,2,1],[2,2,1,2],[1,2,2,2],[2,1,2,2],[2,2,2,1],[2,2,2,2],        // 2544-254B
    X, X, X, X,                                                                             // 254C-254F dashes
    [0,3,0,3],[3,0,3,0],[0,3,1,0],[0,1,3,0],[0,3,3,0],[0,0,1,3],[0,0,3,1],[0,0,3,3],        // 2550-2557
    [1,3,0,0],[3,1,0,0],[3,3,0,0],[1,0,0,3],[3,0,0,1],[3,0,0,3],[1,3,1,0],[3,1,3,0],        // 2558-255F
    [3,3,3,0],[1,0,1,3],[3,0,3,1],[3,0,3,3],[0,3,1,3],[0,1,3,1],[0,3,3,3],[1,3,0,3],        // 2560-2567
    [3,1,0,1],[3,3,0,3],[1,3,1,3],[3,1,3,1],[3,3,3,3],                                      // 2568-256C
    X, X, X, X, X, X, X,                                                                    // 256D-2573 arcs/diagonals
    [0,0,0,1],[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,2],[2,0,0,0],[0,2,0,0],[0,0,2,0],        // 2574-257B
    [0,2,0,1],[1,0,2,0],[0,1,0,2],[2,0,1,0],                                                // 257C-257F
];

/// Returns true if `c` is drawn procedurally by [`draw`].
pub fn is_procedural(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F | 0x2800..=0x28FF | 0xE0B0..=0xE0B3 | 0xE0B4 | 0xE0B6)
}

struct Snap {
    ppp: f32,
}

impl Snap {
    fn px(&self, v: f32) -> f32 {
        (v * self.ppp).round() / self.ppp
    }
}

/// Draw `c` into the cell rectangle. Returns false if the glyph is not handled.
pub fn draw(painter: &Painter, c: char, rect: Rect, color: Color32, ppp: f32) -> bool {
    let cp = c as u32;
    let snap = Snap { ppp };
    let r = Rect::from_min_max(pos2(snap.px(rect.left()), snap.px(rect.top())), pos2(snap.px(rect.right()), snap.px(rect.bottom())));
    let light = ((r.width() * 0.09 * ppp).round().max(1.0)) / ppp;
    match cp {
        0x2500..=0x257F => {
            let idx = (cp - 0x2500) as usize;
            let arms = ARMS[idx];
            if arms != X {
                lines(painter, r, arms, light, color, &snap);
                return true;
            }
            match cp {
                0x2504 | 0x2505 | 0x2508 | 0x2509 | 0x254C | 0x254D => {
                    let n = match cp { 0x2504 | 0x2505 => 3, 0x254C | 0x254D => 2, _ => 4 };
                    let t = if cp % 2 == 1 { light * 2.0 } else { light };
                    dashes(painter, r, n, t, color, true, &snap);
                }
                0x2506 | 0x2507 | 0x250A | 0x250B | 0x254E | 0x254F => {
                    let n = match cp { 0x2506 | 0x2507 => 3, 0x254E | 0x254F => 2, _ => 4 };
                    let t = if cp % 2 == 1 { light * 2.0 } else { light };
                    dashes(painter, r, n, t, color, false, &snap);
                }
                0x256D..=0x2570 => arc(painter, r, cp, light, color, &snap),
                0x2571..=0x2573 => {
                    let s = Stroke::new(light, color);
                    if cp != 0x2572 {
                        painter.line_segment([r.right_top(), r.left_bottom()], s);
                    }
                    if cp != 0x2571 {
                        painter.line_segment([r.left_top(), r.right_bottom()], s);
                    }
                }
                _ => return false,
            }
            true
        }
        0x2580..=0x259F => {
            blocks(painter, r, cp, color);
            true
        }
        0x2800..=0x28FF => {
            braille(painter, r, (cp - 0x2800) as u8, color);
            true
        }
        0xE0B0..=0xE0B3 | 0xE0B4 | 0xE0B6 => {
            powerline(painter, r, cp, light, color);
            true
        }
        _ => false,
    }
}

fn fill(painter: &Painter, x0: f32, y0: f32, x1: f32, y1: f32, color: Color32) {
    if x1 > x0 && y1 > y0 {
        painter.rect_filled(Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), 0.0, color);
    }
}

fn lines(painter: &Painter, r: Rect, arms: [u8; 4], light: f32, color: Color32, snap: &Snap) {
    let [up, right, down, left] = arms;
    let cx = snap.px(r.center().x);
    let cy = snap.px(r.center().y);
    let thick = |w: u8| if w == 2 { light * 2.0 } else { light };
    let gap = snap.px((light * 1.5).max(r.width() * 0.14)).max(light * 1.5);

    let has_double = arms.contains(&3);
    if !has_double {
        // Horizontal arms extend across the widest vertical arm and vice versa so joins are solid.
        let vmax = thick(up.max(down)) * if up > 0 || down > 0 { 1.0 } else { 0.0 };
        let hmax = thick(left.max(right)) * if left > 0 || right > 0 { 1.0 } else { 0.0 };
        if left > 0 {
            let t = thick(left);
            fill(painter, r.left(), cy - t / 2.0, cx + vmax / 2.0, cy + t / 2.0, color);
        }
        if right > 0 {
            let t = thick(right);
            fill(painter, cx - vmax / 2.0, cy - t / 2.0, r.right(), cy + t / 2.0, color);
        }
        if up > 0 {
            let t = thick(up);
            fill(painter, cx - t / 2.0, r.top(), cx + t / 2.0, cy + hmax / 2.0, color);
        }
        if down > 0 {
            let t = thick(down);
            fill(painter, cx - t / 2.0, cy - hmax / 2.0, cx + t / 2.0, r.bottom(), color);
        }
        return;
    }

    let t = light;
    let g_of = |w: u8| if w == 3 { gap } else { 0.0 };
    let single = |w: u8| w == 1 || w == 2;
    // Distance from the centre to the nearest double line on each axis.
    let vg = if up == 3 || down == 3 { gap } else { 0.0 };
    let hg = if left == 3 || right == 3 { gap } else { 0.0 };

    // Double horizontal arms: each of the two lines ends where it meets the perpendicular arms.
    for (w, is_left) in [(left, true), (right, false)] {
        if w != 3 {
            continue;
        }
        let (edge, dir) = if is_left { (r.left(), 1.0) } else { (r.right(), -1.0) };
        for (y, side_has, other_has, g_side, g_other) in [
            (cy - gap, up > 0, down > 0, g_of(up), g_of(down)),
            (cy + gap, down > 0, up > 0, g_of(down), g_of(up)),
        ] {
            let end = if side_has {
                cx - dir * g_side
            } else if other_has {
                cx + dir * g_other
            } else {
                cx
            };
            let (x0, x1) = if is_left { (edge, end + t / 2.0) } else { (end - t / 2.0, edge) };
            fill(painter, x0, y - t / 2.0, x1, y + t / 2.0, color);
        }
    }
    // Double vertical arms.
    for (w, is_up) in [(up, true), (down, false)] {
        if w != 3 {
            continue;
        }
        let (edge, dir) = if is_up { (r.top(), 1.0) } else { (r.bottom(), -1.0) };
        for (x, side_has, other_has, g_side, g_other) in [
            (cx - gap, left > 0, right > 0, g_of(left), g_of(right)),
            (cx + gap, right > 0, left > 0, g_of(right), g_of(left)),
        ] {
            let end = if side_has {
                cy - dir * g_side
            } else if other_has {
                cy + dir * g_other
            } else {
                cy
            };
            let (y0, y1) = if is_up { (edge, end + t / 2.0) } else { (end - t / 2.0, edge) };
            fill(painter, x - t / 2.0, y0, x + t / 2.0, y1, color);
        }
    }
    // Single horizontal arms: continuous when both sides are single, else stop at the nearest double line.
    if single(left) && single(right) {
        fill(painter, r.left(), cy - thick(left) / 2.0, r.right(), cy + thick(left) / 2.0, color);
    } else {
        if single(left) {
            fill(painter, r.left(), cy - thick(left) / 2.0, cx - vg + t / 2.0, cy + thick(left) / 2.0, color);
        }
        if single(right) {
            fill(painter, cx + vg - t / 2.0, cy - thick(right) / 2.0, r.right(), cy + thick(right) / 2.0, color);
        }
    }
    // Single vertical arms.
    if single(up) && single(down) {
        fill(painter, cx - thick(up) / 2.0, r.top(), cx + thick(up) / 2.0, r.bottom(), color);
    } else {
        if single(up) {
            fill(painter, cx - thick(up) / 2.0, r.top(), cx + thick(up) / 2.0, cy - hg + t / 2.0, color);
        }
        if single(down) {
            fill(painter, cx - thick(down) / 2.0, cy + hg - t / 2.0, cx + thick(down) / 2.0, r.bottom(), color);
        }
    }
}

fn dashes(painter: &Painter, r: Rect, n: usize, t: f32, color: Color32, horizontal: bool, snap: &Snap) {
    let cx = snap.px(r.center().x);
    let cy = snap.px(r.center().y);
    let len = if horizontal { r.width() } else { r.height() };
    let seg = len / n as f32;
    for i in 0..n {
        let a = i as f32 * seg + seg * 0.15;
        let b = (i + 1) as f32 * seg - seg * 0.15;
        if horizontal {
            fill(painter, r.left() + a, cy - t / 2.0, r.left() + b, cy + t / 2.0, color);
        } else {
            fill(painter, cx - t / 2.0, r.top() + a, cx + t / 2.0, r.top() + b, color);
        }
    }
}

fn arc(painter: &Painter, r: Rect, cp: u32, t: f32, color: Color32, snap: &Snap) {
    let cx = snap.px(r.center().x);
    let cy = snap.px(r.center().y);
    let rad = (r.width().min(r.height()) * 0.5).max(1.0);
    // (arc centre, start angle, horizontal edge x, vertical edge y)
    let (center, a0, hx, vy) = match cp {
        0x256D => (pos2(cx + rad, cy + rad), std::f32::consts::PI, r.right(), r.bottom()), // ╭ down+right
        0x256E => (pos2(cx - rad, cy + rad), -std::f32::consts::FRAC_PI_2, r.left(), r.bottom()), // ╮ down+left
        0x256F => (pos2(cx - rad, cy - rad), 0.0, r.left(), r.top()),                      // ╯ up+left
        _ => (pos2(cx + rad, cy - rad), std::f32::consts::FRAC_PI_2, r.right(), r.top()),  // ╰ up+right
    };
    let steps = 10;
    let pts: Vec<Pos2> = (0..=steps)
        .map(|k| {
            let a = a0 + std::f32::consts::FRAC_PI_2 * k as f32 / steps as f32;
            center + vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    let first = pts[0];
    let last = *pts.last().unwrap();
    painter.add(Shape::line(pts, Stroke::new(t, color)));
    // Straight continuations to the cell edges.
    let s = Stroke::new(t, color);
    for p in [first, last] {
        if (p.x - cx).abs() < 0.5 {
            painter.line_segment([p, pos2(cx, vy)], s);
        } else {
            painter.line_segment([p, pos2(hx, cy)], s);
        }
    }
}

fn blocks(painter: &Painter, r: Rect, cp: u32, color: Color32) {
    let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    let w = r.width();
    let h = r.height();
    let eighth_h = |n: f32| b - h * n / 8.0;
    match cp {
        0x2580 => fill(painter, l, t, rr, t + h / 2.0, color),
        0x2581..=0x2588 => fill(painter, l, eighth_h((cp - 0x2580) as f32), rr, b, color),
        0x2589..=0x258F => fill(painter, l, t, l + w * (8 - (cp - 0x2588)) as f32 / 8.0, b, color),
        0x2590 => fill(painter, l + w / 2.0, t, rr, b, color),
        0x2591..=0x2593 => {
            let alpha = [0.25, 0.5, 0.75][(cp - 0x2591) as usize];
            painter.rect_filled(r, 0.0, color.gamma_multiply(alpha));
        }
        0x2594 => fill(painter, l, t, rr, t + h / 8.0, color),
        0x2595 => fill(painter, rr - w / 8.0, t, rr, b, color),
        _ => {
            // Quadrants: bits = [upper-left, upper-right, lower-left, lower-right].
            let q: [bool; 4] = match cp {
                0x2596 => [false, false, true, false],
                0x2597 => [false, false, false, true],
                0x2598 => [true, false, false, false],
                0x2599 => [true, false, true, true],
                0x259A => [true, false, false, true],
                0x259B => [true, true, true, false],
                0x259C => [true, true, false, true],
                0x259D => [false, true, false, false],
                0x259E => [false, true, true, false],
                _ => [false, true, true, true],
            };
            let mx = l + w / 2.0;
            let my = t + h / 2.0;
            if q[0] {
                fill(painter, l, t, mx, my, color);
            }
            if q[1] {
                fill(painter, mx, t, rr, my, color);
            }
            if q[2] {
                fill(painter, l, my, mx, b, color);
            }
            if q[3] {
                fill(painter, mx, my, rr, b, color);
            }
        }
    }
}

fn braille(painter: &Painter, r: Rect, bits: u8, color: Color32) {
    let cols = [0.3, 0.7];
    let rows = [0.14, 0.38, 0.62, 0.86];
    let map = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)];
    let rad = (r.width() * 0.13).min(r.height() * 0.07).max(0.8);
    for (bit, (cx, cy)) in map.iter().enumerate() {
        if bits & (1 << bit) != 0 {
            let p = pos2(r.left() + r.width() * cols[*cx], r.top() + r.height() * rows[*cy]);
            painter.circle_filled(p, rad, color);
        }
    }
}

fn powerline(painter: &Painter, r: Rect, cp: u32, t: f32, color: Color32) {
    let (l, top, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    let my = r.center().y;
    match cp {
        0xE0B0 => {
            painter.add(Shape::convex_polygon(vec![pos2(l, top), pos2(rr, my), pos2(l, b)], color, Stroke::NONE));
        }
        0xE0B2 => {
            painter.add(Shape::convex_polygon(vec![pos2(rr, top), pos2(rr, b), pos2(l, my)], color, Stroke::NONE));
        }
        0xE0B1 => {
            painter.add(Shape::line(vec![pos2(l, top), pos2(rr, my), pos2(l, b)], Stroke::new(t, color)));
        }
        0xE0B3 => {
            painter.add(Shape::line(vec![pos2(rr, top), pos2(l, my), pos2(rr, b)], Stroke::new(t, color)));
        }
        _ => {
            // Half circles (E0B4 right, E0B6 left).
            let right = cp == 0xE0B4;
            let rad = r.height() / 2.0;
            let center = if right { pos2(l, my) } else { pos2(rr, my) };
            let start = if right { -std::f32::consts::FRAC_PI_2 } else { std::f32::consts::FRAC_PI_2 };
            let mut pts = vec![center];
            for k in 0..=12 {
                let a = start + std::f32::consts::PI * k as f32 / 12.0;
                let p = center + vec2(a.cos() * r.width().min(rad), a.sin() * rad);
                pts.push(p);
            }
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
    }
}
