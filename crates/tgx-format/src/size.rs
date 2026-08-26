//! Desktop's file-size and duration wording.
//!
//! **Sizes truncate, they never round.** 1,940,744 bytes reads `1.8 MB`, where
//! rounding gives `1.9`. Checked against 1,950 sizes in a reference export,
//! zero mismatches — so this is a measured property of Desktop's writer, not a
//! plausible-looking choice.

/// Desktop's file-size wording: 1024 steps, one decimal, truncated.
pub fn human_size(bytes: i64) -> String {
    if bytes <= 0 {
        return String::new();
    }
    const STEPS: [(&str, i64); 3] = [
        ("GB", 1024 * 1024 * 1024),
        ("MB", 1024 * 1024),
        ("KB", 1024),
    ];
    for (unit, step) in STEPS {
        if bytes >= step {
            // floor(size / step * 10) / 10, done in integer arithmetic so the
            // truncation cannot drift with floating point. `size * 10 / step`
            // truncates by construction.
            let tenths = bytes.saturating_mul(10) / step;
            return format!("{}.{} {}", tenths / 10, tenths % 10, unit);
        }
    }
    format!("{bytes} B")
}

/// `h:mm:ss`, or `mm:ss` under an hour.
pub fn human_duration(seconds: i64) -> String {
    if seconds <= 0 {
        return String::new();
    }
    let (m, s) = (seconds / 60, seconds % 60);
    let (h, m) = (m / 60, m % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_truncate_they_do_not_round() {
        // The measured case from the reference export.
        assert_eq!(human_size(1_940_744), "1.8 MB");
        // Rounding would give 1.9 — prove the two really differ here.
        let rounded = (1_940_744f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0;
        assert_eq!(format!("{rounded:.1}"), "1.9");
    }

    #[test]
    fn each_unit_boundary() {
        assert_eq!(human_size(0), "");
        assert_eq!(human_size(-5), "");
        assert_eq!(human_size(1), "1 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn just_under_a_boundary_truncates_down() {
        // 1048575 B is 0.99999… MB; truncation must give 1023.9 KB, not 1.0 MB.
        assert_eq!(human_size(1024 * 1024 - 1), "1023.9 KB");
    }

    #[test]
    fn a_huge_size_does_not_overflow() {
        // saturating_mul, or a multi-terabyte file panics in debug.
        let s = human_size(i64::MAX);
        assert!(s.ends_with(" GB"), "got {s}");
    }

    #[test]
    fn durations_drop_the_hour_when_there_is_none() {
        assert_eq!(human_duration(0), "");
        assert_eq!(human_duration(9), "00:09");
        assert_eq!(human_duration(69), "01:09");
        assert_eq!(human_duration(3600), "1:00:00");
        assert_eq!(human_duration(3661), "1:01:01");
    }
}
