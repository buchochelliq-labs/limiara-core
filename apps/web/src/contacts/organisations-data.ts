export type Organisation = {
  id: string;
  name: string;
  sector: string;
  website: string;
  phone: string;
  address: string;
  accountStatus: 'Active' | 'Prospect' | 'Partner';
  serviceRegion: 'London' | 'North' | 'Wales' | 'Scotland' | 'South West';
  updated: string;
  updatedOrder: number;
  version: number;
};

export const organisations: Organisation[] = [
  {id: 'ashdown', name: 'Ashdown Studio', sector: 'Design', website: 'https://ashdown.example', phone: '+44 20 7946 0101', address: '18 Ashdown Road, London', accountStatus: 'Active', serviceRegion: 'London', updated: 'Today', updatedOrder: 8, version: 4},
  {id: 'bell-north', name: 'Bell & North', sector: 'Professional services', website: 'https://bellnorth.example', phone: '+44 161 496 0102', address: '4 King Street, Manchester', accountStatus: 'Partner', serviceRegion: 'North', updated: 'Yesterday', updatedOrder: 7, version: 2},
  {id: 'field-house', name: 'Field House Trust', sector: 'Charity', website: 'https://fieldhouse.example', phone: '+44 117 496 0103', address: '27 College Green, Bristol', accountStatus: 'Active', serviceRegion: 'South West', updated: '2 days ago', updatedOrder: 6, version: 5},
  {id: 'morrow', name: 'Morrow Engineering', sector: 'Engineering', website: 'https://morrow.example', phone: '+44 121 496 0104', address: '81 Newhall Street, Birmingham', accountStatus: 'Prospect', serviceRegion: 'North', updated: '4 days ago', updatedOrder: 5, version: 3},
  {id: 'northstar', name: 'Northstar Services', sector: 'Business services', website: 'https://northstar.example', phone: '+44 20 7946 0105', address: '44 Farringdon Street, London', accountStatus: 'Active', serviceRegion: 'London', updated: '6 days ago', updatedOrder: 4, version: 7},
  {id: 'riverview', name: 'Riverview Property Group', sector: 'Property', website: 'https://riverview.example', phone: '+44 113 496 0106', address: '12 Calls Landing, Leeds', accountStatus: 'Prospect', serviceRegion: 'North', updated: '1 week ago', updatedOrder: 3, version: 2},
  {id: 'cedar-row', name: 'Cedar & Row', sector: 'Design', website: 'https://cedarrow.example', phone: '+44 29 2018 0107', address: '6 Cathedral Road, Cardiff', accountStatus: 'Partner', serviceRegion: 'Wales', updated: '2 weeks ago', updatedOrder: 2, version: 6},
  {id: 'westmere', name: 'Westmere Care', sector: 'Healthcare', website: 'https://westmere.example', phone: '+44 141 496 0108', address: '35 Renfield Street, Glasgow', accountStatus: 'Active', serviceRegion: 'Scotland', updated: '3 weeks ago', updatedOrder: 1, version: 3},
];
