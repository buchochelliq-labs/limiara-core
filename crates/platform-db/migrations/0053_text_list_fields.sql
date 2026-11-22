-- Text list fields (ADR 0035 G7): free-form tags, up to 50 distinct values
-- of up to 80 characters each, compared without case. Values are stored as
-- a JSON array on the record, like a multi-select picklist's, and indexed
-- per value through the existing record index, so no new table is needed.
-- A text list is never unique.
ALTER TABLE field_definitions DROP CONSTRAINT field_definitions_field_type_check;
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_field_type_check CHECK (field_type IN (
    'text', 'long_text', 'email', 'phone', 'url', 'number', 'currency', 'percent',
    'boolean', 'date', 'date_time', 'picklist', 'multi_picklist', 'lookup', 'formula',
    'master_detail', 'rollup', 'related_to', 'text_list'));
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_text_list_check
    CHECK (field_type <> 'text_list' OR NOT unique_value);
