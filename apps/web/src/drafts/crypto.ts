/**
 * Draft encryption (ADR 0036 §4). The server holds one random 256-bit key
 * per user and browser. The shell fetches it once per page load, derives a
 * per-workspace AES-256-GCM key with HKDF-SHA-256 (info
 * `limiara.record-drafts.v1`, salt the workspace id), imports that as
 * non-extractable and keeps it in memory only: it is never written to
 * IndexedDB, `localStorage` or anywhere else. Each write uses a fresh
 * random 96-bit nonce, and the associated data binds the payload to its
 * format version, member, workspace and draft, so a payload copied into
 * another member's or workspace's entry fails to decrypt.
 */

export const DRAFT_KEY_INFO = 'limiara.record-drafts.v1';
export const DRAFT_FORMAT = 1;
const NONCE_BYTES = 12;
const KEY_BYTES = 32;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export class DraftCryptoError extends Error {}

/** Decodes the server's base64url key; anything but 32 bytes is refused. */
export function decodeKey(encoded: string): Uint8Array {
  const base64 = encoded.replace(/-/g, '+').replace(/_/g, '/');
  const padded = base64 + '='.repeat((4 - (base64.length % 4)) % 4);
  let binary: string;
  try {
    binary = atob(padded);
  } catch {
    throw new DraftCryptoError('The draft key is malformed.');
  }
  if (binary.length !== KEY_BYTES) throw new DraftCryptoError('The draft key has the wrong length.');
  const bytes = new Uint8Array(KEY_BYTES);
  for (let index = 0; index < KEY_BYTES; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

/**
 * Imports the browser's key as a non-extractable HKDF key and wipes the
 * raw bytes, so from here on only non-extractable keys hold it. One page
 * load imports it once and derives each workspace's key from it.
 */
export async function importBrowserKey(raw: Uint8Array, subtle: SubtleCrypto = crypto.subtle): Promise<CryptoKey> {
  if (raw.byteLength !== KEY_BYTES) throw new DraftCryptoError('The draft key has the wrong length.');
  try {
    return await subtle.importKey('raw', raw, 'HKDF', false, ['deriveKey']);
  } finally {
    raw.fill(0);
  }
}

/** Derives a workspace's non-extractable AES-256-GCM draft key from the browser key. */
export async function deriveWorkspaceKey(browserKey: CryptoKey, workspaceId: string, subtle: SubtleCrypto = crypto.subtle): Promise<CryptoKey> {
  return subtle.deriveKey(
    {name: 'HKDF', hash: 'SHA-256', salt: encoder.encode(workspaceId), info: encoder.encode(DRAFT_KEY_INFO)},
    browserKey,
    {name: 'AES-GCM', length: 256},
    false,
    ['encrypt', 'decrypt'],
  );
}

/** The associated data of one draft: format version, member, workspace and draft id, unambiguously encoded. */
export function associatedData(member: string, workspace: string, draft: string, format = DRAFT_FORMAT): Uint8Array {
  return encoder.encode(JSON.stringify([format, member, workspace, draft]));
}

export type Sealed = {nonce: Uint8Array; data: ArrayBuffer};

export async function seal(key: CryptoKey, aad: Uint8Array, plain: Uint8Array, subtle: SubtleCrypto = crypto.subtle): Promise<Sealed> {
  const nonce = crypto.getRandomValues(new Uint8Array(NONCE_BYTES));
  const data = await subtle.encrypt({name: 'AES-GCM', iv: nonce, additionalData: aad, tagLength: 128}, key, plain);
  return {nonce, data};
}

/** Opens a payload; a wrong key, wrong associated data or tampering all fail the same way. */
export async function open(key: CryptoKey, aad: Uint8Array, sealed: Sealed, subtle: SubtleCrypto = crypto.subtle): Promise<Uint8Array> {
  if (!(sealed.nonce instanceof Uint8Array) || sealed.nonce.byteLength !== NONCE_BYTES) throw new DraftCryptoError('The draft cannot be read.');
  try {
    return new Uint8Array(await subtle.decrypt({name: 'AES-GCM', iv: sealed.nonce, additionalData: aad, tagLength: 128}, key, sealed.data));
  } catch {
    throw new DraftCryptoError('The draft cannot be read.');
  }
}

export const encodeText = (value: string) => encoder.encode(value);
export const decodeText = (value: Uint8Array) => decoder.decode(value);
