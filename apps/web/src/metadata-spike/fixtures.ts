import type {ResolvedPage} from './MetadataRenderer';

export const personFormPage: ResolvedPage = {
  id: 'platform.contacts.person.form',
  definitionVersion: 1,
  root: {
    id: 'person-form', component: 'form', children: [
      {id: 'identity', component: 'section', props: {title: 'Identity'}, children: [
        {id: 'first-name', component: 'field', props: {field: 'first_name', label: 'First name'}},
        {id: 'last-name', component: 'field', props: {field: 'last_name', label: 'Last name'}},
        {id: 'email', component: 'field', props: {field: 'email', label: 'Email'}},
      ]},
      {id: 'customer-context', component: 'section', props: {title: 'Customer context'}, children: [
        {id: 'service-tier', component: 'field', props: {field: 'service_tier__c', label: 'Service tier'}},
      ]},
      {id: 'form-actions', component: 'action-bar'},
    ],
  },
};

export const personRecordPage: ResolvedPage = {
  id: 'platform.contacts.person.record',
  definitionVersion: 1,
  root: {id: 'person-record', component: 'page', children: [
    {id: 'person-summary', component: 'record-summary', props: {title: 'Maya Patel'}},
    {id: 'person-main', component: 'section', props: {title: 'Details'}},
    {id: 'person-sidebar', component: 'slot', props: {label: 'Related information'}, children: [
      {id: 'activity-insights', component: 'card', props: {title: 'Activity insights'}},
    ]},
  ]},
};

export const peopleCollectionPage: ResolvedPage = {
  id: 'platform.contacts.people.collection',
  definitionVersion: 1,
  root: {id: 'people-page', component: 'page', children: [
    {id: 'people-search', component: 'search', props: {queryParam: 'q'}},
    {id: 'people-results', component: 'collection', props: {label: 'People'}},
  ]},
};

export const publicEnquiryPage: ResolvedPage = {
  id: 'platform.contacts.public.enquiry',
  definitionVersion: 1,
  root: {id: 'public-enquiry', component: 'form', children: [
    {id: 'public-name', component: 'field', props: {field: 'first_name', label: 'Your name'}},
    {id: 'public-email', component: 'field', props: {field: 'email', label: 'Email'}},
    {id: 'public-submit', component: 'action', props: {action: 'submit', label: 'Send enquiry'}},
  ]},
};
