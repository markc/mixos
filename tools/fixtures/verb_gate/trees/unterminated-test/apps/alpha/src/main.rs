pub fn route(verb: &str) -> u8 {
    match verb {
        "alpha.ok" => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn never_closes() {
        let open = "{";
        assert!(open.len() == 1);
    }

// No closing brace for the test module: the rest of the file must not be skipped silently.
pub fn late(verb: &str) -> u8 {
    match verb {
        "alpha.late" => 2,
        _ => 0,
    }
}
