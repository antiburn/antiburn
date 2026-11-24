-- Track writes that can change a Sessions row on the single writer connection.
-- Keep only the newest affected activity time. Settings and scan bookkeeping
-- do not change it. A fresh head starts a new observation interval.
CREATE TEMP TABLE IF NOT EXISTS activity_stream_changes (
    slot INTEGER PRIMARY KEY,
    excluded TEXT NOT NULL,
    newest_epoch INTEGER NOT NULL
) STRICT;

CREATE TEMP TABLE IF NOT EXISTS activity_repository_revision (
    revision INTEGER NOT NULL
) STRICT;
INSERT INTO activity_repository_revision SELECT 0
 WHERE NOT EXISTS (SELECT 1 FROM activity_repository_revision);

CREATE TEMP TRIGGER IF NOT EXISTS activity_session_insert
    AFTER INSERT ON main.session
BEGIN
    UPDATE activity_stream_changes SET newest_epoch = coalesce(NEW.updated_at_epoch, 0)
     WHERE newest_epoch < coalesce(NEW.updated_at_epoch, 0)
       AND NEW.agent NOT IN (SELECT value FROM json_each(excluded));
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_session_update
    AFTER UPDATE OF updated_at_epoch, title, cwd, wsl_distro, surface ON main.session
    WHEN OLD.updated_at_epoch IS NOT NEW.updated_at_epoch
      OR OLD.title IS NOT NEW.title OR OLD.cwd IS NOT NEW.cwd
      OR OLD.wsl_distro IS NOT NEW.wsl_distro OR OLD.surface IS NOT NEW.surface
BEGIN
    UPDATE activity_stream_changes SET newest_epoch = max(coalesce(OLD.updated_at_epoch, 0), coalesce(NEW.updated_at_epoch, 0))
     WHERE newest_epoch < max(coalesce(OLD.updated_at_epoch, 0), coalesce(NEW.updated_at_epoch, 0))
       AND NEW.agent NOT IN (SELECT value FROM json_each(excluded));
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_session_delete
    AFTER DELETE ON main.session
BEGIN
    UPDATE activity_stream_changes SET newest_epoch = coalesce(OLD.updated_at_epoch, 0)
     WHERE newest_epoch < coalesce(OLD.updated_at_epoch, 0)
       AND OLD.agent NOT IN (SELECT value FROM json_each(excluded));
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_analysis_insert
    AFTER INSERT ON main.session_analysis
BEGIN
    UPDATE activity_stream_changes SET newest_epoch = max(newest_epoch, coalesce((
        SELECT updated_at_epoch FROM main.session
         WHERE environment_key = NEW.environment_key AND agent = NEW.agent AND session_id = NEW.session_id
    ), 0))
     WHERE NEW.agent NOT IN (SELECT value FROM json_each(excluded));
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_analysis_update
    AFTER UPDATE OF model_breakdown_json, pricing_breakdown_json, inclusive_models_json ON main.session_analysis
    WHEN OLD.model_breakdown_json IS NOT NEW.model_breakdown_json
      OR OLD.pricing_breakdown_json IS NOT NEW.pricing_breakdown_json
      OR OLD.inclusive_models_json IS NOT NEW.inclusive_models_json
BEGIN
    UPDATE activity_stream_changes SET newest_epoch = max(newest_epoch, coalesce((
        SELECT updated_at_epoch FROM main.session
         WHERE environment_key = NEW.environment_key AND agent = NEW.agent AND session_id = NEW.session_id
    ), 0))
     WHERE NEW.agent NOT IN (SELECT value FROM json_each(excluded));
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_analysis_delete
    AFTER DELETE ON main.session_analysis
BEGIN
    UPDATE activity_stream_changes SET newest_epoch = max(newest_epoch, coalesce((
        SELECT updated_at_epoch FROM main.session
         WHERE environment_key = OLD.environment_key AND agent = OLD.agent AND session_id = OLD.session_id
    ), 0))
     WHERE OLD.agent NOT IN (SELECT value FROM json_each(excluded));
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_fork_insert
    AFTER INSERT ON main.session_relation WHEN NEW.kind = 'forkParent'
BEGIN
    UPDATE activity_stream_changes SET newest_epoch = max(newest_epoch, coalesce((
        SELECT max(updated_at_epoch) FROM main.session
         WHERE environment_key = NEW.environment_key AND agent = NEW.agent
           AND session_id IN (NEW.session_id, NEW.related_id)
    ), 0))
     WHERE NEW.agent NOT IN (SELECT value FROM json_each(excluded));
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_fork_delete
    AFTER DELETE ON main.session_relation WHEN OLD.kind = 'forkParent'
BEGIN
    UPDATE activity_stream_changes SET newest_epoch = max(newest_epoch, coalesce((
        SELECT max(updated_at_epoch) FROM main.session
         WHERE environment_key = OLD.environment_key AND agent = OLD.agent
           AND session_id IN (OLD.session_id, OLD.related_id)
    ), 0))
     WHERE OLD.agent NOT IN (SELECT value FROM json_each(excluded));
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_repository_insert
    AFTER INSERT ON main.repository
    WHEN NEW.repo_root IS NOT NULL
BEGIN
    UPDATE activity_repository_revision SET revision = revision + 1;
    UPDATE activity_stream_changes SET newest_epoch = 9223372036854775807;
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_repository_update
    AFTER UPDATE OF repo_name, repo_root, full_name ON main.repository
    WHEN (OLD.repo_root IS NOT NULL OR NEW.repo_root IS NOT NULL)
     AND (OLD.repo_name IS NOT NEW.repo_name OR OLD.repo_root IS NOT NEW.repo_root
          OR OLD.full_name IS NOT NEW.full_name)
BEGIN
    UPDATE activity_repository_revision SET revision = revision + 1;
    UPDATE activity_stream_changes SET newest_epoch = 9223372036854775807;
END;

CREATE TEMP TRIGGER IF NOT EXISTS activity_repository_delete
    AFTER DELETE ON main.repository
    WHEN OLD.repo_root IS NOT NULL
BEGIN
    UPDATE activity_repository_revision SET revision = revision + 1;
    UPDATE activity_stream_changes SET newest_epoch = 9223372036854775807;
END;
