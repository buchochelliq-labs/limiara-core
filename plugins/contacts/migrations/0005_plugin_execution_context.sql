-- Contacts tables are usable only inside the Contacts plugin execution
-- context (see core migration 0012_plugin_execution_context.sql). These
-- RESTRICTIVE policies are ANDed with the existing tenant policies, so a
-- request must carry both the right workspace and the Contacts context; other
-- plugins and kernel code see no rows and cannot write.
CREATE POLICY organisations_plugin_context ON p_contacts.organisations
  AS RESTRICTIVE TO platform_app
  USING (current_setting('app.plugin_id', true) = 'platform.contacts')
  WITH CHECK (current_setting('app.plugin_id', true) = 'platform.contacts');
CREATE POLICY people_plugin_context ON p_contacts.people
  AS RESTRICTIVE TO platform_app
  USING (current_setting('app.plugin_id', true) = 'platform.contacts')
  WITH CHECK (current_setting('app.plugin_id', true) = 'platform.contacts');
CREATE POLICY custom_field_definitions_plugin_context ON p_contacts.custom_field_definitions
  AS RESTRICTIVE TO platform_app
  USING (current_setting('app.plugin_id', true) = 'platform.contacts')
  WITH CHECK (current_setting('app.plugin_id', true) = 'platform.contacts');
CREATE POLICY public_id_counters_plugin_context ON p_contacts.public_id_counters
  AS RESTRICTIVE TO platform_app
  USING (current_setting('app.plugin_id', true) = 'platform.contacts')
  WITH CHECK (current_setting('app.plugin_id', true) = 'platform.contacts');
CREATE POLICY list_cursors_plugin_context ON p_contacts.list_cursors
  AS RESTRICTIVE TO platform_app
  USING (current_setting('app.plugin_id', true) = 'platform.contacts')
  WITH CHECK (current_setting('app.plugin_id', true) = 'platform.contacts');
CREATE POLICY activity_plugin_context ON p_contacts.activity
  AS RESTRICTIVE TO platform_app
  USING (current_setting('app.plugin_id', true) = 'platform.contacts')
  WITH CHECK (current_setting('app.plugin_id', true) = 'platform.contacts');
