pub fn parse_session(message: &Message, body: &str) -> Result<SessionCommand, WireError> {
    let command = match message.command_name() {
        Some("noded.session.hello") => SessionCommand::Hello,
    /* a block comment that never closes
        Some("noded.session.self") => SessionCommand::SelfRecord(args(body)?),
        _ => return Err(WireError("unknown session command")),
    };
    Ok(command)
}
