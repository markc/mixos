pub fn parse_session(message: &Message, body: &str) -> Result<SessionCommand, WireError> {
    let command = match message.command_name() {
        Some("noded.session.hello") => {
            // Unlike SessionCommand::SelfRecord, Hello accepts no args.
            args::<EmptyArgs>(body)?;
            SessionCommand::Hello
        }
        Some("noded.session.self") => SessionCommand::SelfRecord(args(body)?),
        _ => return Err(WireError("unknown session command")),
    };
    Ok(command)
}
