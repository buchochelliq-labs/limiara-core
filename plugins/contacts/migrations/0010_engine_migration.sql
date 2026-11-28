-- Moving a workspace's people and organisations to the object engine
-- (ADR 0035 §2): one state row per workspace, written only by the migration
-- tool in the Contacts execution context. Absent means the workspace has not
-- started (phase `legacy`).
CREATE TABLE p_contacts.engine_migration (
  tenant_id uuid PRIMARY KEY REFERENCES tenants(id) ON DELETE RESTRICT,
  phase text NOT NULL CHECK (phase IN ('legacy', 'copying', 'copied', 'frozen', 'engine')),
  checkpoint jsonb NOT NULL DEFAULT '{}',
  checksums jsonb NOT NULL DEFAULT '{}',
  cutover_at timestamptz,
  rollback_deadline timestamptz,
  version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
  updated_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE p_contacts.engine_migration ENABLE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.engine_migration FORCE ROW LEVEL SECURITY;

CREATE POLICY engine_migration_tenant_isolation
  ON p_contacts.engine_migration
  TO platform_app
  USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
  WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE POLICY engine_migration_plugin_context ON p_contacts.engine_migration
  AS RESTRICTIVE TO platform_app
  USING (current_setting('app.plugin_id', true) = 'platform.contacts')
  WITH CHECK (current_setting('app.plugin_id', true) = 'platform.contacts');

-- The export reads every record in id order and the activity of a batch.
CREATE INDEX activity_tenant_entity_oldest_idx
  ON p_contacts.activity (tenant_id, entity_type, entity_public_id, occurred_at, event_id);
