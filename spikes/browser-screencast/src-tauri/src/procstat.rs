//! CPU and memory of the Chromium process tree, from `ps`.
//!
//! `ps -o %cpu` on macOS is a decaying average and useless for short windows, so CPU is the
//! delta of cumulative CPU time (`ps -o time`, 10 ms resolution) over wall-clock time.
//! RSS is summed across the tree; shared pages are counted once per process, so the sum
//! over-states real memory (the report says so).

use std::collections::HashSet;
use std::process::Command;
use std::time::Instant;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: u32,
    pub cpu_secs: f64,
    pub rss_kb: u64,
    pub comm: String,
}

/// `[[dd-]hh:]mm:ss[.ff]` -> seconds.
pub fn parse_cpu_time(s: &str) -> Option<f64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, s),
    };
    let mut total = 0.0;
    for part in rest.split(':') {
        total = total * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + total)
}

pub fn parse_ps(output: &str) -> Vec<ProcRow> {
    output
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let cpu_secs = parse_cpu_time(it.next()?)?;
            let rss_kb = it.next()?.parse().ok()?;
            let comm = it.collect::<Vec<_>>().join(" ");
            Some(ProcRow { pid, ppid, cpu_secs, rss_kb, comm })
        })
        .collect()
}

pub fn snapshot() -> Vec<ProcRow> {
    let out = Command::new("ps").args(["-axo", "pid=,ppid=,time=,rss=,comm="]).output();
    out.ok().map(|o| parse_ps(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

/// `root` and all its descendants.
pub fn tree(rows: &[ProcRow], root: u32) -> Vec<&ProcRow> {
    let mut pids: HashSet<u32> = HashSet::from([root]);
    loop {
        let before = pids.len();
        for r in rows {
            if pids.contains(&r.ppid) {
                pids.insert(r.pid);
            }
        }
        if pids.len() == before {
            break;
        }
    }
    rows.iter().filter(|r| pids.contains(&r.pid)).collect()
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub at: Instant,
    pub cpu_secs: f64,
    pub rss_kb: u64,
    pub processes: usize,
}

pub fn sample_tree(root: u32) -> Sample {
    let rows = snapshot();
    let t = tree(&rows, root);
    Sample {
        at: Instant::now(),
        cpu_secs: t.iter().map(|r| r.cpu_secs).sum(),
        rss_kb: t.iter().map(|r| r.rss_kb).sum(),
        processes: t.len(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub seconds: f64,
    /// Percent of one core (100 = one full core).
    pub cpu_percent: f64,
    pub rss_mb_median: f64,
    pub rss_mb_max: f64,
    pub processes: usize,
    pub samples: usize,
}

pub fn summarize(samples: &[Sample]) -> Option<Summary> {
    let (first, last) = (samples.first()?, samples.last()?);
    let wall = last.at.duration_since(first.at).as_secs_f64();
    let mut rss: Vec<f64> = samples.iter().map(|s| s.rss_kb as f64 / 1024.0).collect();
    rss.sort_by(f64::total_cmp);
    Some(Summary {
        seconds: wall,
        cpu_percent: if wall > 0.0 { (last.cpu_secs - first.cpu_secs) / wall * 100.0 } else { 0.0 },
        rss_mb_median: rss[rss.len() / 2],
        rss_mb_max: *rss.last()?,
        processes: samples.iter().map(|s| s.processes).max().unwrap_or(0),
        samples: samples.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_time_formats() {
        assert_eq!(parse_cpu_time("0:01.50"), Some(1.5));
        assert_eq!(parse_cpu_time("12:30.25"), Some(750.25));
        assert_eq!(parse_cpu_time("1:02:03.00"), Some(3723.0));
        assert_eq!(parse_cpu_time("2-00:00:10"), Some(172_810.0));
        assert_eq!(parse_cpu_time("junk"), None);
    }

    #[test]
    fn parses_rows_with_spaces_in_the_command_and_builds_the_tree() {
        let out = "  1     0   0:10.00  1000 /sbin/launchd\n\
                   100     1   0:05.00 50000 /Apps/Helium\n\
                   101   100   0:02.00 80000 /Apps/Helium Helper (Renderer)\n\
                   102   101   0:01.00 10000 /Apps/Helium Helper (GPU)\n\
                   200     1   9:00.00  9999 /usr/bin/other\n";
        let rows = parse_ps(out);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[2].comm, "/Apps/Helium Helper (Renderer)");
        let t = tree(&rows, 100);
        let pids: Vec<u32> = t.iter().map(|r| r.pid).collect();
        assert_eq!(pids, [100, 101, 102]);
    }

    #[test]
    fn summary_uses_cpu_time_delta_over_wall_time() {
        let t0 = Instant::now();
        let mk = |ms: u64, cpu: f64, rss: u64| Sample {
            at: t0 + std::time::Duration::from_millis(ms),
            cpu_secs: cpu,
            rss_kb: rss,
            processes: 3,
        };
        let s = summarize(&[mk(0, 10.0, 100 * 1024), mk(1000, 10.5, 120 * 1024), mk(2000, 11.0, 110 * 1024)]).unwrap();
        assert!((s.cpu_percent - 50.0).abs() < 1e-6, "{}", s.cpu_percent);
        assert_eq!(s.rss_mb_median, 110.0);
        assert_eq!(s.rss_mb_max, 120.0);
    }
}
