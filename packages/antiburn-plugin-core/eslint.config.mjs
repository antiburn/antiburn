export default [
  {
    files: ['**/*.mjs'],
    languageOptions: {
      ecmaVersion: 2024,
      sourceType: 'module',
      globals: Object.fromEntries(
        ['console', 'process', 'Buffer', 'URL', 'setTimeout', 'clearTimeout'].map((name) => [
          name,
          'readonly',
        ]),
      ),
    },
    rules: {
      'no-unused-vars': ['error', { argsIgnorePattern: '^_', caughtErrors: 'none' }],
      'no-undef': 'error',
      'no-unreachable': 'error',
      'no-constant-condition': 'error',
      'constructor-super': 'error',
    },
  },
]
