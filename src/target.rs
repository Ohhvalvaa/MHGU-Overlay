//! MHGU 1.4.0 quest-camera state, decoded from the game's own target HUD.
//! No persistent discovery cache: leaving a quest/area cannot reuse stale identity.
pub trait Memory {
    fn u8(&self,a:u32)->Option<u8>;
    fn u16(&self,a:u32)->Option<u16>;
    fn u32(&self,a:u32)->Option<u32>;
}
#[derive(Clone,Copy,Default,Debug)]
pub struct Target { pub selected:u32, pub known:[(u32,u16);2] }
impl Target {
 pub fn revealed(&self,object:u32,species:u16)->bool {
    object!=0 && species!=0 && self.known.iter().any(|&(o,s)|o==object && s==species)
 }
}
// uQuestCamera ctor main+1fa00 installs this vtable; reject village/cutscene cameras.
pub const QUEST_VTABLE:u32=0x01730d0c;
// The live Forge registry does not contain a singleton named sCamera.
// This is the game's verified camera-manager global (runtime address 0x1a4d750).
pub fn capture_game(m:&impl Memory,manager:u32,base:u32)->Target {
    let Some(camera_manager)=base.checked_add(0x184d750).and_then(|a|m.u32(a)) else{return Target::default()};
    capture(m,manager,camera_manager,base)
}
pub fn capture(m:&impl Memory,manager:u32,camera_manager:u32,base:u32)->Target {
 let mut out=Target::default();
 let Some(camera)=m.u32(camera_manager.wrapping_add(0x54)) else{return out};
 if m.u32(camera)!=base.checked_add(QUEST_VTABLE) {return out;}
 let Some(selected)=m.u32(camera+0xa6c) else{return out};
 let Some(mode)=m.u8(camera+0xa8e) else{return out};
 if mode>1 {return out;}
 let objects=[m.u32(camera+0xa70).unwrap_or(0),m.u32(camera+0xa74).unwrap_or(0)];
 let species=[m.u16(camera+0xa80).unwrap_or(0),m.u16(camera+0xa82).unwrap_or(0)];
 let states=[m.u8(camera+0xa7d).unwrap_or(0),m.u8(camera+0xa7e).unwrap_or(0)];
 if mode==1 {
    // Nakarkos's three camera targets belong to the same parent encounter.
    if matches!(states[1],1|2|4|8) && species[1]==0x54 {
        out.known[0]=(objects[1],species[1]);
    }
    if (1..=3).contains(&selected) && species[1]==0x54 {out.selected=objects[1];}
 } else {
    let visibility=m.u32(camera+0xa90).unwrap_or(0);
    let mut eligible=0usize;
    for i in 0..10 {
      let Some(object)=m.u32(manager+0x1c+4*i) else{continue};
      if object==0 || m.u8(object.wrapping_add(0xb5f7))!=Some(0) {continue;}
      // State/species follow current eligible enemy order, NOT discovered-slot order.
      // Discovered references establish membership independently (a monster can leave).
      if eligible<2 {
        let visible=if eligible==0 {visibility|2==3} else {visibility & !1==2};
        if visible && objects.contains(&object) && matches!(states[eligible],1|2|4|8) && species[eligible]!=0 {
            out.known[eligible]=(object,species[eligible]);
        }
      }
      eligible+=1;
      if eligible as u32==selected {out.selected=object;}
    }
 }
 out
}
#[cfg(test)] mod tests {
 use super::*;use std::collections::BTreeMap;
 struct Mem(BTreeMap<u32,u8>);
 impl Mem {
 fn put(&mut self,a:u32,v:u32,n:usize){for(i,b)in v.to_le_bytes()[..n].iter().enumerate(){self.0.insert(a+i as u32,*b);}}
 }
 impl Memory for Mem {
 fn u8(&self,a:u32)->Option<u8>{self.0.get(&a).copied()}
 fn u16(&self,a:u32)->Option<u16>{Some(u16::from_le_bytes([self.u8(a)?,self.u8(a+1)?]))}
 fn u32(&self,a:u32)->Option<u32>{Some(u32::from_le_bytes([self.u8(a)?,self.u8(a+1)?,self.u8(a+2)?,self.u8(a+3)?]))}
 }
 const M:u32=0x10000000;const S:u32=0x11000000;const C:u32=0x12000000;const A:u32=0x13000000;const B:u32=0x14000000;
 fn fixture()->Mem{let mut m=Mem(BTreeMap::new());for(a,v,n)in [(S+0x54,C,4),(C,0x200000+QUEST_VTABLE,4),(C+0xa6c,1,4),(C+0xa8e,0,1),(C+0xa90,3,4),(M+0x1c,A,4),(M+0x20,B,4),(A+0xb5f7,0,1),(B+0xb5f7,0,1),(C+0xa70,A,4),(C+0xa74,B,4),(C+0xa80,2,2),(C+0xa82,1,2),(C+0xa7d,1,1),(C+0xa7e,0,1)]{m.put(a,v,n);}m}
 #[test]fn reads_camera_from_game_global_without_named_singleton(){let mut m=fixture();m.put(0x200000+0x184d750,S,4);assert_eq!(capture_game(&m,M,0x200000).selected,A);}
 #[test]fn follows_player_selection_not_last_hit(){let mut m=fixture();assert_eq!(capture(&m,M,S,0x200000).selected,A);m.put(C+0xa6c,2,4);assert_eq!(capture(&m,M,S,0x200000).selected,B);m.put(C+0xa6c,0,4);assert_eq!(capture(&m,M,S,0x200000).selected,0);}
 #[test]fn unknown_identity_stays_hidden_even_when_selected(){let mut m=fixture();m.put(C+0xa6c,2,4);let t=capture(&m,M,S,0x200000);assert_eq!(t.selected,B);assert!(t.revealed(A,2));assert!(!t.revealed(B,1));m.put(C+0xa7e,2,1);assert!(capture(&m,M,S,0x200000).revealed(B,1));}
 #[test]fn duplicate_species_does_not_reveal_other_object(){let mut m=fixture();m.put(C+0xa82,2,2);assert!(!capture(&m,M,S,0x200000).revealed(B,2));}
 #[test]fn invalid_camera_and_stale_species_fail_closed(){let mut m=fixture();assert!(!capture(&m,M,S,0x200000).revealed(A,3));m.put(C,0,4);let t=capture(&m,M,S,0x200000);assert_eq!(t.selected,0);assert!(!t.revealed(A,2));}
 #[test]fn selection_skips_ineligible_entries(){let mut m=fixture();m.put(A+0xb5f7,1,1);assert_eq!(capture(&m,M,S,0x200000).selected,B);}
 #[test]fn invalid_ui_state_and_missing_memory_never_reveal(){let mut m=fixture();m.put(C+0xa7d,255,1);assert!(!capture(&m,M,S,0x200000).revealed(A,2));assert_eq!(capture(&Mem(BTreeMap::new()),M,S,0x200000).selected,0);}
 #[test]fn special_target_parts_map_to_parent(){let mut m=fixture();m.put(C+0xa8e,1,1);m.put(C+0xa82,0x54,2);for i in 1..=3{m.put(C+0xa6c,i,4);assert_eq!(capture(&m,M,S,0x200000).selected,B);}}
 #[test]fn reordered_enemy_still_matches_its_own_ui_identity(){
 let mut m=fixture();m.put(M+0x1c,0,4);m.put(C+0xa70,0,4);m.put(C+0xa80,1,2);
 let t=capture(&m,M,S,0x200000);assert_eq!(t.selected,B);assert!(t.revealed(B,1));
 }
 #[test]fn discovered_slot_order_does_not_control_identity(){
 let mut m=fixture();m.put(C+0xa70,B,4);m.put(C+0xa74,A,4);
 let t=capture(&m,M,S,0x200000);assert!(t.revealed(A,2));assert!(!t.revealed(B,1));
 }
 #[test]fn hidden_game_panel_never_exposes_identity(){
 let mut m=fixture();m.put(C+0xa90,0,4);assert!(!capture(&m,M,S,0x200000).revealed(A,2));
 }

}
