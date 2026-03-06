import { defineConfig } from 'vite';

export default defineConfig({
  root: '.',
  server: {
    proxy: {
      '/ws': {
        target: 'ws://localhost:3333',
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
  },
});
