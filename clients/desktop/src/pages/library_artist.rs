//! Artist: the library filtered to one artist (`#/library?artist=<name>`). Behaviour
//! reference: web-rs/src/pages/library.rs `render` with `st.artist` set – it is the Library page
//! with the artist badge, its Share button and `&artist=` on the album requests, so this page
//! wraps [`super::library`].

use super::library;
use crate::pages::{Ctx, El};
use iced::{Subscription, Task};

#[derive(Debug, Default)]
pub struct State {
    pub name: String,
    pub inner: library::State,
}

#[derive(Clone, Debug)]
pub enum Message {
    Lib(library::Message),
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, name: String, ctx: &mut Ctx) -> Task<Message> {
    state.name = name.clone();
    library::open_with(&mut state.inner, String::new(), name, ctx).map(Message::Lib)
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Lib(m) => library::update(&mut state.inner, m, ctx).map(Message::Lib),
    }
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    library::view(&state.inner, ctx).map(Message::Lib)
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    library::overlay(&state.inner, ctx).map(|e| e.map(Message::Lib))
}

pub fn subscription(state: &State) -> Subscription<Message> {
    library::subscription(&state.inner).map(Message::Lib)
}
