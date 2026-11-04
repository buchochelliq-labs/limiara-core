-- Stored formula values (ADR 0021 §6): which objects have them built.
-- Formula fields created before values were stored have no index rows, so
-- an object without a mark is reindexed once, the first time a query needs
-- its stored formulas. Every reindex marks the object.
CREATE TABLE formula_index_marks (
    tenant_id uuid NOT NULL,
    object_id uuid NOT NULL,
    indexed_at timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, object_id)
);

ALTER TABLE formula_index_marks ENABLE ROW LEVEL SECURITY;
ALTER TABLE formula_index_marks FORCE ROW LEVEL SECURITY;
CREATE POLICY formula_index_marks_tenant_isolation ON formula_index_marks TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY formula_index_marks_migrator_management ON formula_index_marks TO platform_migrator
    USING (true) WITH CHECK (true);
