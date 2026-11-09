-- Related-to fields (ADR 0033 §1): a reference to a record of any object in
-- an allowlist kept on the field. The value is the target's public id, as
-- for lookups, and `record_references` indexes it by target, so related
-- lists, deletion clearing and query filters work as they do for lookups.
-- A related-to field has no `reference_object_id`; its allowlist is a child
-- table of 1 to 10 objects, in the order the administrator gave them.
ALTER TABLE field_definitions DROP CONSTRAINT field_definitions_field_type_check;
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_field_type_check CHECK (field_type IN (
    'text', 'long_text', 'email', 'phone', 'url', 'number', 'currency', 'percent',
    'boolean', 'date', 'date_time', 'picklist', 'multi_picklist', 'lookup', 'formula',
    'master_detail', 'rollup', 'related_to'));
-- Deleting a target always clears a related-to value; it is never unique
-- and has no default.
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_related_to_check
    CHECK (field_type <> 'related_to'
        OR (delete_behaviour = 'clear' AND NOT unique_value AND default_value IS NULL));

CREATE TABLE field_reference_objects (
    tenant_id uuid NOT NULL,
    field_id uuid NOT NULL,
    object_id uuid NOT NULL,
    position integer NOT NULL CHECK (position BETWEEN 0 AND 9),
    PRIMARY KEY (tenant_id, field_id, object_id),
    UNIQUE (tenant_id, field_id, position),
    FOREIGN KEY (tenant_id, field_id) REFERENCES field_definitions(tenant_id, id),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id)
);

-- A target object's related lists and its deletion find the related-to
-- fields that allow it.
CREATE INDEX field_reference_objects_object_idx
    ON field_reference_objects (tenant_id, object_id, field_id);

ALTER TABLE field_reference_objects ENABLE ROW LEVEL SECURITY;
ALTER TABLE field_reference_objects FORCE ROW LEVEL SECURITY;
CREATE POLICY field_reference_objects_tenant_isolation ON field_reference_objects TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY field_reference_objects_migrator_management ON field_reference_objects TO platform_migrator
    USING (true) WITH CHECK (true);
