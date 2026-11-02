use antiburn_local::analysis::{
    ContentKind, ContentPart, FenceScope, JevInputField, JevInputSelection, MemoryTurnRowStore,
    NormalizedEvent, Role, TurnRowStore, TurnSessionKey, query_turn_content_offset_selected,
    turn_row_from_event,
};

#[test]
fn search_selection_keeps_literal_namespaces_without_promoting_websearch() {
    let store = MemoryTurnRowStore::new("opencode", "session");
    let search_names: Vec<_> = ["", "mcp__", "mcp.", "mcp/", "mcp:"]
        .into_iter()
        .flat_map(|prefix| {
            [
                "grep",
                "glob",
                "find",
                "search",
                "code_search",
                "file_search",
                "search_files",
                "grep_search",
                "codebase_search",
                "list_files",
            ]
            .into_iter()
            .map(move |name| format!("{prefix}{name}"))
        })
        .collect();
    let other_names = ["websearch", "webfetch", "research"];
    let mut row = turn_row_from_event(&NormalizedEvent::new(Role::Assistant), "session", 0);
    row.content = search_names
        .iter()
        .map(String::as_str)
        .chain(other_names)
        .map(|name| {
            let mut part = ContentPart::new(ContentKind::ToolResult, format!("{name} output"));
            part.tool_name = Some(name.to_owned());
            part
        })
        .collect();
    store.write_turn_rows(&[row]).unwrap();
    store.with_connection(|connection| {
        for (field, expected) in [
            (JevInputField::SearchFilesOutput, search_names.clone()),
            (
                JevInputField::OtherToolOutput,
                other_names.into_iter().map(str::to_owned).collect(),
            ),
        ] {
            let selected = query_turn_content_offset_selected(
                connection,
                &TurnSessionKey {
                    environment_key: "native",
                    agent: "opencode",
                    session_id: "session",
                },
                &FenceScope::single(1),
                None,
                &Default::default(),
                0,
                JevInputSelection::from_fields(&[field]),
            )
            .unwrap();
            let actual: Vec<_> = selected
                .parts
                .iter()
                .map(|part| part.part.tool_name.clone().unwrap())
                .collect();
            assert_eq!(actual, expected);
            assert!(selected.parts.iter().all(|part| !part.part.text.is_empty()));
        }
    });
}
