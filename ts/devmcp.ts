#!/usr/bin/env tsx

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { CallToolRequestSchema, ListToolsRequestSchema } from '@modelcontextprotocol/sdk/types.js';
import * as path from 'path';
import { execSync, spawn } from 'child_process';

// Global path to the CLI binary
const CliBinPath = path.join(__dirname, '..', 'dist/cli/main');

const server = new Server(
  {
    name: 'petal-devmcp',
    version: '1.0.0',
  },
  {
    capabilities: {
      tools: {},
    },
  }
);

// Helper function to get the root directory of the Petal project
function getPetalRoot(): string {
  // Assuming this MCP server is run from within the petal project
  return path.resolve(__dirname, '..');
}

// Helper function to build the Petal CLI if needed
function ensurePetalBuilt(): void {
  const petalRoot = getPetalRoot();
  
  try {
    // Check if the CLI exists
    execSync(`test -f "${CliBinPath}"`, { cwd: petalRoot });
  } catch {
    // Build if it doesn't exist
    console.error('Building Petal CLI...');
    try {
      execSync('make', { cwd: petalRoot, stdio: 'inherit' });
    } catch (buildError) {
      console.error('Make failed with error:', buildError);
      console.error('Current working directory:', petalRoot);
      console.error('CLI path being built:', CliBinPath);
      throw new Error(`Failed to build Petal CLI: ${buildError.message || buildError}`);
    }
  }
}

// Tool implementations
async function testCompilation(source: string): Promise<string> {
  ensurePetalBuilt();
  
  const petalRoot = getPetalRoot();
  
  return new Promise((resolve) => {
    const child = spawn(CliBinPath, ['-test-compile-stdin'], {
      cwd: petalRoot,
      stdio: ['pipe', 'pipe', 'pipe']
    });
    
    let stdout = '';
    let stderr = '';
    
    child.stdout.on('data', (data) => {
      stdout += data.toString();
    });
    
    child.stderr.on('data', (data) => {
      stderr += data.toString();
    });
    
    child.on('close', (code) => {
      if (code === 0) {
        resolve(stdout.trim());
      } else {
        resolve(`Error: Process exited with code ${code}\nStderr: ${stderr || 'No stderr'}`);
      }
    });
    
    child.on('error', (error) => {
      resolve(`Error: ${error.message}`);
    });
    
    // Write source code to stdin and close it
    child.stdin.write(source);
    child.stdin.end();
    
    // Add timeout
    setTimeout(() => {
      child.kill();
      resolve('Error: Process timed out after 10 seconds');
    }, 10000);
  });
}

async function testParsing(source: string): Promise<string> {
  ensurePetalBuilt();
  
  const petalRoot = getPetalRoot();
  
  return new Promise((resolve) => {
    const child = spawn(CliBinPath, ['-test-parse-stdin'], {
      cwd: petalRoot,
      stdio: ['pipe', 'pipe', 'pipe']
    });
    
    let stdout = '';
    let stderr = '';
    
    child.stdout.on('data', (data) => {
      stdout += data.toString();
    });
    
    child.stderr.on('data', (data) => {
      stderr += data.toString();
    });
    
    child.on('close', (code) => {
      if (code === 0) {
        resolve(stdout.trim());
      } else {
        resolve(`Error: Process exited with code ${code}\nStderr: ${stderr || 'No stderr'}`);
      }
    });
    
    child.on('error', (error) => {
      resolve(`Error: ${error.message}`);
    });
    
    // Write source code to stdin and close it
    child.stdin.write(source);
    child.stdin.end();
    
    // Add timeout
    setTimeout(() => {
      child.kill();
      resolve('Error: Process timed out after 10 seconds');
    }, 10000);
  });
}

// List available tools
server.setRequestHandler(ListToolsRequestSchema, async () => {
  return {
    tools: [
      {
        name: 'test_compilation',
        description: 'Compiles a sample of Petal source code and returns the bytecode dump',
        inputSchema: {
          type: 'object',
          properties: {
            source: {
              type: 'string',
              description: 'The Petal source code to compile'
            }
          },
          required: ['source']
        }
      },
      {
        name: 'test_parsing',
        description: 'Compiles a sample of Petal source code and returns the AST',
        inputSchema: {
          type: 'object',
          properties: {
            source: {
              type: 'string',
              description: 'The Petal source code to parse'
            }
          },
          required: ['source']
        }
      }
    ]
  };
});

// Handle tool calls
server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;
  
  switch (name) {
    case 'test_compilation':
      if (!args || typeof args.source !== 'string') {
        throw new Error('source argument is required and must be a string');
      }
      const compilationResult = await testCompilation(args.source);
      return {
        content: [
          {
            type: 'text',
            text: compilationResult
          }
        ]
      };
      
    case 'test_parsing':
      if (!args || typeof args.source !== 'string') {
        throw new Error('source argument is required and must be a string');
      }
      const parsingResult = await testParsing(args.source);
      return {
        content: [
          {
            type: 'text',
            text: parsingResult
          }
        ]
      };
      
    default:
      throw new Error(`Unknown tool: ${name}`);
  }
});

// Start the server
async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error('Petal MCP server started');
}

if (require.main === module) {
  main().catch((error) => {
    console.error('Server error:', error);
    process.exit(1);
  });
}
