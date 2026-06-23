; Syntax highlighting for Petal.
;
; Capture names follow the tree-sitter highlight convention so editors using
; the standard theme map (and Garden's `syntax.rs`) light up out of the box.
; More-specific patterns come first; the generic `(identifier) @variable`
; fallback is last.

; ---- Comments ----

(comment) @comment

; ---- Keywords ----

[
  "let"
  "state"
  "fn"
  "enum"
  "return"
  "if"
  "then"
  "elsif"
  "else"
  "end"
  "for"
  "in"
  "while"
  "do"
  "match"
  "when"
] @keyword

; `break` / `continue` are the sole content of their statement node, so they
; are captured via the node rather than as standalone anonymous tokens.
(break_statement) @keyword
(continue_statement) @keyword

; ---- Functions ----

(function_declaration name: (identifier) @function)
(call function: (identifier) @function)
(call function: (field_access field: (identifier) @function.method))

; ---- Types & constructors (enums) ----

(enum_declaration name: (identifier) @type)
(enum_variant name: (identifier) @constructor)
(variant_pattern name: (identifier) @constructor)

; ---- Parameters & record keys ----

(parameter_list (identifier) @variable.parameter)
(record_field key: (identifier) @property)
(record_pattern_field key: (identifier) @property)
(field_access field: (identifier) @property)

; ---- Literals ----

(integer) @number
(float) @number
(boolean) @constant.builtin
(nil) @constant.builtin
(color) @constant

[
  (string)
  (raw_string)
  (string_content)
] @string

(escape_sequence) @string.escape

; Interpolation delimiters, distinct from the string body.
(interpolation "{" @punctuation.special "}" @punctuation.special)

; ---- JSX ----

(jsx_open_tag) @tag
(jsx_close_tag) @tag
(jsx_self_closing_element "/>" @tag)
(jsx_opening_element ">" @tag)
(jsx_closing_element ">" @tag)
(jsx_attribute name: (identifier) @attribute)

; ---- Operators ----

[
  "+" "-" "*" "/" "%" "++"
  "==" "!=" "<" "<=" ">" ">="
  "&&" "||" "!"
  "=" "+=" "-=" "*=" "/=" "%="
  "|>" "->"
] @operator

; ---- Punctuation ----

[ "(" ")" "[" "]" "{" "}" ] @punctuation.bracket
[ "," "." ":" "..." ] @punctuation.delimiter

; ---- Identifiers (fallback) ----

(identifier) @variable
