-- Field history tracking (ADR 0017, #311).

-- An object tracks its records' lifecycle, name and owner; each field opts
-- in separately. Unlike Salesforce there is no cap on tracked fields.
ALTER TABLE object_definitions ADD COLUMN track_history boolean NOT NULL DEFAULT false;
ALTER TABLE field_definitions ADD COLUMN track_history boolean NOT NULL DEFAULT false;

-- One row per tracked change, written in the same transaction as the change
-- it records. Unlike events and audit, history carries the old and new
-- values, so it is read only through the engine, which applies record
-- visibility. Rows go with their record when the record is purged.
CREATE TABLE record_history (
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    id uuid NOT NULL,
    record_id uuid NOT NULL,
    object_id uuid NOT NULL,
    kind text NOT NULL CHECK (kind IN ('created', 'changed', 'deleted', 'undeleted')),
    field text CHECK (field IS NULL OR char_length(field) BETWEEN 1 AND 200),
    old_value jsonb CHECK (old_value IS NULL OR octet_length(old_value::text) <= 262144),
    new_value jsonb CHECK (new_value IS NULL OR octet_length(new_value::text) <= 262144),
    changed_at timestamptz NOT NULL,
    changed_by uuid,
    correlation_id uuid,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, record_id) REFERENCES records(tenant_id, id) ON DELETE CASCADE,
    CHECK ((kind = 'changed') = (field IS NOT NULL))
);
CREATE INDEX record_history_record_idx ON record_history (tenant_id, record_id, changed_at DESC, id DESC);

ALTER TABLE record_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE record_history FORCE ROW LEVEL SECURITY;
CREATE POLICY record_history_tenant_isolation ON record_history TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY record_history_migrator_management ON record_history TO platform_migrator
    USING (true) WITH CHECK (true);
-- History is append-only for the application; purging a record removes
-- its rows through the cascade, which runs with the owner's rights.
REVOKE UPDATE, DELETE, TRUNCATE ON record_history FROM platform_app;
