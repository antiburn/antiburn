use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use antiburn_local::instructions::CurrentSkill;

use super::{Collector, Producer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkillTraversal {
    Direct,
    Grouped,
    OpenCodeV2,
}

impl SkillTraversal {
    pub fn for_opencode(producer: Producer) -> Self {
        if producer == Producer::OpenCodeV2 {
            Self::OpenCodeV2
        } else {
            Self::Direct
        }
    }
}

impl Collector<'_> {
    pub(super) fn entries(
        &mut self,
        directory: &Path,
        safety: &Path,
        global: bool,
        skills: bool,
    ) -> Vec<PathBuf> {
        if self.directories_left == 0 || self.entries_left == 0 {
            self.limit(global, skills, "directory_work_limit");
            return Vec::new();
        }
        self.directories_left -= 1;
        if let Err(reason) = self
            .reads
            .admission
            .check(directory)
            .and_then(|_| self.reads.admission.check(safety))
        {
            self.limit(global, skills, reason);
            return Vec::new();
        }
        let Some(relative) = directory.strip_prefix(safety).ok() else {
            self.limit(global, skills, "path_outside_allowed_root");
            return Vec::new();
        };
        let mut current = safety.to_owned();
        for component in relative.components() {
            if matches!(component, Component::ParentDir) {
                self.limit(global, skills, "path_outside_allowed_root");
                return Vec::new();
            }
            current.push(component);
            if let Err(reason) = self.reads.admission.check(&current) {
                self.limit(global, skills, reason);
                return Vec::new();
            }
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    self.limit(global, skills, "unsafe_directory_not_read");
                    return Vec::new();
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
                Err(_) => {
                    self.limit(global, skills, "directory_unavailable");
                    return Vec::new();
                }
            }
        }
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => {
                self.limit(global, skills, "directory_unavailable");
                return Vec::new();
            }
        };
        let cap = self.entries_left.min(256);
        let mut paths = Vec::new();
        for entry in entries.take(cap + 1) {
            if paths.len() == cap {
                self.limit(global, skills, "directory_entry_limit");
                break;
            }
            self.entries_left = self.entries_left.saturating_sub(1);
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(_) => self.limit(global, skills, "directory_entry_unavailable"),
            }
        }
        paths.sort();
        paths
    }

    pub(super) fn skill_source(
        &mut self,
        directory: &Path,
        safety: &Path,
        global: bool,
        traversal: SkillTraversal,
        depth: usize,
    ) {
        if depth > 6 {
            self.limit(global, true, "skill_depth_limit");
            return;
        }
        if self.skills_left == 0 {
            self.limit(global, true, "skill_file_limit");
            return;
        }
        let paths = self.entries(directory, safety, global, true);
        if traversal != SkillTraversal::OpenCodeV2
            && let Some(path) = paths
                .iter()
                .find(|path| path.file_name().is_some_and(|name| name == "SKILL.md"))
        {
            if let Some(fallback) = directory.file_name().and_then(|name| name.to_str()) {
                self.skill(path, safety, global, fallback);
            }
            return;
        }
        for path in paths {
            if let Err(reason) = self.reads.admission.check(&path) {
                self.limit(global, true, reason);
                continue;
            }
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    self.limit(global, true, "skill_entry_unavailable");
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                self.limit(global, true, "symlink_not_read");
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                self.limit(global, true, "path_not_utf8");
                continue;
            };
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            if metadata.is_file() {
                if name == "SKILL.md"
                    || (depth == 0
                        && traversal != SkillTraversal::Direct
                        && path.extension().is_some_and(|extension| extension == "md"))
                {
                    let fallback = if name == "SKILL.md" {
                        directory.file_name()
                    } else {
                        path.file_stem()
                    };
                    if let Some(fallback) = fallback.and_then(|name| name.to_str()) {
                        self.skill(&path, safety, global, fallback);
                    }
                }
            } else if metadata.is_dir() {
                if depth == 6 {
                    self.limit(global, true, "skill_depth_limit");
                    continue;
                }
                if traversal == SkillTraversal::OpenCodeV2 {
                    self.skill_source(&path, safety, global, traversal, depth + 1);
                    continue;
                }
                let definition = path.join("SKILL.md");
                if let Err(reason) = self.reads.admission.check(&definition) {
                    self.limit(global, true, reason);
                    continue;
                }
                match fs::symlink_metadata(&definition) {
                    Ok(_) => self.skill(&definition, safety, global, name),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        if traversal == SkillTraversal::Grouped {
                            self.skill_source(&path, safety, global, traversal, depth + 1);
                        }
                    }
                    Err(_) => self.limit(global, true, "skill_entry_unavailable"),
                }
            }
        }
    }

    fn skill(&mut self, path: &Path, safety: &Path, global: bool, fallback: &str) {
        if self.skills_left == 0 {
            self.limit(global, true, "skill_file_limit");
            return;
        }
        if self
            .root(global)
            .skills
            .as_ref()
            .is_some_and(|entries| entries.iter().any(|entry| Path::new(&entry.path) == path))
        {
            return;
        }
        let bytes = match self.reads.read(path, safety, true) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                self.limit(global, true, "skill_disappeared");
                return;
            }
            Err(reason) => {
                self.limit(global, true, reason);
                return;
            }
        };
        let (name, description) = match fields(&bytes, fallback) {
            Ok(fields) => fields,
            Err(reason) => {
                self.limit(global, true, reason);
                return;
            }
        };
        let absolute: PathBuf = path.components().collect();
        let Some(path) = absolute.to_str() else {
            self.limit(global, true, "path_not_utf8");
            return;
        };
        if let Some(entries) = &mut self.root(global).skills {
            entries.push(CurrentSkill {
                name,
                description,
                path: path.to_owned(),
            });
            self.skills_left -= 1;
        }
    }
}

fn fields(bytes: &[u8], fallback: &str) -> Result<(String, String), &'static str> {
    if bytes.is_empty() {
        return Ok((valid_name(fallback)?.to_owned(), String::new()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "skill_frontmatter_not_utf8")?;
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Err("skill_frontmatter_invalid");
    }
    let header = lines
        .take_while(|line| *line != "---" && *line != "...")
        .collect::<Vec<_>>()
        .join("\n");
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&header).map_err(|_| "skill_frontmatter_invalid")?;
    if value.is_null() {
        return Ok((valid_name(fallback)?.to_owned(), String::new()));
    }
    let map = value.as_mapping().ok_or("skill_frontmatter_invalid")?;
    let name = match map.get("name") {
        Some(value) => value.as_str().ok_or("skill_name_unsupported")?,
        None => fallback,
    };
    let description = match map.get("description") {
        Some(serde_yaml_ng::Value::Null) | None => "",
        Some(value) => value.as_str().ok_or("skill_description_unsupported")?,
    };
    if description.len() > 16 * 1024 {
        return Err("skill_description_byte_limit");
    }
    Ok((valid_name(name)?.to_owned(), description.to_owned()))
}

fn valid_name(name: &str) -> Result<&str, &'static str> {
    if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        Err("skill_name_unsupported")
    } else {
        Ok(name)
    }
}
