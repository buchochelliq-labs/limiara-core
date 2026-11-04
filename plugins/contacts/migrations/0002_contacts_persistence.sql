CREATE TABLE p_contacts.public_id_counters (
  tenant_id uuid NOT NULL,
  construct text NOT NULL CHECK (construct IN ('person', 'organisation')),
  next_value bigint NOT NULL CHECK (next_value > 0),
  PRIMARY KEY (tenant_id, construct)
);

ALTER TABLE p_contacts.custom_field_definitions
  ADD COLUMN version bigint NOT NULL DEFAULT 1,
  ADD COLUMN disabled_at timestamptz,
  ADD COLUMN archived_at timestamptz;

ALTER TABLE p_contacts.custom_field_definitions
  ADD CONSTRAINT custom_field_definition_status_check
  CHECK (archived_at IS NULL OR disabled_at IS NOT NULL);

CREATE TABLE p_contacts.list_cursors (
  token_hash text PRIMARY KEY CHECK (token_hash ~ '^[0-9a-f]{64}$'),
  tenant_id uuid NOT NULL,
  resource text NOT NULL CHECK (resource IN ('people', 'organisations')),
  query_hash text NOT NULL CHECK (query_hash ~ '^[0-9a-f]{64}$'),
  sort_value text NOT NULL,
  record_id uuid NOT NULL,
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE p_contacts.public_id_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.public_id_counters FORCE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.list_cursors ENABLE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.list_cursors FORCE ROW LEVEL SECURITY;

CREATE POLICY public_id_counters_tenant_isolation
  ON p_contacts.public_id_counters
  USING (tenant_id::text = current_setting('app.tenant_id', true))
  WITH CHECK (tenant_id::text = current_setting('app.tenant_id', true));
CREATE POLICY list_cursors_tenant_isolation
  ON p_contacts.list_cursors
  USING (tenant_id::text = current_setting('app.tenant_id', true))
  WITH CHECK (tenant_id::text = current_setting('app.tenant_id', true));

CREATE INDEX list_cursors_expiry_idx ON p_contacts.list_cursors (expires_at);
CREATE INDEX people_tenant_created_idx ON p_contacts.people (tenant_id, created_at, id);
CREATE INDEX people_tenant_updated_idx ON p_contacts.people (tenant_id, updated_at, id);
CREATE INDEX organisations_tenant_created_idx ON p_contacts.organisations (tenant_id, created_at, id);
CREATE INDEX organisations_tenant_updated_idx ON p_contacts.organisations (tenant_id, updated_at, id);
CREATE UNIQUE INDEX custom_field_definitions_tenant_target_api_name_uidx
  ON p_contacts.custom_field_definitions (tenant_id, applies_to, api_name);

INSERT INTO permissions (id, description) VALUES
  ('contacts.people.custom-fields.read', 'Read permitted Contacts person custom fields'),
  ('contacts.organisations.read', 'Read Contacts organisations'),
  ('contacts.organisations.write', 'Create and update Contacts organisations'),
  ('contacts.organisations.custom-fields.read', 'Read permitted Contacts organisation custom fields'),
  ('contacts.settings.manage', 'Manage Contacts settings')
ON CONFLICT (id) DO NOTHING;
