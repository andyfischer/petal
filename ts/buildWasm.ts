#!/usr/bin/env tsx

import { spawn } from 'child_process';
import { promises as fs } from 'fs';
import * as path from 'path';

enum BuildTarget {
    WasmWeb = 'web',
    WasmNode = 'node',
}

interface BuildOptions {
    target: BuildTarget;
    rootDir: string;
}

const EMSDK_DIR = process.env.EMSDK_DIR;

if (!EMSDK_DIR) {
    console.error('ERROR: EMSDK_DIR environment variable is not set');
    process.exit(1);
}

const ExportedFunctions = [
    'add_callback',
    'set_log_callback',
    'get_library_version',
    'debug_get_lexed',
    'debug_get_parsed',
    'debug_get_bytecode',
    'debug_vm_execute',
    'debug_reset_global_state',
];

const ExportedRuntimeMethods = [
    'cwrap',
    'UTF8ToString',
    'addFunction',
    'removeFunction',
];

async function mkdirp(dirPath: string): Promise<void> {
    try {
        await fs.mkdir(dirPath, { recursive: true });
    } catch (error) {
        // Ignore error if directory already exists
    }
}

async function runShellCommand(command: string, args: string[]): Promise<void> {
    return new Promise((resolve, reject) => {
        console.log('Running:', command, args.join(' '));
        
        const child = spawn(command, args, { 
            stdio: ['inherit', 'pipe', 'pipe'],
            shell: true 
        });

        child.stdout?.on('data', (data) => {
            process.stdout.write(`[build] ${data}`);
        });

        child.stderr?.on('data', (data) => {
            process.stderr.write(`[build] ${data}`);
        });

        child.on('close', (code) => {
            if (code === 0) {
                resolve();
            } else {
                reject(new Error(`Command failed with exit code ${code}`));
            }
        });

        child.on('error', reject);
    });
}

function getLinkBinary(): string {
    return path.join(EMSDK_DIR, 'upstream/emscripten/em++');
}

function getOutputDir(target: BuildTarget, rootDir: string): string {
    switch (target) {
        case BuildTarget.WasmWeb:
            return path.join(rootDir, 'dist/wasm');
        case BuildTarget.WasmNode:
            return path.join(rootDir, 'dist/wasm');
    }
}

function getLinkOutputFilename(target: BuildTarget): string {
    switch (target) {
        case BuildTarget.WasmWeb:
            return 'petal.js';
        case BuildTarget.WasmNode:
            return 'petal-node.js';
    }
}


async function runLink(options: BuildOptions): Promise<void> {
    const targetDir = getOutputDir(options.target, options.rootDir);
    await mkdirp(targetDir);

    const linkBin = getLinkBinary();
    const outputFilename = getLinkOutputFilename(options.target);
    
    const linkFlags: string[] = [];
    
    // Target-specific flags
    if (options.target === BuildTarget.WasmNode) {
        linkFlags.push(
            '-s', 'ENVIRONMENT=node',
            '-s', 'ALLOW_TABLE_GROWTH=1',
            '-s', 'RESERVED_FUNCTION_POINTERS=1',
        );
    }
    
    // Exported functions
    if (ExportedFunctions.length > 0) {
        const exportedFuncs = ExportedFunctions.map(f => `_${f}`).join(',');
        linkFlags.push('-s', `EXPORTED_FUNCTIONS=${exportedFuncs}`);
    }
    
    // Common WASM flags
    linkFlags.push(
        '-s', `EXPORTED_RUNTIME_METHODS=${ExportedRuntimeMethods.join(',')}`,
        '-s', 'ALLOW_MEMORY_GROWTH=1',
        '-s', 'WASM=1',
        '-s', 'MODULARIZE=1',
        '-s', 'EXPORT_NAME=PetalModule',
        '-s', 'STACK_SIZE=1MB',
        '-s', 'INITIAL_MEMORY=16MB',
        '-std=c++17',
        '-O1',
        '-Wno-unused-parameter',
        '-Wno-unused-variable',
        '-Wno-unused-but-set-variable',
        '-Wno-reorder-ctor',
        '-Wno-deprecated-copy-with-user-provided-copy',
        `-I${path.join(options.rootDir, 'src')}`,
    );

    const unityPath = path.join(options.rootDir, 'src/unity.cpp');
    
    const commandArgs = [
        unityPath,
        '-o',
        path.join(targetDir, outputFilename),
        ...linkFlags,
    ];

    await runShellCommand(linkBin, commandArgs);
}

async function build(options: BuildOptions): Promise<void> {
    console.log(`Building for target: ${options.target}`);
    
    const targetDir = getOutputDir(options.target, options.rootDir);
    const unityPath = path.join(options.rootDir, 'src/unity.cpp');
    
    // Check if unity.cpp exists
    try {
        await fs.access(unityPath);
    } catch (error) {
        throw new Error(`Unity file not found at ${unityPath}. Please generate it first.`);
    }
    
    await runLink(options);
    
    console.log(`Build completed successfully in: ${targetDir}`);
}

function parseArgs(): BuildOptions {
    const args = process.argv.slice(2);
    
    let target: BuildTarget = BuildTarget.WasmWeb; // default
    
    for (let i = 0; i < args.length; i++) {
        if (args[i] === '--target' && i + 1 < args.length) {
            const targetArg = args[i + 1];
            if (targetArg === 'web') {
                target = BuildTarget.WasmWeb;
            } else if (targetArg === 'node') {
                target = BuildTarget.WasmNode;
            } else {
                console.error(`Invalid target: ${targetArg}. Use 'web' or 'node'.`);
                process.exit(1);
            }
        }
    }
    
    const rootDir = path.resolve(__dirname, '..');
    
    
    return {
        target,
        rootDir,
    };
}

function printUsage(): void {
    console.log('Usage: node buildWasm.ts [--target <web|node>]');
    console.log('');
    console.log('Options:');
    console.log('  --target web   Build for web browsers (default)');
    console.log('  --target node  Build for Node.js');
}

async function main(): Promise<void> {
    const args = process.argv.slice(2);
    
    if (args.includes('--help') || args.includes('-h')) {
        printUsage();
        return;
    }
    
    try {
        const options = parseArgs();
        await build(options);
    } catch (error) {
        console.error('Build failed:', error);
        process.exit(1);
    }
}

if (require.main === module) {
    main();
}
