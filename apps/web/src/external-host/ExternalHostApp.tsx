import {useEffect, useMemo, useRef, useState} from 'react';
import {Button} from '../components/ui';
import type {CallerSafePageResponse, HostProfile} from '../plugin-ui/contracts';
import {MetadataPageRenderer} from '../plugin-ui/renderer/MetadataPageRenderer';
import type {MetadataActionRequest, MetadataActionResult, MetadataInteractionAdapter, MetadataValues} from '../plugin-ui/renderer/interactions';
import './external-host.css';

export type ExternalRoute = {host: 'public-form' | 'portal'; slug: string};
export type ExternalTheme = 'system' | 'light' | 'dark' | 'contrast';
type ExternalLocale = 'en-GB' | 'fr-FR';

type ExternalPageDocument = CallerSafePageResponse & {
  messages?: Record<string, string>;
};

type SubmissionFailure = {
  code: 'unavailable' | 'invalid' | 'too-large' | 'rate-limited' | 'idempotency-conflict' | 'verification-required' | 'failed';
  correlation_id: string;
  retry: {values: MetadataValues};
};

type Copy = {
  loading: string;
  unavailableTitle: string;
  unavailableBody: string;
  retry: string;
  language: string;
  theme: string;
  themeSystem: string;
  themeLight: string;
  themeDark: string;
  themeContrast: string;
  confirmationTitle: string;
  confirmationBody: string;
  reference: string;
};

const copy: Record<ExternalLocale, Copy> = {
  'en-GB': {
    loading: 'Loading this page.',
    unavailableTitle: 'This page is unavailable',
    unavailableBody: 'The link may have expired or access may have changed. Check the link and try again.',
    retry: 'Try again',
    language: 'Language',
    theme: 'Colour scheme',
    themeSystem: 'Use device setting',
    themeLight: 'Light',
    themeDark: 'Dark',
    themeContrast: 'High contrast',
    confirmationTitle: 'Thank you',
    confirmationBody: 'Your response has been received.',
    reference: 'Reference',
  },
  'fr-FR': {
    loading: 'Chargement de cette page.',
    unavailableTitle: 'Cette page est indisponible',
    unavailableBody: 'Le lien a peut-\u00eatre expir\u00e9 ou l\u2019acc\u00e8s a chang\u00e9. V\u00e9rifiez le lien puis r\u00e9essayez.',
    retry: 'R\u00e9essayer',
    language: 'Langue',
    theme: 'Th\u00e8me de couleur',
    themeSystem: 'R\u00e9glage de l\u2019appareil',
    themeLight: 'Clair',
    themeDark: 'Sombre',
    themeContrast: 'Contraste \u00e9lev\u00e9',
    confirmationTitle: 'Merci',
    confirmationBody: 'Votre r\u00e9ponse a bien \u00e9t\u00e9 re\u00e7ue.',
    reference: 'R\u00e9f\u00e9rence',
  },
};

const slugPattern = /^[A-Za-z0-9_-]{32,96}$/;

export function parseExternalRoute(pathname: string): ExternalRoute | null {
  const match = pathname.match(/^\/(forms|portal)\/([^/]+)\/?$/);
  if (!match || !slugPattern.test(match[2] ?? '')) return null;
  return {host: match[1] === 'forms' ? 'public-form' : 'portal', slug: match[2]!};
}

export function isExternalRoutePath(pathname: string) {
  return /^\/(forms|portal)(?:\/|$)/.test(pathname);
}

function preferredLocale(search: string): ExternalLocale {
  const requested = new URLSearchParams(search).get('lang');
  if (requested?.toLowerCase().startsWith('fr')) return 'fr-FR';
  if (requested?.toLowerCase().startsWith('en')) return 'en-GB';
  return typeof navigator !== 'undefined' && navigator.language.toLowerCase().startsWith('fr') ? 'fr-FR' : 'en-GB';
}

function preferredTheme(search: string): ExternalTheme {
  const requested = new URLSearchParams(search).get('theme');
  if (requested === 'light' || requested === 'dark' || requested === 'contrast' || requested === 'system') return requested;
  if (typeof localStorage !== 'undefined') {
    const stored = localStorage.getItem('limiara.external.theme');
    if (stored === 'light' || stored === 'dark' || stored === 'contrast' || stored === 'system') return stored;
  }
  return 'system';
}

function opaqueId() {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID().replaceAll('-', '');
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}${Math.random().toString(36).slice(2)}`.padEnd(32, '0');
}

function csrfToken(slug: string) {
  const cookie = typeof document === 'undefined' ? '' : document.cookie.split(';').map(item => item.trim()).find(item => item.startsWith('limiara_csrf='))?.split('=')[1];
  if (cookie && /^[A-Za-z0-9_-]{32,128}$/.test(cookie)) return cookie;
  const token = opaqueId();
  if (typeof document !== 'undefined') {
    const secure = window.location.protocol === 'https:' ? '; Secure' : '';
    document.cookie = `limiara_csrf=${token}; Path=/api/v1/external/forms/${slug}; SameSite=Strict${secure}`;
  }
  return token;
}

function failureMessage(code: SubmissionFailure['code']) {
  if (code === 'invalid') return 'Check the highlighted fields and try again.';
  if (code === 'too-large') return 'The response is too large. Shorten it and try again.';
  if (code === 'rate-limited') return 'Too many attempts were made. Wait a moment and try again.';
  if (code === 'idempotency-conflict') return 'This response changed while it was being retried. Review it and submit again.';
  if (code === 'verification-required') return 'Complete the verification step and try again.';
  return 'The response could not be submitted. Try again without reloading the page.';
}

export async function submitPublicForm(
  route: ExternalRoute,
  request: MetadataActionRequest,
  idempotencyKey: string,
  fetcher: typeof fetch = fetch,
): Promise<{result: MetadataActionResult; submissionId?: string}> {
  const response = await fetcher(`/api/v1/external/forms/${encodeURIComponent(route.slug)}/submissions`, {
    method: 'POST',
    credentials: 'same-origin',
    headers: {
      'content-type': 'application/json',
      'idempotency-key': idempotencyKey,
      'x-csrf-token': csrfToken(route.slug),
      'x-request-id': opaqueId(),
    },
    body: JSON.stringify({payload: request.values}),
  });
  if (response.status === 202) {
    const accepted = await response.json() as {submission_id: string};
    return {result: {status: 'success'}, submissionId: accepted.submission_id};
  }
  const failure = await response.json().catch(() => null) as SubmissionFailure | null;
  if (failure?.code === 'invalid') return {result: {status: 'validation', formError: failureMessage(failure.code)}};
  return {
    result: {
      status: 'failure',
      category: failure?.code === 'idempotency-conflict' ? 'duplicate' : failure?.code === 'unavailable' ? 'unavailable' : 'network',
      message: failureMessage(failure?.code ?? 'failed'),
      values: failure?.retry.values,
    },
  };
}

export function ExternalHostFrame({
  route,
  locale,
  theme,
  document: pageDocument,
  status,
  submissionId,
  onLocale,
  onTheme,
  onRetry,
  adapter,
  formFactor = 'wide',
}: {
  route: ExternalRoute;
  locale: ExternalLocale;
  theme: ExternalTheme;
  document?: ExternalPageDocument;
  status: 'loading' | 'unavailable' | 'ready' | 'confirmed';
  submissionId?: string;
  onLocale: (locale: ExternalLocale) => void;
  onTheme: (theme: ExternalTheme) => void;
  onRetry: () => void;
  adapter?: MetadataInteractionAdapter;
  formFactor?: 'wide' | 'compact';
}) {
  const strings = copy[locale];
  return <div className="external-host" data-theme={theme}>
    <a className="external-host__skip" href="#external-content">Skip to content</a>
    <header className="external-host__bar">
      <span aria-label="Limiara" className="external-host__brand">Limiara</span>
      <div className="external-host__preferences">
        <label>{strings.language}<select aria-label={strings.language} onChange={event => onLocale(event.target.value as ExternalLocale)} value={locale}><option value="en-GB">English</option><option value="fr-FR">{'Fran\u00e7ais'}</option></select></label>
        <label>{strings.theme}<select aria-label={strings.theme} onChange={event => onTheme(event.target.value as ExternalTheme)} value={theme}><option value="system">{strings.themeSystem}</option><option value="light">{strings.themeLight}</option><option value="dark">{strings.themeDark}</option><option value="contrast">{strings.themeContrast}</option></select></label>
      </div>
    </header>
    <main aria-busy={status === 'loading'} className="external-host__content" id="external-content" tabIndex={-1}>
      {status === 'loading' && <p aria-live="polite" role="status">{strings.loading}</p>}
      {status === 'unavailable' && <section className="external-host__state" role="alert"><h1>{strings.unavailableTitle}</h1><p>{strings.unavailableBody}</p><Button onClick={onRetry}>{strings.retry}</Button></section>}
      {status === 'confirmed' && <section className="external-host__state external-host__state--success" role="status" tabIndex={-1}><h1>{strings.confirmationTitle}</h1><p>{strings.confirmationBody}</p>{submissionId && <p><strong>{strings.reference}:</strong> <code>{submissionId}</code></p>}</section>}
      {status === 'ready' && pageDocument && <MetadataPageRenderer
        embedded
        embeddedHeadingOffset={0}
        focusOnRender
        formFactor={formFactor}
        host={route.host as HostProfile}
        interactionAdapter={adapter}
        locale={locale}
        messages={pageDocument.messages}
        mode={route.host === 'public-form' ? 'wizard' : 'view'}
        page={pageDocument.page}
        resources={pageDocument.resources}
      />}
    </main>
    <footer className="external-host__footer">Powered by Limiara</footer>
  </div>;
}

export function ExternalHostApp({route}: {route: ExternalRoute}) {
  const [locale, setLocale] = useState<ExternalLocale>(() => preferredLocale(window.location.search));
  const [theme, setTheme] = useState<ExternalTheme>(() => preferredTheme(window.location.search));
  const [pageDocument, setPageDocument] = useState<ExternalPageDocument>();
  const [status, setStatus] = useState<'loading' | 'unavailable' | 'ready' | 'confirmed'>('loading');
  const [submissionId, setSubmissionId] = useState<string>();
  const [formFactor, setFormFactor] = useState<'wide' | 'compact'>(() => window.matchMedia('(max-width: 48rem)').matches ? 'compact' : 'wide');
  const [reload, setReload] = useState(0);
  const idempotency = useRef(opaqueId());

  useEffect(() => {
    window.document.documentElement.lang = locale;
  }, [locale]);
  useEffect(() => {
    localStorage.setItem('limiara.external.theme', theme);
  }, [theme]);
  useEffect(() => {
    const media = window.matchMedia('(max-width: 48rem)');
    const update = () => setFormFactor(media.matches ? 'compact' : 'wide');
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    setStatus('loading');
    const kind = route.host === 'public-form' ? 'forms' : 'portals';
    fetch(`/api/v1/external/${kind}/${encodeURIComponent(route.slug)}?locale=${encodeURIComponent(locale)}`, {credentials: 'same-origin', signal: controller.signal})
      .then(async response => {
        if (!response.ok) throw new Error('unavailable');
        return response.json() as Promise<ExternalPageDocument>;
      })
      .then(page => { setPageDocument(page); setStatus('ready'); })
      .catch(() => { if (!controller.signal.aborted) setStatus('unavailable'); });
    return () => controller.abort();
  }, [locale, reload, route.host, route.slug]);

  const adapter = useMemo<MetadataInteractionAdapter | undefined>(() => route.host === 'public-form' ? {
    execute: async request => {
      const outcome = await submitPublicForm(route, request, idempotency.current);
      if (outcome.submissionId) {
        setSubmissionId(outcome.submissionId);
        setStatus('confirmed');
        idempotency.current = opaqueId();
      }
      return outcome.result;
    },
  } : undefined, [route]);

  return <ExternalHostFrame adapter={adapter} document={pageDocument} formFactor={formFactor} locale={locale} onLocale={setLocale} onRetry={() => setReload(value => value + 1)} onTheme={setTheme} route={route} status={status} submissionId={submissionId} theme={theme} />;
}

export function ExternalUnavailableApp({host}: {host: ExternalRoute['host']}) {
  const [locale, setLocale] = useState<ExternalLocale>(() => preferredLocale(window.location.search));
  const [theme, setTheme] = useState<ExternalTheme>(() => preferredTheme(window.location.search));
  useEffect(() => { window.document.documentElement.lang = locale; }, [locale]);
  useEffect(() => { localStorage.setItem('limiara.external.theme', theme); }, [theme]);
  return <ExternalHostFrame locale={locale} onLocale={setLocale} onRetry={() => window.location.reload()} onTheme={setTheme} route={{host, slug: ''}} status="unavailable" theme={theme} />;
}
