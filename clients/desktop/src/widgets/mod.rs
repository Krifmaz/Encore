//! The building blocks every page uses, styled like the web app's components. Page agents add
//! `pub mod <theirs>;` lines here and a `widgets/<theirs>.rs` file; everything else in this
//! file belongs to the architect and the integrator.

pub mod app_icons;
pub mod catalog;
pub mod centered_bar;
pub mod choice;
pub mod cover;
pub mod details;
pub mod empty;
pub mod eq_graph;
pub mod list_row;
pub mod marquee;
pub mod more_like;
pub mod picker;
pub mod pop_menu;
pub mod request_list;
pub mod search_box;
pub mod sidebar;
pub mod stat;
pub mod tile;
pub mod toast;
pub mod track_row;

// The kit's building blocks (kit_iced::widgets), under the names the pages use.
pub use kit_iced::drop_zone;
pub use kit_iced::widgets::{badge, icons, modal, skeleton, tabs, text_area};

// The re-exports are the pages' toolbox.

pub use cover::Covers;
pub use empty::empty;
pub use app_icons::logo;
pub use icons::{icon, Icon};
pub use list_row::list_row;
pub use modal::confirm;
pub use pop_menu::{pop_menu, MenuItem};
pub use search_box::{search_box, search_box_clearable};
pub use skeleton::{skeleton_rows, skeleton_tiles};
pub use tabs::{seg, tabs_maybe};
pub use tile::{grid, rail, Tile, TileKind};
pub use toast::ToastKind;
pub use track_row::TrackRow;

pub use kit_iced::widgets::{
    avatar, btn, btn_icon, btn_sm, card, card_flat, divider, error_box, faint, field, h1, h2, h3, icon_btn, muted, pad, page_head,
    section_head, small, small_muted, t, tip_box, El,
};
