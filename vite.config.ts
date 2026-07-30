import { defineConfig } from "vite";
import { VitePWA } from "vite-plugin-pwa";

export default defineConfig({
  root: "web",
  publicDir: "public",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: "esnext",
  },
  server: {
    port: 5173,
    headers: {
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Embedder-Policy": "require-corp",
    },
  },
  plugins: [
    VitePWA({
      registerType: "autoUpdate",
      includeAssets: ["icon.svg"],
      manifest: {
        name: "Fountain QR Transfer",
        short_name: "FountainQR",
        description: "Offline file transfer via fountain-coded QR frames",
        theme_color: "#000000",
        background_color: "#000000",
        display: "standalone",
        orientation: "any",
        start_url: "/?v=6",
        icons: [
          {
            src: "icon.svg",
            sizes: "any",
            type: "image/svg+xml",
            purpose: "any maskable",
          },
        ],
      },
      workbox: {
        globPatterns: ["**/*.{js,css,html,wasm,svg,ico}"],
        maximumFileSizeToCacheInBytes: 12 * 1024 * 1024,
        cleanupOutdatedCaches: true,
        clientsClaim: true,
        skipWaiting: true,
      },
    }),
  ],
});
