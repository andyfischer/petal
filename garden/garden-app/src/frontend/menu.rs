//! Native macOS menu bar (File / Edit), used by the windowed frontend.
//!
//! macOS only: it installs an `NSMenu` via [`muda`] and routes clicks (and the
//! standard ⌘ accelerators) back into the core through
//! [`App::dispatch_menu`](crate::app::App::dispatch_menu), so the menu items do
//! exactly what the keyboard shortcuts do. On other platforms (and so the
//! windowed frontend stays cross-platform) this is a no-op stub.

#[cfg(target_os = "macos")]
pub use imp::MenuBar;

#[cfg(not(target_os = "macos"))]
pub use stub::MenuBar;

#[cfg(target_os = "macos")]
mod imp {
    use muda::accelerator::{Accelerator, Code, Modifiers};
    use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

    use crate::app::MenuAction;
    use crate::theme::ThemeScheme;

    /// Owns the installed menu (kept alive for the process lifetime) and maps
    /// each item's id to the action it triggers. `Open…` / `Open Folder…` have
    /// no static action — they show a picker at click time — so their ids are
    /// tracked on their own.
    pub struct MenuBar {
        _menu: Menu,
        items: Vec<(MenuId, MenuAction)>,
        open_id: MenuId,
        open_folder_id: MenuId,
    }

    fn accel(mods: Modifiers, code: Code) -> Option<Accelerator> {
        Some(Accelerator::new(Some(mods), code))
    }

    impl MenuBar {
        /// Build the File/Edit/View/Go/Git/Window menus and install them as the
        /// macOS app menu.
        pub fn new() -> MenuBar {
            let menu = Menu::new();
            let mut items = Vec::new();
            let sup = Modifiers::SUPER;
            let sup_shift = Modifiers::SUPER | Modifiers::SHIFT;

            // The first submenu is the application menu (shown as "Garden").
            let app_menu = Submenu::new("Garden", true);
            let quit = MenuItem::new("Quit Garden", true, accel(sup, Code::KeyQ));
            let _ = app_menu.append_items(&[
                &PredefinedMenuItem::about(Some("About Garden"), None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &quit,
            ]);
            items.push((quit.id().clone(), MenuAction::Quit));

            let file = Submenu::new("File", true);
            let new = MenuItem::new("New", true, accel(sup, Code::KeyN));
            let new_window = MenuItem::new("New Window", true, accel(sup_shift, Code::KeyN));
            let open = MenuItem::new("Open…", true, accel(sup, Code::KeyO));
            let open_folder = MenuItem::new("Open Folder…", true, accel(sup_shift, Code::KeyO));
            let save = MenuItem::new("Save", true, accel(sup, Code::KeyS));
            let save_all = MenuItem::new("Save All", true, accel(sup_shift, Code::KeyS));
            let close = MenuItem::new("Close Window", true, accel(sup, Code::KeyW));
            let _ = file.append_items(&[
                &new,
                &new_window,
                &open,
                &open_folder,
                &PredefinedMenuItem::separator(),
                &save,
                &save_all,
                &PredefinedMenuItem::separator(),
                &close,
            ]);
            items.push((new.id().clone(), MenuAction::NewFile));
            items.push((new_window.id().clone(), MenuAction::NewWindow));
            items.push((save.id().clone(), MenuAction::Save));
            items.push((save_all.id().clone(), MenuAction::SaveAll));
            items.push((close.id().clone(), MenuAction::CloseWindow));
            let open_id = open.id().clone();
            let open_folder_id = open_folder.id().clone();

            // Custom Edit items rather than muda's predefined ones: Garden draws
            // its own text (it is not an NSTextView), so the responder-chain
            // predefined cut/copy/paste/undo would never reach our editor.
            let edit = Submenu::new("Edit", true);
            let undo = MenuItem::new("Undo", true, accel(sup, Code::KeyZ));
            let redo = MenuItem::new("Redo", true, accel(sup_shift, Code::KeyZ));
            let cut = MenuItem::new("Cut", true, accel(sup, Code::KeyX));
            let copy = MenuItem::new("Copy", true, accel(sup, Code::KeyC));
            let paste = MenuItem::new("Paste", true, accel(sup, Code::KeyV));
            let select_all = MenuItem::new("Select All", true, accel(sup, Code::KeyA));
            let find = MenuItem::new("Find…", true, accel(sup, Code::KeyF));
            let find_next = MenuItem::new("Find Next", true, accel(sup, Code::KeyG));
            let find_prev = MenuItem::new("Find Previous", true, accel(sup_shift, Code::KeyG));
            let _ = edit.append_items(&[
                &undo,
                &redo,
                &PredefinedMenuItem::separator(),
                &cut,
                &copy,
                &paste,
                &select_all,
                &PredefinedMenuItem::separator(),
                &find,
                &find_next,
                &find_prev,
            ]);
            items.push((undo.id().clone(), MenuAction::Undo));
            items.push((redo.id().clone(), MenuAction::Redo));
            items.push((cut.id().clone(), MenuAction::Cut));
            items.push((copy.id().clone(), MenuAction::Copy));
            items.push((paste.id().clone(), MenuAction::Paste));
            items.push((select_all.id().clone(), MenuAction::SelectAll));
            items.push((find.id().clone(), MenuAction::Find));
            items.push((find_next.id().clone(), MenuAction::FindNext));
            items.push((find_prev.id().clone(), MenuAction::FindPrev));

            let view = Submenu::new("View", true);
            let color_scheme = Submenu::new("Color Scheme", true);
            for scheme in ThemeScheme::ALL {
                let item = MenuItem::new(scheme.label(), true, None);
                items.push((item.id().clone(), MenuAction::SetTheme(scheme)));
                let _ = color_scheme.append(&item);
            }
            let wrap = MenuItem::new("Toggle Soft Wrap", true, None);
            let line_numbers = MenuItem::new("Toggle Line Numbers", true, None);
            let inspector = MenuItem::new("Toggle State Inspector", true, None);
            let _ = view.append(&color_scheme);
            let _ = view.append_items(&[
                &PredefinedMenuItem::separator(),
                &wrap,
                &line_numbers,
                &PredefinedMenuItem::separator(),
                &inspector,
            ]);
            items.push((wrap.id().clone(), MenuAction::ToggleWrap));
            items.push((line_numbers.id().clone(), MenuAction::ToggleLineNumbers));
            items.push((inspector.id().clone(), MenuAction::ToggleStateInspector));

            let go = Submenu::new("Go", true);
            let go_to_file = MenuItem::new("Go to File…", true, accel(sup, Code::KeyP));
            let back = MenuItem::new("Back", true, accel(sup, Code::BracketLeft));
            let forward = MenuItem::new("Forward", true, accel(sup, Code::BracketRight));
            let explore = MenuItem::new("Browse File's Directory", true, None);
            let _ = go.append_items(&[
                &go_to_file,
                &PredefinedMenuItem::separator(),
                &back,
                &forward,
                &PredefinedMenuItem::separator(),
                &explore,
            ]);
            items.push((go_to_file.id().clone(), MenuAction::GoToFile));
            items.push((back.id().clone(), MenuAction::Back));
            items.push((forward.id().clone(), MenuAction::Forward));
            items.push((explore.id().clone(), MenuAction::ExploreDirectory));

            let git = Submenu::new("Git", true);
            let git_log = MenuItem::new("Show Log", true, None);
            let git_diff = MenuItem::new("Diff Working Tree", true, None);
            let git_diff_stat = MenuItem::new("Diff Stat", true, None);
            let review = MenuItem::new("Review Changes", true, None);
            let _ = git.append_items(&[
                &git_log,
                &PredefinedMenuItem::separator(),
                &git_diff,
                &git_diff_stat,
                &PredefinedMenuItem::separator(),
                &review,
            ]);
            items.push((git_log.id().clone(), MenuAction::GitLog));
            items.push((git_diff.id().clone(), MenuAction::GitDiff));
            items.push((git_diff_stat.id().clone(), MenuAction::GitDiffStat));
            items.push((review.id().clone(), MenuAction::ReviewChanges));

            // Pane management (vim's Ctrl+W commands) under the standard macOS
            // Window menu, alongside the native minimize/fullscreen items.
            let window = Submenu::new("Window", true);
            let split_down = MenuItem::new("Split Pane Down", true, None);
            let split_right = MenuItem::new("Split Pane Right", true, accel(sup, Code::Backslash));
            let close_others = MenuItem::new("Close Other Panes", true, None);
            let close_pane = MenuItem::new("Close Pane", true, None);
            let next_pane = MenuItem::new("Next Pane", true, None);
            let _ = window.append_items(&[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::fullscreen(None),
                &PredefinedMenuItem::separator(),
                &split_down,
                &split_right,
                &PredefinedMenuItem::separator(),
                &next_pane,
                &PredefinedMenuItem::separator(),
                &close_pane,
                &close_others,
            ]);
            items.push((split_down.id().clone(), MenuAction::SplitDown));
            items.push((split_right.id().clone(), MenuAction::SplitRight));
            items.push((close_others.id().clone(), MenuAction::CloseOtherPanes));
            items.push((close_pane.id().clone(), MenuAction::ClosePane));
            items.push((next_pane.id().clone(), MenuAction::NextPane));

            let _ = menu.append(&app_menu);
            let _ = menu.append(&file);
            let _ = menu.append(&edit);
            let _ = menu.append(&view);
            let _ = menu.append(&go);
            let _ = menu.append(&git);
            let _ = menu.append(&window);
            menu.init_for_nsapp();
            window.set_as_windows_menu_for_nsapp();

            MenuBar {
                _menu: menu,
                items,
                open_id,
                open_folder_id,
            }
        }

        /// Drain pending menu clicks into the actions to run. `Open…` / `Open
        /// Folder…` pop a native picker and yield an action only if the user
        /// picked something.
        pub fn drain(&self) -> Vec<MenuAction> {
            let mut actions = Vec::new();
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == self.open_id {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        actions.push(MenuAction::OpenFile(path));
                    }
                } else if event.id == self.open_folder_id {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        actions.push(MenuAction::OpenFolder(path));
                    }
                } else if let Some((_, action)) = self.items.iter().find(|(id, _)| *id == event.id)
                {
                    actions.push(action.clone());
                }
            }
            actions
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod stub {
    use crate::app::MenuAction;

    /// No menu bar on non-macOS platforms.
    pub struct MenuBar;

    impl MenuBar {
        pub fn new() -> MenuBar {
            MenuBar
        }

        pub fn drain(&self) -> Vec<MenuAction> {
            Vec::new()
        }
    }
}
