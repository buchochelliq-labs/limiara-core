ALTER TABLE plugin_catalogue
  ADD COLUMN IF NOT EXISTS latest_usable_version text,
  ADD COLUMN IF NOT EXISTS updated_at timestamptz NOT NULL DEFAULT now();

CREATE TABLE plugin_versions (
  plugin_id text NOT NULL REFERENCES plugin_catalogue(plugin_id),
  version text NOT NULL,
  manifest jsonb NOT NULL,
  manifest_path text NOT NULL,
  artifact_path text,
  declared_digest text,
  verified_digest text CHECK (verified_digest IS NULL OR verified_digest ~ '^sha256:[0-9a-f]{64}$'),
  lifecycle_state text NOT NULL CHECK (lifecycle_state IN (
    'discovered', 'validated', 'staged', 'migrated', 'registered',
    'health_checked', 'failed'
  )),
  failure_control text,
  failure_detail text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (plugin_id, version),
  CHECK (
    (lifecycle_state = 'failed' AND failure_control IS NOT NULL)
    OR (lifecycle_state <> 'failed' AND failure_control IS NULL AND verified_digest IS NOT NULL)
  )
);

ALTER TABLE tenant_plugin_installations
  ADD COLUMN IF NOT EXISTS active_version text,
  ADD COLUMN IF NOT EXISTS candidate_version text,
  ADD COLUMN IF NOT EXISTS previous_state text,
  ADD COLUMN IF NOT EXISTS transition_id uuid,
  ADD COLUMN IF NOT EXISTS transition_started_at timestamptz,
  ADD COLUMN IF NOT EXISTS last_error_control text,
  ADD COLUMN IF NOT EXISTS updated_at timestamptz NOT NULL DEFAULT now();

CREATE INDEX plugin_versions_state_idx
  ON plugin_versions (plugin_id, lifecycle_state, version DESC);
CREATE INDEX tenant_plugin_transition_idx
  ON tenant_plugin_installations (state, transition_started_at)
  WHERE state IN ('enabling', 'disabling');

INSERT INTO tenants (id, display_name)
VALUES ('018f0000-0000-7000-8000-000000000002', 'Local test workspace')
ON CONFLICT (id) DO UPDATE SET display_name = EXCLUDED.display_name;

INSERT INTO users (id, display_name, email)
VALUES ('018f0000-0000-7000-8000-000000000104', 'Riley Operator', 'operator@local.limiara.test')
ON CONFLICT (id) DO UPDATE SET
  display_name = EXCLUDED.display_name,
  email = EXCLUDED.email;

INSERT INTO memberships (tenant_id, user_id) VALUES
  ('018f0000-0000-7000-8000-000000000001', '018f0000-0000-7000-8000-000000000104'),
  ('018f0000-0000-7000-8000-000000000002', '018f0000-0000-7000-8000-000000000104')
ON CONFLICT DO NOTHING;

INSERT INTO external_identities (issuer, subject, user_id)
VALUES (
  'http://auth.localhost:8081/realms/local',
  '018f0000-0000-7000-8000-000000000104',
  '018f0000-0000-7000-8000-000000000104'
)
ON CONFLICT (issuer, subject) DO NOTHING;

INSERT INTO roles (tenant_id, id, name) VALUES
  ('018f0000-0000-7000-8000-000000000002', '018f0000-0000-7000-8000-000000000202', 'Local test administrator')
ON CONFLICT DO NOTHING;

INSERT INTO role_permissions (tenant_id, role_id, permission_id)
SELECT
  '018f0000-0000-7000-8000-000000000002'::uuid,
  '018f0000-0000-7000-8000-000000000202'::uuid,
  id
FROM permissions
WHERE id IN ('platform.workspaces.read', 'platform.plugins.manage')
ON CONFLICT DO NOTHING;

INSERT INTO user_roles (tenant_id, user_id, role_id)
VALUES (
  '018f0000-0000-7000-8000-000000000002',
  '018f0000-0000-7000-8000-000000000104',
  '018f0000-0000-7000-8000-000000000202'
)
ON CONFLICT DO NOTHING;
