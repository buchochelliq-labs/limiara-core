import standardFieldDocument from '../../../../../docs/architecture/standard-record-fields.json';
import type {CallerSafeFieldDescriptor} from '../contracts';

/**
 * The fields the kernel defines on every record (ADR 0014), read from the
 * same definition the kernel checks itself against.
 */
export const standardRecordFields = standardFieldDocument as CallerSafeFieldDescriptor[];

export const standardFieldNames: ReadonlySet<string> = new Set(standardRecordFields.map(field => field.api_name));

/** A workspace member as record responses carry them; `name` is null for former members. */
export type UserReference = {id: string; name: string | null};

/**
 * The resource's own fields followed by the standard fields it does not
 * declare itself, as the kernel serves them. Only `name` may be declared by
 * the resource, when it is the resource's name field.
 */
export function withStandardFields(fields: readonly CallerSafeFieldDescriptor[]): CallerSafeFieldDescriptor[] {
  const declared = new Set(fields.map(field => field.api_name));
  return [...fields, ...standardRecordFields.filter(field => !declared.has(field.api_name))];
}

/** The navigation target a page adapter maps to a user's record. */
export const userRecordTarget = 'platform.users.view';

/** Where a user's record lives in the web app. */
export function userRecordRoute(id: string): string {
  return `/users/${encodeURIComponent(id)}`;
}

export function userReference(value: unknown): UserReference | null {
  if (!value || typeof value !== 'object') return null;
  const {id, name} = value as {id?: unknown; name?: unknown};
  if (typeof id !== 'string') return null;
  return {id, name: typeof name === 'string' ? name : null};
}

/** How a member reference reads on a record: their name, or that they have left the workspace. */
export function formatUserReference(user: UserReference | null): string {
  if (!user) return '';
  return user.name ?? 'Former member';
}

/** A record timestamp in the viewer's locale and time zone. */
export function formatRecordTimestamp(value: string | null | undefined, timeZone?: string): string {
  if (!value) return '';
  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) return '';
  return new Date(timestamp).toLocaleString('en-GB', {day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', ...(timeZone ? {timeZone} : {})});
}
