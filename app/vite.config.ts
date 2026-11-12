import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

// Tauri expects a fixed dev port; the build is a plain static bundle.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "safari16", sourcemap: false },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
