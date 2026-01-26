
export interface PetalModule {
    add_callback(callback: (message: string) => void): number
    set_log_callback(callback_id: number): void
    get_library_version(): string 
    debug_get_lexed(program_text: string): string
    debug_get_parsed(program_text: string): string
    debug_get_bytecode(program_text: string): string
    debug_vm_execute(program_text: string): string
    debug_reset_global_state(): void
}

export function setupPetalModule(wasmModule: any): PetalModule {
    // Set the callback in the C code
    const addCallback = wasmModule.cwrap('add_callback', 'number', ['number']);
    const setLogCallback = wasmModule.cwrap('set_log_callback', null, ['number']);

    // Set up default console.log callback
    const defaultLogCallbackId = addCallback(wasmModule.addFunction((messagePtr: number) => {
        const message = wasmModule.UTF8ToString(messagePtr);
        console.log(message);
    }, 'vi'));
    
    setLogCallback(defaultLogCallbackId);

    return {
        add_callback: (callback: (message: string) => void) => {
            // Create a new callback wrapper
            const callbackPtr = wasmModule.addFunction((messagePtr: number) => {
                const message = wasmModule.UTF8ToString(messagePtr);
                callback(message);
            }, 'vi');
            
            // Register the callback and return its ID
            return addCallback(callbackPtr);
        },
        set_log_callback: setLogCallback,
        get_library_version: wasmModule.cwrap('get_library_version', 'string', []),
        debug_get_lexed: wasmModule.cwrap('debug_get_lexed', 'string', ['string']),
        debug_get_parsed: wasmModule.cwrap('debug_get_parsed', 'string', ['string']),
        debug_get_bytecode: wasmModule.cwrap('debug_get_bytecode', 'string', ['string']),
        debug_vm_execute: wasmModule.cwrap('debug_vm_execute', 'string', ['string']),
        debug_reset_global_state: wasmModule.cwrap('debug_reset_global_state', null, []),
    }
}

