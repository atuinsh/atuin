/// Parses durations like "90s", "5m" or "2h" into seconds.
pub fn parse_duration(s: &str) -> Option<u64> {
    let (n, unit) = s.split_at(s.len() - 1);
    let n: u64 = n.parse().ok()?;
    match unit {
        "s" => Some(n),
        "m" => Some(n * 60),
        "h" => Some(n * 60 * 60),
        _ => None,
    }
}
