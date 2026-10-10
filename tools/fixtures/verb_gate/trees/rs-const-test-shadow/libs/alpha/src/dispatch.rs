// Fixture (round 23): the production const ALPHA_X names alpha.ghost, which no registry lists. A
// #[cfg(test)] module redefines ALPHA_X as alpha.ping and matches on it. The test body is skipped,
// so its definition never shadows the production one: the production arm is unregistered
// (alpha.ghost), and alpha.ping has no answering site outside the test (not-in-code). Not compiled.

const ALPHA_X: &str = "alpha.ghost";

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_X => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    const ALPHA_X: &str = "alpha.ping";

    fn run_test(verb: &str) -> u8 {
        match verb {
            ALPHA_X => 1,
            _ => 0,
        }
    }
}
