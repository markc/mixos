// Fixture: a fn-pointer array whose closure guard names an unregistered verb. The array is a
// routing-shaped table, but its element is a closure, not a tuple row; its guard is still read.
static HANDLERS: [fn(&str) -> bool; 1] = [|verb| verb == "alpha.gone"];
