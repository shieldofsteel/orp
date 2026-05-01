import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: 'dist',
    chunkSizeWarningLimit: 250,
    rollupOptions: {
      output: {
        manualChunks: (id) => {
          // Vendor splitting — keep heavy libs in dedicated chunks so they
          // cache across deploys and don't bloat any app chunk.
          if (id.includes('node_modules')) {
            if (id.includes('react-dom') || id.includes('/react/')) return 'react-vendor';
            if (id.includes('@tanstack') || id.includes('zustand'))   return 'data-vendor';
            // Split the leaflet stack by submodule directory so no single
            // chunk exceeds the 250 kB budget. Files under leaflet/src are
            // grouped by their top-level subfolder (layer, geometry, map,
            // dom, control, core, geo). The minified UMD bundle alone is
            // ~150 kB; this submodule split keeps each piece smaller.
            if (id.includes('react-leaflet')) return 'leaflet-react';
            if (id.includes('leaflet')) {
              const m = id.match(/leaflet\/(?:src|dist)\/(?:src\/)?([^/]+)/);
              const sub = m?.[1] ?? 'core';
              if (['layer', 'Layer'].includes(sub))                return 'leaflet-layer';
              if (['geometry', 'geo'].includes(sub))               return 'leaflet-geo';
              if (['dom', 'control', 'map'].includes(sub))         return 'leaflet-ui';
              return 'leaflet-core';
            }
          }
        },
      },
    },
  },
  server: {
    port: 3000,
    proxy: {
      '/api': 'http://localhost:9090',
      '/ws': { target: 'ws://localhost:9090', ws: true },
    },
  },
})
