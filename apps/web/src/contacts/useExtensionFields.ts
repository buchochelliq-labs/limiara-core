import {useEffect, useState} from 'react';
import {emptyExtensionFieldSet, type ExtensionFieldSet} from '../plugin-ui/extension-fields/model';
import type {ContactsClient} from './api';

export type ExtensionFieldsLoad = {status: 'loading' | 'ready' | 'error'; set: ExtensionFieldSet};

/**
 * Fields other plugins add to a resource and where this workspace places
 * them. A source without extensions, or a failure to load them, leaves the
 * page exactly as the host defines it.
 */
export function useExtensionFields(client: ContactsClient, resource: string, enabled: boolean): [ExtensionFieldsLoad, () => void] {
  const [load, setLoad] = useState<ExtensionFieldsLoad>(() => {
    if (!enabled || typeof client.loadExtensionFields !== 'function') return {status: 'ready', set: emptyExtensionFieldSet};
    const seeded = client.seeds?.extensionFields?.(resource);
    if (!seeded) return {status: 'loading', set: emptyExtensionFieldSet};
    return 'value' in seeded ? {status: 'ready', set: seeded.value} : {status: 'error', set: emptyExtensionFieldSet};
  });
  const [generation, setGeneration] = useState(0);
  useEffect(() => {
    if (!enabled || typeof client.loadExtensionFields !== 'function') return;
    const controller = new AbortController();
    client.loadExtensionFields(resource, controller.signal)
      .then(set => { if (!controller.signal.aborted) setLoad({status: 'ready', set}); })
      .catch(() => { if (!controller.signal.aborted) setLoad(current => ({status: 'error', set: current.set})); });
    return () => controller.abort();
  }, [client, enabled, generation, resource]);
  return [load, () => setGeneration(value => value + 1)];
}
