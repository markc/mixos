// Fixture: a tuple row whose own verb is registered, and whose callback holds an unregistered guard.
static ROWS: [(&str, fn(&str) -> bool); 1] = [("alpha.ping", |verb| verb == "alpha.gone")];
