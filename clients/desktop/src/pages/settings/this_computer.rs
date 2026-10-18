//! Settings → This computer. Behaviour reference: web-rs/src/pages/computer.rs (the classic app's
//! page): start with the computer, the mini player, global keyboard shortcuts, offline music and
//! updates. Everything here lives in the app's config, not on the account; the desktop module
//! (`crate::desktop`) applies it (shortcuts, start with the computer, the update check) and
//! reports back what it could not do.

use super::{block, card, help, switch};
use crate::api::VERSION;
use crate::config::Hotkeys;
use crate::desktop::{self, updater};
use crate::pages::{Ctx, El, Page};
use crate::theme::{self, Btn};
use crate::widgets::{btn_sm, t};
use iced::keyboard::{self, key, Key, Modifiers};
use iced::widget::{button, column, container, row};
use iced::{Alignment, Length, Subscription, Task};
use std::time::Duration;

/// The shortcuts, in the page's order: `(config key, label)`.
pub const KEYS: [(&str, &str); 5] = [
    ("toggle", "Play / pause"),
    ("next", "Next song"),
    ("prev", "Previous song"),
    ("volume_up", "Volume up"),
    ("volume_down", "Volume down"),
];

#[derive(Debug, Default)]
pub struct State {
    /// The shortcut box that listens for a key press.
    recording: Option<&'static str>,
    offline: usize,
}

#[derive(Clone, Debug)]
pub enum Message {
    Autostart(bool),
    MiniPlayer,
    HotkeysOn(bool),
    Record(&'static str),
    Key(Key, key::Physical, Modifiers),
    ResetKeys,
    OpenOffline,
    CheckAtStart(bool),
    Check,
    /// The check finished (the updater keeps what it found).
    Checked,
    Install,
    Installed(Result<updater::Outcome, String>),
    /// While the updater works: draw its progress again.
    Tick,
    OpenReleases,
}

/// A key press as a shortcut ("Ctrl+Alt+P"): Some("") clears (Backspace or Delete alone), None
/// while only modifiers are down or the key has no name.
pub fn accelerator(key: &Key, physical: &key::Physical, m: Modifiers) -> Option<String> {
    let mods = m.control() || m.alt() || m.shift() || m.logo();
    if matches!(key, Key::Named(key::Named::Backspace | key::Named::Delete)) && !mods {
        return Some(String::new());
    }
    if matches!(
        key,
        Key::Named(key::Named::Control | key::Named::Alt | key::Named::Shift | key::Named::Super | key::Named::Meta | key::Named::AltGraph | key::Named::Hyper)
    ) {
        return None;
    }
    let key::Physical::Code(code) = physical else { return None };
    let name = format!("{code:?}");
    let short = if let Some(c) = name.strip_prefix("Key").filter(|c| c.len() == 1) {
        c.to_string()
    } else if let Some(d) = name.strip_prefix("Digit").filter(|d| d.len() == 1) {
        d.to_string()
    } else if let Some(a) = name.strip_prefix("Arrow") {
        a.to_string()
    } else {
        name
    };
    let mut parts: Vec<String> = Vec::new();
    if m.control() {
        parts.push("Ctrl".into());
    }
    if m.alt() {
        parts.push("Alt".into());
    }
    if m.shift() {
        parts.push("Shift".into());
    }
    if m.logo() {
        parts.push("Super".into());
    }
    parts.push(short);
    Some(parts.join("+"))
}

fn hotkey<'a>(h: &'a Hotkeys, key: &str) -> &'a str {
    match key {
        "toggle" => &h.toggle,
        "next" => &h.next,
        "prev" => &h.prev,
        "volume_up" => &h.volume_up,
        _ => &h.volume_down,
    }
}

fn set_hotkey(h: &mut Hotkeys, key: &str, value: String) {
    match key {
        "toggle" => h.toggle = value,
        "next" => h.next = value,
        "prev" => h.prev = value,
        "volume_up" => h.volume_up = value,
        _ => h.volume_down = value,
    }
}

/// The updater is at work (checking, downloading, checking the file, installing).
fn updater_busy(p: &updater::Phase) -> bool {
    use updater::Phase;
    matches!(p, Phase::Checking | Phase::Downloading { .. } | Phase::Verifying(_) | Phase::Installing(_))
}

pub fn open(state: &mut State, _ctx: &mut Ctx) -> Task<Message> {
    state.recording = None;
    state.offline = desktop::offline::count();
    Task::none()
}

pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message> {
    match msg {
        Message::Autostart(on) => {
            ctx.edit_config(move |c| c.autostart = on);
            ctx.toast_ok("Saved");
        }
        Message::MiniPlayer => desktop::send(desktop::Message::MiniPlayer(!ctx.config.mini_player)),
        Message::HotkeysOn(on) => {
            ctx.edit_config(move |c| c.hotkeys.on = on);
            ctx.toast_ok("Saved");
        }
        Message::Record(k) => state.recording = if state.recording == Some(k) { None } else { Some(k) },
        Message::Key(key, physical, mods) => {
            let Some(which) = state.recording else { return Task::none() };
            if matches!(key, Key::Named(key::Named::Escape)) {
                state.recording = None;
                return Task::none();
            }
            if matches!(key, Key::Named(key::Named::Tab)) && !mods.control() && !mods.alt() && !mods.logo() {
                return Task::none();
            }
            let Some(acc) = accelerator(&key, &physical, mods) else { return Task::none() };
            if acc == hotkey(&ctx.config.hotkeys, which) {
                state.recording = None;
                return Task::none();
            }
            state.recording = None;
            ctx.edit_config(move |c| set_hotkey(&mut c.hotkeys, which, acc));
            ctx.toast_ok("Saved");
        }
        Message::ResetKeys => {
            ctx.edit_config(|c| c.hotkeys = Hotkeys::default());
            ctx.toast_ok("Saved");
        }
        Message::OpenOffline => ctx.navigate(Page::Offline),
        Message::CheckAtStart(on) => {
            ctx.edit_config(move |c| c.check_updates = on);
            ctx.toast_ok("Saved");
        }
        Message::Check => {
            return Task::perform(updater::check(), |_| Message::Checked);
        }
        Message::Checked | Message::Tick => {}
        Message::Install => {
            return Task::perform(updater::install_latest(), Message::Installed);
        }
        Message::Installed(Ok(updater::Outcome::Restarting)) => {}
        Message::Installed(Ok(updater::Outcome::Opened(words))) => ctx.toast(words),
        Message::Installed(Err(e)) => ctx.toast_err(if e.is_empty() { "The update didn't install.".to_string() } else { e }),
        Message::OpenReleases => ctx.open_url("https://github.com/Krifmaz/Encore/releases"),
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message> {
    let p = ctx.look;
    let c = ctx.config;
    let tray = if cfg!(target_os = "macos") { "menu bar" } else { "tray" };
    let errors = desktop::hotkeys::errors();

    let mut keys = column![].spacing(8);
    for (k, label) in KEYS {
        let value = hotkey(&c.hotkeys, k);
        let recording = state.recording == Some(k);
        let shown = if recording {
            "Press the keys…".to_string()
        } else if value.is_empty() {
            "Not set".to_string()
        } else {
            value.to_string()
        };
        let box_ = button(
            t(shown)
                .size(13)
                .font(theme::mono())
                .color(if recording { p.accent } else if value.is_empty() { p.faint } else { p.text }),
        )
        .padding([8, 12])
        .width(220)
        .style(theme::btn(if recording { Btn::NavOn } else { Btn::Default }))
        .on_press_maybe(c.hotkeys.on.then_some(Message::Record(k)));
        let mut line = column![row![container(help(label)).width(130), box_].spacing(10).align_y(Alignment::Center)].spacing(2);
        if let Some(e) = errors.get(k).filter(|_| c.hotkeys.on) {
            line = line.push(super::err_text(e.clone()));
        }
        keys = keys.push(line);
    }

    let phase = updater::state();
    let busy = updater_busy(&phase);
    let version = updater::available().map(|r| r.version).unwrap_or_default();
    let line = updater::describe(&phase);
    let is_err = matches!(phase, updater::Phase::Failed(_));
    let mut updates: Vec<El<'a, Message>> = vec![
        help(format!("Version {VERSION}")).into(),
        switch(c.check_updates, "Look for a new version when Encore starts", Message::CheckAtStart),
    ];
    if !line.is_empty() {
        updates.push(if is_err { super::err_text(line).into() } else { help(line).into() });
    }
    let mut actions = row![btn_sm("Check for updates", Btn::Default, (!busy).then_some(Message::Check))].spacing(8);
    if !version.is_empty() {
        actions = actions.push(btn_sm(format!("Install {version} and restart"), Btn::Primary, (!busy).then_some(Message::Install)));
    }
    actions = actions.push(super::link("Releases page", Message::OpenReleases));
    updates.push(actions.align_y(Alignment::Center).into());

    let offline_text = if state.offline > 0 {
        format!("{} song(s) saved on this computer.", state.offline)
    } else {
        "No songs saved on this computer yet.".to_string()
    };

    container(card(
        "This computer",
        vec![
            column![help("Settings of the Encore app on this computer. They stay on this computer and are not saved to your account.")]
                // why this computer is missing from Your devices, when it is
                .extend(crate::link::problem().map(|e| super::err_text(format!("Not in Your devices: {e}")).into()))
                .spacing(6)
                .into(),
            block(
                "Start",
                vec![
                    switch(c.autostart, "Start Encore when I sign in to this computer", Message::Autostart),
                    help(format!("It starts in the {tray} without a window. Click the icon to open it.")).into(),
                ],
            ),
            block(
                "Mini player",
                vec![
                    container(btn_sm("Open or close the mini player", Btn::Default, Some(Message::MiniPlayer))).into(),
                    help(format!("A small window that stays on top of the others, with the cover, the song, the buttons and the seek bar. Also in the {tray} menu and in the player's ⋯ menu.")).into(),
                ],
            ),
            block(
                "Keyboard shortcuts",
                vec![
                    switch(c.hotkeys.on, "Use these shortcuts in every program", Message::HotkeysOn),
                    keys.into(),
                    help("Click a box and press the keys you want, for example Ctrl+Alt+P. Backspace clears it. The media keys on your keyboard always work.").into(),
                    container(btn_sm("Back to the standard shortcuts", Btn::Default, Some(Message::ResetKeys))).into(),
                ],
            ),
            block(
                "Offline music",
                vec![
                    help(format!("{offline_text} Save songs, albums and playlists with ⋯ → Save for offline. When the server can't be reached, they play through Cadenza.")).into(),
                    container(btn_sm("Open offline music", Btn::Default, (state.offline > 0).then_some(Message::OpenOffline))).into(),
                ],
            ),
            block("Updates", updates),
        ],
    ))
    .max_width(820)
    .width(Length::Fill)
    .into()
}

pub fn overlay<'a>(_state: &'a State, _ctx: &Ctx<'a>) -> Option<El<'a, Message>> {
    None
}

/// While a shortcut box listens, every key press is reported; while the updater works, its
/// progress is drawn again twice a second.
pub fn subscription(state: &State) -> Subscription<Message> {
    let mut subs = Vec::new();
    if state.recording.is_some() {
        subs.push(iced::event::listen_with(|e, _status, _window| match e {
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) => Some(Message::Key(key, physical_key, modifiers)),
            _ => None,
        }));
    }
    if updater_busy(&updater::state()) {
        subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick));
    }
    Subscription::batch(subs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::{Code, Named, Physical};

    #[test]
    fn accelerators_read_like_the_web() {
        let ctrl_alt = Modifiers::CTRL | Modifiers::ALT;
        assert_eq!(accelerator(&Key::Character("p".into()), &Physical::Code(Code::KeyP), ctrl_alt).as_deref(), Some("Ctrl+Alt+P"));
        assert_eq!(
            accelerator(&Key::Named(Named::ArrowUp), &Physical::Code(Code::ArrowUp), Modifiers::CTRL | Modifiers::SHIFT).as_deref(),
            Some("Ctrl+Shift+Up")
        );
        assert_eq!(accelerator(&Key::Character("1".into()), &Physical::Code(Code::Digit1), Modifiers::ALT).as_deref(), Some("Alt+1"));
        assert_eq!(accelerator(&Key::Named(Named::F7), &Physical::Code(Code::F7), Modifiers::empty()).as_deref(), Some("F7"));
        assert_eq!(accelerator(&Key::Named(Named::PageUp), &Physical::Code(Code::PageUp), Modifiers::LOGO).as_deref(), Some("Super+PageUp"));
        assert_eq!(accelerator(&Key::Named(Named::Backspace), &Physical::Code(Code::Backspace), Modifiers::empty()).as_deref(), Some(""));
        assert_eq!(accelerator(&Key::Named(Named::Control), &Physical::Code(Code::ControlLeft), Modifiers::CTRL), None);
    }

    #[test]
    fn shortcuts_and_the_updater_states() {
        let mut h = Hotkeys::default();
        set_hotkey(&mut h, "next", "Ctrl+N".into());
        assert_eq!(hotkey(&h, "next"), "Ctrl+N");
        assert_eq!(KEYS.len(), 5);
        assert!(updater_busy(&updater::Phase::Checking));
        assert!(updater_busy(&updater::Phase::Installing("0.28.0".into())));
        assert!(!updater_busy(&updater::Phase::UpToDate));
        assert!(!updater_busy(&updater::Phase::Failed("no".into())));
    }
}
