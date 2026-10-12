// SPDX-License-Identifier: MIT OR Apache-2.0
//! The glyph atlas: every distinct [`GlyphKey`] rasterised once by the CPU
//! raster and packed into one RGBA texture on shelves.
//!
//! A mask layer is stored as `(0, 0, 0, coverage)`; a colour layer as its
//! premultiplied RGBA. The atlas is valid for one raster identity (font, size
//! and scale). A change of identity, or running out of room, empties it and
//! bumps [`Atlas::generation`], and every pane rebuilds its instances.
use std::collections::HashMap;
use std::sync::Arc;
use term_core::raster::{GlyphKey, Raster};

/// One packed layer: where it sits in the atlas and in its cell span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub source: [u16; 4],
    pub offset: [u16; 2],
    pub colour: bool,
}

/// Side of the square atlas. 2048 holds several thousand cell-sized glyphs
/// at 2.5x scale and is within every GLES 3 device's guaranteed limit.
pub(crate) const SIZE: u32 = 2048;

/// Shelf packing: rows of rectangles, each shelf as tall as its first
/// occupant; a rectangle goes on the tightest shelf it fits.
#[derive(Clone)]
pub(crate) struct Shelves {
    size: u32,
    /// (y, height, next free x)
    shelves: Vec<(u32, u32, u32)>,
    bottom: u32,
}

impl Shelves {
    pub fn new(size: u32) -> Self {
        Self {
            size,
            shelves: Vec::new(),
            bottom: 0,
        }
    }

    pub fn clear(&mut self) {
        self.shelves.clear();
        self.bottom = 0;
    }

    pub fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let size = self.size;
        if width > size || height > size {
            return None;
        }
        if let Some(shelf) = self
            .shelves
            .iter_mut()
            .filter(|s| s.1 >= height && s.2 + width <= size)
            .min_by_key(|s| s.1 - height)
        {
            let at = (shelf.2, shelf.0);
            shelf.2 += width;
            return Some(at);
        }
        if self.bottom + height > size {
            return None;
        }
        self.shelves.push((self.bottom, height, width));
        let at = (0, self.bottom);
        self.bottom += height;
        Some(at)
    }
}

pub(crate) struct Atlas {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    shelves: Shelves,
    entries: HashMap<GlyphKey, Vec<Slot>>,
    identity: Option<Arc<()>>,
    pub generation: u64,
    /// Bytes uploaded in total; panes read differences of it.
    pub uploaded: usize,
}

impl Atlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("term glyph atlas"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            shelves: Shelves::new(SIZE),
            entries: HashMap::new(),
            identity: None,
            generation: 0,
            uploaded: 0,
        }
    }

    /// Empty the atlas: it is full, or its raster changed. Panes holding
    /// instances from the old generation rebuild them.
    pub fn reset(&mut self) {
        self.shelves.clear();
        self.entries.clear();
        self.generation += 1;
    }

    /// Empty the atlas when `raster` is not the one it was filled from.
    pub fn bind(&mut self, raster: &Raster) {
        if !self
            .identity
            .as_ref()
            .is_some_and(|id| raster.same_identity(id))
        {
            self.identity = Some(raster.identity());
            self.reset();
        }
    }

    /// The slots `key` draws, rasterising and uploading it on first use.
    /// `None` when the atlas is full.
    pub fn slots(
        &mut self,
        queue: &wgpu::Queue,
        raster: &mut Raster,
        key: &GlyphKey,
    ) -> Option<&[Slot]> {
        if !self.entries.contains_key(key) {
            let layers = raster.glyph_layers(key);
            // Place every layer before committing any: a glyph whose last
            // layer does not fit must not leave its first ones occupying
            // shelf space until the next reset.
            let mut planned = self.shelves.clone();
            let places = layers
                .iter()
                .map(|l| planned.allocate(l.width, l.height))
                .collect::<Option<Vec<_>>>()?;
            self.shelves = planned;
            let mut slots = Vec::with_capacity(layers.len());
            for (layer, (x, y)) in layers.iter().zip(places) {
                let rgba: Vec<u8> = if layer.colour {
                    layer.pixels.clone()
                } else {
                    layer.pixels.iter().flat_map(|&a| [0, 0, 0, a]).collect()
                };
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x, y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(layer.width * 4),
                        rows_per_image: Some(layer.height),
                    },
                    wgpu::Extent3d {
                        width: layer.width,
                        height: layer.height,
                        depth_or_array_layers: 1,
                    },
                );
                self.uploaded += rgba.len();
                slots.push(Slot {
                    source: [x as u16, y as u16, layer.width as u16, layer.height as u16],
                    offset: [layer.x as u16, layer.y as u16],
                    colour: layer.colour,
                });
            }
            self.entries.insert(key.clone(), slots);
        }
        self.entries.get(key).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::Shelves;

    #[test]
    fn shelves_pack_without_overlap_and_refuse_when_full() {
        let mut shelves = Shelves::new(64);
        let mut placed = Vec::new();
        while let Some(at) = shelves.allocate(10, 16) {
            placed.push(at);
        }
        assert_eq!(placed.len(), 6 * 4);
        for (i, a) in placed.iter().enumerate() {
            for b in &placed[i + 1..] {
                let apart =
                    a.0 + 10 <= b.0 || b.0 + 10 <= a.0 || a.1 + 16 <= b.1 || b.1 + 16 <= a.1;
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
        // A shorter rectangle reuses a taller shelf's tail before opening one.
        let mut shelves = Shelves::new(64);
        assert_eq!(shelves.allocate(40, 16), Some((0, 0)));
        assert_eq!(shelves.allocate(20, 8), Some((40, 0)));
        assert_eq!(shelves.allocate(20, 8), Some((0, 16)));
        assert_eq!(shelves.allocate(65, 1), None);
    }
}
