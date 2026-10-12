use smithay::reexports::calloop::channel::Sender as CalloopSender;

use slots::launch::container::container::wrap_podman;
use slots::launch::dispatch::dispatch::LaunchWorker;
use slots::launch::execute::execute::execute;
use slots::launch::policy::policy::{LAUNCH_DISPATCH, LaunchDispatch};
use slots::launch::types::types::{LaunchOutcome, LaunchRequest};
use slots::storage::token::base::{Token, TokenMut};

/// App-launch driver. Held in kernel storage; populated by the loader's
/// `install`. Every outcome is posted on `outcome_tx` so orchestration can
/// broadcast it; the executor itself stays unaware of placeholders / the bus.
/// `scope` (adopt into a systemd transient scope) is decided once at install
/// from a runtime systemd probe, so the same value covers every launch.
pub struct Executor {
    worker: Option<LaunchWorker>,
    outcome_tx: CalloopSender<LaunchOutcome>,
    base_env: BaseEnv,
    scope: bool,
}

/// Produces the faithful base env for ONE launch.
///
/// A closure rather than a cached `Vec` deliberately. Every value in it is
/// the compositor's own — the socket we created, the desktop name we run under,
/// the X display of the Xwayland server we started — so it must be derived from
/// our authoritative state, never read back out of the process or session
/// environment. Reading the session back would mean FOLLOWING anything that
/// trampled `WAYLAND_DISPLAY` (a stray `systemctl --user import-environment`
/// re-importing the login manager's value, say) instead of overriding it,
/// which is the same class of bug the replay filter exists to prevent.
///
/// Recomputing per launch rather than snapshotting at install costs one
/// settings read on a user click, and it is what makes `DISPLAY` correct: our own
/// Xwayland picks its display number asynchronously, long after this closure is
/// built, so a snapshot taken at install would pin the empty value forever with
/// nothing to signal it.
pub type BaseEnv = Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

impl Executor {
    pub fn shutdown(&self) {
        if let Some(worker) = &self.worker {
            worker.shutdown();
        }
    }

    pub fn new(
        worker: Option<LaunchWorker>,
        outcome_tx: CalloopSender<LaunchOutcome>,
        base_env: BaseEnv,
        scope: bool,
    ) -> Self {
        Self {
            worker,
            outcome_tx,
            base_env,
            scope,
        }
    }

    /// Launch `request`. The faithful base env is prepended; caller-supplied
    /// entries (e.g. the activation token) override it. `Inline` returns the
    /// outcome immediately (and posts it for broadcast); `OffThread` returns
    /// `None` and the outcome is posted when the worker finishes.
    ///
    /// A containerised request is rewritten into `podman exec` HERE, after the
    /// env is final: the rewrite turns `env` into `--env` flags, and `podman
    /// exec` inherits nothing from us, so anything merged in after the rewrite
    /// would reach only the podman client and never the app. That ordering is
    /// what puts the live `WAYLAND_DISPLAY` inside the container — overriding
    /// whatever value was frozen into the container's config at `podman run`.
    pub fn launch(&self, mut request: LaunchRequest) -> Option<LaunchOutcome> {
        let mut env = (self.base_env)();
        env.append(&mut request.env);
        request.env = env;

        if let Some(container) = request.container.clone() {
            let start = request.start_container;
            wrap_podman(&mut request, &container, start);
        }

        if matches!(LAUNCH_DISPATCH, LaunchDispatch::OffThread) {
            if let Some(worker) = &self.worker {
                worker.submit(request);
                return None;
            }
        }
        let outcome = execute(&request, self.scope);
        let _ = self.outcome_tx.send(outcome.clone());
        Some(outcome)
    }
}

/// This compositor's Bus service name (`comp`, `comp-nested` or its
/// `--bus-service`), set once at start.
static COMP_SERVICE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Record the compositor's Bus service name for Print Screen's launches.
pub fn set_comp_service(name: &str) {
    let _ = COMP_SERVICE.set(name.to_owned());
}

/// Print Screen: launch the screenshot app for each press the key bindings
/// queued (`--capture region|screen|window`, with the window focused at the
/// press). A screenshot window already running takes the request from the
/// new process over the Bus, so a press never opens a second window. The
/// program is `screenshot` beside this compd (MixOS installs every binary
/// into one directory), else the first on `PATH`.
pub fn launch_screenshots(state: &mut world::state::Loop) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAUNCHES: AtomicU64 = AtomicU64::new(0);
    let launches = state.inner.comp.bindings.take_launches();
    if launches.is_empty() {
        return;
    }
    let program = std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("screenshot"))
        .filter(|path| path.is_file())
        .map_or_else(
            || "screenshot".to_owned(),
            |path| path.display().to_string(),
        );
    let Some(executor) = state.inner.kernel.get(&EXECUTOR).as_ref() else {
        warn!("Print Screen: no launch executor; screenshot not started");
        return;
    };
    // The screenshot captures through the compositor that saw the key.
    let comp = COMP_SERVICE.get().map_or("comp", String::as_str);
    for launch in launches {
        let n = LAUNCHES.fetch_add(1, Ordering::Relaxed);
        executor.launch(LaunchRequest {
            argv: launch.argv(&program, comp),
            env: Vec::new(),
            working_dir: None,
            token: String::new(),
            unit: format!("mixos-screenshot-{}-{n}", std::process::id()),
            correlation: None,
            container: None,
            start_container: false,
        });
    }
}

/// Kernel storage slot for the executor (driver data), set by the loader.
pub static EXECUTOR: Token<Option<Executor>> = Token::new();
pub static EXECUTOR_MUT: TokenMut<Option<Executor>> = TokenMut::new(&EXECUTOR);
