//! Damage done to each large monster by each hunter (and Palico), and DPS.
//! Like MH World Overlay, a hunter's DPS counts only the time since they joined the fight.
use core::sync::atomic::{AtomicU32, Ordering};

use crate::game::{self, Name, Shared};

pub const MAX_ROWS: usize = 6; // 4 hunters + their Palicos in a full party would be 8; 6 covers real parties
const MAX_FIGHTS: usize = 8;

#[derive(Clone, Copy)]
pub struct Row {
    /// Hunter slot 0-3, or 0xFF for a Palico (then `owner` tells them apart).
    pub slot: u8,
    pub owner: u32,
    pub local: bool,
    pub weapon: u8,
    pub name: Name,
    pub damage: u64,
    /// Clock time this row's DPS starts from.
    pub since: f32,
}
impl Row {
    const EMPTY: Self = Self { slot: 0xFF, owner: 0, local: false, weapon: 0xFF, name: Name::EMPTY, damage: 0, since: 0.0 };
}

#[derive(Clone, Copy)]
pub struct Fight {
    pub monster: u32,
    pub count: usize,
    pub rows: [Row; MAX_ROWS],
    pub start: f32,
    /// Clock time of the last hit.
    pub last: f32,
}
impl Fight {
    pub const EMPTY: Self = Self { monster: 0, count: 0, rows: [Row::EMPTY; MAX_ROWS], start: 0.0, last: 0.0 };
    pub fn total(&self) -> u64 {
        self.rows[..self.count].iter().map(|r| r.damage).sum()
    }
}

struct State {
    fights: [Fight; MAX_FIGHTS],
    /// When each hunter slot was first seen in the party (clock time, negative = empty slot).
    joined: [f32; 4],
    party: [u32; 4],
    /// Large monsters of the zone: only damage to these counts.
    large: [u32; 10],
}

static STATE: Shared<State> = Shared::new(State {
    fights: [Fight::EMPTY; MAX_FIGHTS],
    joined: [-1.0; 4],
    party: [0; 4],
    large: [0; 10],
});

/// Seconds since the quest started (f32 bits), advanced by the game's update.
static CLOCK: AtomicU32 = AtomicU32::new(0);

pub fn now() -> f32 {
    f32::from_bits(CLOCK.load(Ordering::Relaxed))
}

/// Forget everything (new quest, back in town).
pub fn reset() {
    CLOCK.store(0f32.to_bits(), Ordering::Relaxed);
    STATE.with(|s| {
        s.fights = [Fight::EMPTY; MAX_FIGHTS];
        s.joined = [-1.0; 4];
        s.party = [0; 4];
    });
}

/// Called every game update: advances the clock and follows hunters joining or leaving.
pub fn update(dt: f32, party: [u32; 4], large: &game::Monsters) {
    let t = now() + if dt.is_finite() && dt > 0.0 && dt < 1.0 { dt } else { 1.0 / 60.0 };
    CLOCK.store(t.to_bits(), Ordering::Relaxed);
    STATE.with(|s| {
        for i in 0..4 {
            if party[i] != s.party[i] {
                // Someone new in this slot: their DPS starts now.
                s.joined[i] = if party[i] >= 0x10000 { t } else { -1.0 };
                s.party[i] = party[i];
            }
        }
        s.large = [0; 10];
        for (i, m) in large.all().iter().enumerate() {
            s.large[i] = m.object;
        }
    });
}

/// A hit the game applied (called from the damage hook, on the game thread).
pub fn hit(monster: u32, owner: u32, slot: u8, local: bool, damage: i32) {
    if damage <= 0 || owner < 0x10000 {
        return;
    }
    let base = crate::GAME_BASE.load(Ordering::Relaxed);
    let t = now();
    STATE.with(|s| {
        if !s.large.contains(&monster) {
            return;
        }
        let index = match s.fights.iter().position(|f| f.monster == monster) {
            Some(i) => i,
            None => {
                // A new fight: take a free entry, or the one with the least damage.
                let i = s.fights.iter().position(|f| f.monster == 0).unwrap_or_else(|| {
                    (0..MAX_FIGHTS).min_by_key(|&i| s.fights[i].total()).unwrap_or(0)
                });
                s.fights[i] = Fight { monster, start: t, ..Fight::EMPTY };
                i
            }
        };
        let joined = if slot < 4 { s.joined[slot as usize] } else { -1.0 };
        let fight = &mut s.fights[index];
        fight.last = t;
        let row = fight.rows[..fight.count]
            .iter()
            .position(|r| if slot < 4 { r.slot == slot } else { r.slot == 0xFF && r.owner == owner });
        let row = match row {
            Some(r) => r,
            None => {
                if fight.count >= MAX_ROWS {
                    return;
                }
                let since = if joined > fight.start { joined } else { fight.start };
                fight.rows[fight.count] = Row { slot, owner, local, since, ..Row::EMPTY };
                fight.count += 1;
                fight.count - 1
            }
        };
        let r = &mut fight.rows[row];
        r.damage = r.damage.saturating_add(damage as u64);
        r.local |= local;
        r.owner = owner;
        let weapon = game::owner_weapon(owner);
        if weapon != 0xFF {
            r.weapon = weapon;
        }
        if r.name.is_empty() {
            r.name = unsafe { game::owner_name(base, owner) };
        }
    });
}

/// The damage done to one monster, biggest first.
pub fn fight(monster: u32) -> Fight {
    let mut f = STATE.with(|s| s.fights.iter().find(|f| f.monster == monster && monster != 0).copied())
        .unwrap_or(Fight::EMPTY);
    f.rows[..f.count].sort_by(|a, b| b.damage.cmp(&a.damage));
    f
}

/// Damage per second of a row, from when that hunter joined the fight until `end` (at least one second).
/// `end` is now while the monster is alive, the last hit once it is dead or captured.
pub fn dps(row: &Row, end: f32) -> f32 {
    let seconds = end - row.since;
    row.damage as f32 / if seconds > 1.0 { seconds } else { 1.0 }
}
