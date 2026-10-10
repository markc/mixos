// Fixture: noded answers SessionCommand::Hello. Not compiled.

impl Sessions {
    fn dispatch(&mut self, command: &SessionCommand) -> u8 {
        match command {
            SessionCommand::Hello => 1,
            _ => 0,
        }
    }
}
