import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

// `npm run dev` serves the page on 127.0.0.1:5173 and passes the API to a running lb-editor (LB_EDITOR_URL, else
// http://127.0.0.1:8090). Open lb-editor's token link once first: the cookie it sets holds for 127.0.0.1 on any port.
const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {}
const target = env.LB_EDITOR_URL ?? 'http://127.0.0.1:8090'

export default defineConfig({
  plugins: [vue()],
  server: {
    host: '127.0.0.1',
    proxy: {
      '/api': { target, changeOrigin: true },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    chunkSizeWarningLimit: 1500,
  },
})
