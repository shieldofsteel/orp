import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: 'dist',
    chunkSizeWarningLimit: 250,
    rollupOptions: {
      output: {
        manualChunks: {
          // Split heavy vendors so they cache across deploys.
          'react-vendor': ['react', 'react-dom'],
          'leaflet-vendor': ['leaflet', 'react-leaflet'],
          'data-vendor': ['@tanstack/react-query', 'zustand'],
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
