import { configDefaults, defineConfig } from 'vitest/config'
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
      'buffer',
      'isomorphic-git',
      'isomorphic-git/http/web',
    ],
  },
  resolve: {
    alias: [
      { find: '@', replacement: path.resolve(__dirname, './src') },
      {
        find: /^isomorphic-git$/,
        replacement: path.resolve(__dirname, './node_modules/isomorphic-git/index.js'),
      },
      {
        find: /^isomorphic-git\/http\/web$/,
        replacement: path.resolve(
          __dirname,
          './node_modules/isomorphic-git/http/web/index.js',
        ),
      },
    ],
  },
  build: {
    chunkSizeWarningLimit: 800,
  },
  test: {
    // e2e/ is Playwright's tree — its test() throws when vitest collects it.
    exclude: [...configDefaults.exclude, 'e2e/**'],
  },
  server: {
    fs: {
      allow: [path.resolve(__dirname, '../../..')],
    },
  },
})
