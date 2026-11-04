import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import {FieldArrangement} from './FieldArrangement';
import {contactsPersonZones, loyaltyPersonFields} from './fixtures';

const noop = () => undefined;
const save = async () => ({status: 'saved' as const});

function render(props: Partial<Parameters<typeof FieldArrangement>[0]> = {}) {
  return renderToStaticMarkup(<FieldArrangement
    recordNoun="person"
    status="ready"
    zones={contactsPersonZones}
    fields={loyaltyPersonFields}
    placements={[{field_id: 'example.loyalty.tier', zone: 'highlights', position: 1}]}
    hostFields={{highlights: [{key: 'organisation_id', label: 'Organisation'}, {key: 'email', label: 'Email'}]}}
    onSave={save}
    onClose={noop}
    {...props}
  />);
}

describe('arrange fields editor', () => {
  it('groups zones by page area and shows host fields as fixed anchors', () => {
    const markup = render();
    expect(markup).toContain('<h2 class="field-arrangement__title"');
    expect(markup).toContain('>Record page</h3>');
    expect(markup).toContain('>Edit form</h3>');
    expect(markup).toContain('>Lists</h3>');
    expect(markup).toContain('<legend>Highlights</legend>');
    expect(markup).toMatch(/Organisation<\/span><\/li><li class="field-arrangement__item"[^>]*><span class="field-arrangement__label">Loyalty tier/);
    expect(markup).toContain('aria-label="Move Loyalty tier up in Highlights"');
    expect(markup).toContain('aria-label="Remove Loyalty tier from Highlights"');
    expect(markup).toContain('Add a field to People list columns');
    expect(markup).toContain('Save arrangement');
  });

  it('shows loading, failure and empty states', () => {
    expect(render({status: 'loading'})).toContain('aria-busy="true"');
    expect(render({status: 'error'})).toContain('The arrangement could not be loaded');
    expect(render({fields: [], placements: []})).toContain('No fields to arrange');
  });

  it('renders labels from other plugins as text', () => {
    const hostile = [{...loyaltyPersonFields[2]!, label: '<img src=x onerror=alert(1)>'}];
    const markup = render({fields: hostile});
    expect(markup).not.toContain('<img src=x');
    expect(markup).toContain('&lt;img src=x onerror=alert(1)&gt;');
  });
});
