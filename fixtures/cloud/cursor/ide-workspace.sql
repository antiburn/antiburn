CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB);
INSERT INTO ItemTable VALUES (
    'composer.composerData',
    '{"allComposers":[{"composerId":"synthetic-other"},{"composerId":"synthetic-main"}]}'
);
INSERT INTO ItemTable VALUES ('unrelated.workspace.state', 'do not capture');
