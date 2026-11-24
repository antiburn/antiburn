CREATE TABLE composerHeaders (composerId TEXT PRIMARY KEY, workspaceId TEXT, value TEXT);
INSERT INTO composerHeaders VALUES ('synthetic-chunked', 'synthetic-workspace', '{"synthetic":"proof only"}');
INSERT INTO composerHeaders VALUES ('synthetic-foreign', 'foreign-workspace', '{"synthetic":"do not capture"}');
INSERT INTO cursorDiskKV VALUES (
    'composerData:synthetic-chunked',
    '{"_v":18,"composerId":"synthetic-chunked","fullConversationHeadersOnly":[],"conversationMap":{},"headerLog":{"generation":"synthetic-generation","chunkCount":2,"chunkSize":256,"count":2}}'
);
INSERT INTO cursorDiskKV VALUES ('composerConvHeaders:synthetic-chunked:000001', '[{"bubbleId":"a-assistant","type":2}]');
INSERT INTO cursorDiskKV VALUES ('composerConvHeaders:synthetic-chunked:000000', '[ {"bubbleId":"z-user","type":1} ]');
INSERT INTO cursorDiskKV VALUES ('composerConvHeaders:synthetic-chunked:000002', 'unreferenced');
INSERT INTO cursorDiskKV VALUES ('composerConvHeaders:synthetic-foreign:000000', '[{"bubbleId":"z-user","type":1}]');
INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-foreign:z-user', '{"bubbleId":"z-user","type":1,"text":"foreign prompt"}');
INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-chunked:z-user', '{"bubbleId":"z-user","type":1,"text":"synthetic prompt"}');
INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-chunked:a-assistant', '{"bubbleId":"a-assistant","type":2,"text":"synthetic reply"}');
