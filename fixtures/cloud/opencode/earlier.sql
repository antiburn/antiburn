-- Synthetic minimum V2 source. This is not the retained V1 table layout.
CREATE TABLE session_v2 (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    fork_session_id TEXT,
    version TEXT,
    directory TEXT
);
CREATE TABLE session_message (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    type TEXT NOT NULL,
    seq INTEGER NOT NULL,
    data TEXT NOT NULL
);
CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message(session_id, seq);
INSERT INTO session_v2 VALUES ('ses_main', NULL, NULL, '2.0.32', '/synthetic/project');
INSERT INTO session_message VALUES ('msg_user', 'ses_main', 'user', 1, '{"text":"earlier native input"}');
