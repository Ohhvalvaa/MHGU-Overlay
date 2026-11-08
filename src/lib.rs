//! MHGU Overlay: the monster you are fighting (health, weaknesses) and the party's damage and DPS,
//! drawn like MHGU's own HUD. A Forge plugin for MHGURecomp / MHGU 1.4.0.
#![no_std]

extern crate alloc;

mod draw;
mod game;
mod hook_policy;
mod hooks;
mod icons;
mod settings;
mod stats;
mod target;
mod weakness;

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use forge::log::prelude::info;

use game::Shared;

pub(crate) static GAME_BASE: AtomicU32 = AtomicU32::new(0);
static HOOKS_TRIED: AtomicBool = AtomicBool::new(false);

/// After this many seconds without hits the monster leaves the panel (also when dead or captured).
const TARGET_TIMEOUT: f32 = 30.0;

/// What the overlay shows, written by the game thread and read by the render thread.
static VIEW: Shared<Option<draw::View>> = Shared::new(None);

/// The monster being followed and what the update saw of the monsters last time.
struct Tracker {
    quest: u32,
    target: u32,
    last_hit: f32,
    hp: [(u32, i32); 10],
    frames: u32,
}
static TRACKER: Shared<Tracker> =
    Shared::new(Tracker { quest: u32::MAX, target: 0, last_hit: 0.0, hp: [(0, 0); 10], frames: 0 });

#[unsafe(no_mangle)]
pub extern "C" fn forge_onLoad(params: *mut forge::sys::init::PluginInitParams) {
    unsafe {
        (*params).required_version = forge::REQUIRED_VERSION;
        // PluginInitParams: ForgeVersion (6 bytes), then the plugin name (64 bytes).
        let base = params as *mut u8;
        let name = b"MHGU Overlay";
        core::ptr::copy_nonoverlapping(name.as_ptr(), base.add(6), name.len());
        *base.add(6 + name.len()) = 0;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn forge_onInit() {
    let _ = forge::log::init();
    let base = forge::mem::text_addr();
    GAME_BASE.store(base, Ordering::Relaxed);
    info!("MHGU Overlay v{}: main=0x{base:08X}", env!("CARGO_PKG_VERSION"));
}

#[unsafe(no_mangle)]
pub extern "C" fn forge_onUpdate(dt: f32) {
    let base = GAME_BASE.load(Ordering::Relaxed);
    // Hooks go in after every plugin's init, so other plugins on the same functions come first.
    if !HOOKS_TRIED.swap(true, Ordering::AcqRel) {
        hooks::install(base);
    }
    let frames = TRACKER.with(|t| {
        t.frames = t.frames.wrapping_add(1);
        t.frames
    });
    // The save folder may not be ready at the very start: try about once a second until it is.
    if frames % 60 == 1 {
        settings::load_once();
    }

    let quest = unsafe { game::quest_number(base) };
    let new_quest = TRACKER.with(|t| {
        let changed = t.quest != quest;
        if changed {
            *t = Tracker { quest, target: 0, last_hit: 0.0, hp: [(0, 0); 10], frames: t.frames };
        }
        changed
    });
    if new_quest {
        stats::reset();
    }

    let monsters = game::monsters(base);
    let party = unsafe { game::party(base) };
    stats::update(dt, party, &monsters);
    let now = stats::now();

    let target = TRACKER.with(|t| {
        // The monster that just lost health becomes the one followed (hits by monsters count too,
        // as in World, but a monster locked with the target camera always wins).
        for m in monsters.all() {
            if let Some((_, hp)) = t.hp.iter().find(|(o, _)| *o == m.object) {
                if m.hp < *hp {
                    t.target = m.object;
                    t.last_hit = now;
                }
            }
        }
        t.hp = [(0, 0); 10];
        for (i, m) in monsters.all().iter().enumerate() {
            t.hp[i] = (m.object, m.hp);
        }
        if let Some(m) = monsters.all().iter().find(|m| m.selected) {
            if t.target != m.object {
                t.target = m.object;
                t.last_hit = now;
            } else if m.hp > 0 {
                t.last_hit = now;
            }
        }
        match monsters.get(t.target) {
            Some(m) if now - t.last_hit < TARGET_TIMEOUT => Some(*m),
            _ => None,
        }
    });

    let view = target.map(|m| {
        let fight = stats::fight(m.object);
        draw::View {
            object: m.object,
            name: game::name(m.species).unwrap_or("Monster"),
            hp: m.hp,
            max_hp: m.max_hp,
            weak: m.weak,
            end: if m.hp > 0 { now } else { fight.last },
            fight,
        }
    });
    VIEW.with(|v| *v = view);
}

/// Roboto Medium from this mod's romfs; without it the overlay uses Forge's font.
#[unsafe(no_mangle)]
pub extern "C" fn forge_onImGuiInit(_context: *mut core::ffi::c_void, _alloc: *mut core::ffi::c_void, _free: *mut core::ffi::c_void) {
    let path = c"app:/nativeNX/plugins/mhgu-overlay/Roboto-Medium.ttf";
    let font = if unsafe { forge::sys::fs::forge_fs_fileExists(path.as_ptr()) } { forge::imgui::ImFont::load(path) } else { None };
    match font {
        Some(font) => {
            draw::set_font(font);
            info!("MHGU Overlay: font loaded");
        }
        None => forge::log::prelude::warn!("MHGU Overlay: font not found, using Forge's"),
    }
}

/// Inside Forge's Plugin UI window (Insert): the button that opens the settings window.
#[forge::imgui_render]
fn menu() {
    settings::draw_menu_button();
}

/// Every frame: the settings window (while the Forge menu is open) and the overlay.
#[forge::imgui_free_render]
fn render() {
    settings::draw_window();
    let mut view = VIEW.with(|v| *v);
    // With the Forge menu open and nothing to show, an example lets you see the overlay and your changes.
    if view.is_none() && forge::graphics::Graphics::is_menu_visible() {
        view = Some(draw::preview());
    }
    if let Some(view) = view {
        draw::overlay(&view, &settings::get());
    }
}
