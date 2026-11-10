-- Timeline sources (ADR 0033 §4): how a plugin object's records appear on
-- the Activity timeline of the records they point at, kept per workspace
-- from the plugin's manifest (`[objects.timeline]`) at install and upgrade
-- and dropped when the declaration is. The timeline itself reads records
-- through the query language as the viewer; this table only says which
-- fields to follow, which date orders the list and when a record is open.
CREATE TABLE timeline_sources (
    tenant_id uuid NOT NULL,
    -- The plugin object whose records are listed: at most one source each.
    object_id uuid NOT NULL,
    owner_plugin text NOT NULL,
    -- Ids of its lookup or related-to fields that may point at the viewed
    -- record, in the declaration's order.
    through jsonb NOT NULL CHECK (jsonb_typeof(through) = 'array'
        AND jsonb_array_length(through) BETWEEN 1 AND 5),
    -- Its date or date-time field ordering the timeline.
    date_field_id uuid NOT NULL,
    -- When a record is open: while a picklist has a value, while the date
    -- is today or later, or never.
    open_kind text CHECK (open_kind IN ('field_equals', 'upcoming')),
    open_field_id uuid,
    open_value text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, object_id),
    CHECK ((open_kind = 'field_equals') = (open_field_id IS NOT NULL AND open_value IS NOT NULL)),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id),
    FOREIGN KEY (tenant_id, date_field_id) REFERENCES field_definitions(tenant_id, id),
    FOREIGN KEY (tenant_id, open_field_id) REFERENCES field_definitions(tenant_id, id)
);
-- Every timeline read lists the workspace's sources: the primary key's
-- tenant-first index serves it.

ALTER TABLE timeline_sources ENABLE ROW LEVEL SECURITY;
ALTER TABLE timeline_sources FORCE ROW LEVEL SECURITY;
CREATE POLICY timeline_sources_tenant_isolation ON timeline_sources TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY timeline_sources_migrator_management ON timeline_sources TO platform_migrator
    USING (true) WITH CHECK (true);
