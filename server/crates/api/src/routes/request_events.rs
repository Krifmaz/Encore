//! `GET /api/requests/events`: live request labels for the signed-in user's open pages. The events
//! themselves come from encore_requests::events.
use crate::auth::{self, User};
use encore_db::RowExt;
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;

/// How often an open stream checks that its session still exists and the user's role is unchanged.
const RECHECK: Duration = Duration::from_secs(30);

/// `GET /api/requests/events`: the user's own requests (an admin's: everyone's) as they change. The stream
/// ends when the session is revoked (logout everywhere) or the user is disabled, and stops sending other
/// users' requests once an admin is demoted.
pub async fn events(State(st): State<AppState>, headers: HeaderMap, User(u): User) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let me = u.i("id");
    let admin = encore_config::perms::is_admin(&u);
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + RECHECK, RECHECK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let all = stream::unfold((st.requests.events.subscribe(), tick, admin), move |(mut rx, mut tick, mut admin)| {
        let (st, headers) = (st.clone(), headers.clone());
        async move {
            loop {
                tokio::select! {
                    m = rx.recv() => match m {
                        Ok((owner, v)) if admin || owner == me => return Some((v, (rx, tick, admin))),
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    },
                    _ = tick.tick() => match auth::current_user(&st, &headers) {
                        Some(now_u) if now_u.i("id") == me => admin = encore_config::perms::is_admin(&now_u),
                        _ => return None,
                    },
                }
            }
        }
    });
    let first = stream::once(async { json!({ "type": "hello" }) });
    Sse::new(first.chain(all).map(|v| Ok(Event::default().event(v["type"].as_str().unwrap_or("message")).data(v.to_string()))))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
}

