#include "../third_party/doctest.h"
#include <iostream>

// Keep the old function for backward compatibility with existing main.cpp
void cli_run_tests() {
    // Run the Doctest tests
    doctest::Context context;
    int result = context.run();
    
    if (result == 0) {
        printf("All tests passed!\n");
    } else {
        printf("Some tests failed (exit code: %d)\n", result);
    }
}