//! What a long step is doing right now, in words, for whoever shows it. The request worker runs an import (or
//! an album lookup) inside [`with_sink`] and writes what it is told into the request's message ("Importing –
//! checking the audio", "Importing – waiting for MusicBrainz (about 2 minutes, 120 lookups ahead)"); the
//! code doing the work only says what it is at ([`step`]) and what it waits for ([`waiting`]). Outside a
//! sink (a scan, a page, a test) every call here does nothing.
//!
//! The sink is a tokio task-local: it follows the future it was given, never a task spawned from it.
use std::future::Future;
use std::sync::{Arc, Mutex};

/// A function that shows a line of progress to whoever waits.
type Say = dyn Fn(&str) + Send + Sync;

/// Where the words go, and the step they are at.
#[derive(Clone)]
struct Sink {
    /// shows a line
    say: Arc<Say>,
    /// the step the work is at now
    step: Arc<Mutex<String>>,
}

tokio::task_local! {
    static SINK: Sink;
}

/// Run `f` with its steps and waits handed to `say` (the step's words; "" when it is at no named step).
pub async fn with_sink<F: Future>(say: impl Fn(&str) + Send + Sync + 'static, f: F) -> F::Output {
    SINK.scope(Sink { say: Arc::new(say), step: Arc::default() }, f).await
}

/// Whether anyone listens (the MusicBrainz line only counts the callers ahead when someone does).
pub fn active() -> bool {
    SINK.try_with(|_| ()).is_ok()
}

/// The work is at this step now ("checking the audio"): said at once, and again after each wait.
pub fn step(words: &str) {
    let _ = SINK.try_with(|s| {
        *s.step.lock().unwrap_or_else(|p| p.into_inner()) = words.to_string();
        (s.say)(words);
    });
}

/// The work waits for something ("waiting for MusicBrainz (under a minute, 12 lookups ahead)"); the step
/// stays, and [`resume`] says it again when the wait is over.
pub fn waiting(words: &str) {
    let _ = SINK.try_with(|s| (s.say)(words));
}

/// The wait is over: the step's words again.
pub fn resume() {
    let _ = SINK.try_with(|s| {
        let step = s.step.lock().unwrap_or_else(|p| p.into_inner()).clone();
        (s.say)(&step);
    });
}

/// What work done off the step's task (a copy on the blocking pool, where the sink doesn't follow) calls as
/// it moves: each call says the step's words again – nothing changes in the message, but whoever watches the
/// step hears it is moving (an import that says nothing for 15 minutes counts as stalled). Taken inside the
/// sink, callable from any thread; outside a sink it does nothing.
pub fn beat() -> impl Fn() + Send + Sync + 'static {
    let sink = SINK.try_with(Sink::clone).ok();
    move || {
        if let Some(s) = &sink {
            let step = s.step.lock().unwrap_or_else(|p| p.into_inner()).clone();
            (s.say)(&step);
        }
    }
}

/// How long a wait of `secs` is, in round words: "under a minute", "about a minute", "about 12 minutes",
/// "about an hour", "about 3 hours".
pub fn about(secs: f64) -> String {
    let mins = (secs / 60.0).round() as u64;
    match secs {
        s if s < 60.0 => "under a minute".into(),
        s if s < 90.0 => "about a minute".into(),
        s if s < 3600.0 - 30.0 => format!("about {mins} minutes"),
        s if s < 5400.0 => "about an hour".into(),
        s => format!("about {} hours", (s / 3600.0).round() as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn steps_and_waits_reach_the_sink_and_nothing_happens_outside_one() {
        // outside a sink: nothing to say it to, and nothing breaks
        step("checking the audio");
        waiting("waiting for MusicBrainz");
        resume();
        assert!(!active());
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = said.clone();
        with_sink(move |w| s.lock().unwrap().push(w.to_string()), async {
            assert!(active());
            resume(); // no step yet: ""
            step("checking the audio");
            waiting("waiting for MusicBrainz (under a minute, 3 lookups ahead)");
            resume();
            step("moving into the library");
        }).await;
        assert_eq!(*said.lock().unwrap(), ["", "checking the audio", "waiting for MusicBrainz (under a minute, 3 lookups ahead)", "checking the audio", "moving into the library"]);
        // a task spawned inside a sink doesn't inherit it
        let s = said.clone();
        with_sink(move |w| s.lock().unwrap().push(w.to_string()), async {
            tokio::spawn(async { step("elsewhere") }).await.unwrap();
        }).await;
        assert_eq!(said.lock().unwrap().len(), 5);
    }

    /// A beat taken inside a sink says the step's words again from another thread; one taken outside does
    /// nothing.
    #[tokio::test]
    async fn a_beat_says_the_step_again_from_another_thread() {
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = said.clone();
        with_sink(move |w| s.lock().unwrap().push(w.to_string()), async {
            step("moving into the library");
            let b = beat();
            tokio::task::spawn_blocking(move || { b(); b(); }).await.unwrap();
        }).await;
        assert_eq!(*said.lock().unwrap(), ["moving into the library"; 3]);
        let b = beat();
        b();
        assert_eq!(said.lock().unwrap().len(), 3);
    }

    #[test]
    fn waits_read_in_round_words() {
        assert_eq!(about(5.0), "under a minute");
        assert_eq!(about(59.0), "under a minute");
        assert_eq!(about(75.0), "about a minute");
        assert_eq!(about(120.0), "about 2 minutes");
        assert_eq!(about(130.0), "about 2 minutes");
        assert_eq!(about(59.0 * 60.0), "about 59 minutes");
        assert_eq!(about(3590.0), "about an hour");
        assert_eq!(about(3600.0), "about an hour");
        assert_eq!(about(3.0 * 3600.0), "about 3 hours");
    }
}
