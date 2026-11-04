// The Contacts fields already in each zone where administrators may place
// fields other plugins add to people. The arrangement editor shows them as
// fixed anchors so a field can go before, between or after them.

import peopleDirectoryDocument from '../../../../plugins/contacts/ui/pages/people-directory.json';
import personDetailDocument from '../../../../plugins/contacts/ui/pages/person-detail.json';
import personFormDocument from '../../../../plugins/contacts/ui/pages/person-form.json';
import personResourceDocument from '../../../../plugins/contacts/ui/resources/person.json';
import type {HostField} from '../plugin-ui/extension-fields/arrangement';
import type {FieldZone} from '../plugin-ui/extension-fields/model';

type Node = {id: string; component: string; props?: Record<string, unknown>; children?: Node[]};

const pages: Record<string, Node> = {
  'platform.contacts.person-detail': (personDetailDocument as {root: Node}).root,
  'platform.contacts.person-form': (personFormDocument as {root: Node}).root,
  'platform.contacts.people-directory': (peopleDirectoryDocument as {root: Node}).root,
};

const resourceLabels = new Map(((personResourceDocument as {fields?: Array<{api_name: string; label: string}>}).fields ?? []).map(field => [field.api_name, field.label]));

function find(node: Node, id: string): Node | undefined {
  if (node.id === id) return node;
  for (const child of node.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return undefined;
}

function hostFieldsOf(zone: FieldZone): HostField[] {
  const root = pages[zone.page];
  const node = root ? find(root, zone.node) : undefined;
  if (!node) return [];
  if (zone.kind === 'form') {
    return (node.children ?? []).flatMap(child => {
      const field = child.props?.field;
      return typeof field === 'string' ? [{key: field, label: resourceLabels.get(field) ?? field}] : [];
    });
  }
  const entries = Array.isArray(node.props?.fields) ? node.props.fields : [];
  return entries.flatMap(entry => {
    const value = entry as {key?: unknown; label?: unknown};
    return typeof value.key === 'string' ? [{key: value.key, label: typeof value.label === 'string' ? value.label : value.key}] : [];
  });
}

/** Host fields per zone id, for the zones the Contacts pages declare. */
export function contactsZoneHostFields(zones: readonly FieldZone[]): Record<string, HostField[]> {
  return Object.fromEntries(zones.map(zone => [zone.id, hostFieldsOf(zone)]));
}
