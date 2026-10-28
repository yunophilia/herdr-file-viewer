//! Right-click context menu: a small modal overlay of entries for the surface under the cursor
//! (a tree row or the content pane). Every entry names the key that already performs it, so the
//! menu only exposes the existing keyboard vocabulary through the mouse (AC-18 stays intact).
//! Part of the Session Controller.

use super::*;

/// One context-menu entry: what it says and what selecting it does.
pub(super) struct MenuItem {
    pub label: String,
    pub action: MenuAction,
}

/// What activating a menu entry performs. `Intent` entries are dispatched through
/// [`Controller::handle`] — the identical code path the key takes — after the menu closes.
#[derive(Clone, Copy)]
pub(super) enum MenuAction {
    Intent(Intent),
    /// Copy the standing ambient content selection (the drag-to-select highlight).
    CopySelection,
}

/// The open context menu: entries, the highlighted row, and the cell it opened at (the
/// Presenter anchors the popup there, clamped inside the frame).
pub(super) struct ContextMenuState {
    pub items: Vec<MenuItem>,
    pub cursor: usize,
    pub anchor: (u16, u16),
}

impl Controller {
    /// Open the context menu for a right-click at `(col, row)`. Over a tree row the click first
    /// selects the row (exactly like a left-click), then offers that row's actions; over the
    /// content pane it offers the content actions. Anywhere else is inert. Only ever opens on
    /// the fresh press — the modal gate routes everything else while a menu is up.
    pub(super) fn open_context_menu(&mut self, col: u16, row: u16) -> Effects {
        let items = match self.hit_test(col, row) {
            MouseRegion::TreeRow(idx) => {
                if idx >= self.tree.visible_nodes().len() {
                    return Effects::noop(); // empty tail below the rows — nothing to act on
                }
                // Right-click selects the row first, as every platform's context menu does:
                // the entries act on the row under the cursor, not a stale selection.
                self.last_click = None;
                self.action_notice = None;
                self.focus = Focus::Tree;
                self.tree.set_cursor(idx);
                self.dispatch_render();
                self.tree_menu_items()
            }
            MouseRegion::Content | MouseRegion::ContentTitle | MouseRegion::ContentSearch => {
                self.last_click = None;
                self.focus = Focus::Content;
                self.content_menu_items()
            }
            _ => return Effects::noop(),
        };
        if items.is_empty() {
            return Effects::noop();
        }
        self.modal = Modal::ContextMenu(ContextMenuState {
            items,
            cursor: 0,
            anchor: (col, row),
        });
        Effects::redraw()
    }

    /// The row-menu entries for the selected node (the one right-click just selected).
    fn tree_menu_items(&self) -> Vec<MenuItem> {
        let Some(node) = self.tree.selected() else {
            return Vec::new();
        };
        let intent = |label: &str, intent: Intent| MenuItem {
            label: label.to_string(),
            action: MenuAction::Intent(intent),
        };
        let mut items = Vec::new();
        match node.kind {
            NodeKind::Dir => items.push(if node.expanded {
                intent("Collapse directory  h", Intent::Collapse)
            } else {
                intent("Expand directory  l", Intent::Expand)
            }),
            NodeKind::File => items.push(intent("Open full-screen  Enter", Intent::Activate)),
        }
        items.push(intent("Copy path  y", Intent::CopyRepoPath));
        items.push(intent("Copy absolute path  Y", Intent::CopyAbsPath));
        if node.kind == NodeKind::File {
            items.push(intent("Annotate  a", Intent::AddAnnotation));
            items.push(intent("Open in editor  e", Intent::OpenInEditor));
            items.push(intent("Pin preview  p", Intent::PinPreview));
        }
        items
    }

    /// The content-pane menu entries. Only what's live right now: view cycling and the diff
    /// presentation when a file is selected, copy-selection only while a selection stands.
    fn content_menu_items(&self) -> Vec<MenuItem> {
        let intent = |label: &str, intent: Intent| MenuItem {
            label: label.to_string(),
            action: MenuAction::Intent(intent),
        };
        let mut items = Vec::new();
        if self
            .tree
            .selected()
            .is_some_and(|n| n.kind == NodeKind::File)
        {
            items.push(intent("Cycle view  v", Intent::CycleView));
        }
        items.push(intent("Toggle wrap  w", Intent::ToggleWrap));
        if self.tree.selected().is_some_and(|n| {
            matches!(
                self.effective_mode(&n.path),
                ViewMode::Diff | ViewMode::FullDiff
            )
        }) {
            items.push(intent("Diff presentation  D", Intent::CycleDiffRender));
        }
        if self.active_interaction.selection.is_some() {
            items.push(MenuItem {
                label: "Copy selection".to_string(),
                action: MenuAction::CopySelection,
            });
        }
        items
    }

    /// Route an intent while the context menu is open (modal, mirroring the picker): Up/Down
    /// move the highlight, Enter activates, Esc/`q` dismiss, everything else is swallowed so no
    /// viewer action fires beneath the menu.
    pub(super) fn handle_menu_intent(&mut self, intent: Intent) -> Effects {
        let Some(menu) = self.modal.context_menu_mut() else {
            return Effects::noop();
        };
        match intent {
            Intent::NavUp => {
                if menu.cursor > 0 {
                    menu.cursor -= 1;
                    return Effects::redraw();
                }
                Effects::noop()
            }
            Intent::NavDown => {
                if menu.cursor + 1 < menu.items.len() {
                    menu.cursor += 1;
                    return Effects::redraw();
                }
                Effects::noop()
            }
            Intent::Activate => {
                let idx = menu.cursor;
                self.perform_menu_item(idx)
            }
            Intent::Close => {
                self.modal = Modal::None;
                Effects::redraw()
            }
            _ => Effects::noop(),
        }
    }

    /// Mouse handling while the context menu owns the pointer: a left press on an entry performs
    /// it, a press outside the popup dismisses the menu (consumed — it never reaches the
    /// columns), the wheel moves the highlight, and everything else is inert.
    pub(super) fn handle_menu_mouse(&mut self, ev: MouseEvent) -> Effects {
        if ev.modifiers.contains(KeyModifiers::SHIFT) {
            return Effects::noop(); // Shift+mouse stays with the terminal
        }
        let pos = Position {
            x: ev.column,
            y: ev.row,
        };
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = self.geom.menu_items.iter().position(|r| r.contains(pos)) {
                    return self.perform_menu_item(idx);
                }
                if self.geom.menu_popup.is_some_and(|r| r.contains(pos)) {
                    return Effects::noop(); // the popup's border/padding is not a dismiss target
                }
                // Outside the popup: dismiss, and swallow so the click can't change the
                // selection under the menu.
                self.modal = Modal::None;
                Effects::redraw()
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.geom.menu_popup.is_some_and(|r| r.contains(pos)) {
                    Effects::noop() // inside the menu: inert
                } else {
                    self.modal = Modal::None; // right-click elsewhere dismisses
                    Effects::redraw()
                }
            }
            MouseEventKind::ScrollDown => self.menu_move(1),
            MouseEventKind::ScrollUp => self.menu_move(-1),
            _ => Effects::noop(),
        }
    }

    fn menu_move(&mut self, delta: i8) -> Effects {
        let Some(menu) = self.modal.context_menu_mut() else {
            return Effects::noop();
        };
        let len = menu.items.len() as i32;
        let next = (menu.cursor as i32 + delta as i32).clamp(0, (len - 1).max(0));
        if next as usize != menu.cursor {
            menu.cursor = next as usize;
            Effects::redraw()
        } else {
            Effects::noop()
        }
    }

    /// Perform a menu entry and close the menu. The entry action runs through the ordinary
    /// intent path once `Modal::None`, so menu and key can never diverge.
    fn perform_menu_item(&mut self, idx: usize) -> Effects {
        let Some(menu) = self.modal.context_menu() else {
            return Effects::noop();
        };
        let Some(item) = menu.items.get(idx) else {
            self.modal = Modal::None; // geometry/state disagree: still dismiss safely
            return Effects::redraw();
        };
        let action = item.action;
        self.modal = Modal::None;
        match action {
            MenuAction::Intent(intent) => self.handle(intent),
            MenuAction::CopySelection => self.copy_content_selection(),
        }
    }

    /// The owned menu draw model for the Presenter, or `None` while no menu is open.
    pub(super) fn menu_view(&self) -> Option<ContextMenuView> {
        let menu = self.modal.context_menu()?;
        Some(ContextMenuView {
            items: menu.items.iter().map(|i| i.label.clone()).collect(),
            cursor: menu.cursor,
            anchor: menu.anchor,
        })
    }
}
