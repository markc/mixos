// SPDX-License-Identifier: MIT OR Apache-2.0
use command::{History, history::Budget};

fn history(entries: usize, bytes: usize) -> History<String> {
    History::new(String::new(), Budget { max_entries: entries, max_bytes: bytes, bytes: String::len })
}
fn append(h: &mut History<String>, text: &str) {
    h.edit("append", |s| { s.push_str(text); Ok::<_, ()>(()) }).unwrap();
}
#[test]
fn count_and_byte_budgets_drop_oldest_snapshots() {
    let mut h = history(2, usize::MAX);
    for _ in 0..4 { append(&mut h, "x"); }
    assert_eq!(h.depth(), (2, 0));
    assert!(h.undo()); assert!(h.undo()); assert!(!h.undo());
    assert_eq!(h.current(), "xx");
    let mut h = history(100, 3);
    for _ in 0..4 { append(&mut h, "x"); }
    assert_eq!(h.depth(), (1, 0)); // only the three-byte snapshot fits
    assert!(h.undo());
    assert_eq!(h.current(), "xxx");
    assert!(!h.can_redo()); // the four-byte snapshot exceeds the budget
}
#[test]
fn coalescing_is_explicit_and_stops_at_boundaries() {
    let mut h = history(10, 100);
    for _ in 0..3 {
        h.edit_coalesced("drag", |s| { s.push('x'); Ok::<_, ()>(()) }).unwrap();
    }
    assert_eq!(h.labels(), (vec!["drag"], vec![]));
    h.undo(); assert_eq!(h.current(), "");
    h.redo(); assert_eq!(h.current(), "xxx");
    append(&mut h, "y");
    h.edit_coalesced("drag", |s| { s.push('z'); Ok::<_, ()>(()) }).unwrap();
    assert_eq!(h.depth(), (3, 0));
}
#[test]
fn failure_preserves_state_labels_redo_and_checkpoint() {
    let mut h = history(10, 100);
    append(&mut h, "x"); h.mark_saved(); append(&mut h, "y"); h.undo();
    let labels = h.labels();
    assert_eq!(h.edit("bad", |s| { s.push('!'); Err::<(), _>("bad") }), Err("bad"));
    assert_eq!(h.current(), "x"); assert_eq!(h.labels(), labels); assert!(!h.dirty());
    h.redo(); assert!(h.dirty()); h.undo(); assert!(!h.dirty());
    append(&mut h, "z"); assert!(!h.can_redo()); assert!(h.dirty());
    h.mark_saved(); assert!(!h.dirty()); h.undo(); assert!(h.dirty()); h.redo(); assert!(!h.dirty());
}
