export type Person = {
  id: string;
  name: string;
  organisation: string;
  role: string;
  email: string;
  phone: string;
  label: 'Client' | 'Prospect' | 'Partner';
  tags: string[];
  updated: string;
  updatedOrder: number;
  version: number;
};

export const peopleFixtureWorkspaceId = '018f0000-0000-7000-8000-000000000001';

export const people: Person[] = [
  {id: 'amelia-hart', name: 'Amelia Hart', organisation: 'Ashdown Studio', role: 'Operations Director', email: 'amelia@ashdown.example', phone: '+44 20 7946 0182', label: 'Client', tags: ['priority'], updated: 'Today', updatedOrder: 8, version: 4},
  {id: 'arthur-bell', name: 'Arthur Bell', organisation: 'Bell & North', role: 'Managing Partner', email: 'arthur@bellnorth.example', phone: '+44 161 496 0401', label: 'Partner', tags: ['referral'], updated: 'Yesterday', updatedOrder: 7, version: 2},
  {id: 'jemima-clarke', name: 'Jemima Clarke', organisation: 'Field House Trust', role: 'Programme Lead', email: 'jemima@fieldhouse.example', phone: '+44 117 496 0220', label: 'Client', tags: ['charity'], updated: '2 days ago', updatedOrder: 6, version: 5},
  {id: 'jonah-okafor', name: 'Jonah Okafor', organisation: 'Morrow Engineering', role: 'Commercial Manager', email: 'jonah@morrow.example', phone: '+44 121 496 0134', label: 'Prospect', tags: ['engineering'], updated: '4 days ago', updatedOrder: 5, version: 3},
  {id: 'maya-patel', name: 'Maya Patel', organisation: 'Northstar Services', role: 'Service Manager', email: 'maya@northstar.example', phone: '+44 20 7946 0271', label: 'Client', tags: ['operations', 'priority'], updated: '6 days ago', updatedOrder: 4, version: 7},
  {id: 'simon-reeves', name: 'Simon Reeves', organisation: 'Riverview Property Group', role: 'Property Director', email: 'simon@riverview.example', phone: '+44 113 496 0912', label: 'Prospect', tags: ['property'], updated: '1 week ago', updatedOrder: 3, version: 2},
  {id: 'sophia-williams', name: 'Sophia Williams', organisation: 'Cedar & Row', role: 'Founder', email: 'sophia@cedarrow.example', phone: '+44 29 2018 0440', label: 'Partner', tags: ['design'], updated: '2 weeks ago', updatedOrder: 2, version: 6},
  {id: 'theo-wright', name: 'Theo Wright', organisation: 'Westmere Care', role: 'Head of Operations', email: 'theo@westmere.example', phone: '+44 141 496 0881', label: 'Client', tags: ['care'], updated: '3 weeks ago', updatedOrder: 1, version: 3},
];

export const organisationOptions = [
  {id: 'ashdown', name: 'Ashdown Studio'},
  {id: 'bell-north', name: 'Bell & North'},
  {id: 'cedar-row', name: 'Cedar & Row'},
  {id: 'field-house', name: 'Field House Trust'},
  {id: 'morrow', name: 'Morrow Engineering'},
  {id: 'northstar', name: 'Northstar Services'},
  {id: 'riverview', name: 'Riverview Property Group'},
  {id: 'westmere', name: 'Westmere Care'},
];

export function organisationIdFor(name: string) {
  return organisationOptions.find(item => item.name === name)?.id ?? '';
}
