//! Drawing the touch overlay's buttons: the game's controls over the HUD and the Touch Controls page's
//! preview, so the settings show exactly what a finger will press.
use crate::{
    hud::{HudAtlas, HudBatch, rgba},
    text::Text,
    touch::{Control, Rect},
};

/// A whole control: vanilla's button behind its mark.
pub fn draw_control(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    control: Control,
    r: Rect,
    held: bool,
    opacity: f32,
) {
    draw_button(batch, atlas, r, held, opacity);
    shadowed(r, |r, color| glyph(batch, atlas, control, r, color));
}

/// A player's button: its name ("NEW" until named) over vanilla's sprite, or over the solid background
/// and inside the stroke color the player picked.
pub fn draw_custom(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    label: &str,
    r: Rect,
    held: bool,
    opacity: f32,
    background: Option<u32>,
    stroke: Option<u32>,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    match background {
        Some(color) => {
            let alpha = ((if held { 230.0 } else { 200.0 }) * opacity) as u8;
            batch.fill(atlas, r[0], r[1], r[2], r[3], with_alpha(color, alpha));
            if held {
                batch.fill(atlas, r[0], r[1], r[2], r[3], rgba(255, 255, 255, 40));
            }
        }
        None => draw_button(batch, atlas, r, held, opacity),
    }
    if let Some(color) = stroke {
        let t = (r[2].min(r[3]) / 16.0).round().clamp(1.0, 3.0);
        let color = with_alpha(color, (255.0 * opacity) as u8);
        batch.fill(atlas, r[0], r[1], r[2], t, color);
        batch.fill(atlas, r[0], r[1] + r[3] - t, r[2], t, color);
        batch.fill(atlas, r[0], r[1], t, r[3], color);
        batch.fill(atlas, r[0] + r[2] - t, r[1], t, r[3], color);
    }
    crate::ui::centered_text(
        batch,
        atlas,
        &Text::literal(label),
        r[0] + 2.0,
        r[1] + ((r[3] - 8.0) / 2.0).floor(),
        r[2] - 4.0,
        rgba(255, 255, 255, 255),
    );
}

/// A color from [`crate::touch::COLORS`] at a new alpha.
fn with_alpha(color: u32, alpha: u8) -> u32 {
    (color & 0x00FF_FFFF) | u32::from(alpha) << 24
}

/// The analog stick: a dark disc, a light ring, and a thumb that follows the finger. `analog` is the
/// stick's `[left, forward]`, each -1..=1; the thumb sits towards the direction it moves the player.
pub fn draw_stick(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    r: Rect,
    analog: [f32; 2],
    held: bool,
    opacity: f32,
) {
    let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    let radius = r[2].min(r[3]) / 2.0;
    if radius <= 0.0 {
        return;
    }
    let alpha = (if held { 150.0 } else { 110.0 }) * opacity.clamp(0.0, 1.0);
    disc(
        batch,
        atlas,
        (cx, cy),
        radius * 0.94,
        rgba(24, 24, 24, (alpha * 0.55) as u8),
    );
    ring(
        batch,
        atlas,
        (cx, cy),
        radius * 0.86,
        radius,
        rgba(230, 230, 230, alpha as u8),
    );
    let thumb = radius * 0.32;
    let (ax, ay) = (-analog[0], -analog[1]);
    let len = ax.hypot(ay);
    let (dx, dy) = if len > 1.0 {
        (ax / len, ay / len)
    } else {
        (ax, ay)
    };
    let reach = radius - thumb;
    disc(
        batch,
        atlas,
        (cx + dx * reach, cy + dy * reach),
        thumb,
        rgba(245, 245, 245, alpha as u8),
    );
}

/// How many quads make the stick's circles; 32 reads as round at any button size.
const STICK_SEGMENTS: usize = 32;

fn on_circle(centre: (f32, f32), radius: f32, i: usize) -> (f32, f32) {
    let a = (i % STICK_SEGMENTS) as f32 / STICK_SEGMENTS as f32 * std::f32::consts::TAU;
    (centre.0 + radius * a.cos(), centre.1 + radius * a.sin())
}

fn disc(batch: &mut HudBatch, atlas: &HudAtlas, centre: (f32, f32), radius: f32, color: u32) {
    for i in 0..STICK_SEGMENTS {
        let (x0, y0) = on_circle(centre, radius, i);
        let (x1, y1) = on_circle(centre, radius, i + 1);
        batch.fill_quad(
            atlas,
            [[centre.0, centre.1], [x0, y0], [x1, y1], [x0, y0]],
            color,
        );
    }
}

fn ring(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    centre: (f32, f32),
    inner: f32,
    outer: f32,
    color: u32,
) {
    for i in 0..STICK_SEGMENTS {
        let (ix0, iy0) = on_circle(centre, inner, i);
        let (ix1, iy1) = on_circle(centre, inner, i + 1);
        let (ox0, oy0) = on_circle(centre, outer, i);
        let (ox1, oy1) = on_circle(centre, outer, i + 1);
        batch.fill_quad(
            atlas,
            [[ix0, iy0], [ox0, oy0], [ox1, oy1], [ix1, iy1]],
            color,
        );
    }
}

/// Vanilla's button sprite, nine-sliced to the touch button's size: highlighted while held, and see-through
/// enough to keep the world behind it in sight.
pub fn draw_button(batch: &mut HudBatch, atlas: &HudAtlas, r: Rect, held: bool, opacity: f32) {
    let sprite = if held {
        "gui/sprites/widget/button_highlighted"
    } else {
        "gui/sprites/widget/button"
    };
    let alpha = (if held { 230.0 } else { 170.0 }) * opacity.clamp(0.0, 1.0);
    crate::ui::nine_slice(
        batch,
        atlas,
        sprite,
        r[0],
        r[1],
        r[2],
        r[3],
        rgba(255, 255, 255, alpha as u8),
    );
}

/// The size of a mark's cell on a button `r`: two GUI pixels on the old 26-pixel buttons, growing with them.
pub fn cell_size(r: Rect) -> f32 {
    (r[2] / 13.0).round().max(2.0)
}

/// Draw a mark twice, as Minecraft draws text: a dark copy a little down and right, then the white one.
pub fn shadowed(r: Rect, mut mark: impl FnMut(Rect, u32)) {
    let off = (cell_size(r) / 2.0).max(1.0);
    mark([r[0] + off, r[1] + off, r[2], r[3]], rgba(63, 63, 63, 230));
    mark(r, rgba(255, 255, 255, 240));
}

/// A button's mark, in cells of a few pixels centred on it.
fn glyph(batch: &mut HudBatch, atlas: &HudAtlas, control: Control, r: Rect, color: u32) {
    let (cx, cy) = ((r[0] + r[2] / 2.0).floor(), (r[1] + r[3] / 2.0).floor());
    let u = cell_size(r);
    let cell = |batch: &mut HudBatch, x: f32, y: f32, w: f32, h: f32| {
        batch.fill(atlas, cx + x * u, cy + y * u, w * u, h * u, color);
    };
    // A triangle pointing up, down, left or right: rows (or columns) narrowing to its tip.
    let arrow = |batch: &mut HudBatch, dir: (f32, f32)| {
        for i in 0..4 {
            let i = i as f32;
            let len = 7.0 - 2.0 * i;
            let at = 1.5 - i;
            match dir {
                (0.0, d) => cell(batch, -len / 2.0, at * -d - 0.5, len, 1.0),
                (d, _) => cell(batch, at * -d - 0.5, -len / 2.0, 1.0, len),
            }
        }
    };
    match control {
        Control::Forward => arrow(batch, (0.0, -1.0)),
        Control::Back => arrow(batch, (0.0, 1.0)),
        Control::Left => arrow(batch, (-1.0, 0.0)),
        Control::Right => arrow(batch, (1.0, 0.0)),
        // Sneak: a hollow diamond; jump: a filled one.
        Control::Sneak | Control::Jump => {
            for i in 0..4 {
                let i = i as f32;
                let len = 7.0 - 2.0 * i;
                for y in [-i - 0.5, i - 0.5] {
                    if control == Control::Jump {
                        cell(batch, -len / 2.0, y, len, 1.0);
                    } else {
                        cell(batch, -len / 2.0, y, 1.0, 1.0);
                        cell(batch, len / 2.0 - 1.0, y, 1.0, 1.0);
                    }
                }
            }
        }
        // Perspective: an eye.
        Control::Perspective => {
            cell(batch, -2.5, -1.5, 5.0, 1.0);
            cell(batch, -3.5, -0.5, 1.0, 1.0);
            cell(batch, -0.5, -0.5, 1.0, 1.0);
            cell(batch, 2.5, -0.5, 1.0, 1.0);
            cell(batch, -2.5, 0.5, 5.0, 1.0);
        }
        // Chat: a speech bubble with its tail.
        Control::Chat => {
            cell(batch, -3.0, -2.5, 6.0, 4.0);
            cell(batch, -2.0, 1.5, 1.0, 1.0);
        }
        // Pause: two bars.
        Control::Pause => {
            cell(batch, -2.0, -2.5, 1.5, 5.0);
            cell(batch, 0.5, -2.5, 1.5, 5.0);
        }
        // Hide HUD: a frame with a slash through it.
        Control::HideHud => {
            cell(batch, -3.5, -2.5, 7.0, 1.0);
            cell(batch, -3.5, 1.5, 7.0, 1.0);
            cell(batch, -3.5, -2.5, 1.0, 5.0);
            cell(batch, 2.5, -2.5, 1.0, 5.0);
            for i in 0..5 {
                let i = i as f32;
                cell(batch, -2.5 + i, 2.0 - i, 1.0, 1.0);
            }
        }
        // The inventory: three dots, as Pocket Edition's button beside the hotbar.
        Control::Inventory => {
            for x in [-3.0, -0.5, 2.0] {
                cell(batch, x, -0.5, 1.0, 1.0);
            }
        }
        Control::Hotbar(_) | Control::Custom(_) | Control::Stick => {}
    }
}
