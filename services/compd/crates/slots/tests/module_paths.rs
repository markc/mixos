// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Public keyboard, launch and storage behaviour after dissolving wrappers.

use slots::library::input::keyboard::{combo::KeyCombo, format, key::Key};
use smithay::input::keyboard::{Keysym, ModifiersState, keysyms};

#[test]
fn keysym_mapping_keeps_shifted_keys_and_vt_numbers() {
    for (raw, expected) in [
        (keysyms::KEY_a, Key::A),
        (keysyms::KEY_A, Key::A),
        (keysyms::KEY_exclam, Key::Num1),
        (keysyms::KEY_Left, Key::Left),
        (keysyms::KEY_KP_Add, Key::KpAdd),
        (keysyms::KEY_XF86AudioMute, Key::AudioMute),
        (keysyms::KEY_XF86Switch_VT_12, Key::SwitchVt12),
    ] {
        assert_eq!(Key::from_keysym(Keysym::new(raw)), Some(expected));
    }
    assert_eq!(Key::from_keysym(Keysym::new(0)), None);
    assert_eq!(Key::SwitchVt12.vt_number(), Some(12));
    assert_eq!(Key::AudioMute.vt_number(), None);
}

#[test]
fn shortcut_macro_and_parser_agree_on_exact_modifiers() {
    use slots::shortcut;
    let combo = shortcut!(Super + Shift + Right);
    assert_eq!(format::combo_string(&combo), "Super+Shift+Right");
    let parsed = format::parse_combo("Super+Shift+Right").unwrap();
    let modifiers = ModifiersState {
        logo: true,
        shift: true,
        ..Default::default()
    };
    assert!(combo.matches(&modifiers, parsed.key));
    assert!(!combo.matches(&ModifiersState::default(), parsed.key));
    assert!(format::parse_combo("Super+Right+Left").is_none());
}

slots::define_storage!(pub VALUE, VALUE_MUT: u32);

#[test]
fn concurrent_storage_retains_the_token_mutation_boundary() {
    let storage = slots::storage::concurrent::StorageConcurrent::new();
    storage.insert(&VALUE, 7);
    storage.with_mut(&VALUE_MUT, |value| *value += 2).unwrap();
    assert_eq!(storage.with(&VALUE, |value| *value), Some(9));
}

#[test]
fn launch_resolution_refuses_a_missing_executable() {
    use slots::library::process::child::{hygiene, spawn};
    let command = hygiene::command("/missing-mixos-test/executable");
    assert!(spawn::resolve(&command).is_none());
}

#[cfg(not(feature = "desktop-dbus"))]
#[test]
fn scope_adoption_is_explicitly_disabled_without_the_adapter() {
    assert!(
        slots::launch::scope::adopt_into_scope(1, "example")
            .unwrap_err()
            .contains("disabled")
    );
}
