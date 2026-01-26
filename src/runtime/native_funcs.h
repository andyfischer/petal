#pragma once

enum class NativeFunctionId {
    None = 0,

    Input = 1,
    Value,

    // Math
    Add,
    Sub,
    Mult,
    Div,

    // Logic
    And,
    Or,
    Xor,

    // Comparison
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,

    // Control flow
    Jump,
    JumpIfTrue,
    JumpIfFalse,
    
    // For loop support
    Inc,

};