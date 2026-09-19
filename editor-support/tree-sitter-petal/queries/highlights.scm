; Syntax highlighting for Petal.
;
; Capture names follow the tree-sitter highlight convention so editors using
; the standard theme map light up out of the box.
;
; ORDERING: when several patterns capture the *same node*, the LAST one wins.
; That is the `tree-sitter-highlight` rule (see its `HighlightIter::next`:
; "keep iterating over any later highlighting patterns that also match this
; node and set the match to it"), and Garden, Helix and the `tree-sitter
; highlight` CLI all run on that crate. So the generic `(identifier)
; @variable` fallback goes FIRST and every more-specific pattern after it —
; the reverse of what reads naturally. With the fallback last, it overrode
; every specific pattern and *every* identifier in a Petal file rendered as a
; plain variable: function names, type names, properties and parameters all
; lost their color.
;
; When patterns capture *different* nodes that nest (a parent and its child),
; the innermost wins regardless of order. So a rule that wants to color an
; identifier must capture the `(identifier)` leaf itself, not its parent —
; which is why `type_name` is written `(type_name (identifier) @type)`.

; ---- Identifiers (fallback; see ORDERING above) ----

(identifier) @variable

; ---- Comments ----

(comment) @comment

; ---- Keywords ----

[
  "import"
  "export"
  "as"
  "config"
  "let"
  "var"
  "set"
  "get"
  "state"
  "fn"
  "enum"
  "class"
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

; ---- Modules ----
; Every segment of an import path is a module name, not an ordinary variable:
; `import bloom/menu` names one module, spelled in two segments.

(module_path segment: (identifier) @module)

; ---- Functions ----

(function_declaration name: (identifier) @function)
(call function: (identifier) @function)
(call function: (field_access field: (identifier) @function.method))

; ---- Types & constructors (enums) ----

(enum_declaration name: (identifier) @type)
(class_declaration name: (identifier) @type)
(class_field name: (identifier) @property)
(function_declaration class: (identifier) @type)
(enum_variant name: (identifier) @constructor)
(variant_pattern name: (identifier) @constructor)

; ---- Parameters & record keys ----

(parameter name: (identifier) @variable.parameter)
(type_name (identifier) @type)
(named_argument name: (identifier) @variable.parameter)
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
  "??" "?." "?.["
  "=" "+=" "-=" "*=" "/=" "%=" "++=" "??="
  "|>" "->"
] @operator

; ---- Punctuation ----

[ "(" ")" "[" "]" "{" "}" ] @punctuation.bracket
[ "," "." ":" "..." ] @punctuation.delimiter
