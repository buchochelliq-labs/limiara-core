import DefaultTheme from "vitepress/theme";
import "./custom.css";
import Layout from "./Layout.vue";

export default {
  ...DefaultTheme,
  Layout,
  enhanceApp({ router }) {
    if (typeof window === "undefined") return;

    const syncHomeLandmark = () => {
      window.requestAnimationFrame(() => {
        const content = document.querySelector("#VPContent");
        if (!content) return;
        if (document.querySelector(".VPHome")) content.setAttribute("role", "main");
        else content.removeAttribute("role");
      });
    };

    router.onAfterRouteChanged = syncHomeLandmark;
    window.addEventListener("load", syncHomeLandmark, { once: true });
    syncHomeLandmark();
  }
};
