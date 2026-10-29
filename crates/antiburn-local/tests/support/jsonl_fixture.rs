pub fn with_line_ending(source: &str, line_ending: &str) -> String {
    source
        .lines()
        .map(|line| format!("{line}{line_ending}"))
        .collect()
}
