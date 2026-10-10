// Fixture (round 26): Alpha::ALPHA_NAME in a free fn. The only ALPHA_NAME definitions are the trait
// default and a fn-local const inside a helper method, and neither is an impl-level associated
// constant of Alpha, so the arm is unreadable (fail closed). Not compiled.
pub trait Named {
    const ALPHA_NAME: &'static str = "alpha.hidden";
}

pub struct Alpha;

impl Named for Alpha {
    fn helper(&self) -> &'static str {
        const ALPHA_NAME: &str = "alpha.ping";
        ALPHA_NAME
    }
}

pub fn run(verb: &str) -> u8 {
    match verb {
        Alpha::ALPHA_NAME => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
