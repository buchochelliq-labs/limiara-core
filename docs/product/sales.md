# Using Sales

**Status:** experimental. Design: [ADR 0037](../adr/0037-sales-pipeline.md), slice 2.
Plugin: `platform.sales`, in `plugins/sales`.

Sales tracks the deals a workspace works on. The plugin ships two objects,
**Leads** and **Opportunities**. Each one has a path: the record page shows
its status or stage as steps, and the list can be shown as a board with one
column per step.

## Turning it on

A workspace administrator enables the plugin
(`POST /api/v1/workspaces/{workspace}/plugins/platform.sales/enable`).
Leads and Opportunities then appear in navigation for anyone who may read
them. Like any new object, no role has access until an administrator grants
it in the object's permissions, and records start private: each workspace
chooses the sharing model.

Disabling the plugin hides leads and opportunities everywhere. Nothing is
deleted. Enabling it again brings everything back unchanged.

## Leads

A lead is someone who may become a customer, before they are qualified.

| Field | What it holds |
| --- | --- |
| Name | The person's name. Required. |
| Status | The lead's path: New (the default), Working, Nurturing, then Qualified or Unqualified, which close it. |
| Company, Title | Where they work and their role. |
| Email, Phone, Website | How to reach them. |
| Lead source | Web, Referral, Event, Partner or Other. |
| Rating | Hot, Warm or Cold. |
| Description | Notes, as long text. |

## Opportunities

An opportunity is a deal in progress, moving through stages until it is won
or lost.

| Field | What it holds |
| --- | --- |
| Opportunity name | What the deal is. Required. |
| Stage | The opportunity's path, below. Prospecting is the default. |
| Amount | The deal's value. |
| Close date | When it is expected to close. Required. |
| Probability | How likely it is to close, as a percentage. |
| Forecast category | Pipeline (the default), Best case, Commit, Closed or Omitted. |
| Next step | What happens next. |
| Lead source | As on leads. |
| Loss reason | Why a lost deal was lost: Price, Lost to a competitor, No decision, Timing or Other. |
| Description | Notes, as long text. |

Each stage carries its usual probability of closing:

| Stage | Probability | Closes the deal |
| --- | --- | --- |
| Prospecting | 10% | |
| Qualification | 20% | |
| Needs analysis | 40% | |
| Proposal | 60% | |
| Negotiation | 80% | |
| Closed won | 100% | Won |
| Closed lost | 0% | Lost |

**Probability follows the stage.** A new opportunity starts at its stage's
probability, and moving it to another stage sets the probability to that
stage's. A member can still set the probability by hand: a save that sets it
keeps the member's value, and so do later saves that leave the stage alone.

This is an ordinary automation, **probability_from_stage**, that the plugin
adds when it creates the Opportunity object. Administrators find it in the
object's automations and may change it, turn it off or delete it. Upgrading
the plugin never adds it back or changes it.

The stages and their probabilities come with the plugin, like all of its
fields: a workspace cannot rename stages or change their probabilities yet.
On a custom object's own picklist, administrators set each option's
probability (its weight) through the objects API.

## Moving along the path

On a record page, choose a step on the path and **Mark as** to move the
record there. Choosing a closing step (Qualified or Unqualified, Closed won
or Closed lost) opens the record's form, so anything else the close needs,
such as a loss reason, is filled in at the same time. On the board, drag a
card to another column, or use its **Move to…** menu.

Every move is an ordinary record update: the record's permissions, sharing,
validation rules, field history and audit apply to it as to any other edit.

## Not yet

Leads and opportunities stand alone for now. Their links to accounts and
contacts arrive when Contacts moves onto the object engine. Converting a
qualified lead into an account, contact and opportunity, contact roles on
opportunities, products and quotes,
multiple currencies and forecasts follow in later slices of [ADR 0037](../adr/0037-sales-pipeline.md).
