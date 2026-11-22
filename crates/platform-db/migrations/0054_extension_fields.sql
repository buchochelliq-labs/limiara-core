-- Fields one plugin adds to another plugin's object (ADR 0035 G6).
--
-- A plugin object may declare the record resource it serves, such as
-- `platform.contacts.person`. Other plugins' ADR 0012 extension fields for
-- that resource then become fields of the object, in the extending
-- plugin's namespace and owned by it. At most one object per resource in
-- a workspace.
ALTER TABLE object_definitions
    ADD COLUMN resource text
        CHECK (resource IS NULL OR resource ~ '^[a-z0-9]+(\.[a-z0-9_-]+)+$');
CREATE UNIQUE INDEX object_definitions_resource_idx
    ON object_definitions (tenant_id, resource) WHERE resource IS NOT NULL;

-- An extension field follows its plugin, not the object's: it is out of
-- service, and kept with its values, while the extending plugin is not
-- enabled. Every read leaves suspended fields out, as it does deleted ones.
ALTER TABLE field_definitions
    ADD COLUMN suspended boolean NOT NULL DEFAULT false;

-- Each plugin's extension declarations in the workspace, kept so they
-- reach host objects installed after the extending plugin.
CREATE TABLE plugin_extensions (
    tenant_id uuid NOT NULL,
    plugin_id text NOT NULL CHECK (plugin_id ~ '^[a-z0-9]+(\.[a-z0-9-]+)+$'),
    declarations jsonb NOT NULL CHECK (jsonb_typeof(declarations) = 'array'),
    suspended boolean NOT NULL DEFAULT true,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, plugin_id)
);

ALTER TABLE plugin_extensions ENABLE ROW LEVEL SECURITY;
ALTER TABLE plugin_extensions FORCE ROW LEVEL SECURITY;
CREATE POLICY plugin_extensions_tenant_isolation ON plugin_extensions TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY plugin_extensions_migrator_management ON plugin_extensions TO platform_migrator
    USING (true) WITH CHECK (true);
