CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB);
INSERT INTO cursorDiskKV VALUES (
    'composerData:synthetic-main',
    '{"composerId":"synthetic-main","workspaceId":"synthetic-workspace","fullConversationHeadersOnly":[{"bubbleId":"z-user"},{"bubbleId":"a-assistant"}],"unknown":true}'
);
INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-main:z-user', X'7b22627562626c654964223a227a2d75736572222c2274797065223a312c2274657874223a2273796e7468657469632070726f6d7074227d');
INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-main:a-assistant', '{"bubbleId":"a-assistant","type":2,"text":"synthetic reply"}');
INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-main:unreferenced', 'do not capture');
INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-other:z-user', 'do not capture');
INSERT INTO cursorDiskKV VALUES ('composerData:synthetic-other', '{"composerId":"synthetic-other","fullConversationHeadersOnly":[]}');
INSERT INTO cursorDiskKV VALUES ('composerData:synthetic-main:near-match', 'do not capture');
INSERT INTO cursorDiskKV VALUES ('unrelated.global.state', 'do not capture');
