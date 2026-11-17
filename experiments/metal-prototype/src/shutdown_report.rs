//! Strict normalization of the game's bounded numeric shutdown trace. Never
//! copy paths, raw errors, NBT, names or arbitrary text into the owner report.
use std::{io::Read, path::Path};
pub(crate) fn read(path: &Path, boot: u64) -> String {
    let mut out = String::from(
        "\nSHUTDOWN/SAVE P0 HOLD — NO WORLD DURABILITY GUARANTEE\nStorage ids are anonymous instances, not names or renderer world epochs. Zero means unobserved.\nFive-second probe is relative to join observation, NOT a measured host deadline.\n",
    );
    for name in ["shutdown-save-previous.txt", "shutdown-save-numeric.txt"] {
        let Ok(file) = std::fs::File::open(path.join(name)) else {
            out.push_str("Shutdown numeric snapshot unavailable.\n");
            continue;
        };
        let mut text = String::new();
        if file.take(32 * 1024 + 1).read_to_string(&mut text).is_err() || text.len() > 32 * 1024 {
            out.push_str("Shutdown numeric snapshot unreadable/over budget.\n");
            continue;
        }
        match normalize(&text) {
            Some(normalized) => {
                let observed_boot = text
                    .lines()
                    .next()
                    .and_then(numbers)
                    .and_then(|h| h.get(6).copied());
                if !name.contains("previous") && observed_boot != Some(boot) {
                    out.push_str("Shutdown trace boot does not match this boot; current attribution refused.\n");
                    continue;
                }
                let text = normalized;
                out.push_str(if name.contains("previous") {
                    "PREVIOUS BOOT SHUTDOWN:\n"
                } else {
                    "CURRENT BOOT SHUTDOWN:\n"
                });
                out.push_str(&text);
            }
            None => out.push_str("Shutdown numeric snapshot INVALID; raw data omitted.\n"),
        }
    }
    out
}
fn normalize(text: &str) -> Option<String> {
    let mut lines = text.lines();
    let header = numbers(lines.next()?)?;
    if header.len() != 7 || header[0] != 1 || header[5] > 1 {
        return None;
    }
    let rows = lines.map(numbers).collect::<Option<Vec<_>>>()?;
    if rows.len() > 160
        || rows.iter().any(|r| {
            r.len() != 7
                || !(1..=7).contains(&r[2])
                || !(1..=15).contains(&r[3])
                || r[6] > 1
                || r[0] > header[4]
        })
    {
        return None;
    }
    let actors = [
        "",
        "Region writer",
        "Player writer",
        "Integrated server",
        "Streaming cleanup",
        "LOD cleanup",
        "Main lifecycle",
        "Native cancellation",
    ];
    let stages = [
        "",
        "Requested",
        "Queued (NOT DURABLE)",
        "Started",
        "Synced component (NOT whole world)",
        "FILE SAVE/WORKER FAILURE",
        "Drain requested",
        "JOIN BEGIN",
        "JOIN END",
        "Suspended",
        "Exit callback",
        "DEADLINE EXPIRED — STILL OWNED",
        "Session lock released",
        "WAIT BEGIN",
        "WAIT END",
        "Superseded handoff (NOT destination commit)",
    ];
    let mut out = format!(
        "trace_sequence={} lost_trace_events={} save_or_worker_errors_total={} elapsed_ms={} main_identity_known={} trace_boot={}\n",
        header[1], header[2], header[3], header[4], header[5], header[6]
    );
    for r in &rows {
        out.push_str(&format!(
            "+{}ms storage={} {} {} pending_records={} outstanding_batches={} on_main={}\n",
            r[0],
            r[1],
            actors[r[2] as usize],
            stages[r[3] as usize],
            count(r[4]),
            count(r[5]),
            r[6]
        ));
        if matches!(r[3], 7 | 13)
            && !rows
                .iter()
                .any(|v| v[1] == r[1] && v[2] == r[2] && v[3] == r[3] + 1 && v[0] >= r[0])
        {
            let age = header[4].saturating_sub(r[0]);
            out.push_str(&format!("Join observed pending for {}ms; five_second_probe_near={} (OS deadline unobserved)\n", age, age >= 4500));
        }
    }
    Some(out)
}
fn numbers(line: &str) -> Option<Vec<u64>> {
    line.split_whitespace().map(|n| n.parse().ok()).collect()
}
fn count(n: u64) -> String {
    if n == u64::MAX {
        "UNKNOWN".into()
    } else {
        n.to_string()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_waiting_and_failure_without_claiming_durability() {
        let text = normalize("1 7 3 1 5100 1 99\n20 2 1 5 1 0 0\n30 2 1 7 1 1 1\n").unwrap();
        assert!(text.contains("FILE SAVE/WORKER FAILURE"));
        assert!(text.contains("five_second_probe_near=true"));
        assert!(text.contains("on_main=1"));
        assert!(normalize("1 7 0 0 0 1 99\nPRIVATE world/path TOKEN=secret").is_none());
        assert!(normalize("1 7 0 0 9 1 99\n1 2 99 7 0 0 1").is_none());
    }
    #[test]
    fn completed_component_is_not_whole_world_save_or_watchdog_prevention() {
        let text =
            normalize("1 5 0 0 5100 1 99\n1 2 1 7 1 1 1\n200 2 1 4 0 0 0\n201 2 1 8 0 0 1\n")
                .unwrap();
        assert!(text.contains("Synced component (NOT whole world)"));
        assert!(!text.contains("five_second_probe_near=true"));
    }
    #[test]
    fn superseded_handoff_does_not_claim_destination_sync() {
        let text = normalize("1 1 0 0 10 1 99\n1 2 2 15 1 0 0\n").unwrap();
        assert!(text.contains("Superseded handoff (NOT destination commit)"));
        assert!(!text.contains("Synced component"));
    }
}
