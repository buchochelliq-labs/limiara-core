-- Change data capture and incremental sync (ADR 0018, #306, #287).

-- One gap-free position counter per workspace. Taking the next position
-- locks the workspace's row until commit, so positions are assigned in
-- commit order and a reader resuming after position N never misses a
-- change that commits later with a smaller position.
CREATE TABLE change_counters (
    tenant_id uuid PRIMARY KEY REFERENCES tenants(id),
    last_position bigint NOT NULL DEFAULT 0 CHECK (last_position >= 0),
    -- Every position up to here has expired and may have been removed.
    expired_through bigint NOT NULL DEFAULT 0 CHECK (expired_through >= 0)
);

-- The change log: which record changed and which fields, never values.
-- Subscribers read current values through the record API, so record
-- visibility applies to every subscriber.
CREATE TABLE record_changes (
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    position bigint NOT NULL CHECK (position >= 1),
    record_id uuid NOT NULL,
    object_id uuid NOT NULL,
    public_id text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('created', 'updated', 'deleted', 'undeleted')),
    fields jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(fields) = 'array'),
    changed_at timestamptz NOT NULL,
    changed_by uuid,
    correlation_id uuid,
    PRIMARY KEY (tenant_id, position)
);
CREATE INDEX record_changes_time_idx ON record_changes (tenant_id, changed_at, position);

ALTER TABLE change_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE change_counters FORCE ROW LEVEL SECURITY;
CREATE POLICY change_counters_tenant_isolation ON change_counters TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY change_counters_migrator_management ON change_counters TO platform_migrator
    USING (true) WITH CHECK (true);

-- The log is append-only for the application, except that changes older
-- than the 72-hour retention window may be removed.
ALTER TABLE record_changes ENABLE ROW LEVEL SECURITY;
ALTER TABLE record_changes FORCE ROW LEVEL SECURITY;
CREATE POLICY record_changes_tenant_read ON record_changes FOR SELECT TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY record_changes_tenant_append ON record_changes FOR INSERT TO platform_app
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY record_changes_expired_removal ON record_changes FOR DELETE TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
           AND changed_at < now() - interval '72 hours');
CREATE POLICY record_changes_migrator_management ON record_changes TO platform_migrator
    USING (true) WITH CHECK (true);
REVOKE UPDATE, TRUNCATE ON record_changes FROM platform_app;

-- Incremental sync: everything touched since a moment, in order.
CREATE INDEX records_tenant_object_touched_idx ON records (tenant_id, object_id, touched_at, id);
