import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  build: { outDir: 'dist' },
  server: { port: 3000, proxy: { '/api': 'http://localhost:9090', '/ws': { target: 'ws://localhost:9090', ws: true } } }
})
