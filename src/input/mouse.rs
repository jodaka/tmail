//! Mouse → action translation (plan §10, Phase 10.1/10.2).
//!
//! Rendering records the rectangle of every interactive widget into a
//! [`HitMap`]; a click is hit-tested against the last frame's map and
//! translated into [`Action::Click`], carrying a [`ClickTarget`] the
//! reducer applies with full state context. Wheel events map onto the same
//! `MoveUp`/`MoveDown` actions the arrow keys produce, so the mouse can
//! never do anything the keyboard cannot (plan §10: "do not create
//! mouse-only behavior").
//!
//! The layer is pure: no terminal access, no state mutation — everything
//! the translation needs comes in as arguments, so it is unit-testable
//! without a terminal.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::app::action::{Action, ClickTarget};
use crate::app::focus::Focus;
use crate::app::state::AppState;

/// Widget rectangles recorded during one frame. Regions may nest (a modal
/// button sits inside the modal area inside the screen); hit-testing picks
/// the smallest containing region so the innermost widget wins.
#[derive(Debug, Default, Clone)]
pub struct HitMap {
    regions: Vec<(Rect, ClickTarget)>,
}

impl HitMap {
    /// Record one widget rectangle. Recorded later draws "on top"; the
    /// smallest-area rule resolves overlap the same way either way.
    pub fn push(&mut self, area: Rect, target: ClickTarget) {
        if area.width > 0 && area.height > 0 {
            self.regions.push((area, target));
        }
    }

    /// Forget all regions (start of a fresh frame).
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// The deepest target under (`x`, `y`), or `None`. While a modal is
    /// open only modal-button regions are returned: the modal intercepts
    /// all input (plan §9), so clicks "through" it onto the dimmed screen
    /// behind do nothing.
    pub fn hit_test(&self, x: u16, y: u16, modal_open: bool) -> Option<ClickTarget> {
        let mut best: Option<(u32, ClickTarget)> = None;
        for (area, target) in &self.regions {
            if modal_open != is_modal_target(*target) {
                continue;
            }
            let inside = x >= area.x
                && y >= area.y
                && x < area.x.saturating_add(area.width)
                && y < area.y.saturating_add(area.height);
            if !inside {
                continue;
            }
            let area_key = u32::from(area.width).saturating_mul(u32::from(area.height));
            // Strictly smaller replaces; ties keep the earliest recorded.
            if best.is_none_or(|(best_area, _)| area_key < best_area) {
                best = Some((area_key, *target));
            }
        }
        best.map(|(_, target)| target)
    }
}

/// Modal buttons are the only targets clickable while an overlay is open.
fn is_modal_target(target: ClickTarget) -> bool {
    matches!(
        target,
        ClickTarget::ErrorButton(_) | ClickTarget::ConfirmButton(_)
    )
}

/// Translate a mouse event into an action. `None` = no binding: motion,
/// drag, and middle/right buttons do nothing in v1.
pub fn to_action(event: MouseEvent, hits: &HitMap, state: &AppState) -> Option<Action> {
    match event.kind {
        MouseEventKind::ScrollUp => wheel(state, -1),
        MouseEventKind::ScrollDown => wheel(state, 1),
        // Click on release: a press that turns into a drag never activates
        // anything.
        MouseEventKind::Up(MouseButton::Left) => {
            let modal_open = state.overlay.is_some();
            hits.hit_test(event.column, event.row, modal_open)
                .map(Action::Click)
        }
        _ => None,
    }
}

/// Wheel: scroll the focused area (plan §10) with the exact actions the
/// arrow keys produce. The reducer gives `MoveUp`/`MoveDown` their
/// contextual meaning — list selection, sidebar selection, reader scroll,
/// modal scroll. Text fields and the composer are deliberately unmapped:
/// wheeling over them must not move carets.
fn wheel(state: &AppState, delta: i64) -> Option<Action> {
    let action = match state.focus {
        Focus::MessageList | Focus::Sidebar | Focus::Reader | Focus::ErrorModal => {
            if delta < 0 {
                Action::MoveUp
            } else {
                Action::MoveDown
            }
        }
        // Wheeling the picker does not preview themes — arrows only.
        Focus::SearchField
        | Focus::Composer
        | Focus::Dialog
        | Focus::ThemePicker
        | Focus::SelectAllToggle
        // The wizard has no scrollable region yet (keyboard-first, ADR 0003).
        | Focus::Wizard => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::overlay::{ConfirmButton, ModalButton};

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn hit_test_prefers_the_smallest_region() {
        let mut hits = HitMap::default();
        hits.push(rect(0, 0, 100, 100), ClickTarget::MessageRow(0));
        hits.push(rect(10, 10, 10, 1), ClickTarget::MessageRow(7));
        assert_eq!(
            hits.hit_test(15, 10, false),
            Some(ClickTarget::MessageRow(7))
        );
        assert_eq!(
            hits.hit_test(15, 11, false),
            Some(ClickTarget::MessageRow(0))
        );
        assert_eq!(hits.hit_test(200, 200, false), None);
    }

    #[test]
    fn hit_test_zero_sized_regions_are_ignored() {
        let mut hits = HitMap::default();
        hits.push(rect(5, 5, 0, 3), ClickTarget::SearchField);
        assert_eq!(hits.hit_test(5, 5, false), None);
    }

    #[test]
    fn modal_targets_are_isolated_while_an_overlay_is_open() {
        let mut hits = HitMap::default();
        hits.push(rect(0, 0, 100, 100), ClickTarget::MessageRow(1));
        hits.push(
            rect(40, 10, 9, 1),
            ClickTarget::ErrorButton(ModalButton::Retry),
        );
        // Overlay open: only the modal button is reachable.
        assert_eq!(
            hits.hit_test(44, 10, true),
            Some(ClickTarget::ErrorButton(ModalButton::Retry))
        );
        assert_eq!(hits.hit_test(15, 10, true), None);
        // Overlay closed: the modal region (stale) is ignored instead.
        assert_eq!(
            hits.hit_test(44, 10, false),
            Some(ClickTarget::MessageRow(1))
        );
    }

    #[test]
    fn confirm_buttons_are_modal_targets_too() {
        let mut hits = HitMap::default();
        hits.push(
            rect(40, 10, 15, 1),
            ClickTarget::ConfirmButton(ConfirmButton::Keep),
        );
        assert_eq!(
            hits.hit_test(42, 10, true),
            Some(ClickTarget::ConfirmButton(ConfirmButton::Keep))
        );
    }

    #[test]
    fn wheel_maps_to_move_actions_only_in_scrollable_foci() {
        let mut state = AppState::initial(20);
        for focus in [
            Focus::MessageList,
            Focus::Sidebar,
            Focus::Reader,
            Focus::ErrorModal,
        ] {
            state.focus = focus;
            assert_eq!(wheel(&state, 1), Some(Action::MoveDown), "{focus:?}");
            assert_eq!(wheel(&state, -1), Some(Action::MoveUp), "{focus:?}");
        }
        // Editing foci: the wheel must not move carets.
        for focus in [Focus::SearchField, Focus::Composer, Focus::Dialog] {
            state.focus = focus;
            assert_eq!(wheel(&state, 1), None, "{focus:?}");
        }
    }
}
