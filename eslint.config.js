import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: [
      "**/node_modules/**",
      "**/target/**",
      "**/dist/**",
      "**/*.node",
      "bindings/native/index.js",
      "bindings/native/index.d.ts",
    ],
  },
  ...tseslint.configs.recommended,
);
