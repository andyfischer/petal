import { config } from 'dotenv';
config({ path: new URL('../.env', import.meta.url).pathname });

import { App, startServer } from '@facetlayer/prism-framework-api';
import { petalService } from './services/petal-service.ts';

const PORT = parseInt(process.env.PRISM_API_PORT || '4810', 10);

async function main() {
  const app = new App({
    name: 'Petal Playground',
    description: 'Interactive playground for the Petal programming language',
    services: [petalService],
  });

  await startServer({
    port: PORT,
    app,
    corsConfig: {
      enableTestEndpoints: true,
    },
  });

  console.log(`Petal Playground API running at http://localhost:${PORT}`);
}

main().catch(console.error);
