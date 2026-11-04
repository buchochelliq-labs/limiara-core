import {describe, expect, it} from 'vitest';
import {addableFields, addField, arrangementFrom, layoutFromArrangement, moveField, removeField} from './arrangement';
import {contactsPersonZones, loyaltyPersonFields} from './fixtures';

const zones = contactsPersonZones.filter(zone => zone.id === 'highlights' || zone.id === 'contact-information');
const hostFields = {
  highlights: [{key: 'organisation_id', label: 'Organisation'}, {key: 'email', label: 'Email'}],
  'contact-information': [{key: 'full_name', label: 'Full name'}],
};
const tier = 'example.loyalty.tier';
const points = 'example.loyalty.points';

describe('field arrangement', () => {
  it('interleaves placed fields with host fields by position', () => {
    const arrangement = arrangementFrom(zones, hostFields, [
      {field_id: tier, zone: 'highlights', position: 1},
      {field_id: points, zone: 'contact-information', position: 1000},
    ]);
    expect(arrangement.highlights).toEqual([
      {kind: 'host', key: 'organisation_id', label: 'Organisation'},
      {kind: 'field', id: tier},
      {kind: 'host', key: 'email', label: 'Email'},
    ]);
    expect(arrangement['contact-information']?.at(-1)).toEqual({kind: 'field', id: points});
  });

  it('moves fields past host fields, removes and adds them, and saves positions', () => {
    let arrangement = arrangementFrom(zones, hostFields, [{field_id: tier, zone: 'highlights', position: 1}]);
    arrangement = moveField(arrangement, 'highlights', tier, -1);
    expect(arrangement.highlights?.[0]).toEqual({kind: 'field', id: tier});
    expect(moveField(arrangement, 'highlights', tier, -1)).toBe(arrangement);
    arrangement = addField(arrangement, 'contact-information', points);
    expect(addField(arrangement, 'contact-information', points)).toBe(arrangement);
    expect(addableFields(arrangement, 'contact-information', loyaltyPersonFields).map(field => field.id)).toEqual(['example.loyalty.member_since', tier]);
    arrangement = removeField(arrangement, 'highlights', tier);
    expect(layoutFromArrangement(arrangement, loyaltyPersonFields)).toEqual({
      arranged: ['example.loyalty.member_since', points, tier],
      placements: [{field_id: points, zone: 'contact-information', position: 1}],
    });
  });
});
