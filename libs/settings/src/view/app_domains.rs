// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The MixOS app domains kept from today's model (§2.16): `terminal.*`,
//! `email.*`, `ced.*`, each decoded into its module's own record, so Term,
//! Email and Ced read exactly the types they read today.
use super::{Reader, View};
use crate::content::{self, Ced};
use crate::email::{self, Email};
use crate::registry::{self, Kind};
use crate::terminal::{self, Terminal};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppDomains {
    pub terminal: Terminal,
    pub email: Email,
    pub ced: Ced,
}

impl View for AppDomains {
    fn read(r: &Reader) -> Self {
        let mut view = Self::default();
        let leaves = registry::keys()
            .iter()
            .filter(|def| matches!(def.kind, Kind::AppDomain));
        for def in leaves {
            let Some(value) = r.authored(def.path) else {
                continue;
            };
            let Some((domain, rest)) = def.path.split_once('.') else {
                continue;
            };
            let field: Vec<&str> = rest.split('.').collect();
            // Validated values set without error; a leaf that would not
            // keeps its default, as every view treats a bad value.
            let _ = match domain {
                "terminal" => terminal::set(&mut view.terminal, def.path, &field, Some(value)),
                "email" => email::set(&mut view.email, def.path, &field, Some(value)),
                "ced" => content::set_ced(&mut view.ced, def.path, &field, Some(value)),
                _ => Ok(()),
            };
        }
        view
    }
}
