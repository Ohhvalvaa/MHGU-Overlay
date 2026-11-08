//! Entry validation and rollback ordering, independent of the game and Forge ABI.

pub fn entry_allowed(actual: [u32; 2], stock: [u32; 2], chain_safe: bool) -> bool {
    actual == stock || chain_safe
        && actual[0] == 0xe51ff004
        && (0x03200000..0x03e00000).contains(&actual[1])
        && actual[1] & 3 == 0
}

pub fn hook_created(actual: [u32; 2], detour: u32, original: u32) -> bool {
    original != 0 && original & 3 == 0 && actual == [0xe51ff004, detour]
}

pub fn install_all(
    count: usize,
    mut create: impl FnMut(usize) -> bool,
    mut restore: impl FnMut(usize),
) -> bool {
    for index in 0..count {
        if !create(index) {
            // Include the failed call: a loader may have changed the entry
            // before returning an invalid original pointer.
            for previous in (0..=index).rev() { restore(previous); }
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const STOCK: [u32; 2] = [0xe92d41f0, 0xe24dd010];

    #[test]
    fn only_exact_stock_or_validated_plugin_jump_can_chain() {
        assert!(entry_allowed(STOCK, STOCK, false));
        assert!(entry_allowed([0xe51ff004, 0x03205d44], STOCK, true));
        for actual in [
            [0, 0],
            [0xe92d41f0, 0],
            [0xea000000, 0x03205d44],
            [0xe51ff004, 0],
            [0xe51ff004, 0x03205d45],
            [0xe51ff004, 0x031ffffc],
            [0xe51ff004, 0x03e00000],
        ] {
            assert!(!entry_allowed(actual, STOCK, true), "accepted {actual:x?}");
        }
    }

    #[test]
    fn caller_sensitive_entry_rejects_even_a_recognized_jump() {
        assert!(!entry_allowed([0xe51ff004, 0x03205d44], STOCK, false));
    }

    #[test]
    fn failed_or_malformed_create_is_never_ready() {
        assert!(hook_created([0xe51ff004, 0x03407b74], 0x03407b74, 0x03e00010));
        assert!(!hook_created([0xe51ff004, 0x03407b74], 0x03407b74, 0));
        assert!(!hook_created([0xe51ff004, 0x03407b74], 0x03407b74, 0x03e00011));
        assert!(!hook_created(STOCK, 0x03407b74, 0x03e00010));
        assert!(!hook_created([0xe51ff004, 0x03205d44], 0x03407b74, 0x03e00010));
    }

    #[test]
    fn each_failed_create_restores_prior_chain_and_clears_originals_in_reverse() {
        for fail in 0..3 {
            let entries = RefCell::new([0x03205d44, 0x03205d94, 0xe92d41f0]);
            let originals = RefCell::new([0; 3]);
            let restored = RefCell::new(std::vec::Vec::new());
            let installed = install_all(3, |index| {
                entries.borrow_mut()[index] = 0x03400000 + index as u32 * 4;
                if index == fail { return false; }
                originals.borrow_mut()[index] = 0x03e00000 + index as u32 * 16;
                true
            }, |index| {
                entries.borrow_mut()[index] = [0x03205d44, 0x03205d94, 0xe92d41f0][index];
                originals.borrow_mut()[index] = 0;
                restored.borrow_mut().push(index);
            });
            assert!(!installed);
            assert_eq!(*entries.borrow(), [0x03205d44, 0x03205d94, 0xe92d41f0]);
            assert_eq!(*originals.borrow(), [0; 3]);
            assert_eq!(*restored.borrow(), (0..=fail).rev().collect::<std::vec::Vec<_>>());
        }
    }

    #[test]
    fn successful_install_keeps_every_original_and_does_not_restore() {
        let originals = RefCell::new([0; 3]);
        assert!(install_all(3, |index| {
            originals.borrow_mut()[index] = 0x03e00000 + index as u32 * 16;
            true
        }, |_| panic!("successful installation was rolled back")));
        assert_eq!(*originals.borrow(), [0x03e00000, 0x03e00010, 0x03e00020]);
    }
}
