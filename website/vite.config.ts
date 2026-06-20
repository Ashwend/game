import { defineConfig } from 'vitest/config'
import { tanstackStart } from '@tanstack/react-start/plugin/vite'
import viteReact from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// The site is fully static: every route is prerendered to HTML at build time
// and served as plain files (Cloudflare Pages). No server runtime is needed at
// request time; the deployable output lives in `dist/client`.
const config = defineConfig({
  resolve: { tsconfigPaths: true },
  plugins: [
    tailwindcss(),
    tanstackStart({
      prerender: {
        enabled: true,
        // Crawl <a> links from the entry so every reachable route is emitted
        // as static HTML, and write `/foo` as `/foo/index.html`.
        crawlLinks: true,
        autoSubfolderIndex: true,
        autoStaticPathsDiscovery: true,
        concurrency: 8,
      },
    }),
    viteReact(),
  ],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
  },
})

export default config
