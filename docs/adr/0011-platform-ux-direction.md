# 0011: Platform UX direction and customisation layers

## Status

Accepted

## Context

The web shell and the Contacts pages were built as metadata-driven proofs
that the plugin boundary holds in the browser. They did not yet describe a
product: the directory was a card list with a preview rail, the record page
a single column, forms were full pages, and navigation named one "module"
per plugin. The platform's goal is a customer operations product that a
workspace administrator configures and that ordinary users personalise, in
the way established CRM platforms are used, without plugins shipping layout
code into the authenticated origin.

## Decision

1. **User-facing vocabulary.** What a plugin contributes to a workspace is a
   *feature*. "Plugin" remains the engineering term for the unit of code,
   manifest, and lifecycle; "feature" is the only word the shell, settings and
   documentation aimed at users use. A plugin may contribute several features
   (navigation items) and each is enabled, listed and personalised as one.

2. **Shell.** A 72px rail holds the feature launcher, Home, the features the
   workspace has enabled, and Settings; labels appear as tooltips and in the
   phone drawer. The launcher, Home and the global search are composed only
   from the contribution endpoint and search results the caller may open.
   Home becomes a dashboard composed from cards features contribute through a
   `dashboard-card` slot; until features contribute cards it lists the
   enabled features.

3. **Object patterns.** Every resource a plugin declares gets the same
   screens from host components: an *object home* (list views, table or
   card or kanban presentation, filters, bulk actions, hover card, cursor
   paging), a *record page* (highlights panel, optional stage path, Details
   / Related / Activity / History tabs, compact field sections, a right rail
   for relationships, the activity timeline and contributed cards), and a
   *record form* (a modal on wide screens, full screen on phones, with Save,
   Save & New and Cancel, validation, denied, conflict and network states).
   The resource and page contracts gain the flags these need: `list_default`,
   `highlight`, `sortable`, `filterable`, `searchable`, `stage`, `group_by`
   on fields; `object-home` and `record` page kinds; host components
   `platform.ui.table`, `platform.ui.highlights`, `platform.ui.tabs`,
   `platform.ui.path`, `platform.ui.hover-card` and
   `platform.ui.modal-form`; and the slots `dashboard-card`,
   `settings-page`, `record-tab` and `global-new`.

4. **Customisation layers.** Four layers compose every screen, each able to
   tighten but never loosen the one below it:
   - *Plugin definition*: resources, fields, types, validation, default
     pages, actions and slots, versioned with the plugin.
   - *Workspace layout*: an administrator holding `ui.metadata.manage`
     edits layouts in a host-owned builder. A layout is a customisation
     overlay on a plugin page, stored in the existing UI publication store
     as draft, preview and published revisions with rollback and audit, and
     assigned by role, form factor and record condition. The builder can
     move, group, relabel, add help text, add required or read-only, and set
     visibility conditions; it cannot add fields the plugin lacks, relax
     plugin validation, or reveal a field a role may not read. Publishing
     refreshes the workspace delivery snapshot; a plugin upgrade that removes
     a field marks dependent layouts for review.
   - *Workspace fields*: custom-field definitions remain the plugin's own
     settings surface (for Contacts, `contacts.settings.manage`).
   - *Personal*: any user keeps per-user, per-workspace preferences
     (columns and order, density, saved and pinned views, dashboard card
     order, rail order, collapsed sections, default tab). They never affect
     other users and never override workspace visibility or required rules.

5. **Unchanged guarantees.** Navigation and cards come only from the
   contribution endpoint; plugin data renders as inert values; no third-party
   script runs in the authenticated origin; permissions are enforced by the
   API, so hiding in a layout is never an access control.

## Consequences

- The shell, host component registry and metadata contracts grow; business
  plugins do not gain layout code.
- The design direction and board-by-board rationale live on the product
  canvas referenced from `docs/product/application-shell.md`; this ADR fixes
  the boundaries, not the pixels.
- Delivery is sliced: shell and vocabulary first, then object home, record
  page, record form, the layout builder with its overlay model, and personal
  preferences. Each slice ships behind the existing metadata validation gate
  and visual suite.
