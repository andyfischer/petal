# Makefile for Petal C++ project

# Compiler settings
CXX = g++
CXXFLAGS = -std=c++17 -O2 -g -w
INCLUDES = -Isrc

# AddressSanitizer flags (disabled by default)
ASAN_FLAGS = -fsanitize=address -fno-omit-frame-pointer -O0
ASAN_LDFLAGS = -fsanitize=address
ASAN_ENV = ASAN_OPTIONS=replace_str=0:replace_intrin=0:detect_odr_violation=0

# Source files
SRCDIR = src
CLI_DIR = $(SRCDIR)/cli

# Unity build files
UNITY_FILE = src/unity.cpp

# Target executable
TARGET = dist/cli/main
ASAN_TARGET = dist/cli/main-asan

# Directories
DISTDIR = dist/cli

# Build tool
CPP_BUILD_TOOL = ../cpp-build-tool/bin/cpp-build-tool

# Shared patterns for source file discovery
# Exclude unity.cpp to avoid circular dependency
SOURCE_FILE_PATTERNS = --include "$(SRCDIR)/*.cpp" \
                      --include "$(SRCDIR)/*.h" \
                      --include "$(SRCDIR)/bytecode/*.cpp" \
                      --include "$(SRCDIR)/bytecode/*.h" \
                      --include "$(SRCDIR)/parser/*.cpp" \
                      --include "$(SRCDIR)/parser/*.h" \
                      --include "$(SRCDIR)/program/*.cpp" \
                      --include "$(SRCDIR)/program/*.h" \
                      --include "$(SRCDIR)/runtime/*.cpp" \
                      --include "$(SRCDIR)/runtime/*.h" \
                      --include "$(SRCDIR)/utils/*.cpp" \
                      --include "$(SRCDIR)/utils/*.h" \
                      --include "$(SRCDIR)/host/*.cpp" \
                      --include "$(SRCDIR)/host/*.h" \
                      --include "$(SRCDIR)/globals/*.cpp" \
                      --include "$(SRCDIR)/globals/*.h" \
                      --include "$(SRCDIR)/variant/*.cpp" \
                      --include "$(SRCDIR)/variant/*.h" \
                      --include "$(CLI_DIR)/*.cpp" \
                      --include "$(CLI_DIR)/*.h" \
                      --exclude "$(SRCDIR)/test_*.cpp" \
                      --exclude "$(SRCDIR)/runtime/test_*.cpp" \
                      --exclude "$(SRCDIR)/unity.cpp"

CPP_SOURCES = $(shell $(CPP_BUILD_TOOL) list-cpp-files $(SOURCE_FILE_PATTERNS))
HEADERS = $(shell $(CPP_BUILD_TOOL) list-h-files $(SOURCE_FILE_PATTERNS))
ALL_SOURCES = $(CPP_SOURCES) $(HEADERS)

# Default target
all: $(TARGET)

# Create directories if they don't exist
$(DISTDIR):
	mkdir -p $(DISTDIR)

# Generate unity file using cpp-build-tool
# Regenerate if any source file changes
$(UNITY_FILE): $(ALL_SOURCES)
	@$(CPP_BUILD_TOOL) write-unity-file $(SOURCE_FILE_PATTERNS) --out $(UNITY_FILE)

# Build the main executable using unity build
$(TARGET): $(UNITY_FILE) $(ALL_SOURCES) | $(DISTDIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -o $@ $(UNITY_FILE)

# Build with AddressSanitizer for memory debugging
$(ASAN_TARGET): $(UNITY_FILE) $(ALL_SOURCES) | $(DISTDIR)
	$(CXX) $(CXXFLAGS) $(ASAN_FLAGS) $(INCLUDES) -o $@ $(UNITY_FILE) $(ASAN_LDFLAGS)

# Clean build artifacts
clean:
	rm -f  $(TARGET) $(ASAN_TARGET)
	rm -rf  $(DISTDIR)

# Run tests
test: $(TARGET)
	$(TARGET) -test

# Run with AddressSanitizer
asan: $(ASAN_TARGET)
	ASAN_OPTIONS=replace_str=0:replace_intrin=0:detect_odr_violation=0:mmap_limit_mb=2048 $(ASAN_TARGET)

# Run tests with AddressSanitizer
test-asan: $(ASAN_TARGET)
	ASAN_OPTIONS=replace_str=0:replace_intrin=0:detect_odr_violation=0:mmap_limit_mb=2048 $(ASAN_TARGET) -test

# Run with leaks tool (macOS only)
leaks: $(TARGET)
	MallocStackLogging=1 leaks --atExit -- $(TARGET)

# Run tests with leaks tool (macOS only)
test-leaks: $(TARGET)
	MallocStackLogging=1 leaks --atExit -- $(TARGET) -test

# Install (copy to a common location, optional)
install: $(TARGET)
	cp $(TARGET) /usr/local/bin/petal

# Force rebuild
rebuild: clean all

# Show help
help:
	@echo "Available targets:"
	@echo "  all        - Build the main executable (default)"
	@echo "  clean      - Remove build artifacts"
	@echo "  test       - Build and run tests"
	@echo "  install    - Install the executable to /usr/local/bin"
	@echo "  rebuild    - Clean and rebuild"
	@echo "  help       - Show this help message"
	@echo ""
	@echo "Memory debugging targets:"
	@echo "  asan       - Build and run with AddressSanitizer"
	@echo "  test-asan  - Run tests with AddressSanitizer"
	@echo "  leaks      - Run with macOS leaks tool"
	@echo "  test-leaks - Run tests with macOS leaks tool"

# Mark phony targets
.PHONY: all clean test install rebuild help asan test-asan leaks test-leaks
