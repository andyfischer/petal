// External scanner for tree-sitter-petal: layout, `else if`, and colours.
//
// Petal blocks close by indentation and `end` is an optional closer
// (docs/tasks/pythonic-syntax.md). The grammar keeps newlines in `extras`, so
// this scanner supplies the layout as three zero-width tokens, each emitted
// only where the parser can take it:
//
//   INDENT      after a block opener whose body starts on a later line that
//               is indented deeper than the enclosing block (§3.1). The
//               column is pushed on the indent stack.
//   DEDENT      when the next code line starts left of the open block, when
//               a closing bracket ends it from inside a call or a hole
//               (§3.2), or at the end of the file. Pops the stack.
//   INLINE_END  the line break (or end of file) that ends a one-line body
//               that has no `end`. Withheld when the next line continues the
//               expression: a leading binary operator, or an `else` / `end`
//               that still belongs to this one-line form (B4, B5).
//
// "Where the parser can take it" is what makes brackets work without the
// scanner counting them. Inside `(…)`, `[…]`, `{…}` and JSX no layout token is
// valid, so a line break there is plain whitespace; a block opened inside the
// bracket makes them valid again, measured from the enclosing block's column.
// The parser generator never merges states across external tokens, so that
// validity is exact.
//
// Comment-only and blank lines never count for layout (B15). The first code
// column of the file is the left margin (the base indent, §3.5); the
// zero-width START token that opens every file is where it is recorded,
// because scanner state is only saved when a token is returned.
//
// Two ordinary tokens also live here because each needs a check a regex
// cannot make:
//
//   CHAIN_IF    the `if` of `else if`, only when it follows `else` on the
//               same line with blanks between (§3.6).
//   COLOR       `:` + 3, 4, 6 or 8 hex digits + a non-identifier character
//               (B16). The previous-token half of that rule comes from the
//               parser: COLOR is valid only where an expression may start,
//               and it gives way wherever a `:` separator is valid too.
//
// END and COLON are never returned. They are the grammar's own `end` and `:`
// tokens, listed as externals only so `valid_symbols` says whether the parser
// could take one here.

#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"

#include <string.h>

enum TokenType {
  INDENT,
  DEDENT,
  INLINE_END,
  CHAIN_IF,
  COLOR,
  START,
  END,
  COLON,
  ERROR_SENTINEL,
};

typedef struct {
  Array(uint16_t) indents;
  // Column of the first code token in the file; the indent of the top level.
  uint16_t base;
} Scanner;

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }

static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

static inline bool is_ident_start(int32_t c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_';
}

static inline bool is_ident_char(int32_t c) {
  return is_ident_start(c) || (c >= '0' && c <= '9');
}

static inline bool is_hex(int32_t c) {
  return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
}

static uint16_t current_indent(const Scanner *scanner) {
  if (scanner->indents.size > 0) {
    return *array_back(&scanner->indents);
  }
  return scanner->base;
}

// What the first code token after the current position looks like, as far as
// layout cares.
typedef enum {
  NEXT_OTHER,
  // An operator that cannot begin a statement, so a line starting with it
  // continues the previous one (core/src/parse.rs `continues_line`, plus the
  // postfix `.` / `?.`).
  NEXT_CONTINUATION,
  // `else` or `elsif`.
  NEXT_ELSE,
  // `then` or `do`: the tail of a header broken across lines.
  NEXT_HEADER_TAIL,
  // `)`, `]` or `}`.
  NEXT_CLOSE_BRACKET,
  // `end`.
  NEXT_END,
} NextKind;

bool tree_sitter_petal_external_scanner_scan(void *payload, TSLexer *lexer,
                                             const bool *valid_symbols) {
  Scanner *scanner = (Scanner *)payload;

  // During error recovery every symbol is valid. Layout guesses made there do
  // more harm than good, so leave the input to the internal lexer.
  if (valid_symbols[ERROR_SENTINEL]) {
    return false;
  }

  // Layout tokens are zero-width and sit where the previous token ended.
  lexer->mark_end(lexer);

  // ---- Find the next code character ----
  bool newline = false;
  // A comment was passed over. It is still in the input for the internal
  // lexer as long as the token returned here is zero-width, so after one
  // only zero-width tokens may be returned.
  bool comment = false;
  // The next code begins with `/` followed by something that is not a
  // comment; the `/` has been consumed to find that out.
  bool slash = false;
  uint16_t column = 0;

  for (;;) {
    int32_t c = lexer->lookahead;
    if (c == '\n') {
      newline = true;
      skip(lexer);
    } else if (c == ' ' || c == '\t' || c == '\r' || c == 0x0c) {
      skip(lexer);
    } else if (c == '#') {
      comment = true;
      while (lexer->lookahead != '\n' && !lexer->eof(lexer)) {
        skip(lexer);
      }
    } else if (c == '/') {
      column = (uint16_t)lexer->get_column(lexer);
      skip(lexer);
      if (lexer->lookahead == '/') {
        comment = true;
        while (lexer->lookahead != '\n' && !lexer->eof(lexer)) {
          skip(lexer);
        }
      } else if (lexer->lookahead == '*') {
        comment = true;
        skip(lexer);
        for (;;) {
          if (lexer->eof(lexer)) {
            break;
          }
          if (lexer->lookahead == '*') {
            skip(lexer);
            if (lexer->lookahead == '/') {
              skip(lexer);
              break;
            }
          } else {
            if (lexer->lookahead == '\n') {
              newline = true;
            }
            skip(lexer);
          }
        }
      } else {
        slash = true;
        break;
      }
    } else {
      column = (uint16_t)lexer->get_column(lexer);
      break;
    }
  }

  bool eof = !slash && lexer->eof(lexer);

  if (valid_symbols[START]) {
    scanner->base = eof ? 0 : column;
    lexer->result_symbol = START;
    return true;
  }

  // ---- Tokens with content: `else if` and colours ----
  // Both need the next code on this side of any comment.

  NextKind next = NEXT_OTHER;
  int32_t c = lexer->lookahead;

  if (slash) {
    // `/` or `/=`: a division at the start of a line continues it.
    next = c == '=' ? NEXT_OTHER : NEXT_CONTINUATION;
  } else if (eof) {
    next = NEXT_OTHER;
  } else if (c == ':') {
    if (valid_symbols[COLOR] && !valid_symbols[COLON] && !comment) {
      // A layout token due at this line break goes first; the colour is
      // lexed on the next call.
      bool layout_first =
        newline &&
        ((valid_symbols[INDENT] && column > current_indent(scanner)) ||
         valid_symbols[INLINE_END] ||
         (valid_symbols[DEDENT] && scanner->indents.size > 0 &&
          column < current_indent(scanner)));
      if (!layout_first) {
        advance(lexer);
        unsigned digits = 0;
        while (is_hex(lexer->lookahead)) {
          advance(lexer);
          digits++;
        }
        if ((digits == 3 || digits == 4 || digits == 6 || digits == 8) &&
            !is_ident_char(lexer->lookahead)) {
          lexer->mark_end(lexer);
          lexer->result_symbol = COLOR;
          return true;
        }
        return false;
      }
    }
  } else if (is_ident_start(c)) {
    char word[8];
    unsigned len = 0;
    if (comment) {
      while (is_ident_char(lexer->lookahead)) {
        if (len < sizeof(word) - 1) {
          word[len] = (char)lexer->lookahead;
        }
        len++;
        skip(lexer);
      }
    } else {
      while (is_ident_char(lexer->lookahead)) {
        if (len < sizeof(word) - 1) {
          word[len] = (char)lexer->lookahead;
        }
        len++;
        advance(lexer);
      }
    }
    bool question = lexer->lookahead == '?';
    if (len < sizeof(word) && !question) {
      word[len] = 0;
      if (strcmp(word, "if") == 0) {
        if (valid_symbols[CHAIN_IF] && !newline && !comment) {
          lexer->mark_end(lexer);
          lexer->result_symbol = CHAIN_IF;
          return true;
        }
      } else if (strcmp(word, "else") == 0 || strcmp(word, "elsif") == 0) {
        next = NEXT_ELSE;
      } else if (strcmp(word, "then") == 0 || strcmp(word, "do") == 0) {
        next = NEXT_HEADER_TAIL;
      } else if (strcmp(word, "end") == 0) {
        next = NEXT_END;
      }
    }
  } else {
    switch (c) {
      case ')':
      case ']':
      case '}':
        next = NEXT_CLOSE_BRACKET;
        break;
      case '*':
      case '%':
        advance(lexer);
        next = lexer->lookahead == '=' ? NEXT_OTHER : NEXT_CONTINUATION;
        break;
      case '+':
        // `+`, `++`; not `+=` / `++=`.
        advance(lexer);
        if (lexer->lookahead == '+') {
          advance(lexer);
        }
        next = lexer->lookahead == '=' ? NEXT_OTHER : NEXT_CONTINUATION;
        break;
      case '=':
      case '!':
        // `==`, `!=`. A lone `!` is a unary operator and begins a statement.
        advance(lexer);
        next = lexer->lookahead == '=' ? NEXT_CONTINUATION : NEXT_OTHER;
        break;
      case '<':
        // `<=` only: `<` may open a JSX tag.
        advance(lexer);
        next = lexer->lookahead == '=' ? NEXT_CONTINUATION : NEXT_OTHER;
        break;
      case '>':
      case '|':
        // `>`, `>=`, `|`, `||`, `|>`.
        next = NEXT_CONTINUATION;
        break;
      case '&':
        advance(lexer);
        next = lexer->lookahead == '&' ? NEXT_CONTINUATION : NEXT_OTHER;
        break;
      case '?':
        // `??` and `?.`; not `??=`.
        advance(lexer);
        if (lexer->lookahead == '.') {
          next = NEXT_CONTINUATION;
        } else if (lexer->lookahead == '?') {
          advance(lexer);
          next = lexer->lookahead == '=' ? NEXT_OTHER : NEXT_CONTINUATION;
        }
        break;
      case '.':
        // `.field`; not the `...` of a spread or the `./` of a path.
        advance(lexer);
        next = is_ident_start(lexer->lookahead) ? NEXT_CONTINUATION : NEXT_OTHER;
        break;
      default:
        break;
    }
  }

  // ---- Layout ----
  // Everything below is zero-width: `mark_end` was called before any input
  // was consumed, so what was read above is only lookahead.

  uint16_t indent = current_indent(scanner);

  if (eof) {
    if (valid_symbols[INLINE_END]) {
      lexer->result_symbol = INLINE_END;
      return true;
    }
    if (valid_symbols[DEDENT] && scanner->indents.size > 0) {
      (void)array_pop(&scanner->indents);
      lexer->result_symbol = DEDENT;
      return true;
    }
    return false;
  }

  if (newline) {
    // A deeper line after an opener starts its block.
    if (valid_symbols[INDENT] && column > indent && next != NEXT_CONTINUATION &&
        next != NEXT_HEADER_TAIL && next != NEXT_CLOSE_BRACKET) {
      array_push(&scanner->indents, column);
      lexer->result_symbol = INDENT;
      return true;
    }

    if (valid_symbols[INLINE_END]) {
      bool continues;
      switch (next) {
        case NEXT_CONTINUATION:
        case NEXT_HEADER_TAIL:
          continues = true;
          break;
        case NEXT_ELSE:
        case NEXT_END:
          // B4: an `else` line continues the open one-line `if` unless it
          // sits left of the block that `if` is in, where it belongs to an
          // enclosing `if` instead. The same goes for the `end` line that
          // closes such a chain (B5).
          continues = scanner->indents.size == 0 || column >= indent;
          break;
        default:
          continues = false;
          break;
      }
      if (!continues) {
        lexer->result_symbol = INLINE_END;
        return true;
      }
      return false;
    }

    if (valid_symbols[DEDENT] && scanner->indents.size > 0 && column < indent) {
      (void)array_pop(&scanner->indents);
      lexer->result_symbol = DEDENT;
      return true;
    }
  }

  // A closing bracket ends a block opened inside the bracket, wherever the
  // bracket sits: `call(fn()⏎  body)`. So does an `end` on the block's last
  // line when no one-line form there can take it: `when _ -> x end`.
  bool closes_block = next == NEXT_CLOSE_BRACKET ? !valid_symbols[INLINE_END]
                      : next == NEXT_END         ? !valid_symbols[END]
                                                 : false;
  if (closes_block && valid_symbols[DEDENT] && scanner->indents.size > 0) {
    (void)array_pop(&scanner->indents);
    lexer->result_symbol = DEDENT;
    return true;
  }

  return false;
}

unsigned tree_sitter_petal_external_scanner_serialize(void *payload, char *buffer) {
  Scanner *scanner = (Scanner *)payload;
  unsigned size = 0;
  buffer[size++] = (char)(scanner->base & 0xff);
  buffer[size++] = (char)(scanner->base >> 8);
  for (uint32_t i = 0;
       i < scanner->indents.size && size + 2 <= TREE_SITTER_SERIALIZATION_BUFFER_SIZE; i++) {
    uint16_t indent = *array_get(&scanner->indents, i);
    buffer[size++] = (char)(indent & 0xff);
    buffer[size++] = (char)(indent >> 8);
  }
  return size;
}

void tree_sitter_petal_external_scanner_deserialize(void *payload, const char *buffer,
                                                    unsigned length) {
  Scanner *scanner = (Scanner *)payload;
  array_clear(&scanner->indents);
  scanner->base = 0;
  if (length < 2) {
    return;
  }
  scanner->base = (uint16_t)((uint8_t)buffer[0] | ((uint8_t)buffer[1] << 8));
  for (unsigned i = 2; i + 1 < length; i += 2) {
    uint16_t indent = (uint16_t)((uint8_t)buffer[i] | ((uint8_t)buffer[i + 1] << 8));
    array_push(&scanner->indents, indent);
  }
}

void *tree_sitter_petal_external_scanner_create(void) {
  Scanner *scanner = ts_calloc(1, sizeof(Scanner));
  array_init(&scanner->indents);
  return scanner;
}

void tree_sitter_petal_external_scanner_destroy(void *payload) {
  Scanner *scanner = (Scanner *)payload;
  array_delete(&scanner->indents);
  ts_free(scanner);
}
