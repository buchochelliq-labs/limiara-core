-- Related actions (ADR 0033 §3): a create quick action (ADR 0032) that a
-- plugin object declares for every object entering one of its related-to
-- fields' allowlists. The kernel keeps the declaration per workspace, with
-- the objects it was already offered to, so the engine creates the action
-- only when an object enters the allowlist: an action an administrator
-- deleted stays deleted while its object stays in the allowlist. The quick
-- actions it creates are ordinary workspace quick actions.
CREATE TABLE related_actions (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    -- The plugin object whose records the action makes, and its related-to
    -- field set to the current record.
    object_id uuid NOT NULL,
    field_id uuid NOT NULL,
    owner_plugin text NOT NULL,
    name text NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 80),
    fields jsonb NOT NULL CHECK (jsonb_typeof(fields) = 'array'
        AND jsonb_array_length(fields) BETWEEN 1 AND 20),
    presets jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(presets) = 'object'),
    -- Ids of the allowlisted objects already offered the action.
    offered_to jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(offered_to) = 'array'),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, field_id, name),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id),
    FOREIGN KEY (tenant_id, field_id) REFERENCES field_definitions(tenant_id, id)
);

-- Installing, resuming and changing an allowlist read a plugin object's
-- related actions.
CREATE INDEX related_actions_object_idx ON related_actions (tenant_id, object_id);

ALTER TABLE related_actions ENABLE ROW LEVEL SECURITY;
ALTER TABLE related_actions FORCE ROW LEVEL SECURITY;
CREATE POLICY related_actions_tenant_isolation ON related_actions TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY related_actions_migrator_management ON related_actions TO platform_migrator
    USING (true) WITH CHECK (true);
