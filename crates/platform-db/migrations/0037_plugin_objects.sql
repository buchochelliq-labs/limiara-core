-- Plugin-declared engine objects (ADR 0033 §2). A plugin's objects are
-- ordinary engine definitions with `origin = 'plugin'` and the plugin as
-- `owner_plugin`; enabling the plugin in a workspace creates or migrates
-- them through the object engine. While the plugin is not enabled there
-- they are suspended: kept with their records and fields, but read as
-- missing everywhere. Only plugin objects can be suspended.
ALTER TABLE object_definitions ADD COLUMN suspended boolean NOT NULL DEFAULT false;
ALTER TABLE object_definitions ADD CONSTRAINT object_definitions_suspended_check
    CHECK (NOT suspended OR origin = 'plugin');
-- A plugin object names its owner, and its API name is in the owner's
-- namespace; a workspace object has no owner.
ALTER TABLE object_definitions ADD CONSTRAINT object_definitions_owner_plugin_check
    CHECK ((origin = 'plugin') = (owner_plugin IS NOT NULL)
        AND (owner_plugin IS NULL OR split_part(api_name, ':', 1) = owner_plugin));

-- Enabling, disabling and installing find a plugin's objects in a
-- workspace.
CREATE INDEX object_definitions_owner_plugin_idx
    ON object_definitions (tenant_id, owner_plugin) WHERE owner_plugin IS NOT NULL;
