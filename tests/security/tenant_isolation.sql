\set ON_ERROR_STOP on

-- This file is intentionally self-guarding. A green result from an owner or a
-- privileged role would not prove that forced RLS protects the runtime role.
BEGIN;
DO $$
DECLARE
  role_is_superuser boolean;
  role_bypasses_rls boolean;
  session_is_superuser boolean;
  session_bypasses_rls boolean;
BEGIN
  SELECT rolsuper, rolbypassrls
  INTO role_is_superuser, role_bypasses_rls
  FROM pg_catalog.pg_roles
  WHERE rolname = current_user;

  SELECT rolsuper, rolbypassrls
  INTO session_is_superuser, session_bypasses_rls
  FROM pg_catalog.pg_roles
  WHERE rolname = session_user;

  -- PostgreSQL superusers also bypass RLS, so report that stronger privilege
  -- first; the disposable BYPASSRLS probe connects directly as a non-superuser.
  IF role_is_superuser THEN
    RAISE EXCEPTION 'RLS harness refuses superuser sessions';
  END IF;
  IF role_bypasses_rls THEN
    RAISE EXCEPTION 'RLS harness refuses BYPASSRLS roles';
  END IF;
  IF session_is_superuser THEN
    RAISE EXCEPTION 'RLS harness refuses superuser sessions';
  END IF;
  IF session_bypasses_rls THEN
    RAISE EXCEPTION 'RLS harness refuses BYPASSRLS roles';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM pg_catalog.pg_class relation
    JOIN pg_catalog.pg_roles owner ON owner.oid = relation.relowner
    WHERE relation.oid = ANY (ARRAY[
      'p_contacts.people'::regclass,
      'p_contacts.organisations'::regclass,
      'p_contacts.custom_field_definitions'::regclass,
      'p_contacts.list_cursors'::regclass,
      'tenant_plugin_installations'::regclass,
      'audit_log'::regclass,
      'inbox'::regclass,
      'outbox'::regclass
    ])
      AND owner.rolname IN (current_user, session_user)
  ) THEN
    RAISE EXCEPTION 'RLS harness refuses table-owner sessions';
  END IF;
  IF current_user <> 'platform_app' OR session_user <> 'platform_app' THEN
    RAISE EXCEPTION 'RLS harness requires a direct platform_app session';
  END IF;
END
$$;

-- Plugin rows below run in the Contacts plugin execution context, which the
-- restrictive plugin-context policies require in addition to the tenant.
SELECT set_config('app.plugin_id', 'platform.contacts', true);

DO $$
DECLARE
  protected_table regclass;
  worker_function regprocedure;
BEGIN
  FOREACH protected_table IN ARRAY ARRAY[
    'p_contacts.people'::regclass,
    'p_contacts.organisations'::regclass,
    'p_contacts.custom_field_definitions'::regclass,
    'p_contacts.list_cursors'::regclass,
    'p_contacts.activity'::regclass,
    'tenant_plugin_installations'::regclass,
    'audit_log'::regclass,
    'inbox'::regclass,
    'outbox'::regclass,
    'plugin_job_schedules'::regclass,
    'memberships'::regclass,
    'roles'::regclass,
    'role_permissions'::regclass,
    'user_roles'::regclass,
    'idempotency_keys'::regclass,
    'entitlement_snapshots'::regclass,
    'deployment_placements'::regclass
  ] LOOP
    IF NOT EXISTS (
      SELECT 1
      FROM pg_catalog.pg_class relation
      WHERE relation.oid = protected_table
        AND relation.relrowsecurity
        AND relation.relforcerowsecurity
    ) THEN
      RAISE EXCEPTION 'protected table % does not force RLS', protected_table;
    END IF;
  END LOOP;

  IF has_table_privilege(current_user, 'audit_log', 'UPDATE')
     OR has_table_privilege(current_user, 'audit_log', 'DELETE')
     OR has_table_privilege(current_user, 'inbox', 'UPDATE')
     OR has_table_privilege(current_user, 'inbox', 'DELETE')
     OR has_table_privilege(current_user, 'outbox', 'UPDATE')
     OR has_table_privilege(current_user, 'outbox', 'DELETE')
     OR has_table_privilege(current_user, 'p_contacts.activity', 'UPDATE')
     OR has_table_privilege(current_user, 'p_contacts.activity', 'DELETE') THEN
    RAISE EXCEPTION 'platform_app can rewrite an append-only ledger';
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
      RAISE EXCEPTION 'platform_app can execute cross-tenant worker function %', worker_function;
    END IF;
  END LOOP;
END
$$;

INSERT INTO plugin_catalogue (plugin_id, manifest, lifecycle_state, latest_usable_version)
VALUES (
  'test.rls',
  '{"name":"RLS fixture"}'::jsonb,
  'health_checked',
  '1.0.0'
)
ON CONFLICT (plugin_id) DO NOTHING;

INSERT INTO plugin_versions (
  plugin_id, version, manifest, manifest_path, declared_digest,
  verified_digest, lifecycle_state
)
VALUES (
  'test.rls',
  '1.0.0',
  '{"name":"RLS fixture"}'::jsonb,
  'tests/security/tenant_isolation.sql',
  'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  'health_checked'
)
ON CONFLICT (plugin_id, version) DO NOTHING;

-- Seed deterministic records through exactly the context-setting mechanism
-- used by the application transaction helper.
SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000001', true);
INSERT INTO p_contacts.organisations (tenant_id, id, public_id, name)
VALUES (
  '018f0000-0000-7000-8000-000000000001',
  '018f1000-0000-7000-8000-000000000001',
  'org000000000001abc',
  'Workspace A Org'
);
INSERT INTO p_contacts.people (tenant_id, id, public_id, full_name, email)
VALUES (
  '018f0000-0000-7000-8000-000000000001',
  '018f2000-0000-7000-8000-000000000001',
  'per000000000001abc',
  'Workspace A Person',
  'workspace-a@example.test'
);
INSERT INTO p_contacts.custom_field_definitions (
  tenant_id, id, applies_to, api_name, label, field_type, data_classification
) VALUES (
  '018f0000-0000-7000-8000-000000000001',
  '018f3000-0000-7000-8000-000000000001',
  'person',
  'private__c',
  'Workspace A Private',
  'text',
  'personal'
);
INSERT INTO p_contacts.list_cursors (
  token_hash, tenant_id, resource, query_hash, sort_value, record_id, expires_at
) VALUES (
  repeat('a', 64),
  '018f0000-0000-7000-8000-000000000001',
  'people',
  repeat('b', 64),
  'workspace-a-sort-position',
  '018f2000-0000-7000-8000-000000000001',
  now() + interval '1 hour'
);
INSERT INTO tenant_plugin_installations (
  tenant_id, plugin_id, state, active_version, enabled_at
) VALUES (
  '018f0000-0000-7000-8000-000000000001',
  'test.rls',
  'enabled',
  '1.0.0',
  now()
);
INSERT INTO audit_log (id, tenant_id, action, target, metadata)
VALUES (
  '018f4000-0000-7000-8000-000000000001',
  '018f0000-0000-7000-8000-000000000001',
  'test.rls.seeded',
  'person:per000000000001abc',
  '{"workspace":"a"}'::jsonb
);
INSERT INTO inbox (tenant_id, consumer_id, event_id)
VALUES (
  '018f0000-0000-7000-8000-000000000001',
  'platform.contacts.rls-tenant-isolation',
  '018f5000-0000-7000-8000-000000000001'
);
INSERT INTO outbox (id, tenant_id, event_type, envelope)
VALUES (
  '018f6000-0000-7000-8000-000000000001',
  '018f0000-0000-7000-8000-000000000001',
  'platform.contacts.rls-seeded.v1',
  '{"workspace":"a","source_plugin":"platform.contacts"}'::jsonb
);

SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000002', true);
INSERT INTO p_contacts.organisations (tenant_id, id, public_id, name)
VALUES (
  '018f0000-0000-7000-8000-000000000002',
  '018f1000-0000-7000-8000-000000000002',
  'org000000000002abc',
  'Workspace B Org'
);
INSERT INTO p_contacts.people (tenant_id, id, public_id, full_name, email)
VALUES (
  '018f0000-0000-7000-8000-000000000002',
  '018f2000-0000-7000-8000-000000000002',
  'per000000000002abc',
  'Workspace B Person',
  'workspace-b@example.test'
);
INSERT INTO p_contacts.custom_field_definitions (
  tenant_id, id, applies_to, api_name, label, field_type, data_classification
) VALUES (
  '018f0000-0000-7000-8000-000000000002',
  '018f3000-0000-7000-8000-000000000002',
  'person',
  'private__c',
  'Workspace B Private',
  'text',
  'personal'
);
INSERT INTO p_contacts.list_cursors (
  token_hash, tenant_id, resource, query_hash, sort_value, record_id, expires_at
) VALUES (
  repeat('c', 64),
  '018f0000-0000-7000-8000-000000000002',
  'people',
  repeat('d', 64),
  'workspace-b-secret-position',
  '018f2000-0000-7000-8000-000000000002',
  now() + interval '1 hour'
);
INSERT INTO tenant_plugin_installations (
  tenant_id, plugin_id, state, active_version, enabled_at
) VALUES (
  '018f0000-0000-7000-8000-000000000002',
  'test.rls',
  'enabled',
  '1.0.0',
  now()
);
INSERT INTO audit_log (id, tenant_id, action, target, metadata)
VALUES (
  '018f4000-0000-7000-8000-000000000002',
  '018f0000-0000-7000-8000-000000000002',
  'test.rls.seeded',
  'person:per000000000002abc',
  '{"workspace":"b"}'::jsonb
);
INSERT INTO inbox (tenant_id, consumer_id, event_id)
VALUES (
  '018f0000-0000-7000-8000-000000000002',
  'platform.contacts.rls-tenant-isolation',
  '018f5000-0000-7000-8000-000000000002'
);
INSERT INTO outbox (id, tenant_id, event_type, envelope)
VALUES (
  '018f6000-0000-7000-8000-000000000002',
  '018f0000-0000-7000-8000-000000000002',
  'platform.contacts.rls-seeded.v1',
  '{"workspace":"b","source_plugin":"platform.contacts"}'::jsonb
);

-- Workspace A cannot enumerate or read known Workspace B identifiers.
SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000001', true);
DO $$
BEGIN
  IF EXISTS (
      SELECT 1 FROM p_contacts.people
      WHERE id = '018f2000-0000-7000-8000-000000000002'
    )
    OR EXISTS (
      SELECT 1 FROM p_contacts.organisations
      WHERE id = '018f1000-0000-7000-8000-000000000002'
    )
    OR EXISTS (
      SELECT 1 FROM p_contacts.custom_field_definitions
      WHERE id = '018f3000-0000-7000-8000-000000000002'
    )
    OR EXISTS (
      SELECT 1 FROM p_contacts.list_cursors
      WHERE token_hash = repeat('c', 64)
    )
    OR EXISTS (
      SELECT 1 FROM tenant_plugin_installations
      WHERE tenant_id = '018f0000-0000-7000-8000-000000000002'
    )
    OR EXISTS (
      SELECT 1 FROM audit_log
      WHERE id = '018f4000-0000-7000-8000-000000000002'
    )
    OR EXISTS (
      SELECT 1 FROM inbox
      WHERE event_id = '018f5000-0000-7000-8000-000000000002'
    )
    OR EXISTS (
      SELECT 1 FROM outbox
      WHERE id = '018f6000-0000-7000-8000-000000000002'
    ) THEN
    RAISE EXCEPTION 'Workspace A can read a known Workspace B identifier';
  END IF;

  IF (SELECT count(*) FROM p_contacts.people WHERE full_name LIKE 'Workspace % Person') <> 1
     OR (SELECT count(*) FROM p_contacts.organisations WHERE name LIKE 'Workspace % Org') <> 1
     OR (SELECT count(*) FROM p_contacts.custom_field_definitions WHERE api_name = 'private__c') <> 1
     OR (SELECT count(*) FROM tenant_plugin_installations WHERE plugin_id = 'test.rls') <> 1
     OR (SELECT count(*) FROM audit_log WHERE action = 'test.rls.seeded') <> 1
     OR (SELECT count(*) FROM inbox WHERE consumer_id = 'platform.contacts.rls-tenant-isolation') <> 1
     OR (SELECT count(*) FROM outbox WHERE event_type = 'platform.contacts.rls-seeded.v1') <> 1 THEN
    RAISE EXCEPTION 'Workspace A can enumerate Workspace B fixture rows';
  END IF;

  IF (
    SELECT string_agg(full_name, ',' ORDER BY lower(full_name), id)
    FROM (
      SELECT full_name, id
      FROM p_contacts.people
      ORDER BY lower(full_name), id
      LIMIT 2
    ) AS page
  ) IS DISTINCT FROM 'Workspace A Person' THEN
    RAISE EXCEPTION 'Workspace A inferred Workspace B through list pagination';
  END IF;
END
$$;

-- Known-ID updates must affect zero rows rather than crossing the boundary.
DO $$
DECLARE
  affected bigint;
BEGIN
  UPDATE p_contacts.people
  SET full_name = 'Cross-tenant overwrite'
  WHERE id = '018f2000-0000-7000-8000-000000000002';
  GET DIAGNOSTICS affected = ROW_COUNT;
  IF affected <> 0 THEN RAISE EXCEPTION 'cross-tenant person update succeeded'; END IF;

  UPDATE p_contacts.organisations
  SET name = 'Cross-tenant overwrite'
  WHERE id = '018f1000-0000-7000-8000-000000000002';
  GET DIAGNOSTICS affected = ROW_COUNT;
  IF affected <> 0 THEN RAISE EXCEPTION 'cross-tenant organisation update succeeded'; END IF;

  UPDATE p_contacts.custom_field_definitions
  SET label = 'Cross-tenant overwrite'
  WHERE id = '018f3000-0000-7000-8000-000000000002';
  GET DIAGNOSTICS affected = ROW_COUNT;
  IF affected <> 0 THEN RAISE EXCEPTION 'cross-tenant custom-field update succeeded'; END IF;

  UPDATE p_contacts.list_cursors
  SET sort_value = 'inferred-and-overwritten'
  WHERE token_hash = repeat('c', 64);
  GET DIAGNOSTICS affected = ROW_COUNT;
  IF affected <> 0 THEN RAISE EXCEPTION 'cross-tenant cursor update succeeded'; END IF;

  UPDATE tenant_plugin_installations
  SET state = 'disabled'
  WHERE tenant_id = '018f0000-0000-7000-8000-000000000002'
    AND plugin_id = 'test.rls';
  GET DIAGNOSTICS affected = ROW_COUNT;
  IF affected <> 0 THEN RAISE EXCEPTION 'cross-tenant plugin update succeeded'; END IF;

  DELETE FROM p_contacts.people
  WHERE id = '018f2000-0000-7000-8000-000000000002';
  GET DIAGNOSTICS affected = ROW_COUNT;
  IF affected <> 0 THEN RAISE EXCEPTION 'cross-tenant person delete succeeded'; END IF;

  DELETE FROM p_contacts.organisations
  WHERE id = '018f1000-0000-7000-8000-000000000002';
  GET DIAGNOSTICS affected = ROW_COUNT;
  IF affected <> 0 THEN RAISE EXCEPTION 'cross-tenant organisation delete succeeded'; END IF;
END
$$;

-- Cross-tenant inserts fail the policy rather than creating rows for a known
-- foreign workspace.
DO $$
BEGIN
  BEGIN
    INSERT INTO p_contacts.people (tenant_id, id, public_id, full_name)
    VALUES (
      '018f0000-0000-7000-8000-000000000002',
      '018f2000-0000-7000-8000-000000000003',
      'per000000000003abc',
      'Forbidden cross-tenant insert'
    );
    RAISE EXCEPTION 'cross-tenant person insert succeeded';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    INSERT INTO audit_log (id, tenant_id, action, target)
    VALUES (
      '018f4000-0000-7000-8000-000000000003',
      '018f0000-0000-7000-8000-000000000002',
      'test.rls.forbidden',
      'person:forbidden'
    );
    RAISE EXCEPTION 'cross-tenant audit insert succeeded';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    INSERT INTO inbox (tenant_id, consumer_id, event_id)
    VALUES (
      '018f0000-0000-7000-8000-000000000002',
      'platform.contacts.rls-cross-tenant',
      '018f5000-0000-7000-8000-000000000003'
    );
    RAISE EXCEPTION 'cross-tenant inbox insert succeeded';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    INSERT INTO outbox (id, tenant_id, event_type, envelope)
    VALUES (
      '018f6000-0000-7000-8000-000000000003',
      '018f0000-0000-7000-8000-000000000002',
      'platform.contacts.rls-forbidden.v1',
      '{"source_plugin":"platform.contacts"}'::jsonb
    );
    RAISE EXCEPTION 'cross-tenant outbox insert succeeded';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;
END
$$;

-- Tenant-scoped identities must not expose a known foreign identifier through
-- a unique-constraint error. Reusing Workspace B's IDs or token in Workspace A
-- succeeds once, while a true same-workspace duplicate still fails.
INSERT INTO audit_log (id, tenant_id, action, target, metadata)
VALUES (
  '018f4000-0000-7000-8000-000000000002',
  '018f0000-0000-7000-8000-000000000001',
  'test.rls.shared-id',
  'person:per000000000001abc',
  '{"workspace":"a"}'::jsonb
);
INSERT INTO outbox (id, tenant_id, event_type, envelope)
VALUES (
  '018f6000-0000-7000-8000-000000000002',
  '018f0000-0000-7000-8000-000000000001',
  'platform.contacts.rls-shared-id.v1',
  '{"workspace":"a","source_plugin":"platform.contacts"}'::jsonb
);
INSERT INTO p_contacts.list_cursors (
  token_hash, tenant_id, resource, query_hash, sort_value, record_id, expires_at
) VALUES (
  repeat('c', 64),
  '018f0000-0000-7000-8000-000000000001',
  'people',
  repeat('e', 64),
  'workspace-a-shared-token',
  '018f2000-0000-7000-8000-000000000001',
  now() + interval '1 hour'
);

DO $$
BEGIN
  BEGIN
    INSERT INTO audit_log (id, tenant_id, action, target)
    VALUES (
      '018f4000-0000-7000-8000-000000000002',
      '018f0000-0000-7000-8000-000000000001',
      'test.rls.same-tenant-duplicate',
      'person:duplicate'
    );
    RAISE EXCEPTION 'same-tenant audit ID was not unique';
  EXCEPTION WHEN unique_violation THEN
    NULL;
  END;

  BEGIN
    INSERT INTO outbox (id, tenant_id, event_type, envelope)
    VALUES (
      '018f6000-0000-7000-8000-000000000002',
      '018f0000-0000-7000-8000-000000000001',
      'platform.contacts.rls-same-tenant-duplicate.v1',
      '{"source_plugin":"platform.contacts"}'::jsonb
    );
    RAISE EXCEPTION 'same-tenant outbox ID was not unique';
  EXCEPTION WHEN unique_violation THEN
    NULL;
  END;

  BEGIN
    INSERT INTO p_contacts.list_cursors (
      token_hash, tenant_id, resource, query_hash, sort_value, record_id, expires_at
    ) VALUES (
      repeat('c', 64),
      '018f0000-0000-7000-8000-000000000001',
      'people',
      repeat('f', 64),
      'same-tenant-duplicate',
      '018f2000-0000-7000-8000-000000000001',
      now() + interval '1 hour'
    );
    RAISE EXCEPTION 'same-tenant cursor token was not unique';
  EXCEPTION WHEN unique_violation THEN
    NULL;
  END;

  IF (SELECT count(*) FROM audit_log WHERE id = '018f4000-0000-7000-8000-000000000002') IS DISTINCT FROM 1
     OR (SELECT metadata ->> 'workspace' FROM audit_log WHERE id = '018f4000-0000-7000-8000-000000000002') IS DISTINCT FROM 'a'
     OR (SELECT count(*) FROM outbox WHERE id = '018f6000-0000-7000-8000-000000000002') IS DISTINCT FROM 1
     OR (SELECT envelope ->> 'workspace' FROM outbox WHERE id = '018f6000-0000-7000-8000-000000000002') IS DISTINCT FROM 'a'
     OR (SELECT count(*) FROM p_contacts.list_cursors WHERE token_hash = repeat('c', 64)) IS DISTINCT FROM 1
     OR (SELECT sort_value FROM p_contacts.list_cursors WHERE token_hash = repeat('c', 64)) IS DISTINCT FROM 'workspace-a-shared-token' THEN
    RAISE EXCEPTION 'tenant-scoped identity probe exposed or replaced a foreign row';
  END IF;
END
$$;

-- With no context every protected relation is empty and writes fail closed.
SELECT set_config('app.tenant_id', '', true);
DO $$
DECLARE
  protected_table regclass;
  visible bigint;
BEGIN
  FOREACH protected_table IN ARRAY ARRAY[
    'p_contacts.people'::regclass,
    'p_contacts.organisations'::regclass,
    'p_contacts.custom_field_definitions'::regclass,
    'p_contacts.list_cursors'::regclass,
    'tenant_plugin_installations'::regclass,
    'audit_log'::regclass,
    'inbox'::regclass,
    'outbox'::regclass
  ] LOOP
    EXECUTE format('SELECT count(*) FROM %s', protected_table) INTO visible;
    IF visible <> 0 THEN
      RAISE EXCEPTION 'missing tenant context exposed rows from %', protected_table;
    END IF;
  END LOOP;

  BEGIN
    INSERT INTO p_contacts.people (tenant_id, id, public_id, full_name)
    VALUES (
      '018f0000-0000-7000-8000-000000000001',
      '018f2000-0000-7000-8000-000000000004',
      'per000000000004abc',
      'Missing-context insert'
    );
    RAISE EXCEPTION 'missing tenant context allowed a write';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;
END
$$;

-- Malformed context either produces an empty result (text policies) or a
-- UUID parsing error (typed policies); both outcomes deny every row.
SELECT set_config('app.tenant_id', 'not-a-uuid', true);
DO $$
DECLARE
  protected_table regclass;
  visible bigint;
BEGIN
  FOREACH protected_table IN ARRAY ARRAY[
    'p_contacts.people'::regclass,
    'p_contacts.organisations'::regclass,
    'p_contacts.custom_field_definitions'::regclass,
    'p_contacts.list_cursors'::regclass,
    'tenant_plugin_installations'::regclass,
    'audit_log'::regclass,
    'inbox'::regclass,
    'outbox'::regclass
  ] LOOP
    BEGIN
      EXECUTE format('SELECT count(*) FROM %s', protected_table) INTO visible;
      IF visible <> 0 THEN
        RAISE EXCEPTION 'invalid tenant context exposed rows from %', protected_table;
      END IF;
    EXCEPTION WHEN invalid_text_representation THEN
      NULL;
    END;
  END LOOP;

  BEGIN
    INSERT INTO tenant_plugin_installations (tenant_id, plugin_id, state)
    VALUES (
      '018f0000-0000-7000-8000-000000000001',
      'test.rls',
      'enabled'
    );
    RAISE EXCEPTION 'invalid tenant context allowed a write';
  EXCEPTION
    WHEN invalid_text_representation OR insufficient_privilege THEN
      NULL;
  END;
END
$$;

-- Re-establish Workspace A and prove the denied operations changed nothing.
SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000001', true);
DO $$
BEGIN
  IF (SELECT full_name FROM p_contacts.people WHERE id = '018f2000-0000-7000-8000-000000000001') IS DISTINCT FROM 'Workspace A Person'
     OR (SELECT count(*) FROM p_contacts.people WHERE id = '018f2000-0000-7000-8000-000000000003') IS DISTINCT FROM 0
     OR (SELECT count(*) FROM audit_log WHERE id = '018f4000-0000-7000-8000-000000000003') IS DISTINCT FROM 0
     OR (SELECT count(*) FROM inbox WHERE event_id = '018f5000-0000-7000-8000-000000000003') IS DISTINCT FROM 0
     OR (SELECT count(*) FROM outbox WHERE id = '018f6000-0000-7000-8000-000000000003') IS DISTINCT FROM 0 THEN
    RAISE EXCEPTION 'a denied tenant-isolation probe changed persisted state';
  END IF;
END
$$;

-- RBAC assignments are tenant-scoped, read-only to the request role, and
-- reachable across tenants only through the live-session lookup routine.
SELECT set_config('app.tenant_id', '', true);
DO $$
BEGIN
  IF (SELECT count(*) FROM memberships) <> 0
     OR (SELECT count(*) FROM user_roles) <> 0
     OR (SELECT count(*) FROM role_permissions) <> 0
     OR (SELECT count(*) FROM roles) <> 0 THEN
    RAISE EXCEPTION 'RBAC rows are visible without a tenant context';
  END IF;
END
$$;

SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000002', true);
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM memberships)
     OR EXISTS (SELECT 1 FROM memberships WHERE tenant_id <> '018f0000-0000-7000-8000-000000000002')
     OR EXISTS (SELECT 1 FROM role_permissions WHERE tenant_id <> '018f0000-0000-7000-8000-000000000002') THEN
    RAISE EXCEPTION 'RBAC reads are not limited to the current workspace';
  END IF;

  BEGIN
    INSERT INTO user_roles (tenant_id, user_id, role_id)
    VALUES (
      '018f0000-0000-7000-8000-000000000002',
      '018f0000-0000-7000-8000-000000000101',
      '018f0000-0000-7000-8000-000000000202'
    );
    RAISE EXCEPTION 'platform_app granted itself a role assignment';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    DELETE FROM memberships;
    RAISE EXCEPTION 'platform_app removed workspace memberships';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;
END
$$;

INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES
  ('018f7000-0000-7000-8000-000000000001', 'rls-harness-live', '018f0000-0000-7000-8000-000000000104', now() + interval '1 hour'),
  ('018f7000-0000-7000-8000-000000000002', 'rls-harness-revoked', '018f0000-0000-7000-8000-000000000104', now() + interval '1 hour'),
  ('018f7000-0000-7000-8000-000000000003', 'rls-harness-expired', '018f0000-0000-7000-8000-000000000104', now() - interval '1 second');
UPDATE sessions SET revoked_at = now() WHERE id = '018f7000-0000-7000-8000-000000000002';

DO $$
BEGIN
  IF (SELECT count(DISTINCT tenant_id) FROM platform_session_workspace_access(
        '018f7000-0000-7000-8000-000000000001', '018f0000-0000-7000-8000-000000000104', NULL)) <> 2 THEN
    RAISE EXCEPTION 'a live session cannot list its own workspaces';
  END IF;
  IF (SELECT count(DISTINCT tenant_id) FROM platform_session_workspace_access(
        '018f7000-0000-7000-8000-000000000001', '018f0000-0000-7000-8000-000000000104',
        '018f0000-0000-7000-8000-000000000002')) <> 1 THEN
    RAISE EXCEPTION 'workspace narrowing returned the wrong scope';
  END IF;
  IF EXISTS (SELECT 1 FROM platform_session_workspace_access(
        '018f7000-0000-7000-8000-000000000001', '018f0000-0000-7000-8000-000000000101', NULL))
     OR EXISTS (SELECT 1 FROM platform_session_workspace_access(
        '018f7000-0000-7000-8000-000000000002', '018f0000-0000-7000-8000-000000000104', NULL))
     OR EXISTS (SELECT 1 FROM platform_session_workspace_access(
        '018f7000-0000-7000-8000-000000000003', '018f0000-0000-7000-8000-000000000104', NULL))
     OR EXISTS (SELECT 1 FROM platform_session_workspace_access(
        '018f7000-0000-7000-8000-0000000000ff', '018f0000-0000-7000-8000-000000000104', NULL)) THEN
    RAISE EXCEPTION 'the workspace lookup answered for a mismatched, revoked, expired, or unknown session';
  END IF;
END
$$;

-- Plugin execution context. Every forced-RLS table in a plugin schema must
-- carry a RESTRICTIVE policy bound to app.plugin_id, so a new plugin cannot
-- forget the cross-plugin boundary.
DO $$
DECLARE
  unguarded text;
BEGIN
  SELECT string_agg(format('%I.%I', namespace.nspname, relation.relname), ', ')
  INTO unguarded
  FROM pg_catalog.pg_class relation
  JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace
  WHERE namespace.nspname LIKE 'p\_%'
    AND relation.relkind = 'r'
    AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_policies policy
      WHERE policy.schemaname = namespace.nspname
        AND policy.tablename = relation.relname
        AND policy.permissive = 'RESTRICTIVE'
        AND policy.qual LIKE '%app.plugin_id%'
    );
  IF unguarded IS NOT NULL THEN
    RAISE EXCEPTION 'plugin tables without a plugin-context policy: %', unguarded;
  END IF;
END
$$;

-- In the right workspace, another plugin's context sees no Contacts rows and
-- cannot write them, write Contacts inbox markers, or publish as Contacts.
SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000001', true);
SELECT set_config('app.plugin_id', 'platform.rls-other', true);
DO $$
BEGIN
  IF (SELECT count(*) FROM p_contacts.people) <> 0
     OR (SELECT count(*) FROM p_contacts.organisations) <> 0
     OR (SELECT count(*) FROM p_contacts.list_cursors) <> 0 THEN
    RAISE EXCEPTION 'another plugin context can read Contacts rows';
  END IF;

  BEGIN
    INSERT INTO p_contacts.organisations (tenant_id, id, public_id, name)
    VALUES ('018f0000-0000-7000-8000-000000000001', '018f1000-0000-7000-8000-0000000000f1', 'org00000000000zzz', 'Forged');
    RAISE EXCEPTION 'another plugin context wrote a Contacts row';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    INSERT INTO inbox (tenant_id, consumer_id, event_id)
    VALUES ('018f0000-0000-7000-8000-000000000001', 'platform.contacts.forged', '018f5000-0000-7000-8000-0000000000f1');
    RAISE EXCEPTION 'another plugin context wrote a Contacts inbox marker';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;

  BEGIN
    INSERT INTO outbox (id, tenant_id, event_type, envelope)
    VALUES ('018f6000-0000-7000-8000-0000000000f1', '018f0000-0000-7000-8000-000000000001',
            'platform.contacts.person-created.v1', '{"source_plugin":"platform.contacts"}'::jsonb);
    RAISE EXCEPTION 'another plugin context published as Contacts';
  EXCEPTION WHEN insufficient_privilege THEN
    NULL;
  END;
END
$$;

-- Without any plugin context, kernel code sees no plugin rows either.
SELECT set_config('app.plugin_id', '', true);
DO $$
BEGIN
  IF (SELECT count(*) FROM p_contacts.people) <> 0 THEN
    RAISE EXCEPTION 'code without a plugin context can read Contacts rows';
  END IF;
END
$$;

ROLLBACK;
