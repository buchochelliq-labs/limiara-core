-- Archiving engine records (ADR 0035 G1). `records.archived_at` exists
-- since 0015; archiving sets it and unarchiving clears it, each as a
-- member's edit with its own event, change-log entry and history entry.
--
-- History gains two lifecycle kinds. Like `created`, `deleted` and
-- `undeleted` they name no field; the CHECK pairing `changed` with a field
-- stays as it is. The table keeps its forced row-level security, its
-- policies and its append-only grants.
ALTER TABLE record_history DROP CONSTRAINT record_history_kind_check;
ALTER TABLE record_history ADD CONSTRAINT record_history_kind_check
    CHECK (kind IN ('created', 'changed', 'deleted', 'undeleted', 'archived', 'unarchived'));

-- `status=archived` lists one object's archived, live records; a partial,
-- tenant-first index keeps them cheap however few they are.
CREATE INDEX records_tenant_archived_idx ON records (tenant_id, object_id, archived_at, id)
    WHERE archived_at IS NOT NULL AND deleted_at IS NULL;
