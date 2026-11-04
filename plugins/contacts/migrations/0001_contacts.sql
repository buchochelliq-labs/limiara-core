CREATE SCHEMA IF NOT EXISTS p_contacts;
CREATE TABLE p_contacts.organisations (
  tenant_id uuid NOT NULL,
  id uuid NOT NULL,
  public_id text NOT NULL CHECK (public_id ~ '^[0-9a-z]{18}$'),
  name text NOT NULL,
  email text,
  telephone text,
  custom_fields jsonb NOT NULL DEFAULT '{}',
  version bigint NOT NULL DEFAULT 1,
  archived_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, id)
);
CREATE TABLE p_contacts.people (
  tenant_id uuid NOT NULL,
  id uuid NOT NULL,
  public_id text NOT NULL CHECK (public_id ~ '^[0-9a-z]{18}$'),
  full_name text NOT NULL,
  email text,
  telephone text,
  organisation_id uuid,
  tags text[] NOT NULL DEFAULT '{}',
  custom_fields jsonb NOT NULL DEFAULT '{}',
  version bigint NOT NULL DEFAULT 1,
  archived_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, id),
  FOREIGN KEY (tenant_id, organisation_id) REFERENCES p_contacts.organisations(tenant_id, id)
);

CREATE TABLE p_contacts.custom_field_definitions (
  tenant_id uuid NOT NULL,
  id uuid NOT NULL,
  applies_to text NOT NULL CHECK (applies_to IN ('person', 'organisation')),
  api_name text NOT NULL CHECK (api_name ~ '^[a-z][a-z0-9_]*__c$'),
  label text NOT NULL,
  description text,
  field_type text NOT NULL CHECK (field_type IN ('text', 'textarea', 'email', 'phone', 'number', 'boolean', 'date', 'datetime', 'select', 'multi_select', 'json')),
  required boolean NOT NULL DEFAULT false,
  unique_per_tenant boolean NOT NULL DEFAULT false,
  indexed boolean NOT NULL DEFAULT false,
  options jsonb NOT NULL DEFAULT '[]',
  validation_schema jsonb NOT NULL DEFAULT '{}',
  data_classification text NOT NULL DEFAULT 'personal' CHECK (data_classification IN ('public', 'internal', 'personal', 'sensitive')),
  purpose text,
  retention_days integer CHECK (retention_days IS NULL OR retention_days > 0),
  encrypted boolean NOT NULL DEFAULT false,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  created_by uuid,
  updated_by uuid,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, applies_to, api_name)
);
INSERT INTO id_prefix_registry (prefix, plugin_id, construct, description) VALUES ('per', 'platform.contacts', 'person', 'Contacts person public ID'), ('org', 'platform.contacts', 'organisation', 'Contacts organisation public ID') ON CONFLICT (prefix) DO NOTHING;
ALTER TABLE p_contacts.organisations ENABLE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.organisations FORCE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.people ENABLE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.people FORCE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.custom_field_definitions ENABLE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.custom_field_definitions FORCE ROW LEVEL SECURITY;
CREATE POLICY organisations_tenant_isolation ON p_contacts.organisations USING (tenant_id::text = current_setting('app.tenant_id', true)) WITH CHECK (tenant_id::text = current_setting('app.tenant_id', true));
CREATE POLICY people_tenant_isolation ON p_contacts.people USING (tenant_id::text = current_setting('app.tenant_id', true)) WITH CHECK (tenant_id::text = current_setting('app.tenant_id', true));
CREATE POLICY custom_field_definitions_tenant_isolation ON p_contacts.custom_field_definitions USING (tenant_id::text = current_setting('app.tenant_id', true)) WITH CHECK (tenant_id::text = current_setting('app.tenant_id', true));
CREATE UNIQUE INDEX organisations_tenant_public_id_idx ON p_contacts.organisations (tenant_id, public_id);
CREATE UNIQUE INDEX people_tenant_public_id_idx ON p_contacts.people (tenant_id, public_id);
CREATE INDEX people_tenant_name_idx ON p_contacts.people (tenant_id, lower(full_name), id);
CREATE INDEX organisations_tenant_name_idx ON p_contacts.organisations (tenant_id, lower(name), id);

CREATE INDEX people_tenant_email_idx ON p_contacts.people (tenant_id, lower(email)) WHERE email IS NOT NULL;
CREATE INDEX people_tenant_archived_idx ON p_contacts.people (tenant_id, archived_at, id);
CREATE INDEX people_tenant_org_idx ON p_contacts.people (tenant_id, organisation_id, id) WHERE organisation_id IS NOT NULL;
CREATE INDEX organisations_tenant_email_idx ON p_contacts.organisations (tenant_id, lower(email)) WHERE email IS NOT NULL;
CREATE INDEX organisations_tenant_archived_idx ON p_contacts.organisations (tenant_id, archived_at, id);
CREATE INDEX custom_field_definitions_lookup_idx ON p_contacts.custom_field_definitions (tenant_id, applies_to, api_name);
CREATE INDEX custom_field_definitions_classification_idx ON p_contacts.custom_field_definitions (tenant_id, data_classification, applies_to);
CREATE INDEX people_custom_fields_gin_idx ON p_contacts.people USING gin (custom_fields);
CREATE INDEX organisations_custom_fields_gin_idx ON p_contacts.organisations USING gin (custom_fields);
