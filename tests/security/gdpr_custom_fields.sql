-- GDPR metadata checks for custom fields.
-- Sensitive fields should require encryption metadata before activation.
INSERT INTO p_contacts.custom_field_definitions (
  tenant_id, id, applies_to, api_name, label, field_type, data_classification, encrypted, purpose
)
VALUES (
  '018f0000-0000-7000-8000-000000000001',
  '018f3000-0000-7000-8000-000000000002',
  'person',
  'medical_notes__c',
  'Medical notes',
  'textarea',
  'sensitive',
  true,
  'Field service accessibility and safety planning'
);
