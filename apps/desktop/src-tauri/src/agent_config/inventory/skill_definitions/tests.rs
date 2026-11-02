use super::*;

fn write(path: &Path, value: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

#[test]
fn semantic_frontmatter_preserves_multiline_descriptions_metadata_and_comments() {
    let parsed = parse_frontmatter(
        br#"---
# Private comment is not a semantic field.
name: code-review # inline comment
description: >-
  Review code for defects,
  including resource ownership.
metadata:
  version: 2
  supported: [rust, typescript]
allowed-tools: "Read, Bash"
---
Private skill body.
"#,
        "directory-alias",
    )
    .unwrap();
    assert_eq!(parsed.name, "code-review");
    assert_eq!(
        parsed.declared_description,
        "Review code for defects, including resource ownership."
    );
    assert_eq!(parsed.frontmatter["metadata"]["version"], 2);
    assert_eq!(parsed.frontmatter["metadata"]["supported"][1], "typescript");
    assert!(!parsed.frontmatter.to_string().contains("Private"));
    let literal = parse_frontmatter(
        b"---\r\ndescription: |\r\n  First line.\r\n  Second line.\r\n---\r\nPrivate body.\r\n",
        "review",
    )
    .unwrap();
    assert_eq!(literal.declared_description, "First line.\nSecond line.");
    let quoted = parse_frontmatter(
        b"---\ndescription: 'Read # values: preserve them.' # comment\n---\n",
        "review",
    )
    .unwrap();
    assert_eq!(quoted.declared_description, "Read # values: preserve them.");
}

#[test]
fn malformed_duplicate_and_oversized_shapes_are_unavailable() {
    for text in [
        "---\ndescription: []\n---\n",
        "---\ndescription: 42\n---\n",
        "---\ndescription: true\n---\n",
        "---\ndescription: one\ndescription: two\n---\n",
        "---\ndescription: unfinished\n",
        "---\nname: []\n---\n",
        "---\nname: ''\n---\n",
        "---\n- item\n---\n",
    ] {
        assert!(
            parse_frontmatter(text.as_bytes(), "review").is_none(),
            "{text}"
        );
    }
    for text in [
        format!(
            "---\ndescription: {}\n---\n",
            "x".repeat(MAX_SKILL_DESCRIPTION_BYTES + 1)
        ),
        format!(
            "---\nmetadata: {}\n---\n",
            "x".repeat(MAX_SKILL_FRONTMATTER_BYTES + 1)
        ),
        format!("---\nname: {}\n---\n", "x".repeat(257)),
    ] {
        assert!(parse_frontmatter(text.as_bytes(), "review").is_none());
    }
}

#[test]
fn names_reject_empty_control_and_oversized_values_with_or_without_frontmatter() {
    for name in [
        String::new(),
        " \t ".to_owned(),
        "re\tview".to_owned(),
        "re\nview".to_owned(),
        "x".repeat(257),
    ] {
        for text in [
            "# Review\nPRIVATE_BODY_MARKER\n",
            "---\n---\nPRIVATE_BODY_MARKER\n",
        ] {
            assert!(
                parse_frontmatter(text.as_bytes(), &name).is_none(),
                "{name:?}"
            );
        }
        let text = format!(
            "---\nname: {}\n---\nPRIVATE_BODY_MARKER\n",
            serde_json::to_string(&name).unwrap()
        );
        assert!(
            parse_frontmatter(text.as_bytes(), "review").is_none(),
            "{name:?}"
        );
    }
}

#[test]
fn names_trim_and_accept_the_byte_limit_with_or_without_frontmatter() {
    for name in [" review ".to_owned(), format!(" {} ", "x".repeat(256))] {
        let declared = format!(
            "---\nname: {}\n---\nPRIVATE_BODY_MARKER\n",
            serde_json::to_string(&name).unwrap()
        );
        for text in [
            "# Review\nPRIVATE_BODY_MARKER\n",
            "---\n---\nPRIVATE_BODY_MARKER\n",
            &declared,
        ] {
            let parsed = parse_frontmatter(text.as_bytes(), &name).unwrap();
            assert_eq!(parsed.name, name.trim());
            assert!(parsed.declared_description.is_empty());
            assert!(
                !parsed
                    .frontmatter
                    .to_string()
                    .contains("PRIVATE_BODY_MARKER")
            );
        }
    }
}

#[test]
fn full_description_is_not_cut_to_the_name_limit() {
    let description = "Review the resource ownership and error paths. ".repeat(40);
    let text = format!("---\r\nname: review\r\ndescription: \"{description}\"\r\n---\r\n");
    let parsed = parse_frontmatter(text.as_bytes(), "review").unwrap();
    assert_eq!(parsed.declared_description, description.trim());
    assert!(parsed.declared_description.len() > 256);
}

#[test]
fn missing_null_blank_descriptions_and_plain_markdown_have_no_body_fallback() {
    for text in [
        "---\n---\nPRIVATE_BODY_MARKER\n",
        "---\nname: review\n---\nPRIVATE_BODY_MARKER\n",
        "---\ndescription: null\n---\nPRIVATE_BODY_MARKER\n",
        "---\ndescription: '  '\n---\nPRIVATE_BODY_MARKER\n",
        "# Review\n\nPRIVATE_BODY_MARKER\n",
        "---\r\nname: review\r\n---\r\nPRIVATE_BODY_MARKER\r\n",
    ] {
        let parsed = parse_frontmatter(text.as_bytes(), "review").unwrap();
        assert_eq!(parsed.name, "review");
        assert!(parsed.declared_description.is_empty(), "{text}");
        assert!(
            !parsed
                .frontmatter
                .to_string()
                .contains("PRIVATE_BODY_MARKER")
        );
    }
    let large = "# Review\n\nPrivate body.\n".repeat(1000);
    let parsed = parse_frontmatter(large.as_bytes(), "review").unwrap();
    assert!(parsed.declared_description.is_empty());
    assert_eq!(parsed.frontmatter, serde_json::json!({}));
}

#[test]
fn invalid_utf8_has_no_declared_fields() {
    assert!(parse_frontmatter(b"---\ndescription: Review code.\n---\n\xff", "review").is_none());
}

#[test]
fn checked_definition_reader_and_parser_keep_the_file_size_bound() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("review/SKILL.md");
    let mut bytes = vec![b'x'; MAX_SKILL_FILE_BYTES];
    write(&path, &bytes);
    assert_eq!(read_definition(&path, &root).unwrap().bytes, bytes);
    assert!(parse_frontmatter(&bytes, "review").is_some());
    bytes.push(b'x');
    write(&path, &bytes);
    assert!(matches!(
        read_definition(&path, &root),
        Err(ConfigUnavailableReason::FileTooLarge)
    ));
    assert!(parse_frontmatter(&bytes, "review").is_none());
}

#[test]
fn checked_definition_reader_rejects_non_files_and_paths_outside_the_root() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let directory = root.join("review");
    std::fs::create_dir(&directory).unwrap();
    assert!(matches!(
        read_definition(&directory, &root),
        Err(ConfigUnavailableReason::NonRegularFile)
    ));
    let path = root.join("outside/SKILL.md");
    write(&path, b"---\ndescription: Review code.\n---\n");
    assert!(matches!(
        read_definition(&path, &directory),
        Err(ConfigUnavailableReason::UnsafePath)
    ));
}

#[cfg(unix)]
#[test]
fn checked_definition_reader_keeps_ownership_metadata() {
    use crate::agent_config::filesystem::file_ownership;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("review/SKILL.md");
    write(&path, b"---\ndescription: Review code.\n---\n");
    let ownership = file_ownership(&std::fs::metadata(&path).unwrap());
    assert_eq!(read_definition(&path, &root).unwrap().ownership, ownership);
    assert_eq!(
        file_ownership(&std::fs::metadata(&path).unwrap()),
        ownership
    );
}

#[cfg(unix)]
#[test]
fn metadata_identity_rejects_replacement_and_symlinked_definitions() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("review/SKILL.md");
    write(&path, b"---\ndescription: Review code.\n---\n");
    let before = std::fs::metadata(&path).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    write(&path, b"---\ndescription: Review code.\n---\n");
    assert!(!same_file(&before, &std::fs::metadata(&path).unwrap()));
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(path.with_extension("old"), &path).unwrap();
    assert!(matches!(
        read_definition(&path, &root),
        Err(ConfigUnavailableReason::SymlinkTarget)
    ));
    let alias = root.join("alias");
    std::os::unix::fs::symlink(path.parent().unwrap(), &alias).unwrap();
    assert!(matches!(
        read_definition(&alias.join("SKILL.old"), &root),
        Err(ConfigUnavailableReason::SymlinkTarget)
    ));
}
