
# Working with bytecode #

The Petal bytecode system is implemeted with some code generation:

The 'source definition' for bytecode operations is in: ./ts/bytecodeOps.ts . This is the first
file to update if you're modifying or extending the bytecode operations.

The file './ts/generateCpp.ts' takes those definitions and writes to: src/bytecode/bytecode_encoding.h
This step can be done by running `yarn generate:cpp`

# Bytecode Execution #

Details on how bytecode is executed in our VM.

## Compilation Process ##

A parsed program is compiled into bytecode using src/bytecode/compile.cpp (and other helper files)

Compiled bytecode is then executed in src/runtime/vm.cpp

## VM Data Structure ##

Relevant data fields in the VM data structure:

    `u32 pc` - Program counter (current execution position in bytecode).
    `vector slots` - Flat array of register slots.
    `u32 stack_top` - The index of the 'top' register slot. This is used to find local slots.

## Handling Function calls ##

The way function calls work in bytecode is:

### Starting the frame

In some cases we'll use op_reserve_stack to make sure the slots array is big enough.
(this may be omitted in situations where we know the array is preallocated to be big enough)

     op_reserve_slots <size>

Then, local variables are copied into the 'new frame' slots. These are slots that are located
past the end of the 'locals' section for the current block. These slots will be the new local
slots after op_call is triggered.

     op_copy <from> <to>
     op_move <from> <to>

Then, op_call is triggered:

     op_call <func address: u16> <stack size: u8>

   - This increases stack_top based on the size.
   - Also saves a 'frame header' in slot local:0. The frame header is packed with u16 for the RA
      and u8 for the previous frame size.
   - Also jumps the PC to the new func address.

### Diagram of the old & new frames

Before op_call:

    [...existing locals...][            ][input 1][input 2][input 3]
    ^                      ^
    stack_top (local:0)    end of local frame, start of 'new frame'

After op_call:

    [...existing locals...][frame header][input 1][input 2][input 3][...new locals...]
                           ^ stack_top (local:0)

(Then the function does stuff)

Before op_return:

    [...existing locals...][frame header][output 1][output 2][output 3][...dead slots...]
                           ^ stack_top (local:0)

After op_return:

    [...existing locals...][            ][output 1][output 2][output 3][...dead slots...]
    ^                      ^
    stack_top (local:0)    end of local frame, start of 'new frame'

### Future

- We may add specialized versions of op_call, for example a version that can take
an input arg and copy/move it, in a single instruction.

- We may also add 'tail call' style calls, which just jumps the PC and no
frame header is stored.

### Function body

Once in the function, it will run its bytecode.

It will find inputs at slots local:(n+1).

During the function it will copy/move/compute its output values to local:(n+1).
These are the same slots used by inputs, so the input slots are overwritten.

### Returning

The function uses `op_return`

  1. This jumps the PC back to the RA (stored in the frame header at local:0)
  2. And also reduces stack_top by the previous stack size (also stored in the frame header)

### After return

Once back at the callsite, the caller will continue.

The function's outputs will be found in 'new frame' slots, which
are then used by the caller (maybe with op_call/op_move/etc).

The caller will need to consume or move those slots before they perform another
function call, because the same 'new frame' slots would be used for inputs for
the next call.

## Host Function Calls

'Host' calls are custom callback pointers that are added by the embedding host.

The process for calling a host function is similar to regular function calls:

 - Still use `op_reserve_slots` to prepare the new frame
 - Still use op_copy/op_move to place values into 'new frame' slots

Then a different opcode is used:

    op_call_host <host func idx> <frame size: u8>

 - This increases stack_top based on the frame size
 - It does NOT modify 'pc' or save a frame header.
 - It triggers the C function pointer for the host function.
 - The host function receives the VM* as input, which gives it a way to access the input & output
   slots for the 'new frame'.
 - The host function is responsible for saving values into the output slots.
 - Then, execution continues. The callsite will use the output slots.
