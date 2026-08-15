/**
 * @file Petal grammar for tree-sitter
 * @author Petal
 * @license MIT
 *
 * A reference tree-sitter grammar for the Petal language. It models the
 * surface syntax tokenized by `rust/src/lexer.rs` and parsed by
 * `rust/src/parse.rs`, primarily so editors can syntax-highlight `.ptl` files.
 *
 * Newlines are treated as insignificant (part of `extras`): the real parser
 * uses them as statement separators, but statement boundaries are recoverable
 * from grammar structure in practice. See the README.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// Precedence ladder, lowest → highest, mirroring the precedence-climbing
// parser in rust/src/parse.rs (pipe → or → and → equality → comparison →
// coalesce → concat → additive → multiplicative → unary → postfix).
//
// `??` sits between comparison and concat, exactly where `parse_coalesce`
// sits in the Rust parser: `count ?? 0 > 5` is `(count ?? 0) > 5` and
// `"a" ++ b ?? "x"` is `("a" ++ b) ?? "x"`.
const PREC = {
  assign: -1,
  pipe: 1,
  or: 2,
  and: 3,
  equality: 4,
  comparison: 5,
  coalesce: 6,
  concat: 7,
  additive: 8,
  multiplicative: 9,
  unary: 10,
  postfix: 11,
  jsx: 12,
};

/**
 * A delimited list. A comma is required between adjacent elements; a trailing
 * comma before the closing delimiter is allowed.
 * See docs/syntax/commas.md.
 * @param {RuleOrLiteral} rule
 */
function commaSep(rule) {
  return optional(seq(rule, repeat(seq(',', rule)), optional(',')));
}

module.exports = grammar({
  name: 'petal',

  // Keyword extraction needs a single token; `identifier` is a two-token
  // sequence (see its definition), so the core regex is the word token.
  word: $ => $._identifier_core,

  extras: $ => [
    // The lexer turns `;` into a newline-style separator; with newlines
    // insignificant, both are simply ignorable here.
    /[ \t\r\n;]/,
    $.comment,
  ],

  rules: {
    source_file: $ => repeat($._statement),

    // ---- Statements ----

    _statement: $ => choice(
      $.import_statement,
      $.let_declaration,
      $.state_declaration,
      $.function_declaration,
      $.enum_declaration,
      $.class_declaration,
      $.for_statement,
      $.while_statement,
      $.return_statement,
      $.break_statement,
      $.continue_statement,
      $.assignment,
      $.set_statement,
      $._expression,
    ),

    // Three module-import forms (see docs/module-system.md):
    //   import ui                    (qualified)
    //   import ui: button, clicked   (selective, comma-separated names)
    //   import ui as u               (aliased)
    // `as` is contextual in the real parser; here it is a keyword literal,
    // which is safe because a syntax-highlighting grammar never needs `as`
    // as an ordinary identifier. The selective list requires commas between
    // names, matching the parser and stopping the list at the next statement.
    import_statement: $ => seq(
      'import',
      field('module', $.identifier),
      optional(choice(
        seq('as', field('alias', $.identifier)),
        seq(
          ':',
          field('name', $.identifier),
          repeat(seq(',', field('name', $.identifier))),
        ),
      )),
    ),

    // `export` is a prefix on a declaration, not a wrapper node: the real
    // parser (rust/src/parse.rs `parse_export`) dispatches straight into
    // `parse_fn_decl` / `parse_let` / `parse_state` / `parse_enum_decl` with an
    // `exported` flag, producing the same node with one extra leading token.
    // Mirroring that keeps the tree shapes identical. Those five forms — fn,
    // let, var, state, enum — are exactly what `export` may precede.

    // `let x = v` and its mutable twin `var x = v`, each with an optional
    // `: type` annotation.
    let_declaration: $ => seq(
      optional('export'),
      choice('let', 'var'),
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
      '=',
      field('value', $._expression),
    ),

    // `state name = init` or keyed `state(expr) name = init`, with the same
    // optional `: type` annotation `let`/`var` take.
    state_declaration: $ => seq(
      optional('export'),
      'state',
      optional(seq('(', field('key', $._expression), ')')),
      optional('var'),
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
      '=',
      field('value', $._expression),
    ),

    // `fn name(...)` or the method form `fn Class.name(...)`, whose receiver
    // class is the name before the dot (rust/src/parse.rs `parse_fn_decl`).
    function_declaration: $ => seq(
      optional('export'),
      'fn',
      optional(seq(field('class', $.identifier), '.')),
      field('name', $.identifier),
      field('parameters', $.parameter_list),
      optional(field('return_type', $.return_type)),
      optional(field('body', $.block)),
      'end',
    ),

    // Optional static type declarations (docs/syntax/types.md). Type names are
    // *contextual* — `int`/`float`/`str` stay callable builtins elsewhere — so
    // a type is just an identifier here, plus the two vocabulary names that lex
    // as keywords (`nil`, `enum`). Unrecognized names parse fine; the real
    // checker warns about them.
    type_annotation: $ => seq(':', field('type', $.type_name)),

    // `-> t` on a *named* fn only. A lambda's `->` introduces its body, so
    // lambdas take no return annotation (type-declarations-plan.md §2).
    return_type: $ => seq('->', field('type', $.type_name)),

    type_name: $ => choice($.identifier, 'nil', 'enum'),

    parameter_list: $ => seq('(', commaSep($.parameter), ')'),

    parameter: $ => seq(
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
    ),

    // `class Name` … `end`, comma-separated `field: type` declarations — the
    // same comma rule as `enum` (docs/syntax/commas.md). `class` is a
    // *contextual* keyword in the real lexer (it stays usable as an identifier
    // and as the JSX `class=` attribute); as with `as`, a highlighting grammar
    // can treat it as a literal.
    class_declaration: $ => seq(
      'class',
      field('name', $.identifier),
      commaSep($.class_field),
      'end',
    ),

    class_field: $ => seq(
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
    ),

    enum_declaration: $ => seq(
      optional('export'),
      'enum',
      field('name', $.identifier),
      commaSep($.enum_variant),
      'end',
    ),

    enum_variant: $ => seq(
      field('name', $.identifier),
      optional(field('fields', $.parameter_list)),
    ),

    for_statement: $ => seq(
      'for',
      field('variable', $.identifier),
      'in',
      field('iterator', $._expression),
      'do',
      optional(field('body', $.block)),
      'end',
    ),

    while_statement: $ => seq(
      'while',
      field('condition', $._expression),
      'do',
      optional(field('body', $.block)),
      'end',
    ),

    return_statement: $ => prec.right(seq('return', optional($._expression))),

    break_statement: _ => 'break',

    continue_statement: _ => 'continue',

    assignment: $ => prec.right(PREC.assign, seq(
      field('left', $._expression),
      field('operator', choice('=', '+=', '-=', '*=', '/=', '%=')),
      field('right', $._expression),
    )),

    // `set x = v` — a write through a `var` cell. Same shape as `assignment`,
    // under its own node because it is a different statement.
    set_statement: $ => prec.right(PREC.assign, seq(
      'set',
      field('left', $._expression),
      field('operator', choice('=', '+=', '-=', '*=', '/=', '%=')),
      field('right', $._expression),
    )),

    // A run of statements; the `end`/`else`/`elsif`/`when` that closes the
    // enclosing form is a keyword, so the repeat stops there naturally.
    block: $ => repeat1($._statement),

    // ---- Expressions ----

    _expression: $ => choice(
      $.identifier,
      $.integer,
      $.float,
      $.string,
      $.raw_string,
      $.boolean,
      $.nil,
      $.color,
      $.list,
      $.record,
      $.lambda,
      $.if_expression,
      $.match_expression,
      $.unary_expression,
      $.get_expression,
      $.binary_expression,
      $.pipe_expression,
      $.call,
      $.field_access,
      $.index_access,
      $.parenthesized_expression,
      $.jsx_element,
    ),

    parenthesized_expression: $ => seq('(', $._expression, ')'),

    unary_expression: $ => prec.right(PREC.unary, seq(
      field('operator', choice('-', '!')),
      field('operand', $._expression),
    )),

    // `get x` — a read of a `var` cell. Required wherever the read crosses a
    // function boundary from the declaration, so a captured snapshot and a
    // live cell read never look alike.
    get_expression: $ => prec.right(PREC.unary, seq(
      'get',
      field('operand', $._expression),
    )),

    binary_expression: $ => {
      const table = [
        ['||', PREC.or],
        ['&&', PREC.and],
        ['==', PREC.equality],
        ['!=', PREC.equality],
        ['<', PREC.comparison],
        ['<=', PREC.comparison],
        ['>', PREC.comparison],
        ['>=', PREC.comparison],
        ['??', PREC.coalesce],
        ['++', PREC.concat],
        ['+', PREC.additive],
        ['-', PREC.additive],
        ['*', PREC.multiplicative],
        ['/', PREC.multiplicative],
        ['%', PREC.multiplicative],
      ];
      return choice(...table.map(([op, p]) => prec.left(p, seq(
        field('left', $._expression),
        field('operator', op),
        field('right', $._expression),
      ))));
    },

    pipe_expression: $ => prec.left(PREC.pipe, seq(
      field('left', $._expression),
      '|>',
      field('right', $._expression),
    )),

    call: $ => prec(PREC.postfix, seq(
      field('function', $._expression),
      field('arguments', $.argument_list),
    )),

    argument_list: $ => seq('(', commaSep($._expression), ')'),

    // `?.` is the tolerant spelling: `rec?.missing` is nil rather than an
    // error, and one `?.` makes the rest of its chain tolerant too. It is a
    // read-only form — `a?.b = v` is rejected by the Rust parser, a rule the
    // grammar does not try to encode (an assignment target is a plain
    // `_expression` here).
    field_access: $ => prec(PREC.postfix, seq(
      field('object', $._expression),
      field('operator', choice('.', '?.')),
      field('field', $.identifier),
    )),

    // The `[` is immediate (no whitespace before it) so that real indexing
    // (`arr[i]`) is distinguished from an expression statement followed by a
    // list literal on the next line — newlines being insignificant here.
    // `a?.[i]` is the optional spelling, matching JavaScript. It is lexed as
    // the single token `?.[` rather than `?.` followed by `[`: as two tokens
    // the state after `?.` is ambiguous between this rule and `field_access`,
    // and the generator resolves it toward a field name, misparsing `a?.[0]`
    // as `a?.<missing>` indexed by `0`. One token makes the lexer's
    // longest-match decide, which needs no lookahead at all.
    index_access: $ => prec(PREC.postfix, seq(
      field('object', $._expression),
      choice(token.immediate('['), field('operator', '?.[')),
      field('index', $._expression),
      ']',
    )),

    list: $ => seq('[', commaSep($._expression), ']'),

    record: $ => seq('{', commaSep(choice($.record_field, $.spread_element)), '}'),

    record_field: $ => seq(
      field('key', $.identifier),
      ':',
      field('value', $._expression),
    ),

    spread_element: $ => seq('...', $._expression),

    // `fn(params) -> expr` or `fn(params) body end`.
    lambda: $ => seq(
      'fn',
      field('parameters', $.parameter_list),
      choice(
        seq('->', field('body', $._expression)),
        seq(optional(field('body', $.block)), 'end'),
      ),
    ),

    if_expression: $ => seq(
      'if',
      field('condition', $._expression),
      'then',
      optional(field('consequence', $.block)),
      repeat($.elsif_clause),
      optional($.else_clause),
      'end',
    ),

    elsif_clause: $ => seq(
      'elsif',
      field('condition', $._expression),
      'then',
      optional(field('consequence', $.block)),
    ),

    else_clause: $ => seq('else', optional(field('body', $.block))),

    match_expression: $ => seq(
      'match',
      field('subject', $._expression),
      repeat($.match_arm),
      'end',
    ),

    match_arm: $ => seq(
      'when',
      field('pattern', $._pattern),
      optional($.guard),
      choice(
        seq('->', field('body', $._expression)),
        seq('do', optional(field('body', $.block)), 'end'),
      ),
    ),

    guard: $ => seq('if', field('condition', $._expression)),

    // ---- Patterns ----

    _pattern: $ => choice(
      $.variant_pattern,
      $.list_pattern,
      $.record_pattern,
      $.literal_pattern,
      $.identifier, // variable binding or `_` wildcard
    ),

    variant_pattern: $ => prec(1, seq(
      field('name', $.identifier),
      '(',
      commaSep($._pattern),
      ')',
    )),

    list_pattern: $ => seq(
      '[',
      commaSep(choice($._pattern, $.rest_pattern)),
      ']',
    ),

    rest_pattern: $ => seq('...', $.identifier),

    record_pattern: $ => seq(
      '{',
      commaSep($.record_pattern_field),
      '}',
    ),

    record_pattern_field: $ => seq(
      field('key', $.identifier),
      ':',
      field('value', $._pattern),
    ),

    literal_pattern: $ => choice(
      $.integer,
      $.float,
      $.string,
      $.boolean,
      $.nil,
      seq('-', choice($.integer, $.float)),
    ),

    // ---- JSX (React-like elements, see integrations/petal-web-html) ----

    jsx_element: $ => prec(PREC.jsx, choice(
      $.jsx_self_closing_element,
      seq(
        $.jsx_opening_element,
        repeat($._jsx_child),
        $.jsx_closing_element,
      ),
    )),

    jsx_opening_element: $ => seq(
      field('name', $.jsx_open_tag),
      repeat($.jsx_attribute),
      '>',
    ),

    jsx_self_closing_element: $ => seq(
      field('name', $.jsx_open_tag),
      repeat($.jsx_attribute),
      '/>',
    ),

    jsx_closing_element: $ => seq(field('name', $.jsx_close_tag), '>'),

    // `<tag` and `</tag` are single tokens: a `<` (or `</`) immediately
    // followed by a letter is unambiguously a JSX tag start, never the `<`
    // operator — exactly how the lexer disambiguates (lexer.rs reads a JSX tag
    // when `<` is followed by an alphabetic char). Because this token is longer
    // than the `<` operator, tree-sitter's lexer prefers it whenever it
    // applies, so a comparison must be written with a space (`a < b`).
    jsx_open_tag: _ => token(seq('<', /[a-zA-Z][a-zA-Z0-9_.-]*/)),

    jsx_close_tag: _ => token(seq('</', /[a-zA-Z][a-zA-Z0-9_.-]*/)),

    jsx_attribute: $ => seq(
      field('name', $.identifier),
      '=',
      field('value', choice($.string, $.jsx_expression)),
    ),

    jsx_expression: $ => seq('{', optional($._expression), '}'),

    jsx_text: _ => token(prec(-1, /[^<>{}]+/)),

    _jsx_child: $ => choice(
      $.jsx_element,
      $.jsx_expression,
      $.jsx_text,
    ),

    // ---- Literals & terminals ----

    // Identifiers may end in `?` (e.g. `even?`). That trailing `?` is a
    // *separate* immediate token rather than part of the identifier regex,
    // because `rec?.field` and `a??b` have to split the other way. Folding it
    // into the regex makes the identifier DFA swallow the `?` — longest match
    // — so `rec?.field` lexes as the identifier `rec?` followed by a stray `.`
    // and `xs?.[0]` fails outright. As its own token the choice is made at the
    // `?` by longest match among the operators valid there: `?.` and `??` (two
    // characters) beat a bare `?` (one), which is exactly the rule
    // `read_identifier` applies in rust/src/lexer.rs.
    //
    // `identifier` therefore stays a node — every use site is unchanged — but
    // is no longer a single token, so `word` names the core regex instead.
    identifier: $ => seq($._identifier_core, optional(token.immediate('?'))),

    _identifier_core: _ => /[a-zA-Z_][a-zA-Z0-9_]*/,

    integer: _ => /\d+/,

    float: _ => /\d+\.\d+/,

    boolean: _ => choice('true', 'false'),

    nil: _ => 'nil',

    // #rgb, #rgba, #rrggbb, #rrggbbaa
    color: _ => /#([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{4}|[0-9a-fA-F]{3})/,

    // Double-quoted string with `{expr}` interpolation and backslash escapes.
    string: $ => seq(
      '"',
      repeat(choice(
        $.string_content,
        $.escape_sequence,
        $.interpolation,
      )),
      '"',
    ),

    string_content: _ => token.immediate(prec(1, /[^"\\{]+/)),

    escape_sequence: _ => token.immediate(/\\./),

    interpolation: $ => seq(
      alias(token.immediate('{'), '{'),
      $._expression,
      '}',
    ),

    // Triple-quoted raw string: verbatim, no interpolation or escapes.
    raw_string: _ => token(seq(
      '"""',
      repeat(choice(/[^"]/, /"[^"]/, /""[^"]/)),
      '"""',
    )),

    comment: _ => token(seq('//', /[^\n]*/)),
  },
});
