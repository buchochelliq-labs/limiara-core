-- The path field of an object (ADR 0037 §2): the single-value picklist the
-- record page shows as a path and the board groups by, by API name. Option
-- outcomes and weights (ADR 0037 §1) live in the options jsonb and need no
-- column.
ALTER TABLE object_definitions ADD COLUMN path_field text
    CHECK (path_field IS NULL OR length(path_field) BETWEEN 1 AND 255);
