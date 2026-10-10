pub fn parse_session(message: &Message, body: &str) -> Result<SessionCommand, WireError> {
    let command = match message.command_name() {
        Some("noded.session.hello") => {
            // One arm constructs two variants: the gate must refuse to pick one.
            let _extra = SessionCommand::SelfRecord(args(body)?);
            SessionCommand::Hello
        }
        Some("noded.session.self") => SessionCommand::SelfRecord(args(body)?),
        _ => return Err(WireError("unknown session command")),
    };
    Ok(command)
}
