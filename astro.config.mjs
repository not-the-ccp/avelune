import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://not-the-ccp.github.io",
  base: "/avelune",
  output: "static",
  srcDir: "./site/src",
  outDir: "./dist/site",
  publicDir: "./site/public",
  build: {
    format: "directory",
  },
});
