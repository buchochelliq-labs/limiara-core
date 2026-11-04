import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import {Plus} from '@phosphor-icons/react';
import {Alert, Button, IconButton, Select, Skeleton, TextField} from './primitives';

describe('accessible UI primitives', () => {
  it('exposes loading and disabled button state', () => {
    const markup = renderToStaticMarkup(<Button loading>Saving</Button>);
    expect(markup).toContain('aria-busy="true"');
    expect(markup).toContain('disabled=""');
    expect(markup).toContain('Saving');
  });

  it('requires a readable label for icon-only buttons', () => {
    const markup = renderToStaticMarkup(<IconButton label="Add person" icon={<Plus />} />);
    expect(markup).toContain('aria-label="Add person"');
    expect(markup).toContain('title="Add person"');
  });

  it('associates field descriptions and errors with their controls', () => {
    const markup = renderToStaticMarkup(<TextField id="email" label="Email" description="Use a work address." error="Enter a valid email." />);
    expect(markup).toContain('for="email"');
    expect(markup).toContain('aria-invalid="true"');
    expect(markup).toContain('aria-describedby="email-description email-error"');
    expect(markup).toContain('id="email-error"');
  });

  it('associates select errors without replacing the label', () => {
    const markup = renderToStaticMarkup(<Select id="organisation" label="Organisation" error="Choose an organisation"><option>Northstar</option></Select>);
    expect(markup).toContain('for="organisation"');
    expect(markup).toContain('aria-describedby="organisation-error"');
  });

  it('uses assertive semantics only for danger alerts', () => {
    expect(renderToStaticMarkup(<Alert variant="danger" title="Save failed" />)).toContain('role="alert"');
    expect(renderToStaticMarkup(<Alert variant="success" title="Saved" />)).toContain('role="status"');
  });

  it('names loading skeletons for assistive technology', () => {
    const markup = renderToStaticMarkup(<Skeleton label="Loading people" />);
    expect(markup).toContain('role="status"');
    expect(markup).toContain('aria-label="Loading people"');
  });
});
