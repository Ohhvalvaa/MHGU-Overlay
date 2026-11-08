//! Read-only access to MHGU 1.4.0 game memory: large monsters, hunters and the current quest.
//! Offsets come from HuntLog (MIT, Rikiar73574/mhgu-huntlog) and MHGURecomp (MIT) research;
//! all of them are relative to the main module, so they follow its load address.
use core::sync::atomic::{AtomicBool, Ordering};
use core::{cell::UnsafeCell, ptr};

// ---------- small helpers ----------

/// A value shared between the game thread (update, hooks) and the render thread (ImGui).
/// Held only while copying plugin data, never while calling the game, Forge or ImGui.
pub struct Shared<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}
unsafe impl<T: Send> Sync for Shared<T> {}
impl<T> Shared<T> {
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
        struct Guard<'a>(&'a AtomicBool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = Guard(&self.locked);
        f(unsafe { &mut *self.value.get() })
    }
}

/// Guest heap addresses where game objects live.
pub fn heap(a: u32, n: u32) -> bool {
    a >= 0x0800_0000 && a < 0xa800_0000 && n <= 0xa800_0000 - a
}
pub unsafe fn read<T: Copy>(a: u32) -> T {
    unsafe { ptr::read_volatile(a as *const T) }
}
pub unsafe fn read_u32(a: u32) -> u32 {
    unsafe { read::<u32>(a) }
}
pub unsafe fn read_u8(a: u32) -> u8 {
    unsafe { read::<u8>(a) }
}

// ---------- game globals (offsets from main) ----------

const S_PLAYER: u32 = 0x0188_5B58; // sPlayer *: +0x1c the 4 hunter slots
const S_QUEST: u32 = 0x0188_AD90; // sQuest *: +0xa4 quest data (+0x8 quest number)
const FN_LOCAL_PLAYER: u32 = 0x0027_6278; // (sPlayer) -> local hunter object

/// The local hunter object, or 0 (title, loading and result screens have none).
pub unsafe fn local_player(base: u32) -> u32 {
    let manager = unsafe { read_u32(base + S_PLAYER) };
    if manager < 0x10000 {
        return 0;
    }
    let get: unsafe extern "C" fn(u32) -> u32 = unsafe { core::mem::transmute(base + FN_LOCAL_PLAYER) };
    unsafe { get(manager) }
}

/// The 4 hunter slots (0 = empty).
pub unsafe fn party(base: u32) -> [u32; 4] {
    let manager = unsafe { read_u32(base + S_PLAYER) };
    if manager < 0x10000 {
        return [0; 4];
    }
    core::array::from_fn(|i| unsafe { read_u32(manager + 0x1c + 4 * i as u32) })
}

/// The slot (0-3) of a hunter object, 0xFF for anything else (Palicos).
pub unsafe fn slot_of(base: u32, owner: u32) -> u8 {
    let slots = unsafe { party(base) };
    slots.iter().position(|s| *s == owner && owner >= 0x10000).map_or(0xFF, |i| i as u8)
}

/// The quest being played (its number), 0 outside quests.
pub unsafe fn quest_number(base: u32) -> u32 {
    let quest = unsafe { read_u32(base + S_QUEST) };
    if !heap(quest, 0xa8) || quest & 3 != 0 {
        return 0;
    }
    let data = unsafe { read_u32(quest + 0xa4) };
    if !heap(data, 0xc) || data & 3 != 0 {
        return 0;
    }
    unsafe { read_u32(data + 8) }
}

/// Weapon type of a hunter or Palico object: 0..14 weapons, 15 Palico, 0xFF unknown.
pub fn owner_weapon(owner: u32) -> u8 {
    if owner < 0x10000 {
        return 0xFF;
    }
    let data = unsafe { read_u32(owner + 0x13C4) };
    if data < 0x10000 || data > 0xFFFF_0000 - 0x14C {
        return 0xFF;
    }
    let value = unsafe { read_u8(data + 0x14C) };
    if value <= 0x0F { value } else { 0xFF }
}

#[derive(Clone, Copy)]
pub struct Name {
    pub bytes: [u8; 32],
    pub len: u8,
}
impl Name {
    pub const EMPTY: Self = Self { bytes: [0; 32], len: 0 };
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn from_str(text: &str) -> Self {
        let mut name = Self::EMPTY;
        let n = text.len().min(32);
        name.bytes[..n].copy_from_slice(&text.as_bytes()[..n]);
        name.len = n as u8;
        name
    }
}

/// The name of a hunter (or Palico) object, from the party name blocks.
pub unsafe fn owner_name(base: u32, owner: u32) -> Name {
    if owner < 0x10000 {
        return Name::EMPTY;
    }
    let manager = unsafe { read_u32(base + S_PLAYER) };
    if manager < 0x10000 {
        return Name::EMPTY;
    }
    let local = unsafe { local_player(base) };
    let mut local_fallback = Name::EMPTY;
    for i in 0..5u32 {
        let block = manager + 0x300 + i * 0x560;
        let mut name = Name::EMPTY;
        for j in 0..32u32 {
            let byte = unsafe { read_u8(block + 0x290 + j) };
            if byte == 0 {
                break;
            }
            name.bytes[name.len as usize] = byte;
            name.len += 1;
        }
        if name.is_empty() || core::str::from_utf8(&name.bytes[..name.len as usize]).is_err() {
            continue;
        }
        let owner_ptr = unsafe { read_u32(block + 0x288) };
        if owner_ptr >= 0x10000 {
            if owner_ptr == owner {
                return name;
            }
        } else if owner == local && local_fallback.is_empty() {
            // The local hunter's own block (usual in offline quests) has no owner pointer.
            local_fallback = name;
        }
    }
    local_fallback
}

// ---------- large monsters ----------

#[derive(Clone, Copy)]
pub struct Monster {
    pub object: u32,
    pub species: u16,
    pub hp: i32,
    pub max_hp: i32,
    pub selected: bool,
    /// Up to 3 weaknesses, strongest first: 0 fire, 1 water, 2 ice, 3 thunder, 4 dragon; 0xFF none.
    pub weak: [u8; 3],
}
impl Monster {
    pub const EMPTY: Self = Self { object: 0, species: 0, hp: 0, max_hp: 0, selected: false, weak: [0xFF; 3] };
}

#[derive(Clone, Copy)]
pub struct Monsters {
    pub count: usize,
    pub entries: [Monster; 10],
}
impl Monsters {
    pub const EMPTY: Self = Self { count: 0, entries: [Monster::EMPTY; 10] };
    pub fn all(&self) -> &[Monster] {
        &self.entries[..self.count]
    }
    pub fn get(&self, object: u32) -> Option<&Monster> {
        self.all().iter().find(|m| m.object == object)
    }
}

unsafe extern "C" {
    fn forge_singleton_getInstanceByName(name: *const core::ffi::c_char) -> *mut core::ffi::c_void;
}

struct Direct {
    base: u32,
}
impl Direct {
    fn allowed(&self, a: u32, n: u32) -> bool {
        heap(a, n) || (a >= self.base && a.checked_add(n).is_some_and(|end| end <= self.base + 0x0220_0000))
    }
}
impl crate::target::Memory for Direct {
    fn u8(&self, a: u32) -> Option<u8> {
        if self.allowed(a, 1) { Some(unsafe { read(a) }) } else { None }
    }
    fn u16(&self, a: u32) -> Option<u16> {
        if a & 1 == 0 && self.allowed(a, 2) { Some(unsafe { read(a) }) } else { None }
    }
    fn u32(&self, a: u32) -> Option<u32> {
        if a & 3 == 0 && self.allowed(a, 4) { Some(unsafe { read(a) }) } else { None }
    }
}

/// Large monsters of the loaded zone (small monsters have no name here and are left out).
pub fn monsters(base: u32) -> Monsters {
    let mut out = Monsters::EMPTY;
    let manager = unsafe { forge_singleton_getInstanceByName(c"sEnemy".as_ptr()) as u32 };
    if !heap(manager, 0x44) || manager & 3 != 0 {
        return out;
    }
    let target = crate::target::capture_game(&Direct { base }, manager, base);
    for i in 0..10 {
        let obj = unsafe { read::<u32>(manager + 0x1c + 4 * i) };
        if !heap(obj, 0x142c) || obj & 3 != 0 || out.all().iter().any(|m| m.object == obj) {
            continue;
        }
        let s = unsafe { read::<u32>(obj + 0x1428) };
        if !heap(s, 0x6212) || s & 3 != 0 {
            continue;
        }
        let (id, hp, max_hp, species) =
            unsafe { (read::<u16>(s + 0x1b6), read::<i32>(s + 0x370), read::<i32>(s + 0x374), read::<u16>(s + 0x6210)) };
        if id >= 10000 || max_hp <= 0 || max_hp > 200_000 || hp < 0 || hp > max_hp || name(species).is_none() {
            continue;
        }
        let mut m = Monster { object: obj, species, hp, max_hp, selected: target.selected == obj, weak: [0xFF; 3] };
        // Lao-Shan Lung, Nakarkos and Ahtal-Neset have no usable hitzone rows.
        if ![12, 84, 87].contains(&species) {
            let mut best = [0u8; 5];
            for part in 0..8 {
                let row = unsafe { read::<u32>(s + 0x418 + 4 * part) };
                if !heap(row, 10) {
                    continue;
                }
                crate::weakness::accumulate(&mut best, unsafe { read::<[u8; 10]>(row) });
            }
            m.weak = strongest(best);
        }
        out.entries[out.count] = m;
        out.count += 1;
    }
    out
}

/// Elements that hit hardest (hitzone 20 or more, or the best one), at most 3, strongest first.
fn strongest(best: [u8; 5]) -> [u8; 3] {
    let mut order = [0u8, 1, 2, 3, 4];
    order.sort_by(|a, b| best[*b as usize].cmp(&best[*a as usize]));
    let top = best[order[0] as usize];
    let mut out = [0xFF; 3];
    if top == 0 {
        return out;
    }
    let mut n = 0;
    for e in order {
        let v = best[e as usize];
        if n < 3 && v > 0 && (v >= 20 || v == top) {
            out[n] = e;
            n += 1;
        }
    }
    out
}

// Large monster names by species id (MHGU 1.4.0, English), from HuntLog.
pub fn name(id:u16)->Option<&'static str>{ match id {
1=>Some("Rathian"),
2=>Some("Rathalos"),
3=>Some("Khezu"),
4=>Some("Basarios"),
5=>Some("Gravios"),
7=>Some("Diablos"),
8=>Some("Yian Kut-ku"),
9=>Some("Gypceros"),
10=>Some("Plesioth"),
11=>Some("Kirin"),
12=>Some("Lao-Shan Lung"),
13=>Some("Fatalis"),
14=>Some("Velocidrome"),
15=>Some("Gendrome"),
16=>Some("Iodrome"),
17=>Some("Cephadrome"),
18=>Some("Yian Garuga"),
19=>Some("Daimyo Hermitaur"),
20=>Some("Shogun Ceanataur"),
21=>Some("Congalala"),
22=>Some("Blangonga"),
23=>Some("Rajang"),
24=>Some("Kushala Daora"),
25=>Some("Chameleos"),
27=>Some("Teostra"),
30=>Some("Bulldrome"),
32=>Some("Tigrex"),
33=>Some("Akantor"),
34=>Some("Giadrome"),
36=>Some("Lavasioth"),
37=>Some("Nargacuga"),
38=>Some("Ukanlos"),
42=>Some("Barioth"),
43=>Some("Deviljho"),
44=>Some("Barroth"),
45=>Some("Uragaan"),
46=>Some("Lagiacrus"),
47=>Some("Royal Ludroth"),
49=>Some("Agnaktor"),
50=>Some("Alatreon"),
55=>Some("Duramboros"),
56=>Some("Niblesnarf"),
57=>Some("Zinogre"),
58=>Some("Amatsu"),
60=>Some("Arzuros"),
61=>Some("Lagombi"),
62=>Some("Volvidon"),
63=>Some("Brachydios"),
65=>Some("Kecha Wacha"),
66=>Some("Tetsucabra"),
67=>Some("Zamtrios"),
68=>Some("Najarala"),
69=>Some("Seltas Queen"),
70=>Some("Nerscylla"),
71=>Some("Gore Magala"),
72=>Some("Shagaru Magala"),
76=>Some("Seltas"),
77=>Some("Seregios"),
79=>Some("Malfestio"),
80=>Some("Glavenus"),
81=>Some("Astalos"),
82=>Some("Mizutsune"),
83=>Some("Gammoth"),
84=>Some("Nakarkos"),
85=>Some("Great Maccao"),
86=>Some("Valstrax"),
87=>Some("Ahtal-Neset"),
88=>Some("Ahtal-Ka"),
269=>Some("Crimson Fatalis"),
513=>Some("Gold Rathian"),
514=>Some("Silver Rathalos"),
525=>Some("White Fatalis"),
1025=>Some("Dreadqueen Rathian"),
1026=>Some("Dreadking Rathalos"),
1031=>Some("Bloodbath Diablos"),
1042=>Some("Deadeye Yian Garuga"),
1043=>Some("Stonefist Hermitaur"),
1044=>Some("Rustrazor Ceanataur"),
1056=>Some("Grimclaw Tigrex"),
1061=>Some("Silverwind Nargacuga"),
1069=>Some("Crystalbeard Uragaan"),
1081=>Some("Thunderlord Zinogre"),
1084=>Some("Redhelm Arzuros"),
1085=>Some("Snowbaron Lagombi"),
1090=>Some("Drilltusk Tetsucabra"),
1103=>Some("Nightcloak Malfestio"),
1104=>Some("Hellblade Glavenus"),
1105=>Some("Boltreaver Astalos"),
1106=>Some("Soulseer Mizutsune"),
1107=>Some("Elderfrost Gammoth"),
1303=>Some("Furious Rajang"),
1323=>Some("Savage Deviljho"),
1343=>Some("Raging Brachydios"),
1351=>Some("Chaotic Gore Magala"),
_=>None,
}}
