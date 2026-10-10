pub fn parse_session(message: &Message, body: &str) -> Result<SessionCommand, WireError> {
    let command = match message.command_name() {
        Some("noded.session.hello") => {
            args::<EmptyArgs>(body)?;
            SessionCommand::Hello
        }
        Some("noded.session.self") => {
            // A raw string may name SessionCommand::Hello without constructing it.
            let _doc = r#"SessionCommand::Hello and "quotes" { here"#;
            SessionCommand::SelfRecord(args(body)?)
        }
        _ => return Err(WireError("unknown session command")),
    };
    Ok(command)
}
