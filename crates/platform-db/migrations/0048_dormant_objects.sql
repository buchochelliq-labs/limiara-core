-- Plugin objects kept out of service until their plugin wakes them (ADR
-- 0035 G11). A dormant object is installed suspended and stays suspended
-- when its plugin is enabled; only the plugin's own wake brings it into
-- service, after which it behaves like any plugin object.
ALTER TABLE object_definitions ADD COLUMN dormant boolean NOT NULL DEFAULT false;
