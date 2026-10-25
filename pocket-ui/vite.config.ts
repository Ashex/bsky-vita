import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
export default defineConfig({ plugins: [solid()], build: { outDir: "../app/static/pocket", emptyOutDir: true, target: "es2020" } });
