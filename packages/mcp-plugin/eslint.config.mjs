import tseslint from "typescript-eslint";

export default tseslint.config(
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.ts", "test/**/*.ts"],
    rules: {
      "no-console": ["warn", { allow: ["error", "warn"] }],
    },
  },
  {
    ignores: ["dist/**", "node_modules/**"],
  }
);
