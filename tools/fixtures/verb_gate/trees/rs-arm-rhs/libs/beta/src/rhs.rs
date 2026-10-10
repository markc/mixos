// Fixture: a client maps its operations to the verb names it sends. The literals are return
// values of the arms, not patterns, and alpha answers alpha.ping, not beta.

pub enum Op {
    Ping,
}

impl Op {
    fn verb(&self) -> &'static str {
        match self {
            Op::Ping => "alpha.ping",
        }
    }
}
