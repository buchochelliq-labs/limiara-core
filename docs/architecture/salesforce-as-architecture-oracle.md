# Salesforce as an architecture oracle

Salesforce is a useful architectural oracle because it has spent decades evolving a metadata-driven CRM platform with standard objects, custom objects, custom fields, relationship fields, packaged extensions, events, APIs, reporting, and governance. Limiara should learn from those patterns without becoming a clone or inheriting every Salesforce trade-off.

## Salesforce concepts to learn from

### Standard objects and fields
Salesforce ships standard objects and fields for common CRM concepts. Limiara's equivalent is first-party plugins such as Contacts, where standard fields remain typed plugin-owned columns and standard APIs remain stable.

Design rule: keep out-of-the-box plugin constructs typed and indexed instead of storing all fields in generic EAV tables.

### Custom objects and custom fields
Salesforce exposes custom objects and custom fields as metadata and distinguishes many custom API names with suffixes such as `__c`. Limiara adopts the API-name idea for tenant-defined fields while keeping customisation plugin-scoped.

Design rule: custom fields are metadata-addressable and tenant-scoped; they extend plugin constructs but do not grant raw schema mutation privileges.

### Metadata API
Salesforce's Metadata API treats fields and objects as deployable configuration. Limiara should eventually provide a metadata API for custom fields, layouts, validation rules, read-model projections, and plugin UI contributions.

Design rule: admin configuration must be exportable, reviewable, auditable, and promotable across environments.

### Custom metadata types
Salesforce separates some configuration-like records from ordinary business data through custom metadata types. Limiara should use domain-neutral metadata registries for platform/plugin configuration and avoid burying operational behavior in business records.

Design rule: workflow/configuration metadata needs lifecycle, migration, audit, package, and environment-promotion semantics.

### Relationship fields
Salesforce relationship fields are first-class metadata. Limiara should model cross-plugin relationships through explicit public APIs, events, governed read models, or declared relationship metadata rather than letting plugins join private tables.

Design rule: relationship customisation must not break plugin data ownership or tenant isolation.

### External objects and integration projections
Salesforce external objects inform how to expose or virtualize data not owned by the core transactional database. Limiara should support external service plugins and governed read projections before exposing direct data federation.

Design rule: remote data is a plugin/integration concern with capability grants, caching policy, and failure isolation.

### Platform events and async automation
Salesforce platform events are a reminder that automation and integration need durable event contracts. Limiara's outbox/NATS event fabric should keep event schemas versioned and tenant-scoped.

Design rule: asynchronous extension points must be versioned, replay-aware, idempotent, and auditable.

### Large data patterns
Salesforce's broader platform has separate patterns for large objects/analytics. Limiara should not force all workloads into PostgreSQL forever; object storage, search, analytics, and vector stores should be added when access patterns justify them.

Design rule: choose persistence by workload, but keep PostgreSQL as the Phase 1 source of truth.

## What Limiara should not copy

- Do not put every domain into one global object model.
- Do not make admin customisation bypass plugin contracts.
- Do not make proprietary cloud control-plane services mandatory for community installations.
- Do not expose arbitrary marketplace JavaScript in the authenticated origin.
- Do not treat entitlement limits as authorization decisions.

## Near-term backlog inspired by Salesforce

1. Metadata API for Contacts custom field definitions.
2. Field-level permissions and layouts for standard/custom fields.
3. Validation rules for custom fields using a safe expression language.
4. Import/export mapping that understands custom fields by API name.
5. Relationship metadata for safe links between plugin-owned constructs.
6. Report/read-model metadata that can include standard and custom fields.
7. Packageable metadata bundles for plugins and tenant configuration promotion.
