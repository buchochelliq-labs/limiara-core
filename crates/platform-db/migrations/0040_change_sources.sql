-- Change sources (#233, ADR 0017 "Change source"): where each history
-- entry's change came from (the web app, an API client, a synced device,
-- an automation, a quick action, a device assistant, a bulk job or the
-- platform), and its detail: the verified client or device id, the
-- automation or quick action's API name, the assistant's name or the bulk
-- job's id. Never free text. Entries written before this migration keep
-- both columns null and read as unknown; there is no backfill.
--
-- The columns live on the existing tenant table, so its forced row-level
-- security, its policies and its append-only grants stay as they are.
-- Audit entries keep the source in their metadata document instead.
ALTER TABLE record_history
    ADD COLUMN source_kind text CHECK (source_kind IN ('web', 'api_client', 'device',
        'automation', 'quick_action', 'assistant', 'import', 'system')),
    ADD COLUMN source_detail text CHECK (source_detail ~ '^[A-Za-z0-9_.:-]{1,64}$'),
    ADD CONSTRAINT record_history_source_detail_kind
        CHECK (source_detail IS NULL OR source_kind IS NOT NULL);
