//! Party damage: two hooks in the game's hit code (MHGU 1.4.0), adapted from HuntLog (MIT).
//! The collision hook learns who hit what; the damage hook, called inside it on the same thread,
//! gets the damage the game actually applied. Both chain with other plugins (e.g. Damage Numbers).
use core::{
    mem::{self, MaybeUninit},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};
use forge::log::prelude::{error, info};

use crate::game::{self, Shared, read_u32, read_u8};
use crate::hook_policy;

const FN_COLLISION: u32 = 0x0017_0968; // (check, attack, monster, point, body, attack point, part, shape)
const FN_DAMAGE: u32 = 0x0009_D6F8; // (monster, part slot, raw damage, mode) -> applied damage
const FN_LOCAL_PLAYER: u32 = 0x0027_6278;

pub static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Who is hitting what, kept on the collision hook's stack while the game applies the damage.
#[derive(Clone, Copy)]
struct Contact {
    monster: u32,
    slot: u32,
    owner: u32,
    player_slot: u8,
    local: bool,
    valid: bool,
}

#[derive(Clone, Copy)]
struct Binding {
    thread: u32,
    contact: *const Contact,
}
struct Bindings([Binding; 8]);
// The pointer is only used while the collision call that owns it has not returned.
unsafe impl Send for Bindings {}
static BINDINGS: Shared<Bindings> = Shared::new(Bindings([Binding { thread: 0, contact: ptr::null() }; 8]));

// Forge's hook result on MHGURecomp is a 76-byte structure, kept but never read.
#[repr(C)]
struct RuntimeHook([u32; 19]);
static mut COLLISION_HOOK: MaybeUninit<RuntimeHook> = MaybeUninit::uninit();
static mut DAMAGE_HOOK: MaybeUninit<RuntimeHook> = MaybeUninit::uninit();
static mut COLLISION_ORIGINAL: u32 = 0;
static mut DAMAGE_ORIGINAL: u32 = 0;

unsafe extern "C" {
    #[link_name = "forge_hook_create"]
    fn create_hook_raw(output: *mut RuntimeHook, target: u32, detour: u32, original: *mut u32) -> *mut RuntimeHook;
}

fn thread_key() -> u32 {
    let value: u32;
    // AArch32 TPIDRURO: tells calling threads apart without touching OS structures.
    unsafe { core::arch::asm!("mrc p15, 0, {value}, c13, c0, 3", value = out(reg) value, options(nostack, preserves_flags)) };
    value
}

unsafe fn entry_words(target: u32) -> [u32; 2] {
    unsafe { [read_u32(target), read_u32(target + 4)] }
}

/// The game code at both hook points is the one this plugin was made for (1.4.0), possibly
/// already hooked by another Forge plugin.
pub fn compatible(base: u32) -> bool {
    for (offset, stock, chain_safe) in [
        (FN_COLLISION, [0xE92D41F0, 0xE24DD010], true),
        (FN_DAMAGE, [0xE92D4BF0, 0xED2D8B02], true),
        (FN_LOCAL_PLAYER, [0xE92D4010, 0xE1A04000], false),
    ] {
        let actual = unsafe { entry_words(base + offset) };
        if !hook_policy::entry_allowed(actual, stock, chain_safe) {
            error!("MHGU Overlay: unknown game code at main+0x{offset:08X} (not MHGU 1.4.0?); party damage off");
            return false;
        }
    }
    true
}

unsafe fn create(target: u32, detour: u32, storage: *mut MaybeUninit<RuntimeHook>, original: *mut u32) -> bool {
    let mut hook: RuntimeHook = unsafe { mem::zeroed() };
    unsafe {
        ptr::write(original, 0);
        create_hook_raw(&raw mut hook, target, detour, original);
        if hook_policy::hook_created(entry_words(target), detour, ptr::read(original)) {
            ptr::write(storage, MaybeUninit::new(hook));
            true
        } else {
            false
        }
    }
}

/// Installs both hooks, or neither (the first is undone if the second fails).
pub fn install(base: u32) -> bool {
    if !compatible(base) {
        return false;
    }
    let collision_before = unsafe { entry_words(base + FN_COLLISION) };
    let ok = unsafe {
        create(base + FN_COLLISION, collision as *const () as u32, &raw mut COLLISION_HOOK, &raw mut COLLISION_ORIGINAL)
    };
    if !ok {
        error!("MHGU Overlay: collision hook failed; party damage off");
        return false;
    }
    let ok = unsafe { create(base + FN_DAMAGE, damage as *const () as u32, &raw mut DAMAGE_HOOK, &raw mut DAMAGE_ORIGINAL) };
    if !ok {
        // Put the collision entry back exactly as it was (including another plugin's jump).
        let bytes = collision_before.as_ptr().cast::<core::ffi::c_void>();
        let mut patch = forge::sys::patch::Patch {
            addr: base + FN_COLLISION,
            size: 8,
            patch_bytes: bytes,
            original_bytes: bytes,
            enabled: true,
        };
        unsafe { forge::sys::patch::forge_patch_disable(&raw mut patch) };
        error!("MHGU Overlay: damage hook failed; party damage off");
        return false;
    }
    INSTALLED.store(true, Ordering::Release);
    info!("MHGU Overlay: party damage hooks installed");
    true
}

unsafe extern "C" fn collision(
    check: u32,
    attack: u32,
    monster: u32,
    point: u32,
    body: u32,
    attack_point: u32,
    part: u32,
    shape: u32,
) -> u32 {
    let base = crate::GAME_BASE.load(Ordering::Relaxed);
    let mut contact = Contact { monster, slot: 0, owner: 0, player_slot: 0xFF, local: false, valid: false };
    // +0x6D == 1: an attack from a hunter or Palico. Forwarded attacks (online) keep their owner at +0x70.
    if unsafe { read_u8(attack + 0x6D) } == 1 {
        let forwarded = unsafe { read_u8(attack + 0x10) } != 0;
        if !forwarded || unsafe { read_u8(attack + 0x6E) } == 0 {
            let owner = unsafe { read_u32(attack + if forwarded { 0x70 } else { 0x0C }) };
            let player = unsafe { game::local_player(base) };
            contact.owner = owner;
            contact.local = owner == player && player >= 0x10000;
            contact.player_slot = unsafe { game::slot_of(base, owner) };
            contact.slot = unsafe { read_u8(part + 8) } as u32;
            contact.valid = true;
        }
    }
    let thread = thread_key();
    let binding = BINDINGS.with(|b| {
        let index = b.0.iter().position(|e| e.thread == thread).or_else(|| b.0.iter().position(|e| e.thread == 0))?;
        let previous = b.0[index];
        b.0[index] = Binding { thread, contact: &contact };
        Some((index, previous))
    });
    let original: unsafe extern "C" fn(u32, u32, u32, u32, u32, u32, u32, u32) -> u32 =
        unsafe { mem::transmute(COLLISION_ORIGINAL) };
    let result = unsafe { original(check, attack, monster, point, body, attack_point, part, shape) };
    if let Some((index, previous)) = binding {
        BINDINGS.with(|b| b.0[index] = previous);
    }
    result
}

unsafe extern "C" fn damage(monster: u32, slot: u32, raw: i32, mode: i32) -> i32 {
    let original: unsafe extern "C" fn(u32, u32, i32, i32) -> i32 = unsafe { mem::transmute(DAMAGE_ORIGINAL) };
    let result = unsafe { original(monster, slot, raw, mode) };
    if raw > 0 && result > 0 && mode != 2 && slot < 8 {
        let thread = thread_key();
        let contact = BINDINGS.with(|b| b.0.iter().find(|e| e.thread == thread).map(|e| unsafe { *e.contact }));
        if let Some(c) = contact {
            if c.valid && c.monster == monster && c.slot == slot {
                crate::stats::hit(c.monster, c.owner, c.player_slot, c.local, result);
            }
        }
    }
    result
}
