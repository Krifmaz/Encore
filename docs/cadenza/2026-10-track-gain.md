# Cadenza: a per-track gain field for Encore's volume normalization – brief for the Cadenza agent

Written Oct 20, 2026 in the Encore repo (docs/cadenza/2026-10-track-gain.md). It is the prompt for an agent working in
the **Cadenza repo** (github.com/Krifmaz/cadenza). Encore never edits that repo; the work below is done there, and
Encore then moves its pins (`cadenza` and `cadenza-dsp` in clients/desktop/Cargo.toml, `cadenza-dsp` in the server and
the phone) to the new rev. File names refer to the Cadenza tree at rev `17673b3` ("DSD on the Output page, native DSD
first with a fallback ladder; volume, filter families"), the rev the Encore desktop app pins today.

Rules: plain words everywhere (log lines, errors, JSON labels), no exclamation marks; no behaviour change for anything
not named here (config keys, `GET/PUT /api/settings`, the CLI, the endpoint protocol, the heartbeat keys Encore reads);
new keys are added, old ones are not renamed. Every item gets a test that runs without a DAC (the null output, files
made in the test). `cargo test`, `cargo clippy -- -D warnings` and `cargo fmt --check` stay clean.

## Why

Encore 0.31 normalizes loudness (ReplayGain / R128 tags, or Encore's own EBU R128 measurement). The server sends a
`gain` object on every track row: `{track, album, track_peak, album_peak}` (dB relative to −18 LUFS, linear peaks). The
web and the phone apply it as a gain stage per song. The desktop app plays through the embedded Cadenza, which decodes
the stream itself, so the app cannot scale samples before the engine. The pinned rev has no per-track gain: `Track`
(crates/cadenza/src/engine/mod.rs) has no gain field and `Command` has no gain command. Section 4.L of the 0.27 brief
(docs/cadenza/2026-10-engine.md) asked for one; it is not in `17673b3`.

What Encore does meanwhile (clients/desktop/src/engine/normalize.rs): it sends `Command::SetVolume(fader dB + gain dB)`
when `Event::TrackStarted` arrives. That has three flaws only the engine can fix:

1. The first blocks of each song are processed with the previous song's gain (the event, the round trip, the next
   pump), and on a gapless transition the jump is audible as a step in level.
2. The heartbeat's `volume` (and the UPnP / OpenHome volume) then reports the fader with the gain in it, so another
   device's volume slider moves with every song; Encore has to tell its own `SetVolume` from another device's.
3. DSD pass-through (`Worker::passthrough_target`) decides "untouched" from the volume of the song before.

## What to build

### 1. `Track.gain_db`

`engine::Track` gets `gain_db: Option<f64>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`). Encore
fills it when it builds a queue entry (desktop: `engine::tracks::from_encore`); for the songs the link builds from
another device's rows, the app's hook of section 3 gives it.

- Applied in the volume stage as an extra factor (dB added to the volume's dB), as part of the track: it takes effect
  with the first sample of the track, including on a gapless transition and on a prefetched decoder.
- It does not change `Status.volume` / `volume_linear`, the heartbeat's `volume`, or the persisted `volume_db`: those
  stay the fader.
- A non-zero gain counts as touching the samples: DSD pass-through is off for that track (`passthrough_target`'s
  `untouched` check includes it), and the signal path is not bit-perfect.
- In bit-perfect mode (`dsp.bit_perfect`) the gain is ignored; the `volume` stage detail says "track gain ignored
  (bit-perfect)".
- Clamped to −30 … +15 dB (out-of-range values are clamped and logged once per track).
- The `volume` stage detail reads "−5.2 dB (track −7.2 dB · volume +2.0 dB)" while a gain is applied.
- `cadenza-dsp`: keep `cadenza_dsp::volume::Volume` as it is (the Encore server and the phone build it); add the gain as
  a second field of the stage or a separate stage. Say in the report what changed in `cadenza-dsp`.

### 2. `Command::SetTrackGain { index: Option<usize>, gain_db: Option<f64> }`

Changes the gain of a queue entry (`None` = the playing one) without a rebuild: Encore's normalization settings change
while a song plays (Off / Track / Album, the target level). For the playing entry the new gain is ramped over 50 ms.

### 3. Hooks for the embedding app (the link's tracks, commands and state)

Songs another device sends to the desktop arrive through Cadenza's own Encore link (`encore::commands::run`,
`track_from_encore`), which drops the row's `gain` object; the desktop's normalization settings (Off / Track / Album,
the target, Prevent clipping) live in the app, not in the engine. So:

- `EmbeddedPlayer::set_track_gain(f: impl Fn(&serde_json::Value) -> Option<f64> + Send + Sync + 'static)`: the link
  calls it with each Encore row it turns into a `Track` and puts the answer in `Track.gain_db`.
- `EmbeddedPlayer::set_command_handler(f: impl Fn(&serde_json::Value) -> bool + Send + Sync + 'static)`: the link
  offers every command to it first and runs its own only when it answers false. Encore 0.31's web sends
  `{"action": "prefs", "norm": {"mode", "target", "clip"}}` (client-shapes §5.1), which the desktop then handles.
- `EmbeddedPlayer::set_state_extra(f: impl Fn() -> serde_json::Map<String, serde_json::Value> + Send + Sync + 'static)`:
  merged into the heartbeat state at the top level (`norm`), and `signal` gains the key `norm` when the map has
  `signal_norm` (the Signal path line, "−3.2 dB (album gain, target −14 LUFS)").

### 4. Tests

- A WAV through the null output with `gain_db = -6.0206` comes out at half amplitude; with bit-perfect on, unchanged.
- Two tracks gapless with gains −6 dB and 0 dB: the first sample of the second track is at full level (no step inside
  either track).
- `Status.volume` is the same before and after a track with a gain starts.
- DSD pass-through is refused for a track with a gain and allowed for the next one without.
- `SetTrackGain` on the playing entry changes the level within 50 ms and does not rebuild the chain.
- A `play_queue` command through the link with a row carrying `gain` gets the hook's `gain_db`; a `prefs` command
  reaches the command handler and nothing else; the heartbeat carries the extra state.

## Report back

The rev, the new `Track` field, command and hooks, what changed in `cadenza-dsp`, and anything that could only be tried
on the hardware. Encore then moves the pins and replaces the `SetVolume` path in clients/desktop/src/engine/normalize.rs
with `Track.gain_db` (and `SetTrackGain` when the settings change), and answers `prefs` and reports `norm` as a linked
device.
