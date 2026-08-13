import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// 开发时通过代理访问后端 API，避免 CORS。
// 后端默认监听 127.0.0.1:3780（见 web/config.ts）。
export default defineConfig({
    plugins: [vue()],
    server: {
        port: 5173,
        proxy: {
            "/api": {
                target: "http://127.0.0.1:3780",
                changeOrigin: true,
            },
        },
    },
    build: {
        outDir: "dist",
        emptyOutDir: true,
    },
});
