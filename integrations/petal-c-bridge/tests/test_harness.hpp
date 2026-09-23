// A minimal header-only test runner for the petal-c-bridge C++ tests (no
// framework download).
//
//   TEST(name) { ... }          registers a test
//   CHECK(cond)                 records a failure and continues
//   REQUIRE(cond)               records a failure and aborts the current test
//   CHECK_EQ(a, b)              a == b, printing both on failure
//   CHECK_NEAR(a, b, tol)       |a - b| <= tol
//   CHECK_CONTAINS(s, needle)   string contains
//
//   int main(int argc, char** argv) { return petal_test::run_tests(argc, argv); }
//
// The runner takes optional name filters on the command line:
//   petal_bridge_tests            # everything
//   petal_bridge_tests reload     # tests whose name contains "reload"
// It exits non-zero when any check failed or no test ran.
#pragma once

#include <cmath>
#include <cstdio>
#include <cstring>
#include <exception>
#include <functional>
#include <ostream>
#include <sstream>
#include <string>
#include <vector>

namespace petal_test {

struct Test {
    const char* name;
    std::function<void()> fn;
};
inline std::vector<Test>& registry() {
    static std::vector<Test> tests;
    return tests;
}
inline int failures = 0;
struct Abort {};
struct Registrar {
    Registrar(const char* name, std::function<void()> fn) { registry().push_back({name, std::move(fn)}); }
};

inline void fail(const char* file, int line, const std::string& what) {
    ++failures;
    std::fprintf(stderr, "    FAIL %s:%d: %s\n", file, line, what.c_str());
}

template <class T>
void print_value(std::ostream& os, const T& v) {
    if constexpr (requires { os << v; }) {
        os << v;
    } else if constexpr (requires { v.begin(); v.end(); }) {
        os << "{";
        for (const auto& item : v) { print_value(os, item); os << " "; }
        os << "}";
    } else if constexpr (requires { v.has_value(); *v; }) {
        if (v) print_value(os, *v); else os << "nullopt";
    } else {
        os << "<unprintable>";
    }
}
template <class A, class B>
std::string show(const char* expr, const A& a, const B& b) {
    std::ostringstream os;
    os << expr << "  (got ";
    print_value(os, a);
    os << ", expected ";
    print_value(os, b);
    os << ")";
    return os.str();
}

inline void check_near(const char* file, int line, const char* expr, double a, double b, double tol) {
    if (!(std::fabs(a - b) <= tol)) fail(file, line, show(expr, a, b));
}

/// Describes an exception that escaped a test; suites can install a better
/// one for their own exception types.
inline std::function<std::string(const std::exception&)> describe_exception = [](const std::exception& e) {
    return std::string("uncaught exception: ") + e.what();
};

inline int run_tests(int argc, char** argv) {
    int ran = 0, failed_tests = 0;
    for (const Test& t : registry()) {
        bool selected = argc <= 1;
        for (int i = 1; i < argc; ++i)
            if (std::strstr(t.name, argv[i])) selected = true;
        if (!selected) continue;
        ++ran;
        const int before = failures;
        std::printf("[ RUN  ] %s\n", t.name);
        std::fflush(stdout);
        try {
            t.fn();
        } catch (const Abort&) {
        } catch (const std::exception& e) {
            fail(__FILE__, __LINE__, describe_exception(e));
        }
        const bool ok = failures == before;
        if (!ok) ++failed_tests;
        std::printf("[ %s ] %s\n", ok ? " OK " : "FAIL", t.name);
        std::fflush(stdout);
    }
    std::printf("\n%d test(s) run, %d failed, %d failed check(s)\n", ran, failed_tests, failures);
    return (failures == 0 && ran > 0) ? 0 : 1;
}

}  // namespace petal_test

#define PETAL_TEST_CAT2(a, b) a##b
#define PETAL_TEST_CAT(a, b) PETAL_TEST_CAT2(a, b)
#define TEST(name)                                                                              \
    static void test_##name();                                                                  \
    static ::petal_test::Registrar PETAL_TEST_CAT(registrar_, name)(#name, test_##name);        \
    static void test_##name()

#define CHECK(cond) \
    do { if (!(cond)) ::petal_test::fail(__FILE__, __LINE__, #cond); } while (0)
#define REQUIRE(cond)                                                \
    do {                                                             \
        if (!(cond)) {                                               \
            ::petal_test::fail(__FILE__, __LINE__, #cond);           \
            throw ::petal_test::Abort{};                             \
        }                                                            \
    } while (0)
#define CHECK_EQ(a, b)                                                                              \
    do {                                                                                            \
        const auto& va_ = (a);                                                                      \
        const auto& vb_ = (b);                                                                      \
        if (!(va_ == vb_))                                                                          \
            ::petal_test::fail(__FILE__, __LINE__, ::petal_test::show(#a " == " #b, va_, vb_));     \
    } while (0)
#define CHECK_NEAR(a, b, tol) \
    ::petal_test::check_near(__FILE__, __LINE__, #a " ~= " #b " +- " #tol, double(a), double(b), (tol))
#define CHECK_CONTAINS(haystack, needle)                                                            \
    do {                                                                                            \
        const std::string h_ = (haystack);                                                          \
        if (h_.find(needle) == std::string::npos)                                                   \
            ::petal_test::fail(__FILE__, __LINE__, std::string(#haystack " contains \"") + (needle) + \
                                                       "\"  (got \"" + h_ + "\")");                 \
    } while (0)
