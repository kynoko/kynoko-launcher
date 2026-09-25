import { defineConfig } from 'vite';

// Tauri serves the window from this dev server (tauri.conf.json devUrl).
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: 'es2022' },
});
