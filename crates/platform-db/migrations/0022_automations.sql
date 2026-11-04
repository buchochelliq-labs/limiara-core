-- Record-triggered automations (ADR 0022, #298): per-object metadata that
-- sets fields before a save, or creates and updates records after it, with
-- values written in the formula language. Evaluated in Rust, never as SQL.
CREATE TABLE automations (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    object_id uuid NOT NULL,
    name text NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    description text CHECK (char_length(description) <= 1000),
    trigger text NOT NULL CHECK (trigger IN ('create', 'update', 'create_or_update')),
    timing text NOT NULL CHECK (timing IN ('before_save', 'after_save')),
    condition text CHECK (char_length(condition) BETWEEN 1 AND 3900),
    actions jsonb NOT NULL CHECK (jsonb_typeof(actions) = 'array'),
    active boolean NOT NULL DEFAULT true,
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, object_id, name),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id)
);

ALTER TABLE automations ENABLE ROW LEVEL SECURITY;
ALTER TABLE automations FORCE ROW LEVEL SECURITY;
CREATE POLICY automations_tenant_isolation ON automations TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY automations_migrator_management ON automations TO platform_migrator
    USING (true) WITH CHECK (true);
