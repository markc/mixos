// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Samples {
    values: VecDeque<u64>,
    count: u64,
}
impl Samples {
    pub fn add(&mut self, elapsed: Duration) {
        self.count += 1;
        if self.values.len() == 2048 {
            self.values.pop_front();
        }
        self.values
            .push_back(elapsed.as_micros().min(u64::MAX as u128) as u64);
    }
    fn summary(&self, name: &str) -> String {
        let mut v: Vec<_> = self.values.iter().copied().collect();
        v.sort_unstable();
        if v.is_empty() {
            return format!("{name}:n=0");
        }
        format!(
            "{name}:n={} window={} p50={}us p95={}us max={}us",
            self.count,
            v.len(),
            v[(v.len() - 1) / 2],
            v[(v.len() - 1) * 95 / 100],
            v[v.len() - 1]
        )
    }
}
#[derive(Default)]
pub struct Metrics {
    pub key_write: Samples,
    pub read_vt_bound: Samples,
    pub vt_rgba: Samples,
    /// Time the render thread spent rasterising the frame into the texture's
    /// own buffer. There is no separate convert-then-upload step to time, and the
    /// name and the label say so.
    pub raster_paint: Samples,
    pub frame: Samples,
    pub frames: u64,
    pub reads: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub input_written: u64,
    pub reply_written: u64,
    pub wakes: u64,
    pub uploads: u64,
    pub last_read: Option<Instant>,
    pub vt_updated: Option<Instant>,
    /// When a frontend last handed this pane's grid to its renderer.
    pub last_frame: Option<Instant>,
    /// The frontend draws on the GPU: `vt_rgba` is the VT notification to
    /// the snapshot handed to the renderer, and `raster_paint` the
    /// renderer's prepare (instances and uploads, CPU side).
    pub gpu: bool,
}
impl Metrics {
    pub fn summary(&self) -> String {
        format!(
            "DIAGNOSTIC process-side only; frames={} reads={} bytes_read={} bytes_written={} input_written={} reply_written={} wakes={} uploads={}\n{}\n{}\n{}\n{}\n{}",
            self.frames,
            self.reads,
            self.bytes_read,
            self.bytes_written,
            self.input_written,
            self.reply_written,
            self.wakes,
            self.uploads,
            self.key_write.summary("key_to_actual_PTY_write"),
            self.read_vt_bound
                .summary("PTY_last_read_to_damage_notification_upper_bound"),
            if self.gpu {
                self.vt_rgba.summary("VT_notification_to_GPU_snapshot")
            } else {
                self.vt_rgba.summary("VT_notification_to_RGBA")
            },
            if self.gpu {
                self.raster_paint.summary("gpu_prepare_cpu_side")
            } else {
                self.raster_paint.summary("raster_paint_into_texture")
            },
            self.frame.summary("process_frame_interval")
        )
    }
    /// A GPU frontend handed this pane's snapshot to its renderer now.
    /// `prepare` is how long the renderer's prepare of the PREVIOUS
    /// snapshot took (known only once it has drawn).
    pub fn gpu_frame(&mut self, prepare: Option<Duration>) {
        let now = Instant::now();
        self.gpu = true;
        self.frames += 1;
        if let Some(updated) = self.vt_updated.take() {
            self.vt_rgba.add(now.saturating_duration_since(updated));
        }
        if let Some(last) = self.last_frame.replace(now) {
            self.frame.add(now.saturating_duration_since(last));
        }
        if let Some(prepare) = prepare {
            self.uploads += 1;
            self.raster_paint.add(prepare);
        }
    }

    pub fn parsed_boundary(&mut self) {
        if let Some(read) = self.last_read.take() {
            self.read_vt_bound.add(read.elapsed());
            self.vt_updated = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_frames_fill_the_frame_samples_under_gpu_labels() {
        let mut metrics = Metrics::default();
        assert!(metrics.summary().contains("VT_notification_to_RGBA:n=0"));
        metrics.vt_updated = Some(Instant::now());
        metrics.gpu_frame(None);
        metrics.gpu_frame(Some(Duration::from_micros(80)));
        let summary = metrics.summary();
        assert!(summary.contains("frames=2"), "{summary}");
        assert!(
            summary.contains("VT_notification_to_GPU_snapshot:n=1"),
            "{summary}"
        );
        assert!(summary.contains("gpu_prepare_cpu_side:n=1"), "{summary}");
        assert!(summary.contains("process_frame_interval:n=1"), "{summary}");
    }
}
