CREATE TABLE session (
    id TEXT PRIMARY KEY,
    parent_id TEXT
);
CREATE TABLE message (
    id TEXT PRIMARY KEY,
    session_id TEXT,
    data TEXT
);
CREATE TABLE part (
    message_id TEXT,
    data TEXT
);

INSERT INTO session (id, parent_id) VALUES ('root', NULL);
INSERT INTO session (id, parent_id) VALUES ('child', 'root');
INSERT INTO message (id, session_id, data) VALUES
    ('root-message', 'root', '{"role":"assistant","time":{"created":1000},"modelID":"model-root","tokens":{"input":2,"output":3}}');
INSERT INTO message (id, session_id, data) VALUES
    ('child-message', 'child', '{"role":"user","time":{"created":2000}}');
INSERT INTO part (message_id, data) VALUES
    ('root-message', '{"type":"text","text":"root response"}');
INSERT INTO part (message_id, data) VALUES
    ('child-message', '{"type":"text","text":"child prompt"}');
