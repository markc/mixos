// Fixture: a listed test double whose only registration is a variant handler, SessionCommand::Hello.
// It is fresh (a variant handler is an answering site), and its answer is not an owner, so noded's
// own answer is the owner and the file is clean. Not compiled.

impl FakeSessions {
    fn dispatch(&mut self, command: &SessionCommand) -> u8 {
        match command {
            SessionCommand::Hello => 2,
            _ => 0,
        }
    }
}
