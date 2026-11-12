; Indentation for Petal, in the nvim-treesitter capture convention
; (`@indent.begin` / `@indent.branch` / `@indent.end` / `@indent.auto`).
;
; Petal blocks are delimited by indentation and `end` is an optional closer
; (see the README, "Layout"), so the editor's job is to indent the line after
; a block opener and to pull `else` / `else if` / `end` and closing brackets
; back to the opener's column.

; A line inside one of these is indented one step from the line it starts on.
[
  (function_declaration)
  (lambda)
  (test_declaration)
  (class_declaration)
  (enum_declaration)
  (if_expression)
  (for_statement)
  (while_statement)
  (match_expression)
  (match_arm)
  (list)
  (record)
  (argument_list)
  (parameter_list)
  (native_parameter_list)
  (record_pattern)
  (list_pattern)
  (parenthesized_expression)
  (jsx_expression)
] @indent.begin

; The children of an element, but not its closing tag.
(jsx_element
  (jsx_opening_element)) @indent.begin
(jsx_closing_element) @indent.branch

; `else` and `else if` / `elsif` sit at the column of their `if`; their bodies
; are indented again because they are still inside the `if_expression`.
(elsif_clause) @indent.branch
(else_clause) @indent.branch

; An explicit `end` and a closing bracket return to the opener's column.
"end" @indent.branch @indent.end
[ ")" "]" "}" ] @indent.branch @indent.end

; Comments and the insides of multi-line strings keep the indent they have.
[
  (comment)
  (string)
  (raw_string)
] @indent.auto
