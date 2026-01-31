# WebAssembly

More about the WASM API for using Petal from JavaScript/browser environments.

## Related Topics

- [[Setup]] - Native Rust setup
- [[Execution]] - Running programs

## Overview

Petal compiles to WebAssembly for use in browsers and other JavaScript environments. The WASM API uses numeric handles instead of direct references.

## Handle Types

```rust
pub type EnvHandle = u32;      // Reference to an Env
pub type ProgramHandle = u32;  // Reference to a Program
pub type StackHandle = u32;    // Reference to a Stack
pub type TermHandle = u32;     // Reference to a Term
```

Handle `0` is reserved for null/error conditions.

## JavaScript Wrapper

A typical JavaScript wrapper:

```javascript
class PetalEnv {
    constructor() {
        this.handle = wasm.petal_create_env();
        if (this.handle === 0) {
            throw new Error("Failed to create Petal environment");
        }
    }

    destroy() {
        wasm.petal_destroy_env(this.handle);
        this.handle = 0;
    }

    loadProgram(source) {
        const sourceBytes = new TextEncoder().encode(source);
        const ptr = wasm.petal_alloc(sourceBytes.length);
        new Uint8Array(wasm.memory.buffer, ptr, sourceBytes.length)
            .set(sourceBytes);

        const programHandle = wasm.petal_load_program(
            this.handle, ptr, sourceBytes.length
        );

        wasm.petal_free(ptr, sourceBytes.length);

        if (programHandle === 0) {
            throw new Error(this.getError());
        }
        return new PetalProgram(this, programHandle);
    }

    getError() {
        const bufferSize = 1024;
        const ptr = wasm.petal_alloc(bufferSize);
        const len = wasm.petal_get_error(this.handle, ptr, bufferSize);
        const error = new TextDecoder().decode(
            new Uint8Array(wasm.memory.buffer, ptr, len)
        );
        wasm.petal_free(ptr, bufferSize);
        return error;
    }
}

class PetalProgram {
    constructor(env, handle) {
        this.env = env;
        this.handle = handle;
    }

    createStack() {
        const stackHandle = wasm.petal_create_stack(
            this.env.handle, this.handle
        );
        if (stackHandle === 0) {
            throw new Error(this.env.getError());
        }
        return new PetalStack(this.env, stackHandle);
    }
}

class PetalStack {
    constructor(env, handle) {
        this.env = env;
        this.handle = handle;
    }

    destroy() {
        wasm.petal_destroy_stack(this.env.handle, this.handle);
    }

    step() {
        const result = wasm.petal_step(this.env.handle, this.handle);
        switch (result) {
            case 0: return { status: 'continue' };
            case 1: return { status: 'complete', value: this.getResult() };
            case 2: throw new Error(this.env.getError());
            case 3: return { status: 'breakpoint' };
        }
    }

    run() {
        const result = wasm.petal_run(this.env.handle, this.handle);
        if (result === 0) {
            throw new Error(this.env.getError());
        }
        return this.getResult();
    }

    getResult() {
        const lenPtr = wasm.petal_alloc(4);
        const jsonPtr = wasm.petal_get_result_json(
            this.env.handle, this.handle, lenPtr
        );
        const len = new Uint32Array(wasm.memory.buffer, lenPtr, 1)[0];
        wasm.petal_free(lenPtr, 4);

        if (jsonPtr === 0) return null;

        const json = new TextDecoder().decode(
            new Uint8Array(wasm.memory.buffer, jsonPtr, len)
        );
        wasm.petal_free(jsonPtr, len);
        return JSON.parse(json);
    }
}
```

## Usage Example

```javascript
// Initialize
const env = new PetalEnv();

// Load and run a program
const program = env.loadProgram(`
    let x = 1 + 2
    let y = x * 3
    y
`);

const stack = program.createStack();
const result = stack.run();
console.log("Result:", result); // 9

// Cleanup
stack.destroy();
env.destroy();
```

## Memory Management

WASM has linear memory that must be managed explicitly:

```rust
// Allocate memory for passing strings in
#[no_mangle]
pub extern "C" fn petal_alloc(size: u32) -> *mut u8;

// Free memory
#[no_mangle]
pub extern "C" fn petal_free(ptr: *mut u8, size: u32);
```

Always free memory after use to prevent leaks.

## Live Editing via WASM

```javascript
// Apply an edit
const editJson = JSON.stringify({
    range: { start: 10, end: 20 },
    new_text: "new_code"
});

const editBytes = new TextEncoder().encode(editJson);
const editPtr = wasm.petal_alloc(editBytes.length);
new Uint8Array(wasm.memory.buffer, editPtr, editBytes.length)
    .set(editBytes);

const success = wasm.petal_live_edit(
    env.handle, program.handle, editPtr, editBytes.length
);

wasm.petal_free(editPtr, editBytes.length);

if (!success) {
    throw new Error(env.getError());
}
```

## Building for WASM

```bash
# Install wasm-pack
cargo install wasm-pack

# Build the WASM module
wasm-pack build --target web

# Output is in pkg/
# - petal_bg.wasm (the binary)
# - petal.js (JS bindings)
```

## Performance Considerations

- **Minimize boundary crossings**: Batch operations where possible
- **Reuse buffers**: Allocate once, reuse for multiple calls
- **Use typed arrays**: Direct memory access is faster than serialization

```javascript
// Batch multiple steps
while (true) {
    const result = stack.step();
    if (result.status !== 'continue') break;
}

// vs calling run() which does this internally in WASM
const result = stack.run(); // Faster
```

---

See also: [[Outline|Implementation Plan]]
