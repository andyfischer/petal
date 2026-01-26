#pragma once

struct Program;

struct ParseProgramOptions {
    bool stdout_trace = false;
};

Program* parse_program(const char* source, const ParseProgramOptions& options = {});