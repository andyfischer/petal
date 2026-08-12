//! Building the render frame: every pane's primitives plus the status bar
//! (focused file, vim mode, cursor position, and any script error or note).

use garden_render::{Color, Primitive, Rect, Scene, TextStyle, FONT_SIZE};

use super::{App, ToolbarAction, ToolbarButton, MARGIN, TRAFFIC_LIGHTS_W};

/// Horizontal padding inside a toolbar button, logical pixels.
const TOOLBAR_BTN_PAD: f32 = 11.0;

/// Most result rows the fuzzy finder shows at once (it scrolls beyond that).
const FINDER_MAX_ROWS: usize = 12;

impl App {
    /// Build the frame for the current state: every pane's primitives plus
    /// the status bar (focused file, cursor position, any script error).
    ///
    /// Every build advances the global frame counter ([`App::frame`]); the
    /// value after this call is the built scene's frame number, which the
    /// debug server reports (`X-Garden-Frame`, `GET /frame`, `/state.frame`).
    pub fn build_scene(&self) -> Scene {
        self.frame.set(self.frame.get() + 1);
        let mut prims = Vec::new();
        let cell = self.viewport.cell;

        for (i, pane) in self.panes.iter().enumerate() {
            if let Some(panel) = &pane.panel {
                let awake = panel.is_awake(std::time::Instant::now());
                panel.build_scene(pane.rect, cell, &self.theme, awake, &mut prims);
            } else {
                pane.view
                    .build_scene(pane.rect, cell, i == self.focus, &self.theme, &mut prims);
            }
        }

        // Draggable split dividers: a thin line in each inter-pane gap, drawn
        // over the panes and brightened under the pointer (or while dragging)
        // so it reads as a resize handle.
        let (mx, my) = self.mouse;
        for d in &self.dividers {
            let active = match &self.divider_drag {
                Some(dd) => dd.path == d.path && dd.before == d.before,
                None => d.rect.contains(mx, my),
            };
            let color = if active {
                self.theme.border_focused
            } else {
                self.theme.border
            };
            let line = if d.vertical {
                Rect {
                    x: d.rect.x + d.rect.w / 2.0 - 1.0,
                    y: d.rect.y,
                    w: 2.0,
                    h: d.rect.h,
                }
            } else {
                Rect {
                    x: d.rect.x,
                    y: d.rect.y + d.rect.h / 2.0 - 1.0,
                    w: d.rect.w,
                    h: 2.0,
                }
            };
            prims.push(Primitive::Quad { rect: line, color });
        }

        // The Petal-IDE state inspector (`:State`): overlay each panel pane's
        // observed values and frame count in the top-left corner, over a
        // translucent card.
        if self.show_panel_state {
            for pane in &self.panes {
                if let Some(panel) = &pane.panel {
                    self.draw_state_overlay(pane.rect, panel, cell, &mut prims);
                }
            }
        }

        let (w, h) = self.viewport.size;
        self.build_titlebar(w, cell, &mut prims);
        self.build_toolbar(w, cell, &mut prims);
        let status_h = self.status_height();
        let bar = Rect {
            x: 0.0,
            y: h - status_h,
            w,
            h: status_h,
        };
        prims.push(Primitive::Quad {
            rect: bar,
            color: self.theme.status_bg,
        });
        let text_y = bar.y + 4.0;
        if let Some(cl) = &self.command_line {
            // The command/search line takes over the status bar while open.
            prims.push(Primitive::Text {
                pos: (8.0, text_y),
                text: format!("{}_", cl.display()),
                color: self.theme.status_text,
                clip: bar,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
        } else if let Some(pane) = self.panes.get(self.focus) {
            // Keystrokes go to a panel's focused editable region, not the pane's
            // own (placeholder) buffer — so that region is what the bar has to
            // report, or it reads a frozen `NORMAL … 1:1` while the user edits.
            let region = pane
                .panel
                .as_ref()
                .and_then(|p| p.focused_region())
                .and_then(|id| pane.panel.as_ref().unwrap().region_view(id));
            let view = region.unwrap_or(&pane.view);
            let c = view.cursor;
            prims.push(Primitive::Text {
                pos: (8.0, text_y),
                text: format!(
                    "{}  {}  {}:{}",
                    view.vim.mode.label(),
                    pane.view.title(),
                    c.line + 1,
                    c.col + 1
                ),
                color: self.theme.status_text,
                clip: bar,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
        }
        // The right slot shows an error (red) if any — a standing script
        // reload error (prefixed so it reads as the script's, not the user's)
        // outranks a transient action error — else an informational note
        // (file written / reloaded from disk) in the normal status color.
        // Suppressed while the command line is open: it takes over the whole
        // bar, so a long command never overlaps the right-slot text.
        if self.command_line.is_none() {
            if let Some(err) = &self.script_error {
                prims.push(Primitive::Text {
                    pos: (w * 0.4, text_y),
                    text: format!("script: {}", err.replace('\n', " ")),
                    color: self.theme.error_text,
                    clip: bar,
                    size: FONT_SIZE,
                    style: TextStyle::default(),
                });
            } else if let Some(err) = self.effective_status_error() {
                prims.push(Primitive::Text {
                    pos: (w * 0.4, text_y),
                    text: err.replace('\n', " "),
                    color: self.theme.error_text,
                    clip: bar,
                    size: FONT_SIZE,
                    style: TextStyle::default(),
                });
            } else if let Some(note) = &self.status_note {
                prims.push(Primitive::Text {
                    pos: (w * 0.4, text_y),
                    text: note.replace('\n', " "),
                    color: self.theme.status_text,
                    clip: bar,
                    size: FONT_SIZE,
                    style: TextStyle::default(),
                });
            }
        }

        // The fuzzy file finder, when open, overlays everything else.
        self.build_file_finder(&mut prims);

        Scene {
            bg: self.theme.window_bg,
            primitives: prims,
        }
    }

    /// Draw the fuzzy file finder overlay: a centered panel with a query line, a
    /// match count, and the scored results, the selection highlighted. A no-op
    /// unless the finder is open. See [`crate::file_finder`].
    fn build_file_finder(&self, prims: &mut Vec<Primitive>) {
        let Some(ff) = &self.file_finder else { return };
        let (w, h) = self.viewport.size;
        let (cell_w, cell_h) = self.viewport.cell;
        let pad = 8.0;
        let row_h = cell_h + 4.0;

        let view = ff.visible(FINDER_MAX_ROWS);
        let body_rows = view.paths.len().max(1); // reserve a row for "no matches"
        let header_h = row_h + pad;
        let panel_w = (w * 0.6).clamp(360.0, 760.0).min(w - 2.0 * MARGIN);
        let panel_h = header_h + body_rows as f32 * row_h + pad;
        let x = ((w - panel_w) / 2.0).max(MARGIN);
        // Sit a little below the titlebar + IDE toolbar, but never off the bottom.
        let chrome_top = self.top_inset + self.toolbar_h;
        let y = (chrome_top + 48.0).min((h - panel_h - MARGIN).max(chrome_top + MARGIN));
        let panel = Rect {
            x,
            y,
            w: panel_w,
            h: panel_h,
        };

        // A 1px frame: a slightly larger border-colored quad behind the panel.
        prims.push(Primitive::Quad {
            rect: Rect {
                x: x - 1.0,
                y: y - 1.0,
                w: panel_w + 2.0,
                h: panel_h + 2.0,
            },
            color: self.theme.border_focused,
        });
        prims.push(Primitive::Quad {
            rect: panel,
            color: self.theme.pane_bg_focused,
        });

        // Query line, with a block "cursor" appended like the command line.
        let text_x = x + pad;
        prims.push(Primitive::Text {
            pos: (text_x, y + pad / 2.0),
            text: format!("> {}_", ff.query()),
            color: self.theme.text,
            clip: panel,
            size: FONT_SIZE,
            style: TextStyle::default(),
        });
        // Match count, right-aligned in the header.
        let count = format!("{}", view.total);
        let count_x = x + panel_w - pad - count.chars().count() as f32 * cell_w;
        prims.push(Primitive::Text {
            pos: (count_x, y + pad / 2.0),
            text: count,
            color: self.theme.text_dim,
            clip: panel,
            size: FONT_SIZE,
            style: TextStyle::default(),
        });
        // Hairline under the header.
        prims.push(Primitive::Quad {
            rect: Rect {
                x,
                y: y + header_h - 1.0,
                w: panel_w,
                h: 1.0,
            },
            color: self.theme.border,
        });

        let list_top = y + header_h;
        if view.paths.is_empty() {
            prims.push(Primitive::Text {
                pos: (text_x, list_top + 2.0),
                text: "no matching files".to_string(),
                color: self.theme.text_dim,
                clip: panel,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
            return;
        }

        let max_chars = ((panel_w - 2.0 * pad) / cell_w).max(1.0) as usize;
        for (i, path) in view.paths.iter().enumerate() {
            let row_y = list_top + i as f32 * row_h;
            let selected = view.selected_row == Some(i);
            if selected {
                prims.push(Primitive::Quad {
                    rect: Rect {
                        x,
                        y: row_y,
                        w: panel_w,
                        h: row_h,
                    },
                    color: self.theme.selection,
                });
            }
            prims.push(Primitive::Text {
                pos: (text_x, row_y + 2.0),
                text: fit_tail(path, max_chars),
                color: if selected {
                    self.theme.text
                } else {
                    self.theme.text_dim
                },
                clip: panel,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
        }
    }

    /// Draw the slim titlebar across the top: a background band that matches the
    /// pane content, a hairline separator beneath it, and the focused document's
    /// name centered (clear of the macOS traffic-light controls on the left).
    /// A no-op unless a frontend reserved space with [`enable_titlebar`](App::enable_titlebar).
    fn build_titlebar(&self, w: f32, cell: (f32, f32), prims: &mut Vec<Primitive>) {
        let bar_h = self.top_inset;
        if bar_h <= 0.0 {
            return;
        }
        let bar = Rect {
            x: 0.0,
            y: 0.0,
            w,
            h: bar_h,
        };
        prims.push(Primitive::Quad {
            rect: bar,
            color: self.theme.titlebar_bg,
        });
        // Hairline separator so the band reads as chrome above the panes.
        prims.push(Primitive::Quad {
            rect: Rect {
                x: 0.0,
                y: bar_h - 1.0,
                w,
                h: 1.0,
            },
            color: self.theme.border,
        });

        // The focused document's name, centered but never under the controls.
        let title = self
            .panes
            .get(self.focus)
            .map(|p| p.view.title())
            .unwrap_or_else(|| "Garden".to_string());
        let text_w = title.chars().count() as f32 * cell.0;
        let x = ((w - text_w) / 2.0).max(TRAFFIC_LIGHTS_W);
        let y = ((bar_h - cell.1) / 2.0).max(0.0);
        prims.push(Primitive::Text {
            pos: (x, y),
            text: title,
            color: self.theme.titlebar_text,
            clip: bar,
            size: FONT_SIZE,
            style: TextStyle::default(),
        });
    }

    /// Lay out the Petal-IDE toolbar buttons, left-to-right in the band below the
    /// titlebar. Empty outside IDE mode. Shared by [`build_toolbar`](Self::build_toolbar)
    /// (draw) and [`dispatch_toolbar`](Self::dispatch_toolbar) (hit-test) so the
    /// clickable regions always match the drawn ones. Labels are static; the
    /// play/pause label flips with [`paused`](App::paused).
    pub(in crate::app) fn toolbar_buttons(&self) -> Vec<ToolbarButton> {
        if self.toolbar_h <= 0.0 {
            return Vec::new();
        }
        let (cw, _) = self.viewport.cell;
        let band_y = self.top_inset;
        let btn_h = (self.toolbar_h - 10.0).max(1.0);
        let btn_y = band_y + (self.toolbar_h - btn_h) / 2.0;
        let ir_open = self.ide.as_ref().is_some_and(|i| i.ir_open);
        let specs: [(ToolbarAction, &'static str, bool); 4] = [
            (
                ToolbarAction::TogglePlay,
                if self.paused { "> Play" } else { "|| Pause" },
                self.paused,
            ),
            (ToolbarAction::ToggleIr, "IR", ir_open),
            (ToolbarAction::ToggleState, "State", self.show_panel_state),
            (ToolbarAction::ResetSketch, "Reset", false),
        ];
        let mut x = MARGIN;
        let mut out = Vec::new();
        for (action, label, active) in specs {
            let text_w = label.chars().count() as f32 * cw;
            let bw = text_w + 2.0 * TOOLBAR_BTN_PAD;
            out.push(ToolbarButton {
                rect: Rect {
                    x,
                    y: btn_y,
                    w: bw,
                    h: btn_h,
                },
                action,
                label,
                active,
            });
            x += bw + 6.0;
        }
        out
    }

    /// Draw the Petal-IDE toolbar: a band below the titlebar with a hairline
    /// separator and the control buttons (play/pause, IR, state, reset). An
    /// active button reads as lit (filled + bright label). A no-op outside IDE
    /// mode (`toolbar_h == 0`).
    fn build_toolbar(&self, w: f32, cell: (f32, f32), prims: &mut Vec<Primitive>) {
        if self.toolbar_h <= 0.0 {
            return;
        }
        let band = Rect {
            x: 0.0,
            y: self.top_inset,
            w,
            h: self.toolbar_h,
        };
        prims.push(Primitive::Quad {
            rect: band,
            color: self.theme.titlebar_bg,
        });
        // Hairline separator beneath the band, so it reads as chrome.
        prims.push(Primitive::Quad {
            rect: Rect {
                x: 0.0,
                y: band.y + band.h - 1.0,
                w,
                h: 1.0,
            },
            color: self.theme.border,
        });
        for btn in self.toolbar_buttons() {
            let (bg, fg) = if btn.active {
                (self.theme.selection, self.theme.text)
            } else {
                (self.theme.pane_bg, self.theme.text_dim)
            };
            prims.push(Primitive::Quad {
                rect: btn.rect,
                color: bg,
            });
            // A thin outline so an inactive button still reads as a control.
            if !btn.active {
                push_outline(prims, btn.rect, self.theme.border);
            }
            let ty = btn.rect.y + (btn.rect.h - cell.1) / 2.0;
            prims.push(Primitive::Text {
                pos: (btn.rect.x + TOOLBAR_BTN_PAD, ty),
                text: btn.label.to_string(),
                color: fg,
                clip: btn.rect,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
        }
    }

    /// Draw the live-state inspector card in the top-left of a panel `rect`.
    fn draw_state_overlay(
        &self,
        rect: Rect,
        panel: &crate::panel_view::PanelView,
        cell: (f32, f32),
        prims: &mut Vec<Primitive>,
    ) {
        let lines = panel_state_lines(panel);
        let (cw, ch) = cell;
        let pad = 6.0;
        let line_h = ch + 2.0;
        let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
        let card = Rect {
            x: rect.x,
            y: rect.y,
            w: (widest * cw + pad * 2.0).min(rect.w),
            h: (lines.len() as f32 * line_h + pad * 2.0).min(rect.h),
        };
        prims.push(Primitive::Quad {
            rect: card,
            // A translucent dark card so the overlay reads over any canvas.
            color: Color::rgba(0.04, 0.05, 0.07, 0.85),
        });
        for (i, line) in lines.iter().enumerate() {
            let color = if line.starts_with(' ') {
                self.theme.text
            } else {
                self.theme.text_dim
            };
            prims.push(Primitive::Text {
                pos: (card.x + pad, card.y + pad + i as f32 * line_h),
                text: line.clone(),
                color,
                clip: card,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
        }
    }
}

/// Format a panel's observed values and frame count into display lines for the
/// state inspector. Section headers are flush-left; values are indented, so the
/// caller can color them differently.
///
/// One `values` section carries the whole picture, because a `state` var is a
/// named term like any other and observation already reports it — a separate
/// `state` section would mostly restate this one. The exception is state that
/// observation *structurally* cannot represent: a `state` declared inside a loop
/// (or keyed explicitly) exists once per iteration and dumps as `sel[3]` /
/// `scroll[k17…]`, while observation keeps one slot per term and would show only
/// the final iteration's value. Those keys — the ones `state_json` has that
/// `observed` doesn't — get their own trailing section rather than being lost.
fn panel_state_lines(panel: &crate::panel_view::PanelView) -> Vec<String> {
    let mut lines = Vec::new();
    let fmt = |k: &str, v: &serde_json::Value| {
        let mut v = serde_json::to_string(v).unwrap_or_default();
        if v.chars().count() > 60 {
            v = v.chars().take(57).collect::<String>() + "…";
        }
        format!("  {k} = {v}")
    };

    let observed = panel.observed();
    lines.push(format!("values ({})", observed.len()));
    let mut keys: Vec<&String> = observed.keys().collect();
    keys.sort();
    for k in keys {
        lines.push(fmt(k, &observed[k]));
    }

    let state = panel.state_json();
    let mut extra: Vec<&String> = state
        .keys()
        .filter(|k| !observed.contains_key(*k))
        .collect();
    extra.sort();
    if !extra.is_empty() {
        lines.push(format!("per-key state ({})", extra.len()));
        for k in extra {
            lines.push(fmt(k, &state[k]));
        }
    }

    lines.push(format!("frame {}", panel.frame_count()));
    lines
}

/// Push a 1px rectangular outline (four edge quads) around `r` in `color`.
fn push_outline(prims: &mut Vec<Primitive>, r: Rect, color: Color) {
    let edges = [
        Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: 1.0,
        },
        Rect {
            x: r.x,
            y: r.y + r.h - 1.0,
            w: r.w,
            h: 1.0,
        },
        Rect {
            x: r.x,
            y: r.y,
            w: 1.0,
            h: r.h,
        },
        Rect {
            x: r.x + r.w - 1.0,
            y: r.y,
            w: 1.0,
            h: r.h,
        },
    ];
    for rect in edges {
        prims.push(Primitive::Quad { rect, color });
    }
}

/// Shorten `s` to at most `max` characters, keeping the **tail** (the filename
/// end, which is the part the user cares about) behind a leading ellipsis.
fn fit_tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let tail: String = s.chars().skip(n - (max - 1)).collect();
    format!("…{tail}")
}
