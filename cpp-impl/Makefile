CXX = clang++
CXXFLAGS = -std=c++17 -Wall -Wextra -g -O2
LDFLAGS = -lm

SRCS = main.cpp lexer.cpp parser.cpp ast.cpp value.cpp interpreter.cpp
OBJS = $(SRCS:.cpp=.o)
TARGET = petal

.PHONY: all clean test

all: $(TARGET)

$(TARGET): $(OBJS)
	$(CXX) $(CXXFLAGS) -o $@ $^ $(LDFLAGS)

%.o: %.cpp
	$(CXX) $(CXXFLAGS) -c -o $@ $<

clean:
	rm -f $(OBJS) $(TARGET)

# Dependencies
main.o: main.cpp parser.h interpreter.h
lexer.o: lexer.cpp lexer.h
parser.o: parser.cpp parser.h lexer.h ast.h
ast.o: ast.cpp ast.h
value.o: value.cpp value.h
interpreter.o: interpreter.cpp interpreter.h ast.h value.h

# Run tests
test: $(TARGET)
	@echo "Running sample programs..."
	@for f in ../samples/*.petal; do \
		echo "=== Running $$f ==="; \
		./$(TARGET) "$$f"; \
		echo ""; \
	done
