// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Bus service names screenshot registers as. One window and one
//! headless instance per compositor, so a request only ever reaches an
//! instance that captures through the compositor it came from:
//!
//! | compositor | window | headless |
//! |---|---|---|
//! | `comp` | `screenshot` | `screenshot-headless` |
//! | `X`, if the name fits | `screenshot-X` | `screenshot-headless-X` |
//! | otherwise | `screenshot--<hash>` | `screenshot-headless--<hash>` |
//!
//! `<hash>` is 8 hex digits of a stable hash of the compositor's name. A
//! name must be at most 31 bytes (the ABP grammar), and a window name never
//! starts with `screenshot-headless`: a compositor whose name would make it
//! do so takes the hashed form. Since no compositor name starts with `-`,
//! the hashed forms never collide with a derived one, and the two
//! namespaces are disjoint by construction.

/// The window's name for the desktop's own compositor.
pub const WINDOW: &str = "screenshot";
/// The headless instance's name for the desktop's own compositor.
pub const HEADLESS: &str = "screenshot-headless";
/// The desktop's own compositor's service.
pub const DEFAULT_COMP: &str = "comp";
/// The longest ABP service name.
pub const MAX: usize = 31;

/// Whether `name` is an ABP service name: `^[a-z][a-z0-9-]{1,30}$`.
pub fn valid(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && (2..=MAX).contains(&name.len())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 32-bit FNV-1a of `text`, as 8 hex digits: stable across builds.
fn hash(text: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        h ^= u32::from(byte);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

/// The window's service for compositor `comp`.
pub fn window(comp: &str) -> String {
    if comp == DEFAULT_COMP {
        return WINDOW.into();
    }
    let derived = format!("{WINDOW}-{comp}");
    if derived.len() <= MAX && !derived.starts_with(HEADLESS) {
        derived
    } else {
        format!("{WINDOW}--{}", hash(comp))
    }
}

/// The headless instance's service for compositor `comp`.
pub fn headless(comp: &str) -> String {
    if comp == DEFAULT_COMP {
        return HEADLESS.into();
    }
    let derived = format!("{HEADLESS}-{comp}");
    if derived.len() <= MAX {
        derived
    } else {
        format!("{HEADLESS}--{}", hash(comp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_is_abps() {
        for good in ["comp", "comp-nested", "ab", "a1-b2", &"a".repeat(31)] {
            assert!(valid(good), "{good}");
        }
        for bad in [
            "c",
            "Comp",
            "comp.test",
            "1comp",
            "-comp",
            "comp_x",
            &"a".repeat(32),
            "",
        ] {
            assert!(!valid(bad), "{bad}");
        }
    }

    #[test]
    fn names_fit_and_the_two_families_never_meet() {
        assert_eq!(window("comp"), "screenshot");
        assert_eq!(headless("comp"), "screenshot-headless");
        assert_eq!(window("comp-nested"), "screenshot-comp-nested");
        assert_eq!(headless("comp-nested"), "screenshot-headless-comp-nested");
        // 20 bytes fits a window name exactly; 21 is hashed.
        let twenty = "c".repeat(20);
        assert_eq!(window(&twenty), format!("screenshot-{twenty}"));
        assert_eq!(window(&twenty).len(), 31);
        let long = "c".repeat(21);
        assert!(window(&long).starts_with("screenshot--"));
        assert!(headless(&long).starts_with("screenshot-headless--"));
        // A compositor called "headless…" never yields a headless-looking
        // window name.
        for comp in ["headless", "headless-x", "headless1"] {
            assert!(!window(comp).starts_with(HEADLESS), "{comp}");
            assert!(window(comp).starts_with("screenshot--"));
        }
        // Every derived name is a valid service name, and distinct.
        let comps = [
            "comp",
            "comp-nested",
            "headless",
            "nested",
            "a",
            "ab",
            &"z".repeat(31),
            &"y".repeat(21),
        ];
        let mut all = Vec::new();
        for comp in comps {
            for name in [window(comp), headless(comp)] {
                assert!(valid(&name), "{name}");
                assert!(name.len() <= MAX);
                all.push(name);
            }
        }
        let count = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), count, "no two collide");
        // Stable: the same compositor always gets the same name.
        assert_eq!(window(&long), window(&long));
    }
}
