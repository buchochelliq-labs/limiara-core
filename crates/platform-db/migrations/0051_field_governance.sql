-- Field governance metadata (ADR 0035 G9): what a field holds and why.
-- A field may declare a data classification, the purpose of processing
-- it, a retention period and whether it is stored encrypted, with the
-- rules Contacts applies to its custom fields, which the engine checks.
-- They are declarations only: nothing enforces retention or encryption
-- yet.
--
-- One JSON document on the existing tenant table, whose forced row-level
-- security and policies stay as they are; NULL means no declarations.
ALTER TABLE field_definitions
    ADD COLUMN governance jsonb
        CHECK (governance IS NULL OR jsonb_typeof(governance) = 'object');
