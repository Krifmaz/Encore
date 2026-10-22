//! Encore desktop: the Encore web app, native, with Cadenza playing on this device.
//!
//! Flags: `--server <url>`, `--minimized`, `--screenshot-tour <dir> [--login user:pass]`,
//! `--help`. One instance runs at a time; a second launch brings the first window forward.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod config;
mod desktop;
mod engine;
mod fmt;
mod link;
mod pages;
mod player;
mod theme;
mod tour;
mod whats_new;
mod widgets;

use app::{App, Flags};

const HELP: &str = "encore-desktop [--server <url>] [--minimized] [--screenshot-tour <dir> [--login user:pass]]

  --server <url>          open this Encore server (remembered once you sign in)
  --minimized             start minimized
  --screenshot-tour <dir> sign in (with --login), open every page, save one PNG per page in <dir>, exit
  --login user:pass       the account the tour signs in with
";

fn parse_flags() -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut tour_dir: Option<std::path::PathBuf> = None;
    let mut login: Option<(String, String)> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => flags.server = Some(args.next().ok_or("--server needs a URL")?),
            "--minimized" => flags.minimized = true,
            "--screenshot-tour" => {
                tour_dir = Some(std::path::PathBuf::from(
                    args.next().ok_or("--screenshot-tour needs a directory")?,
                ))
            }
            "--login" => {
                let v = args.next().ok_or("--login needs user:pass")?;
                let (u, p) = v.split_once(':').ok_or("--login needs user:pass")?;
                login = Some((u.to_string(), p.to_string()));
            }
            "--help" | "-h" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("encore-desktop {}", api::VERSION);
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag {other}\n{HELP}")),
        }
    }
    flags.tour = tour_dir.map(|d| (d, login));
    Ok(flags)
}

fn main() -> iced::Result {
    env_logger::Builder::from_env(
        env_logger::Env::default()
            // iced_winit logs every window's attributes at info, the icon's pixels included
            .default_filter_or("info,iced_winit=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,iced_wgpu=warn,cosmic_text=warn,symphonia=warn,sctk=warn,calloop=warn,reqwest=warn,hyper=warn,zbus=warn"),
    )
    .format_timestamp_millis()
    .init();
    let flags = match parse_flags() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let touring = flags.tour.is_some();
    if touring {
        // The tour must not touch (or depend on) the person's own config.
        let dir = std::env::temp_dir().join(format!("encore-desktop-tour-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        config::set_dir(dir);
    }
    desktop::init();
    if !touring && !desktop::single::acquire() {
        log::info!("Encore is already running: brought its window forward");
        return Ok(());
    }
    log::info!("Encore desktop {} starting", api::VERSION);
    iced::daemon(move || App::boot(flags.clone()), App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .antialiasing(true)
        .run()
}
