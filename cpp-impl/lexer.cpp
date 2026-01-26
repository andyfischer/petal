#include "lexer.h"
#include <cstdlib>
#include <cstring>
#include <cctype>
#include <cstdio>

void lexer_init(Lexer* lexer, const char* source) {
    lexer->source = source;
    lexer->start = source;
    lexer->current = source;
    lexer->line = 1;
    lexer->column = 1;
}

static bool is_at_end(Lexer* lexer) {
    return *lexer->current == '\0';
}

static char advance(Lexer* lexer) {
    char c = *lexer->current++;
    if (c == '\n') {
        lexer->line++;
        lexer->column = 1;
    }
    else {
        lexer->column++;
    }
    return c;
}

static char peek(Lexer* lexer) {
    return *lexer->current;
}

static char peek_next(Lexer* lexer) {
    if (is_at_end(lexer)) return '\0';
    return lexer->current[1];
}

static bool match(Lexer* lexer, char expected) {
    if (is_at_end(lexer)) return false;
    if (*lexer->current != expected) return false;
    advance(lexer);
    return true;
}

static Token make_token(Lexer* lexer, TokenType type) {
    Token token;
    token.type = type;
    token.start = lexer->start;
    token.length = (int)(lexer->current - lexer->start);
    token.line = lexer->line;
    token.column = lexer->column - token.length;
    token.int_value = 0;
    token.float_value = 0.0;
    return token;
}

static Token error_token(Lexer* lexer, const char* message) {
    Token token;
    token.type = TOK_ERROR;
    token.start = message;
    token.length = (int)strlen(message);
    token.line = lexer->line;
    token.column = lexer->column;
    return token;
}

static void skip_whitespace(Lexer* lexer) {
    for (;;) {
        char c = peek(lexer);
        switch (c) {
            case ' ':
            case '\r':
            case '\t':
                advance(lexer);
                break;
            case '\n':
                advance(lexer);
                break;
            case '/':
                if (peek_next(lexer) == '/') {
                    // Single-line comment
                    while (peek(lexer) != '\n' && !is_at_end(lexer)) {
                        advance(lexer);
                    }
                }
                else if (peek_next(lexer) == '*') {
                    // Multi-line comment
                    advance(lexer); // /
                    advance(lexer); // *
                    while (!is_at_end(lexer)) {
                        if (peek(lexer) == '*' && peek_next(lexer) == '/') {
                            advance(lexer);
                            advance(lexer);
                            break;
                        }
                        advance(lexer);
                    }
                }
                else {
                    return;
                }
                break;
            default:
                return;
        }
    }
}

static TokenType check_keyword(const char* start, int length, const char* rest, TokenType type) {
    int rest_len = (int)strlen(rest);
    if (length == rest_len + 1 && memcmp(start + 1, rest, rest_len) == 0) {
        return type;
    }
    return TOK_IDENTIFIER;
}

static TokenType identifier_type(Lexer* lexer) {
    int length = (int)(lexer->current - lexer->start);
    const char* start = lexer->start;

    switch (start[0]) {
        case 'b':
            if (length > 1 && start[1] == 'r') return check_keyword(start, length, "reak", TOK_BREAK);
            break;
        case 'c':
            return check_keyword(start, length, "ontinue", TOK_CONTINUE);
        case 'e':
            if (length > 1) {
                if (start[1] == 'l') return check_keyword(start, length, "lse", TOK_ELSE);
                if (start[1] == 'n') return check_keyword(start, length, "num", TOK_ENUM);
            }
            break;
        case 'f':
            if (length > 1) {
                if (start[1] == 'n') {
                    if (length == 2) return TOK_FN;
                }
                if (start[1] == 'o') return check_keyword(start, length, "or", TOK_FOR);
                if (start[1] == 'a') return check_keyword(start, length, "alse", TOK_FALSE);
            }
            break;
        case 'i':
            if (length == 2 && start[1] == 'f') return TOK_IF;
            if (length == 2 && start[1] == 'n') return TOK_IN;
            break;
        case 'l':
            if (length > 1) {
                if (start[1] == 'e') return check_keyword(start, length, "et", TOK_LET);
                if (start[1] == 'o') return check_keyword(start, length, "oop", TOK_LOOP);
            }
            break;
        case 'm':
            return check_keyword(start, length, "atch", TOK_MATCH);
        case 'n':
            return check_keyword(start, length, "ull", TOK_NULL);
        case 'r':
            return check_keyword(start, length, "eturn", TOK_RETURN);
        case 's':
            if (length > 1) {
                if (start[1] == 't') {
                    if (length > 2 && start[2] == 'r') return check_keyword(start, length, "truct", TOK_STRUCT);
                    if (length > 2 && start[2] == 'a') return check_keyword(start, length, "tate", TOK_STATE);
                }
            }
            break;
        case 't':
            return check_keyword(start, length, "rue", TOK_TRUE);
        case 'w':
            return check_keyword(start, length, "hile", TOK_WHILE);
    }

    return TOK_IDENTIFIER;
}

static Token identifier(Lexer* lexer) {
    while (isalnum(peek(lexer)) || peek(lexer) == '_') {
        advance(lexer);
    }
    return make_token(lexer, identifier_type(lexer));
}

static Token number(Lexer* lexer) {
    bool is_float = false;

    // Check for hex/binary
    if (lexer->start[0] == '0' && (peek(lexer) == 'x' || peek(lexer) == 'X')) {
        advance(lexer); // x
        while (isxdigit(peek(lexer))) {
            advance(lexer);
        }
        Token token = make_token(lexer, TOK_INT);
        token.int_value = strtoll(lexer->start, nullptr, 16);
        return token;
    }

    if (lexer->start[0] == '0' && (peek(lexer) == 'b' || peek(lexer) == 'B')) {
        advance(lexer); // b
        while (peek(lexer) == '0' || peek(lexer) == '1') {
            advance(lexer);
        }
        Token token = make_token(lexer, TOK_INT);
        token.int_value = strtoll(lexer->start + 2, nullptr, 2);
        return token;
    }

    while (isdigit(peek(lexer))) {
        advance(lexer);
    }

    // Check for decimal point
    if (peek(lexer) == '.' && isdigit(peek_next(lexer))) {
        is_float = true;
        advance(lexer); // .
        while (isdigit(peek(lexer))) {
            advance(lexer);
        }
    }

    // Check for exponent
    if (peek(lexer) == 'e' || peek(lexer) == 'E') {
        is_float = true;
        advance(lexer); // e/E
        if (peek(lexer) == '+' || peek(lexer) == '-') {
            advance(lexer);
        }
        while (isdigit(peek(lexer))) {
            advance(lexer);
        }
    }

    if (is_float) {
        Token token = make_token(lexer, TOK_FLOAT);
        token.float_value = strtod(lexer->start, nullptr);
        return token;
    }
    else {
        Token token = make_token(lexer, TOK_INT);
        token.int_value = strtoll(lexer->start, nullptr, 10);
        return token;
    }
}

static Token string(Lexer* lexer, char quote) {
    // Check for triple-quoted string
    if (peek(lexer) == quote && peek_next(lexer) == quote) {
        advance(lexer);
        advance(lexer);
        // Triple-quoted string
        while (!is_at_end(lexer)) {
            if (peek(lexer) == quote && peek_next(lexer) == quote) {
                const char* check = lexer->current + 2;
                if (*check == quote) {
                    advance(lexer);
                    advance(lexer);
                    advance(lexer);
                    return make_token(lexer, TOK_STRING);
                }
            }
            advance(lexer);
        }
        return error_token(lexer, "Unterminated triple-quoted string");
    }

    while (peek(lexer) != quote && !is_at_end(lexer)) {
        if (peek(lexer) == '\\') {
            advance(lexer); // backslash
            if (!is_at_end(lexer)) {
                advance(lexer); // escaped char
            }
        }
        else if (peek(lexer) == '\n') {
            return error_token(lexer, "Unterminated string");
        }
        else {
            advance(lexer);
        }
    }

    if (is_at_end(lexer)) {
        return error_token(lexer, "Unterminated string");
    }

    advance(lexer); // closing quote
    return make_token(lexer, TOK_STRING);
}

static Token symbol(Lexer* lexer) {
    // Symbol starts after :
    // Can be alphanumeric identifier or hex color
    if (isxdigit(peek(lexer))) {
        // Could be hex color or symbol
        int hex_count = 0;
        const char* check = lexer->current;
        while (isxdigit(*check)) {
            hex_count++;
            check++;
        }
        // Hex colors are 6 or 8 digits (RGB or RGBA)
        if ((hex_count == 6 || hex_count == 8) && !isalnum(*check) && *check != '_') {
            // It's a color
            while (isxdigit(peek(lexer))) {
                advance(lexer);
            }
            return make_token(lexer, TOK_SYMBOL);
        }
    }

    // Regular symbol
    while (isalnum(peek(lexer)) || peek(lexer) == '_') {
        advance(lexer);
    }
    return make_token(lexer, TOK_SYMBOL);
}

Token lexer_next_token(Lexer* lexer) {
    skip_whitespace(lexer);

    lexer->start = lexer->current;

    if (is_at_end(lexer)) {
        return make_token(lexer, TOK_EOF);
    }

    char c = advance(lexer);

    if (isalpha(c) || c == '_') {
        return identifier(lexer);
    }

    if (isdigit(c)) {
        return number(lexer);
    }

    switch (c) {
        case '(': return make_token(lexer, TOK_LPAREN);
        case ')': return make_token(lexer, TOK_RPAREN);
        case '{': return make_token(lexer, TOK_LBRACE);
        case '}': return make_token(lexer, TOK_RBRACE);
        case '[': return make_token(lexer, TOK_LBRACKET);
        case ']': return make_token(lexer, TOK_RBRACKET);
        case ',': return make_token(lexer, TOK_COMMA);
        case ';': return make_token(lexer, TOK_SEMICOLON);
        case '.': return make_token(lexer, TOK_DOT);
        case '@': return make_token(lexer, TOK_AT);
        case '?': return make_token(lexer, TOK_QUESTION);

        case '+':
            if (match(lexer, '=')) return make_token(lexer, TOK_PLUS_EQ);
            return make_token(lexer, TOK_PLUS);

        case '-':
            if (match(lexer, '>')) return make_token(lexer, TOK_ARROW);
            if (match(lexer, '=')) return make_token(lexer, TOK_MINUS_EQ);
            return make_token(lexer, TOK_MINUS);

        case '*':
            if (match(lexer, '*')) {
                if (match(lexer, '=')) return make_token(lexer, TOK_STAR_EQ); // **= not in spec but consistent
                return make_token(lexer, TOK_STAR_STAR);
            }
            if (match(lexer, '=')) return make_token(lexer, TOK_STAR_EQ);
            return make_token(lexer, TOK_STAR);

        case '/':
            if (match(lexer, '=')) return make_token(lexer, TOK_SLASH_EQ);
            return make_token(lexer, TOK_SLASH);

        case '%':
            if (match(lexer, '=')) return make_token(lexer, TOK_PERCENT_EQ);
            return make_token(lexer, TOK_PERCENT);

        case '=':
            if (match(lexer, '=')) return make_token(lexer, TOK_EQ);
            if (match(lexer, '>')) return make_token(lexer, TOK_FAT_ARROW);
            return make_token(lexer, TOK_ASSIGN);

        case '!':
            if (match(lexer, '=')) return make_token(lexer, TOK_NE);
            return make_token(lexer, TOK_NOT);

        case '<':
            if (match(lexer, '=')) return make_token(lexer, TOK_LE);
            return make_token(lexer, TOK_LT);

        case '>':
            if (match(lexer, '=')) return make_token(lexer, TOK_GE);
            return make_token(lexer, TOK_GT);

        case '&':
            if (match(lexer, '&')) return make_token(lexer, TOK_AND);
            return error_token(lexer, "Expected '&&'");

        case '|':
            if (match(lexer, '|')) return make_token(lexer, TOK_OR);
            return error_token(lexer, "Expected '||'");

        case ':':
            if (match(lexer, ':')) return make_token(lexer, TOK_COLON_COLON);
            // Check if it's a symbol
            if (isalnum(peek(lexer)) || peek(lexer) == '_') {
                return symbol(lexer);
            }
            return make_token(lexer, TOK_COLON);

        case '"':
            return string(lexer, '"');

        case '\'':
            return string(lexer, '\'');
    }

    return error_token(lexer, "Unexpected character");
}

Token lexer_peek_token(Lexer* lexer) {
    // Save state
    const char* start = lexer->start;
    const char* current = lexer->current;
    int line = lexer->line;
    int column = lexer->column;

    Token token = lexer_next_token(lexer);

    // Restore state
    lexer->start = start;
    lexer->current = current;
    lexer->line = line;
    lexer->column = column;

    return token;
}

const char* token_type_name(TokenType type) {
    switch (type) {
        case TOK_INT: return "INT";
        case TOK_FLOAT: return "FLOAT";
        case TOK_STRING: return "STRING";
        case TOK_SYMBOL: return "SYMBOL";
        case TOK_IDENTIFIER: return "IDENTIFIER";
        case TOK_FN: return "FN";
        case TOK_LET: return "LET";
        case TOK_RETURN: return "RETURN";
        case TOK_IF: return "IF";
        case TOK_ELSE: return "ELSE";
        case TOK_WHILE: return "WHILE";
        case TOK_FOR: return "FOR";
        case TOK_IN: return "IN";
        case TOK_TRUE: return "TRUE";
        case TOK_FALSE: return "FALSE";
        case TOK_NULL: return "NULL";
        case TOK_STRUCT: return "STRUCT";
        case TOK_ENUM: return "ENUM";
        case TOK_STATE: return "STATE";
        case TOK_MATCH: return "MATCH";
        case TOK_LOOP: return "LOOP";
        case TOK_BREAK: return "BREAK";
        case TOK_CONTINUE: return "CONTINUE";
        case TOK_PLUS: return "PLUS";
        case TOK_MINUS: return "MINUS";
        case TOK_STAR: return "STAR";
        case TOK_SLASH: return "SLASH";
        case TOK_PERCENT: return "PERCENT";
        case TOK_STAR_STAR: return "STAR_STAR";
        case TOK_EQ: return "EQ";
        case TOK_NE: return "NE";
        case TOK_LT: return "LT";
        case TOK_GT: return "GT";
        case TOK_LE: return "LE";
        case TOK_GE: return "GE";
        case TOK_AND: return "AND";
        case TOK_OR: return "OR";
        case TOK_NOT: return "NOT";
        case TOK_ASSIGN: return "ASSIGN";
        case TOK_PLUS_EQ: return "PLUS_EQ";
        case TOK_MINUS_EQ: return "MINUS_EQ";
        case TOK_STAR_EQ: return "STAR_EQ";
        case TOK_SLASH_EQ: return "SLASH_EQ";
        case TOK_PERCENT_EQ: return "PERCENT_EQ";
        case TOK_AT: return "AT";
        case TOK_DOT: return "DOT";
        case TOK_COLON: return "COLON";
        case TOK_COLON_COLON: return "COLON_COLON";
        case TOK_ARROW: return "ARROW";
        case TOK_FAT_ARROW: return "FAT_ARROW";
        case TOK_QUESTION: return "QUESTION";
        case TOK_LPAREN: return "LPAREN";
        case TOK_RPAREN: return "RPAREN";
        case TOK_LBRACE: return "LBRACE";
        case TOK_RBRACE: return "RBRACE";
        case TOK_LBRACKET: return "LBRACKET";
        case TOK_RBRACKET: return "RBRACKET";
        case TOK_COMMA: return "COMMA";
        case TOK_SEMICOLON: return "SEMICOLON";
        case TOK_NEWLINE: return "NEWLINE";
        case TOK_EOF: return "EOF";
        case TOK_ERROR: return "ERROR";
    }
    return "UNKNOWN";
}

char* token_to_string(Token* token) {
    char* buf = (char*)malloc(token->length + 1);
    memcpy(buf, token->start, token->length);
    buf[token->length] = '\0';
    return buf;
}
