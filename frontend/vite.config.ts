import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      '/workspaces': {
        target: 'http://localhost:8088',
        changeOrigin: true,
      },
      '/executions': {
        target: 'http://localhost:8088',
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: false,
  },
})
