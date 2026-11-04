CREATE TABLE ui_metadata_page_state (
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    page_id text NOT NULL,
    revision bigint NOT NULL CHECK (revision >= 0),
    publication_generation bigint NOT NULL CHECK (publication_generation >= 0),
    state jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, page_id)
);

ALTER TABLE ui_metadata_page_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE ui_metadata_page_state FORCE ROW LEVEL SECURITY;

CREATE POLICY ui_metadata_page_state_tenant_isolation
ON ui_metadata_page_state
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE INDEX ui_metadata_page_state_generation_idx
ON ui_metadata_page_state (tenant_id, publication_generation DESC, page_id);
