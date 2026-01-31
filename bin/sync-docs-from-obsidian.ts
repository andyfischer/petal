#!/usr/bin/env node
/**
 * Syncs documentation from Obsidian vault to the docs/ directory.
 * Copies all files from ~/obsidian/petal/* into docs/
 */

import { cpSync, existsSync, mkdirSync, rmSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, join } from 'node:path';

const sourceDir = join(homedir(), 'obsidian', 'petal');
const targetDir = join(import.meta.dirname, '..', 'docs');

if (!existsSync(sourceDir)) {
  console.error(`Source directory does not exist: ${sourceDir}`);
  process.exit(1);
}

// Ensure docs directory exists
if (!existsSync(targetDir)) {
  mkdirSync(targetDir, { recursive: true });
}

// Clear existing docs and copy fresh
console.log(`Syncing from ${sourceDir} to ${targetDir}...`);

// Remove existing contents
rmSync(targetDir, { recursive: true, force: true });
mkdirSync(targetDir, { recursive: true });

// Copy all files, excluding .obsidian
cpSync(sourceDir, targetDir, {
  recursive: true,
  filter: (src) => basename(src) !== '.obsidian',
});

console.log('Done!');
