-- Objects of an optional dependency a plugin's related-to field names in
-- its starting allowlist (ADR 0035 G18). Each entry waits here until its
-- object is in service in the workspace, then joins the field's allowlist
-- once: `offered_at` records that, so an administrator who removes it keeps
-- it removed.
CREATE TABLE field_allowlist_offers (
    tenant_id uuid NOT NULL,
    field_id uuid NOT NULL,
    object_api_name text NOT NULL CHECK (char_length(object_api_name) BETWEEN 1 AND 255),
    offered_at timestamptz,
    PRIMARY KEY (tenant_id, field_id, object_api_name)
);
-- Enabling a plugin looks up the entries still waiting.
CREATE INDEX field_allowlist_offers_pending_idx ON field_allowlist_offers (tenant_id)
    WHERE offered_at IS NULL;

ALTER TABLE field_allowlist_offers ENABLE ROW LEVEL SECURITY;
ALTER TABLE field_allowlist_offers FORCE ROW LEVEL SECURITY;
CREATE POLICY field_allowlist_offers_tenant_isolation ON field_allowlist_offers TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY field_allowlist_offers_migrator_management ON field_allowlist_offers TO platform_migrator
    USING (true) WITH CHECK (true);
