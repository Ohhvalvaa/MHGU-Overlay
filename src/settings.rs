//! Settings: a small text file next to the game saves (savedata:/mhgu_overlay.ini), and the
//! settings window opened from the Forge menu (Insert > Plugin UI > MHGU Overlay settings).
//! Reading or writing never stops the game: if the save folder cannot be opened, defaults are used
//! and changes simply stay for this session.
use core::ffi::CStr;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};

use forge::{
    imgui,
    log::prelude::{info, warn},
    mt::fs::{MtFile, OpenMode},
    mt::object::Object,
    sys::imgui::{ImGuiColorEditFlags, ImGuiComboFlags, ImGuiSelectableFlags, ImGuiSliderFlags, ImGuiWindowFlags, ImVec2},
};

use crate::game::Shared;

#[derive(Clone, Copy, PartialEq)]
pub enum Display {
    Damage = 0,
    Dps = 1,
    Both = 2,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Settings {
    pub show_party: bool,
    pub display: Display,
    /// Size relative to the default (which is relative to the screen height).
    pub scale: f32,
    /// Right edge of the panel and its top, as a share of the screen width / height.
    pub x: f32,
    pub y: f32,
    pub monster_color: Option<[u8; 3]>,
    pub my_color: Option<[u8; 3]>,
    pub others_color: Option<[u8; 3]>,
}

impl Settings {
    pub const DEFAULT: Self = Self {
        show_party: true,
        display: Display::Both,
        scale: 1.0,
        x: 0.985,
        y: 0.32,
        monster_color: None,
        my_color: None,
        others_color: None,
    };

    fn parse(text: &str) -> Self {
        let mut s = Self::DEFAULT;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "show_party_damage" => s.show_party = value != "false",
                "damage_meter_shows" => {
                    s.display = match value {
                        "damage" => Display::Damage,
                        "dps" => Display::Dps,
                        _ => Display::Both,
                    }
                }
                "size" => {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            s.scale = v.clamp(0.5, 2.0);
                        }
                    }
                }
                "right_edge" | "top" => {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let v = v.clamp(0.0, 1.0);
                            if key == "top" { s.y = v } else { s.x = v }
                        }
                    }
                }
                "monster_colour" => s.monster_color = hex(value),
                "your_colour" => s.my_color = hex(value),
                "others_colour" => s.others_color = hex(value),
                _ => {}
            }
        }
        s
    }

    fn write(&self, out: &mut impl Write) -> core::fmt::Result {
        writeln!(out, "# MHGU Overlay settings (also changed in game: Insert > Plugin UI)")?;
        writeln!(out, "show_party_damage = {}", self.show_party)?;
        let display = match self.display {
            Display::Damage => "damage",
            Display::Dps => "dps",
            Display::Both => "both",
        };
        writeln!(out, "damage_meter_shows = {display}")?;
        writeln!(out, "size = {:.2}", self.scale)?;
        writeln!(out, "right_edge = {:.3}", self.x)?;
        writeln!(out, "top = {:.3}", self.y)?;
        for (key, color) in [("monster_colour", self.monster_color), ("your_colour", self.my_color), ("others_colour", self.others_color)] {
            match color {
                Some([r, g, b]) => writeln!(out, "{key} = #{r:02X}{g:02X}{b:02X}")?,
                None => writeln!(out, "{key} = default")?,
            }
        }
        Ok(())
    }
}

fn hex(value: &str) -> Option<[u8; 3]> {
    let v = value.strip_prefix('#')?;
    if v.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(v, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

pub static SETTINGS: Shared<Settings> = Shared::new(Settings::DEFAULT);
static LOADED: AtomicBool = AtomicBool::new(false);
static WINDOW_OPEN: AtomicBool = AtomicBool::new(false);

pub fn get() -> Settings {
    SETTINGS.with(|s| *s)
}

// ---------- file ----------

const FILE: &str = "mhgu_overlay.ini";

/// Text buffer for the settings file (it is a few hundred bytes).
struct Buffer {
    bytes: [u8; 1024],
    len: usize,
}
impl Write for Buffer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > self.bytes.len() {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// "savedata:/mhgu_overlay.ini" as a C string.
fn path(out: &mut [u8; 96]) -> Option<&CStr> {
    let mount = unsafe { CStr::from_ptr(forge::sys::fs::forge_fs_getSaveDataMountPoint()) }.to_bytes();
    let n = mount.len() + 2 + FILE.len();
    if n + 1 > out.len() {
        return None;
    }
    out[..mount.len()].copy_from_slice(mount);
    out[mount.len()..mount.len() + 2].copy_from_slice(b":/");
    out[mount.len() + 2..n].copy_from_slice(FILE.as_bytes());
    out[n] = 0;
    CStr::from_bytes_with_nul(&out[..=n]).ok()
}

/// Runs `f` with the save folder mounted; does nothing if it cannot be mounted.
fn with_savedata<R>(f: impl FnOnce(&CStr) -> R) -> Option<R> {
    if !unsafe { forge::sys::fs::forge_fs_mountSaveData() } {
        return None;
    }
    let mut buf = [0u8; 96];
    let result = path(&mut buf).map(f);
    unsafe { forge::sys::fs::forge_fs_unmountSaveData() };
    result
}

/// Loads the settings once (the save folder can be unavailable at the very start: retried later).
pub fn load_once() {
    if LOADED.load(Ordering::Acquire) {
        return;
    }
    let loaded = with_savedata(|path| {
        if !unsafe { forge::sys::fs::forge_fs_fileExists(path.as_ptr()) } {
            return Some(Settings::DEFAULT);
        }
        let file = MtFile::open(path, OpenMode::Read).ok()?;
        let mut buf = [0u8; 1024];
        let size = file.size().min(buf.len());
        let read = file.read(&mut buf[..size]);
        file.close();
        file.destroy();
        Some(Settings::parse(core::str::from_utf8(&buf[..read]).unwrap_or("")))
    });
    if let Some(Some(settings)) = loaded {
        SETTINGS.with(|s| *s = settings);
        LOADED.store(true, Ordering::Release);
        info!("MHGU Overlay: settings loaded");
    }
}

fn save(settings: Settings) {
    let mut text = Buffer { bytes: [0; 1024], len: 0 };
    if settings.write(&mut text).is_err() {
        return;
    }
    let saved = with_savedata(|path| {
        let file = MtFile::open(path, OpenMode::Write).ok()?;
        let written = file.write(&text.bytes[..text.len]);
        file.close();
        file.destroy();
        Some(written == text.len)
    });
    if saved != Some(Some(true)) {
        warn!("MHGU Overlay: could not save settings (they stay until the game is closed)");
    }
}

// ---------- window ----------

/// Inside Forge's Plugin UI window: only a button, so it does not mix with other plugins.
pub fn draw_menu_button() {
    imgui::push_id(imgui::Id::Str(c"mhgu_overlay"));
    if imgui::button(c"MHGU Overlay settings", ImVec2::new(0.0, 0.0)) {
        WINDOW_OPEN.fetch_xor(true, Ordering::Relaxed);
    }
    imgui::pop_id();
}

pub fn window_open() -> bool {
    WINDOW_OPEN.load(Ordering::Relaxed) && forge::graphics::Graphics::is_menu_visible()
}

/// The settings window: every change is saved at once (sliders when released).
pub fn draw_window() {
    if !window_open() {
        return;
    }
    let mut open = true;
    let flags = ImGuiWindowFlags(ImGuiWindowFlags::AlwaysAutoResize.0 | ImGuiWindowFlags::NoCollapse.0);
    if imgui::begin(c"MHGU Overlay settings", Some(&mut open), flags) {
        let mut s = get();
        let mut changed = false;

        changed |= imgui::checkbox(c"Show party damage meter", &mut s.show_party);

        let names: [(&CStr, Display); 3] = [(c"Damage", Display::Damage), (c"DPS", Display::Dps), (c"Damage + DPS", Display::Both)];
        let current = names.iter().find(|(_, d)| *d == s.display).map_or(c"Damage + DPS", |(n, _)| *n);
        if imgui::begin_combo(c"Damage meter shows", current, ImGuiComboFlags::None) {
            for (name, d) in names {
                if imgui::selectable(name, s.display == d, ImGuiSelectableFlags::None, ImVec2::new(0.0, 0.0)) {
                    s.display = d;
                    changed = true;
                }
            }
            imgui::end_combo();
        }

        imgui::slider_float(c"Size", &mut s.scale, 0.5, 2.0, ImGuiSliderFlags::None);
        changed |= imgui::is_item_deactivated_after_edit();
        imgui::text_disabled(c"Size is relative to your screen height (1.00 = default).");
        imgui::slider_float(c"Right edge", &mut s.x, 0.2, 1.0, ImGuiSliderFlags::None);
        changed |= imgui::is_item_deactivated_after_edit();
        imgui::slider_float(c"Top", &mut s.y, 0.0, 0.9, ImGuiSliderFlags::None);
        changed |= imgui::is_item_deactivated_after_edit();
        if imgui::button(c"Reset position", ImVec2::new(0.0, 0.0)) {
            s.x = Settings::DEFAULT.x;
            s.y = Settings::DEFAULT.y;
            changed = true;
        }

        imgui::separator();
        changed |= color_option(c"Monster bar", &mut s.monster_color, crate::draw::MONSTER_FILL.center);
        changed |= color_option(c"Your bar", &mut s.my_color, crate::draw::MY_FILL.center);
        changed |= color_option(c"Other hunters", &mut s.others_color, crate::draw::OTHERS_FILL.center);
        if imgui::button(c"Reset colours", ImVec2::new(0.0, 0.0)) {
            s.monster_color = None;
            s.my_color = None;
            s.others_color = None;
            changed = true;
        }
        if !crate::hooks::INSTALLED.load(Ordering::Acquire) {
            imgui::separator();
            imgui::text_wrapped(c"Party damage is off: the game code is not the one this overlay knows (MHGU 1.4.0). See the Forge log.");
        }

        // Live while dragging, saved when a change is finished.
        SETTINGS.with(|current| *current = s);
        if changed {
            save(s);
        }
    }
    imgui::end();
    if !open {
        WINDOW_OPEN.store(false, Ordering::Relaxed);
    }
}

/// A colour picker; returns true when the choice is finished and must be saved.
fn color_option(label: &CStr, value: &mut Option<[u8; 3]>, fallback: [u8; 3]) -> bool {
    let c = value.unwrap_or(fallback);
    let mut rgb = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
    let no_inputs = ImGuiColorEditFlags::NoInputs;
    if imgui::color_edit3(label, &mut rgb, no_inputs) {
        *value = Some(rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
    }
    imgui::is_item_deactivated_after_edit()
}
