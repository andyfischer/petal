#pragma once
#include "standard_headers.h"
#include "host/host_api.h"
#include "runtime/vm.h"
#include <cairo/cairo.h>

// Cairo module - expose Cairo graphics library functions to Petal

const char* source = " \
struct Cairo; \
struct CairoSurface; \
\
// Cairo context functions \
func cairo_create(CairoSurface) -> Cairo; \
func cairo_destroy(Cairo); \
func cairo_save(Cairo); \
func cairo_restore(Cairo); \
\
// Surface functions \
func cairo_image_surface_create(i32, i32, i32) -> CairoSurface; \
func cairo_surface_destroy(CairoSurface); \
func cairo_surface_write_to_png(CairoSurface, string); \
\
// Drawing functions \
func cairo_paint(Cairo); \
func cairo_clear(Cairo); \
func cairo_set_source_rgb(Cairo, f32, f32, f32); \
func cairo_set_source_rgba(Cairo, f32, f32, f32, f32); \
\
// Text functions \
func cairo_select_font_face(Cairo, string, i32, i32); \
func cairo_set_font_size(Cairo, f32); \
func cairo_show_text(Cairo, string); \
func cairo_text_path(Cairo, string); \
func cairo_move_to(Cairo, f32, f32); \
\
// Path functions \
func cairo_rectangle(Cairo, f32, f32, f32, f32); \
func cairo_fill(Cairo); \
func cairo_stroke(Cairo); \
";

// Wrapper functions for all Cairo functions that can be used by the VM.

// Context management
Variant32 cairo_create_wrapped(VM* vm) {
    cairo_surface_t* surface = (cairo_surface_t*)petal_get_void_ptr(vm);
    cairo_t* ctx = cairo_create(surface);
    return Variant32::from_heap_ptr(ctx);
}

Variant32 cairo_destroy_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    cairo_destroy(ctx);
    return Variant32::None();
}

Variant32 cairo_save_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    cairo_save(ctx);
    return Variant32::None();
}

Variant32 cairo_restore_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    cairo_restore(ctx);
    return Variant32::None();
}

// Surface functions
Variant32 cairo_image_surface_create_wrapped(VM* vm) {
    i32 format = petal_get_i32(vm);  // Cairo format (CAIRO_FORMAT_ARGB32 = 0)
    i32 width = petal_get_i32(vm);
    i32 height = petal_get_i32(vm);
    cairo_surface_t* surface = cairo_image_surface_create((cairo_format_t)format, width, height);
    return Variant32::from_heap_ptr(surface);
}

Variant32 cairo_surface_destroy_wrapped(VM* vm) {
    cairo_surface_t* surface = (cairo_surface_t*)petal_get_void_ptr(vm);
    cairo_surface_destroy(surface);
    return Variant32::None();
}

Variant32 cairo_surface_write_to_png_wrapped(VM* vm) {
    cairo_surface_t* surface = (cairo_surface_t*)petal_get_void_ptr(vm);
    // TODO: Extract string from VM properly
    const char* filename = "output.png";  // Placeholder
    cairo_surface_write_to_png(surface, filename);
    return Variant32::None();
}

// Drawing functions
Variant32 cairo_paint_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    cairo_paint(ctx);
    return Variant32::None();
}

Variant32 cairo_clear_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    cairo_save(ctx);
    cairo_set_operator(ctx, CAIRO_OPERATOR_CLEAR);
    cairo_paint(ctx);
    cairo_restore(ctx);
    return Variant32::None();
}

Variant32 cairo_set_source_rgb_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    f32 r = petal_get_f32(vm);
    f32 g = petal_get_f32(vm);
    f32 b = petal_get_f32(vm);
    cairo_set_source_rgb(ctx, r, g, b);
    return Variant32::None();
}

Variant32 cairo_set_source_rgba_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    f32 r = petal_get_f32(vm);
    f32 g = petal_get_f32(vm);
    f32 b = petal_get_f32(vm);
    f32 a = petal_get_f32(vm);
    cairo_set_source_rgba(ctx, r, g, b, a);
    return Variant32::None();
}

// Text functions
Variant32 cairo_select_font_face_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    // TODO: Extract string from VM properly
    const char* family = "Arial";  // Placeholder
    i32 slant = petal_get_i32(vm);  // CAIRO_FONT_SLANT_NORMAL = 0
    i32 weight = petal_get_i32(vm); // CAIRO_FONT_WEIGHT_NORMAL = 0
    cairo_select_font_face(ctx, family, (cairo_font_slant_t)slant, (cairo_font_weight_t)weight);
    return Variant32::None();
}

Variant32 cairo_set_font_size_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    f32 size = petal_get_f32(vm);
    cairo_set_font_size(ctx, size);
    return Variant32::None();
}

Variant32 cairo_show_text_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    // TODO: Extract string from VM properly
    const char* text = "Hello Cairo!";  // Placeholder
    cairo_show_text(ctx, text);
    return Variant32::None();
}

Variant32 cairo_text_path_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    // TODO: Extract string from VM properly
    const char* text = "Text Path";  // Placeholder
    cairo_text_path(ctx, text);
    return Variant32::None();
}

Variant32 cairo_move_to_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    f32 x = petal_get_f32(vm);
    f32 y = petal_get_f32(vm);
    cairo_move_to(ctx, x, y);
    return Variant32::None();
}

// Path functions
Variant32 cairo_rectangle_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    f32 x = petal_get_f32(vm);
    f32 y = petal_get_f32(vm);
    f32 width = petal_get_f32(vm);
    f32 height = petal_get_f32(vm);
    cairo_rectangle(ctx, x, y, width, height);
    return Variant32::None();
}

Variant32 cairo_fill_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    cairo_fill(ctx);
    return Variant32::None();
}

Variant32 cairo_stroke_wrapped(VM* vm) {
    cairo_t* ctx = (cairo_t*)petal_get_void_ptr(vm);
    cairo_stroke(ctx);
    return Variant32::None();
}

extern "C" Program* petal_module_setup() {
    Program* program = petal_parse_defs(source);

    // Context management
    petal_register_host_function(program, "cairo_create", cairo_create_wrapped);
    petal_register_host_function(program, "cairo_destroy", cairo_destroy_wrapped);
    petal_register_host_function(program, "cairo_save", cairo_save_wrapped);
    petal_register_host_function(program, "cairo_restore", cairo_restore_wrapped);
    
    // Surface functions
    petal_register_host_function(program, "cairo_image_surface_create", cairo_image_surface_create_wrapped);
    petal_register_host_function(program, "cairo_surface_destroy", cairo_surface_destroy_wrapped);
    petal_register_host_function(program, "cairo_surface_write_to_png", cairo_surface_write_to_png_wrapped);
    
    // Drawing functions
    petal_register_host_function(program, "cairo_paint", cairo_paint_wrapped);
    petal_register_host_function(program, "cairo_clear", cairo_clear_wrapped);
    petal_register_host_function(program, "cairo_set_source_rgb", cairo_set_source_rgb_wrapped);
    petal_register_host_function(program, "cairo_set_source_rgba", cairo_set_source_rgba_wrapped);
    
    // Text functions
    petal_register_host_function(program, "cairo_select_font_face", cairo_select_font_face_wrapped);
    petal_register_host_function(program, "cairo_set_font_size", cairo_set_font_size_wrapped);
    petal_register_host_function(program, "cairo_show_text", cairo_show_text_wrapped);
    petal_register_host_function(program, "cairo_text_path", cairo_text_path_wrapped);
    petal_register_host_function(program, "cairo_move_to", cairo_move_to_wrapped);
    
    // Path functions
    petal_register_host_function(program, "cairo_rectangle", cairo_rectangle_wrapped);
    petal_register_host_function(program, "cairo_fill", cairo_fill_wrapped);
    petal_register_host_function(program, "cairo_stroke", cairo_stroke_wrapped);
    
    return program;
}
