// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Pressure stall information from `/proc/pressure/{cpu,memory,io}`: the
//! share of time some (or all) runnable tasks waited on a resource. Absent
//! on kernels built without PSI.
use super::Root;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Stall {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Resource {
    pub some: Stall,
    /// The CPU line has no meaningful `full` at the system level; older
    /// kernels omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<Stall>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Pressure {
    pub cpu: Option<Resource>,
    pub memory: Option<Resource>,
    pub io: Option<Resource>,
}

fn stall(rest: &str) -> Option<Stall> {
    let mut stall = Stall::default();
    for pair in rest.split_whitespace() {
        let (key, value) = pair.split_once('=')?;
        let value: f64 = value.parse().ok()?;
        match key {
            "avg10" => stall.avg10 = value,
            "avg60" => stall.avg60 = value,
            "avg300" => stall.avg300 = value,
            _ => {}
        }
    }
    Some(stall)
}

pub fn parse(text: &str) -> Option<Resource> {
    let mut some = None;
    let mut full = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("some ") {
            some = stall(rest);
        } else if let Some(rest) = line.strip_prefix("full ") {
            full = stall(rest);
        }
    }
    Some(Resource { some: some?, full })
}

pub fn read(root: &Root) -> Pressure {
    let get = |name: &str| root.read(&format!("proc/pressure/{name}")).and_then(|t| parse(&t));
    Pressure {
        cpu: get("cpu"),
        memory: get("memory"),
        io: get("io"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn some_and_full_lines_parse() {
        let r = parse(
            "some avg10=1.50 avg60=0.75 avg300=0.10 total=123\n\
             full avg10=0.20 avg60=0.00 avg300=0.00 total=4\n",
        )
        .unwrap();
        assert_eq!(r.some.avg10, 1.5);
        assert_eq!(r.full.unwrap().avg10, 0.2);
    }

    #[test]
    fn a_file_without_some_is_absent() {
        assert!(parse("").is_none());
    }
}
