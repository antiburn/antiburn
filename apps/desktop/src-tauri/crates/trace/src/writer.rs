use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use time::OffsetDateTime;
use tracing_subscriber::fmt::MakeWriter;

use crate::LOG_FILE_PREFIX;

#[derive(Clone)]
pub(crate) struct HourlyFileWriter {
    state: Arc<Mutex<State>>,
}

impl HourlyFileWriter {
    pub(crate) fn new(dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            state: Arc::new(Mutex::new(State::new(dir, HourKey::now())?)),
        })
    }

    pub(crate) fn flush(&self) -> io::Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .file
            .flush()
    }

    pub(crate) fn stop(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.active = false;
        state.file.flush()
    }
}

impl<'a> MakeWriter<'a> for HourlyFileWriter {
    type Writer = HourlyWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        HourlyWriter {
            state: self.state.lock().unwrap_or_else(|error| error.into_inner()),
            checked_hour: false,
        }
    }
}

pub(crate) struct HourlyWriter<'a> {
    state: MutexGuard<'a, State>,
    checked_hour: bool,
}

impl Write for HourlyWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if !self.state.active {
            return Ok(buffer.len());
        }
        if !self.checked_hour {
            self.state.rotate_if_needed(HourKey::now())?;
            self.checked_hour = true;
        }
        self.state.file.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.state.file.flush()
    }
}

struct State {
    active: bool,
    dir: PathBuf,
    hour: HourKey,
    file: File,
}

impl State {
    fn new(dir: &Path, hour: HourKey) -> io::Result<Self> {
        Ok(Self {
            active: true,
            dir: dir.to_path_buf(),
            hour,
            file: open_log_file(dir, hour)?,
        })
    }

    fn rotate_if_needed(&mut self, hour: HourKey) -> io::Result<()> {
        if hour == self.hour {
            return Ok(());
        }
        let file = open_log_file(&self.dir, hour)?;
        self.file = file;
        self.hour = hour;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HourKey {
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
}

impl HourKey {
    fn now() -> Self {
        Self::from(OffsetDateTime::now_utc())
    }

    fn filename(self) -> String {
        format!(
            "{LOG_FILE_PREFIX}.{:04}-{:02}-{:02}-{:02}.log",
            self.year, self.month, self.day, self.hour
        )
    }
}

impl From<OffsetDateTime> for HourKey {
    fn from(value: OffsetDateTime) -> Self {
        Self {
            year: value.year(),
            month: u8::from(value.month()),
            day: value.day(),
            hour: value.hour(),
        }
    }
}

fn open_log_file(dir: &Path, hour: HourKey) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(hour.filename()))
}

#[cfg(test)]
mod tests {
    use super::{HourKey, State};
    use std::io::Write;

    #[test]
    fn state_rotates_between_hourly_files() {
        let temp = tempfile::tempdir().unwrap();
        let first = HourKey {
            year: 2026,
            month: 8,
            day: 20,
            hour: 4,
        };
        let second = HourKey { hour: 5, ..first };
        let mut state = State::new(temp.path(), first).unwrap();
        state.file.write_all(b"first").unwrap();

        state.rotate_if_needed(second).unwrap();
        state.file.write_all(b"second").unwrap();

        assert_eq!(
            std::fs::read_to_string(temp.path().join(first.filename())).unwrap(),
            "first"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join(second.filename())).unwrap(),
            "second"
        );
    }
}
