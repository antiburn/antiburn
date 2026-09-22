import js from "@eslint/js"
import reactHooks from "eslint-plugin-react-hooks"
import globals from "globals"
import tseslint from "typescript-eslint"

export default tseslint.config(
  {
    ignores: ["dist/**", "node_modules/**", "src-tauri/target/**"],
  },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      ...reactHooks.configs["recommended-latest"].rules,
      "@typescript-eslint/consistent-type-imports": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
  {
    files: [
      "src/lib/navigation/mainViews.ts",
      "src/lib/navigation/sessionFilterDefinitions.ts",
      "src/lib/settingsPanes.ts",
    ],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "ImportDeclaration, ImportExpression, TSImportType, ExportNamedDeclaration[source], ExportAllDeclaration, CallExpression[callee.name='require']",
          message: "Keep navigation definitions independent of other modules.",
        },
      ],
    },
  },
  {
    files: ["src/lib/presentation/checkDefinitions.ts", "src/lib/settingsSearchTargets.ts"],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "ImportDeclaration[importKind!='type'][source.value!='./settingsPanes'], ImportDeclaration[importKind='type'][source.value!='../insightsIpc'][source.value!='./platform'], ImportExpression, TSImportType, ExportNamedDeclaration[source], ExportAllDeclaration, CallExpression[callee.name='require']",
          message:
            "Descriptors may depend only on pure pane metadata and declared domain types.",
        },
      ],
    },
  },
  {
    files: ["src/components/ui/{Row,ToggleRow,SectionGroup}.tsx"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["**/*settings*", "**/settings/**"],
              message: "Keep Settings metadata in the Settings adapters.",
            },
          ],
        },
      ],
    },
  },
  {
    // Config and generator scripts run under Node, not the webview.
    files: ["*.js", "*.ts", "scripts/**/*.mjs"],
    extends: [js.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: globals.node,
    },
  },
)
