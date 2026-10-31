import { defineConfig } from "astro/config";

// GitHub Pages serves the project site under /cyberbrain/.
export default defineConfig({
  site: "https://bassprofressor-lab.github.io",
  base: "/cyberbrain",
  trailingSlash: "always",
  build: { format: "directory" },
  markdown: { shikiConfig: { theme: "github-dark-dimmed" } },
});
