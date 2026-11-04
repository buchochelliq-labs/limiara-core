\set ON_ERROR_STOP on

DO $$
DECLARE
  role_name text;
BEGIN
  FOREACH role_name IN ARRAY ARRAY['platform_migrator', 'platform_app', 'platform_worker'] LOOP
    IF NOT EXISTS (
      SELECT 1
      FROM pg_catalog.pg_roles
      WHERE rolname = role_name
        AND rolcanlogin
        AND NOT rolsuper
        AND NOT rolcreatedb
        AND NOT rolcreaterole
        AND NOT rolinherit
        AND NOT rolreplication
        AND NOT rolbypassrls
    ) THEN
      RAISE EXCEPTION '% does not satisfy the role boundary', role_name;
    END IF;
  END LOOP;

  IF NOT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_database database
    JOIN pg_catalog.pg_roles owner ON owner.oid = database.datdba
    WHERE database.datname = 'platform' AND owner.rolname = 'platform_migrator'
  ) THEN
    RAISE EXCEPTION 'platform_migrator does not own the platform database';
  END IF;

  IF NOT has_database_privilege('platform_worker', 'platform', 'CONNECT') THEN
    RAISE EXCEPTION 'platform_worker cannot connect to the platform database';
  END IF;
END
$$;

\connect platform platform_migrator

DROP TABLE IF EXISTS public.role_boundary_probe;
CREATE TABLE public.role_boundary_probe (id integer PRIMARY KEY, value text NOT NULL);

DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_class relation
    JOIN pg_catalog.pg_roles owner ON owner.oid = relation.relowner
    WHERE relation.oid = 'public.role_boundary_probe'::regclass
      AND owner.rolname = 'platform_migrator'
  ) THEN
    RAISE EXCEPTION 'migration objects are not owned by platform_migrator';
  END IF;
END
$$;

INSERT INTO tenants (id, display_name) VALUES
  ('01910000-0000-7000-8000-000000000001', 'Worker probe tenant A'),
  ('01910000-0000-7000-8000-000000000002', 'Worker probe tenant B');
INSERT INTO outbox (id, tenant_id, event_type, envelope) VALUES
  (
    '01910000-0000-7000-8000-000000000100',
    '01910000-0000-7000-8000-000000000001',
    'test.worker.duplicate-id.v1',
    '{"tenant":"a"}'::jsonb
  ),
  (
    '01910000-0000-7000-8000-000000000100',
    '01910000-0000-7000-8000-000000000002',
    'test.worker.duplicate-id.v1',
    '{"tenant":"b"}'::jsonb
  );

\connect platform platform_app

DO $$
BEGIN
  IF has_database_privilege(current_user, current_database(), 'CREATE') THEN
    RAISE EXCEPTION 'platform_app can create database schemas';
  END IF;
  IF has_schema_privilege(current_user, 'public', 'CREATE') THEN
    RAISE EXCEPTION 'platform_app can create objects in public';
  END IF;
  IF NOT has_function_privilege(
    current_user,
    'p_role_probe.intentional_runtime_function()'::regprocedure,
    'EXECUTE'
  ) OR p_role_probe.intentional_runtime_function() IS DISTINCT FROM 1 THEN
    RAISE EXCEPTION 'role reprovision removed an intentional plugin capability';
  END IF;
  IF NOT has_table_privilege(current_user, 'p_role_probe.append_only_records', 'SELECT')
     OR NOT has_table_privilege(current_user, 'p_role_probe.append_only_records', 'INSERT')
     OR has_table_privilege(current_user, 'p_role_probe.append_only_records', 'UPDATE')
     OR has_table_privilege(current_user, 'p_role_probe.append_only_records', 'DELETE') THEN
    RAISE EXCEPTION 'role reprovision changed an intentional plugin table ACL';
  END IF;

  BEGIN
    EXECUTE 'CREATE TABLE public.platform_app_forbidden (id integer)';
    RAISE EXCEPTION 'platform_app unexpectedly created a table';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    EXECUTE 'ALTER TABLE public.role_boundary_probe ADD COLUMN forbidden integer';
    RAISE EXCEPTION 'platform_app unexpectedly altered a migration-owned table';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    EXECUTE 'CREATE ROLE platform_app_forbidden';
    RAISE EXCEPTION 'platform_app unexpectedly created a role';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;
END
$$;

INSERT INTO p_role_probe.append_only_records (id, value) VALUES (1, 'created');
DO $$
BEGIN
  BEGIN
    UPDATE p_role_probe.append_only_records SET value = 'forbidden' WHERE id = 1;
    RAISE EXCEPTION 'platform_app unexpectedly updated a plugin append-only table';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    DELETE FROM p_role_probe.append_only_records WHERE id = 1;
    RAISE EXCEPTION 'platform_app unexpectedly deleted from a plugin append-only table';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;
END
$$;

INSERT INTO public.role_boundary_probe (id, value) VALUES (1, 'created');
UPDATE public.role_boundary_probe SET value = 'updated' WHERE id = 1;
DO $$
BEGIN
  IF (SELECT value FROM public.role_boundary_probe WHERE id = 1) IS DISTINCT FROM 'updated' THEN
    RAISE EXCEPTION 'platform_app cannot read its DML result';
  END IF;
END
$$;
DELETE FROM public.role_boundary_probe WHERE id = 1;

DO $$
DECLARE
  worker_function regprocedure;
BEGIN
  IF has_table_privilege(current_user, 'audit_log', 'UPDATE')
     OR has_table_privilege(current_user, 'audit_log', 'DELETE')
     OR has_table_privilege(current_user, 'outbox', 'UPDATE')
     OR has_table_privilege(current_user, 'outbox', 'DELETE') THEN
    RAISE EXCEPTION 'platform_app can rewrite append-only audit or outbox records';
  END IF;

  IF has_table_privilege(current_user, 'plugin_job_schedules', 'INSERT')
     OR has_table_privilege(current_user, 'plugin_job_schedules', 'UPDATE')
     OR has_table_privilege(current_user, 'plugin_job_schedules', 'DELETE') THEN
    RAISE EXCEPTION 'platform_app can change plugin job schedules outside lease routines';
  END IF;

  IF has_column_privilege(current_user, 'plugin_job_schedules', 'claim_token', 'SELECT') THEN
    RAISE EXCEPTION 'platform_app can read plugin job claim tokens';
  END IF;

  IF NOT has_function_privilege(current_user, 'platform_complete_plugin_job(text,text,uuid,jsonb)', 'EXECUTE') THEN
    RAISE EXCEPTION 'platform_app cannot settle a leased plugin job in its tenant transaction';
  END IF;

  FOREACH worker_function IN ARRAY ARRAY[
    'platform_recover_plugin_transitions()'::regprocedure,
    'platform_claim_outbox_batch(bigint,uuid,bigint)'::regprocedure,
    'platform_mark_outbox_published(uuid,uuid,uuid)'::regprocedure,
    'platform_mark_outbox_failed(uuid,uuid,uuid,bigint,text)'::regprocedure,
    'platform_mark_outbox_quarantined(uuid,uuid,uuid,text)'::regprocedure,
    'platform_outbox_backlog()'::regprocedure,
    'platform_ensure_plugin_job_schedules(text,text,integer)'::regprocedure,
    'platform_claim_plugin_jobs(text,text,bigint,uuid,bigint)'::regprocedure,
    'platform_fail_plugin_job(uuid,text,text,uuid,bigint,text)'::regprocedure,
    'platform_release_plugin_job(uuid,text,text,uuid,bigint)'::regprocedure
  ] LOOP
    IF has_function_privilege(current_user, worker_function, 'EXECUTE') THEN
      RAISE EXCEPTION 'platform_app can execute worker function %', worker_function;
    END IF;
  END LOOP;
END
$$;

\connect platform platform_worker

DO $$
DECLARE
  protected_table oid;
  protected_tables oid[];
  worker_function regprocedure;
BEGIN
  SELECT array_agg(relation.oid ORDER BY namespace.nspname, relation.relname)
  INTO protected_tables
  FROM pg_catalog.pg_class relation
  JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace
  WHERE (namespace.nspname, relation.relname) IN (
    ('public', 'tenant_plugin_installations'),
    ('public', 'audit_log'),
    ('public', 'inbox'),
    ('public', 'outbox'),
    ('public', 'plugin_job_schedules'),
    ('p_contacts', 'people'),
    ('p_contacts', 'organisations'),
    ('p_contacts', 'custom_field_definitions'),
    ('p_contacts', 'list_cursors'),
    ('p_contacts', 'activity')
  );

  IF coalesce(array_length(protected_tables, 1), 0) IS DISTINCT FROM 10 THEN
    RAISE EXCEPTION 'protected table assertion did not resolve every expected table';
  END IF;

  FOREACH protected_table IN ARRAY protected_tables LOOP
    IF has_table_privilege(current_user, protected_table, 'SELECT')
       OR has_table_privilege(current_user, protected_table, 'INSERT')
       OR has_table_privilege(current_user, protected_table, 'UPDATE')
       OR has_table_privilege(current_user, protected_table, 'DELETE') THEN
      RAISE EXCEPTION 'platform_worker has direct access to protected table %', protected_table;
    END IF;
  END LOOP;

  IF has_function_privilege(current_user, 'platform_complete_plugin_job(text,text,uuid,jsonb)', 'EXECUTE') THEN
    RAISE EXCEPTION 'platform_worker can settle tenant job effects outside a tenant transaction';
  END IF;

  IF NOT has_table_privilege(current_user, 'plugin_migration_ledger', 'SELECT') THEN
    RAISE EXCEPTION 'platform_worker cannot verify the migration sentinel';
  END IF;

  FOREACH worker_function IN ARRAY ARRAY[
    'platform_recover_plugin_transitions()'::regprocedure,
    'platform_claim_outbox_batch(bigint,uuid,bigint)'::regprocedure,
    'platform_mark_outbox_published(uuid,uuid,uuid)'::regprocedure,
    'platform_mark_outbox_failed(uuid,uuid,uuid,bigint,text)'::regprocedure,
    'platform_mark_outbox_quarantined(uuid,uuid,uuid,text)'::regprocedure,
    'platform_outbox_backlog()'::regprocedure,
    'platform_ensure_plugin_job_schedules(text,text,integer)'::regprocedure,
    'platform_claim_plugin_jobs(text,text,bigint,uuid,bigint)'::regprocedure,
    'platform_fail_plugin_job(uuid,text,text,uuid,bigint,text)'::regprocedure,
    'platform_release_plugin_job(uuid,text,text,uuid,bigint)'::regprocedure
  ] LOOP
    IF NOT has_function_privilege(current_user, worker_function, 'EXECUTE') THEN
      RAISE EXCEPTION 'platform_worker cannot execute worker function %', worker_function;
    END IF;
  END LOOP;
END
$$;

DO $$
BEGIN
  IF platform_recover_plugin_transitions() IS DISTINCT FROM 0 THEN
    RAISE EXCEPTION 'empty worker recovery probe unexpectedly changed plugin state';
  END IF;
END
$$;

DO $$
DECLARE
  claimed bigint;
BEGIN
  SELECT count(*)
  INTO claimed
  FROM platform_claim_outbox_batch(
    2,
    '01910000-0000-7000-8000-000000000200',
    60000
  );
  IF claimed IS DISTINCT FROM 2 THEN
    RAISE EXCEPTION 'worker did not claim both tenant-scoped duplicate IDs';
  END IF;

  IF platform_mark_outbox_published(
    '01910000-0000-7000-8000-000000000001',
    '01910000-0000-7000-8000-000000000100',
    '01910000-0000-7000-8000-000000000200'
  ) IS DISTINCT FROM true THEN
    RAISE EXCEPTION 'worker could not settle the targeted tenant row';
  END IF;
END
$$;

\connect platform platform_migrator

DO $$
BEGIN
  IF (
    SELECT published_at IS NOT NULL AND claim_token IS NULL
    FROM outbox
    WHERE tenant_id = '01910000-0000-7000-8000-000000000001'
      AND id = '01910000-0000-7000-8000-000000000100'
  ) IS DISTINCT FROM true
     OR (
       SELECT published_at IS NULL
              AND claim_token = '01910000-0000-7000-8000-000000000200'
       FROM outbox
       WHERE tenant_id = '01910000-0000-7000-8000-000000000002'
         AND id = '01910000-0000-7000-8000-000000000100'
     ) IS DISTINCT FROM true THEN
    RAISE EXCEPTION 'tenant-qualified settlement changed the non-target outbox row';
  END IF;
END
$$;

DO $$
DECLARE
  bounded_function regprocedure;
BEGIN
  FOREACH bounded_function IN ARRAY ARRAY[
    'platform_append_global_plugin_audit(uuid,text,text,jsonb)'::regprocedure,
    'platform_recover_plugin_transitions()'::regprocedure,
    'platform_claim_outbox_batch(bigint,uuid,bigint)'::regprocedure,
    'platform_mark_outbox_published(uuid,uuid,uuid)'::regprocedure,
    'platform_mark_outbox_failed(uuid,uuid,uuid,bigint,text)'::regprocedure,
    'platform_mark_outbox_quarantined(uuid,uuid,uuid,text)'::regprocedure,
    'platform_outbox_backlog()'::regprocedure
  ] LOOP
    IF NOT EXISTS (
      SELECT 1
      FROM pg_catalog.pg_proc procedure
      WHERE procedure.oid = bounded_function
        AND procedure.prosecdef
        AND procedure.proconfig @> ARRAY['search_path=pg_catalog, public']
    ) THEN
      RAISE EXCEPTION 'function % is not a pinned SECURITY DEFINER', bounded_function;
    END IF;

    IF EXISTS (
      SELECT 1
      FROM pg_catalog.pg_proc procedure
      CROSS JOIN LATERAL aclexplode(
        COALESCE(procedure.proacl, acldefault('f', procedure.proowner))
      ) AS privilege
      WHERE procedure.oid = bounded_function
        AND privilege.grantee = 0
        AND privilege.privilege_type = 'EXECUTE'
    ) THEN
      RAISE EXCEPTION 'PUBLIC can execute function %', bounded_function;
    END IF;
  END LOOP;
END
$$;

DROP TABLE public.role_boundary_probe;
DROP SCHEMA p_role_probe CASCADE;
DELETE FROM outbox
WHERE tenant_id IN (
  '01910000-0000-7000-8000-000000000001',
  '01910000-0000-7000-8000-000000000002'
)
  AND id = '01910000-0000-7000-8000-000000000100';
DELETE FROM tenants
WHERE id IN (
  '01910000-0000-7000-8000-000000000001',
  '01910000-0000-7000-8000-000000000002'
);
