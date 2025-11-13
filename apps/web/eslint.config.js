import { fileURLToPath } from "node:url";
import { includeIgnoreFile } from "@eslint/compat";
import baseConfig from "@bmxtract/eslint-config";
import ts from "typescript-eslint";
import svelteConfig from "./svelte.config.js";

const gitignorePath = fileURLToPath(new URL("./.gitignore", import.meta.url));

export default [
  includeIgnoreFile(gitignorePath),
  ...baseConfig,
  {
    files: ["**/*.svelte", "**/*.svelte.ts", "**/*.svelte.js"],
    languageOptions: {
      parserOptions: {
        projectService: true,
        extraFileExtensions: [".svelte"],
        parser: ts.parser,
        svelteConfig,
      },
    },
  },
];
