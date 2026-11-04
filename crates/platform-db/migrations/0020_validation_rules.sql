-- Validation rules (ADR 0021, #298): per-object conditions, written in the
-- formula language, that block a save with a message. Rules are metadata:
-- versioned, audited and evaluated in Rust, never as SQL.
CREATE TABLE validation_rules (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    object_id uuid NOT NULL,
    name text NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    description text CHECK (char_length(description) <= 1000),
    condition text NOT NULL CHECK (char_length(condition) BETWEEN 1 AND 3900),
    message text NOT NULL CHECK (char_length(message) BETWEEN 1 AND 255),
    error_field text,
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

ALTER TABLE validation_rules ENABLE ROW LEVEL SECURITY;
ALTER TABLE validation_rules FORCE ROW LEVEL SECURITY;
CREATE POLICY validation_rules_tenant_isolation ON validation_rules TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY validation_rules_migrator_management ON validation_rules TO platform_migrator
    USING (true) WITH CHECK (true);
