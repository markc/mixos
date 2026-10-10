// Fixture: bus parses noded.session.hello into SessionCommand::Hello. Not compiled.

pub fn parse_session(message: &Message, body: &str) -> Result<SessionCommand, WireError> {
    let command = match message.command_name() {
        Some("noded.session.hello") => {
            args::<EmptyArgs>(body)?;
            SessionCommand::Hello
        }
        _ => return Err(WireError("unknown session command")),
    };
    Ok(command)
}
