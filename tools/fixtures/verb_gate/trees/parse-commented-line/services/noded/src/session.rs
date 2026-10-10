impl Sessions {
    fn dispatch(&mut self, command: &SessionCommand) -> u8 {
        match command {
            // SessionCommand::Hello => 1,
            SessionCommand::SelfRecord(a) => a.len(),
            _ => 0,
        }
    }
}
