CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE TABLE tenants (id uuid PRIMARY KEY, display_name text NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE users (id uuid PRIMARY KEY, display_name text NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE external_identities (issuer text NOT NULL, subject text NOT NULL, user_id uuid NOT NULL REFERENCES users(id), PRIMARY KEY (issuer, subject));
CREATE TABLE memberships (tenant_id uuid NOT NULL REFERENCES tenants(id), user_id uuid NOT NULL REFERENCES users(id), PRIMARY KEY (tenant_id, user_id));
CREATE TABLE roles (tenant_id uuid NOT NULL REFERENCES tenants(id), id uuid NOT NULL, name text NOT NULL, PRIMARY KEY (tenant_id, id));
CREATE TABLE permissions (id text PRIMARY KEY, description text NOT NULL);
CREATE TABLE role_permissions (tenant_id uuid NOT NULL, role_id uuid NOT NULL, permission_id text NOT NULL REFERENCES permissions(id), PRIMARY KEY (tenant_id, role_id, permission_id));
CREATE TABLE user_roles (tenant_id uuid NOT NULL, user_id uuid NOT NULL, role_id uuid NOT NULL, PRIMARY KEY (tenant_id, user_id, role_id));
CREATE TABLE plugin_catalogue (plugin_id text PRIMARY KEY, manifest jsonb NOT NULL, lifecycle_state text NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE tenant_plugin_installations (tenant_id uuid NOT NULL REFERENCES tenants(id), plugin_id text NOT NULL REFERENCES plugin_catalogue(plugin_id), state text NOT NULL, enabled_at timestamptz, disabled_at timestamptz, PRIMARY KEY (tenant_id, plugin_id));
CREATE TABLE plugin_migration_ledger (tenant_id uuid, plugin_id text NOT NULL, migration_id text NOT NULL, checksum text NOT NULL, applied_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (tenant_id, plugin_id, migration_id));
CREATE TABLE audit_log (id uuid PRIMARY KEY, tenant_id uuid, actor_id uuid, action text NOT NULL, target text NOT NULL, metadata jsonb NOT NULL DEFAULT '{}', occurred_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE outbox (id uuid PRIMARY KEY, tenant_id uuid NOT NULL, event_type text NOT NULL, envelope jsonb NOT NULL, published_at timestamptz, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE inbox (tenant_id uuid NOT NULL, consumer_id text NOT NULL, event_id uuid NOT NULL, processed_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (tenant_id, consumer_id, event_id));
CREATE TABLE idempotency_keys (tenant_id uuid NOT NULL, key text NOT NULL, response jsonb, created_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (tenant_id, key));
CREATE TABLE sessions (id uuid PRIMARY KEY, user_id uuid NOT NULL REFERENCES users(id), expires_at timestamptz NOT NULL, revoked_at timestamptz, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE entitlement_snapshots (tenant_id uuid PRIMARY KEY REFERENCES tenants(id), entitlements jsonb NOT NULL DEFAULT '{}', updated_at timestamptz NOT NULL DEFAULT now());

CREATE TABLE id_prefix_registry (prefix text PRIMARY KEY CHECK (prefix ~ '^[a-z0-9]{3}$'), plugin_id text NOT NULL, construct text NOT NULL, description text NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE deployment_placements (tenant_id uuid PRIMARY KEY REFERENCES tenants(id), cell text NOT NULL DEFAULT 'local', region text NOT NULL DEFAULT 'local');

CREATE INDEX memberships_user_tenant_idx ON memberships (user_id, tenant_id);
CREATE INDEX tenant_plugin_state_idx ON tenant_plugin_installations (tenant_id, state, plugin_id);
CREATE INDEX audit_log_tenant_time_idx ON audit_log (tenant_id, occurred_at DESC, id DESC);
CREATE INDEX outbox_unpublished_idx ON outbox (published_at, created_at, id) WHERE published_at IS NULL;
CREATE INDEX outbox_tenant_type_idx ON outbox (tenant_id, event_type, created_at DESC);
CREATE INDEX sessions_user_expiry_idx ON sessions (user_id, expires_at DESC) WHERE revoked_at IS NULL;
CREATE INDEX idempotency_keys_created_idx ON idempotency_keys (tenant_id, created_at DESC);
