// SPDX-License-Identifier: MIT OR Apache-2.0
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// One headless event-loop deadline, shortened by asynchronous repaint requests.
pub struct Wake(Arc<Mutex<Option<Instant>>>);

impl Wake {
    pub fn install(ctx: &egui::Context) -> Self {
        let wake = Self(Arc::new(Mutex::new(None::<Instant>)));
        let requested = wake.0.clone();
        let thread = std::thread::current();
        ctx.set_request_repaint_callback(move |request| {
            let mut deadline = requested.lock().unwrap();
            if let Some(at) = Instant::now().checked_add(request.delay)
                && deadline.is_none_or(|old| at < old)
            {
                *deadline = Some(at);
                thread.unpark();
            }
        });
        wake
    }

    pub fn begin(&self) {
        *self.0.lock().unwrap() = None;
    }

    /// egui may retain a delayed request without calling the callback again.
    /// Every completed pass therefore also supplies its authoritative delay.
    pub fn schedule(&self, delay: std::time::Duration) {
        let mut deadline = self.0.lock().unwrap();
        if let Some(at) = Instant::now().checked_add(delay)
            && deadline.is_none_or(|old| at < old)
        {
            *deadline = Some(at);
        }
    }

    pub fn wait(&self) {
        loop {
            let deadline = *self.0.lock().unwrap();
            match deadline {
                Some(at) if at <= Instant::now() => return,
                Some(at) => std::thread::park_timeout(at.saturating_duration_since(Instant::now())),
                None => std::thread::park(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn focused_text_field_stays_idle_for_two_seconds() {
        let ctx = egui::Context::default();
        let wake = Wake::install(&ctx);
        let end = Instant::now() + Duration::from_secs(2);
        let mut passes = 0;
        let mut text = String::from("focused");
        while Instant::now() < end {
            wake.begin();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.text_edit_singleline(&mut text).request_focus();
                // Also ensures the test eventually terminates if egui stops blinking.
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            });
            for viewport in output.viewport_output.values() {
                wake.schedule(viewport.repaint_delay);
            }
            output.textures_delta.clear();
            passes += 1;
            wake.wait();
        }
        eprintln!("idle focused text field: {passes} passes over two seconds");
        assert!(
            passes <= 20,
            "idle headless loop spun {passes} passes in two seconds"
        );
    }
}
