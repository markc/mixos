// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lab: bounded, coalescing preparation; completion wakes the existing UI.
use super::dataset::Dataset;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    path: String,
    token: String,
    session: String,
    owner: String,
    generation: u64,
    total: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Ready {
    pub data: Result<Arc<Dataset>, String>,
    pub micros: u64,
    pub job: u64,
}

#[derive(Debug)]
struct Job {
    key: Key,
    id: u64,
    #[cfg(test)]
    gate: Option<Arc<(std::sync::Barrier, std::sync::Barrier)>>,
}

#[derive(Debug)]
struct Slot {
    wanted: Key,
    next: Option<Job>,
    ready: Option<Ready>,
    scheduled: bool,
    wake: egui::Context,
    stale: u64,
    cancelled: bool,
}

type Cell = Mutex<Slot>;

struct Pool {
    sender: mpsc::SyncSender<Weak<Cell>>,
}

fn pool() -> Result<&'static Pool, String> {
    static POOL: OnceLock<Result<Pool, String>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Weak<Cell>>(16);
        let receiver = Arc::new(Mutex::new(receiver));
        for n in 0..2 {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("scene-model-{n}"))
                .spawn(move || {
                    loop {
                        let Ok(cell) = receiver.lock().unwrap().recv() else {
                            return;
                        };
                        let Some(cell) = cell.upgrade() else { continue };
                        loop {
                            let job = cell.lock().unwrap().next.take();
                            let Some(job) = job else { break };
                            #[cfg(test)]
                            if let Some(gate) = &job.gate {
                                gate.0.wait();
                                gate.1.wait();
                            }
                            let start = Instant::now();
                            let mut span = ui::frame_trace::span("table_model_load", job.id);
                            // Fault injection for the lab, never sleeps on the UI thread.
                            delay_for_lab();
                            if cell.lock().unwrap().cancelled {
                                break;
                            }
                            let k = &job.key;
                            let data = Dataset::load(
                                &json!({"path":k.path,"token":k.token}),
                                &k.session,
                                &k.owner,
                                k.generation,
                                k.total,
                            );
                            let ready = Ready {
                                data,
                                micros: start.elapsed().as_micros() as u64,
                                job: job.id,
                            };
                            span.counters(k.generation, k.total as u64);
                            drop(span);
                            let (wake, again) = {
                                let mut slot = cell.lock().unwrap();
                                let wake = if !slot.cancelled && slot.wanted == job.key {
                                    slot.ready = Some(ready);
                                    Some(slot.wake.clone())
                                } else {
                                    slot.stale += 1;
                                    None
                                };
                                let again = !slot.cancelled && slot.next.is_some();
                                if !again {
                                    slot.scheduled = false;
                                }
                                (wake, again)
                            };
                            if let Some(ctx) = wake {
                                ctx.request_repaint();
                            }
                            if !again {
                                break;
                            }
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(Pool { sender })
    })
    .as_ref()
    .map_err(Clone::clone)
}

#[derive(Default, Debug)]
pub(super) struct Importer {
    cell: Option<Arc<Cell>>,
    #[cfg(test)]
    gate: Option<Arc<(std::sync::Barrier, std::sync::Barrier)>>,
}

pub(super) fn delay_for_lab() {
    let delay = std::env::var("COMPD_MODEL_IMPORT_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(5000);
    if delay != 0 {
        std::thread::sleep(std::time::Duration::from_millis(delay));
    }
}

impl Drop for Importer {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Importer {
    pub fn cancel(&mut self) {
        if let Some(cell) = self.cell.take() {
            let mut slot = cell.lock().unwrap();
            slot.cancelled = true;
            slot.next = None;
            slot.ready = None;
        }
    }

    pub fn request(
        &mut self,
        source: &Value,
        session: &str,
        owner: &str,
        generation: u64,
        total: usize,
        ctx: &egui::Context,
    ) -> Option<Ready> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let key = Key {
            path: source["path"].as_str().unwrap_or("").into(),
            token: source["token"].as_str().unwrap_or("").into(),
            session: session.into(),
            owner: owner.into(),
            generation,
            total,
        };
        if key.path.len() > 4096
            || key.token.len() > 4096
            || key.session.len() > 4096
            || key.owner.len() > 4096
        {
            return Some(Ready {
                data: Err("source identity exceeds budget".into()),
                micros: 0,
                job: 0,
            });
        }
        let cell = self.cell.get_or_insert_with(|| {
            Arc::new(Mutex::new(Slot {
                wanted: key.clone(),
                next: None,
                ready: None,
                scheduled: false,
                wake: ctx.clone(),
                stale: 0,
                cancelled: false,
            }))
        });
        let mut slot = cell.lock().unwrap();
        slot.wake = ctx.clone();
        if slot.wanted == key && (slot.scheduled || slot.ready.is_some()) {
            return slot.ready.clone();
        }
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        slot.wanted = key.clone();
        slot.ready = None;
        slot.next = Some(Job {
            key,
            id,
            #[cfg(test)]
            gate: self.gate.clone(),
        });
        ui::frame_trace::event("table_model_submit", || (id, generation, total as u64));
        if !slot.scheduled {
            slot.scheduled = true;
            if let Err(error) = pool().and_then(|p| {
                p.sender
                    .try_send(Arc::downgrade(cell))
                    .map_err(|e| e.to_string())
            }) {
                slot.scheduled = false;
                slot.next = None;
                slot.ready = Some(Ready {
                    data: Err(format!("model preparation unavailable: {error}")),
                    micros: 0,
                    job: id,
                });
            }
        }
        slot.ready.clone()
    }

    pub fn snapshot(&self) -> Value {
        self.cell.as_ref().map_or(Value::Null, |cell| {
            let s = cell.lock().unwrap();
            json!({"pending":s.ready.is_none(),"generation":s.wanted.generation,
                "stale":s.stale,"job":s.ready.as_ref().map(|r|r.job)})
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(
        importer: &mut Importer,
        source: &Value,
        generation: u64,
        ctx: &egui::Context,
    ) -> Ready {
        let end = Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(ready) = importer.request(source, "s", "table", generation, 1, ctx) {
                return ready;
            }
            assert!(Instant::now() < end, "native preparation did not finish");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn blocked_import_coalesces_latest_keeps_other_pane_live_and_cancels_without_retaining_owner() {
        let ctx = egui::Context::default();
        let source = json!({"path":"/nonexistent/scene-import-test","token":"t"});
        let gate = Arc::new((std::sync::Barrier::new(2), std::sync::Barrier::new(2)));
        let mut left = Importer::default();
        left.gate = Some(gate.clone());
        assert!(left.request(&source, "s", "table", 1, 1, &ctx).is_none());
        gate.0.wait(); // Actual worker is stopped, not a simulated pending flag.
        let mut right = Importer::default();
        assert!(wait(&mut right, &source, 1, &ctx).data.is_err());
        left.gate = None;
        assert!(left.request(&source, "s", "table", 2, 1, &ctx).is_none());
        assert!(left.request(&source, "s", "table", 3, 1, &ctx).is_none());
        assert_eq!(
            left.cell
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .next
                .as_ref()
                .unwrap()
                .key
                .generation,
            3
        );
        gate.1.wait();
        assert!(wait(&mut left, &source, 3, &ctx).data.is_err());
        assert_eq!(left.snapshot()["stale"], 1);
        assert_eq!(left.snapshot()["generation"], 3);
        let gate = Arc::new((std::sync::Barrier::new(2), std::sync::Barrier::new(2)));
        left.gate = Some(gate.clone());
        assert!(left.request(&source, "s", "table", 4, 1, &ctx).is_none());
        gate.0.wait();
        let weak = Arc::downgrade(left.cell.as_ref().unwrap());
        left.cancel();
        gate.1.wait();
        let end = Instant::now() + std::time::Duration::from_secs(3);
        while weak.upgrade().is_some() {
            assert!(Instant::now() < end, "cancelled owner was retained");
            std::thread::yield_now();
        }
        assert!(left.snapshot().is_null());
    }
}
