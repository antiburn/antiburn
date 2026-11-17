-- Authored synthetic values and schema from OpenCode v2.3.0, a6b15d33daf6ab7d35569f6d711fdb630b9f04e7.
-- See packages/core/src/session/sql.ts, event/sql.ts, and job/sql.ts.
CREATE TABLE session_v2 (
    id TEXT PRIMARY KEY, project_id TEXT NOT NULL, workspace_id TEXT,
    parent_id TEXT, fork_session_id TEXT, fork_boundary TEXT,
    slug TEXT NOT NULL, directory TEXT NOT NULL, path TEXT, title TEXT,
    version TEXT NOT NULL, share_url TEXT,
    summary_additions INTEGER, summary_deletions INTEGER, summary_files INTEGER,
    summary_diffs TEXT, metadata TEXT, cost REAL DEFAULT 0 NOT NULL,
    tokens_input INTEGER DEFAULT 0 NOT NULL, tokens_output INTEGER DEFAULT 0 NOT NULL,
    tokens_reasoning INTEGER DEFAULT 0 NOT NULL, tokens_cache_read INTEGER DEFAULT 0 NOT NULL,
    tokens_cache_write INTEGER DEFAULT 0 NOT NULL, revert TEXT, permission TEXT,
    agent TEXT, model TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
    time_busy INTEGER, time_idle INTEGER, time_viewed INTEGER, idle_outcome TEXT,
    time_compacting INTEGER, time_archived INTEGER, time_suspended INTEGER,
    resume_attempts INTEGER DEFAULT 0 NOT NULL
);
CREATE INDEX session_v2_parent_idx ON session_v2(parent_id);
CREATE TABLE session_message (
    id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES session_v2(id) ON DELETE CASCADE,
    type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL, data TEXT NOT NULL
);
CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message(session_id, seq);
CREATE TABLE session_part (
    message_id TEXT NOT NULL REFERENCES session_message(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL REFERENCES session_v2(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL, type TEXT NOT NULL, call_id TEXT,
    time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL,
    PRIMARY KEY(message_id, ordinal)
);
CREATE INDEX session_part_session_message_idx ON session_part(session_id, message_id, ordinal);
CREATE TABLE event_sequence (aggregate_id TEXT NOT NULL PRIMARY KEY, seq INTEGER NOT NULL, owner_id TEXT);
CREATE TABLE event (
    id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL REFERENCES event_sequence(aggregate_id),
    seq INTEGER NOT NULL, created INTEGER NOT NULL DEFAULT 0, type TEXT NOT NULL, data TEXT NOT NULL
);
CREATE UNIQUE INDEX event_aggregate_seq_idx ON event(aggregate_id, seq);
CREATE TABLE session_inbox (
    id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES session_v2(id),
    type TEXT NOT NULL, payload TEXT NOT NULL, delivery TEXT NOT NULL,
    enqueued_seq INTEGER NOT NULL, time_created INTEGER NOT NULL
);
CREATE TABLE instruction_entry (
    session_id TEXT NOT NULL REFERENCES session_v2(id), key TEXT NOT NULL, value TEXT,
    removed INTEGER NOT NULL DEFAULT 0, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
    PRIMARY KEY(session_id, key)
);
CREATE TABLE instruction_state (
    session_id TEXT PRIMARY KEY REFERENCES session_v2(id),
    epoch_start INTEGER NOT NULL, through_seq INTEGER NOT NULL,
    initial_values TEXT NOT NULL, current_values TEXT NOT NULL
);
CREATE TABLE instruction_blob (hash TEXT PRIMARY KEY, value TEXT);
CREATE TABLE blob (
    hash TEXT PRIMARY KEY, mime TEXT NOT NULL, bytes INTEGER NOT NULL,
    width INTEGER, height INTEGER, data BLOB NOT NULL, time_created INTEGER NOT NULL
);
CREATE TABLE blob_ref (
    session_id TEXT NOT NULL REFERENCES session_v2(id), message_id TEXT NOT NULL,
    hash TEXT NOT NULL REFERENCES blob(hash), seq INTEGER NOT NULL,
    PRIMARY KEY(session_id, message_id, hash)
);
CREATE TABLE job (
    notification_id TEXT PRIMARY KEY, id TEXT NOT NULL, session_id TEXT NOT NULL,
    child_session_id TEXT, kind TEXT NOT NULL, title TEXT, status TEXT NOT NULL,
    output TEXT, error TEXT, recovery TEXT NOT NULL,
    time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
);
CREATE INDEX job_session_idx ON job(session_id);
-- These rows must never become capture payload.
CREATE TABLE credential (id TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE schedule (id TEXT PRIMARY KEY, session_id TEXT, action TEXT);

INSERT INTO session_v2 (id, project_id, parent_id, fork_session_id, fork_boundary, slug, directory, title, version, model, time_created, time_updated, time_archived)
VALUES ('ses_main', 'global', NULL, NULL, NULL, 'main', '/synthetic/project', 'Main', '2.3.0', ' { "id": "Exact Model ID", "providerID": "synthetic", "future": 1 } ', 1, 2, 3),
       ('ses_child', 'global', 'ses_main', NULL, NULL, 'child', '/synthetic/child', 'Child', '2.3.0', NULL, 1, 2, NULL),
       ('ses_detached', 'global', 'ses_absent', NULL, NULL, 'detached', '/synthetic/detached', 'Detached', '2.3.0', NULL, 1, 2, NULL),
       ('ses_fork', 'global', NULL, 'ses_main', '{"type":"through","messageID":"msg_user"}', 'fork', '/synthetic/fork', 'Fork', '2.0.32', NULL, 1, 2, NULL);
INSERT INTO session_message VALUES
('msg_user', 'ses_main', 'user', 1, 1, 1, '{"text":"main request","files":[{"mime":"text/plain","uri":"media:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"}]}'),
('msg_inline', 'ses_main', 'assistant', 2, 1, 1, '{"content":[{"type":"text","text":"inline answer"},{"state":{"status":"completed","content":[{"mime":"text/plain","uri":"media:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","type":"file"}]},"type":"tool"}]}'),
('msg_split', 'ses_main', 'assistant', 3, 1, 1, '{"content":[]}'),
('msg_opaque', 'ses_main', 'future-record', 4, 1, 1, 'not JSON: preserve this unknown native record'),
('msg_child', 'ses_child', 'user', 1, 1, 1, '{"text":"CHILD-ONLY-CONTENT"}'),
('msg_detached', 'ses_detached', 'user', 1, 1, 1, '{"text":"detached request"}'),
('msg_fork', 'ses_fork', 'user', 1, 1, 1, '{"text":"fork-owned retained context"}');
INSERT INTO session_part VALUES
('msg_split', 'ses_main', 0, 'text', NULL, 1, 1, '{"type":"text","text":"split answer","state":{"status":false,"content":"opaque provider state"}}'),
('msg_split', 'ses_main', 1, 'tool', 'call_split', 1, 1, '{"type":"tool","state":{"status":"completed","content":[{"type":"text","text":"split tool output"}]}}');
INSERT INTO event_sequence VALUES ('ses_main', 10, NULL), ('ses_child', 2, NULL), ('ses_fork', 3, NULL);
INSERT INTO event VALUES
('evt_instruction', 'ses_main', 5, 1, 'session.instructions.updated.2', '{"sessionID":"ses_main","delta":{"api/retired":"c6f3c2883e2f62d62929fb9ee0a98ce91fbce5fcc5d1076ec95d8d2839b416c1","api/deleted":"removed"},"text":"retained instruction update"}'),
('evt_tool', 'ses_main', 6, 1, 'session.tool.failed.2', '{"sessionID":"ses_main","content":[{"uri":"media:dd2bf3282325d2e67286772e2f8119ec7a19f73a416f1a41e4b0c4c514d7acd5","mime":"text/plain","type":"file"}],"error":{"message":"synthetic tool failure"}}'),
('evt_queue', 'ses_main', 7, 1, 'session.inbox.enqueued.1', '{"sessionID":"ses_main","item":{"payload":{"text":"retained queued source","files":[{"mime":"text/plain","uri":"media:a8dfbdec100174316bc8656e1e50cdbf96c2fa90c9dcf43f72d92b59fa96046e"}]},"type":"user"}}'),
('evt_removed', 'ses_main', 8, 1, 'session.message.removed.1', '{"messageID":"msg_removed","future":"retained event"}'),
('evt_fork', 'ses_fork', 2, 1, 'session.forked.2', '{"sessionID":"ses_fork","parentID":"ses_main","instructions":{"api/fork":"0d96adb0259ab76fd91a4abe6bb3ed8fbf9a11c29e317f599e410fcfc949360e"}}');
INSERT INTO session_inbox VALUES
('inbox_current', 'ses_main', 'user', '{"text":"queued request","files":[{"mime":"text/plain","uri":"media:a8dfbdec100174316bc8656e1e50cdbf96c2fa90c9dcf43f72d92b59fa96046e"}]}', 'queued', 10, 1);
INSERT INTO instruction_entry VALUES
('ses_main', 'api/current', '"current"', 0, 1, 1),
('ses_main', 'api/removed', '"removed entry source"', 1, 1, 1);
INSERT INTO instruction_state VALUES
('ses_main', 1, 8, '{"api/initial":"4f1cf8d86a66caf554db9ac662dfe98a531cc614c2c4309a5e04c5f74912d3fd"}', '{"api/current":"694df57a7909d9f9edbd3acb4ccfce5fbee156fab749383121f17fef16bbab57"}');
INSERT INTO instruction_blob VALUES
('4f1cf8d86a66caf554db9ac662dfe98a531cc614c2c4309a5e04c5f74912d3fd', '"initial"'),
('694df57a7909d9f9edbd3acb4ccfce5fbee156fab749383121f17fef16bbab57', '"current"'),
('c6f3c2883e2f62d62929fb9ee0a98ce91fbce5fcc5d1076ec95d8d2839b416c1', '"retired"'),
('0d96adb0259ab76fd91a4abe6bb3ed8fbf9a11c29e317f599e410fcfc949360e', '"fork baseline"'),
('34dd830a7088d2242b635c33961974c34d0b3f8398d12f4d50a8d97cde9c5996', '"unrelated"');
INSERT INTO blob VALUES
('2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824', 'text/plain', 5, NULL, NULL, X'68656c6c6f', 1),
('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad', 'text/plain', 3, NULL, NULL, X'616263', 1),
('dd2bf3282325d2e67286772e2f8119ec7a19f73a416f1a41e4b0c4c514d7acd5', 'text/plain', 10, NULL, NULL, X'6576656e742d6f6e6c79', 1),
('a8dfbdec100174316bc8656e1e50cdbf96c2fa90c9dcf43f72d92b59fa96046e', 'text/plain', 11, NULL, NULL, X'7175657565642d6f6e6c79', 1),
('34dd830a7088d2242b635c33961974c34d0b3f8398d12f4d50a8d97cde9c5996', 'text/plain', 9, NULL, NULL, X'756e72656c61746564', 1);
INSERT INTO blob_ref VALUES ('ses_main', 'msg_user', '2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824', 1);
INSERT INTO job VALUES
('notify_main', 'job_main', 'ses_main', 'ses_child', 'subagent', 'Owned child result', 'completed', 'parent-owned child summary', 'retained job error', '{"kind":"subagent","parentSessionID":"ses_main","childSessionID":"ses_child","agent":"synthetic","description":"owned work"}', 1, 2),
('notify_other', 'job_other', 'ses_child', 'ses_main', 'subagent', 'Other job', 'completed', 'UNRELATED-JOB-CONTENT', NULL, '{}', 1, 2);
INSERT INTO credential VALUES ('synthetic', 'EXCLUDED-GLOBAL-VALUE');
INSERT INTO schedule VALUES ('synthetic', 'ses_main', 'EXCLUDED-SCHEDULE-VALUE');
