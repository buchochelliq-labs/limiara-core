ALTER TABLE audit_log
    ADD COLUMN correlation_id uuid,
    ADD COLUMN causation_id uuid;

CREATE INDEX audit_log_tenant_correlation_idx
    ON audit_log (tenant_id, correlation_id)
    WHERE correlation_id IS NOT NULL;
