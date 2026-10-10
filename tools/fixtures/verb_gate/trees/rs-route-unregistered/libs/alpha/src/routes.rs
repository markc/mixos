// Fixture: a routing table whose tuple rows name verbs (Sol's reproducer). The row for
// alpha.gone names a verb the registry does not hold: unregistered. Not compiled.

pub const ROUTES: &[(&str, &str)] = &[
    ("alpha.ping", "ready"),
    ("alpha.gone", "gone"),
];

pub const WRAPPED: &[(&str, &str)] =
    &[("alpha.wrapped", "wrapped")];
