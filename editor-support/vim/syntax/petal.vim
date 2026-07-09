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

" ---- Comments ----
syn keyword petalTodo contained TODO FIXME XXX NOTE
syn match   petalComment "//.*$" contains=petalTodo,@Spell

" ---- Keywords ----
syn keyword petalKeyword     import as let state fn enum return for in while do match when
syn keyword petalKeyword     break continue
syn keyword petalConditional if then elsif else end
syn keyword petalBoolean     true false
syn keyword petalConstant    nil

" ---- Function declarations ----
syn match petalFunction "\<fn\>\s\+\zs[a-zA-Z_][a-zA-Z0-9_?]*" contains=NONE
syn match petalFunction "\<enum\>\s\+\zs[a-zA-Z_][a-zA-Z0-9_?]*" contains=NONE

" ---- Numbers ----
syn match petalFloat  "\<\d\+\.\d\+\>"
syn match petalNumber "\<\d\+\>"

" ---- Colors: #rgb #rgba #rrggbb #rrggbbaa ----
syn match petalColor "#\%(\x\{8}\|\x\{6}\|\x\{4}\|\x\{3}\)\>"

" ---- Strings ----
" Double-quoted string with `{expr}` interpolation and backslash escapes.
syn match  petalEscape       contained "\\."
syn region petalInterp       contained matchgroup=petalInterpDelim start="{" end="}" contains=@petalExpr
syn region petalString       start=+"+ skip=+\\"+ end=+"+ contains=petalEscape,petalInterp,@Spell
" Triple-quoted raw string: verbatim, no interpolation or escapes.
syn region petalRawString    start=+"""+ end=+"""+ keepend contains=@Spell

" ---- Operators ----
syn match petalOperator "|>\|->\|++\|+=\|-=\|\*=\|/=\|%=\|==\|!=\|<=\|>=\|&&\|||\|[-+*/%<>=!]"
syn match petalPipe     "\.\.\."

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

let b:current_syntax = "petal"
