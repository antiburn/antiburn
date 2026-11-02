use std::{
    fs::{self, File, Metadata},
    io::{Read, Seek},
    path::{Component, Path},
};

use super::admission::PathAdmission;

pub(super) const MAX_HEADER_BYTES: usize = 32 * 1024;
pub(super) const MAX_FILE_BYTES: usize = 128 * 1024;

pub(super) struct ReadBudget<'a> {
    pub admission: PathAdmission<'a>,
    pub files: usize,
    pub bytes: usize,
}

impl<'a> ReadBudget<'a> {
    pub fn new(admission: PathAdmission<'a>) -> Self {
        Self {
            admission,
            files: 256,
            bytes: 1024 * 1024,
        }
    }
}

impl ReadBudget<'_> {
    pub fn read(
        &mut self,
        path: &Path,
        root: &Path,
        header_only: bool,
    ) -> Result<Option<Vec<u8>>, &'static str> {
        if self.files == 0 || self.bytes == 0 {
            return Err("read_budget_limit");
        }
        self.files -= 1;
        self.admission.check(path)?;
        self.admission.check(root)?;
        if !path.is_absolute()
            || !path.starts_with(root)
            || path.components().any(|c| matches!(c, Component::ParentDir))
        {
            return Err("path_outside_allowed_root");
        }
        let mut current = root.to_owned();
        for component in path
            .strip_prefix(root)
            .map_err(|_| "path_outside_allowed_root")?
            .components()
        {
            current.push(component);
            self.admission.check(&current)?;
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err("symlink_not_read");
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err("path_unavailable"),
            }
        }
        let before = fs::symlink_metadata(path).map_err(|_| "file_unavailable")?;
        if !before.is_file() {
            return Err("non_regular_file");
        }
        if !header_only && before.len() > MAX_FILE_BYTES as u64 {
            return Err("file_byte_limit");
        }
        let mut file = File::open(path).map_err(|_| "file_unavailable")?;
        let opened = file.metadata().map_err(|_| "file_unavailable")?;
        if !same_file(&before, &opened) {
            return Err("file_changed_during_read");
        }
        let limit = self.bytes.min(if header_only {
            MAX_HEADER_BYTES
        } else {
            MAX_FILE_BYTES
        });
        if header_only && limit < 5 {
            return Err("read_budget_limit");
        }
        let mut bytes = Vec::new();
        let result = if header_only {
            read_header(&mut file, &mut bytes, limit)
        } else {
            file.by_ref()
                .take((limit + 1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| ())
                .map_err(|_| "file_read_failed")
        };
        let consumed = file.stream_position().map_err(|_| "file_read_failed")?;
        self.bytes = self.bytes.saturating_sub(consumed as usize);
        result?;
        if bytes.len() > limit {
            return Err("file_byte_limit");
        }
        let after = fs::symlink_metadata(path).map_err(|_| "file_changed_during_read")?;
        let closed = file.metadata().map_err(|_| "file_changed_during_read")?;
        if !same_file(&before, &after) || !same_file(&before, &closed) {
            return Err("file_changed_during_read");
        }
        Ok(Some(bytes))
    }
}

// Stop at the closing marker. Do not buffer the following skill body.
fn read_header(
    reader: &mut impl Read,
    bytes: &mut Vec<u8>,
    limit: usize,
) -> Result<(), &'static str> {
    for expected in b"---" {
        let mut byte = [0];
        if reader.read(&mut byte).map_err(|_| "file_read_failed")? == 0 || byte[0] != *expected {
            bytes.clear();
            return Ok(());
        }
        bytes.push(byte[0]);
    }
    let mut byte = [0];
    if reader.read(&mut byte).map_err(|_| "file_read_failed")? == 0 {
        return Err("skill_frontmatter_unclosed");
    }
    bytes.push(byte[0]);
    if byte[0] == b'\r' {
        if reader.read(&mut byte).map_err(|_| "file_read_failed")? == 0 {
            return Err("skill_frontmatter_unclosed");
        }
        bytes.push(byte[0]);
    }
    if byte[0] != b'\n' {
        bytes.clear();
        return Ok(());
    }
    let mut line_start = bytes.len();
    loop {
        let mut byte = [0];
        let count = reader.read(&mut byte).map_err(|_| "file_read_failed")?;
        if count == 0 {
            let last = bytes[line_start..]
                .strip_suffix(b"\r")
                .unwrap_or(&bytes[line_start..]);
            return if line_start > 0 && (last == b"---" || last == b"...") {
                Ok(())
            } else {
                Err("skill_frontmatter_unclosed")
            };
        }
        bytes.push(byte[0]);
        if bytes.len() > limit {
            return Err("skill_frontmatter_byte_limit");
        }
        if byte[0] != b'\n' {
            continue;
        }
        let line = bytes[line_start..]
            .strip_suffix(b"\n")
            .unwrap_or(&bytes[line_start..]);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line == b"---" || line == b"..." {
            return Ok(());
        }
        line_start = bytes.len();
    }
}

fn same_file(before: &Metadata, after: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return false;
        }
    }
    before.is_file()
        && after.is_file()
        && !after.file_type().is_symlink()
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && before.created().ok() == after.created().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashSet, io::Cursor};

    #[test]
    fn header_reader_stops_before_body_bytes() {
        let header = b"---\r\nname: review\r\n---\r\n";
        let mut input = Cursor::new([header.as_slice(), &[0xff; 100]].concat());
        let mut bytes = Vec::new();
        read_header(&mut input, &mut bytes, MAX_HEADER_BYTES).unwrap();
        assert_eq!(bytes, header);
        assert_eq!(input.position(), header.len() as u64);
    }

    #[test]
    fn changed_file_metadata_does_not_match() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rules.md");
        fs::write(&path, "old").unwrap();
        let before = fs::metadata(&path).unwrap();
        fs::write(&path, "new text").unwrap();
        assert!(!same_file(&before, &fs::metadata(path).unwrap()));
    }

    #[test]
    fn missing_files_consume_the_work_budget() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let grants = HashSet::new();
        let mut budget = ReadBudget {
            admission: PathAdmission::new(&root, &grants),
            files: 2,
            bytes: 100,
        };
        assert_eq!(
            budget.read(&root.join("missing-a"), &root, false).unwrap(),
            None
        );
        assert_eq!(
            budget.read(&root.join("missing-b"), &root, false).unwrap(),
            None
        );
        assert_eq!(
            budget.read(&root.join("missing-c"), &root, false),
            Err("read_budget_limit")
        );
    }

    #[test]
    fn malformed_headers_do_not_become_body_free_definitions() {
        for input in [b"---".as_slice(), b"---\nname: bad\n", b"---\nname: bad"] {
            assert_eq!(
                read_header(&mut Cursor::new(input), &mut Vec::new(), MAX_HEADER_BYTES),
                Err("skill_frontmatter_unclosed")
            );
        }
    }

    #[test]
    fn header_reader_only_checks_the_prefix_when_frontmatter_is_absent() {
        let mut input = Cursor::new(b"--not a header\nprivate body");
        let mut bytes = Vec::new();
        read_header(&mut input, &mut bytes, MAX_HEADER_BYTES).unwrap();
        assert!(bytes.is_empty());
        assert_eq!(input.position(), 3);
    }
}
