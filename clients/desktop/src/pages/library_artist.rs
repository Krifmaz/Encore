//! Artist: the library filtered to one artist (`#/library?artist=<name>`). Behaviour
//! reference: web-rs/src/pages/library.rs `render` with `st.artist` set – it is the Library page
//! with the artist badge, its Share button and `&artist=` on the album requests, so this page
//! wraps [`super::library`]. Under its Albums tab, the artist's One-offs shelf (0.36, §6.3 of
//! docs/contracts/library-types.md): songs from compilations, remix albums and soundtracks the
//! library has only a few songs of; the Type dropdown's One-offs lists their albums.

use super::library;
use super::library_types as lt;
use crate::api::ApiError;
use crate::pages::{Ctx, El};
use iced::widget::column;
use iced::{Subscription, Task};
use serde_json::Value;

#[derive(Debug, Default)]
pub struct State {
    pub name: String,
    pub inner: library::State,
    /// `/library/artists/{name}/one-offs`: `(total, songs)`; None until it answers (or when it failed).
    one_offs: Option<(usize, Vec<Value>)>,
    loading_one_offs: bool,
}

#[derive(Clone, Debug)]
pub enum Message {
    Lib(library::Message),
    /// The One-offs shelf for this artist name.
    OneOffs(String, Result<Value, ApiError>),
    OneOffsAll,
    PlayOneOff(usize),
    OneOffMenu(usize),
}

/// Called when the app navigates here (with the route's parameters).
pub fn open(state: &mut State, name: String, ctx: &mut Ctx) -> Task<Message> {
    state.name = name.clone();
    state.one_offs = None;
    state.loading_one_offs = false;
    Task::batch([
        library::open_with(&mut state.inner, String::new(), name.clone(), ctx).map(Message::Lib),
        load_one_offs(ctx, name, lt::ONE_OFFS_FIRST),
    ])
}

fn load_one_offs(ctx: &Ctx, name: String, limit: usize) -> Task<Message> {
    let api = ctx.api.clone();
    let url = super::artist::artist_route(&name, "", "one-offs") + &format!("&offset=0&limit={limit}");
    Task::perform(async move { api.get(&url).await }, move |r| Message::OneOffs(name, r))
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Lib(m) => return library::update(&mut state.inner, m, ctx).map(Message::Lib),
        Message::OneOffs(name, r) => {
            if name == state.name {
                let all = std::mem::take(&mut state.loading_one_offs);
                match r {
                    Ok(v) => state.one_offs = Some(super::artist::songs_answer(&v)),
                    Err(e) if all => ctx.toast_err(e.message),
                    Err(_) => {}
                }
            }
        }
        Message::OneOffsAll => {
            if !state.loading_one_offs {
                state.loading_one_offs = true;
                return load_one_offs(ctx, state.name.clone(), lt::ONE_OFFS_ALL);
            }
        }
        Message::PlayOneOff(i) => {
            if let Some((_, list)) = &state.one_offs {
                if i < list.len() {
                    ctx.play(list.clone(), i);
                }
            }
        }
        Message::OneOffMenu(i) => {
            if let Some(t) = state.one_offs.as_ref().and_then(|(_, l)| l.get(i)).cloned() {
                return library::track_menu(&mut state.inner.shared, &t, ctx)
                    .map(|a| Message::Lib(library::Message::Shared(a)));
            }
        }
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let page = library::view(&state.inner, ctx).map(Message::Lib);
    // the shelf under the Albums tab while it lists the albums (a Type picked lists its own)
    let shelf = (state.inner.tab == "albums" && state.inner.album_type.is_empty() && state.inner.albums.is_some())
        .then(|| {
            lt::one_offs_section(
                ctx,
                state.one_offs.as_ref(),
                state.loading_one_offs,
                Message::PlayOneOff,
                Message::OneOffMenu,
                Message::OneOffsAll,
            )
        })
        .flatten();
    match shelf {
        Some(s) => column![page, s].spacing(26).into(),
        None => page,
    }
}

/// A modal or ⋯ menu to lay over the whole window, when one is open.
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    library::overlay(&state.inner, ctx).map(|e| e.map(Message::Lib))
}

pub fn subscription(state: &State) -> Subscription<Message> {
    library::subscription(&state.inner).map(Message::Lib)
}
