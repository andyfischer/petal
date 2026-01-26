import { expect, it } from 'vitest'
import { getWasmModule } from './testSetup'

it('should be able to load the WASM library', async () => {
    const lib = await getWasmModule();
    expect(lib).toBeDefined();

    const version = lib.get_library_version();
    expect(version).toContain('petal');
});