# 0031: Record ids assigned by devices

## Status

Accepted (2026-10-20). Builds on ADR 0024 (§3, push) and ADR 0015
(public ids). Implemented in `platform_id::PublicId::new_client`, the
engine's create step (`ObjectService::with_client_ids`, and `id` on
create), and the conformance suite `client_ids`.

## Context

A synced device creates records offline and pushes them later as
composite requests (ADR 0024 §3). Offline records refer to each other: a
contact made offline points at an account made offline. If the server
assigned every id, the device would push with temporary ids and then
rewrite each id, and every reference to it, in its replica, which is easy
to get wrong. If the device assigns the id, nothing needs rewriting.

A public id is a 3-character key prefix, a 12-character payload and a
3-character checksum (ADR 0015). The server fills the payload with the
workspace's sequence for the object, which starts at 1.

## Decision

### 1. Devices take ids from a range the server never uses

The payload's top bit marks a device-assigned id. A device fills the
other 59 bits at random (the random tail of a version 7 UUID). Server
sequences stay below the top bit, so the two ranges never meet. Ids from
both ranges have the same format and checksum, so every route, the change
log and references treat them alike.

### 2. The server keeps an assigned id on create

A create may carry `id`, on the record route, in a composite request or in
a bulk job. The server accepts it when it:
- parses as a public id;
- carries the object's key prefix;
- is in the device range.

Anything else is `400 records.id_invalid`. Ids from the server's own range
cannot be chosen, so a client can never take an id the sequence will hand
out later.

### 3. A retried create returns the record

A device that times out may push the same create again. When a record with
the id already exists, belongs to the same object, was created by the same
member and is still readable to them, the create returns that record
unchanged. Otherwise the id is taken: `409 records.id_taken`. A later edit
pushes as an update.

### 4. A replica's engine assigns device ids by default

`ObjectService::with_client_ids()` makes new records take device-range
ids when none is given. The device's engine runs this way in synced mode,
so every local record already has the id it keeps on the server. On the
server, ids still come from the sequence unless a create names one.

## Alternatives considered

- **Server-assigned ids with a remapping step.** The device rewrites
  temporary ids and their references after each push, in every replica
  table and queued change. This means more code on every device and more
  ways to corrupt a replica.
- **Random ids everywhere.** Server-made ids would lose their order and
  the compact sequence ADR 0015 chose, for no gain on the server.
- **Ids namespaced per device.** A device id in the record id would leak
  which device created a record and complicate the format. A random 59 bits
  make collisions negligible, and the server still checks uniqueness.

## Consequences

- Ids alone no longer show creation order across devices and the server.
  Sorting uses `created_at`, as it already does.
- A member can choose an id in the device range for a new record. They
  gain nothing by it beyond what the create grants, and the id cannot be
  in use.
- Old servers reject `id` on create, so devices need a server with this
  change before they sync offline creates.
