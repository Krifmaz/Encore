//! `.toast`: short messages bottom right, gone after four seconds. An error stays until it is
//! dismissed and carries a Copy button, so it can be read in full and pasted into a report. A toast
//! can carry one action ("Up next cleared · Undo", the web's `toast_action`) and then stays as long
//! as the action can be taken.
//!
//! The toasts and their rules are the kit's (`kit_iced::toast`); this app's toast action is a player
//! message (Undo), and the stack sits above the player bar.

use super::El;
use crate::theme;

pub use kit_iced::toast::ToastKind;

/// One toast of this app: its action is a player message.
pub type Toast = kit_iced::toast::Toast<crate::player::Message>;

/// The app's toasts, with the kit's rules (a repeated error does not pile up, at most three errors
/// stay, at most five toasts show).
pub type Toasts = kit_iced::toast::Toasts<crate::player::Message>;

/// The toast column, to be laid over the shell bottom right (above the player bar). `on_dismiss(id)`
/// closes one early; `on_copy(id)` copies an error's text; `on_action(id)` takes a toast's action.
pub fn view<'a, M: Clone + 'a>(
    toasts: &'a [Toast],
    on_dismiss: impl Fn(u64) -> M,
    on_copy: impl Fn(u64) -> M,
    on_action: impl Fn(u64) -> M,
) -> El<'a, M> {
    kit_iced::toast::view(toasts, theme::PLAYER_H + 18.0, on_dismiss, on_copy, on_action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn errors_stay_until_closed_and_the_rest_go() {
        let mut toasts = Toasts::default();
        toasts.push("x", ToastKind::Err);
        toasts.push("Copied to clipboard", ToastKind::Ok);
        // the failed copy says what to do instead: it goes like any other toast
        toasts.push(super::super::details::COPY_FAILED, ToastKind::Err);
        toasts.push_action("Up next cleared", "Undo", crate::player::Message::UndoClear, Duration::from_millis(6000));
        let list = toasts.list();
        assert!(list[0].stays() && list[0].expires_at().is_none());
        assert!(!list[1].stays() && list[1].life_ms == 4200);
        assert!(!list[2].stays() && list[2].expires_at().is_some());
        // a toast with Undo stays as long as Undo can be pressed
        assert_eq!(list[3].life_ms, 6000);
        let undo = list[3].id;
        assert!(matches!(toasts.take_action(undo), Some(crate::player::Message::UndoClear)));
    }
}
