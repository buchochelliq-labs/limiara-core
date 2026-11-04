<script setup lang="ts">
import DefaultTheme from "vitepress/theme";
import DocumentGovernance from "./DocumentGovernance.vue";
</script>

<template>
  <DefaultTheme.Layout>
    <template #doc-before>
      <DocumentGovernance />
    </template>
    <template #home-features-before>
      <div class="home-governance">
        <DocumentGovernance />
      </div>
    </template>
  </DefaultTheme.Layout>
</template>
