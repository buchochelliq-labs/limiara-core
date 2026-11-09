import inventory from "../inventory.json";

const preview = process.env.DOCS_DEPLOYMENT_ENV === "preview";
const canonicalOrigin = "https://docs.limiara.org";

export default {
  srcDir: "../../docs",
  srcExclude: inventory.entries.filter((entry) => entry.publication !== "public").map((entry) => entry.path),
  publicDir: "../../docs/public",
  title: "Limiara",
  description: "Documentation for the open, composable Limiara business platform.",
  lang: "en-GB",
  cleanUrls: true,
  lastUpdated: true,
  sitemap: { hostname: canonicalOrigin },
  head: [
    ["meta", { name: "theme-color", content: "#16423c" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:site_name", content: "Limiara documentation" }],
    ["meta", { property: "og:title", content: "Limiara documentation" }],
    ["meta", { property: "og:description", content: "Documentation for the open, composable Limiara business platform." }],
    ["meta", { property: "og:url", content: canonicalOrigin }],
    ["meta", { name: "twitter:card", content: "summary" }],
    ...(preview ? [["meta", { name: "robots", content: "noindex, nofollow" }]] : [])
  ],
  themeConfig: {
    logo: "/architecture-overview.svg",
    siteTitle: "Limiara docs",
    nav: [
      {
        text: "Product",
        items: [
          { text: "Overview and concepts", link: "/product/overview" },
          { text: "Administration", link: "/admin/" },
          { text: "Roadmap", link: "/roadmap/README" }
        ]
      },
      {
        text: "Build",
        items: [
          { text: "Quick start", link: "/guide/getting-started" },
          { text: "Plugin development", link: "/plugins/native-plugin" },
          { text: "API reference", link: "/api/reference" },
          { text: "Events", link: "/api/events" },
          { text: "Architecture", link: "/architecture/open-core" }
        ]
      },
      {
        text: "Operate",
        items: [
          { text: "Operations", link: "/operations/migrations" },
          { text: "Security and privacy", link: "/security/gdpr-and-privacy" },
          { text: "Support", link: "/support/" }
        ]
      },
      {
        text: "Contribute",
        items: [
          { text: "Documentation", link: "/contributing/documentation" },
          { text: "Quality gates", link: "/guide/documentation-quality" },
          { text: "Releases and versions", link: "/releases/" }
        ]
      }
    ],
    sidebar: {
      "/guide/": [
        {
          text: "Guide",
          items: [
            { text: "Get started", link: "/guide/getting-started" },
            { text: "Documentation workflow", link: "/guide/documentation-workflow" },
            { text: "Quality gates", link: "/guide/documentation-quality" }
          ]
        }
      ],
      "/api/": [
        {
          text: "API and events",
          items: [
            { text: "API reference proof", link: "/api/reference" },
            { text: "Events reference status", link: "/api/events" },
            { text: "Objects API", link: "/api/objects" }
          ]
        }
      ],
      "/product/": [
        {
          text: "Product",
          items: [
            { text: "Overview", link: "/product/overview" },
            { text: "Application shell", link: "/product/application-shell" },
            { text: "UI foundations", link: "/product/ui-foundations" },
            { text: "Contacts UX benchmark", link: "/product/contacts-ux-benchmark" }
          ]
        }
      ],
      "/admin/": [
        { text: "Administration", items: [{ text: "Administration status", link: "/admin/" }] }
      ],
      "/architecture/": [
        {
          text: "Architecture",
          collapsed: false,
          items: [
            { text: "Open-core boundary", link: "/architecture/open-core" },
            { text: "Service plugins", link: "/architecture/service-plugins" },
            { text: "Metadata-driven UI", link: "/architecture/metadata-driven-ui" },
            { text: "Metadata resolution", link: "/architecture/metadata-resolution" },
            { text: "Metadata delivery", link: "/architecture/metadata-delivery" },
            { text: "Metadata interactions", link: "/architecture/metadata-interactions" },
            { text: "Plugin UI contracts", link: "/architecture/plugin-ui-contracts" },
            { text: "Persistence", link: "/architecture/persistence-and-indexing" },
            { text: "Customisation", link: "/architecture/customisation-model" },
            { text: "Public IDs", link: "/architecture/id-design" },
            { text: "OData strategy", link: "/architecture/odata" }
          ]
        }
      ],
      "/adr/": [
        {
          text: "Architecture decisions",
          collapsed: false,
          items: [
            { text: "Modular monolith", link: "/adr/0001-modular-monolith-first" },
            { text: "Plugin execution", link: "/adr/0002-plugin-execution-modes" },
            { text: "PostgreSQL tenancy", link: "/adr/0003-postgresql-rls-tenancy" },
            { text: "Outbox and events", link: "/adr/0004-transactional-outbox-and-events" },
            { text: "UI extension security", link: "/adr/0005-ui-extension-security" },
            { text: "Open-core entitlements", link: "/adr/0006-entitlements-and-open-core-boundary" },
            { text: "Control/data planes", link: "/adr/0007-control-plane-and-data-plane" },
            { text: "Documentation platform", link: "/adr/0008-vitepress-documentation-platform" },
            { text: "Plugin worker contributions", link: "/adr/0009-plugin-worker-contributions" },
            { text: "Plugin execution context", link: "/adr/0010-plugin-execution-context" },
            { text: "Platform UX direction", link: "/adr/0011-platform-ux-direction" },
            { text: "Plugins extending plugins", link: "/adr/0012-plugin-extensions" },
            { text: "Swappable storage backends", link: "/adr/0013-swappable-storage-backends" },
            { text: "Standard record fields", link: "/adr/0014-standard-record-fields" },
            { text: "Metadata-driven objects", link: "/adr/0015-metadata-driven-objects" },
            { text: "Record ownership and sharing", link: "/adr/0016-record-sharing" },
            { text: "Field history tracking", link: "/adr/0017-field-history" },
            { text: "Change data capture and sync", link: "/adr/0018-change-data-capture" },
            { text: "Record query language", link: "/adr/0019-record-query-language" },
            { text: "Global search", link: "/adr/0020-global-search" },
            { text: "Formulas and validation rules", link: "/adr/0021-formulas-and-validation-rules" },
            { text: "Record-triggered automation", link: "/adr/0022-record-triggered-automation" },
            { text: "Integration APIs", link: "/adr/0023-integration-apis" },
            { text: "Limiara on the phone", link: "/adr/0024-limiara-on-device" },
            { text: "Master-detail relationships", link: "/adr/0025-master-detail-relationships" },
            { text: "Roll-up summary fields", link: "/adr/0026-rollup-summary-fields" },
            { text: "Saved reports", link: "/adr/0027-saved-reports" },
            { text: "Related lists and lookup filters", link: "/adr/0028-related-lists-and-lookup-filters" },
            { text: "Device sign-in and revocation", link: "/adr/0029-device-sign-in" },
            { text: "Content-free push notifications", link: "/adr/0030-push-notifications" },
            { text: "Record ids assigned by devices", link: "/adr/0031-device-assigned-record-ids" },
            { text: "Quick actions and global New", link: "/adr/0032-quick-actions" },
            { text: "Activities", link: "/adr/0033-activities" }
          ]
        }
      ],
      "/plugins/": [
        {
          text: "Plugins",
          items: [
            { text: "Native plugins", link: "/plugins/native-plugin" },
            { text: "Extending other plugins", link: "/plugins/extensions" },
            { text: "Wasm contract", link: "/plugins/wasm" },
            { text: "Supply chain", link: "/plugins/supply-chain" }
          ]
        }
      ],
      "/operations/": [
        {
          text: "Operations",
          items: [
            { text: "Runtime configuration", link: "/operations/configuration" },
            { text: "Local startup", link: "/operations/local-startup" },
            { text: "First boot and recovery", link: "/operations/first-boot-and-recovery" },
            { text: "Local identity", link: "/operations/local-identity" },
            { text: "Migrations", link: "/operations/migrations" },
            { text: "Backup and restore", link: "/operations/backup-restore" },
            { text: "Documentation publishing", link: "/operations/docs-publishing" }
          ]
        }
      ],
      "/security/": [
        { text: "Security and privacy", items: [{ text: "GDPR and privacy", link: "/security/gdpr-and-privacy" }] }
      ],
      "/threat-model/": [
        { text: "Threat models", items: [{ text: "Phase 1", link: "/threat-model/phase-1" }] }
      ],
      "/contributing/": [
        { text: "Contribute", items: [{ text: "Documentation", link: "/contributing/documentation" }] }
      ],
      "/support/": [
        { text: "Support", items: [{ text: "Issue reporting", link: "/support/" }] }
      ],
      "/releases/": [
        { text: "Releases", items: [{ text: "Versions and releases", link: "/releases/" }] }
      ],
      "/roadmap/": [
        { text: "Roadmap", items: [{ text: "Active roadmap", link: "/roadmap/README" }] }
      ]
    },
    search: { provider: "local" },
    editLink: {
      pattern: "https://github.com/buchochelliq-labs/limiara-core/edit/main/docs/:path",
      text: "Edit this page on GitHub"
    },
    socialLinks: [
      { icon: "github", link: "https://github.com/buchochelliq-labs/limiara-core" }
    ],
    footer: {
      message: "Repository-owned documentation for the Limiara community.",
      copyright: "Limiara contributors"
    }
  }
};
