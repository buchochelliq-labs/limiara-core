-- Reports a plugin declares (ADR 0037 §5). A plugin report names its plugin
-- and its name in the manifest, and the version it had when the plugin last
-- installed it: an upgrade changes only a report still at that version,
-- which nobody has changed since. While the plugin is not enabled its
-- reports are suspended and hidden.
ALTER TABLE reports
    ADD COLUMN owner_plugin text CHECK (owner_plugin IS NULL OR char_length(owner_plugin) BETWEEN 1 AND 255),
    ADD COLUMN plugin_report text CHECK (plugin_report IS NULL OR char_length(plugin_report) BETWEEN 1 AND 63),
    ADD COLUMN plugin_version bigint CHECK (plugin_version IS NULL OR plugin_version >= 1),
    ADD COLUMN suspended boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT reports_plugin_shape CHECK (
        (owner_plugin IS NULL AND plugin_report IS NULL AND plugin_version IS NULL)
        OR (owner_plugin IS NOT NULL AND plugin_report IS NOT NULL AND plugin_version IS NOT NULL)
    );
CREATE UNIQUE INDEX reports_plugin_report_idx ON reports (tenant_id, owner_plugin, plugin_report)
    WHERE owner_plugin IS NOT NULL;

-- Plugin reports a workspace deleted: the plugin never adds them again.
CREATE TABLE plugin_report_removals (
    tenant_id uuid NOT NULL,
    plugin_id text NOT NULL CHECK (char_length(plugin_id) BETWEEN 1 AND 255),
    report text NOT NULL CHECK (char_length(report) BETWEEN 1 AND 63),
    removed_at timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, plugin_id, report)
);

ALTER TABLE plugin_report_removals ENABLE ROW LEVEL SECURITY;
ALTER TABLE plugin_report_removals FORCE ROW LEVEL SECURITY;
CREATE POLICY plugin_report_removals_tenant_isolation ON plugin_report_removals TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY plugin_report_removals_migrator_management ON plugin_report_removals TO platform_migrator
    USING (true) WITH CHECK (true);
