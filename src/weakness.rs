//! Current elemental maxima across the eight verified live hitzone pointers.
//! Byte order verified against MHGU ARM damage routine and ArmoredRaven17 extractor.
pub fn accumulate(best:&mut [u8;5], row:[u8;10]) {
    if row[..8].iter().all(|v|*v==100) || row[..8].iter().any(|v|*v>100) {return;}
    for i in 0..5 {best[i]=best[i].max(row[3+i]);}
}
pub fn strongest(best:[u8;5])->u8 {
    let max=*best.iter().max().unwrap();
    if max==0 {return 0;}
    best.iter().enumerate().fold(0,|mask,(i,v)|if *v==max {mask | (1<<i)}else{mask})
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn placeholders_and_corrupt_rows_do_not_invent_weakness() {
        let mut b=[0;5];accumulate(&mut b,[100;10]);accumulate(&mut b,[255;10]);accumulate(&mut b,[0;10]);assert_eq!(strongest(b),0);
    }
    #[test] fn ties_and_current_state_changes_preserve_element_order() {
        let mut dry=[0;5];accumulate(&mut dry,[50,50,50,40,0,20,0,0,0,0]);assert_eq!(strongest(dry),1);
        let mut wet=[0;5];accumulate(&mut wet,[50,50,50,0,50,20,50,0,0,0]);assert_eq!(strongest(wet),2|8);
    }
}
