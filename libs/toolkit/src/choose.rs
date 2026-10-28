// SPDX-License-Identifier: MIT OR Apache-2.0
//! Non-blocking files.choose client, system chooser and in-process fallback.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use egui::{Context, ViewportId};
pub use files::chooser::{Choice, Defaults, Filter, Mode, OptionLabel, Refusal, Request};
use tokio::sync::watch;

mod native;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod platform;
#[cfg(all(target_os = "linux", feature = "portal"))]
mod portal;

const IO_BOUND: Duration = Duration::from_secs(2);
static SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Absolute paths as exact OS bytes, rather than lossy display strings.
    Chosen {
        paths: Vec<PathBuf>,
        choices: BTreeMap<String, String>,
        current_filter: Option<Filter>,
    },
    Cancelled,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Bus,
    System,
    InProcess,
}

enum Message {
    Tier(Tier),
    Answer(Outcome),
    Fallback,
}

#[derive(Clone)]
struct Emitter {
    tx: Sender<Message>,
    ctx: Context,
    viewport: ViewportId,
}

impl Emitter {
    fn send(&self, message: Message) {
        if self.tx.send(message).is_ok() {
            self.ctx.request_repaint_of(self.viewport);
        }
    }
}

impl Drop for Emitter {
    fn drop(&mut self) {
        // Also wake on an unwinding/dead worker so poll observes Disconnected.
        self.ctx.request_repaint_of(self.viewport);
    }
}

/// Parse the Bus request shape with toolkit's translated mode defaults.
pub fn request(body: &str) -> Result<Request, Refusal> {
    use crate::strings::own;
    Request::parse(
        body,
        &Defaults {
            open_title: own("file-chooser-open"),
            save_title: own("file-chooser-save"),
            save_files_title: own("file-chooser-save-files"),
            open_accept: own("file-chooser-open"),
            save_accept: own("file-chooser-save"),
        },
    )
}

/// Create a chooser. Poll it in the opening viewport's frame to bind and start
/// it. Construction and polling never wait for I/O or join a worker.
pub fn choose(mut request: Request) -> Handle {
    if request.correlation.is_none() {
        request.correlation = Some(format!("toolkit-{}", uuid::Uuid::now_v7()));
    }
    let (cancel, _) = watch::channel(false);
    Handle {
        request,
        receiver: None,
        cancel,
        viewport: None,
        tier: Tier::Bus,
        fallback: None,
        ended: false,
        #[cfg(test)]
        driver: None,
    }
}

pub struct Handle {
    #[cfg(test)]
    driver: Option<Box<dyn FnOnce(Request, watch::Receiver<bool>, Emitter) + Send>>,
    request: Request,
    receiver: Option<Receiver<Message>>,
    cancel: watch::Sender<bool>,
    viewport: Option<ViewportId>,
    tier: Tier,
    fallback: Option<crate::file_chooser::FileChooser>,
    ended: bool,
}

impl Handle {
    pub fn viewport(&self) -> Option<ViewportId> {
        self.viewport
    }
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Returns the outcome once. The first poll captures the opening viewport;
    /// polling from another viewport neither draws nor consumes its answer.
    pub fn poll(&mut self, ctx: &Context) -> Option<Outcome> {
        if self.ended {
            return None;
        }
        if self.viewport.is_some_and(|id| id != ctx.viewport_id()) {
            return None;
        }
        if *self.cancel.borrow() {
            self.ended = true;
            return Some(Outcome::Cancelled);
        }
        if self.receiver.is_none() {
            let viewport = ctx.viewport_id();
            self.viewport = Some(viewport);
            // Forward for the application, so separate compose windows share
            // remembered folders. eframe may omit the native root builder
            // from egui's state; toolkit's viewport helper retains that id.
            self.request.on_behalf_of = ctx
                .viewport_for(ViewportId::ROOT, |v| v.builder.app_id.clone())
                .or_else(crate::titlebar::app_id)
                .or_else(|| ctx.viewport(|v| v.builder.app_id.clone()))
                .or_else(|| self.request.on_behalf_of.clone());
            if let Err(error) = self.request.body() {
                self.ended = true;
                return Some(Outcome::Failed(error.message));
            }
            let (tx, rx) = mpsc::channel();
            self.receiver = Some(rx);
            let emitter = Emitter {
                tx,
                ctx: ctx.clone(),
                viewport,
            };
            let request = self.request.clone();
            let cancel = self.cancel.subscribe();
            #[cfg(test)]
            let driver = self.driver.take();
            if let Err(error) = std::thread::Builder::new()
                .name("toolkit-choose".into())
                .spawn(move || {
                    #[cfg(test)]
                    if let Some(driver) = driver {
                        driver(request, cancel, emitter);
                        return;
                    }
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build();
                    match runtime {
                        Ok(runtime) => runtime.block_on(remote(request, cancel, &emitter)),
                        Err(error) => {
                            emitter.send(Message::Answer(Outcome::Failed(error.to_string())))
                        }
                    }
                })
            {
                self.ended = true;
                return Some(Outcome::Failed(error.to_string()));
            }
        }
        loop {
            let message = match self.receiver.as_ref().expect("started").try_recv() {
                Ok(message) => message,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.fallback.is_none() {
                        self.start_fallback();
                    }
                    break;
                }
            };
            match message {
                Message::Tier(tier) => self.tier = tier,
                Message::Answer(outcome) => {
                    self.ended = true;
                    return Some(outcome);
                }
                Message::Fallback => {
                    self.start_fallback();
                }
            }
        }
        let chooser = self.fallback.as_mut()?;
        let outcome = match chooser.show(ctx)? {
            crate::file_chooser::Outcome::Cancelled => Outcome::Cancelled,
            crate::file_chooser::Outcome::Picked(paths) => chooser.chosen(paths),
            crate::file_chooser::Outcome::Save { path, .. } => chooser.chosen(vec![path]),
        };
        self.ended = true;
        Some(outcome)
    }

    fn start_fallback(&mut self) {
        self.tier = Tier::InProcess;
        self.fallback = Some(crate::file_chooser::FileChooser::from_request(
            self.request.clone(),
        ));
    }

    /// End this request. Native cancellation runs on the worker and is bounded
    /// by discovery/recovery I/O; polling returns Cancelled exactly once.
    pub fn cancel(&mut self) {
        self.cancel.send_replace(true);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
    }
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    while !*cancel.borrow_and_update() {
        if cancel.changed().await.is_err() {
            break;
        }
    }
}

async fn remote(request: Request, cancel: watch::Receiver<bool>, emitter: &Emitter) {
    let mut bus_cancel = cancel.clone();
    let mut system_cancel = cancel.clone();
    let bus = native::attempt(&request, &mut bus_cancel);
    #[cfg(all(target_os = "linux", feature = "portal"))]
    let system = portal::attempt(&request, &mut system_cancel);
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let system = platform::attempt(&request);
    #[cfg(not(any(
        all(target_os = "linux", feature = "portal"),
        target_os = "windows",
        target_os = "macos"
    )))]
    let system = async {
        let _ = &mut system_cancel;
        None
    };
    cascade(bus, system, cancel, emitter).await;
}

async fn cascade(
    bus: impl std::future::Future<Output = Option<Outcome>>,
    system: impl std::future::Future<Output = Option<Outcome>>,
    cancel: watch::Receiver<bool>,
    emitter: &Emitter,
) {
    emitter.send(Message::Tier(Tier::Bus));
    let outcome = bus.await;
    if *cancel.borrow() {
        emitter.send(Message::Answer(Outcome::Cancelled));
        return;
    }
    if final_outcome(&outcome) {
        emitter.send(Message::Answer(outcome.unwrap()));
        return;
    }
    emitter.send(Message::Tier(Tier::System));
    let outcome = system.await;
    if *cancel.borrow() {
        emitter.send(Message::Answer(Outcome::Cancelled));
        return;
    }
    if final_outcome(&outcome) {
        emitter.send(Message::Answer(outcome.unwrap()));
    } else {
        emitter.send(Message::Fallback);
    }
}

fn final_outcome(outcome: &Option<Outcome>) -> bool {
    outcome.is_some()
}

fn decode_outcome(request: &Request, value: &serde_json::Value) -> Outcome {
    use files::chooser::{Acceptance, location};
    let parsed = || -> anyhow::Result<Outcome> {
        match value["response"].as_u64() {
            Some(1) => return Ok(Outcome::Cancelled),
            Some(0) => {}
            _ => {
                return Ok(Outcome::Failed(
                    value["message"].as_str().unwrap_or("chooser failed").into(),
                ));
            }
        }
        let uris: Vec<String> = serde_json::from_value(value["uris"].clone())?;
        let paths = uris
            .iter()
            .map(|uri| location(&serde_json::json!(uri)).map_err(|e| anyhow::anyhow!(e.message)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut choices = request.choice_defaults();
        if let Some(values) = value.get("choices") {
            choices.extend(serde_json::from_value::<BTreeMap<String, String>>(
                values.clone(),
            )?);
        }
        let filter = value
            .get("current_filter")
            .map(Filter::parse)
            .transpose()
            .map_err(|e| anyhow::anyhow!(e.message))?
            .or_else(|| request.current_filter.clone());
        let folder = paths.first().and_then(|path| {
            path.iter().rposition(|&b| b == b'/').map(|i| {
                if i == 0 {
                    vec![b'/']
                } else {
                    path[..i].to_vec()
                }
            })
        });
        request
            .validate_acceptance(&Acceptance {
                paths: paths.clone(),
                folder,
                current_filter: filter.clone(),
                choices: choices.clone(),
            })
            .map_err(|e| anyhow::anyhow!(e.message))?;
        Ok(Outcome::Chosen {
            paths: paths.iter().map(|p| path_from_bytes(p)).collect(),
            choices,
            current_filter: filter,
        })
    };
    parsed().unwrap_or_else(|e| Outcome::Failed(e.to_string()))
}

#[cfg(any(target_os = "windows", target_os = "macos", test))]
fn platform_paths(request: &Request, mut paths: Vec<PathBuf>) -> Outcome {
    if request.mode == Mode::Save {
        for path in &mut paths {
            let Some(name) = path.file_name() else {
                return Outcome::Failed("save result has no filename".into());
            };
            let name = request.save_name(&path_bytes(Path::new(name)));
            if let Err(error) = files::chooser::validate_name(&name) {
                return Outcome::Failed(error.message);
            }
            path.set_file_name(path_from_bytes(&name));
        }
    }
    Outcome::Chosen {
        paths,
        choices: request.choice_defaults(),
        current_filter: None,
    }
}

#[cfg(unix)]
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(bytes.to_vec()).into()
}
#[cfg(unix)]
pub(crate) fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}
#[cfg(not(unix))]
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    String::from_utf8_lossy(bytes).as_ref().into()
}
#[cfg(not(unix))]
pub(crate) fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(test)]
mod tests;
