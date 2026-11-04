import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import {MetadataRenderer, type ResolvedPage} from './MetadataRenderer';
import {peopleCollectionPage, personFormPage, personRecordPage, publicEnquiryPage} from './fixtures';

describe('metadata UI renderer spike', () => {
  it('renders create/edit fields from a resource-addressable form tree', () => {
    const markup = renderToStaticMarkup(<MetadataRenderer page={personFormPage} />);
    expect(markup).toContain('data-page-id="platform.contacts.person.form"');
    expect(markup).toContain('name="first_name"');
    expect(markup).toContain('name="service_tier__c"');
    expect(markup).toContain('Customer context');
    expect(markup).toContain('type="submit"');
  });

  it('renders a customer plugin card inside an explicit record-page slot', () => {
    const markup = renderToStaticMarkup(<MetadataRenderer page={personRecordPage} />);
    expect(markup).toContain('Maya Patel');
    expect(markup).toContain('Related information');
    expect(markup).toContain('Activity insights');
  });

  it('renders collection and public-form page kinds through the same grammar', () => {
    const collection = renderToStaticMarkup(<MetadataRenderer page={peopleCollectionPage} />);
    const publicForm = renderToStaticMarkup(<MetadataRenderer page={publicEnquiryPage} />);
    expect(collection).toContain('type="search"');
    expect(collection).toContain('aria-label="People"');
    expect(publicForm).toContain('Your name');
    expect(publicForm).toContain('Send enquiry');
  });

  it('contains an unknown component instead of crashing the page', () => {
    const page: ResolvedPage = {
      id: 'customer.unknown', definitionVersion: 1,
      root: {id: 'unknown', component: 'customer-arbitrary-react'},
    };
    const markup = renderToStaticMarkup(<MetadataRenderer page={page} />);
    expect(markup).toContain('role="alert"');
    expect(markup).toContain('Unsupported component: customer-arbitrary-react');
  });
});
