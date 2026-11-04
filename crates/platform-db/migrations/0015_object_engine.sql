-- Metadata-driven objects over generic record storage (ADR 0015).
--
-- Object and field definitions are metadata; records of every object live in
-- one table with standard fields as columns and custom values in a validated
-- document. Typed pivot tables give tenant-first indexes, uniqueness and
-- reverse references without per-tenant DDL. Every table is tenant-scoped
-- under forced row-level security.

CREATE TABLE object_definitions (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    id uuid NOT NULL,
    api_name text NOT NULL CHECK (api_name ~ '^[a-z][a-z0-9_.]*:[a-z][a-z0-9_]{0,62}$'),
    label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 80),
    plural_label text NOT NULL CHECK (char_length(plural_label) BETWEEN 1 AND 80),
    description text CHECK (description IS NULL OR char_length(description) <= 1000),
    origin text NOT NULL CHECK (origin IN ('workspace', 'plugin')),
    owner_plugin text,
    key_prefix text NOT NULL CHECK (key_prefix ~ '^[0-9a-z]{3}$'),
    name_label text NOT NULL DEFAULT 'Name' CHECK (char_length(name_label) BETWEEN 1 AND 80),
    sharing_model text NOT NULL DEFAULT 'read_write'
        CHECK (sharing_model IN ('private', 'read', 'read_write')),
    searchable boolean NOT NULL DEFAULT true,
    in_navigation boolean NOT NULL DEFAULT true,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'deleted')),
    record_sequence bigint NOT NULL DEFAULT 0 CHECK (record_sequence >= 0),
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    deleted_at timestamptz,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, api_name),
    UNIQUE (tenant_id, key_prefix),
    -- Workspace objects take digit-first prefixes; plugin prefixes start with
    -- a letter, so the two ranges never collide.
    CHECK ((origin = 'workspace') = (key_prefix ~ '^[0-9]'))
);

CREATE TABLE field_definitions (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    object_id uuid NOT NULL,
    api_name text NOT NULL CHECK (api_name ~ '^[a-z][a-z0-9_.]*:[a-z][a-z0-9_]{0,62}$'),
    label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 80),
    description text CHECK (description IS NULL OR char_length(description) <= 1000),
    field_type text NOT NULL CHECK (field_type IN (
        'text', 'long_text', 'email', 'phone', 'url', 'number', 'currency', 'percent',
        'boolean', 'date', 'date_time', 'picklist', 'multi_picklist', 'lookup')),
    origin text NOT NULL CHECK (origin IN ('workspace', 'plugin')),
    required boolean NOT NULL DEFAULT false,
    unique_value boolean NOT NULL DEFAULT false,
    indexed boolean NOT NULL DEFAULT false,
    searchable boolean NOT NULL DEFAULT false,
    max_length integer CHECK (max_length IS NULL OR max_length BETWEEN 1 AND 131072),
    scale integer CHECK (scale IS NULL OR scale BETWEEN 0 AND 8),
    options jsonb NOT NULL DEFAULT '[]'
        CHECK (jsonb_typeof(options) = 'array' AND octet_length(options::text) <= 65536),
    default_value jsonb,
    reference_object_id uuid,
    delete_behaviour text CHECK (delete_behaviour IS NULL OR delete_behaviour IN ('clear', 'restrict')),
    position integer NOT NULL DEFAULT 0,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'deleted')),
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    deleted_at timestamptz,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, object_id, api_name),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id),
    FOREIGN KEY (tenant_id, reference_object_id) REFERENCES object_definitions(tenant_id, id),
    CHECK ((field_type = 'lookup') = (reference_object_id IS NOT NULL))
);
CREATE INDEX field_definitions_tenant_object_idx ON field_definitions (tenant_id, object_id, position);

-- Object permissions per role (Salesforce object permissions on profiles and
-- permission sets). Rows, not permission strings, because objects are created
-- at runtime.
CREATE TABLE object_permissions (
    tenant_id uuid NOT NULL,
    object_id uuid NOT NULL,
    role_id uuid NOT NULL,
    can_read boolean NOT NULL DEFAULT false,
    can_create boolean NOT NULL DEFAULT false,
    can_edit boolean NOT NULL DEFAULT false,
    can_delete boolean NOT NULL DEFAULT false,
    view_all boolean NOT NULL DEFAULT false,
    modify_all boolean NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    PRIMARY KEY (tenant_id, object_id, role_id),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id),
    -- Each grant implies the weaker ones, as in Salesforce.
    CHECK (NOT can_create OR can_read),
    CHECK (NOT can_edit OR can_read),
    CHECK (NOT can_delete OR can_edit),
    CHECK (NOT view_all OR can_read),
    CHECK (NOT modify_all OR (view_all AND can_delete))
);
CREATE INDEX object_permissions_tenant_role_idx ON object_permissions (tenant_id, role_id);

CREATE TABLE records (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    object_id uuid NOT NULL,
    public_id text NOT NULL CHECK (public_id ~ '^[0-9a-z]{18}$'),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 255),
    -- Byte-ordered, so sorting and keyset cursors behave identically on
    -- every backend whatever the cluster locale (see Contacts migration 0007).
    name_key text COLLATE "C" NOT NULL,
    owner_id uuid,
    record_type_id uuid,
    data jsonb NOT NULL DEFAULT '{}'
        CHECK (jsonb_typeof(data) = 'object' AND octet_length(data::text) <= 262144),
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    touched_at timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz,
    deleted_at timestamptz,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, public_id),
    FOREIGN KEY (tenant_id, object_id) REFERENCES object_definitions(tenant_id, id)
);
CREATE INDEX records_tenant_object_name_idx ON records (tenant_id, object_id, name_key, id);
CREATE INDEX records_tenant_object_created_idx ON records (tenant_id, object_id, created_at, id);
CREATE INDEX records_tenant_object_updated_idx ON records (tenant_id, object_id, updated_at, id);
CREATE INDEX records_tenant_object_owner_idx ON records (tenant_id, object_id, owner_id);
CREATE INDEX records_tenant_touched_idx ON records (tenant_id, touched_at, id);
CREATE INDEX records_tenant_deleted_idx ON records (tenant_id, object_id, deleted_at, id)
    WHERE deleted_at IS NOT NULL;

-- Typed index pivot (Salesforce MT_Indexes): one row per indexed value.
CREATE TABLE record_index (
    tenant_id uuid NOT NULL,
    field_id uuid NOT NULL,
    record_id uuid NOT NULL,
    text_value text COLLATE "C",
    number_value double precision,
    time_value timestamptz,
    CHECK (num_nonnulls(text_value, number_value, time_value) = 1),
    FOREIGN KEY (tenant_id, record_id) REFERENCES records(tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX record_index_text_idx ON record_index (tenant_id, field_id, text_value, record_id)
    WHERE text_value IS NOT NULL;
CREATE INDEX record_index_number_idx ON record_index (tenant_id, field_id, number_value, record_id)
    WHERE number_value IS NOT NULL;
CREATE INDEX record_index_time_idx ON record_index (tenant_id, field_id, time_value, record_id)
    WHERE time_value IS NOT NULL;
CREATE INDEX record_index_record_idx ON record_index (tenant_id, record_id);

-- Uniqueness pivot: the primary key enforces unique field values
-- transactionally across one object's records.
CREATE TABLE record_unique (
    tenant_id uuid NOT NULL,
    field_id uuid NOT NULL,
    value_key text COLLATE "C" NOT NULL,
    record_id uuid NOT NULL,
    PRIMARY KEY (tenant_id, field_id, value_key),
    FOREIGN KEY (tenant_id, record_id) REFERENCES records(tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX record_unique_record_idx ON record_unique (tenant_id, record_id);

-- Reference pivot: one row per lookup value, for related lists, delete
-- behaviour and query traversal.
CREATE TABLE record_references (
    tenant_id uuid NOT NULL,
    field_id uuid NOT NULL,
    record_id uuid NOT NULL,
    target_id uuid NOT NULL,
    PRIMARY KEY (tenant_id, field_id, record_id),
    FOREIGN KEY (tenant_id, record_id) REFERENCES records(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, target_id) REFERENCES records(tenant_id, id)
);
CREATE INDEX record_references_target_idx ON record_references (tenant_id, target_id, field_id, record_id);

DO $$
DECLARE
    table_name text;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'object_definitions', 'field_definitions', 'object_permissions',
        'records', 'record_index', 'record_unique', 'record_references'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format(
            'CREATE POLICY %I ON %I TO platform_app '
            'USING (tenant_id = nullif(current_setting(''app.tenant_id'', true), '''')::uuid) '
            'WITH CHECK (tenant_id = nullif(current_setting(''app.tenant_id'', true), '''')::uuid)',
            table_name || '_tenant_isolation', table_name);
        EXECUTE format(
            'CREATE POLICY %I ON %I TO platform_migrator USING (true) WITH CHECK (true)',
            table_name || '_migrator_management', table_name);
    END LOOP;
END
$$;

-- Kernel system permissions for the engine (Salesforce "Customize
-- Application", "View All Data" and "Modify All Data").
INSERT INTO permissions (id, description) VALUES
  ('objects.manage', 'Create and change objects, fields and object permissions'),
  ('records.view_all', 'Read every record of every object'),
  ('records.modify_all', 'Read, change and delete every record of every object')
ON CONFLICT (id) DO NOTHING;

-- The local development administrator holds them.
INSERT INTO role_permissions (tenant_id, role_id, permission_id)
SELECT r.tenant_id, r.id, p.id
FROM roles r
CROSS JOIN permissions p
WHERE r.tenant_id = '018f0000-0000-7000-8000-000000000001'
  AND r.id = '018f0000-0000-7000-8000-000000000201'
  AND p.id IN ('objects.manage', 'records.view_all', 'records.modify_all')
ON CONFLICT DO NOTHING;
