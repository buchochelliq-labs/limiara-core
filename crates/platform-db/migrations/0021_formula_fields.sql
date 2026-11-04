-- Formula fields (ADR 0021, #298): computed when records are read, never
-- stored. The definition keeps the formula and the type it gives.
ALTER TABLE field_definitions DROP CONSTRAINT field_definitions_field_type_check;
ALTER TABLE field_definitions ADD CONSTRAINT field_definitions_field_type_check CHECK (field_type IN (
    'text', 'long_text', 'email', 'phone', 'url', 'number', 'currency', 'percent',
    'boolean', 'date', 'date_time', 'picklist', 'multi_picklist', 'lookup', 'formula'));
ALTER TABLE field_definitions
    ADD COLUMN formula text CHECK (formula IS NULL OR char_length(formula) BETWEEN 1 AND 3900),
    ADD COLUMN formula_type text CHECK (formula_type IS NULL OR formula_type IN (
        'text', 'number', 'currency', 'percent', 'boolean', 'date', 'date_time')),
    ADD CONSTRAINT field_definitions_formula_shape_check
        CHECK ((field_type = 'formula') = (formula IS NOT NULL AND formula_type IS NOT NULL));
