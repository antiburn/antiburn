use std::fs::File;
use std::io::Read;

use super::ProcessSample;

const MAX_PROC_FILE_BYTES: u64 = 4096;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

pub(super) fn sample() -> ProcessSample {
    let mut result = ProcessSample::default();

    if let Some(stat) = read_bounded("/proc/self/stat") {
        let parsed = parse_stat(&stat);
        result.cpu_time_ns = parsed.cpu_ticks.and_then(cpu_ticks_to_ns);
        result.memory_bytes = parsed.resident_pages.and_then(resident_pages_to_bytes);
    }

    if let Some(io) = read_bounded("/proc/self/io") {
        let parsed = parse_io(&io);
        result.read_bytes = parsed.read_bytes;
        result.write_bytes = parsed.write_bytes;
    }

    result
}

fn read_bounded(path: &str) -> Option<String> {
    let file = File::open(path).ok()?;
    read_bounded_reader(file)
}

fn read_bounded_reader(reader: impl Read) -> Option<String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_PROC_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_PROC_FILE_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedStat {
    cpu_ticks: Option<u64>,
    resident_pages: Option<u64>,
}

fn parse_stat(input: &str) -> ParsedStat {
    let Some(comm_end) = input.rfind(')') else {
        return ParsedStat::default();
    };
    let Some(remainder) = input.get(comm_end + 1..) else {
        return ParsedStat::default();
    };
    let mut fields = remainder.split_ascii_whitespace();
    let user_ticks = fields.nth(11).and_then(|value| value.parse::<u64>().ok());
    let system_ticks = fields.next().and_then(|value| value.parse::<u64>().ok());
    let cpu_ticks = user_ticks
        .zip(system_ticks)
        .and_then(|(user, system)| user.checked_add(system));
    let resident_pages = fields
        .nth(8)
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|value| u64::try_from(value).ok());

    ParsedStat {
        cpu_ticks,
        resident_pages,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedIo {
    read_bytes: Option<u64>,
    write_bytes: Option<u64>,
}

fn parse_io(input: &str) -> ParsedIo {
    let mut result = ParsedIo::default();
    for line in input.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let parsed = value.trim().parse::<u64>().ok();
        match key {
            "read_bytes" => result.read_bytes = parsed,
            "write_bytes" => result.write_bytes = parsed,
            _ => {}
        }
    }
    result
}

fn cpu_ticks_to_ns(ticks: u64) -> Option<u64> {
    let ticks_per_second = sysconf_positive(libc::_SC_CLK_TCK)?;
    let seconds = ticks / ticks_per_second;
    let remaining_ticks = ticks % ticks_per_second;
    seconds.checked_mul(NANOS_PER_SECOND)?.checked_add(
        remaining_ticks
            .checked_mul(NANOS_PER_SECOND)?
            .checked_div(ticks_per_second)?,
    )
}

fn resident_pages_to_bytes(pages: u64) -> Option<u64> {
    pages.checked_mul(sysconf_positive(libc::_SC_PAGESIZE)?)
}

fn sysconf_positive(name: libc::c_int) -> Option<u64> {
    // SAFETY: sysconf reads process-independent configuration for a valid constant.
    let value = unsafe { libc::sysconf(name) };
    u64::try_from(value).ok().filter(|value| *value > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_with_spaces_and_parentheses_in_comm() {
        let input =
            "42 (name with ) marks) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23";

        assert_eq!(
            parse_stat(input),
            ParsedStat {
                cpu_ticks: Some(23),
                resident_pages: Some(21),
            }
        );
    }

    #[test]
    fn keeps_independent_stat_values_when_one_is_invalid() {
        let input = "42 (shell) S 1 2 3 4 5 6 7 8 9 10 bad 12 13 14 15 16 17 18 19 20 21";

        assert_eq!(
            parse_stat(input),
            ParsedStat {
                cpu_ticks: None,
                resident_pages: Some(21),
            }
        );
    }

    #[test]
    fn rejects_truncated_and_negative_stat_values() {
        assert_eq!(parse_stat("42 (shell) S 1 2"), ParsedStat::default());

        let input = "42 (shell) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 -1";
        assert_eq!(
            parse_stat(input),
            ParsedStat {
                cpu_ticks: Some(23),
                resident_pages: None,
            }
        );
    }

    #[test]
    fn rejects_overflowing_cpu_total() {
        let input = format!(
            "42 (shell) S 1 2 3 4 5 6 7 8 9 10 {} 1 13 14 15 16 17 18 19 20 21",
            u64::MAX
        );

        assert_eq!(parse_stat(&input).cpu_ticks, None);
        assert_eq!(parse_stat(&input).resident_pages, Some(21));
    }

    #[test]
    fn parses_only_storage_io_counters() {
        let input =
            "rchar: 90\nwchar: 80\nread_bytes: 70\nwrite_bytes: 60\ncancelled_write_bytes: 50\n";

        assert_eq!(
            parse_io(input),
            ParsedIo {
                read_bytes: Some(70),
                write_bytes: Some(60),
            }
        );
    }

    #[test]
    fn keeps_valid_io_counter_when_the_other_is_malformed() {
        assert_eq!(
            parse_io("read_bytes: invalid\nwrite_bytes: 60\n"),
            ParsedIo {
                read_bytes: None,
                write_bytes: Some(60),
            }
        );
    }

    #[test]
    fn rejects_proc_content_over_the_read_limit() {
        let content = vec![b'x'; MAX_PROC_FILE_BYTES as usize + 1];
        assert_eq!(read_bounded_reader(std::io::Cursor::new(content)), None);
    }

    #[test]
    fn native_sample_does_not_require_every_counter() {
        let sample = sample();
        assert!(
            sample.cpu_time_ns.is_some()
                || sample.memory_bytes.is_some()
                || sample.read_bytes.is_some()
                || sample.write_bytes.is_some()
        );
    }
}
