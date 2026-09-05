import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { createHash } from "node:crypto";
import { fileURLToPath, URL } from "node:url";

/**
 * Production-only Content-Security-Policy.
 *
 * The page must not be able to reach anything but its own origin (SPEC §12.1: the UI is
 * served from the binary and fetches no external asset). The only inline script is the
 * theme bootstrap in index.html, which is allow-listed by hash rather than 'unsafe-inline'.
 * Not applied in dev because Vite's HMR preamble is inline and unhashed.
 *
 * `cyberbrain serve` should also send this as a real header (plus `frame-ancestors 'none'`,
 * which browsers ignore in a meta tag); the meta tag is belt and braces.
 */
function csp(): Plugin {
  return {
    name: "cyberbrain-csp",
    apply: "build",
    transformIndexHtml(html) {
      const hashes: string[] = [];
      for (const m of html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)) {
        hashes.push("'sha256-" + createHash("sha256").update(m[1] ?? "").digest("base64") + "'");
      }
      const policy = [
        "default-src 'none'",
        `script-src 'self' ${hashes.join(" ")}`.trim(),
        "style-src 'self' 'unsafe-inline'", // Tailwind emits a stylesheet; React sets a few inline styles
        "img-src 'self' data:",
        "font-src 'self'",
        "connect-src 'self'",
        "manifest-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'none'",
        // frame-ancestors is ignored in a <meta> CSP; `cyberbrain serve` must send it as a header.
      ].join("; ");
      return {
        html,
        tags: [
          {
            tag: "meta",
            attrs: { "http-equiv": "Content-Security-Policy", content: policy },
            injectTo: "head-prepend",
          },
        ],
      };
    },
  };
}

// The bundle is served by `cyberbrain serve` from inside the binary (rust-embed), always
// at the root of 127.0.0.1:7777. Relative asset paths keep it working if that ever changes.
export default defineConfig({
  base: "./",
  plugins: [react(), tailwindcss(), csp()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    modulePreload: { polyfill: false },
    rolldownOptions: {
      output: {
        manualChunks: (id: string) => (id.includes("node_modules") ? "vendor" : undefined),
      },
    },
  },
  server: { port: 5173, strictPort: false },
});
