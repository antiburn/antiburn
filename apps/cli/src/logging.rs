use std::{fs::OpenOptions, io};

pub fn init() -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let directory = executable.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "executable has no parent directory",
        )
    })?;
    let log_path = directory.join("calls.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_writer(file)
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .map_err(io::Error::other)
}
