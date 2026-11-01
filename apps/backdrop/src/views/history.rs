// SPDX-License-Identifier: MIT OR Apache-2.0
//! Owner history → bounded plot series. Epoch seconds are converted once;
//! rendering never fabricates historic samples from the current queue.
use serde_json::Value;
pub struct Series {
    pub landings: Vec<[f64; 2]>,
    pub gate_minutes: Vec<[f64; 2]>,
}
pub fn series(history: &Value, hours: u16, now: f64) -> Series {
    if !history["landed"].is_array() || !history["batches"].is_array() {
        return Series {landings:vec![],gate_minutes:vec![]};
    }
    let since = now - f64::from(hours) * 3600.;
    let first = (since / 3600.).floor() as i64;
    let last = (now / 3600.).floor() as i64;
    let mut counts = vec![0.; (last - first + 1).max(0) as usize];
    if let Some(rows) = history["landed"].as_array() {
        for row in rows {
            if let Some(t) = row["updated"]
                .as_f64()
                .filter(|t| t.is_finite() && *t >= since && *t <= now)
            {
                counts[((t / 3600.).floor() as i64 - first) as usize] += 1.;
            }
        }
    }
    let landings = counts
        .into_iter()
        .enumerate()
        .map(|(i, n)| {
            [
                (((first + i as i64) as f64 * 3600.).max(since) - now) / 3600.,
                n,
            ]
        })
        .collect();
    let mut minutes = Vec::new();
    if let Some(rows) = history["batches"].as_array() {
        for row in rows {
            let Some(start) = row["started"]
                .as_f64()
                .filter(|t| t.is_finite() && *t >= since && *t <= now)
            else {
                continue;
            };
            let Some(end) = row["ended"]
                .as_f64()
                .filter(|t| t.is_finite() && *t >= start && *t <= now)
            else {
                continue;
            };
            let reused = row["gate_reused"] == 1 || row["gate_reused"] == true;
            minutes.push([
                (start - now) / 3600.,
                if reused { 0. } else { (end - start) / 60. },
            ]);
        }
    }
    minutes.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let gate_minutes = if minutes.len() <= 600 {
        minutes
    } else {
        (0..600)
            .map(|i| minutes[i * (minutes.len() - 1) / 599])
            .collect()
    };
    Series {
        landings,
        gate_minutes,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn owner_epochs_are_binned_and_batch_seconds_become_minutes() {
        let data = json!({"landed":[{"updated":10_900},{"updated":11_000},{"updated":14_400},{"updated":1}],
            "batches":[{"started":10_900,"ended":11_020,"gate_reused":0},{"started":12_000,"ended":12_600,"gate_reused":1},{"started":13_000,"ended":null}]});
        let s = series(&data, 1, 14_400.);
        assert_eq!(s.landings, vec![[-1., 2.], [0., 1.]]);
        assert_eq!(s.gate_minutes.len(), 2);
        assert_eq!(s.gate_minutes[0][1], 2.);
        assert_eq!(s.gate_minutes[1][1], 0.);
    }
    #[test]
    fn seven_day_series_are_bounded_and_keep_first_and_last_batch() {
        let batches: Vec<Value> = (0..1000)
            .map(|i| json!({"started":i,"ended":i+1,"gate_reused":0}))
            .collect();
        let s = series(&json!({"batches":batches,"landed":[]}), 168, 1000.);
        assert_eq!(s.gate_minutes.len(), 600);
        assert_eq!(s.landings.len(), 169);
        assert_eq!(s.gate_minutes.first().unwrap()[0], -1000. / 3600.);
        assert_eq!(s.gate_minutes.last().unwrap()[0], -1. / 3600.);
    }
    #[test]
    fn unavailable_history_has_no_fabricated_zero_series() {
        assert!(series(&Value::Null,24,1000.).landings.is_empty());
        assert!(series(&json!({}),24,1000.).gate_minutes.is_empty());
        assert!(!series(&json!({"batches":[],"landed":[]}),24,1000.).landings.is_empty());
    }
}
