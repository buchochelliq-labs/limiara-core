-- The assistant policy (ADR 0034 §4, #234): what a device's assistant
-- bridge may reach. It is object metadata like validation rules and quick
-- actions: versioned, audited, published and checked in Rust, and it moves
-- the workspace metadata version so it reaches device replicas with the
-- object definitions. It only narrows what the member may already do and
-- grants nothing. Every setting is off until an administrator turns it on:
-- a workspace or object without a row has assistant access off.

-- The workspace switch: assistant access on or off, one row per workspace.
CREATE TABLE assistant_settings (
    tenant_id uuid NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    PRIMARY KEY (tenant_id)
);

ALTER TABLE assistant_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE assistant_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY assistant_settings_tenant_isolation ON assistant_settings TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY assistant_settings_migrator_management ON assistant_settings TO platform_migrator
    USING (true) WITH CHECK (true);

-- Per object: exposed (assistants may find and read its records as the
-- member), writable and indexable (each requires exposed), the fields whose
-- values never enter the OS index, and the quick actions offered to
-- assistants. Fields and actions are kept by id, so a field or action that
-- is deleted and made again under the same name is not excluded or offered
-- by accident; ids that no longer resolve are ignored when read.
CREATE TABLE assistant_object_policies (
    tenant_id uuid NOT NULL,
    object_id uuid NOT NULL,
    exposed boolean NOT NULL DEFAULT false,
    writable boolean NOT NULL DEFAULT false,
    indexable boolean NOT NULL DEFAULT false,
    -- Whether the record name is excluded from the index too.
    exclude_name boolean NOT NULL DEFAULT false,
    excluded_field_ids jsonb NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(excluded_field_ids) = 'array'),
    offered_action_ids jsonb NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(offered_action_ids) = 'array'
            AND jsonb_array_length(offered_action_ids) <= 10),
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    CHECK (exposed OR NOT writable),
    CHECK (exposed OR NOT indexable),
    CHECK (exposed OR jsonb_array_length(offered_action_ids) = 0),
    PRIMARY KEY (tenant_id, object_id),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id)
);
-- Every read lists the workspace's policies: the primary key's tenant-first
-- index serves it.

ALTER TABLE assistant_object_policies ENABLE ROW LEVEL SECURITY;
ALTER TABLE assistant_object_policies FORCE ROW LEVEL SECURITY;
CREATE POLICY assistant_object_policies_tenant_isolation ON assistant_object_policies TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY assistant_object_policies_migrator_management ON assistant_object_policies
    TO platform_migrator USING (true) WITH CHECK (true);
