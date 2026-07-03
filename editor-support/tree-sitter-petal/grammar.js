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
 * from grammar structure in practice, and ignoring them keeps comma-less
 * juxtaposition (`[1 2 3]`, `color(0 1 2)`) simple. See the README.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// Precedence ladder, lowest → highest, mirroring the precedence-climbing
// parser in rust/src/parse.rs (pipe → or → and → equality → comparison →
// concat → additive → multiplicative → unary → postfix).
const PREC = {
  assign: -1,
  pipe: 1,
  or: 2,
  and: 3,
  equality: 4,
  comparison: 5,
  concat: 6,
  additive: 7,
  multiplicative: 8,
  unary: 9,
  postfix: 10,
  jsx: 11,
};

/**
 * A delimited list whose separating commas are optional: elements may be
 * separated by commas, newlines, or plain whitespace (juxtaposition).
 * See docs/syntax/optional-commas.md.
 * @param {RuleOrLiteral} rule
 */
function commaSep(rule) {
  return optional(seq(rule, repeat(seq(optional(','), rule)), optional(',')));
}

module.exports = grammar({
  name: 'petal',

  word: $ => $.identifier,

  extras: $ => [
    // The lexer turns `;` into a newline-style separator; with newlines
    // insignificant, both are simply ignorable here.
    /[ \t\r\n;]/,
    $.comment,
  ],

  conflicts: $ => [
    // In comma-less juxtaposition (`[a (b)]`, `f(a)(b)`), a `(` after an
    // expression could open a call's args or a new parenthesized element. The
    // real parser is greedy: it always binds as a call (see the dynamic
    // precedence on `call`).
    [$.argument_list, $.parenthesized_expression],
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
      $.for_statement,
      $.while_statement,
      $.return_statement,
      $.break_statement,
      $.continue_statement,
      $.assignment,
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

    let_declaration: $ => seq(
      'let',
      field('name', $.identifier),
      '=',
      field('value', $._expression),
    ),

    // `state name = init` or keyed `state(expr) name = init`.
    state_declaration: $ => seq(
      'state',
      optional(seq('(', field('key', $._expression), ')')),
      field('name', $.identifier),
      '=',
      field('value', $._expression),
    ),

    function_declaration: $ => seq(
      'fn',
      field('name', $.identifier),
      field('parameters', $.parameter_list),
      optional(field('body', $.block)),
      'end',
    ),

    parameter_list: $ => seq('(', commaSep(field('parameter', $.identifier)), ')'),

    enum_declaration: $ => seq(
      'enum',
      field('name', $.identifier),
      repeat($.enum_variant),
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

    call: $ => prec.dynamic(1, prec(PREC.postfix, seq(
      field('function', $._expression),
      field('arguments', $.argument_list),
    ))),

    argument_list: $ => seq('(', commaSep($._expression), ')'),

    field_access: $ => prec(PREC.postfix, seq(
      field('object', $._expression),
      '.',
      field('field', $.identifier),
    )),

    // The `[` is immediate (no whitespace before it) so that real indexing
    // (`arr[i]`) is distinguished from two juxtaposed bracket-lists on adjacent
    // lines (`[..]` `[..]`), which the comma-less list syntax allows.
    index_access: $ => prec(PREC.postfix, seq(
      field('object', $._expression),
      token.immediate('['),
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

    // ---- JSX (React-like elements, see apps/petal-web) ----

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

    // Identifiers may end in `?` (e.g. `even?`); the lexer also permits `?`
    // mid-identifier.
    identifier: _ => /[a-zA-Z_][a-zA-Z0-9_?]*/,

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
