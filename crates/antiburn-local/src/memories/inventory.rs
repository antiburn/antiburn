//! Inventory of `~/.claude/projects/<slug>/memory/` directories.
//!
//! The parser is lenient. Claude Code and people write these files by hand, so
//! frontmatter and index lines vary. A line or file that does not fit is
//! skipped, never an error.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The index file name.
const INDEX_FILE: &str = "MEMORY.md";
/// Marker in the name of a backup that antiburn writes next to a file.
const BACKUP_MARKER: &str = ".antiburn-bak";
/// Bytes read from one memory file.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Memory files listed for one project.
const MAX_FILES_PER_PROJECT: usize = 500;
/// Body bytes read for one project.
const MAX_PROJECT_BYTES: u64 = 2 * 1024 * 1024;
/// Characters kept when the hook comes from the first body line.
const MAX_BODY_HOOK_CHARS: usize = 200;

/// One parsed line of `MEMORY.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub title: String,
    /// The link target, relative to the memory directory.
    pub target: String,
    pub hook: Option<String>,
    /// One-based line number in `MEMORY.md`.
    pub line_number: usize,
}

/// Where a memory's hook line comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSource {
    Index,
    Frontmatter,
    /// The first body line. A memory with no hook at all also reports `Body`.
    Body,
}

/// The four memory types that Claude Code writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    User,
    Feedback,
    Project,
    Reference,
}

impl MemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "user" => Some(Self::User),
            "feedback" => Some(Self::Feedback),
            "project" => Some(Self::Project),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

/// One memory file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub file_name: String,
    pub title: String,
    pub kind: Option<MemoryKind>,
    pub hook: Option<String>,
    pub hook_source: HookSource,
    pub body: String,
    pub has_frontmatter: bool,
    /// The body is cut or empty because of a read cap.
    pub truncated: bool,
    pub size_bytes: u64,
    pub modified_ms: Option<i64>,
    /// An index entry names this file.
    pub in_index: bool,
}

/// One project directory that has memories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryProjectInventory {
    pub slug: String,
    pub memory_dir: PathBuf,
    pub index_path: Option<PathBuf>,
    pub entries: Vec<IndexEntry>,
    /// Index entries whose target file does not exist.
    pub dangling: Vec<IndexEntry>,
    pub memories: Vec<MemoryFile>,
}

/// Scans every `home/.claude/projects/*/memory/` directory. The result is
/// sorted by slug.
pub fn scan_claude_memory_projects(home: &Path) -> Vec<MemoryProjectInventory> {
    let Ok(read_dir) = fs::read_dir(home.join(".claude").join("projects")) else {
        return Vec::new();
    };
    let mut projects: Vec<MemoryProjectInventory> = read_dir
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let slug = entry.file_name().to_str()?.to_owned();
            scan_project(&slug, &entry.path().join("memory"))
        })
        .collect();
    projects.sort_by(|a, b| a.slug.cmp(&b.slug));
    projects
}

fn scan_project(slug: &str, memory_dir: &Path) -> Option<MemoryProjectInventory> {
    let names = memory_file_names(memory_dir)?;
    let index_file = memory_dir.join(INDEX_FILE);
    let index_text = read_capped(&index_file).map(|(text, _)| text);
    let entries = index_text.as_deref().map(parse_index).unwrap_or_default();
    if names.is_empty() && entries.is_empty() {
        return None;
    }

    let mut budget = MAX_PROJECT_BYTES;
    let memories = names
        .iter()
        .take(MAX_FILES_PER_PROJECT)
        .map(|name| read_memory(memory_dir, name, &entries, &mut budget))
        .collect();
    let dangling = entries
        .iter()
        .filter(|entry| !memory_dir.join(entry_target(entry)).is_file())
        .cloned()
        .collect();
    Some(MemoryProjectInventory {
        slug: slug.to_owned(),
        memory_dir: memory_dir.to_path_buf(),
        index_path: index_text.is_some().then_some(index_file),
        entries,
        dangling,
        memories,
    })
}

/// Sorted names of the memory files, or `None` when the directory is missing.
fn memory_file_names(memory_dir: &Path) -> Option<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(memory_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".md") && name != INDEX_FILE && !name.contains(BACKUP_MARKER))
        .collect();
    names.sort();
    Some(names)
}

/// The link target without a leading `./`.
fn entry_target(entry: &IndexEntry) -> &str {
    entry.target.strip_prefix("./").unwrap_or(&entry.target)
}

/// Reads at most [`MAX_FILE_BYTES`]. The flag is true when the file is larger.
fn read_capped(path: &Path) -> Option<(String, bool)> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    let cut = bytes.len() as u64 > MAX_FILE_BYTES;
    if cut {
        bytes.truncate(MAX_FILE_BYTES as usize);
    }
    Some((String::from_utf8_lossy(&bytes).into_owned(), cut))
}

fn read_memory(
    memory_dir: &Path,
    name: &str,
    entries: &[IndexEntry],
    budget: &mut u64,
) -> MemoryFile {
    let path = memory_dir.join(name);
    let metadata = fs::metadata(&path).ok();
    let size_bytes = metadata.as_ref().map_or(0, fs::Metadata::len);
    let modified_ms = metadata
        .and_then(|data| data.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok());

    let (text, mut truncated) = if *budget == 0 {
        (String::new(), true)
    } else {
        let (text, cut) = read_capped(&path).unwrap_or_default();
        *budget = budget.saturating_sub(text.len() as u64);
        (text, cut)
    };
    let parsed = parse_frontmatter(&text);
    if *budget == 0 && text.is_empty() {
        truncated = true;
    }

    let entry = entries.iter().find(|entry| entry_target(entry) == name);
    let stem = name.strip_suffix(".md").unwrap_or(name);
    let title = entry
        .map(|entry| entry.title.clone())
        .filter(|title| !title.is_empty())
        .or(parsed.name.clone())
        .unwrap_or_else(|| stem.to_owned());
    let (hook, hook_source) = if let Some(hook) = entry.and_then(|entry| entry.hook.clone()) {
        (Some(hook), HookSource::Index)
    } else if let Some(description) = parsed.description.clone() {
        (Some(description), HookSource::Frontmatter)
    } else {
        let line = parsed
            .body
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(MAX_BODY_HOOK_CHARS).collect());
        (line, HookSource::Body)
    };
    MemoryFile {
        path,
        file_name: name.to_owned(),
        title,
        kind: parsed.kind.as_deref().and_then(MemoryKind::parse),
        hook,
        hook_source,
        body: parsed.body,
        has_frontmatter: parsed.has_frontmatter,
        truncated,
        size_bytes,
        modified_ms,
        in_index: entry.is_some(),
    }
}

#[derive(Debug, Default)]
struct ParsedFrontmatter {
    name: Option<String>,
    description: Option<String>,
    kind: Option<String>,
    body: String,
    has_frontmatter: bool,
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_owned();
        }
    }
    value.to_owned()
}

fn parse_frontmatter(text: &str) -> ParsedFrontmatter {
    let mut lines = text.split('\n');
    let first = lines.next().unwrap_or("");
    let opens = first.trim_end_matches('\r') == "---";
    let plain = || ParsedFrontmatter {
        body: text.to_owned(),
        ..ParsedFrontmatter::default()
    };
    if !opens {
        return plain();
    }

    let mut top: [Option<String>; 3] = [None, None, None];
    let mut nested: [Option<String>; 3] = [None, None, None];
    let mut in_metadata = false;
    let mut closed = false;
    let mut consumed = first.len() + 1;
    for line in lines.by_ref() {
        consumed += line.len() + 1;
        let line = line.trim_end_matches('\r');
        if line == "---" {
            closed = true;
            break;
        }
        let indented = line.starts_with([' ', '\t']);
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let slot = match key.trim() {
            "name" => Some(0),
            "description" => Some(1),
            "type" => Some(2),
            _ => None,
        };
        if indented {
            if let (true, Some(slot)) = (in_metadata, slot) {
                nested[slot].get_or_insert_with(|| unquote(value));
            }
        } else {
            in_metadata = key.trim() == "metadata" && value.trim().is_empty();
            if let Some(slot) = slot {
                top[slot].get_or_insert_with(|| unquote(value));
            }
        }
    }
    if !closed {
        return plain();
    }

    let rest = text.get(consumed.min(text.len())..).unwrap_or("");
    let [name, description, kind] = std::array::from_fn(|index| {
        top[index]
            .take()
            .or_else(|| nested[index].take())
            .filter(|value| !value.is_empty())
    });
    ParsedFrontmatter {
        name,
        description,
        kind,
        body: rest.trim_start_matches(['\n', '\r']).to_owned(),
        has_frontmatter: true,
    }
}

/// Parses lines shaped like `- [title](target.md) — hook`. Other lines are
/// ignored.
fn parse_index(text: &str) -> Vec<IndexEntry> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| parse_index_line(line, index + 1))
        .collect()
}

fn parse_index_line(line: &str, line_number: usize) -> Option<IndexEntry> {
    let rest = line.trim_start().strip_prefix(['-', '*'])?.trim_start();
    let rest = rest.strip_prefix('[')?;
    let (title, rest) = rest.split_once("](")?;
    let (target, rest) = rest.split_once(')')?;
    if title.contains(']') || target.is_empty() {
        return None;
    }
    let rest = rest.trim();
    let hook = if rest.is_empty() {
        None
    } else {
        let hook = rest.strip_prefix(['—', '–', '-'])?;
        let hook = hook.trim_start_matches(['—', '–', '-']).trim();
        (!hook.is_empty()).then(|| hook.to_owned())
    };
    Some(IndexEntry {
        title: title.trim().to_owned(),
        target: target.trim().to_owned(),
        hook,
        line_number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let memory = home.path().join(".claude/projects/-work-app/memory");
        fs::create_dir_all(&memory).unwrap();
        (home, memory)
    }

    fn scan_one(home: &Path) -> MemoryProjectInventory {
        let mut projects = scan_claude_memory_projects(home);
        assert_eq!(projects.len(), 1);
        projects.remove(0)
    }

    #[test]
    fn parses_every_frontmatter_variant() {
        let (home, memory) = fixture();
        write(
            &memory,
            "nested.md",
            "---\nname: nested\ndescription: \"Quoted: hook\"\nmetadata:\n  type: feedback\n  originSessionId: abc\n---\n\n\nBody text\n",
        );
        write(
            &memory,
            "flat.md",
            "---\r\nname: 'flat'\r\ntype: project\r\n---\r\nFirst line\r\n",
        );
        write(&memory, "none.md", "\n# Heading\nNo frontmatter here\n");
        write(&memory, "unclosed.md", "---\nname: x\nstill body\n");
        write(
            &memory,
            "unknown.md",
            "---\ntype: weird\nextra: 1\n---\nbody\n",
        );
        let project = scan_one(home.path());
        let by_name = |name: &str| {
            project
                .memories
                .iter()
                .find(|memory| memory.file_name == name)
                .unwrap()
        };

        let nested = by_name("nested.md");
        assert!(nested.has_frontmatter);
        assert_eq!(nested.title, "nested");
        assert_eq!(nested.kind, Some(MemoryKind::Feedback));
        assert_eq!(nested.hook.as_deref(), Some("Quoted: hook"));
        assert_eq!(nested.hook_source, HookSource::Frontmatter);
        assert_eq!(nested.body, "Body text\n");

        let flat = by_name("flat.md");
        assert_eq!(flat.title, "flat");
        assert_eq!(flat.kind, Some(MemoryKind::Project));
        assert_eq!(flat.hook.as_deref(), Some("First line"));
        assert_eq!(flat.hook_source, HookSource::Body);

        let none = by_name("none.md");
        assert!(!none.has_frontmatter);
        assert_eq!(none.title, "none");
        assert_eq!(none.hook.as_deref(), Some("# Heading"));
        assert!(none.body.contains("No frontmatter here"));

        let unclosed = by_name("unclosed.md");
        assert!(!unclosed.has_frontmatter);
        assert!(unclosed.body.starts_with("---"));

        assert_eq!(by_name("unknown.md").kind, None);
    }

    #[test]
    fn index_supplies_title_hook_and_flags_dangling_and_orphans() {
        let (home, memory) = fixture();
        write(
            &memory,
            "a.md",
            "---\nname: file-name\ndescription: fm hook\n---\nbody\n",
        );
        write(&memory, "orphan.md", "orphan body\n");
        write(
            &memory,
            "MEMORY.md",
            "# Memory\n\nNotes that are not entries.\n- [Title A](a.md) — index hook\n* [Gone](gone.md) - missing file\n- [No hook](./orphan2.md)\n  - [Indented](a.md)–x\n- not a link\n",
        );
        let project = scan_one(home.path());

        assert_eq!(project.index_path, Some(memory.join("MEMORY.md")));
        assert_eq!(project.entries.len(), 4);
        assert_eq!(project.entries[0].line_number, 4);
        assert_eq!(project.entries[0].hook.as_deref(), Some("index hook"));
        assert_eq!(project.entries[2].hook, None);
        let dangling: Vec<_> = project.dangling.iter().map(|e| e.target.as_str()).collect();
        assert_eq!(dangling, ["gone.md", "./orphan2.md"]);

        let a = &project.memories[0];
        assert_eq!(a.title, "Title A");
        assert_eq!(a.hook.as_deref(), Some("index hook"));
        assert_eq!(a.hook_source, HookSource::Index);
        assert!(a.in_index);
        let orphan = &project.memories[1];
        assert!(!orphan.in_index);
    }

    #[test]
    fn skips_index_backups_non_markdown_subdirectories_and_empty_projects() {
        let (home, memory) = fixture();
        write(&memory, "MEMORY.md", "just prose\n");
        write(&memory, "MEMORY.md.antiburn-bak", "- [x](x.md)\n");
        write(&memory, "x.antiburn-bak.md", "backup");
        write(&memory, "notes.txt", "text");
        fs::create_dir(memory.join("sub.md")).unwrap();
        let empty = home.path().join(".claude/projects/-other/memory");
        fs::create_dir_all(&empty).unwrap();
        fs::create_dir_all(home.path().join(".claude/projects/-none")).unwrap();
        assert!(scan_claude_memory_projects(home.path()).is_empty());

        write(&memory, "real.md", "body");
        assert_eq!(scan_one(home.path()).memories.len(), 1);
    }

    #[test]
    fn index_entries_alone_keep_a_project() {
        let (home, memory) = fixture();
        write(&memory, "MEMORY.md", "- [Only](only.md) — hook\n");
        let project = scan_one(home.path());
        assert!(project.memories.is_empty());
        assert_eq!(project.dangling.len(), 1);
    }

    #[test]
    fn caps_file_and_project_bytes() {
        let (home, memory) = fixture();
        write(&memory, "a-big.md", &"x".repeat(70 * 1024));
        let project = scan_one(home.path());
        let big = &project.memories[0];
        assert!(big.truncated);
        assert_eq!(big.body.len(), 64 * 1024);
        assert_eq!(big.size_bytes, 70 * 1024);

        for index in 0..40 {
            write(&memory, &format!("b{index:02}.md"), &"y".repeat(60 * 1024));
        }
        let project = scan_one(home.path());
        let empty: Vec<_> = project
            .memories
            .iter()
            .filter(|memory| memory.body.is_empty())
            .collect();
        assert!(!empty.is_empty());
        assert!(empty.iter().all(|memory| memory.truncated));
        assert_eq!(project.memories.len(), 41);
    }
}
