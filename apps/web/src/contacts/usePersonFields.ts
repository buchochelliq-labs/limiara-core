import {useEffect, useMemo, useState} from 'react';
import {ContactsApiError, type ContactsClient, type OrganisationOption, type PersonFieldDefinition, type Seed} from './api';
import {contactsPersonPermissions, organisationsReadPermission} from './person-metadata';

/** A catalogue read: `undefined` while loading, `null` when it could not be read. */
export type Catalogue<T> = T | undefined | null;

type Loaded<T> = {client: ContactsClient; value: Catalogue<T>};

function useCatalogue<T>(client: ContactsClient, seed: () => Seed<T> | undefined, read: (signal: AbortSignal) => Promise<T>, enabled: boolean, disabledValue: T): Catalogue<T> {
  const [loaded, setLoaded] = useState<Loaded<T> | undefined>(() => {
    const seeded = seed();
    return seeded ? {client, value: 'value' in seeded ? seeded.value : null} : undefined;
  });
  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    const seeded = seed();
    if (seeded) setLoaded({client, value: 'value' in seeded ? seeded.value : null});
    read(controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoaded({client, value}); })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        // A caller who may not list the catalogue works without it; any other
        // failure is reported so a form never saves against unknown fields.
        const forbidden = error instanceof ContactsApiError && (error.status === 403 || error.status === 401);
        setLoaded(current => current?.client === client && current.value ? current : {client, value: forbidden ? disabledValue : null});
      });
    return () => controller.abort();
    // seed and read are derived from the client.
  }, [client, enabled]);
  if (!enabled) return disabledValue;
  return loaded?.client === client ? loaded.value : undefined;
}

/** Tenant custom field definitions for people, seeded synchronously when the client can. */
export function usePersonFields(client: ContactsClient, permissions: ReadonlySet<string>): Catalogue<PersonFieldDefinition[]> {
  const allowed = permissions.has(contactsPersonPermissions.customFields);
  return useCatalogue(client, () => client.seeds?.personFields?.(), signal => client.listPersonFields(signal), allowed, []);
}

/** Api names of the custom fields configured for people; empty until definitions are known. */
export function usePersonFieldNames(client: ContactsClient, permissions: ReadonlySet<string>): ReadonlySet<string> {
  const fields = usePersonFields(client, permissions);
  return useMemo(() => new Set(fields?.map(field => field.apiName) ?? []), [fields]);
}

/** Organisations the caller may link a person to, seeded synchronously when the client can; empty without read permission. */
export function useOrganisationOptions(client: ContactsClient, permissions: ReadonlySet<string>): Catalogue<OrganisationOption[]> {
  return useCatalogue(client, () => client.seeds?.organisations?.(), signal => client.listOrganisations(signal), permissions.has(organisationsReadPermission), []);
}
