# petal-c-bridge: builds the Rust staticlib with cargo and exposes it as the
# CMake target `petal::bridge` (static library + include dir + system libs).
# Guide: docs/embedding-c.md at the repository root.
#
# Works both as a subdirectory of a host's build
# (add_subdirectory(path/to/petal/integrations/petal-c-bridge)) and standalone:
#
#   cmake -S integrations/petal-c-bridge -B integrations/petal-c-bridge/build -G Ninja
#   cmake --build integrations/petal-c-bridge/build
#   ctest --test-dir integrations/petal-c-bridge/build --output-on-failure
#
# Cache options:
#   PETAL_BRIDGE_PROFILE      cargo profile: "release" (default, even for Debug
#                             C++ builds — the VM is slow unoptimized) or "dev".
#   PETAL_CARGO_TARGET_DIR    CARGO_TARGET_DIR for the build.
#   PETAL_BRIDGE_BUILD_TESTS  build bridge tests + example (default ON).
cmake_minimum_required(VERSION 3.20)

if(CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)
  project(petal_bridge LANGUAGES C CXX)
  set(CMAKE_CXX_STANDARD 20)
  set(CMAKE_CXX_STANDARD_REQUIRED ON)
  set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
  enable_testing()
  # Standalone builds use the crate's own target/ dir, as `cargo build` in
  # this directory would, so the two share one build.
  set(_petal_default_target_dir "${CMAKE_CURRENT_SOURCE_DIR}/target")
else()
  set(_petal_default_target_dir "${CMAKE_BINARY_DIR}/cargo-target")
endif()

set(PETAL_BRIDGE_PROFILE "release" CACHE STRING "Cargo profile used to build petal-bridge (release|dev)")
set_property(CACHE PETAL_BRIDGE_PROFILE PROPERTY STRINGS release dev)
set(PETAL_CARGO_TARGET_DIR "${_petal_default_target_dir}" CACHE PATH "CARGO_TARGET_DIR for petal-bridge")
option(PETAL_BRIDGE_BUILD_TESTS "Build the petal-bridge tests and example" ON)

find_program(CARGO_EXECUTABLE cargo HINTS "$ENV{HOME}/.cargo/bin" REQUIRED)

# Cargo writes the "dev" profile to target/debug, every other profile to target/<name>.
if(PETAL_BRIDGE_PROFILE STREQUAL "dev")
  set(_petal_profile_dir "debug")
else()
  set(_petal_profile_dir "${PETAL_BRIDGE_PROFILE}")
endif()

if(WIN32)
  set(_petal_lib_name "petal_bridge.lib")
else()
  set(_petal_lib_name "libpetal_bridge.a")
endif()
set(PETAL_BRIDGE_LIBRARY "${PETAL_CARGO_TARGET_DIR}/${_petal_profile_dir}/${_petal_lib_name}")

# Always invoke cargo: it is incremental, so an up-to-date build is a quick
# no-op, and it tracks the Rust sources (including the vendored Petal crates)
# far better than a hand-maintained DEPENDS list could. Cargo.toml points at
# the in-repo Petal crates (../../rust, ../../petal-ui, ../../petal-query).
add_custom_target(petal_bridge_cargo
  COMMAND ${CMAKE_COMMAND} -E env "CARGO_TARGET_DIR=${PETAL_CARGO_TARGET_DIR}"
          "${CARGO_EXECUTABLE}" build --profile "${PETAL_BRIDGE_PROFILE}"
          --manifest-path "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml"
  BYPRODUCTS "${PETAL_BRIDGE_LIBRARY}"
  WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
  COMMENT "Building petal-bridge (cargo --profile ${PETAL_BRIDGE_PROFILE})"
  USES_TERMINAL
  VERBATIM)

add_library(petal_bridge INTERFACE)
add_library(petal::bridge ALIAS petal_bridge)
add_dependencies(petal_bridge petal_bridge_cargo)
target_include_directories(petal_bridge INTERFACE "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_compile_features(petal_bridge INTERFACE cxx_std_20)
target_link_libraries(petal_bridge INTERFACE "${PETAL_BRIDGE_LIBRARY}")

# System libraries the Rust standard library needs (from
# `cargo rustc -- --print native-static-libs`). On macOS that is only
# libSystem/libc/libm, which the C/C++ driver links anyway.
if(APPLE)
  # nothing extra
elseif(UNIX)
  find_package(Threads REQUIRED)
  target_link_libraries(petal_bridge INTERFACE Threads::Threads ${CMAKE_DL_LIBS} m)
elseif(WIN32)
  target_link_libraries(petal_bridge INTERFACE ws2_32 userenv bcrypt ntdll advapi32 kernel32)
endif()

if(PETAL_BRIDGE_BUILD_TESTS)
  # tests/test_harness.hpp is a tiny header-only runner (no framework download).
  add_executable(petal_bridge_tests tests/bridge_tests.cpp)
  target_link_libraries(petal_bridge_tests PRIVATE petal::bridge)
  target_compile_definitions(petal_bridge_tests PRIVATE
    PETAL_TEST_SCRATCH="${CMAKE_CURRENT_BINARY_DIR}/test-scratch"
    PETAL_LIBS_DIR="${CMAKE_CURRENT_SOURCE_DIR}/../../petal-libs")
  if(NOT MSVC)
    target_compile_options(petal_bridge_tests PRIVATE -Wall -Wextra)
  endif()
  add_test(NAME petal_bridge_tests COMMAND petal_bridge_tests)

  # The C header must also compile as plain C.
  add_executable(petal_bridge_c_smoke tests/c_smoke.c)
  target_link_libraries(petal_bridge_c_smoke PRIVATE petal::bridge)
  add_test(NAME petal_bridge_c_smoke COMMAND petal_bridge_c_smoke)

  add_executable(petal_bridge_hello examples/hello.cpp)
  target_link_libraries(petal_bridge_hello PRIVATE petal::bridge)
  add_test(NAME petal_bridge_hello COMMAND petal_bridge_hello)

  # The crate's own Rust unit tests (error-position parsing, boxed-native
  # ownership, typed reload/call errors, scenarios). Built on first run into
  # the same cargo target dir, so give it time.
  add_test(NAME petal_bridge_rust_unit
           COMMAND ${CMAKE_COMMAND} -E env "CARGO_TARGET_DIR=${PETAL_CARGO_TARGET_DIR}"
                   "${CARGO_EXECUTABLE}" test --profile "${PETAL_BRIDGE_PROFILE}" --lib --quiet
                   --manifest-path "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml")
  set_tests_properties(petal_bridge_rust_unit PROPERTIES TIMEOUT 900)
endif()
