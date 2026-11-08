//! The overlay, drawn like MHGU's own HUD: white names with a dark outline on a dark strip that
//! fades out, gauges like the hunter's health gauge (green, red for health just lost, dark track,
//! pointed end), and the game's player colours (P1 red, P2 blue, P3 yellow, P4 green).
use core::fmt::Write;

use forge::{imgui, sys::imgui::ImVec2};

use crate::game::Name;
use crate::settings::{Display, Settings};
use crate::stats::{self, Fight, Row};

#[derive(Clone, Copy)]
pub struct Fill {
    pub center: [u8; 3],
    pub edge: [u8; 3],
}
// Health gauge green.
pub const MONSTER_FILL: Fill = Fill { center: [120, 224, 70], edge: [30, 120, 30] };
// MHGU player colours, by hunter slot.
pub const PLAYER_FILLS: [Fill; 4] = [
    Fill { center: [236, 76, 64], edge: [130, 24, 20] },
    Fill { center: [70, 170, 240], edge: [20, 80, 150] },
    Fill { center: [250, 210, 60], edge: [160, 110, 10] },
    Fill { center: [100, 210, 90], edge: [30, 110, 30] },
];
// Palicos and unknown slots.
pub const PALICO_FILL: Fill = Fill { center: [210, 200, 180], edge: [110, 100, 80] };
// Defaults shown in the settings for "Your bar" / "Other hunters" (= P1 colour / P2 colour).
pub const MY_FILL: Fill = PLAYER_FILLS[0];
pub const OTHERS_FILL: Fill = PLAYER_FILLS[1];

const RECENT: [u8; 3] = [214, 40, 34];
const TRACK: u32 = 0xE0_0A0A0A; // ABGR
const FRAME: u32 = 0xF0_000000;
const RIM: u32 = 0xB4_C8C8BE; // light rim of the gauges
const TEXT: u32 = 0xFF_FFFFFF;
const SHADOW: u32 = 0xD2_000000;

// Sizes for a 2160-pixel-high screen (4K), scaled with the screen height and the Size setting.
const REFERENCE_HEIGHT: f32 = 2160.0;
const FONT: f32 = 34.0;
const BAR_WIDTH: f32 = 760.0;
const MAX_ICONS: usize = 3;

fn rgba(c: [u8; 3], a: u8) -> u32 {
    imgui::color(c[0], c[1], c[2], a)
}

fn edge_of(c: [u8; 3]) -> [u8; 3] {
    c.map(|v| (v as f32 * 0.45) as u8)
}

fn fill_from(custom: Option<[u8; 3]>, fallback: Fill) -> Fill {
    match custom {
        Some(c) => Fill { center: c, edge: edge_of(c) },
        None => fallback,
    }
}

/// A short text built on the stack.
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> Text<N> {
    pub const fn new() -> Self {
        Self { bytes: [0; N], len: 0 }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(N - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

static mut CUSTOM_FONT: Option<imgui::ImFont> = None;

/// Set once in forge_onImGuiInit, before any drawing, then only read on the render thread.
pub fn set_font(font: imgui::ImFont) {
    unsafe { CUSTOM_FONT = Some(font) };
}

fn font() -> imgui::ImFont {
    match unsafe { &*(&raw const CUSTOM_FONT) } {
        Some(f) => *f,
        None => imgui::ImFont::current(),
    }
}

/// Everything the overlay shows, prepared on the game thread.
#[derive(Clone, Copy)]
pub struct View {
    pub object: u32,
    pub name: &'static str,
    pub hp: i32,
    pub max_hp: i32,
    pub weak: [u8; 3],
    pub fight: Fight,
    /// Clock time DPS is measured to.
    pub end: f32,
}

struct Painter {
    draw: imgui::DrawList,
    font: imgui::ImFont,
    font_size: f32,
    s: f32,
}

impl Painter {
    fn text_width(&self, text: &str) -> f32 {
        imgui::push_font(&self.font, Some(self.font_size));
        let current = imgui::ImFont::current_size();
        let w = imgui::calc_text_size(text, false, 0.0).x;
        imgui::pop_font();
        if current > 0.0 { w * self.font_size / current } else { w }
    }

    /// White text with a dark outline, like the game's HUD text.
    fn text(&self, x: f32, y: f32, text: &str) {
        let o = (2.0 * self.s).max(1.0);
        for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (o, o)] {
            self.draw.add_text_with_font(&self.font, self.font_size, ImVec2::new(x + dx, y + dy), SHADOW, text);
        }
        self.draw.add_text_with_font(&self.font, self.font_size, ImVec2::new(x, y), TEXT, text);
    }

    /// The dark name strip of the HUD: solid on the left, fading to nothing on the right.
    fn strip(&self, x: f32, y: f32, w: f32, h: f32) {
        let solid = 0xB4_000000;
        let clear = 0x00_000000;
        self.draw.add_rect_filled_multicolor(ImVec2::new(x, y), ImVec2::new(x + w, y + h), solid, clear, clear, solid);
    }

    /// A gauge like the hunter's health gauge. Returns nothing; `recent` is the red part.
    fn gauge(&self, x: f32, y: f32, w: f32, h: f32, fraction: f32, recent: f32, fill: Fill) {
        let s = self.s;
        let d = &self.draw;
        let fraction = fraction.clamp(0.0, 1.0);
        let recent = recent.clamp(0.0, 1.0);
        let mid = y + h / 2.0;
        let tip = h * 0.9;
        // Outer dark frame including the pointed right end, then the light rim on top of it.
        let outer = [
            ImVec2::new(x - 2.0 * s, y - 2.0 * s),
            ImVec2::new(x + w, y - 2.0 * s),
            ImVec2::new(x + w + tip + 2.0 * s, mid),
            ImVec2::new(x + w, y + h + 2.0 * s),
            ImVec2::new(x - 2.0 * s, y + h + 2.0 * s),
        ];
        d.add_convex_poly_filled(&outer, FRAME);
        // Track.
        let track = [ImVec2::new(x, y), ImVec2::new(x + w, y), ImVec2::new(x + w + tip, mid), ImVec2::new(x + w, y + h), ImVec2::new(x, y + h)];
        d.add_convex_poly_filled(&track, TRACK);
        // Red: health just lost.
        let fill_end = x + w * fraction;
        let recent_end = x + w * recent;
        if recent_end > fill_end {
            d.add_rect_filled(ImVec2::new(fill_end, y), ImVec2::new(recent_end, y + h), rgba(RECENT, 255), 0.0);
        }
        // Fill: light in the middle, darker on the edges.
        if fill_end > x {
            let (c, e) = (rgba(fill.center, 255), rgba(fill.edge, 255));
            d.add_rect_filled_multicolor(ImVec2::new(x, y), ImVec2::new(fill_end, mid), e, e, c, c);
            d.add_rect_filled_multicolor(ImVec2::new(x, mid), ImVec2::new(fill_end, y + h), c, c, e, e);
            // Shine along the top.
            d.add_line(ImVec2::new(x, y + h * 0.25), ImVec2::new(fill_end, y + h * 0.25), 0x50_FFFFFF, (h * 0.15).max(1.0));
        }
        // Light rim around the gauge.
        let rim = [
            ImVec2::new(x - 1.0 * s, y - 1.0 * s),
            ImVec2::new(x + w, y - 1.0 * s),
            ImVec2::new(x + w + tip + 1.0 * s, mid),
            ImVec2::new(x + w, y + h + 1.0 * s),
            ImVec2::new(x - 1.0 * s, y + h + 1.0 * s),
        ];
        d.add_polyline(&rim, RIM, (1.2 * s).max(1.0), forge::sys::imgui::ImDrawFlags::Closed);
    }

    /// A small element icon (weakness) in a dark round badge.
    fn element(&self, cx: f32, cy: f32, r: f32, element: u8) {
        let d = &self.draw;
        d.add_circle_filled(ImVec2::new(cx, cy), r, 0xC8_000000, 24);
        d.add_circle(ImVec2::new(cx, cy), r, RIM, 24, (1.2 * self.s).max(1.0));
        let p = |x: f32, y: f32| ImVec2::new(cx + x * r, cy + y * r);
        match element {
            0 => {
                // Fire: a flame.
                d.add_convex_poly_filled(&[p(0.0, -0.75), p(0.45, 0.0), p(0.4, 0.4), p(0.0, 0.65), p(-0.4, 0.4), p(-0.45, 0.0)], imgui::color(236, 80, 40, 255));
                d.add_convex_poly_filled(&[p(0.0, -0.1), p(0.22, 0.25), p(0.0, 0.5), p(-0.22, 0.25)], imgui::color(255, 210, 80, 255));
            }
            1 => {
                // Water: a drop.
                d.add_circle_filled(p(0.0, 0.2), r * 0.42, imgui::color(60, 150, 240, 255), 20);
                d.add_triangle_filled(p(0.0, -0.7), p(0.4, 0.05), p(-0.4, 0.05), imgui::color(60, 150, 240, 255));
            }
            2 => {
                // Ice: a snowflake.
                let c = imgui::color(150, 230, 250, 255);
                let t = (2.0 * self.s).max(1.0);
                d.add_line(p(0.0, -0.7), p(0.0, 0.7), c, t);
                d.add_line(p(-0.6, -0.35), p(0.6, 0.35), c, t);
                d.add_line(p(-0.6, 0.35), p(0.6, -0.35), c, t);
            }
            3 => {
                // Thunder: a bolt.
                let c = imgui::color(250, 220, 60, 255);
                d.add_triangle_filled(p(0.15, -0.75), p(-0.4, 0.1), p(0.05, 0.1), c);
                d.add_triangle_filled(p(-0.05, -0.1), p(0.4, -0.1), p(-0.15, 0.75), c);
            }
            _ => {
                // Dragon: a crest.
                let c = imgui::color(190, 70, 200, 255);
                d.add_convex_poly_filled(&[p(0.0, -0.7), p(0.5, -0.1), p(0.0, 0.7), p(-0.5, -0.1)], c);
                d.add_convex_poly_filled(&[p(0.0, -0.3), p(0.2, -0.05), p(0.0, 0.3), p(-0.2, -0.05)], imgui::color(40, 10, 50, 255));
            }
        }
    }

    /// The weapon icon of a hunter (MHGenDatabase classic artwork, drawn pixel by pixel).
    fn weapon(&self, left: f32, top: f32, size: f32, weapon: u8) {
        let index = if weapon == 15 { 15 } else { weapon as usize };
        let Some(icon) = crate::icons::ICONS.get(index) else { return };
        if icon.width == 0 || icon.height == 0 || weapon == 0xFF {
            return;
        }
        let (lw, lh) = ((icon.width / 2) as usize, (icon.height / 2) as usize);
        let cell = size / lw as f32;
        let width = icon.width as usize;
        let block = |lx: usize, ly: usize| {
            let px = ((ly * 2).min(icon.height as usize - 1) * width + (lx * 2).min(width - 1)) * 4;
            (icon.pixels[px], icon.pixels[px + 1], icon.pixels[px + 2], icon.pixels[px + 3])
        };
        for ly in 0..lh {
            let mut lx = 0;
            while lx < lw {
                let px = block(lx, ly);
                if px.3 == 0 {
                    lx += 1;
                    continue;
                }
                let start = lx;
                lx += 1;
                while lx < lw && block(lx, ly) == px {
                    lx += 1;
                }
                self.draw.add_rect_filled(
                    ImVec2::new(left + start as f32 * cell, top + ly as f32 * cell),
                    ImVec2::new(left + lx as f32 * cell, top + (ly + 1) as f32 * cell),
                    imgui::color(px.0, px.1, px.2, px.3),
                    0.0,
                );
            }
        }
    }
}

/// The red "health just lost" part of the monster gauge: it holds for a moment, then drains.
struct Recent {
    object: u32,
    recent: f32,
    last: f32,
    hold: f32,
}
static RECENT_STATE: crate::game::Shared<Recent> = crate::game::Shared::new(Recent { object: 0, recent: 0.0, last: 0.0, hold: 0.0 });

fn recent(object: u32, fraction: f32, dt: f32) -> f32 {
    RECENT_STATE.with(|r| {
        if object != r.object || fraction > r.recent {
            *r = Recent { object, recent: fraction, last: fraction, hold: 0.0 };
        } else if fraction < r.last {
            r.hold = 0.8;
        } else if r.hold > 0.0 {
            r.hold -= dt;
        } else {
            r.recent = (r.recent - 0.25 * dt).max(fraction);
        }
        r.last = fraction;
        r.recent
    })
}

fn row_name(row: &Row) -> Name {
    if !row.name.is_empty() {
        return row.name;
    }
    if row.local {
        return Name::from_str("You");
    }
    match row.slot {
        0 => Name::from_str("Hunter 1"),
        1 => Name::from_str("Hunter 2"),
        2 => Name::from_str("Hunter 3"),
        3 => Name::from_str("Hunter 4"),
        _ => Name::from_str("Palico"),
    }
}

pub fn overlay(view: &View, settings: &Settings) {
    let io = imgui::get_io();
    let (sw, sh) = (io.DisplaySize.x, io.DisplaySize.y);
    if sw < 320.0 || sh < 240.0 {
        return;
    }
    let draw = imgui::DrawList::foreground();
    if draw.raw().is_null() {
        return;
    }
    let s = sh / REFERENCE_HEIGHT * settings.scale.clamp(0.5, 2.0);
    let p = Painter { draw, font: font(), font_size: FONT * s, s };
    let fs = p.font_size;
    let dt = if io.DeltaTime.is_finite() && io.DeltaTime > 0.0 { io.DeltaTime } else { 1.0 / 60.0 };

    let width = BAR_WIDTH * s;
    let icon = fs * 1.6;
    let gap = 8.0 * s;
    let icon_area = (icon + gap) * MAX_ICONS as f32;
    let tip = fs * 0.5;
    let total = icon_area + width + tip + 8.0 * s;
    // Position: right edge and top as a share of the screen (default: right side, below the map).
    let left = (sw * settings.x - total).max(0.0);
    let mut y = sh * settings.y;

    // ---- monster ----
    let fraction = if view.max_hp > 0 { view.hp as f32 / view.max_hp as f32 } else { 0.0 };
    let red = recent(view.object, fraction, dt);
    let bar_x = left + icon_area;
    p.strip(bar_x - gap, y, width * 0.75, fs * 1.15);
    let mut values = Text::<64>::new();
    let pct = (fraction * 100.0 + 0.5) as u32;
    let _ = write!(values, "{} / {}  ({}%)", view.hp.max(0), view.max_hp, pct);
    p.text(bar_x + 4.0 * s, y, view.name);
    p.text(bar_x + width - p.text_width(values.as_str()), y, values.as_str());
    let bar_h = fs * 0.5;
    let bar_y = y + fs * 1.2;
    // Weakness icons, right-aligned next to the gauge.
    let weak: usize = view.weak.iter().filter(|e| **e != 0xFF).count();
    let r = icon * 0.45;
    for (i, e) in view.weak.iter().filter(|e| **e != 0xFF).enumerate() {
        let cx = left + icon_area - gap - (weak - i) as f32 * (icon + gap) + icon / 2.0 + gap / 2.0;
        p.element(cx, bar_y + bar_h / 2.0 - fs * 0.35, r, *e);
    }
    p.gauge(bar_x, bar_y, width, bar_h, fraction, red, fill_from(settings.monster_color, MONSTER_FILL));
    y = bar_y + bar_h + 22.0 * s;

    // ---- party damage ----
    let total_damage = view.fight.total();
    if !settings.show_party || total_damage == 0 {
        return;
    }
    for row in &view.fight.rows[..view.fight.count] {
        let share = row.damage as f32 / total_damage as f32;
        let fill = if row.slot >= 4 {
            PALICO_FILL
        } else if row.local {
            fill_from(settings.my_color, PLAYER_FILLS[row.slot as usize])
        } else {
            fill_from(settings.others_color, PLAYER_FILLS[row.slot as usize])
        };
        let name = row_name(row);
        let mut values = Text::<64>::new();
        let dps = stats::dps(row, view.end);
        let pct = (share * 100.0 + 0.5) as u32;
        let _ = match settings.display {
            Display::Damage => write!(values, "{}  ({}%)", row.damage, pct),
            Display::Dps => write!(values, "{:.1} DPS  ({}%)", dps, pct),
            Display::Both => write!(values, "{}   {:.1} DPS  ({}%)", row.damage, dps, pct),
        };
        // Player-colour marker on the strip, like the game's party list.
        p.strip(bar_x - gap, y, width * 0.75, fs * 1.15);
        p.draw.add_rect_filled(ImVec2::new(bar_x - gap, y + 2.0 * s), ImVec2::new(bar_x - gap + 6.0 * s, y + fs * 1.15 - 2.0 * s), rgba(fill.center, 255), 0.0);
        p.text(bar_x + 4.0 * s, y, name.as_str());
        p.text(bar_x + width - p.text_width(values.as_str()), y, values.as_str());
        let bar_h = fs * 0.32;
        let bar_y = y + fs * 1.2;
        p.weapon(left + icon_area - gap - icon, y + (fs * 1.2 + bar_h - icon) / 2.0, icon, row.weapon);
        p.gauge(bar_x, bar_y, width, bar_h, share, share, fill);
        y = bar_y + bar_h + 16.0 * s;
    }
}

/// Example data to see the overlay (and try the settings) outside a hunt.
pub fn preview() -> View {
    let t = stats::now();
    let step = ((t as u32) % 6) as i32;
    let mut fight = Fight::EMPTY;
    let rows = [(0u8, true, "Sarah", 3u8, 6200u64), (1, false, "Hunter 2", 13, 4900), (2, false, "Hunter 3", 4, 2500)];
    for (i, (slot, local, name, weapon, damage)) in rows.iter().enumerate() {
        fight.rows[i] = Row { slot: *slot, owner: 0, local: *local, weapon: *weapon, name: Name::from_str(name), damage: *damage, since: t - 65.0 };
    }
    fight.count = 3;
    View {
        object: 1,
        name: "Rathalos",
        hp: if step == 5 { 0 } else { 9400 - step * 500 },
        max_hp: 21000,
        weak: [4, 3, 0xFF],
        fight,
        end: t,
    }
}
