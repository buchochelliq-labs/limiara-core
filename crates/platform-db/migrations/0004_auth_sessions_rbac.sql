ALTER TABLE users ADD COLUMN IF NOT EXISTS email text;

ALTER TABLE sessions ADD COLUMN IF NOT EXISTS token_hash text;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS last_seen_at timestamptz NOT NULL DEFAULT now();
UPDATE sessions
SET token_hash = encode(digest(id::text, 'sha256'), 'hex')
WHERE token_hash IS NULL;
ALTER TABLE sessions ALTER COLUMN token_hash SET NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS sessions_token_hash_idx ON sessions (token_hash);

CREATE TABLE oidc_login_transactions (
  state_hash text PRIMARY KEY,
  nonce text NOT NULL,
  pkce_verifier text NOT NULL,
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX oidc_login_transactions_expiry_idx ON oidc_login_transactions (expires_at);

INSERT INTO tenants (id, display_name)
VALUES ('018f0000-0000-7000-8000-000000000001', 'Local workspace')
ON CONFLICT (id) DO NOTHING;

INSERT INTO users (id, display_name, email)
VALUES ('018f0000-0000-7000-8000-000000000101', 'Morgan Member', 'member@local.limiara.test')
ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email;

INSERT INTO external_identities (issuer, subject, user_id)
VALUES (
  'http://auth.localhost:8081/realms/local',
  '018f0000-0000-7000-8000-000000000101',
  '018f0000-0000-7000-8000-000000000101'
)
ON CONFLICT (issuer, subject) DO NOTHING;

INSERT INTO memberships (tenant_id, user_id)
VALUES (
  '018f0000-0000-7000-8000-000000000001',
  '018f0000-0000-7000-8000-000000000101'
)
ON CONFLICT DO NOTHING;

INSERT INTO roles (tenant_id, id, name)
VALUES (
  '018f0000-0000-7000-8000-000000000001',
  '018f0000-0000-7000-8000-000000000201',
  'Local administrator'
)
ON CONFLICT DO NOTHING;

INSERT INTO permissions (id, description) VALUES
  ('platform.workspaces.read', 'List workspaces available to the current user'),
  ('platform.plugins.manage', 'Enable and disable workspace plugins'),
  ('contacts.people.read', 'Read Contacts people'),
  ('contacts.people.write', 'Create and update Contacts people'),
  ('ui.metadata.manage', 'Manage tenant UI metadata publications')
ON CONFLICT (id) DO NOTHING;

INSERT INTO role_permissions (tenant_id, role_id, permission_id)
SELECT
  '018f0000-0000-7000-8000-000000000001'::uuid,
  '018f0000-0000-7000-8000-000000000201'::uuid,
  id
FROM permissions
WHERE id IN (
  'platform.workspaces.read',
  'platform.plugins.manage',
  'contacts.people.read',
  'contacts.people.write',
  'ui.metadata.manage'
)
ON CONFLICT DO NOTHING;

INSERT INTO user_roles (tenant_id, user_id, role_id)
VALUES (
  '018f0000-0000-7000-8000-000000000001',
  '018f0000-0000-7000-8000-000000000101',
  '018f0000-0000-7000-8000-000000000201'
)
ON CONFLICT DO NOTHING;
