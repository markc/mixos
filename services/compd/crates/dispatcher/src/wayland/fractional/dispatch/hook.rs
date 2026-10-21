// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::state::fractional_base;
use crate::state::fractional_base::{Published, emit_to_surfaces};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Resource identities stay in the values. Only their immutable hash selects a
/// bucket, and equality resolves collisions within that bucket.
pub struct IdentityMap<K, V> {
    buckets: HashMap<u64, Vec<(K, V)>>,
}

impl<K, V> Default for IdentityMap<K, V> {
    fn default() -> Self {
        Self { buckets: HashMap::new() }
    }
}

impl<K: Eq + Hash, V> IdentityMap<K, V> {
    fn hash(key: &K) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.buckets.get(&Self::hash(key))?.iter()
            .find(|(stored, _)| stored == key).map(|(_, value)| value)
    }

    fn insert(&mut self, key: K, value: V) {
        let bucket = self.buckets.entry(Self::hash(&key)).or_default();
        if let Some((_, stored)) = bucket.iter_mut().find(|(stored, _)| *stored == key) {
            *stored = value;
        } else {
            bucket.push((key, value));
        }
    }

    fn values(&self) -> impl Iterator<Item = &V> {
        self.buckets.values().flatten().map(|(_, value)| value)
    }
}

pub type SurfaceScales = IdentityMap<ObjectId, Published>;

/// One pending entry's contribution to the batch fingerprint. Summed, not chained,
/// so the value is independent of `HashMap` iteration order.
fn stamp(id: &ObjectId, want: Published) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    want.scale.to_bits().hash(&mut hasher);
    want.idle.hash(&mut hasher);
    hasher.finish()
}

/// Publish each surface's fractional scale. `per_surface` is the already-aggregated
/// `(published scale, surface)` per surface across ALL outputs (`debounce::published`:
/// zoom on the output's scale), so a window on two monitors follows the sharper one; `idle` (disjoint from it) is mapped but
/// invisible everywhere and gets [`Published::IDLE`].
///
/// Everything differing from `sent` forms the PENDING BATCH, which is fingerprinted
/// and handed to `Fractional::tick`. Because the debounce observes the batch itself
/// rather than a proxy, one cycle covers every source of churn — zoom eases, drift
/// pans, pane-to-pane moves — and the batch is submitted whole once it settles.
/// A held surface keeps the scale the client ACTUALLY has, so a pan out-and-back
/// inside the quiet window resolves to no change and emits nothing at all.
///
/// Two RECORDED transitions bypass the gate: a surface with no [`Published`] record
/// (a new window) and one whose record is idle and which is visible again. A window
/// drifting into view must reach its real scale on the frame it appears. Returns the
/// surfaces a new VISIBLE scale reached — the windows about to re-lay themselves out,
/// which is what the caller re-states its decided size to.
pub fn emit_best_per_surface(
    fractional: &mut fractional_base::Fractional,
    sent: &mut SurfaceScales,
    per_surface: &[(f64, WlSurface)],
    idle: &[WlSurface],
) -> Vec<WlSurface> {
    let mut emitted: Vec<WlSurface> = Vec::new();
    let desired = per_surface
        .iter()
        .map(|(scale, surface)| (surface, Published::visible(*scale)))
        .chain(idle.iter().map(|surface| (surface, Published::IDLE)));

    let mut next = SurfaceScales::default();
    let mut batch: Vec<(&WlSurface, Published)> = Vec::new();
    let mut fingerprint: u64 = 0;

    for (surface, want) in desired {
        let id = surface.id();
        let previous = sent.get(&id).copied();
        // Already holds this value; only the classification may differ, and
        // re-recording that costs no wire event.
        if previous.is_some_and(|p| p.scale == want.scale) {
            next.insert(id, want);
            continue;
        }
        // New window (no record) or un-idling (record says idle, visible now).
        if !want.idle && previous.is_none_or(|p| p.is_idle()) {
            emit_to_surfaces(want.scale, std::iter::once(surface));
            emitted.push(surface.clone());
            next.insert(id, want);
            continue;
        }
        // Held: keep the scale the client HAS, with the new classification. A
        // never-seen surface stays OUT of `sent` — recording a value it was never
        // told would suppress its emit forever.
        fingerprint = fingerprint.wrapping_add(stamp(&id, want));
        if let Some(previous) = previous {
            next.insert(
                id,
                Published {
                    scale: previous.scale,
                    idle: want.idle,
                },
            );
        }
        batch.push((surface, want));
    }

    if fractional.tick((!batch.is_empty()).then_some(fingerprint)) {
        for (surface, want) in batch {
            emit_to_surfaces(want.scale, std::iter::once(surface));
            if !want.idle {
                emitted.push(surface.clone());
            }
            next.insert(surface.id(), want);
        }
    }
    // Seed for `new_surface` / `new_fractional_scale`. Still one global value: an
    // unmapped surface belongs to no pane, so there is no per-surface answer yet.
    if let Some(sharpest) = next
        .values()
        .filter(|p| !p.idle)
        .map(|p| p.scale)
        .reduce(f64::max)
    {
        fractional.last_emitted_scale = Some(sharpest);
    }
    *sent = next;
    emitted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(PartialEq, Eq)]
    struct Colliding(u8);
    impl Hash for Colliding {
        fn hash<H: Hasher>(&self, state: &mut H) { 0u8.hash(state); }
    }

    #[test]
    fn equal_hashes_keep_distinct_identities_and_updates_replace_only_the_owner() {
        let mut map = IdentityMap::default();
        map.insert(Colliding(1), 1.25);
        map.insert(Colliding(2), 2.5);
        map.insert(Colliding(1), 1.75);
        assert_eq!(map.get(&Colliding(1)), Some(&1.75));
        assert_eq!(map.get(&Colliding(2)), Some(&2.5));
        assert_eq!(map.get(&Colliding(3)), None);
        assert_eq!(map.values().count(), 2);
    }
}
