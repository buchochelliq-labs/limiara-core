-- Customising one object (ADR 0035 G10): a role may be granted
-- `customise` on an object, which lets its members add, change and retire
-- that object's workspace fields, validation rules, quick actions and
-- automations, without `objects.manage`, which covers every object.
-- It grants nothing on records, and no record grant implies it.
--
-- The column lives on the existing tenant table, so its forced row-level
-- security and policies stay as they are.
ALTER TABLE object_permissions
    ADD COLUMN customise boolean NOT NULL DEFAULT false;
