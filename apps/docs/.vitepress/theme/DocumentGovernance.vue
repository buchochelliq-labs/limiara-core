<script setup lang="ts">
import { computed } from "vue";
import { useData } from "vitepress";
import inventory from "../../../../docs/inventory.json";
import ownership from "../../../../docs/owners.json";

type InventoryEntry = (typeof inventory.entries)[number];

const { page } = useData();
const byPath = new Map(inventory.entries.map((entry) => [entry.path, entry]));
const entry = computed<InventoryEntry | undefined>(() => byPath.get(page.value.relativePath.replaceAll("\\", "/")));
const owner = computed(() => entry.value ? ownership.owners[entry.value.owner as keyof typeof ownership.owners] : undefined);

const statusLabels: Record<string, string> = {
  shipped: "Shipped",
  experimental: "Experimental",
  planned: "Planned",
  mixed: "Mixed delivery status",
  historical: "Historical",
  "not-applicable": "Policy"
};

const statusDescriptions: Record<string, string> = {
  shipped: "Describes behavior backed by executable repository evidence.",
  experimental: "Describes a runnable proof or unstable contract; compatibility is not guaranteed.",
  planned: "Describes intended behavior that is not delivered yet.",
  mixed: "Combines delivered and planned behavior; the page distinguishes them explicitly.",
  historical: "Retained for decision or research context, not current product guidance.",
  "not-applicable": "Describes repository policy rather than product delivery."
};

const audienceLabels: Record<string, string> = {
  user: "users",
  administrator: "administrators",
  operator: "operators",
  contributor: "contributors",
  "plugin-developer": "plugin developers",
  architect: "architects",
  "security-reviewer": "security reviewers"
};

const audienceText = computed(() => entry.value?.audiences.map((value) => audienceLabels[value] ?? value).join(", "));
</script>

<template>
  <aside v-if="entry" class="doc-governance" :class="`status-${entry.productStatus}`" aria-label="Document status">
    <div class="doc-governance__status">
      <strong>{{ statusLabels[entry.productStatus] }}</strong>
      <span>{{ statusDescriptions[entry.productStatus] }}</span>
    </div>
    <dl>
      <div>
        <dt>Owner</dt>
        <dd>{{ owner?.name }}</dd>
      </div>
      <div>
        <dt>Audience</dt>
        <dd>{{ audienceText }}</dd>
      </div>
      <div v-if="entry.edition !== 'not-applicable'">
        <dt>Edition</dt>
        <dd>{{ entry.edition }}</dd>
      </div>
      <div>
        <dt>Version</dt>
        <dd>{{ entry.versioning }}</dd>
      </div>
    </dl>
    <p v-if="entry.sensitivity === 'security-design'" class="doc-governance__sensitivity">
      Public security design — publication rationale is recorded in the inventory.
    </p>
    <a href="/contributing/documentation#choose-the-audience-and-status-first">How status and ownership work</a>
  </aside>
</template>
