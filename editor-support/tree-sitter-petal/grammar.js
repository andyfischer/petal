/**
 * @file Petal grammar for tree-sitter
 * @author Petal
 * @license MIT
 *
 * A reference tree-sitter grammar for the Petal language. It models the
 * surface syntax tokenized by `rust/src/lexer.rs` and parsed by
 * `rust/src/parse.rs`, primarily so editors can syntax-highlight `.ptl` files.
 *
 * Newlines between statements are treated as insignificant (part of
 * `extras`): the real parser uses them as statement separators, but statement
 * boundaries are recoverable from grammar structure in practice.
 *
 * Layout is the exception. Blocks close by indentation and `end` is an
 * optional closer (docs/tasks/pythonic-syntax.md), so the external scanner
 * (src/scanner.c) supplies three zero-width layout tokens:
 *
 *   _indent      after a block opener, when the body starts on a later line
 *                indented deeper than the enclosing block
 *   _dedent      when a line starts to the left of the open indented block
 *                (or a closing bracket / the end of the file ends it)
 *   _inline_end  the line break that ends a one-line body with no `end`
 *
 * Every block body therefore comes in two shapes: an indented block
 * (`_indent … _dedent`, then an optional `end`), or a one-line body (closed by
 * an `end` on the line, by the line break, or by whatever token follows that
 * cannot continue it, such as the `)` of an enclosing call). The scanner also
 * lexes the `if` of a same-line `else if`, and `:f80` colour literals. See the
 * README.
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
// The export modifier on a declaration or an `import`: `pub`, or its
// deprecated spelling `export`, which the real parser still accepts
// everywhere `pub` goes (rust/src/parse.rs `parse_export`).
const PUB = choice('pub', 'export');

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

/**
 * What follows a block opener (`do`, a `fn` header, …) for a form with one
 * body: the body and its closer.
 *
 * - An indented block on the following lines, then an optional `end`.
 * - A one-line body, closed by `end`, by the line break, or by nothing at all
 *   when the next token cannot continue it (`map(xs, fn(v) v * 2)`).
 * - No body: just `end`.
 * @param {GrammarSymbols<string>} $
 * @param {string} name
 */
function closedBody($, name) {
  return choice(
    seq(field(name, alias($._indented_block, $.block)), optional('end')),
    seq(field(name, $.block), optional(choice('end', $._inline_end))),
    'end',
  );
}

/**
 * The comma-separated members of a `class` or `enum`, with the same three
 * shapes as `closedBody`. A declaration with no members and no `end` closes at
 * the line break.
 * @param {GrammarSymbols<string>} $
 * @param {RuleOrLiteral} rule
 */
function closedList($, rule) {
  const members = seq(rule, repeat(seq(',', rule)), optional(','));
  return choice(
    seq($._indent, members, $._dedent, optional('end')),
    seq(optional(members), choice('end', $._inline_end)),
  );
}

module.exports = grammar({
  name: 'petal',

  // Keyword extraction needs a single token; `identifier` is a two-token
  // sequence (see its definition), so the core regex is the word token.
  word: $ => $._identifier_core,

  externals: $ => [
    $._indent,
    $._dedent,
    $._inline_end,
    // The `if` of `else if`, when it follows the `else` on the same line with
    // only blanks between. On a later line the `if` is an ordinary nested
    // `if` inside the `else` block.
    $._chain_if,
    $.color,
    // Zero-width, at the very start of the file: where the scanner records
    // the base indent (the first statement's column is the left margin).
    $._start,
    // Never emitted by the scanner; the internal lexer lexes `end` as the
    // keyword it is. Listing it here tells the scanner whether an `end` can
    // be taken where the parser stands, which is how it knows that an `end`
    // on the last line of an indented block closes that block
    // (`when _ -> x end`) and not a one-line form on the line.
    'end',
    // Likewise never emitted. Where a `:` separator can be taken, `:fff` is
    // that separator and a name (`import ui:add`), not a colour: the half of
    // B16 that looks at the previous token.
    ':',
    // Never used in a rule: valid only during error recovery, which is how
    // the scanner recognises it.
    $._error_sentinel,
  ],

  extras: $ => [
    // The lexer turns `;` into a newline-style separator; with newlines
    // insignificant, both are simply ignorable here.
    /[ \t\r\n;]/,
    $.comment,
  ],

  rules: {
    // A test block is a statement of the file's top level only.
    source_file: $ => seq($._start, repeat(choice($._statement, $.test_declaration))),

    // ---- Statements ----

    _statement: $ => choice(
      $.import_statement,
      $.let_declaration,
      $.state_declaration,
      $.memo_declaration,
      $.function_declaration,
      $.native_function_declaration,
      $.enum_declaration,
      $.class_declaration,
      $.return_statement,
      $.break_statement,
      $.continue_statement,
      $.assignment,
      $.set_statement,
      $._expression,
    ),

    // The module-import forms (see docs/module-system.md):
    //   import ui                    (every export bare, plus the qualifier `ui`)
    //   import ui as u               (the same, the qualifier renamed)
    //   import ui: button, clicked   (selective, comma-separated names)
    //   import bloom/menu            (a package path: `/`-joined segments)
    //   import ./level               (a file beside the importing one)
    //   import ../shared/rng         (a file above it)
    //   pub import ./menu            (a re-export)
    //   import "./my-lib" as lib     (a quoted path, for a non-identifier segment)
    // `as` is contextual in the real parser; here it is a keyword literal,
    // which is safe because a syntax-highlighting grammar never needs `as`
    // as an ordinary identifier. The selective list requires commas between
    // names, matching the parser and stopping the list at the next statement.
    import_statement: $ => prec.right(seq(
      optional(PUB),
      'import',
      field('module', $.module_path),
      optional(choice(
        seq('as', field('alias', $.identifier)),
        seq(
          ':',
          field('name', $.identifier),
          repeat(seq(',', field('name', $.identifier))),
        ),
      )),
    )),

    // What an import names: an optional relative prefix — `./`, or one or
    // more `../` — then one or more identifier segments joined by `/`
    // (core/src/parse.rs `parse_import`). Without a prefix the first segment
    // is a package name. A flat name is the one-segment case, so this node
    // covers `import ui` too. A path whose segments are not all identifiers
    // (`./my-lib`) is written as a quoted string instead, and is the whole
    // path, prefix included.
    module_path: $ => choice(
      field('quoted', $.string),
      prec.right(seq(
        optional(field('relative', $.relative_prefix)),
        field('segment', $.identifier),
        repeat(seq('/', field('segment', $.identifier))),
      )),
    ),

    relative_prefix: _ => token(choice('./', /(\.\.\/)+/)),

    // `export` is a prefix on a declaration, not a wrapper node: the real
    // parser (rust/src/parse.rs `parse_export`) dispatches straight into
    // `parse_fn_decl` / `parse_let` / `parse_state` / `parse_enum_decl` with an
    // `exported` flag, producing the same node with one extra leading token.
    // Mirroring that keeps the tree shapes identical. Those forms — fn, let,
    // var, const, `config let`, state, enum, class and import — are exactly what
    // `export` may precede.

    // `@tunable(0, 10)`, `@const`, `@rigid`: a declaration annotation
    // (docs/proposals/annotation-syntax.md). Written before a `let`, `state`
    // or `memo` (and before its `pub`), on the declaration's line or on lines
    // of its own; the real parser lowers `@f(a) let X = v` to
    // `let X = f(v, a)` and accepts only `const`, `tunable` and `rigid`. The
    // name follows the `@` with no space.
    annotation: $ => seq(
      '@',
      field('name', alias(token.immediate(/[a-zA-Z_][a-zA-Z0-9_]*/), $.identifier)),
      optional(field('arguments', $.argument_list)),
    ),

    // `let x = v`, its mutable twin `var x = v` and its must-be-static twin
    // `const X = v`, each with an optional `: type` annotation.
    let_declaration: $ => seq(
      repeat(field('annotation', $.annotation)),
      optional(PUB),
      // `config let x = …` marks an edit knob for propose-edit (docs/CLI.md).
      // `config` is a contextual identifier in the real lexer, recognized
      // only directly before `let`, and an everyday name everywhere else
      // (`config.speed`). So, like `test`, it cannot be a keyword literal:
      // `config`, blanks and `let` are one token, which is longer than the
      // identifier `config` and wins only where a declaration can start. It
      // is aliased to the anonymous node `config` for the highlight query.
      choice(alias($._config_let, 'config'), 'let', 'var', 'const'),
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
      '=',
      field('value', $._expression),
    ),

    // `state name = init`, with the same optional `: type` annotation
    // `let`/`var` take. (The keyed `state(expr) name = init` was removed on
    // 2026-11-13; `memo(key)` and `for … key` are the keyed forms.)
    state_declaration: $ => seq(
      repeat(field('annotation', $.annotation)),
      optional(PUB),
      'state',
      optional('var'),
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
      '=',
      field('value', $._expression),
    ),

    // `memo(key) name = init`: a content-keyed cache. The key is required and
    // there is no `var` form.
    memo_declaration: $ => seq(
      repeat(field('annotation', $.annotation)),
      'memo',
      '(', field('key', $._expression), ')',
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
      '=',
      field('value', $._expression),
    ),

    // `fn name(...)` or the method form `fn Class.name(...)`, whose receiver
    // class is the name before the dot (rust/src/parse.rs `parse_fn_decl`).
    // Higher precedence than `lambda`: at statement start `fn name(…)` is
    // always a declaration, never an argless lambda calling `name`.
    function_declaration: $ => prec.right(1, seq(
      optional(PUB),
      'fn',
      optional(seq(field('class', $.identifier), '.')),
      field('name', $.identifier),
      field('parameters', $.parameter_list),
      optional(field('return_type', $.return_type)),
      closedBody($, 'body'),
    )),

    // `native fn name(params) -> type = "host_name"` — the declaration of a
    // function the host implements (rust/src/parse.rs `parse_native_fn_decl`,
    // docs/usage/ffi.md). No body and no `end`. `native` is a *contextual* keyword
    // in the real lexer (a keyword only directly before `fn`); as with `as`
    // and `class`, a highlighting grammar can treat it as a literal.
    native_function_declaration: $ => prec.right(2, seq(
      optional(PUB),
      'native',
      'fn',
      optional(seq(field('class', $.identifier), '.')),
      field('name', $.identifier),
      field('parameters', $.native_parameter_list),
      optional(field('return_type', $.return_type)),
      optional(seq('=', field('host', $.string))),
    )),

    native_parameter_list: $ => seq(
      '(',
      commaSep(choice($.native_parameter, $.rest_parameter)),
      ')',
    ),

    // `name`, `name: type`, and the optional forms `name?` / `name?: type`
    // (the `?` is part of the identifier, as everywhere). The real parser
    // takes any keyword as a parameter name, since the name is only ever an
    // argument label; this rule models the one the builtins use, `end`.
    native_parameter: $ => seq(
      field('name', choice($.identifier, $.keyword_name)),
      optional(field('type', $.type_annotation)),
    ),

    keyword_name: _ => seq('end', optional(token.immediate('?'))),

    // `...name` — any number of further arguments (`native fn print(...values)`).
    rest_parameter: $ => seq('...', field('name', $.identifier)),

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

    // Higher precedence than an expression: after `fn`, a leading `(` is always
    // the parameter list, never a parenthesized body (the documented wart —
    // `fn (a + b) * 2 end` reads `(a + b)` as parameters).
    //
    // `= expr` gives the parameter a default value: an expression evaluated in
    // the callee on every call that omits the argument (rust/src/parse.rs
    // `parse_param_default`). The annotation, when there is one, comes first.
    parameter: $ => prec(1, seq(
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
      optional(seq('=', field('default', $._expression))),
    )),

    // `class Name` … `end`, comma-separated `field: type` declarations — the
    // same comma rule as `enum` (docs/syntax/commas.md). `class` is a
    // *contextual* keyword in the real lexer (it stays usable as an identifier
    // and as the JSX `class=` attribute); as with `as`, a highlighting grammar
    // can treat it as a literal.
    class_declaration: $ => prec.right(seq(
      optional(PUB),
      'class',
      field('name', $.identifier),
      closedList($, $.class_field),
    )),

    class_field: $ => seq(
      field('name', $.identifier),
      optional(field('type', $.type_annotation)),
    ),

    enum_declaration: $ => prec.right(seq(
      optional(PUB),
      'enum',
      field('name', $.identifier),
      closedList($, $.enum_variant),
    )),

    // A variant: `red`, `custom(r, g, b)`, `park(shape: "area")`. Inside the
    // parentheses a bare name is a payload field and `key: expr` is constant
    // data; the two may be mixed (rust/src/parse.rs `parse_enum_decl`).
    enum_variant: $ => seq(
      field('name', $.identifier),
      optional(seq(
        '(',
        commaSep(choice($.enum_data, field('field', $.identifier))),
        ')',
      )),
    ),

    enum_data: $ => seq(
      field('key', $.identifier),
      ':',
      field('value', $._expression),
    ),

    // `test "name" do … end` — a block `petal test` finds and runs, and an
    // ordinary run never does (core/src/parse.rs `parse_test_decl`,
    // docs/testing.md). `test` is a *contextual* keyword: a keyword only at the
    // start of a top-level statement, directly before a string. Unlike `as` and
    // `class` it cannot be a plain keyword literal here, because a file that
    // calls its own `test()` at the top level is ordinary Petal. So the opening
    // token is `test`, blanks and the name's opening quote together: where a
    // statement may start it is longer than the identifier `test` and wins,
    // and it matches nothing else. It is aliased to the anonymous node `test`
    // so the highlight query can name it; the rest of the name is aliased to
    // an ordinary `string`. The name takes no `{…}` holes.
    test_declaration: $ => prec.right(seq(
      alias($._test_open, 'test'),
      field('name', alias($._test_name, $.string)),
      'do',
      closedBody($, 'body'),
    )),

    _test_open: _ => token(seq('test', /[ \t]+/, '"')),

    _config_let: _ => token(seq('config', /[ \t]+/, 'let')),

    _test_name: $ => seq(
      repeat(choice($.string_content, $.escape_sequence)),
      '"',
    ),

    for_statement: $ => prec.right(seq(
      'for',
      field('variable', $.identifier),
      'in',
      field('iterator', $._expression),
      // `for t in tasks key t.id do`: key each iteration's state by item.
      optional(seq('key', field('key', $._expression))),
      // `for a in xs, b in ys do`: more generators, nested loops in one
      // header, each with its own optional `key`.
      repeat(seq(
        ',',
        field('variable', $.identifier),
        'in',
        field('iterator', $._expression),
        optional(seq('key', field('key', $._expression))),
      )),
      'do',
      closedBody($, 'body'),
    )),

    while_statement: $ => prec.right(seq(
      'while',
      field('condition', $._expression),
      'do',
      closedBody($, 'body'),
    )),

    return_statement: $ => prec.right(seq('return', optional($._expression))),

    break_statement: _ => 'break',

    continue_statement: _ => 'continue',

    // The target is a name, a field or an index, not any expression: with
    // `end` optional, `while c do x += 1` must not read as
    // `(while c do x) += 1`.
    assignment: $ => prec.right(PREC.assign, seq(
      field('left', $._assignable),
      field('operator', choice('=', '+=', '-=', '*=', '/=', '%=', '++=', '??=')),
      field('right', $._expression),
    )),

    _assignable: $ => choice($.identifier, $.field_access, $.index_access),

    // `set x = v` — a write through a `var` cell. Same shape as `assignment`,
    // under its own node because it is a different statement.
    set_statement: $ => prec.right(PREC.assign, seq(
      'set',
      field('left', $._assignable),
      field('operator', choice('=', '+=', '-=', '*=', '/=', '%=', '++=', '??=')),
      field('right', $._expression),
    )),

    // A one-line body: a run of statements on the opener's line. It stops at
    // the `end`/`else`/`when` that closes the enclosing form, at the line
    // break (`_inline_end`), or at a token that cannot continue it. Given the
    // choice it keeps going (`if c then a; b end` is one body of two
    // statements), hence the right associativity.
    block: $ => prec.right(repeat1($._statement)),

    // A body on the lines after its opener, delimited by indentation. It is
    // aliased to `block` wherever it is used, so both shapes are one node.
    _indented_block: $ => seq($._indent, repeat1($._statement), $._dedent),

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
      // `for` and `while` are expressions too: in value position they
      // collect their body's values into a list
      // (`let squares = for i in range(1, 6) do i * i`).
      $.for_statement,
      $.while_statement,
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

    // `f(a, ...rest)` — a spread argument (a list: by position; a record: by
    // parameter name). The real parser admits it only in last place and never
    // after a named argument; the grammar does not encode either rule.
    argument_list: $ => seq('(', commaSep(choice($.named_argument, $.spread_element, $._expression)), ')'),

    // `f(x, limit: 10)` — a named argument binds to the parameter of that
    // name. The real parser accepts any keyword as the name (`f(end: 1)`),
    // just as it does for a record key; this rule takes the same (lossy)
    // shortcut as `record_field` and models only an identifier.
    named_argument: $ => seq(
      field('name', $.identifier),
      ':',
      field('value', $._expression),
    ),

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

    record: $ => seq('{', commaSep(choice($.record_field, $.computed_field, $.spread_element)), '}'),

    // `{[key]: value}` — a field named by a string expression.
    computed_field: $ => seq(
      '[',
      field('key', $._expression),
      ']',
      ':',
      field('value', $._expression),
    ),

    record_field: $ => seq(
      field('key', $.identifier),
      ':',
      field('value', $._expression),
    ),

    spread_element: $ => seq('...', $._expression),

    // `fn(params) -> expr` or `fn(params) body end`. The parameter list is
    // optional: `fn 42 end` and `fn -> 42` are argless lambdas.
    lambda: $ => prec.right(seq(
      'fn',
      optional(field('parameters', $.parameter_list)),
      choice(
        seq('->', field('body', $._expression)),
        closedBody($, 'body'),
      ),
    )),

    // `if c then … [else if c then …]* [else …] [end]`. Each body is an
    // indented block or a one-line body, independently of the others, and the
    // `end` is optional: the chain closes after its last body. `_if_tail` is
    // what follows a `then`; it is right-recursive through
    // `_if_continuation` so a dangling `else` goes to the nearest `if` (B2)
    // and an `end` closes the innermost open form.
    if_expression: $ => prec.right(seq(
      'if',
      field('condition', $._expression),
      'then',
      $._if_tail,
    )),

    _if_tail: $ => prec.right(choice(
      seq(
        field('consequence', $.block),
        optional(choice($._if_continuation, 'end', $._inline_end)),
      ),
      seq(
        field('consequence', alias($._indented_block, $.block)),
        optional(choice($._if_continuation, 'end')),
      ),
      // An empty body needs something after it.
      $._if_continuation,
      'end',
    )),

    _if_continuation: $ => prec.right(choice(
      seq(
        alias($._elsif_inline, $.elsif_clause),
        optional(choice($._if_continuation, 'end', $._inline_end)),
      ),
      seq(
        alias($._elsif_indented, $.elsif_clause),
        optional(choice($._if_continuation, 'end')),
      ),
      seq(alias($._elsif_empty, $.elsif_clause), choice($._if_continuation, 'end')),
      seq(alias($._else_inline, $.else_clause), optional(choice('end', $._inline_end))),
      seq(alias($._else_indented, $.else_clause), optional('end')),
      seq(alias($._else_empty, $.else_clause), 'end'),
    )),

    // `else if c then …`, or its older spelling `elsif c then …`, in the
    // three body shapes. All three are aliased to `elsif_clause`.
    //
    // `else if` chains only when the two words share a line; the scanner
    // lexes that `if` as `_chain_if` (docs/tasks/pythonic-syntax.md §3.6).
    _elsif_head: $ => seq(
      choice('elsif', seq('else', alias($._chain_if, 'if'))),
      field('condition', $._expression),
      'then',
    ),

    _elsif_inline: $ => seq($._elsif_head, field('consequence', $.block)),
    _elsif_indented: $ => seq(
      $._elsif_head,
      field('consequence', alias($._indented_block, $.block)),
    ),
    _elsif_empty: $ => $._elsif_head,

    _else_inline: $ => seq('else', field('body', $.block)),
    _else_indented: $ => seq('else', field('body', alias($._indented_block, $.block))),
    _else_empty: _ => 'else',

    // `match x` then its arms: indented on the following lines, or on the
    // `match` line itself (`match x when 1 -> "a" when 2 -> "b" end`).
    match_expression: $ => prec.right(seq(
      'match',
      field('subject', $._expression),
      choice(
        seq($._indent, repeat1($.match_arm), $._dedent, optional('end')),
        seq(repeat1($.match_arm), optional(choice('end', $._inline_end))),
        'end',
      ),
    )),

    // After `->` an arm's body is a run of statements: an indented block on
    // the following lines, or a one-line body up to the line break, the next
    // `when` of this match, or its `end`. The last expression is the arm's
    // value. `do … end` is the older block form.
    match_arm: $ => prec.right(seq(
      'when',
      field('pattern', $._pattern),
      optional($.guard),
      choice(
        seq('->', choice(
          seq(field('body', $.block), optional($._inline_end)),
          field('body', alias($._indented_block, $.block)),
        )),
        seq('do', closedBody($, 'body')),
      ),
    )),

    guard: $ => seq('if', field('condition', $._expression)),

    // ---- Patterns ----

    _pattern: $ => choice(
      $.or_pattern,
      $.variant_pattern,
      $.list_pattern,
      $.record_pattern,
      $.literal_pattern,
      $.identifier, // variable binding or `_` wildcard: a bare name always binds
    ),

    // `Kind.park | Kind.plaza`, `"a" | "b"` — at the top of an arm or nested.
    or_pattern: $ => prec.left(seq($._pattern, '|', $._pattern)),

    // A variant is matched through its enum: `Kind.park`, `Op.fight(place, won)`,
    // and through a module, `town.Kind.park`. The last name is the variant;
    // the one before it the enum.
    variant_pattern: $ => seq(
      optional(seq(field('module', $.identifier), '.')),
      field('enum', $.identifier),
      '.',
      field('name', $.identifier),
      optional(seq('(', commaSep($._pattern), ')')),
    ),

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
      $.color,
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

    // A bare `<video muted />` is `muted={true}`, as in JSX.
    jsx_attribute: $ => seq(
      field('name', $.identifier),
      optional(seq('=', field('value', choice($.string, $.jsx_expression)))),
    ),

    jsx_expression: $ => seq('{', optional($._expression), '}'),

    // One line of text at a time: a line in element children whose first
    // non-blank characters are `//` or `/*` is a comment, and a line break
    // between two runs of text is plain whitespace. Text that starts a line
    // loses to a comment at the same spot (it has the lower precedence).
    jsx_text: _ => token(prec(-1, /[^<>{}\n]+/)),

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

    // `1.5`, and with an exponent `1.5e-3`, `1e9`.
    float: _ => /\d+(\.\d+([eE][+-]?\d+)?|[eE][+-]?\d+)/,

    boolean: _ => choice('true', 'false'),

    nil: _ => 'nil',

    // `:rgb`, `:rgba`, `:rrggbb`, `:rrggbbaa` are lexed by the external
    // scanner (`color` in `externals`), which can check what a regex cannot:
    // that the hex digits are followed by a non-identifier character.

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

    // `#` to the end of the line; its deprecated spelling `//`; or a
    // `/* … */` block (which does not nest).
    //
    // The `#` form is its own token with a lower precedence than `jsx_text`,
    // so a children line that starts with `#` stays text ("# of items",
    // docs/tasks/pythonic-syntax.md §3.4), while `//` and `/*` still win
    // there.
    comment: _ => choice(
      token(prec(-2, seq('#', /[^\n]*/))),
      token(choice(
        seq('//', /[^\n]*/),
        seq('/*', /[^*]*\*+([^/*][^*]*\*+)*/, '/'),
      )),
    ),
  },
});
