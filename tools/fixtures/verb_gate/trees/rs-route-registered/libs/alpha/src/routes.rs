// Fixture: routing tables, read in full. A tuple row and a struct row name registered verbs
// (clean). A row whose first item is an unshaped literal names no verb and is ignored. Not
// compiled.

pub static ROUTES: [(&str, &str); 2] = [
    ("alpha.ping", "ready"),
    ("ready text", "ignored"),
];

pub static HANDLERS: &[Route] = &[
    Route { verb: "alpha.status", handler: "status" },
];
