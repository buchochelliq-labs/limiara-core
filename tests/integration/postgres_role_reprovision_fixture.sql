\set ON_ERROR_STOP on

-- Model an intentional plugin capability granted after the one-time legacy
-- ACL cleanup. Recurring password rotation must preserve this explicit grant.
SET ROLE platform_migrator;
CREATE SCHEMA p_role_probe;
CREATE FUNCTION p_role_probe.intentional_runtime_function()
RETURNS integer
LANGUAGE sql
AS 'SELECT 1';
CREATE TABLE p_role_probe.append_only_records (
  id integer PRIMARY KEY,
  value text NOT NULL
);
REVOKE ALL ON FUNCTION p_role_probe.intentional_runtime_function() FROM PUBLIC;
GRANT USAGE ON SCHEMA p_role_probe TO platform_app;
GRANT EXECUTE ON FUNCTION p_role_probe.intentional_runtime_function() TO platform_app;
GRANT SELECT, INSERT ON TABLE p_role_probe.append_only_records TO platform_app;
REVOKE UPDATE, DELETE ON TABLE p_role_probe.append_only_records FROM platform_app;
RESET ROLE;
