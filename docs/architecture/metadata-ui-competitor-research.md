# Metadata-driven UI competitor research

Research snapshot: 2026-07-15. Primary product/developer documentation was used.
The goal is to select durable platform patterns, not reproduce any competitor.

| Platform | Evidence | Adopt | Adapt or reject |
| --- | --- | --- | --- |
| Salesforce | [UI API](https://developer.salesforce.com/blogs/2018/01/introduction-salesforce-ui-api) returns data and layout metadata with permissions and form factor; [Dynamic Forms](https://developer.salesforce.com/files/accessibility/lightning_app_builder/get-started-with-dynamic-forms-lab/index.html) composes individual fields and sections with visibility rules. | Runtime metadata shaped for the caller; fields, sections, actions, modes, and device context. | Keep typed plugin resources instead of a global Salesforce-style object engine. |
| Dynamics / Dataverse | [Model-driven forms](https://learn.microsoft.com/en-us/power-apps/maker/model-driven-apps/create-design-forms) provide several form types; [PCF](https://learn.microsoft.com/en-us/power-apps/developer/component-framework/overview) supplies code components; [form ALM](https://learn.microsoft.com/en-us/power-platform/alm/form-alm) supports differential layers. | Reusable components, distinct modes, publish lifecycle, and layered customisation. | Use JSON contracts and stable IDs rather than exposing FormXML as the public authoring model. |
| ServiceNow | [UI Builder components](https://www.servicenow.com/docs/r/application-development/ui-builder/work-components.html) combine layout, components, bindings, visibility, and events; [data resources](https://www.servicenow.com/docs/r/application-development/ui-builder/data-resources.html) separate data acquisition from presentation. | Explicit data sources, bindings, events, reusable components, and audience variants. | Reject arbitrary page scripts and per-node CSS in ordinary metadata. |
| HubSpot | [UI extensions](https://developers.hubspot.com/docs/apps/developer-platform/add-features/ui-extensions/overview) target named locations, use a host component library, and run custom React in a secure sandbox. | Named extension locations, controlled SDK, passed context, and isolated custom UI. | Declarative host rendering is the default; the sandbox is an escape hatch rather than the main form system. |
| Odoo | [View records](https://www.odoo.com/documentation/19.0/developer/reference/user_interface/view_records.html) define form, list, kanban, calendar, graph, pivot, and other views and apply ordered inherited patches. | Multiple views over a resource and additive inherited customisation. | Replace brittle XPath targets and executable Python expressions with stable IDs and a bounded AST. |
| Zoho CRM | [Page layouts](https://help.zoho.com/portal/en/kb/crm/customize-crm-account/customizing-page-layouts/articles/create-page-layouts) vary by profile and record layout; [widgets](https://www.zoho.com/crm/developer/docs/widgets/) provide custom embedded UI. | Audience variants, conditional layouts, quick-create/detail forms, and an embed escape hatch. | Do not reproduce inconsistent rule support across entry points; the same authoritative rules apply to every operation. |
| Pipedrive | [App extensions](https://pipedrive.readme.io/docs/app-extensions) offer predefined JSON panels/modals and iframe-based custom UI in named locations. | Simple declarative surfaces plus an isolated bespoke surface. | Generalize beyond CRM detail panels so the contract also serves portals, settings, scheduling, and future service operations. |
| SAP Sales Cloud | The [extension guide](https://help.sap.com/doc/82be840a106e4dcfbbb20940d471b1a6/CLOUD/en-US/ExtensionGuide.pdf) supports adaptation and mandatory/read-only extension-field behavior. | Governed administrator adaptation. | Avoid product-specific extension-field limits in the platform contract. |
| Oracle Fusion | [Application Composer](https://docs.oracle.com/cd/G55130_01/trans/G51907-01/configuring-applications-using-application-composer.pdf) creates and modifies application pages and layouts for standard and custom objects. | Separate design-time composition from published runtime pages. | Avoid coupling the renderer to a single enterprise object catalogue. |

## Cross-platform conclusions

1. Resource/field metadata, page composition, data operations, and custom code are
   separate concerns even when a product's tooling presents them together.
2. Pages need stable component trees, reusable fragments, variants, explicit data
   binding, actions, and lifecycle—not only lists of fields.
3. Layered patches inherit vendor/plugin improvements better than full tenant
   clones, but only when anchors and conflicts are deterministic and diagnosable.
4. Permission-filtered server metadata is safer than sending everything and
   relying on client visibility rules.
5. A bounded host component library provides consistency and accessibility; rich
   customer code requires origin isolation and mediated capabilities.
6. Limiara should support diverse view types while retaining typed plugin-owned
   persistence rather than creating a lowest-common-denominator object engine.
