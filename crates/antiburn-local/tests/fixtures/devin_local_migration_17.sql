PRAGMA user_version = 17;
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    working_directory TEXT,
    model TEXT,
    main_chain_id INTEGER,
    hidden INTEGER,
    created_at INTEGER,
    last_activity_at INTEGER
);
CREATE TABLE message_nodes (
    session_id TEXT,
    node_id INTEGER,
    parent_node_id INTEGER,
    raw_message TEXT,
    created_at INTEGER
);
CREATE TABLE subagent_heads (
    session_id TEXT,
    tool_call_id TEXT,
    child_agent_id TEXT,
    child_chain_node_id INTEGER
);
CREATE TABLE tool_call_state (
    session_id TEXT,
    tool_call_id TEXT,
    state TEXT
);
INSERT INTO sessions VALUES
    ('root', '/work', 'claude-opus-4-6', 2, 0, 1700000000, 1700000100),
    ('child', '/work', 'claude-opus-4-7-20260115', 3, 0, 1700000000, 1700000100);
INSERT INTO message_nodes VALUES
    ('root', 1, NULL, '{"role":"user","message_id":"u1"}', 1700000000),
    ('root', 2, 1, '{"role":"assistant","message_id":"a1","metadata":{"generation_model":"claude-opus-4-6"},"tool_calls":[{"id":"call-1","name":"run_subagent"}]}', 1700000001),
    ('child', 3, NULL, '{"role":"assistant","message_id":"ca1","metadata":{"generation_model":"claude-opus-4-7-20260115"}}', 1700000002);
INSERT INTO subagent_heads VALUES ('root', 'call-1', 'child', 3);
INSERT INTO tool_call_state VALUES ('root', 'call-1', '{"status":"completed"}');
