
import Path from 'path';
import { ProjectRootDir } from "../dirs";
import { PetalModule, setupPetalModule } from '../PetalModule';

let _module: PetalModule | null = null;
let _loadingPromise: Promise<void> | null = null;

async function loadWasmModule() {
    const wasmPath = Path.resolve(ProjectRootDir, 'dist/wasm/petal-node.js');
    const loadModule = require(wasmPath);
    const wasmModule = await loadModule();
    _module = setupPetalModule(wasmModule);
}

export async function getWasmModule() {
    if (!_loadingPromise) {
        _loadingPromise = loadWasmModule();
    }
    await _loadingPromise;
    return _module;
}
