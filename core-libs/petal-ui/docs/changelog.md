# `ui` prelude levels

The changelog of [`PRELUDE_LEVEL`](../src/lib.rs), moved here verbatim from
its doc comment. `PRELUDE_LEVEL` is the monotonic counter of *additive*
prelude growth; see [Versioning](../README.md#versioning). When you bump it,
add the new level to the end of this list.

A host that reports capability by *name* (Garden's `garden --version`
derives the export list from [`prelude_source`]) does not need this; it is
for the coarse "how new is this?" check.

Levels:
- 1 — the prelude as of the first release (`UI_VERSION` 1).
- 2 — `luma(c)`, `contrast_text(bg)`, `text_field_update(fc, id, r, buf)`
  and the 4-argument `draw_text_field(r, text, has, style)` (2026-08-12).
- 3 — the component-library expansion (2026-08-25): host-palette theme
  resolution ([`input::bind_host_palette`]; `ui_theme()` defaults from the
  host, semantic tokens + spacing/radius/type scales on `theme`), RectCut
  layout (`cut_left/right/top/bottom`, `split_h/split_v`, `pad`,
  `hstack/vstack`, `row/col`), motion (`approach`, `ease_out`,
  `ease_in_out`), and the widget set: `spinner`/`spinner_glyph`,
  `progress_bar`, `checkbox`, `toggle`, `radio_group`, `slider`, `tab_bar`,
  `tooltip`, the modal family (`modal_state` … `modal`), `badge`/`pill`,
  `card`, `empty_state`, `hint`/`hint_bar`, `splitter_state`/`splitter`,
  the table family (`table_state`/`table_col_rects`/`table`), `wrap_px`,
  the load-state family (`load_state` … `draw_load`), and caret editing in
  `text_field` (`{focus, text, caret, submitted}`, 5-argument
  `draw_text_field(r, text, has, caret, style)`).
- 4 — font objects (2026-08-30): `font(name)` / `font(name, size)` and the
  decorations `font_size`, `font_weight`, `font_bold`, `font_italic`,
  `font_spacing`, `font_color`, plus the `fonts()` native listing the
  families the host can draw. Backed by the optional
  [`text::FontSource`] a host attaches, which is what lets a script name a
  face installed on the machine rather than only the host's own.
- 5 — gradients, shadows and nested clips (2026-08-30):
  `draw_rect_gradient` / `draw_rect_gradient_rounded` /
  `draw_circle_gradient` and the multi-stop `linear_gradient(rect, stops,
  angle, [radius])`; `draw_shadow(rect, opts)` and its positional form,
  backed by [`tess::shadow_mesh`]; `clip_push`/`clip_pop` (clips that nest
  instead of replacing), a trailing `radius` on `clip` and `draw_image`,
  and the record overloads of `clip`.
- 6 — proportional type, real elevation, sRGB tints (2026-08-31): the theme
  gained a `font` face (defaulting to `"ui"`, so widget text is no longer
  monospace) and an `elevation_1/2/3` scale; every widget now measures and
  draws through one style record; `over` / `tint` / `hairline` /
  `alpha_pct` composite flat tints to opaque colours; `draw_elevation`,
  `draw_focus_ring`, `draw_scroll_shadow`, `draw_scrim`, `draw_fade`,
  `draw_area_fill`, `draw_text_along` and the axis-label helpers.
- 7 — layers, backdrops and blur (2026-09-01): the offscreen-canvas ops
  joined the standard draw set and grew `snapshot_to` (copy what is
  already painted into a canvas), `blur_canvas` (an in-place Gaussian) and
  opacity/scaling on `draw_canvas`; `draw_to` returns the target it
  replaced. The prelude builds `layer(rect, opts, body)` (a closure drawn
  into a canvas and composited back — group opacity, a blurred glow),
  `snapshot(rect)`, `draw_backdrop_blur` and `draw_material` (the
  translucent background-sampling surface behind an iOS bar) on those.
- 8 — text editing (2026-10-09): `text_field` gained a selection, the
  clipboard and undo (its result adds `changed`, `anchor`, `sel_start`,
  `sel_end`, `scroll`, `overflow`; `draw_text_field` a 6-argument form
  taking that result); `clipboard_get` / `clipboard_set` over the
  [`clipboard`] channel; `text_wrap_rows` (rows with their character
  ranges), `text_row_of` and `text_range_rects`. `text_wrap` no longer
  collapses a run of spaces inside a row.
- 9 — layers, hit testing, retained drawing (2026-10-13): `ui_layer` /
  `with_layer` / `ui_overlay` (a layer orders drawing and the pointer),
  `ui_modal`, `interact` / `ui_hot` / `ui_block`, `pointer_captured`, and
  the target registry (`ui_target`, `ui_target_rect`, [`hit::targets`]) —
  every prelude widget now goes through them and registers itself;
  `cached_layer` and `draw_rects` ([`compose`]); `draw_rect` as one native
  in every shape, and a packed-int colour there and in `draw_polyline`;
  `draw_image` with `tint` / `src` / `slice`, `image_load`, `image_size`;
  `shadow` / `outline` / `tracking` on a text style; `flow_begin` …
  `flow_end`, `ui_measure`, `clip_rect`. `clip_none` is a prelude function
  over the native now (the prelude tracks the clip in force).
- 10 — multi-line text (2026-10-16): `text_area` / `text_area_update` /
  `draw_text_area` / `text_area_height`. Behavior changes alongside it: a
  press outside a focused `text_field` clears its focus, and
  `interact().click` goes only to a rect that was under the press when
  the button went down.
- 11 — declarative dragging (2026-11-06): the `ui/drag` module
  (`import ui/drag`: `place`, `draggable`, `drag_session`, the drag-spec
  constructors, the candidate-set helpers, `drag_debug` /
  `drag_explain`; [`DRAG_MODULE_NAME`], `docs/drag.md`), not re-exported
  by `ui`; the input native `ui_debug(name)`. Behavior change alongside
  it: `ui_measure` runs its body through core `speculate`, so state,
  `set`s, random draws and registrations made inside it are undone.
- 12 — runtime focus (2026-11-07): `shortcut(chord)`,
  `shortcut(chord, id)`, `has_focus`, `focus_owner`, `focus_request`,
  `interact`'s `wants`, and the natives `ui_focus`, `ui_focus_request`,
  `ui_shortcut` ([`focus`]); `popover`, `popover_open`, `popover_close`,
  `popover_shown`, `popover_owner` and the natives `ui_popover`,
  `ui_popover_dismissed`, `ui_popover_request` ([`popover`]). Behavior
  changes alongside it: overlapping rects have one owner, the one in
  front (`hit`, "One owner"); `focus_set` / `focus_clear` also make a
  focus request; a context menu is opened and dismissed by the runtime,
  so it no longer skips its opening frame (`menu_state` records carry an
  `id`).
- 13 — sub-pixel drawing (2026-11-08): `pixel_snap(on)` (`true` snaps
  everything, `false` nothing, `nil` restores the default of snapping the
  rect family only) and the native `pixel_snap` behind it. It takes effect
  on a host registered with `DrawOptions { subpixel: true }`
  ([`draw::register_draw_with`], drained with
  [`draw::take_frame_commands`]); on every other host it only checks its
  argument. No behavior change for a host that does not opt in.

[`prelude_source`]: ../src/lib.rs
[`DRAG_MODULE_NAME`]: ../src/lib.rs
[`input::bind_host_palette`]: ../src/input.rs
[`text::FontSource`]: ../src/text.rs
[`tess::shadow_mesh`]: ../src/tess.rs
[`clipboard`]: ../src/clipboard.rs
[`hit::targets`]: ../src/hit.rs
[`compose`]: ../src/compose.rs
[`focus`]: ../src/focus.rs
[`popover`]: ../src/popover.rs
[`draw::register_draw_with`]: ../src/draw.rs
[`draw::take_frame_commands`]: ../src/subpixel.rs
