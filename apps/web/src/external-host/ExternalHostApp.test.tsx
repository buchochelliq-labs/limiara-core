import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import {rendererFixture, rendererMessages} from '../plugin-ui/renderer/fixtures';
import type {MetadataActionRequest} from '../plugin-ui/renderer/interactions';
import {ExternalHostFrame, isExternalRoutePath, parseExternalRoute, submitPublicForm} from './ExternalHostApp';

const slug = 'S7m_t10aLmN3vW5uQ8zC2bD9fH4jK6sX';

describe('external metadata host', () => {
  it('recognises only opaque public and portal routes', () => {
    expect(parseExternalRoute(`/forms/${slug}`)).toEqual({host: 'public-form', slug});
    expect(parseExternalRoute(`/portal/${slug}/`)).toEqual({host: 'portal', slug});
    expect(parseExternalRoute('/forms/short')).toBeNull();
    expect(isExternalRoutePath('/forms/short')).toBe(true);
    expect(isExternalRoutePath('/portal')).toBe(true);
    expect(parseExternalRoute(`/forms/${slug}/private`)).toBeNull();
    expect(parseExternalRoute(`/workspaces/tenant-a/forms/${slug}`)).toBeNull();
  });

  it('renders localized, themed, non-enumerating states with landmark and preference controls', () => {
    const unavailable = renderToStaticMarkup(<ExternalHostFrame
      locale="fr-FR"
      onLocale={() => undefined}
      onRetry={() => undefined}
      onTheme={() => undefined}
      route={{host: 'public-form', slug}}
      status="unavailable"
      theme="dark"
    />);
    expect(unavailable).toContain('data-theme="dark"');
    expect(unavailable).toContain('href="#external-content"');
    expect(unavailable).toContain('Cette page est indisponible');
    expect(unavailable).toContain('aria-label="Langue"');
    expect(unavailable).toContain('role="alert"');
    expect(unavailable).not.toContain(slug);
    expect(unavailable).not.toContain('tenant');

    const loading = renderToStaticMarkup(<ExternalHostFrame locale="en-GB" onLocale={() => undefined} onRetry={() => undefined} onTheme={() => undefined} route={{host: 'portal', slug}} status="loading" theme="system" />);
    expect(loading).toContain('aria-busy="true"');
    expect(loading).toContain('role="status"');
  });

  it('reuses the restricted metadata renderer for ready and confirmation states', () => {
    const publicPage = {
      ...rendererFixture,
      root: {
        id: 'public-form',
        component: 'platform.ui.form',
        props: {label: 'Contact form'},
        children: [{
          id: 'public-steps',
          component: 'platform.ui.wizard',
          props: {label: 'Contact form steps'},
          children: [
            {id: 'step-one', component: 'platform.ui.text', props: {text: 'First step'}},
            {id: 'step-two', component: 'platform.ui.text', props: {text: 'Second step'}},
          ],
        }],
      },
    };
    const ready = renderToStaticMarkup(<ExternalHostFrame
      document={{page: publicPage, resources: [], messages: rendererMessages}}
      formFactor="compact"
      locale="en-GB"
      onLocale={() => undefined}
      onRetry={() => undefined}
      onTheme={() => undefined}
      route={{host: 'public-form', slug}}
      status="ready"
      theme="contrast"
    />);
    expect(ready).toContain('data-host="public-form"');
    expect(ready).toContain('metadata-renderer--compact');
    expect(ready).toContain('meta-form--wizard');
    expect(ready).toContain('Step 1 of 2');
    expect(ready).toContain('First step');
    expect(ready).not.toContain('Second step');
    expect(ready).toContain('data-theme="contrast"');

    const confirmed = renderToStaticMarkup(<ExternalHostFrame locale="en-GB" onLocale={() => undefined} onRetry={() => undefined} onTheme={() => undefined} route={{host: 'public-form', slug}} status="confirmed" submissionId="submission-1" theme="light" />);
    expect(confirmed).toContain('Thank you');
    expect(confirmed).toContain('submission-1');
    expect(confirmed).toContain('role="status"');
  });

  it('submits only field values with opaque transport controls and maps safe retry state', async () => {
    const request: MetadataActionRequest = {
      action: {id: 'submit-contact', kind: 'resource-operation', target: 'platform.contacts.person.create'},
      values: {email: 'person@example.test'},
      mode: 'wizard',
    };
    const acceptedFetch = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body));
      expect(body).toEqual({payload: {email: 'person@example.test'}});
      expect(JSON.stringify(body)).not.toContain('platform.contacts.person');
      expect(new Headers(init?.headers).get('idempotency-key')).toBe('request-0000000001');
      return new Response(JSON.stringify({submission_id: 'submission-1'}), {status: 202, headers: {'content-type': 'application/json'}});
    });
    const accepted = await submitPublicForm({host: 'public-form', slug}, request, 'request-0000000001', acceptedFetch as typeof fetch);
    expect(accepted).toEqual({result: {status: 'success'}, submissionId: 'submission-1'});

    const failedFetch = vi.fn(async () => new Response(JSON.stringify({code: 'failed', correlation_id: 'request-correlation-1', retry: {values: {email: 'person@example.test'}}}), {status: 503, headers: {'content-type': 'application/json'}}));
    const failed = await submitPublicForm({host: 'public-form', slug}, request, 'request-0000000002', failedFetch as typeof fetch);
    expect(failed.result).toMatchObject({status: 'failure', values: {email: 'person@example.test'}});
    expect(failed.result).not.toHaveProperty('audit');
  });
});
