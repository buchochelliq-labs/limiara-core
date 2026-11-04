-- Lookup filters (ADR 0028, #295): a lookup or master-detail field may keep
-- a query-language condition on its target object that new values must
-- meet. field_definitions is already tenant-scoped with forced RLS.
ALTER TABLE field_definitions
    ADD COLUMN lookup_filter text
        CHECK (lookup_filter IS NULL OR length(lookup_filter) BETWEEN 1 AND 1000),
    ADD CONSTRAINT field_definitions_lookup_filter_shape_check CHECK (
        lookup_filter IS NULL OR field_type IN ('lookup', 'master_detail'));
