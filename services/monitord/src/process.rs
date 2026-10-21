// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The process table: one scan of `/proc/<pid>`, CPU shares between two
//! scans, and the table sorted, filtered or laid out as a tree.
//!
//! What an unprivileged reader cannot see (another user's I/O counters, for
//! one) is `None`, never zero.
use crate::kernel::{Root, Units, field, field_u64};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::MetadataExt;

/// The kernel marks its own threads with this flag in `stat`.
const PF_KTHREAD: u64 = 0x0020_0000;
/// Command lines longer than this are cut in the table; `monitord.process`
/// returns the whole of one.
const CMDLINE_TABLE: usize = 512;

/// What one scan reads for a process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raw {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
    pub state: char,
    pub uid: u32,
    pub kernel_thread: bool,
    pub threads: u64,
    pub nice: i64,
    pub priority: i64,
    /// User plus system time, in clock ticks.
    pub cpu_ticks: u64,
    /// Since boot, in clock ticks: with the pid, it tells a process from a
    /// later one that reused its pid.
    pub start_ticks: u64,
    pub rss_pages: u64,
    pub vsize: u64,
    pub cmdline: String,
    pub read_bytes: Option<u64>,
    pub write_bytes: Option<u64>,
    pub cgroup: Option<String>,
}

/// The fields of `/proc/<pid>/stat`. The command name sits in parentheses
/// and may itself hold spaces and parentheses, so the fields after it are
/// counted from the last `)`.
pub fn parse_stat(text: &str) -> Option<Raw> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid = text[..open].trim().parse().ok()?;
    let comm = text.get(open + 1..close)?.to_owned();
    let f: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // f[0] is field 3 (state) in proc(5)'s numbering.
    let n = |field: usize| f.get(field - 3)?.parse::<u64>().ok();
    let i = |field: usize| f.get(field - 3)?.parse::<i64>().ok();
    Some(Raw {
        pid,
        comm,
        state: f.first()?.chars().next()?,
        ppid: n(4)? as u32,
        kernel_thread: n(9)? & PF_KTHREAD != 0,
        cpu_ticks: n(14)? + n(15)?,
        priority: i(18)?,
        nice: i(19)?,
        threads: n(20)?,
        start_ticks: n(22)?,
        vsize: n(23)?,
        rss_pages: n(24)?,
        ..Raw::default()
    })
}

/// `/proc/<pid>/io`: bytes that went to and from storage. Readable only
/// for one's own processes, or with `CAP_SYS_PTRACE`.
pub fn parse_io(text: &str) -> (Option<u64>, Option<u64>) {
    (
        field_u64(text, "read_bytes"),
        field_u64(text, "write_bytes"),
    )
}

/// The cgroup v2 path (`0::/...`), else the first v1 line's path.
pub fn parse_cgroup(text: &str) -> Option<String> {
    let line = text
        .lines()
        .find(|l| l.starts_with("0::"))
        .or_else(|| text.lines().next())?;
    Some(line.splitn(3, ':').nth(2)?.to_owned())
}

/// The systemd unit a cgroup path ends in: `user@1000.service`,
/// `app-firefox-1234.scope`. A process in a unit's sub-cgroup is still the
/// unit's.
pub fn unit(cgroup: &str) -> Option<String> {
    cgroup
        .rsplit('/')
        .find(|part| part.ends_with(".service") || part.ends_with(".scope"))
        .map(str::to_owned)
}

fn cmdline(bytes: &[u8], limit: usize) -> String {
    let mut text: String = String::from_utf8_lossy(bytes)
        .split('\0')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.len() > limit {
        let mut cut = limit;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push('…');
    }
    text
}

/// Read one process. `None` when it exited during the read.
pub fn read(root: &Root, pid: u32, cmdline_limit: usize) -> Option<Raw> {
    let dir = root.join(&format!("proc/{pid}"));
    let uid = std::fs::metadata(&dir).ok()?.uid();
    let mut raw = parse_stat(&std::fs::read_to_string(dir.join("stat")).ok()?)?;
    raw.uid = uid;
    if !raw.kernel_thread {
        raw.cmdline = std::fs::read(dir.join("cmdline"))
            .map(|b| cmdline(&b, cmdline_limit))
            .unwrap_or_default();
        if let Ok(io) = std::fs::read_to_string(dir.join("io")) {
            (raw.read_bytes, raw.write_bytes) = parse_io(&io);
        }
    }
    raw.cgroup = std::fs::read_to_string(dir.join("cgroup"))
        .ok()
        .and_then(|t| parse_cgroup(&t));
    Some(raw)
}

/// Every process in `/proc`, read once.
pub fn scan(root: &Root) -> Vec<Raw> {
    let Ok(entries) = std::fs::read_dir(root.join("proc")) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| read(root, pid, CMDLINE_TABLE))
        .collect()
}

/// One process as the table shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub cmdline: String,
    pub user: String,
    pub uid: u32,
    pub state: String,
    /// Percent of the whole machine (every core) over the last interval,
    /// as Task Manager shows it: one busy core of eight is 12.5.
    pub cpu: f64,
    /// Resident memory, bytes.
    pub memory: u64,
    pub virtual_memory: u64,
    pub threads: u64,
    pub nice: i64,
    pub priority: i64,
    /// Unix seconds.
    pub started_at: u64,
    /// Storage bytes per second over the last interval, when readable.
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub unit: Option<String>,
    pub kernel_thread: bool,
    /// Set only in tree order: how deep under its root the process is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

fn state_name(state: char) -> &'static str {
    match state {
        'R' => "running",
        'S' => "sleeping",
        'D' => "disk-sleep",
        'Z' => "zombie",
        'T' => "stopped",
        't' => "traced",
        'I' => "idle",
        'X' | 'x' => "dead",
        'P' => "parked",
        'W' => "waking",
        _ => "unknown",
    }
}

/// User names by uid, from the root's `/etc/passwd`. A uid not listed
/// shows as its number.
#[derive(Clone, Debug, Default)]
pub struct Users(HashMap<u32, String>);

impl Users {
    pub fn read(root: &Root) -> Self {
        Self::parse(&root.read("etc/passwd").unwrap_or_default())
    }

    pub fn parse(passwd: &str) -> Self {
        Self(
            passwd
                .lines()
                .filter_map(|line| {
                    let mut f = line.split(':');
                    let name = f.next()?;
                    let uid = f.nth(1)?.parse().ok()?;
                    Some((uid, name.to_owned()))
                })
                .collect(),
        )
    }

    pub fn name(&self, uid: u32) -> String {
        self.0.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
    }

    pub fn uid(&self, name: &str) -> Option<u32> {
        name.parse()
            .ok()
            .or_else(|| self.0.iter().find(|(_, n)| *n == name).map(|(uid, _)| *uid))
    }
}

/// Turn two scans `dt` seconds apart into the table. A process only in the
/// later scan (new, or a reused pid) has no CPU share or I/O rate yet.
pub fn table(
    before: &[Raw],
    after: &[Raw],
    dt: f64,
    cores: usize,
    boot_time: u64,
    units: Units,
    users: &Users,
) -> Vec<Process> {
    let previous: HashMap<(u32, u64), &Raw> =
        before.iter().map(|r| ((r.pid, r.start_ticks), r)).collect();
    let capacity = dt * units.ticks_per_s as f64 * cores.max(1) as f64;
    after
        .iter()
        .map(|r| {
            let prev = previous.get(&(r.pid, r.start_ticks));
            let cpu = match prev {
                Some(p) if capacity > 0.0 => {
                    (r.cpu_ticks.saturating_sub(p.cpu_ticks) as f64 * 100.0 / capacity)
                        .clamp(0.0, 100.0)
                }
                _ => 0.0,
            };
            let io_rate =
                |now: Option<u64>, then: Option<u64>| crate::kernel::rate(then?, now?, dt);
            Process {
                pid: r.pid,
                ppid: r.ppid,
                name: r.comm.clone(),
                cmdline: r.cmdline.clone(),
                user: users.name(r.uid),
                uid: r.uid,
                state: state_name(r.state).to_owned(),
                cpu,
                memory: r.rss_pages * units.page_bytes,
                virtual_memory: r.vsize,
                threads: r.threads,
                nice: r.nice,
                priority: r.priority,
                started_at: boot_time + r.start_ticks / units.ticks_per_s.max(1),
                read_bps: prev.and_then(|p| io_rate(r.read_bytes, p.read_bytes)),
                write_bps: prev.and_then(|p| io_rate(r.write_bytes, p.write_bytes)),
                unit: r.cgroup.as_deref().and_then(unit),
                kernel_thread: r.kernel_thread,
                depth: None,
            }
        })
        .collect()
}

/// How a caller wants the table.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Query {
    /// `cpu` (the default), `memory`, `io`, `pid`, `name` or `user`.
    pub sort: Option<String>,
    /// Ascending instead of the sort's natural order (largest first for
    /// numbers, A to Z for names).
    pub reverse: bool,
    /// At most this many rows; 0 is every row. The default is 100.
    pub limit: Option<usize>,
    /// Rows whose name or command line contains this, ignoring case.
    pub filter: Option<String>,
    /// Only this user's processes, by name or uid.
    pub user: Option<String>,
    /// Parents before children, siblings in sort order, each row with its
    /// depth. A filter keeps a match's ancestors so the tree still joins.
    pub tree: bool,
    /// Include the kernel's own threads.
    pub kernel_threads: bool,
}

pub const DEFAULT_LIMIT: usize = 100;
pub const MAX_LIMIT: usize = 20_000;

fn compare(sort: &str, a: &Process, b: &Process) -> std::cmp::Ordering {
    let io = |p: &Process| p.read_bps.unwrap_or(0.0) + p.write_bps.unwrap_or(0.0);
    match sort {
        "memory" => b.memory.cmp(&a.memory),
        "io" => io(b).total_cmp(&io(a)),
        "pid" => a.pid.cmp(&b.pid),
        "name" => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        "user" => a.user.cmp(&b.user),
        _ => b.cpu.total_cmp(&a.cpu),
    }
    .then(a.pid.cmp(&b.pid))
}

pub const SORTS: &[&str] = &["cpu", "memory", "io", "pid", "name", "user"];

/// Apply a query. Returns the rows and how many matched before the limit.
pub fn select(
    mut rows: Vec<Process>,
    query: &Query,
    users: &Users,
) -> Result<(Vec<Process>, usize), String> {
    let sort = query.sort.as_deref().unwrap_or("cpu");
    if !SORTS.contains(&sort) {
        return Err(format!(
            "unknown sort {sort:?}; one of {}",
            SORTS.join(", ")
        ));
    }
    let uid = match &query.user {
        Some(user) => Some(
            users
                .uid(user)
                .ok_or_else(|| format!("unknown user {user:?}"))?,
        ),
        None => None,
    };
    let needle = query.filter.as_ref().map(|f| f.to_lowercase());
    let matches = |p: &Process| {
        (query.kernel_threads || !p.kernel_thread)
            && uid.is_none_or(|uid| p.uid == uid)
            && needle.as_ref().is_none_or(|n| {
                p.name.to_lowercase().contains(n) || p.cmdline.to_lowercase().contains(n)
            })
    };
    let order = |a: &Process, b: &Process| {
        let o = compare(sort, a, b);
        if query.reverse { o.reverse() } else { o }
    };
    let limit = match query.limit.unwrap_or(DEFAULT_LIMIT) {
        0 => MAX_LIMIT,
        n => n.min(MAX_LIMIT),
    };
    if query.tree {
        rows.retain(|p| query.kernel_threads || !p.kernel_thread);
        let kept = tree(rows, &matches, &order);
        let total = kept.len();
        return Ok((kept.into_iter().take(limit).collect(), total));
    }
    rows.retain(matches);
    rows.sort_by(order);
    let total = rows.len();
    rows.truncate(limit);
    Ok((rows, total))
}

/// Depth-first order: each root, then its children, siblings sorted. A
/// process whose parent isn't in the table is a root. Rows that don't
/// match are kept when a descendant does, so every match keeps its path.
fn tree(
    rows: Vec<Process>,
    matches: &dyn Fn(&Process) -> bool,
    order: &dyn Fn(&Process, &Process) -> std::cmp::Ordering,
) -> Vec<Process> {
    let by_pid: BTreeMap<u32, usize> = rows.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    let mut children: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut roots = Vec::new();
    for (i, p) in rows.iter().enumerate() {
        match by_pid.get(&p.ppid) {
            Some(_) if p.ppid != p.pid => children.entry(p.ppid).or_default().push(i),
            _ => roots.push(i),
        }
    }
    let sort = |list: &mut Vec<usize>| list.sort_by(|a, b| order(&rows[*a], &rows[*b]));
    sort(&mut roots);
    for list in children.values_mut() {
        sort(list);
    }
    // Which rows stay: a match, or an ancestor of one.
    let mut keep = vec![false; rows.len()];
    for (i, p) in rows.iter().enumerate() {
        if !matches(p) {
            continue;
        }
        let mut at = i;
        // Bounded by the row count, so a parent loop can't spin forever.
        for _ in 0..rows.len() {
            if keep[at] {
                break;
            }
            keep[at] = true;
            match by_pid.get(&rows[at].ppid) {
                Some(&parent) if parent != at => at = parent,
                _ => break,
            }
        }
    }
    let mut out = Vec::new();
    let mut visited = vec![false; rows.len()];
    let mut stack: Vec<(usize, u32)> = roots.iter().rev().map(|&i| (i, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        if std::mem::replace(&mut visited[i], true) {
            continue;
        }
        if keep[i] {
            let mut row = rows[i].clone();
            row.depth = Some(depth);
            out.push(row);
        }
        if let Some(kids) = children.get(&rows[i].pid) {
            stack.extend(kids.iter().rev().map(|&k| (k, depth + 1)));
        }
    }
    out
}

/// One process in full, for `monitord.process`.
pub fn detail(
    root: &Root,
    pid: u32,
    units: Units,
    users: &Users,
    boot_time: u64,
) -> Option<serde_json::Value> {
    let raw = read(root, pid, usize::MAX)?;
    let dir = root.join(&format!("proc/{pid}"));
    let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
    let kb = |key: &str| field_u64(&status, key).map(|v| v * 1024);
    let fds = std::fs::read_dir(dir.join("fd")).ok().map(|d| d.count());
    let exe = std::fs::read_link(dir.join("exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let cwd = std::fs::read_link(dir.join("cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let children: Vec<u32> = scan_children(root, pid);
    Some(serde_json::json!({
        "pid": raw.pid,
        "ppid": raw.ppid,
        "name": raw.comm,
        "cmdline": raw.cmdline,
        "exe": exe,
        "cwd": cwd,
        "user": users.name(raw.uid),
        "uid": raw.uid,
        "state": state_name(raw.state),
        "kernel_thread": raw.kernel_thread,
        "threads": raw.threads,
        "nice": raw.nice,
        "priority": raw.priority,
        "started_at": boot_time + raw.start_ticks / units.ticks_per_s.max(1),
        "cpu_time_s": raw.cpu_ticks as f64 / units.ticks_per_s.max(1) as f64,
        "memory": raw.rss_pages * units.page_bytes,
        "virtual_memory": raw.vsize,
        "memory_peak": kb("VmHWM"),
        "swap": kb("VmSwap"),
        "read_bytes": raw.read_bytes,
        "write_bytes": raw.write_bytes,
        "open_files": fds,
        "voluntary_switches": field_u64(&status, "voluntary_ctxt_switches"),
        "involuntary_switches": field_u64(&status, "nonvoluntary_ctxt_switches"),
        "cgroup": raw.cgroup,
        "unit": raw.cgroup.as_deref().and_then(unit),
        "children": children,
        "state_detail": field(&status, "State"),
    }))
}

/// The pids whose parent is `pid`, from a fresh scan of the stat files.
fn scan_children(root: &Root, pid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(root.join("proc")) else {
        return Vec::new();
    };
    let mut kids: Vec<u32> = entries
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|child| {
            std::fs::read_to_string(root.join(&format!("proc/{child}/stat")))
                .ok()
                .and_then(|t| parse_stat(&t))
                .is_some_and(|r| r.ppid == pid)
        })
        .collect();
    kids.sort_unstable();
    kids
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNITS: Units = Units {
        ticks_per_s: 100,
        page_bytes: 4096,
    };

    fn stat(pid: u32, comm: &str, ppid: u32, ticks: u64) -> String {
        format!(
            "{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 4194560 100 0 0 0 {ticks} 0 0 0 20 0 3 0 500 1048576 256 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 2 0 0 0 0 0"
        )
    }

    #[test]
    fn stat_parses_a_name_with_spaces_and_parens() {
        let raw = parse_stat(&stat(42, "Web Content (x)", 1, 250)).unwrap();
        assert_eq!(raw.pid, 42);
        assert_eq!(raw.comm, "Web Content (x)");
        assert_eq!(raw.ppid, 1);
        assert_eq!(raw.cpu_ticks, 250);
        assert_eq!(raw.threads, 3);
        assert_eq!(raw.start_ticks, 500);
        assert_eq!(raw.rss_pages, 256);
        assert!(!raw.kernel_thread);
    }

    #[test]
    fn the_kernel_thread_flag_is_read() {
        let text =
            "2 (kthreadd) S 0 0 0 0 -1 2129984 0 0 0 0 0 0 0 0 20 0 1 0 1 0 0 18446744073709551615";
        assert!(parse_stat(text).unwrap().kernel_thread);
    }

    #[test]
    fn cgroups_name_their_unit() {
        let path = parse_cgroup(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox-99.scope\n",
        )
        .unwrap();
        assert_eq!(unit(&path).as_deref(), Some("app-firefox-99.scope"));
        assert_eq!(
            unit("/system.slice/sshd.service/sub").as_deref(),
            Some("sshd.service")
        );
        assert_eq!(unit("/"), None);
    }

    #[test]
    fn long_command_lines_are_cut_on_a_char_boundary() {
        assert_eq!(cmdline(b"ls\0-la\0/tmp\0", 100), "ls -la /tmp");
        let cut = cmdline("ééééé".as_bytes(), 3);
        assert_eq!(cut, "é…");
    }

    fn raw(pid: u32, ppid: u32, ticks: u64, start: u64) -> Raw {
        Raw {
            pid,
            ppid,
            comm: format!("p{pid}"),
            state: 'S',
            cpu_ticks: ticks,
            start_ticks: start,
            rss_pages: pid as u64,
            read_bytes: Some(1000),
            ..Raw::default()
        }
    }

    #[test]
    fn cpu_is_a_share_of_the_whole_machine() {
        let users = Users::parse("root:x:0:0::/:/bin/sh\n");
        let before = [raw(1, 0, 100, 5), raw(2, 1, 0, 6)];
        // One second, four cores: 400 ticks of capacity. Process 1 used 100.
        let mut after = vec![raw(1, 0, 200, 5), raw(2, 1, 400, 7)];
        after[0].read_bytes = Some(3000);
        let rows = table(&before, &after, 1.0, 4, 1_000, UNITS, &users);
        assert!((rows[0].cpu - 25.0).abs() < 1e-9);
        assert_eq!(rows[0].read_bps, Some(2000.0));
        assert_eq!(rows[0].user, "root");
        // Process 2 started again (a reused pid): no share yet.
        assert_eq!(rows[1].cpu, 0.0);
        assert_eq!(rows[1].read_bps, None);
        assert_eq!(rows[0].started_at, 1_000);
    }

    fn rows() -> Vec<Process> {
        let users = Users::default();
        let after = [
            raw(1, 0, 10, 1),
            raw(10, 1, 50, 1),
            raw(11, 10, 90, 1),
            raw(20, 1, 70, 1),
        ];
        let before: Vec<Raw> = after
            .iter()
            .map(|r| Raw {
                cpu_ticks: 0,
                ..r.clone()
            })
            .collect();
        table(&before, &after, 1.0, 1, 0, UNITS, &users)
    }

    #[test]
    fn the_table_sorts_filters_and_limits() {
        let users = Users::default();
        let (got, total) = select(
            rows(),
            &Query {
                limit: Some(2),
                ..Query::default()
            },
            &users,
        )
        .unwrap();
        assert_eq!(total, 4);
        assert_eq!(got.iter().map(|p| p.pid).collect::<Vec<_>>(), [11, 20]);
        let q = Query {
            sort: Some("pid".into()),
            filter: Some("P1".into()),
            ..Query::default()
        };
        let (got, _) = select(rows(), &q, &users).unwrap();
        assert_eq!(got.iter().map(|p| p.pid).collect::<Vec<_>>(), [1, 10, 11]);
        assert!(
            select(
                rows(),
                &Query {
                    sort: Some("bogus".into()),
                    ..Query::default()
                },
                &users
            )
            .is_err()
        );
    }

    #[test]
    fn the_tree_puts_parents_first_and_keeps_a_match_path() {
        let users = Users::default();
        let (got, _) = select(
            rows(),
            &Query {
                tree: true,
                ..Query::default()
            },
            &users,
        )
        .unwrap();
        let order: Vec<(u32, u32)> = got.iter().map(|p| (p.pid, p.depth.unwrap())).collect();
        // Siblings 10 and 20 by CPU: 20 (70) before 10 (50).
        assert_eq!(order, [(1, 0), (20, 1), (10, 1), (11, 2)]);
        let q = Query {
            tree: true,
            filter: Some("p11".into()),
            ..Query::default()
        };
        let (got, _) = select(rows(), &q, &users).unwrap();
        assert_eq!(got.iter().map(|p| p.pid).collect::<Vec<_>>(), [1, 10, 11]);
    }

    #[test]
    fn users_resolve_by_name_or_number() {
        let users = Users::parse("root:x:0:0::/:/bin/sh\nalice:x:1000:1000::/home/user:/bin/mix\n");
        assert_eq!(users.uid("alice"), Some(1000));
        assert_eq!(users.uid("1000"), Some(1000));
        assert_eq!(users.name(4242), "4242");
    }
}
