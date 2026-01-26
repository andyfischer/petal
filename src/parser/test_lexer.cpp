#include "../third_party/doctest.h"
#include <iostream>
#include "lexer.h"
#include "parse_token_iterator.h"
#include "lexer_char_iterator.h"

TEST_CASE("lexer basic test") {
    const char* source = "let x = 1;";

    TokenIterator it(source);
    CHECK(it.next_text() == "let");
    CHECK(it.next()->tok_match == Token::Let);
    it.consume();
    CHECK(it.next()->tok_match == Token::Whitespace);
    CHECK(it.next_text() == " ");
    it.consume();
}

TEST_CASE("lexer symbol parsing") {
    const char* source = "send_effect(:log 1)";

    TokenIterator it(source);
    CHECK(it.next_text() == "send_effect");
    CHECK(it.next()->tok_match == Token::Identifier);
    it.consume();
    CHECK(it.next()->tok_match == Token::LParen);
    it.consume();
    CHECK(it.next()->tok_match == Token::Symbol);
    CHECK(it.next_text() == ":log");
    it.consume();
}

// ============================================================================
// COMPREHENSIVE LEXER TESTS
// ============================================================================

TEST_CASE("lexer - integer literals") {
    SUBCASE("positive integer") {
        TokenIterator it("42");
        CHECK(it.next()->tok_match == Token::Integer);
        CHECK(it.next_text() == "42");
    }
    
    SUBCASE("negative integer") {
        TokenIterator it("-123");
        CHECK(it.next()->tok_match == Token::Minus);
        it.consume();
        CHECK(it.next()->tok_match == Token::Integer);
        CHECK(it.next_text() == "123");
    }
    
    SUBCASE("zero") {
        TokenIterator it("0");
        CHECK(it.next()->tok_match == Token::Integer);
        CHECK(it.next_text() == "0");
    }
}

TEST_CASE("lexer - float literals") {
    SUBCASE("basic float") {
        TokenIterator it("3.14159");
        CHECK(it.next()->tok_match == Token::Float);
        CHECK(it.next_text() == "3.14159");
    }
    
    SUBCASE("float starting with dot") {
        TokenIterator it(".5");
        CHECK(it.next()->tok_match == Token::Float);
        CHECK(it.next_text() == ".5");
    }
    
    SUBCASE("negative float") {
        TokenIterator it("-2.5");
        CHECK(it.next()->tok_match == Token::Minus);
        it.consume();
        CHECK(it.next()->tok_match == Token::Float);
        CHECK(it.next_text() == "2.5");
    }
}

TEST_CASE("lexer - scientific notation") {
    SUBCASE("positive exponent") {
        TokenIterator it("1.23e4");
        CHECK(it.next()->tok_match == Token::Float);
        CHECK(it.next_text() == "1.23e4");
    }
    
    SUBCASE("negative exponent") {
        TokenIterator it("1.23e-4");
        CHECK(it.next()->tok_match == Token::Float);
        CHECK(it.next_text() == "1.23e-4");
    }
    
    SUBCASE("explicit positive exponent") {
        TokenIterator it("2.5E+6");
        CHECK(it.next()->tok_match == Token::Float);
        CHECK(it.next_text() == "2.5E+6");
    }
    
    SUBCASE("integer with scientific notation") {
        TokenIterator it("1e4");
        CHECK(it.next()->tok_match == Token::Float);
        CHECK(it.next_text() == "1e4");
    }
}

TEST_CASE("lexer - hexadecimal literals") {
    SUBCASE("basic hex") {
        TokenIterator it("0xFF");
        CHECK(it.next()->tok_match == Token::HexInteger);
        CHECK(it.next_text() == "0xFF");
    }
    
    SUBCASE("lowercase hex") {
        TokenIterator it("0xabc123");
        CHECK(it.next()->tok_match == Token::HexInteger);
        CHECK(it.next_text() == "0xabc123");
    }
    
    SUBCASE("mixed case hex") {
        TokenIterator it("0xDeAdBeEf");
        CHECK(it.next()->tok_match == Token::HexInteger);
        CHECK(it.next_text() == "0xDeAdBeEf");
    }
}

TEST_CASE("lexer - binary literals") {
    SUBCASE("basic binary") {
        TokenIterator it("0b1010");
        CHECK(it.next()->tok_match == Token::BinaryInteger);
        CHECK(it.next_text() == "0b1010");
    }
    
    SUBCASE("all zeros") {
        TokenIterator it("0b0000");
        CHECK(it.next()->tok_match == Token::BinaryInteger);
        CHECK(it.next_text() == "0b0000");
    }
    
    SUBCASE("all ones") {
        TokenIterator it("0b1111");
        CHECK(it.next()->tok_match == Token::BinaryInteger);
        CHECK(it.next_text() == "0b1111");
    }
}

TEST_CASE("lexer - string literals") {
    SUBCASE("double quotes") {
        TokenIterator it("\"hello world\"");
        CHECK(it.next()->tok_match == Token::StringLiteral);
        CHECK(it.next_text() == "\"hello world\"");
    }
    
    SUBCASE("single quotes") {
        TokenIterator it("'hello world'");
        CHECK(it.next()->tok_match == Token::StringLiteral);
        CHECK(it.next_text() == "'hello world'");
    }
    
    SUBCASE("empty string") {
        TokenIterator it("\"\"");
        CHECK(it.next()->tok_match == Token::StringLiteral);
        CHECK(it.next_text() == "\"\"");
    }
    
    SUBCASE("string with escape sequences") {
        TokenIterator it("\"hello\\nworld\"");
        CHECK(it.next()->tok_match == Token::StringLiteral);
        CHECK(it.next_text() == "\"hello\\nworld\"");
    }
}

TEST_CASE("lexer - boolean and null literals") {
    SUBCASE("true keyword") {
        TokenIterator it("true");
        CHECK(it.next()->tok_match == Token::True);
        CHECK(it.next_text() == "true");
    }
    
    SUBCASE("false keyword") {
        TokenIterator it("false");
        CHECK(it.next()->tok_match == Token::False);
        CHECK(it.next_text() == "false");
    }
    
    SUBCASE("null keyword") {
        TokenIterator it("null");
        CHECK(it.next()->tok_match == Token::Null);
        CHECK(it.next_text() == "null");
    }
}

TEST_CASE("lexer - symbols and colors") {
    SUBCASE("basic symbol") {
        TokenIterator it(":symbol");
        CHECK(it.next()->tok_match == Token::Symbol);
        CHECK(it.next_text() == ":symbol");
    }
    
    SUBCASE("symbol with underscores") {
        TokenIterator it(":my_symbol_123");
        CHECK(it.next()->tok_match == Token::Symbol);
        CHECK(it.next_text() == ":my_symbol_123");
    }
    
    SUBCASE("color hex 6 digits") {
        TokenIterator it("#FF0000");
        CHECK(it.next()->tok_match == Token::Color);
        CHECK(it.next_text() == "#FF0000");
    }
    
    SUBCASE("color hex 3 digits") {
        TokenIterator it("#F0A");
        CHECK(it.next()->tok_match == Token::Color);
        CHECK(it.next_text() == "#F0A");
    }
}

TEST_CASE("lexer - operators and punctuation") {
    SUBCASE("arithmetic operators") {
        TokenIterator it("+ - * / %");
        CHECK(it.next()->tok_match == Token::Plus);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::Minus);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::Star);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::Slash);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::Percent);
    }
    
    SUBCASE("assignment operators") {
        TokenIterator it("= += -= *= ==");
        CHECK(it.next()->tok_match == Token::Equals);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::PlusEquals);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::MinusEquals);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::StarEquals);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::DoubleEquals);
    }
    
    SUBCASE("brackets and braces") {
        TokenIterator it("() {} []");
        CHECK(it.next()->tok_match == Token::LParen);
        it.consume();
        CHECK(it.next()->tok_match == Token::RParen);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::LBrace);
        it.consume();
        CHECK(it.next()->tok_match == Token::RBrace);
        it.consume();
        CHECK(it.next()->tok_match == Token::Whitespace);
        it.consume();
        CHECK(it.next()->tok_match == Token::LSquare);
        it.consume();
        CHECK(it.next()->tok_match == Token::RSquare);
    }
}

TEST_CASE("lexer - keywords") {
    SUBCASE("all keywords") {
        TokenIterator it("fn let return if else while for");
        CHECK(it.next()->tok_match == Token::Fn);
        CHECK(it.next_text() == "fn");
        it.consume();
        it.skip_whitespace();
        
        CHECK(it.next()->tok_match == Token::Let);
        CHECK(it.next_text() == "let");
        it.consume();
        it.skip_whitespace();
        
        CHECK(it.next()->tok_match == Token::Return);
        CHECK(it.next_text() == "return");
        it.consume();
        it.skip_whitespace();
        
        CHECK(it.next()->tok_match == Token::If);
        CHECK(it.next_text() == "if");
        it.consume();
        it.skip_whitespace();
        
        CHECK(it.next()->tok_match == Token::Else);
        CHECK(it.next_text() == "else");
        it.consume();
        it.skip_whitespace();
        
        CHECK(it.next()->tok_match == Token::While);
        CHECK(it.next_text() == "while");
        it.consume();
        it.skip_whitespace();
        
        CHECK(it.next()->tok_match == Token::For);
        CHECK(it.next_text() == "for");
    }
}