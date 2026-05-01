import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": "http://127.0.0.1:8964",
      "/stream": {
        target: "http://127.0.0.1:8964",
        changeOrigin: true,
        // SSE-friendly: don't buffer, don't compress
        configure: (proxy) => {
          proxy.on("proxyRes", (proxyRes) => {
            delete proxyRes.headers["content-length"];
          });
        },
      },
    },
  },
  build: {
    target: "es2022",
    sourcemap: true,
  },
});
