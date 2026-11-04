# GDPR and privacy controls

## Data minimisation
Contacts custom fields are tenant-defined metadata, not arbitrary hidden columns. Each non-public custom field must carry a processing purpose and data classification so admins can review why the field exists.

## Lawful basis and purpose limitation
The platform does not decide lawful basis automatically. Instead, custom field definitions store purpose metadata and data classification (`public`, `internal`, `personal`, `sensitive`) to support controller review, DPIA workflows, and future exports.

## Tenant-specific customisation
Custom field definitions are scoped by `tenant_id`; two workspaces may use the same `api_name` with different labels, validation schemas, retention periods, or visibility rules without sharing metadata or values.

## Sensitive data
Sensitive custom fields must be marked encrypted before activation. Phase 1 records the metadata flag; production enforcement should bind this to column/value encryption, key management, export redaction, and audit events.

## Retention and erasure
Definitions can carry `retention_days`. Future retention workers should evaluate field-level retention together with record archival and legal hold state. Erasure workflows must delete or anonymise both standard fields and matching `custom_fields` JSONB keys.

## Subject access and portability
Because custom fields are metadata-addressable, exports can include labels, API names, field types, purposes, and values. This avoids opaque JSON dumps in DSAR responses.
