-- Activity timeline read model, built by the Contacts worker consumer
-- platform.contacts.activity-timeline-v1 from the plugin's own versioned
-- events. It records what happened to a record and when, never personal
-- attributes: those stay in the source tables, as in the integration events.
CREATE TABLE p_contacts.activity (
  tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
  event_id uuid NOT NULL,
  entity_type text NOT NULL CHECK (entity_type IN ('person', 'organisation')),
  entity_public_id text NOT NULL CHECK (entity_public_id ~ '^(per|org)[0-9a-hjkmnp-tv-z]{15}$'),
  action text NOT NULL CHECK (action IN ('created', 'updated', 'archived')),
  entity_version bigint NOT NULL CHECK (entity_version > 0),
  actor_id uuid,
  correlation_id uuid NOT NULL,
  occurred_at timestamptz NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, event_id),
  CHECK (left(entity_public_id, 3) = CASE entity_type WHEN 'person' THEN 'per' ELSE 'org' END)
);

CREATE INDEX activity_tenant_entity_recent_idx
  ON p_contacts.activity (tenant_id, entity_type, entity_public_id, occurred_at DESC, event_id DESC);

ALTER TABLE p_contacts.activity ENABLE ROW LEVEL SECURITY;
ALTER TABLE p_contacts.activity FORCE ROW LEVEL SECURITY;

CREATE POLICY activity_tenant_isolation
  ON p_contacts.activity
  TO platform_app
  USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
  WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

-- The timeline is an append-only projection of committed events.
REVOKE UPDATE, DELETE, TRUNCATE ON p_contacts.activity FROM platform_app;

-- Expired list cursors are removed per tenant by the
-- platform.contacts.list-cursor-cleanup-v1 job; keep that scan tenant-first.
CREATE INDEX list_cursors_tenant_expiry_idx
  ON p_contacts.list_cursors (tenant_id, expires_at);
