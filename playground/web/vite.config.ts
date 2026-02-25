import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { config } from 'dotenv';
import path from 'path';

config({ path: path.resolve(__dirname, '../.env') });

const API_PORT = process.env.PRISM_API_PORT || '4006';
const WEB_PORT = parseInt(process.env.VITE_PORT || '4007', 10);

export default defineConfig({
  plugins: [react()],
  server: {
    port: WEB_PORT,
    proxy: {
      '/analyze': `http://localhost:${API_PORT}`,
      '/examples': `http://localhost:${API_PORT}`,
    },
  },
});
