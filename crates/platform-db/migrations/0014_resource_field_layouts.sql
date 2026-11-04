-- Where each workspace shows the fields other plugins add to a resource
-- (ADR 0012). One governed layout per tenant and resource; the kernel
-- extension service validates it against active fields and host zones.
CREATE TABLE resource_field_layouts (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    resource text NOT NULL CHECK (resource ~ '^[a-z0-9]+(\.[a-z0-9-]+)+$' AND octet_length(resource) <= 200),
    layout jsonb NOT NULL
        CHECK (jsonb_typeof(layout) = 'object' AND octet_length(layout::text) <= 65536),
    revision bigint NOT NULL CHECK (revision >= 1),
    updated_by uuid NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, resource)
);

ALTER TABLE resource_field_layouts ENABLE ROW LEVEL SECURITY;
ALTER TABLE resource_field_layouts FORCE ROW LEVEL SECURITY;

CREATE POLICY resource_field_layouts_tenant_isolation
ON resource_field_layouts
TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE POLICY resource_field_layouts_migrator_management
ON resource_field_layouts
TO platform_migrator
USING (true)
WITH CHECK (true);
