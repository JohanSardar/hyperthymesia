import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

// HX_SERVER=http://127.0.0.1:PORT bun run dev sends API calls and sign-in to a running hx-server.
const target = process.env.HX_SERVER

export default defineConfig({
  base: "/app/",
  plugins: [react()],
  build: { outDir: "dist", emptyOutDir: true },
  server: target ? { proxy: { "/v1": target, "/login": target } } : undefined,
})
