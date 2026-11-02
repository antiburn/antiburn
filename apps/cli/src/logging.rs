use std::{fs::OpenOptions, io};

pub fn init(log_path: &std::path::Path) -> io::Result<()> {
    if let Some(directory) = log_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(log_path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("CLI log is not a regular file"));
    }

    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_writer(file)
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .map_err(io::Error::other)
}
