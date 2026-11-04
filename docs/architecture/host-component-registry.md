# Host component registry and renderer

Issues #115 and #130 introduce and extend the production React renderer in
`apps/web/src/plugin-ui/renderer`. It consumes an already validated and resolved
page; resource submission, action invocation, metadata resolution, and tenant
editing remain separate concerns.

## Security and failure boundary

- Metadata selects components only from `HostComponentRegistry`.
- Every component declares allowed hosts, props, child behavior, slots,
  keyboard behavior, and accessibility semantics.
- Raw `style`, CSS/class, colour, and background props are rejected. Host CSS
  uses semantic tokens so plugins cannot bypass themes or forced-colour rules.
- Unknown, disabled, host-incompatible, invalid, or failed components produce a
  contained fallback. Ordinary users see no plugin or binding details;
  development diagnostics include the page, node, component, sources, and first
  invalid property.
- React escapes all metadata text. No HTML, script, arbitrary event handler, or
  plugin CSS execution path exists in this renderer.
- Literal and resolved binding values are validated against the target
  component's declared prop shape before rendering. Diagnostics identify the
  failed binding name but never include its value.

## Initial registry

| Component | Props | Children / slots | Keyboard and semantics |
| --- | --- | --- | --- |
| `platform.ui.page` | title, description, icon | required | Main landmark and one level-one heading; optional route focus target. |
| `platform.ui.stack` | gap | required | Neutral vertical layout; document order. |
| `platform.ui.grid` | columns | required | Responsive grid; collapses without changing reading order. |
| `platform.ui.split` | none | required; primary, secondary | Responsive two-region layout; source order remains primary first. |
| `platform.ui.section` | title, description | optional | Labelled section with level-two heading. |
| `platform.ui.card` | title, description | optional | Article with level-three heading. |
| `platform.ui.tabs` | label, tabs | required | Arrow Left/Right and Home/End select tabs; tab/tabpanel relationships are explicit. |
| `platform.ui.accordion` | title, open | optional | Native details/summary; Enter and Space toggle. |
| `platform.ui.text` | text, tone | none | Escaped paragraph text. |
| `platform.ui.alert` | title, variant | optional | Status semantics except danger, which uses an assertive alert. |
| `platform.ui.field.text` | name, label, description, error, value, type, required, disabled, read_only | none | Native labelled input with described-by and invalid relationships. |
| `platform.ui.field.select` | name, label, description, error, value, options, required, disabled | none | Native labelled select and options. |
| `platform.ui.collection-controls` | label | required | Search landmark grouping query, filter, and sort controls. |
| `platform.ui.query-input` | label, placeholder, value, action, parameter, submit_label | none | Native search input; Enter or the apply button invokes only the declared action. |
| `platform.ui.query-select` | label, value, options, action, parameter | none | Native filter/sort select; change invokes only the declared action. |
| `platform.ui.collection` | label, resource, items, fields, empty_title, empty_description, selected_id, select_action, selection_parameter, presentation, identity_label, identity_sort, sort_action, sort_parameter, sort, sort_direction, open_action, open_parameter, open_label | none | Labelled semantic list with row/card presentation, or a captioned table (`presentation: table`; stacked rows on compact hosts) whose first column names each row; sortable headers are toggle buttons with `aria-sort` set from `sort_direction` (ascending by default); an optional open action renders a named button per item; Arrow keys and Home/End move focus between selectable items. |
| `platform.ui.filter-chips` | label, chips, clear_action, clear_label | none | Active filters as chips with named remove buttons and an optional clear-all; renders nothing when empty. |
| `platform.ui.record-preview` | label, empty_title, empty_description | optional selected record | Complementary preview region with a bounded empty state. |
| `platform.ui.highlights` | label, resource, record, fields, icon | optional action groups | Record identity with an optional decorative object tile, a definition list of configured key fields, and child action groups in reading order. |
| `platform.ui.path` | label, stages, current, empty_description | none | Navigation landmark with an ordered stage list; the current stage carries `aria-current="step"`, earlier stages are marked complete; read-only. |
| `platform.ui.timeline` | label, items, empty_title, empty_description | none | Ordered activity entries with title, description, actor and a machine-readable time; bounded empty state. |
| `platform.ui.record-summary` | label, resource, record, fields, show_identity | none | Labelled optional identity and definition list containing only configured caller-readable fields. |
| `platform.ui.relationship-list` | label, resource, items, fields, select_action, selection_parameter, empty_title, empty_description, presentation | none | Labelled semantic list for declared related records; optional native-button selection uses the collection keyboard contract. |
| `platform.ui.cursor` | label, previous_action, next_action, previous_cursor, next_cursor, parameter, previous_label, next_label | none | Navigation landmark with disabled native buttons when opaque cursors are absent. |
| `platform.ui.state` | variant, title, description, lines | optional recovery action | Loading, empty, error, denied, archived, and unavailable semantics; denied text is host-owned and non-disclosing. |
| `platform.ui.table` | label, columns, rows | none | Captioned table and scoped headers; internal/desktop/portal only. |
| `platform.ui.action-bar` | label | required | Labelled action group; buttons remain in document order. |
| `platform.ui.action` | label, variant, disabled | none | Native button; #117 supplies invocation and state behavior. |
| `platform.ui.dialog` | title, open | required | Native labelled dialog; full focus/close action integration belongs to #117. |
| `platform.ui.slot` | label, kind | optional | Stable card/action/related-list/timeline target; empty slots render no landmark or chrome. |
| `platform.ui.fragment` | fragment_id | required | Neutral reusable-fragment boundary. |
| `platform.ui.loading` | label, lines | none | Polite named loading status; reduced motion is inherited. |
| `platform.ui.empty` | title, description | optional action | Explanatory heading, description, and optional action. |
| `platform.ui.denied` | title, description | none | Non-disclosing permission state. |
| `platform.ui.error` | title, description | optional recovery action | Contained assertive failure state. |

Localized strings use `<property>_key` references resolved against the caller's
message catalogue, with safe host-provided fallbacks. The renderer records the
locale and host on its root for diagnostics and future hydration checks.

## Collection and record data boundary

Collection items and record summaries have a deliberately small caller-safe
shape: stable `id`, display `title`, optional `description` and `archived`
state, plus a `values` map. A component renders only keys declared in its
`fields` list **and** present as readable in the named caller-safe `resource`
descriptor. Labels come from that filtered descriptor rather than arbitrary
data-source content. Additional or forbidden values and labels are never dumped
or serialized into page markup. Each requested display field has a stable key
and an optional host-owned format hint.

Page bindings may supply these props from a literal, context, field, or declared
data source. The host resolves bindings before prop validation. Unknown binding
targets, invalid item/record shapes, raw styling targets, or missing required
bound values produce the same contained fallback as malformed literal props.
Query and cursor controls can override only parameters declared with a literal
placeholder on a registered metadata action. Field, context, and data-source
parameters are resolved by the host and cannot be replaced by control input.
The interaction adapter remains responsible for URL/history and API behavior.

Collection selection uses native buttons. Arrow keys move focus without
changing selection; activation invokes the declared selection action. Compact
and narrow layouts change rows to cards without changing DOM order. Opaque
cursors are passed back unchanged and are never decoded by the renderer.

## Responsive and development fixtures

The fixture in `renderer/fixtures.ts` is rendered unchanged with wide and compact
form-factor classes. Layout containers use `minmax(0, …)`, bounded overflow, and
a single-column narrow breakpoint. The development-only `/__ui` showcase renders
the fixture alongside the base primitives so component and token changes remain
reviewable. Automated tests cover semantics, localization, narrow/wide output,
unsupported hosts, disabled/unknown components, prop rejection, renderer
failure containment, and development diagnostic redaction.
