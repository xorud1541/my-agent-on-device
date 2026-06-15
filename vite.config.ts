import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  // Tauri API 를 서버 기동 시 eager 로 사전 번들한다. 미지정 시 vite 는 첫 요청 때
  // lazy 최적화하는데, 이게 webview 첫 로드와 부딪혀 dev 모드에서 빈 화면(reload 필요)을
  // 만든다. include 로 ready 이전에 최적화를 끝내 첫 로드를 안정화한다.
  optimizeDeps: {
    include: ["@tauri-apps/api/core", "@tauri-apps/api/event"],
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
  },
});
