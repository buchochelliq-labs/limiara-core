# 0037: Sales: leads, opportunities and pipelines

## Status

Proposed (2026-11-20). Builds on ADR 0015 (metadata-driven objects),
ADR 0017 (field history), ADR 0019 (the query language), ADR 0023
(composite writes), ADR 0016 (sharing and the role hierarchy), ADR 0026
(rollups), ADR 0027 (reports), ADR 0032 (quick actions), ADR 0033
(plugin-declared objects and Activities) and ADR 0035 (Contacts on the
object engine). Part of the roadmap's "CRM application" milestone. This is
a design; delivery slices are in §12.

Decided in review (2026-11-20):

- **Multi-currency** is in scope, with dated exchange rates (§7).
- **Forecasting** (quotas, and rollups by owner and period) is part of
  this milestone (§8).
- **Contact roles** come now: an opportunity has any number of contacts
  with roles, one of them marked primary, following the Salesforce
  `OpportunityContactRole` model and Salesforce Industries (Vlocity)
  party relationships as guidance (§6, §9).
- **Products, price books and quotes** follow directly after conversion,
  with Salesforce Industries' product and pricing model as guidance
  (§10).

## Context

The CRM milestone starts with the sales pipeline: leads that sales people
qualify, and opportunities that move through stages until they are won or
lost. Salesforce's Sales Cloud is the reference: a lead record with a
status, a **Convert** action that turns a qualified lead into an account,
a contact and an opportunity, and opportunities with an amount, a close
date and a stage. Each stage carries a probability and closes the deal as
won or lost. A path across the top of the record page shows the stages,
and a board (kanban) groups open deals by stage.

The platform already carries most of what this needs:

- plugins declare engine objects in their manifests (ADR 0033 §2), so
  sales objects get tenancy, sharing, permissions, field-level security,
  validation rules, automations, quick actions, audit, events, history,
  search, reports and the device replica without code of their own;
- picklists, currency, percent, date and lookup fields, field history and
  related-to references exist;
- the device replica carries every active object, so new objects work
  offline on iOS and desktop with no extra work (ADR 0024).

Four pieces are missing. Each is domain-neutral, so each belongs in the
platform, not in a sales plugin:

1. **A path on the generic record page.** The renderer has a
   `platform.ui.path` component, but only the Contacts person page uses
   it. The engine's record page (`ObjectRecordPage`) has none.
2. **A board view.** Object homes are always a table. `kind: 'kanban'`
   exists only as a contract value.
3. **Converting a record.** Composite writes can create several records
   in one transaction, but nothing declares "convert this record into
   these" with a field mapping, and nothing marks the source converted.
4. **Default reports from a plugin.** Reports exist, but a plugin cannot
   ship any.

Opportunities and leads also need Person and Organisation as engine
objects, for the account and contact a deal belongs to. That is ADR 0035,
whose migration slices are still to come.

CLAUDE.md keeps business capabilities out of the kernel. The sales
objects are therefore a first-party, manifest-only plugin, like
Activities, and the four pieces above are platform features any object
can use.

## Decision

### 1. Stage semantics on picklist options

A picklist option may carry an **outcome**: `open` (the default),
`positive` or `negative`. An option with an outcome other than `open` is
a *closed* value. An option may also carry a **weight**, a percentage
from 0 to 100.

These are domain-neutral. Sales calls them won, lost and probability; a
support case could call them resolved, rejected and confidence. The
engine exposes both attributes in describe results, in replica
definitions and to formulas (`PICKLISTOUTCOME(field)` and
`PICKLISTWEIGHT(field)`). Reports can group by outcome.

Manifest options gain `outcome` and `weight`. Administrators can set
them on custom picklists in the Object Manager.

### 2. The path

An object may name one picklist field as its **path field**, in the
manifest (`path_field = "stage"`) or in the Object Manager for custom
objects.

The generic record page shows the path above the details:

- the field's active options in order, with the current value marked;
- closed options grouped at the end;
- one step to move the record to the next option, which saves through the
  normal update with the record's version.

The path is the existing `platform.ui.path` component. Moving to a closed
option opens the record form with that value preset, so validation rules
that require fields on closing (for example a lost reason) are met in
the same save.

On the phone the path is a horizontal strip that scrolls to the current
option, with a full-width button for the next step.

### 3. The board view

An object home can show its records as a **board**: one column per active
option of a picklist field, the path field by default. A card shows the
record's name and up to three readable fields. Each column shows a count,
and a sum when the object has a currency field chosen for it.

Moving a card to another column updates that field through the normal
update path, with the version check: the change is recorded, audited,
synced and validated, and on a conflict the card returns to its column
with a message.

- **Columns:** closed options are hidden by default, behind a "Show
  closed" control.
- **Size:** a column loads 25 cards at a time with cursor paging, from the
  same list query as the table.
- **Choosing the view:** the board is offered only when the object has a
  path field or a picklist field. The member's choice of table or board
  is remembered per object on the device.
- **Phone and keyboard:**
  - On the phone the board shows one column at a time, with swipe
    between columns. A card's menu offers "Move to…" instead of
    dragging.
  - Keyboard users get the same menu, and every move is announced.

Offline, the board reads and writes the replica like every list, so it
works on a phone with no network.

### 4. Conversions

A manifest may declare a **conversion** on an object: a named action that
creates records of one or more target objects from the source record, in
one transaction.

```toml
[[objects.conversions]]
name = "convert"
label = "Convert"
when = "ISBLANK(platform.sales:converted_at)"
mark_field = "converted_at"

[[objects.conversions.targets]]
object = "platform.contacts:organisation"
key = "account"
match_on = ["name"]
fields = { name = "company", website = "website" }

[[objects.conversions.targets]]
object = "platform.contacts:person"
key = "contact"
fields = { first_name = "first_name", last_name = "last_name", email = "email", phone = "phone" }
references = { organisation = "account" }

[[objects.conversions.targets]]
object = "platform.sales:opportunity"
key = "opportunity"
optional = true
fields = { name = "company" }
references = { account = "account" }
```

`when` is a formula: the action is offered only while it is true, here
while the lead is unconverted. `mark_field` is the date-time field set on
the source when it is converted. `match_on` offers an existing record
instead of a new one, and `optional` lets the member skip a target.

The engine runs a conversion as a composite, all-or-none write (ADR 0023)
as the member, under every normal check:

- the member must be able to edit the source and create each target;
- field-level security applies to every mapped field;
- each target's validation rules run.

The conversion screen shows each target prefilled from the mapping, lets
the member edit the values, choose an existing match instead of creating
one (`match_on`), or skip an optional target, then saves once.

After the save:

- the source's `mark_field` is set and stays set;
- lookups on the source to each created or chosen record are filled when
  the source declares them (`converted_<key>` fields);
- one `platform.objects.record-converted.v1` event and an audit entry
  name the source and the targets.

Conversions work offline as one queued composite change, pushed together.
A conflict on any part keeps the whole conversion queued for the member
to resolve. Mapped fields and targets are checked when the plugin is
installed: a mapping to a field that does not exist, or of incompatible
types, is refused with a `plugin.objects_conversion_invalid` code.

### 5. Default reports and boards from a plugin

A manifest may declare **reports** (`[[reports]]`): a name, a label, a
query in the query language (ADR 0019) over the plugin's own objects or
its dependencies', and a chart. Installing the plugin adds them to each
workspace as public reports owned by the plugin.

Administrators may copy and change them. An upgrade updates only reports
nobody has changed, and never deletes one.

### 6. The Sales plugin

`plugins/sales` (`platform.sales`) is manifest-only, like Activities. It
depends on Contacts and Activities, and declares:

**Lead** (`key_prefix = "led"`, private sharing, history tracked), with
fields:

- first name, last name (required), company, title, email, phone,
  website;
- status, the path field: new, working, nurturing; qualified (positive),
  unqualified (negative);
- source (web, referral, event, partner, other) and rating (hot, warm,
  cold);
- `converted_at`, and `converted_account`, `converted_contact` and
  `converted_opportunity` (lookups).

It also has the conversion of §4 into an organisation, a person and an
opportunity.

**Opportunity** (`key_prefix = "opp"`, private sharing, history tracked),
with fields:

- name (the record name), account (lookup to Organisation);
- amount (currency, in the record's currency, §7), close date (required,
  indexed);
- stage, the path field, with weights: prospecting 10, qualification 20,
  needs analysis 40, proposal 60, negotiation 80, closed won (positive,
  100), closed lost (negative, 0);
- probability (percent; filled from the stage's weight when the stage
  changes, and editable, by a starting automation, below);
- next step, lead source, forecast category (pipeline, best case, commit,
  closed, omitted), and loss reason.

The plugin also ships:

- **Reports:** open pipeline by stage, won this quarter, pipeline by
  close month, and lead conversion by source.
- **Activities:** it names Lead and Opportunity in Activities' starting
  allowlist (ADR 0035 §7, gap G18), so "Log a call" and the Activity
  timeline appear on both from the start.

**Starting automations.** A manifest may declare, per object,
`[[objects.automations]]`: before-save automations that set fields of the
record being saved from formulas (ADR 0022). The kernel adds them as
workspace automations when it creates the object, through the same checks
as an administrator's, and never adds, changes or deletes them afterwards:
they are the workspace's. Sales declares one, `probability_from_stage`,
setting probability to `PICKLISTWEIGHT(stage)` when the stage changes
unless the same save changes it. Updating other records, creating records and
publishing events stay out of manifests.

**Opportunity contact role** (`key_prefix = "ocr"`, sharing controlled by
its opportunity), the people involved in a deal, with fields:

- opportunity (master-detail to Opportunity), contact (lookup to Person,
  required);
- role: decision maker, economic buyer, evaluator, influencer, technical
  buyer, business user, other;
- primary (boolean, at most one per opportunity, §9).

The opportunity page lists its contact roles as a related list with the
primary first. The lead conversion creates the converted contact's role,
marked primary. The primary contact's name is available to list views,
reports and formulas through the role, so no copy of it is kept on the
opportunity.

### 7. Multi-currency

Currency is platform behaviour, not sales behaviour, so the engine gains
it for every object.

- **Currencies.** A workspace has a **corporate currency** and a list of
  active ISO 4217 currencies, set by administrators.
- **Records.** Every record of an object with currency fields carries a
  `currency_code`, which defaults to the member's currency. All of the
  record's currency fields are in that currency, as in Salesforce.
- **Exchange rates.**
  - Administrators keep **dated exchange rates**: each rate applies to
    the corporate currency from its start date until the next one.
  - A currency field's converted value uses the rate in force on a date
    field the object names (for opportunities, the close date), or today
    when it names none.
  - Changing a rate changes converted values, not stored amounts.
- **Converted values.**
  - The query language and reports gain `CONVERTED(field)`, which gives
    the value in the corporate currency.
  - Sums, rollups (ADR 0026) and board column totals use converted
    values, and say so.
  - A member may choose a display currency for reports, converted
    through the corporate currency.
- **Device.** The device holds the rates in the replica definitions, so
  converted values and totals work offline.
- **Existing records.** Records created before a workspace turns on more
  than one currency keep the corporate currency.

### 8. Forecasting

Forecasting is part of the Sales plugin, built on platform pieces.

- **Quotas.** A Quota object (owner, period, amount, currency) holds each
  owner's target per month or quarter.
- **The forecast view.** For a period and a member, it shows the open and
  closed opportunities in that period by forecast category (pipeline,
  best case, commit, closed), against quota, for the member and, through
  the role hierarchy (ADR 0016), for everyone below them.
  - Totals are converted to the viewer's chosen currency (§7).
  - Each cell opens the opportunities behind it.
- **Adjustments.** A manager may adjust a subordinate's or their own
  commit and best-case figures. An adjustment is a Forecast adjustment
  record (owner, period, category, amount, note, adjusted by), kept with
  history, so the original figure stays visible.
- **Domain-neutral support.** The view needs one platform piece: a report
  grouped by the role hierarchy, rolled up to each level. It is a report
  grouping any object can use, not a sales feature.
- **Sharing.** The forecast reads only opportunities the viewer may see,
  so it follows sharing like any report.

### 9. Platform pieces for contact roles

Two manifest gaps from ADR 0033 close for this:

- **Master-detail in manifests.** A plugin may declare a master-detail
  field when it first declares the object. As with every plugin field,
  its type and master cannot change in an upgrade, which removes the
  migration risk ADR 0033 named. Deleting an opportunity then deletes
  its contact roles, and the roles' sharing follows it.
- **One flag per parent.** A boolean field may be declared
  `one_per = "<field>"`. Setting it on a record clears it on every other
  record with the same value of that field, in the same transaction.
  Each cleared record is recorded in history and audit as changed by the
  engine. This gives one primary contact per opportunity, and works for
  any "default" or "primary" flag.

### 10. Products, price books and quotes

Directly after conversion, a further ADR designs the product catalogue,
pricing and quotes, using Salesforce Industries (Vlocity) as guidance:

- products with typed attributes (for example size or term);
- price lists, with price list entries per product and currency;
- opportunity line items with quantity, list price, discount and total,
  rolled up to the opportunity amount;
- quotes with line items, versions and a PDF.

It is listed here so that this ADR's pieces leave room for it: line items
need multi-currency (§7), master-detail (§9) and rollups.

### 11. On the phone and desktop

Nothing here is web-only. The path, the board, conversions and reports
are parts of the web shell over the engine's routes, which the device
answers over the replica (ADR 0024 §5). So iOS and desktop get them with
the web, and offline.

The phone layouts named in §2 and §3 are part of each slice's acceptance,
checked at 390 points wide. Android follows when its push and release
work is picked up.

### 12. Slices

1. **Path and board for every engine object.**
   - Option outcome and weight in the engine, the manifest, the Object
     Manager, describe and replica definitions.
   - The path field and the path on the generic record page.
   - The board view.

   Useful at once on custom objects, such as the demonstration
   workspace's deals.
2. **The Sales plugin, part one.** Lead and Opportunity without their
   Contacts lookups and without conversion, with the stage weights and
   the probability automation. The Activities allowlist lands with G18.
3. **Plugin reports.** `[[reports]]` and the Sales reports.
4. **Contacts on the engine.** ADR 0035's migration slices, which give
   Person and Organisation as engine objects; then the Sales lookups to
   them.
5. **Conversions.** §4, and the lead conversion.
6. **Contact roles.** Master-detail in manifests and `one_per` (§9),
   and Opportunity contact role.
7. **Multi-currency.** §7 for every engine object, then the Sales
   objects in the record's currency.
8. **Forecasting.** Hierarchy rollups in reports, quotas, the forecast
   view and adjustments (§8).
9. **Products, price books and quotes**, after their own ADR (§10).

Each slice runs on both stores (PostgreSQL and SQLite), on the web and in
the device shell, with conformance tests on both stores and a Playwright
journey at desktop and phone widths.

## Alternatives considered

- **Sales code in a native plugin.** This would let conversion and
  probability run as plugin code. It is rejected: the engine's
  declarations cover the need, a manifest-only plugin cannot bypass
  tenancy or permissions, and it runs unchanged on the device.
- **Stage semantics as sales-specific fields.** For example, an
  `is_won` flag on the opportunity. It is rejected for the kernel: an
  outcome on picklist options serves every pipeline-like object, cases
  and projects included.
- **Conversion as an automation.** An after-save automation that creates
  records cannot show the member a review screen, offer matches, or skip
  a target, and it would run on every save that meets its condition.

## Consequences

- Every engine object, custom objects included, gains a path and a board,
  so the demonstration and admin-built objects get them too.
- Manifests grow three declarations (`path_field`, `conversions`,
  `reports`), two option attributes, master-detail fields and the
  `one_per` flag. The manifest's limits and
  installer checks extend to them.
- The Sales plugin depends on ADR 0035's migration for its links to
  people and organisations. Until then (slices 2 and 3) leads and
  opportunities stand alone, and conversion and contact roles wait for
  slices 5 and 6.
- Formulas gain two functions, and the query language
  `CONVERTED(field)`, which the device's evaluators must match.
- Multi-currency touches every currency field, rollup and report, so it
  is a slice of its own with conformance tests on both stores.

## Open questions

- **Exchange rate sources.** Rates are entered by administrators. Should
  a later slice fetch them from a provider, as an integration plugin?
- **Forecast types.** Forecasts are by opportunity amount. Should
  forecasts by quantity or by product family follow with products?
