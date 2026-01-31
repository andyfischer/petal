//! Parser for Petal source code
//!
//! Converts source text into a Program (collection of Terms)

use std::collections::HashMap;

use crate::Error;

/// Unique identifier for a term within a program
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TermId(pub u32);

/// Unique identifier for a function
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

/// A program is a collection of terms
pub struct Program {
    terms: Vec<Term>,
    constants: Vec<ConstantValue>,
    entry_point: TermId,
    functions: HashMap<String, FunctionId>,
    function_defs: HashMap<FunctionId, FunctionDef>,
}

#[derive(Clone, Debug)]
pub struct FunctionDef {
    pub name: String,
    pub params: Vec<String>,
    pub body_start: TermId,
}

#[derive(Clone, Debug)]
pub struct Term {
    pub id: TermId,
    pub op: TermOp,
    pub inputs: Vec<TermId>,
    pub control_flow_next: Option<TermId>,
    pub control_flow_prev: Option<TermId>,
}

#[derive(Clone, Debug)]
pub enum TermOp {
    Constant(ConstantId),
    Error(String),

    // Variables and assignment
    Let { name: String },
    Assign { name: String },
    GetVariable(String),

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,

    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // Logical
    And,
    Or,
    Not,

    // Control flow
    Branch { then_term: TermId, else_term: TermId },
    Jump { target: TermId },
    Return,

    // Loops
    ForLoop { var: String, iter_expr: TermId, body_start: TermId, end_term: TermId },
    WhileLoop { condition: TermId, body_start: TermId, end_term: TermId },
    Break,
    Continue,

    // Functions
    Call { function: FunctionId },
    CallBuiltin { name: String },
    DefineFunction { name: String, function_id: FunctionId },

    // Collections
    CreateList,
    CreateMap,
    GetIndex,
    SetIndex,
    GetField { field: String },
    SetField { field: String },

    // State (Petal's unique feature)
    StateRead { name: String },
    StateWrite { name: String },
    StateInit { name: String, initial_value: TermId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConstantId(pub u32);

#[derive(Clone, Debug)]
pub enum ConstantValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl Program {
    pub fn new() -> Self {
        Self {
            terms: Vec::new(),
            constants: Vec::new(),
            entry_point: TermId(0),
            functions: HashMap::new(),
            function_defs: HashMap::new(),
        }
    }

    pub fn add_term(&mut self, mut term: Term) -> TermId {
        let id = TermId(self.terms.len() as u32);
        term.id = id;
        self.terms.push(term);
        id
    }

    pub fn get_term(&self, id: TermId) -> Option<&Term> {
        self.terms.get(id.0 as usize)
    }

    pub fn get_term_mut(&mut self, id: TermId) -> Option<&mut Term> {
        self.terms.get_mut(id.0 as usize)
    }

    pub fn add_constant(&mut self, value: ConstantValue) -> ConstantId {
        // Check if already exists
        for (i, existing) in self.constants.iter().enumerate() {
            if existing == &value {
                return ConstantId(i as u32);
            }
        }
        let id = ConstantId(self.constants.len() as u32);
        self.constants.push(value);
        id
    }

    pub fn get_constant(&self, id: ConstantId) -> Option<&ConstantValue> {
        self.constants.get(id.0 as usize)
    }

    pub fn entry_point(&self) -> TermId {
        self.entry_point
    }

    pub fn set_entry_point(&mut self, id: TermId) {
        self.entry_point = id;
    }

    pub fn register_function(&mut self, name: String, id: FunctionId, def: FunctionDef,
    ) {
        self.functions.insert(name, id);
        self.function_defs.insert(id, def);
    }

    pub fn get_function(&self, name: &str) -> Option<FunctionId> {
        self.functions.get(name).copied()
    }

    pub fn get_function_def(&self, id: FunctionId) -> Option<&FunctionDef> {
        self.function_defs.get(&id)
    }
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse source code into a Program
pub fn parse(source: &str) -> Result<Program, Error> {
    let mut parser = Parser::new(source);
    parser.parse_program()
}

struct Parser<'a> {
    source: &'a str,
    chars: std::str::Chars<'a>,
    current: Option<char>,
    position: usize,
    line: u32,
    column: u32,
}

impl<'a> Parser<'a> {
    fn new(source: &str) -> Parser {
        let mut chars = source.chars();
        let current = chars.next();
        Parser {
            source,
            chars,
            current,
            position: 0,
            line: 1,
            column: 1,
        }
    }

    fn advance(&mut self) -> Option<char> {
        if let Some(c) = self.current {
            if c == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
            self.position += 1;
        }
        self.current = self.chars.next();
        self.current
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.current {
            if c.is_whitespace() {
                self.advance();
            } else if c == '/' {
                // Check for comment
                let next_pos = self.position + 1;
                if next_pos < self.source.len() {
                    let next_char = self.source.chars().nth(next_pos);
                    if next_char == Some('/') {
                        // Line comment, skip to end of line
                        while let Some(c) = self.current {
                            if c == '\n' {
                                break;
                            }
                            self.advance();
                        }
                    } else if next_char == Some('*') {
                        // Block comment
                        self.advance(); // /
                        self.advance(); // *
                        while let Some(c) = self.current {
                            if c == '*' {
                                self.advance();
                                if self.current == Some('/') {
                                    self.advance();
                                    break;
                                }
                            } else {
                                self.advance();
                            }
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    fn parse_program(&mut self) -> Result<Program, Error> {
        let mut program = Program::new();
        let mut statements = Vec::new();

        self.skip_whitespace();

        while self.current.is_some() {
            self.skip_whitespace();
            if self.current.is_none() {
                break;
            }

            match self.parse_statement(&mut program) {
                Ok(Some(term_id)) => {
                    statements.push(term_id);
                }
                Ok(None) => {
                    self.skip_whitespace();
                    if self.current.is_none() {
                        break;
                    }
                    // Try to recover by skipping
                    self.advance();
                }
                Err(e) => return Err(e),
            }

            self.skip_whitespace();
        }

        // Set up control flow links
        for i in 0..statements.len() {
            let current = statements[i];
            if i > 0 {
                let prev = statements[i - 1];
                if let Some(term) = program.get_term_mut(current) {
                    term.control_flow_prev = Some(prev);
                }
                if let Some(term) = program.get_term_mut(prev) {
                    term.control_flow_next = Some(current);
                }
            }
        }

        if !statements.is_empty() {
            program.set_entry_point(statements[0]);
        } else {
            // Empty program returns nil
            let nil_const = program.add_constant(ConstantValue::Nil);
            let nil_term = program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(nil_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            });
            program.set_entry_point(nil_term);
        }

        Ok(program)
    }

    fn parse_statement(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        self.skip_whitespace();

        let start_line = self.line;
        let start_col = self.column;

        // Check for various statement types
        if self.match_keyword("let") {
            return self.parse_let_statement(program).map(Some);
        }

        if self.match_keyword("state") {
            return self.parse_state_statement(program).map(Some);
        }

        if self.match_keyword("fn") {
            return self.parse_function_def(program).map(|_| Ok(None))?;
        }

        if self.match_keyword("if") {
            return self.parse_if_statement(program).map(Some);
        }

        if self.match_keyword("for") {
            return self.parse_for_loop(program).map(Some);
        }

        if self.match_keyword("while") {
            return self.parse_while_loop(program).map(Some);
        }

        if self.match_keyword("return") {
            return self.parse_return_statement(program).map(Some);
        }

        if self.match_keyword("break") {
            self.expect_semicolon_or_newline();
            let term = program.add_term(Term {
                id: TermId(0),
                op: TermOp::Break,
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            });
            return Ok(Some(term));
        }

        if self.match_keyword("continue") {
            self.expect_semicolon_or_newline();
            let term = program.add_term(Term {
                id: TermId(0),
                op: TermOp::Continue,
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            });
            return Ok(Some(term));
        }

        // Try expression statement
        if let Some(expr) = self.parse_expression(program)? {
            // Check for assignment
            self.skip_whitespace();
            if self.current == Some('=') {
                self.advance(); // =
                if self.current == Some('=') {
                    // It was actually == comparison, backtrack
                    // This is simplified - we should handle this better
                } else {
                    // Assignment
                    let value = self.parse_expression(program)?
                        .ok_or_else(|| Error::ParseError(
                            format!("Expected expression after '=' at line {}, col {}",
                                    start_line, start_col)))?;
                    self.expect_semicolon_or_newline();

                    // Get the variable name from the left-hand side expression
                    let name = match program.get_term(expr) {
                        Some(term) => match &term.op {
                            TermOp::GetVariable(n) => n.clone(),
                            _ => return Err(Error::ParseError(
                                format!("Left-hand side of assignment must be a variable at line {}, col {}",
                                        start_line, start_col))),
                        },
                        None => return Err(Error::ParseError(
                            format!("Invalid left-hand side of assignment at line {}, col {}",
                                    start_line, start_col))),
                    };

                    let assign_term = program.add_term(Term {
                        id: TermId(0),
                        op: TermOp::Assign { name },
                        inputs: vec![value],
                        control_flow_next: None,
                        control_flow_prev: None,
                    });
                    return Ok(Some(assign_term));
                }
            }

            self.expect_semicolon_or_newline();
            return Ok(Some(expr));
        }

        Ok(None)
    }

    fn parse_let_statement(&mut self, program: &mut Program) -> Result<TermId, Error> {
        self.skip_whitespace();

        let name = self.parse_identifier()
            .ok_or_else(|| Error::ParseError(
                format!("Expected variable name after 'let' at line {}, col {}",
                        self.line, self.column)))?;

        self.skip_whitespace();

        // Type annotation (optional)
        if self.current == Some(':') {
            self.advance(); // :
            self.skip_whitespace();
            self.parse_type_annotation()?;
        }

        let init = if self.current == Some('=') {
            self.advance(); // =
            self.skip_whitespace();
            Some(self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected expression after '=' in let statement at line {}, col {}",
                        self.line, self.column)))?)
        } else {
            None
        };

        self.expect_semicolon_or_newline();

        let term = program.add_term(Term {
            id: TermId(0),
            op: TermOp::Let { name: name.clone() },
            inputs: init.into_iter().collect(),
            control_flow_next: None,
            control_flow_prev: None,
        });

        Ok(term)
    }

    fn parse_state_statement(&mut self, program: &mut Program) -> Result<TermId, Error> {
        self.skip_whitespace();

        let name = self.parse_identifier()
            .ok_or_else(|| Error::ParseError(
                format!("Expected variable name after 'state' at line {}, col {}",
                        self.line, self.column)))?;

        self.skip_whitespace();

        let initial = if self.current == Some('=') {
            self.advance(); // =
            self.skip_whitespace();
            Some(self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected expression after '=' in state statement at line {}, col {}",
                        self.line, self.column)))?)
        } else {
            None
        };

        self.expect_semicolon_or_newline();

        let op = if let Some(initial) = initial {
            TermOp::StateInit { name: name.clone(), initial_value: initial }
        } else {
            TermOp::StateRead { name: name.clone() }
        };

        let term = program.add_term(Term {
            id: TermId(0),
            op,
            inputs: initial.into_iter().collect(),
            control_flow_next: None,
            control_flow_prev: None,
        });

        Ok(term)
    }

    fn parse_function_def(&mut self, program: &mut Program) -> Result<(), Error> {
        self.skip_whitespace();

        let name = self.parse_identifier()
            .ok_or_else(|| Error::ParseError(
                format!("Expected function name after 'fn' at line {}, col {}",
                        self.line, self.column)))?;

        self.skip_whitespace();

        // Parse parameters
        if self.current != Some('(') {
            return Err(Error::ParseError(
                format!("Expected '(' after function name at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // (

        let mut params = Vec::new();
        self.skip_whitespace();

        while self.current.is_some() && self.current != Some(')') {
            let param = self.parse_identifier()
                .ok_or_else(|| Error::ParseError(
                    format!("Expected parameter name at line {}, col {}",
                            self.line, self.column)))?;
            params.push(param);

            self.skip_whitespace();

            if self.current == Some(',') {
                self.advance(); // ,
                self.skip_whitespace();
            } else {
                break;
            }
        }

        if self.current != Some(')') {
            return Err(Error::ParseError(
                format!("Expected ')' after parameters at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // )

        self.skip_whitespace();

        // Check for single-expression function (fn name() -> expr)
        if self.current == Some('-') {
            self.advance(); // -
            if self.current != Some('>') {
                return Err(Error::ParseError(
                    format!("Expected '>' after '-' in arrow function at line {}, col {}",
                            self.line, self.column)));
            }
            self.advance(); // >
            self.skip_whitespace();

            let expr = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected expression after '->' at line {}, col {}",
                        self.line, self.column)))?;

            // Create return term
            let return_term = program.add_term(Term {
                id: TermId(0),
                op: TermOp::Return,
                inputs: vec![expr],
                control_flow_next: None,
                control_flow_prev: None,
            });

            let function_id = FunctionId(program.functions.len() as u32 + 1);
            let def = FunctionDef {
                name: name.clone(),
                params,
                body_start: return_term,
            };

            program.register_function(name, function_id, def);

            return Ok(());
        }

        // Parse body block
        if self.current != Some('{') {
            return Err(Error::ParseError(
                format!("Expected '{{' before function body at line {}, col {}",
                        self.line, self.column)));
        }

        let body_statements = self.parse_block(program)?;

        let function_id = FunctionId(program.functions.len() as u32 + 1);

        let body_start = if body_statements.is_empty() {
            // Empty function returns nil
            let nil_const = program.add_constant(ConstantValue::Nil);
            program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(nil_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })
        } else {
            body_statements[0]
        };

        let def = FunctionDef {
            name: name.clone(),
            params,
            body_start,
        };

        program.register_function(name, function_id, def);

        Ok(())
    }

    fn parse_if_statement(&mut self, program: &mut Program) -> Result<TermId, Error> {
        self.skip_whitespace();

        if self.current != Some('(') {
            return Err(Error::ParseError(
                format!("Expected '(' after 'if' at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // (

        let condition = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
            format!("Expected condition in if statement at line {}, col {}",
                    self.line, self.column)))?;

        if self.current != Some(')') {
            return Err(Error::ParseError(
                format!("Expected ')' after condition at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // )

        self.skip_whitespace();

        // Parse then branch
        let then_statements = if self.current == Some('{') {
            self.parse_block(program)?
        } else {
            let stmt = self.parse_statement(program)?;
            stmt.into_iter().collect()
        };

        self.skip_whitespace();

        // Parse else branch (optional)
        let else_statements = if self.match_keyword("else") {
            self.skip_whitespace();
            if self.match_keyword("if") {
                // else if
                let elseif = self.parse_if_statement(program)?;
                vec![elseif]
            } else if self.current == Some('{') {
                self.parse_block(program)?
            } else {
                let stmt = self.parse_statement(program)?;
                stmt.into_iter().collect()
            }
        } else {
            Vec::new()
        };

        // Create branch term
        let then_term = then_statements.first().copied().unwrap_or_else(|| {
            let nil_const = program.add_constant(ConstantValue::Nil);
            program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(nil_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })
        });

        let else_term = else_statements.first().copied().unwrap_or_else(|| {
            let nil_const = program.add_constant(ConstantValue::Nil);
            program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(nil_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })
        });

        let term = program.add_term(Term {
            id: TermId(0),
            op: TermOp::Branch { then_term, else_term },
            inputs: vec![condition],
            control_flow_next: None,
            control_flow_prev: None,
        });

        Ok(term)
    }

    fn parse_for_loop(&mut self, program: &mut Program) -> Result<TermId, Error> {
        self.skip_whitespace();

        let var = self.parse_identifier()
            .ok_or_else(|| Error::ParseError(
                format!("Expected variable name after 'for' at line {}, col {}",
                        self.line, self.column)))?;

        self.skip_whitespace();

        if !self.match_keyword("in") {
            return Err(Error::ParseError(
                format!("Expected 'in' after loop variable at line {}, col {}",
                        self.line, self.column)));
        }

        let iter_expr = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
            format!("Expected iterable expression at line {}, col {}",
                    self.line, self.column)))?;

        self.skip_whitespace();

        // Parse body
        let body_statements = if self.current == Some('{') {
            self.parse_block(program)?
        } else {
            let stmt = self.parse_statement(program)?;
            stmt.into_iter().collect()
        };

        let body_start = body_statements.first().copied().unwrap_or_else(|| {
            let nil_const = program.add_constant(ConstantValue::Nil);
            program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(nil_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })
        });

        let nil_const = program.add_constant(ConstantValue::Nil);
        let end_term = program.add_term(Term {
            id: TermId(0),
            op: TermOp::Constant(nil_const),
            inputs: vec![],
            control_flow_next: None,
            control_flow_prev: None,
        });

        let term = program.add_term(Term {
            id: TermId(0),
            op: TermOp::ForLoop { var, iter_expr, body_start, end_term },
            inputs: vec![iter_expr],
            control_flow_next: None,
            control_flow_prev: None,
        });

        Ok(term)
    }

    fn parse_while_loop(&mut self, program: &mut Program) -> Result<TermId, Error> {
        self.skip_whitespace();

        let condition = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
            format!("Expected condition in while loop at line {}, col {}",
                    self.line, self.column)))?;

        self.skip_whitespace();

        // Parse body
        let body_statements = if self.current == Some('{') {
            self.parse_block(program)?
        } else {
            let stmt = self.parse_statement(program)?;
            stmt.into_iter().collect()
        };

        let body_start = body_statements.first().copied().unwrap_or_else(|| {
            let nil_const = program.add_constant(ConstantValue::Nil);
            program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(nil_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })
        });

        let nil_const = program.add_constant(ConstantValue::Nil);
        let end_term = program.add_term(Term {
            id: TermId(0),
            op: TermOp::Constant(nil_const),
            inputs: vec![],
            control_flow_next: None,
            control_flow_prev: None,
        });

        let term = program.add_term(Term {
            id: TermId(0),
            op: TermOp::WhileLoop { condition, body_start, end_term },
            inputs: vec![condition],
            control_flow_next: None,
            control_flow_prev: None,
        });

        Ok(term)
    }

    fn parse_return_statement(&mut self, program: &mut Program) -> Result<TermId, Error> {
        self.skip_whitespace();

        let value = if self.current == Some(';') || self.current == Some('\n') || self.current.is_none() {
            None
        } else {
            self.parse_expression(program)?
        };

        self.expect_semicolon_or_newline();

        let term = program.add_term(Term {
            id: TermId(0),
            op: TermOp::Return,
            inputs: value.into_iter().collect(),
            control_flow_next: None,
            control_flow_prev: None,
        });

        Ok(term)
    }

    fn parse_block(&mut self, program: &mut Program) -> Result<Vec<TermId>, Error> {
        if self.current != Some('{') {
            return Err(Error::ParseError(
                format!("Expected '{{' at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // {

        let mut statements = Vec::new();

        self.skip_whitespace();

        while self.current.is_some() && self.current != Some('}') {
            if let Some(stmt) = self.parse_statement(program)? {
                statements.push(stmt);
            }
            self.skip_whitespace();
        }

        if self.current != Some('}') {
            return Err(Error::ParseError(
                format!("Expected '}}' at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // }

        // Set up control flow links within the block
        for i in 0..statements.len() {
            let current = statements[i];
            if i > 0 {
                let prev = statements[i - 1];
                if let Some(term) = program.get_term_mut(current) {
                    term.control_flow_prev = Some(prev);
                }
                if let Some(term) = program.get_term_mut(prev) {
                    term.control_flow_next = Some(current);
                }
            }
        }

        Ok(statements)
    }

    fn parse_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        self.parse_or_expression(program)
    }

    fn parse_or_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let mut left = self.parse_and_expression(program)?;

        while left.is_some() {
            self.skip_whitespace();
            if self.match_keyword("or") || self.current == Some('|') && self.peek() == Some('|') {
                if self.current == Some('|') {
                    self.advance();
                    self.advance();
                }

                let right = self.parse_and_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected expression after 'or' at line {}, col {}",
                            self.line, self.column)))?;

                left = Some(program.add_term(Term {
                    id: TermId(0),
                    op: TermOp::Or,
                    inputs: vec![left.unwrap(), right],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_and_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let mut left = self.parse_equality_expression(program)?;

        while left.is_some() {
            self.skip_whitespace();
            if self.match_keyword("and") || self.current == Some('&') && self.peek() == Some('&') {
                if self.current == Some('&') {
                    self.advance();
                    self.advance();
                }

                let right = self.parse_equality_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected expression after 'and' at line {}, col {}",
                            self.line, self.column)))?;

                left = Some(program.add_term(Term {
                    id: TermId(0),
                    op: TermOp::And,
                    inputs: vec![left.unwrap(), right],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_equality_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let mut left = self.parse_comparison_expression(program)?;

        while left.is_some() {
            self.skip_whitespace();
            if self.current == Some('=') && self.peek() == Some('=') {
                self.advance(); // =
                self.advance(); // =

                let right = self.parse_comparison_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected expression after '==' at line {}, col {}",
                            self.line, self.column)))?;

                left = Some(program.add_term(Term {
                    id: TermId(0),
                    op: TermOp::Eq,
                    inputs: vec![left.unwrap(), right],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
            } else if self.current == Some('!') && self.peek() == Some('=') {
                self.advance(); // !
                self.advance(); // =

                let right = self.parse_comparison_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected expression after '!=' at line {}, col {}",
                            self.line, self.column)))?;

                left = Some(program.add_term(Term {
                    id: TermId(0),
                    op: TermOp::Ne,
                    inputs: vec![left.unwrap(), right],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_comparison_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let mut left = self.parse_additive_expression(program)?;

        while left.is_some() {
            self.skip_whitespace();
            let op = if self.current == Some('<') {
                if self.peek() == Some('=') {
                    self.advance(); // <
                    self.advance(); // =
                    Some(TermOp::Le)
                } else {
                    self.advance(); // <
                    Some(TermOp::Lt)
                }
            } else if self.current == Some('>') {
                if self.peek() == Some('=') {
                    self.advance(); // >
                    self.advance(); // =
                    Some(TermOp::Ge)
                } else {
                    self.advance(); // >
                    Some(TermOp::Gt)
                }
            } else {
                None
            };

            if let Some(term_op) = op {
                let right = self.parse_additive_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected expression after comparison operator at line {}, col {}",
                            self.line, self.column)))?;

                left = Some(program.add_term(Term {
                    id: TermId(0),
                    op: term_op,
                    inputs: vec![left.unwrap(), right],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_additive_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let mut left = self.parse_multiplicative_expression(program)?;

        while left.is_some() {
            self.skip_whitespace();
            let op = if self.current == Some('+') {
                self.advance();
                Some(TermOp::Add)
            } else if self.current == Some('-') {
                self.advance();
                Some(TermOp::Sub)
            } else {
                None
            };

            if let Some(term_op) = op {
                let right = self.parse_multiplicative_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected expression after '+'/'-' at line {}, col {}",
                            self.line, self.column)))?;

                left = Some(program.add_term(Term {
                    id: TermId(0),
                    op: term_op,
                    inputs: vec![left.unwrap(), right],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_multiplicative_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let mut left = self.parse_power_expression(program)?;

        while left.is_some() {
            self.skip_whitespace();
            let op = if self.current == Some('*') {
                self.advance();
                Some(TermOp::Mul)
            } else if self.current == Some('/') {
                self.advance();
                Some(TermOp::Div)
            } else if self.current == Some('%') {
                self.advance();
                Some(TermOp::Mod)
            } else {
                None
            };

            if let Some(term_op) = op {
                let right = self.parse_power_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected expression after '*'/'/'/'%' at line {}, col {}",
                            self.line, self.column)))?;

                left = Some(program.add_term(Term {
                    id: TermId(0),
                    op: term_op,
                    inputs: vec![left.unwrap(), right],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_power_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let mut left = self.parse_unary_expression(program)?;

        if left.is_some() {
            self.skip_whitespace();
            if self.current == Some('*') && self.peek() == Some('*') {
                self.advance(); // *
                self.advance(); // *

                let right = self.parse_unary_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected expression after '**' at line {}, col {}",
                            self.line, self.column)))?;

                left = Some(program.add_term(Term {
                    id: TermId(0),
                    op: TermOp::Pow,
                    inputs: vec![left.unwrap(), right],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
            }
        }

        Ok(left)
    }

    fn parse_unary_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        self.skip_whitespace();

        if self.current == Some('-') {
            self.advance(); // -

            let operand = self.parse_unary_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected expression after '-' at line {}, col {}",
                        self.line, self.column)))?;

            // Create 0 - operand
            let zero_const = program.add_constant(ConstantValue::Int(0));
            let zero = program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(zero_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            });

            return Ok(Some(program.add_term(Term {
                id: TermId(0),
                op: TermOp::Sub,
                inputs: vec![zero, operand],
                control_flow_next: None,
                control_flow_prev: None,
            })));
        }

        if self.current == Some('!') || self.match_keyword("not") {
            if self.current == Some('!') {
                self.advance(); // !
            }

            let operand = self.parse_unary_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected expression after '!'/'not' at line {}, col {}",
                        self.line, self.column)))?;

            return Ok(Some(program.add_term(Term {
                id: TermId(0),
                op: TermOp::Not,
                inputs: vec![operand],
                control_flow_next: None,
                control_flow_prev: None,
            })));
        }

        self.parse_postfix_expression(program)
    }

    fn parse_postfix_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let mut expr = self.parse_primary_expression(program)?;

        loop {
            self.skip_whitespace();

            if self.current.is_none() {
                break;
            }

            // Function call
            if self.current == Some('(') {
                if let Some(func_expr) = expr {
                    expr = self.parse_call_expression(program, func_expr)?;
                    continue;
                }
            }

            // Index access
            if self.current == Some('[') {
                self.advance(); // [
                let index = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
                    format!("Expected index expression at line {}, col {}",
                            self.line, self.column)))?;

                if self.current != Some(']') {
                    return Err(Error::ParseError(
                        format!("Expected ']' at line {}, col {}",
                                self.line, self.column)));
                }
                self.advance(); // ]

                expr = Some(program.add_term(Term {
                    id: TermId(0),
                    op: TermOp::GetIndex,
                    inputs: vec![expr.unwrap(), index],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
                continue;
            }

            // Field access
            if self.current == Some('.') {
                self.advance(); // .
                let field = self.parse_identifier().ok_or_else(|| Error::ParseError(
                    format!("Expected field name at line {}, col {}",
                            self.line, self.column)))?;

                expr = Some(program.add_term(Term {
                    id: TermId(0),
                    op: TermOp::GetField { field },
                    inputs: vec![expr.unwrap()],
                    control_flow_next: None,
                    control_flow_prev: None,
                }));
                continue;
            }

            break;
        }

        Ok(expr)
    }

    fn parse_call_expression(&mut self, program: &mut Program, func: TermId) -> Result<Option<TermId>, Error> {
        self.advance(); // (

        let mut args = vec![func];
        self.skip_whitespace();

        while self.current.is_some() && self.current != Some(')') {
            let arg = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected argument at line {}, col {}",
                        self.line, self.column)))?;
            args.push(arg);

            self.skip_whitespace();

            if self.current == Some(',') {
                self.advance(); // ,
                self.skip_whitespace();
            } else {
                break;
            }
        }

        if self.current != Some(')') {
            return Err(Error::ParseError(
                format!("Expected ')' at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // )

        // Determine if it's a builtin or user function
        let op = if let Some(term) = program.get_term(func) {
            match &term.op {
                TermOp::GetVariable(name) => {
                    // Could be builtin - we'll determine at runtime
                    TermOp::CallBuiltin { name: name.clone() }
                }
                _ => TermOp::Call { function: FunctionId(0) }, // Will be resolved later
            }
        } else {
            TermOp::Call { function: FunctionId(0) }
        };

        Ok(Some(program.add_term(Term {
            id: TermId(0),
            op,
            inputs: args,
            control_flow_next: None,
            control_flow_prev: None,
        })))
    }

    fn parse_primary_expression(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        self.skip_whitespace();

        // Parenthesized expression
        if self.current == Some('(') {
            self.advance(); // (
            let expr = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected expression after '(' at line {}, col {}",
                        self.line, self.column)))?;

            if self.current != Some(')') {
                return Err(Error::ParseError(
                    format!("Expected ')' at line {}, col {}",
                            self.line, self.column)));
            }
            self.advance(); // )
            return Ok(Some(expr));
        }

        // Lambda expression (fn (params) -> expr)
        if self.peek_keyword("fn") {
            // Parse inline function - for now just return nil
            self.advance();
            self.advance();
            // Skip to end of lambda for now
            while self.current.is_some() && self.current != Some(')') {
                self.advance();
            }
            if self.current == Some(')') {
                self.advance();
            }
            while self.current.is_some() && self.current != Some('}') {
                self.advance();
            }
            if self.current == Some('}') {
                self.advance();
            }

            let nil_const = program.add_constant(ConstantValue::Nil);
            return Ok(Some(program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(nil_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })));
        }

        // Boolean literals
        if self.match_keyword("true") {
            let const_id = program.add_constant(ConstantValue::Bool(true));
            return Ok(Some(program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(const_id),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })));
        }

        if self.match_keyword("false") {
            let const_id = program.add_constant(ConstantValue::Bool(false));
            return Ok(Some(program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(const_id),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })));
        }

        if self.match_keyword("null") || self.match_keyword("nil") {
            let const_id = program.add_constant(ConstantValue::Nil);
            return Ok(Some(program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(const_id),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            })));
        }

        // Number literal
        if let Some(number) = self.parse_number_literal(program)? {
            return Ok(Some(number));
        }

        // String literal
        if self.current == Some('"') || self.current == Some('\'') {
            if let Some(string) = self.parse_string_literal(program)? {
                return Ok(Some(string));
            }
        }

        // Array literal
        if self.current == Some('[') {
            return self.parse_array_literal(program);
        }

        // Object literal
        if self.current == Some('{') {
            return self.parse_object_literal(program);
        }

        // Identifier
        if let Some(name) = self.parse_identifier() {
            let term = program.add_term(Term {
                id: TermId(0),
                op: TermOp::GetVariable(name),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            });
            return Ok(Some(term));
        }

        Ok(None)
    }

    fn parse_number_literal(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let start = self.position;

        // Check for negative
        if self.current == Some('-') {
            // Look ahead to see if followed by digit
            let next_pos = self.position + 1;
            if next_pos < self.source.len() {
                let next_char = self.source.chars().nth(next_pos);
                if next_char.map(|c| c.is_ascii_digit()).unwrap_or(false) {
                    // This will be handled by unary minus
                    return Ok(None);
                }
            }
        }

        // Integer or float
        if !self.current.map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return Ok(None);
        }

        let mut has_dot = false;
        while let Some(c) = self.current {
            if c.is_ascii_digit() {
                self.advance();
            } else if c == '.' && !has_dot {
                // Check next char is also digit for float
                let next_pos = self.position + 1;
                if next_pos < self.source.len() {
                    let next_char = self.source.chars().nth(next_pos);
                    if next_char.map(|c| c.is_ascii_digit()).unwrap_or(false) {
                        has_dot = true;
                        self.advance();
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let num_str = &self.source[start..self.position];

        let term = if has_dot {
            match num_str.parse::<f64>() {
                Ok(f) => {
                    let const_id = program.add_constant(ConstantValue::Float(f));
                    program.add_term(Term {
                        id: TermId(0),
                        op: TermOp::Constant(const_id),
                        inputs: vec![],
                        control_flow_next: None,
                        control_flow_prev: None,
                    })
                }
                Err(_) => return Err(Error::ParseError(
                    format!("Invalid float literal '{}' at line {}, col {}",
                            num_str, self.line, self.column))),
            }
        } else {
            match num_str.parse::<i64>() {
                Ok(i) => {
                    let const_id = program.add_constant(ConstantValue::Int(i));
                    program.add_term(Term {
                        id: TermId(0),
                        op: TermOp::Constant(const_id),
                        inputs: vec![],
                        control_flow_next: None,
                        control_flow_prev: None,
                    })
                }
                Err(_) => return Err(Error::ParseError(
                    format!("Invalid integer literal '{}' at line {}, col {}",
                            num_str, self.line, self.column))),
            }
        };

        Ok(Some(term))
    }

    fn parse_string_literal(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        let quote = self.current.unwrap();
        self.advance(); // opening quote

        let start = self.position;
        let mut result = String::new();

        while let Some(c) = self.current {
            if c == quote {
                self.advance(); // closing quote
                break;
            } else if c == '\\' {
                self.advance(); // \
                match self.current {
                    Some('n') => { result.push('\n'); self.advance(); }
                    Some('t') => { result.push('\t'); self.advance(); }
                    Some('r') => { result.push('\r'); self.advance(); }
                    Some('\\') => { result.push('\\'); self.advance(); }
                    Some('"') => { result.push('"'); self.advance(); }
                    Some('\'') => { result.push('\''); self.advance(); }
                    Some(c) => { result.push(c); self.advance(); }
                    None => break,
                }
            } else {
                result.push(c);
                self.advance();
            }
        }

        let const_id = program.add_constant(ConstantValue::String(result));
        Ok(Some(program.add_term(Term {
            id: TermId(0),
            op: TermOp::Constant(const_id),
            inputs: vec![],
            control_flow_next: None,
            control_flow_prev: None,
        })))
    }

    fn parse_array_literal(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        self.advance(); // [

        let mut elements = Vec::new();
        self.skip_whitespace();

        while self.current.is_some() && self.current != Some(']') {
            let elem = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected expression in array literal at line {}, col {}",
                        self.line, self.column)))?;
            elements.push(elem);

            self.skip_whitespace();

            if self.current == Some(',') {
                self.advance(); // ,
                self.skip_whitespace();
            } else {
                break;
            }
        }

        if self.current != Some(']') {
            return Err(Error::ParseError(
                format!("Expected ']' at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // ]

        Ok(Some(program.add_term(Term {
            id: TermId(0),
            op: TermOp::CreateList,
            inputs: elements,
            control_flow_next: None,
            control_flow_prev: None,
        })))
    }

    fn parse_object_literal(&mut self, program: &mut Program) -> Result<Option<TermId>, Error> {
        self.advance(); // {

        let mut fields = Vec::new();
        self.skip_whitespace();

        while self.current.is_some() && self.current != Some('}') {
            let key = if self.current == Some('"') || self.current == Some('\'') {
                // Parse string key
                self.advance(); // quote
                let start = self.position;
                while self.current.is_some() && self.current != Some('"') && self.current != Some('\'') {
                    self.advance();
                }
                let key_str = self.source[start..self.position].to_string();
                if self.current == Some('"') || self.current == Some('\'') {
                    self.advance(); // closing quote
                }
                key_str
            } else {
                // Parse identifier key
                self.parse_identifier().ok_or_else(|| Error::ParseError(
                    format!("Expected field name at line {}, col {}",
                            self.line, self.column)))?
            };

            self.skip_whitespace();

            if self.current != Some(':') {
                return Err(Error::ParseError(
                    format!("Expected ':' after field name at line {}, col {}",
                            self.line, self.column)));
            }
            self.advance(); // :

            self.skip_whitespace();

            let value = self.parse_expression(program)?.ok_or_else(|| Error::ParseError(
                format!("Expected value for field '{}' at line {}, col {}",
                        key, self.line, self.column)))?;

            // Create field entry as a key-value pair term
            let key_const = program.add_constant(ConstantValue::String(key));
            let key_term = program.add_term(Term {
                id: TermId(0),
                op: TermOp::Constant(key_const),
                inputs: vec![],
                control_flow_next: None,
                control_flow_prev: None,
            });
            fields.push(key_term);
            fields.push(value);

            self.skip_whitespace();

            if self.current == Some(',') {
                self.advance(); // ,
                self.skip_whitespace();
            } else {
                break;
            }
        }

        if self.current != Some('}') {
            return Err(Error::ParseError(
                format!("Expected '}}' at line {}, col {}",
                        self.line, self.column)));
        }
        self.advance(); // }

        Ok(Some(program.add_term(Term {
            id: TermId(0),
            op: TermOp::CreateMap,
            inputs: fields,
            control_flow_next: None,
            control_flow_prev: None,
        })))
    }

    fn parse_identifier(&mut self) -> Option<String> {
        let start = self.position;

        if let Some(c) = self.current {
            if !c.is_ascii_alphabetic() && c != '_' {
                return None;
            }
        } else {
            return None;
        }

        while let Some(c) = self.current {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }

        Some(self.source[start..self.position].to_string())
    }

    fn parse_type_annotation(&mut self) -> Result<(), Error> {
        // For now, just skip the type annotation
        // Parse identifier or complex type
        self.skip_whitespace();

        if self.parse_identifier().is_none() {
            return Err(Error::ParseError(
                format!("Expected type name at line {}, col {}",
                        self.line, self.column)));
        }

        // Handle generic types, arrays, etc.
        self.skip_whitespace();
        if self.current == Some('<') {
            // Generic type parameter
            self.advance(); // <
            self.skip_whitespace();
            self.parse_type_annotation()?;
            self.skip_whitespace();
            if self.current == Some('>') {
                self.advance(); // >
            }
        }

        if self.current == Some('[') {
            // Array type
            self.advance(); // [
            self.skip_whitespace();
            if self.current == Some(']') {
                self.advance(); // ]
            }
        }

        Ok(())
    }

    fn match_keyword(&mut self, keyword: &str) -> bool {
        let start = self.position;
        let start_line = self.line;
        let start_col = self.column;

        if let Some(ident) = self.parse_identifier() {
            if ident == keyword {
                // Make sure it's not a prefix of another identifier
                if let Some(c) = self.current {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        // Backtrack
                        self.position = start;
                        self.line = start_line;
                        self.column = start_col;
                        // Reset chars iterator
                        self.chars = self.source[self.position..].chars();
                        self.current = self.chars.next();
                        return false;
                    }
                }
                return true;
            }
        }

        // Backtrack
        self.position = start;
        self.line = start_line;
        self.column = start_col;
        self.chars = self.source[self.position..].chars();
        self.current = self.chars.next();
        false
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        let remaining = &self.source[self.position..];
        if remaining.starts_with(keyword) {
            // Make sure it's not a prefix
            let after = &remaining[keyword.len()..];
            if after.is_empty() || !after.chars().next().unwrap().is_ascii_alphanumeric() {
                return true;
            }
        }
        false
    }

    fn peek(&self) -> Option<char> {
        let next_pos = self.position + 1;
        if next_pos < self.source.len() {
            self.source.chars().nth(next_pos)
        } else {
            None
        }
    }

    fn expect_semicolon_or_newline(&mut self) {
        self.skip_whitespace();
        if self.current == Some(';') {
            self.advance();
        }
        // Newlines are handled naturally by skip_whitespace
    }
}

impl PartialEq for ConstantValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ConstantValue::Nil, ConstantValue::Nil) => true,
            (ConstantValue::Bool(a), ConstantValue::Bool(b)) => a == b,
            (ConstantValue::Int(a), ConstantValue::Int(b)) => a == b,
            (ConstantValue::Float(a), ConstantValue::Float(b)) => a.to_bits() == b.to_bits(),
            (ConstantValue::String(a), ConstantValue::String(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for ConstantValue {}
