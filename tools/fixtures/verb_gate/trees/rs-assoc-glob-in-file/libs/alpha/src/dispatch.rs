// Fixture (round 28): Alpha is declared once, at file level, with its own ALPHA_NAME = "alpha.ping".
// The file also holds a glob import (use std::fmt::*), which can bind any name, so Alpha::ALPHA_NAME
// is not read and the arm is unreadable (fail closed). Not compiled.
use std::fmt::*;

pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";
}

pub fn run(verb: &str) -> u8 {
    match verb {
        Alpha::ALPHA_NAME => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
