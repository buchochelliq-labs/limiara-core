-- Roll-up summary fields (ADR 0026, #295): a master's count, sum, minimum
-- or maximum over its detail records, kept on the master. The function,
-- the master-detail field reached through and the summarised detail field
-- are stored; `formula_type` holds the result type, as for formulas.
ALTER TABLE field_definitions DROP CONSTRAINT field_definitions_field_type_check;
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_field_type_check CHECK (field_type IN (
    'text', 'long_text', 'email', 'phone', 'url', 'number', 'currency', 'percent',
    'boolean', 'date', 'date_time', 'picklist', 'multi_picklist', 'lookup', 'formula',
    'master_detail', 'rollup'));
ALTER TABLE field_definitions
    ADD COLUMN rollup_function text
        CHECK (rollup_function IS NULL OR rollup_function IN ('count', 'sum', 'min', 'max')),
    ADD COLUMN rollup_relationship_id uuid,
    ADD COLUMN rollup_field_id uuid,
    ADD CONSTRAINT field_definitions_rollup_shape_check CHECK (
        (field_type = 'rollup') = (rollup_function IS NOT NULL AND rollup_relationship_id IS NOT NULL
            AND formula_type IS NOT NULL)
        AND (rollup_function IS DISTINCT FROM 'count' OR rollup_field_id IS NULL)),
    ADD CONSTRAINT field_definitions_rollup_relationship_fk
        FOREIGN KEY (tenant_id, rollup_relationship_id) REFERENCES field_definitions(tenant_id, id),
    ADD CONSTRAINT field_definitions_rollup_field_fk
        FOREIGN KEY (tenant_id, rollup_field_id) REFERENCES field_definitions(tenant_id, id);
