//! `select.input`: a drop-down over a fixed table of `(key, label)` pairs, styled like the web's
//! selects. Pages keep the chosen key as a `String` and get it back from `on_pick`.

use super::El;
use crate::theme;
use iced::widget::pick_list;

/// One option of a [`select`]: the key a page stores and the words the list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub key: &'static str,
    pub label: &'static str,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label)
    }
}

/// The options of a `(key, label)` table.
pub fn choices(table: &[(&'static str, &'static str)]) -> Vec<Choice> {
    table.iter().map(|(key, label)| Choice { key, label }).collect()
}

/// The select; `selected` is a key of `table` (an unknown key shows the placeholder).
pub fn select<'a, M: Clone + 'a>(
    table: &[(&'static str, &'static str)],
    selected: &str,
    on_pick: impl Fn(String) -> M + 'a,
) -> El<'a, M> {
    let options = choices(table);
    let current = options.iter().find(|c| c.key == selected).copied();
    pick_list(options, current, move |c: Choice| on_pick(c.key.to_string()))
        .text_size(13)
        .padding([7, 10])
        .style(theme::pick)
        .menu_style(theme::pick_menu)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_keep_the_table_order_and_words() {
        let c = choices(&[("a", "First"), ("b", "Second")]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].key, "a");
        assert_eq!(c[1].to_string(), "Second");
    }
}
