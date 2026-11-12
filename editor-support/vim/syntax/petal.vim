" Vim syntax file
" Language:   Petal
" Filenames:  *.ptl
" Maintainer: Petal
"
" Classic-Vim syntax highlighting for Petal source. Mirrors the reference
" tree-sitter grammar in editor-support/tree-sitter-petal/grammar.js. Editors
" with tree-sitter (Neovim, Helix, Zed, Emacs) should prefer that grammar; this
" file is for stock Vim (and Neovim without a tree-sitter parser installed).

if exists("b:current_syntax")
  finish
endif

" Petal identifiers may contain and end with `?` (e.g. `even?`).
setlocal iskeyword+=?

" ---- Keywords ----
syn keyword petalKeyword     import pub export as config native let const var set get state memo fn enum class return for in while do match when
syn keyword petalKeyword     break continue
" `test` opens a test block only before its name: `test "adds" do`. Anywhere
" else it is an ordinary identifier, so it is matched in that position only.
syn match   petalKeyword     "^\s*\zstest\ze\s\+\""
" `key` is a keyword only in a `for` header, after the iterable:
" `for t in tasks key t.id do`. Anywhere else it is an ordinary identifier.
" A lookbehind, because a match starting at `for` would lose to the keyword.
syn match   petalKeyword     "\(\<for\>.\{-}\<in\>.\{-}\s\)\@<=\zskey\ze\s"
" `else if` is the two keywords `else` and `if`; `elsif` is its older
" spelling. `end` is an optional closer: blocks close by indentation.
syn keyword petalConditional if then elsif else end
syn keyword petalBoolean     true false
syn keyword petalConstant    nil

" ---- Function declarations ----
syn match petalFunction "\<fn\>\s\+\zs[a-zA-Z_][a-zA-Z0-9_?]*" contains=NONE
" `enum Name` and `class Name` name a type; `fn Rect.method(...)` names the
" class it extends.
syn match petalType     "\<enum\>\s\+\zs[a-zA-Z_][a-zA-Z0-9_?]*" contains=NONE
syn match petalType     "\<class\>\s\+\zs[a-zA-Z_][a-zA-Z0-9_?]*" contains=NONE
syn match petalType     "\<fn\>\s\+\zs[a-zA-Z_][a-zA-Z0-9_?]*\ze\." contains=NONE
syn match petalFunction "\<fn\>\s\+[a-zA-Z_][a-zA-Z0-9_?]*\.\zs[a-zA-Z_][a-zA-Z0-9_?]*" contains=NONE

" ---- Type annotations ----
" `: type` on a let/var/state binding or a parameter, `-> type` on a named fn.
" Only the known type vocabulary is highlighted, and only in type position, so
" record keys (`{a: 1}`) and cast calls (`fn(x) -> int(x)`) are left alone.
" A named call argument (`f(kind: list)`) reads like an annotation here and can
" still light up its value if that value happens to be a type name — the same
" harmless overlap a record key has, and not separable with a `syn match`.
" `nil` and `enum` stay Constant/Keyword — syn keyword outranks syn match.
syn match petalType "\%(:\|->\)\s*\zs\%(any\|nil\|bool\|int\|float\|num\|string\|str\|list\|record\|map\|function\|enum\|vec2\|vec3\|f64_array\|element\|symbol\|handle\|pending\)\>\ze\s*(\@!"

" ---- Numbers ----
syn match petalFloat  "\<\d\+\.\d\+\>"
syn match petalNumber "\<\d\+\>"

" ---- Colors: :rgb :rgba :rrggbb :rrggbbaa ----
" A `:` directly after a name, a literal or a closing bracket is a separator
" (`{x:100}`, `f(a:fff)`, `let c:add = 1`), so the colon of a colour must not
" follow one. The real rule looks at the previous token whatever the spacing;
" this looks at the previous character, which agrees wherever the separator
" colon is written against its name.
syn match petalColor "\%([a-zA-Z0-9_?)\]}\"]\)\@1<!:\%(\x\{8}\|\x\{6}\|\x\{4}\|\x\{3}\)\>"

" ---- Strings ----
" Double-quoted string with `{expr}` interpolation and backslash escapes.
syn match  petalEscape       contained "\\."
syn region petalInterp       contained matchgroup=petalInterpDelim start="{" end="}" contains=@petalExpr
syn region petalString       start=+"+ skip=+\\"+ end=+"+ contains=petalEscape,petalInterp,@Spell
" Triple-quoted raw string: verbatim, no interpolation or escapes.
syn region petalRawString    start=+"""+ end=+"""+ keepend contains=@Spell

" ---- Operators ----
" A lone `|` separates the alternatives of an or-pattern (`when "a" | "b"`).
syn match petalOperator "|>\|->\|++=\|??=\|++\|+=\|-=\|\*=\|/=\|%=\|==\|!=\|<=\|>=\|&&\|||\|??\|?\.\|\.\.\|[-+*/%<>=!|]"
" `...` is defined after `..` so it wins where both match.
syn match petalPipe     "\.\.\."

" ---- Comments ----
" Defined after the operators: when two matches start at the same place Vim
" takes the one defined last, and `//` and `/*` both start with an operator
" character.
syn keyword petalTodo contained TODO FIXME XXX NOTE
" `#` to the end of the line. `//` is the deprecated spelling, and `/* … */`
" is a block comment (it does not nest). There is no `#f80` colour any more:
" colours are written `:f80`, so a `#` is always a comment.
syn match   petalComment "#.*$" contains=petalTodo,@Spell
syn match   petalComment "//.*$" contains=petalTodo,@Spell
syn region  petalComment start="/\*" end="\*/" contains=petalTodo,@Spell

" `@tunable(0, 10) let x = 4`, `@const`, `@rigid`: a declaration annotation
" (docs/proposals/annotation-syntax.md). The `@` and its name; the arguments
" highlight as any call's. The match starts at the `@`, so it takes in
" `@const` whole before the `const` keyword is tried.
syn match petalAnnotation "@[a-zA-Z_][a-zA-Z0-9_]*"

" Expression cluster used inside string interpolation.
syn cluster petalExpr contains=petalKeyword,petalConditional,petalBoolean,petalConstant,petalFloat,petalNumber,petalColor,petalString,petalRawString,petalOperator,petalComment

" ---- Highlight links ----
hi def link petalComment     Comment
hi def link petalTodo        Todo
hi def link petalKeyword     Keyword
hi def link petalConditional Conditional
hi def link petalBoolean     Boolean
hi def link petalConstant    Constant
hi def link petalFunction    Function
hi def link petalType        Type
hi def link petalFloat       Float
hi def link petalNumber      Number
hi def link petalColor       Constant
hi def link petalString      String
hi def link petalRawString   String
hi def link petalEscape      SpecialChar
hi def link petalInterp      Normal
hi def link petalInterpDelim Special
hi def link petalOperator    Operator
hi def link petalPipe        Operator
hi def link petalAnnotation  PreProc

let b:current_syntax = "petal"
