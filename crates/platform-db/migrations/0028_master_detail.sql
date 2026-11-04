-- Master-detail relationships (ADR 0025, #295). A master-detail field is a
-- required lookup to the record's master: the master controls the detail's
-- sharing, and deleting or restoring the master deletes or restores its
-- details with it. `deleted_with` names the master whose deletion took a
-- detail to the recycle bin, so restoring that master brings it back.
ALTER TABLE field_definitions DROP CONSTRAINT field_definitions_field_type_check;
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_field_type_check CHECK (field_type IN (
    'text', 'long_text', 'email', 'phone', 'url', 'number', 'currency', 'percent',
    'boolean', 'date', 'date_time', 'picklist', 'multi_picklist', 'lookup', 'formula',
    'master_detail'));
ALTER TABLE field_definitions DROP CONSTRAINT field_definitions_check;
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_reference_check
    CHECK ((field_type IN ('lookup', 'master_detail')) = (reference_object_id IS NOT NULL));
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_master_detail_check
    CHECK (field_type <> 'master_detail' OR (required AND delete_behaviour IS NULL));

ALTER TABLE records ADD COLUMN deleted_with uuid;
CREATE INDEX records_deleted_with_idx ON records (tenant_id, deleted_with)
    WHERE deleted_with IS NOT NULL;
