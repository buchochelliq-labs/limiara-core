-- Field-level permissions (G5 of ADR 0035, #316): a field may name the
-- permission a member needs to read its values and the one they need, on
-- top of that, to change them. Both are permission ids from the RBAC
-- catalog; the engine checks them for every read and write as a member.
--
-- The columns live on the existing tenant table, so its forced row-level
-- security and its policies stay as they are. Formula and roll-up summary
-- fields are never written, so they take a read permission only.
ALTER TABLE field_definitions
    ADD COLUMN read_permission text
        CHECK (read_permission ~ '^[a-z][a-z0-9_-]*(\.[a-z0-9_-]+)+$'
            AND length(read_permission) <= 120),
    ADD COLUMN write_permission text
        CHECK (write_permission ~ '^[a-z][a-z0-9_-]*(\.[a-z0-9_-]+)+$'
            AND length(write_permission) <= 120),
    ADD CONSTRAINT field_definitions_write_permission_shape_check CHECK (
        write_permission IS NULL OR field_type NOT IN ('formula', 'rollup'));
