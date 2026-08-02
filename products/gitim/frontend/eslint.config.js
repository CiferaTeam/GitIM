import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

// These modules synchronize component state with route, storage, or runtime boundaries.
const stateSyncEffectFiles = [
  'src/components/chat/input-area.tsx',
  'src/components/chat/sidebar.tsx',
  'src/components/flows/run-detail.tsx',
  'src/components/management/add-agent-dialog.tsx',
  'src/components/setup/local-setup.tsx',
  'src/hooks/use-channel-operations.ts',
  'src/hooks/use-version-check.ts',
]

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
  },
  {
    files: stateSyncEffectFiles,
    rules: {
      'react-hooks/set-state-in-effect': 'off',
    },
  },
  {
    // The poll loop mirrors router values into refs consumed by its async cycle.
    files: ['src/hooks/use-poll-loop.ts'],
    rules: {
      'react-hooks/refs': 'off',
    },
  },
])
