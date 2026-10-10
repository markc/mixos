// Fixture: a verb match whose opening brace is on the next line, so its arms cannot be
// read from the match line. Not compiled.

pub fn run(verb: &str) -> u8
{
    match verb
    {
        "alpha.other" => 2,
        _ => 0,
    }
}
