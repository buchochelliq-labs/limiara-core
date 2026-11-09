-- Quick actions (ADR 0032): per-object shortcuts to the record forms that
-- update chosen fields of a record or create a related child record, with
-- constant presets. Actions are metadata: versioned, audited and checked in
-- Rust, and they grant nothing beyond the normal create and update APIs.
CREATE TABLE quick_actions (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    object_id uuid NOT NULL,
    name text NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 80),
    kind text NOT NULL CHECK (kind IN ('update', 'create')),
    position integer NOT NULL CHECK (position >= 0),
    -- The child object and its reference field, for create actions only.
    target_object_id uuid,
    target_field text,
    fields jsonb NOT NULL CHECK (jsonb_typeof(fields) = 'array'
        AND jsonb_array_length(fields) BETWEEN 1 AND 20),
    presets jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(presets) = 'object'),
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    CHECK ((kind = 'create') = (target_object_id IS NOT NULL AND target_field IS NOT NULL)),
    CHECK ((target_object_id IS NULL) = (target_field IS NULL)),
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, object_id, name),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id),
    FOREIGN KEY (tenant_id, target_object_id) REFERENCES object_definitions(tenant_id, id)
);

-- Deleting a child object or its fields finds the create actions that
-- depend on it.
CREATE INDEX quick_actions_target_idx ON quick_actions (tenant_id, target_object_id)
    WHERE target_object_id IS NOT NULL;

ALTER TABLE quick_actions ENABLE ROW LEVEL SECURITY;
ALTER TABLE quick_actions FORCE ROW LEVEL SECURITY;
CREATE POLICY quick_actions_tenant_isolation ON quick_actions TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY quick_actions_migrator_management ON quick_actions TO platform_migrator
    USING (true) WITH CHECK (true);
