import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { config } from 'dotenv';
import path from 'path';

config({ path: path.resolve(__dirname, '../.env') });

if (!process.env.PRISM_API_PORT) {
  throw new Error('PRISM_API_PORT environment variable is required (set it in playground/.env)');
}
const API_PORT = process.env.PRISM_API_PORT;
const WEB_PORT = parseInt(process.env.VITE_PORT || '4007', 10);

export default defineConfig({
  plugins: [react()],
  server: {
    port: WEB_PORT,
    proxy: {
      '/api': `http://localhost:${API_PORT}`,
    },
  },
});
