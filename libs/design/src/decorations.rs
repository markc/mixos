// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Framing policy, with session I/O supplied by the windowing integration.
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::{Decorations, RequestedDecorations};

/// Desktops which keep the house title bar even with the decoration global.
pub const CLIENT_SIDE_DESKTOPS: &[&str] = &["MixOS"];

/// Facts about the app's backend, not the session's nominal display type.
#[derive(Clone, Debug)]
pub struct SessionFacts {
    pub wayland: bool,
    pub desktops: String,
    /// `None` covers failure, timeout and descriptor hand-off skips.
    pub decoration_global: Option<bool>,
}

/// An effective mode is always two-valued. The reason is part of Bus reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resolution {
    pub mode: Decorations,
    pub reason: &'static str,
}

pub fn automatic(facts: &SessionFacts) -> Resolution {
    let (mode, reason) = if !facts.wayland {
        (Decorations::Server, "rule1")
    } else if facts.desktops.split(':').any(|desktop| {
        CLIENT_SIDE_DESKTOPS
            .iter()
            .any(|name| desktop.eq_ignore_ascii_case(name))
    }) {
        (Decorations::Client, "rule2")
    } else if facts.decoration_global == Some(true) {
        (Decorations::Server, "rule3")
    } else {
        (Decorations::Client, "rule4")
    };
    Resolution { mode, reason }
}

/// The detector must return within the toolkit's detection budget. One
/// resolver is shared by the process; explicit choices never call it.
pub struct Resolver {
    detect: Box<dyn Fn() -> SessionFacts + Send + Sync>,
    environment: Box<dyn Fn() -> Option<String> + Send + Sync>,
    warn: Box<dyn Fn(&str) + Send + Sync>,
    facts: OnceLock<SessionFacts>,
    warned: AtomicBool,
}

impl Resolver {
    pub fn new(
        detect: impl Fn() -> SessionFacts + Send + Sync + 'static,
        environment: impl Fn() -> Option<String> + Send + Sync + 'static,
        warn: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        Self {
            detect: Box::new(detect),
            environment: Box::new(environment),
            warn: Box::new(warn),
            facts: OnceLock::new(),
            warned: AtomicBool::new(false),
        }
    }

    pub fn resolve(
        &self,
        choice: Option<RequestedDecorations>,
        session: RequestedDecorations,
    ) -> Resolution {
        let value = (self.environment)();
        let value = value.as_deref().unwrap_or_default().trim();
        let override_mode = if value.is_empty() {
            None
        } else {
            let parsed = RequestedDecorations::from_name(&value.to_ascii_lowercase());
            if parsed.is_none() && !self.warned.swap(true, Ordering::Relaxed) {
                (self.warn)(&format!(
                    "Ignoring MIXOS_DECORATIONS={value:?}: expected csd, ssd or auto"
                ));
            }
            parsed
        };
        let requested = override_mode.or(choice).unwrap_or(session);
        let mode = match requested {
            RequestedDecorations::Auto => {
                return automatic(self.facts.get_or_init(|| (self.detect)()));
            }
            RequestedDecorations::Client => Decorations::Client,
            RequestedDecorations::Server => Decorations::Server,
        };
        Resolution {
            mode,
            reason: if override_mode.is_some() {
                "override"
            } else {
                "choice"
            },
        }
    }
}

static PROCESS: OnceLock<Arc<Resolver>> = OnceLock::new();

/// Called by toolkit after the windowing backend is known. This performs no
/// detection; the first winning `auto` does that lazily.
pub fn install(resolver: Arc<Resolver>) {
    let _ = PROCESS.set(resolver);
}

thread_local! {
    static INJECTED: RefCell<Option<Arc<Resolver>>> = const { RefCell::new(None) };
}

/// Run a synchronous engine/UI operation against injected facts and env.
/// Thread-local scoping keeps parallel tests independent of the real session.
pub fn with_resolver<T>(resolver: Arc<Resolver>, operation: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<Resolver>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            INJECTED.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(INJECTED.with(|slot| slot.replace(Some(resolver))));
    operation()
}

pub fn resolve(choice: Option<RequestedDecorations>, session: RequestedDecorations) -> Resolution {
    let injected = INJECTED.with(|slot| slot.borrow().clone());
    if let Some(resolver) = injected.as_ref().or_else(|| PROCESS.get()) {
        return resolver.resolve(choice, session);
    }
    // Headless engines and offscreen UI have no windowing integration. They
    // read neither the environment nor the real session.
    let mode = match choice.unwrap_or(session) {
        RequestedDecorations::Auto => {
            return automatic(&SessionFacts {
                wayland: true,
                desktops: String::new(),
                decoration_global: None,
            });
        }
        RequestedDecorations::Client => Decorations::Client,
        RequestedDecorations::Server => Decorations::Server,
    };
    Resolution {
        mode,
        reason: "choice",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RequestedDecorations::{Auto, Client, Server};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    fn facts(wayland: bool, desktops: &str, global: Option<bool>) -> SessionFacts {
        SessionFacts {
            wayland,
            desktops: desktops.into(),
            decoration_global: global,
        }
    }

    #[test]
    fn requested_names_are_closed_and_effective_names_stay_two_valued() {
        assert_eq!(RequestedDecorations::default(), Auto);
        for requested in RequestedDecorations::ALL {
            assert_eq!(
                RequestedDecorations::from_name(requested.name()),
                Some(requested)
            );
        }
        assert_eq!(RequestedDecorations::from_name("bogus"), None);
        assert_eq!(Decorations::from_name("auto"), None);
        assert_eq!(Decorations::ALL.map(Decorations::name), ["csd", "ssd"]);
    }

    #[test]
    fn automatic_rules_use_the_actual_backend_and_desktop_list() {
        for desktop in ["", "MixOS", "KDE", "ubuntu:GNOME"] {
            for global in [None, Some(false), Some(true)] {
                assert_eq!(
                    automatic(&facts(false, desktop, global)),
                    Resolution {
                        mode: Decorations::Server,
                        reason: "rule1",
                    }
                );
            }
        }
        for desktop in ["MixOS", "foo:mixos", "KDE:MixOS"] {
            assert_eq!(
                automatic(&facts(true, desktop, Some(true))),
                Resolution {
                    mode: Decorations::Client,
                    reason: "rule2",
                }
            );
        }
        assert_eq!(
            automatic(&facts(true, "KDE", Some(true))),
            Resolution {
                mode: Decorations::Server,
                reason: "rule3",
            }
        );
        for desktop in ["GNOME", "ubuntu:GNOME"] {
            assert_eq!(
                automatic(&facts(true, desktop, Some(false))),
                Resolution {
                    mode: Decorations::Client,
                    reason: "rule4",
                }
            );
        }
    }

    #[test]
    fn failed_timed_out_or_skipped_detection_uses_rule4() {
        assert_eq!(
            automatic(&facts(true, "KDE", None)),
            Resolution {
                mode: Decorations::Client,
                reason: "rule4",
            }
        );
    }

    #[test]
    fn overrides_accept_case_and_whitespace_including_auto() {
        for value in ["ssd", " SSD ", "\tSsD\n"] {
            let r = Resolver::new(
                || facts(true, "KDE", Some(true)),
                move || Some(value.into()),
                |_| {},
            );
            assert_eq!(
                r.resolve(None, Client),
                Resolution {
                    mode: Decorations::Server,
                    reason: "override",
                }
            );
        }
        let r = Resolver::new(
            || facts(true, "KDE", Some(true)),
            || Some(" AuTo ".into()),
            |_| {},
        );
        assert_eq!(
            r.resolve(Some(Client), Client),
            Resolution {
                mode: Decorations::Server,
                reason: "rule3",
            }
        );
    }

    #[test]
    fn invalid_override_warns_once_and_empty_is_unset() {
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let seen = warnings.clone();
        let r = Resolver::new(
            || facts(true, "KDE", Some(true)),
            || Some("bogus".into()),
            move |warning| {
                seen.lock().unwrap().push(warning.to_owned());
            },
        );
        assert_eq!(r.resolve(Some(Server), Client).reason, "choice");
        assert_eq!(r.resolve(None, Client).mode, Decorations::Client);
        let warnings = warnings.lock().unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("MIXOS_DECORATIONS") && warnings[0].contains("bogus"));
        for value in [None, Some(String::new()), Some("  ".into())] {
            let r = Resolver::new(|| facts(false, "", None), move || value.clone(), |_| {});
            assert_eq!(r.resolve(None, Server).reason, "choice");
        }
    }

    #[test]
    fn explicit_choices_never_detect_and_runtime_auto_detects_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let r = Resolver::new(
            move || {
                count.fetch_add(1, Ordering::Relaxed);
                facts(true, "KDE", Some(true))
            },
            || None,
            |_| {},
        );
        for session in [Client, Server] {
            assert_eq!(r.resolve(None, session).reason, "choice");
            assert_eq!(r.resolve(Some(session), Auto).reason, "choice");
        }
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(r.resolve(Some(Server), Client).mode, Decorations::Server);
        assert_eq!(r.resolve(None, Server).mode, Decorations::Server);
        let started = Instant::now();
        assert_eq!(r.resolve(Some(Auto), Client).reason, "rule3");
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(r.resolve(None, Auto).reason, "rule3");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn override_keeps_the_choice_and_removal_reveals_it() {
        let variable = Arc::new(Mutex::new(Some("csd".to_owned())));
        let env = variable.clone();
        let r = Resolver::new(
            || facts(true, "KDE", Some(true)),
            move || env.lock().unwrap().clone(),
            |_| {},
        );
        let mut choice = Some(Client);
        assert_eq!(r.resolve(choice, Auto).reason, "override");
        choice = Some(Server);
        assert_eq!(
            r.resolve(choice, Auto),
            Resolution {
                mode: Decorations::Client,
                reason: "override",
            }
        );
        *variable.lock().unwrap() = None;
        assert_eq!(
            r.resolve(choice, Auto),
            Resolution {
                mode: Decorations::Server,
                reason: "choice",
            }
        );
    }
}
