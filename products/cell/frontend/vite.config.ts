import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'path'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
  ],
  define: {
    global: 'globalThis',
  },
  optimizeDeps: {
    include: [
      '@isomorphic-git/lightning-fs',
      'isomorphic-git',
      'isomorphic-git/http/web',
    ],
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    chunkSizeWarningLimit: 800,
  },
  server: {
    fs: {
      allow: [path.resolve(__dirname, '../../..')],
    },
  },
})
