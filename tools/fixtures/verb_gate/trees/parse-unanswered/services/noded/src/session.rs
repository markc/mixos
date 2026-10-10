impl Sessions {
    fn dispatch(&mut self, command: &SessionCommand) -> u8 {
        match command {
            SessionCommand::SelfRecord(a) => a.len(),
            _ => 0,
        }
    }
}
