-- The fields a record name is composed from (ADR 0035 G17): two or three of
-- the object's own text fields and a separator, as JSON. When it is set the
-- engine writes every record's name itself.
ALTER TABLE object_definitions ADD COLUMN name_from jsonb
    CHECK (name_from IS NULL OR jsonb_typeof(name_from) = 'object');
