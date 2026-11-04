ALTER TABLE inbox
    ADD CONSTRAINT inbox_tenant_fk
        FOREIGN KEY (tenant_id) REFERENCES tenants(id) ON DELETE RESTRICT,
    ADD CONSTRAINT inbox_consumer_id_length
        CHECK (octet_length(consumer_id) BETWEEN 1 AND 100),
    ADD CONSTRAINT inbox_consumer_id_shape
        CHECK (
            consumer_id ~ '^[a-z][a-z0-9._-]*[a-z0-9]$'
            OR consumer_id ~ '^[a-z]$'
        );

ALTER TABLE inbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE inbox FORCE ROW LEVEL SECURITY;

CREATE POLICY inbox_tenant_isolation
ON inbox
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

-- The inbox is an append-only processing ledger. Runtime consumers may read
-- their tenant-scoped markers and insert new ones, but cannot rewrite history.
REVOKE UPDATE, DELETE ON inbox FROM platform_app;
