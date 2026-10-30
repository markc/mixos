// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Framing policy, with session I/O supplied by the windowing integration.
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::{ActionsPlace, Decorations, RequestedDecorations};

/// Desktops which keep the house title bar even with the decoration global.
pub const CLIENT_SIDE_DESKTOPS: &[&str] = &["MixOS"];

/// Whether any desktop in the colon-separated list prefers the app title bar.
pub fn prefers_client_side(desktops: &str) -> bool {
    desktops.split(':').any(|desktop| {
        CLIENT_SIDE_DESKTOPS
            .iter()
            .any(|name| desktop.eq_ignore_ascii_case(name))
    })
}

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
    automatic_with_actions(facts, ActionsPlace::Row)
}

/// Placement belongs to the theme, while session facts are process-wide.
pub fn automatic_with_actions(facts: &SessionFacts, actions: ActionsPlace) -> Resolution {
    let host_desktop = prefers_client_side(&facts.desktops);
    let (mode, reason) = if !facts.wayland {
        // X11 has no window menu or shadow request for an app-drawn bar.
        (Decorations::Server, "rule1")
    } else if actions == ActionsPlace::Titlebar && !host_desktop {
        // Only compd can host the app's Action Bar in a server title bar.
        (Decorations::Client, "rule3b")
    } else if host_desktop {
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
    /// `detect` must itself be bounded within AD-5's time limit because
    /// `resolve` calls it synchronously on the first winning `auto` request.
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
        self.resolve_with_actions(choice, session, ActionsPlace::Row)
    }

    pub fn facts(&self) -> &SessionFacts {
        self.facts.get_or_init(|| (self.detect)())
    }

    pub fn resolve_with_actions(
        &self,
        choice: Option<RequestedDecorations>,
        session: RequestedDecorations,
        actions: ActionsPlace,
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
                return automatic_with_actions(self.facts(), actions);
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

/// Without an installed resolver, uses headless facts without reading the
/// environment or detecting a display, for engines and offscreen UI.
pub fn resolve(choice: Option<RequestedDecorations>, session: RequestedDecorations) -> Resolution {
    resolve_with_actions(choice, session, ActionsPlace::Row)
}

/// Whether the installed session can host app menus and Action Bars.
/// Headless callers have no host; tests can inject the resolver.
pub fn has_host() -> bool {
    let injected = INJECTED.with(|slot| slot.borrow().clone());
    injected
        .as_ref()
        .or_else(|| PROCESS.get())
        .is_some_and(|r| {
            let facts = r.facts();
            facts.wayland && prefers_client_side(&facts.desktops)
        })
}

pub fn is_wayland() -> bool {
    let injected = INJECTED.with(|slot| slot.borrow().clone());
    injected
        .as_ref()
        .or_else(|| PROCESS.get())
        .is_some_and(|r| r.facts().wayland)
}

pub fn resolve_with_actions(
    choice: Option<RequestedDecorations>,
    session: RequestedDecorations,
    actions: ActionsPlace,
) -> Resolution {
    let injected = INJECTED.with(|slot| slot.borrow().clone());
    if let Some(resolver) = injected.as_ref().or_else(|| PROCESS.get()) {
        return resolver.resolve_with_actions(choice, session, actions);
    }
    // Headless engines and offscreen UI have no windowing integration. They
    // read neither the environment nor the real session.
    let mode = match choice.unwrap_or(session) {
        RequestedDecorations::Auto => {
            return automatic_with_actions(
                &SessionFacts {
                    wayland: true,
                    desktops: String::new(),
                    decoration_global: None,
                },
                actions,
            );
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
    fn titlebar_actions_are_per_theme_and_explicit_modes_win() {
        let r = Resolver::new(|| facts(true, "KDE", Some(true)), || None, |_| {});
        for _ in 0..2 {
            assert_eq!(
                r.resolve_with_actions(None, Auto, ActionsPlace::Titlebar),
                Resolution {
                    mode: Decorations::Client,
                    reason: "rule3b",
                }
            );
            assert_eq!(
                r.resolve_with_actions(None, Auto, ActionsPlace::Row).reason,
                "rule3"
            );
        }
        for mode in [Client, Server] {
            assert_eq!(
                r.resolve_with_actions(Some(mode), Auto, ActionsPlace::Titlebar)
                    .reason,
                "choice"
            );
        }
        let r = Resolver::new(
            || facts(true, "KDE", Some(true)),
            || Some("ssd".into()),
            |_| {},
        );
        assert_eq!(
            r.resolve_with_actions(None, Auto, ActionsPlace::Titlebar)
                .mode,
            Decorations::Server
        );
        assert_eq!(
            automatic_with_actions(&facts(false, "KDE", Some(true)), ActionsPlace::Titlebar).reason,
            "rule1"
        );
        assert_eq!(
            automatic_with_actions(
                &facts(true, "KDE:MixOS", Some(true)),
                ActionsPlace::Titlebar
            )
            .reason,
            "rule2"
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
