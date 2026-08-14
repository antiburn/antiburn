// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `antiburn-sound` — programmatic notification sounds for the antiburn desktop app.
//!
//! This crate is the **mechanism**: it knows how to turn a [`SoundKind`] and an
//! optional person's name into audio, and how to get that audio to the speaker. It
//! knows nothing about *why* a sound plays, about nudges, or about user
//! preferences — that is the app's **policy**, the same mechanism/policy split the
//! rest of the shell already uses.
//!
//! The seam is one call:
//!
//! ```no_run
//! # use antiburn_sound::{SoundPlayer, SoundKind};
//! let player = SoundPlayer::new();
//! player.play(SoundKind::Radar, Some("marty"));   // marty's three notes
//! player.play(SoundKind::Notification, None);     // the same notes for everybody
//! ```
//!
//! Every sound is three notes stepped through quickly. What varies is *which*
//! three: a person's name picks them for the Radar sounds, while Notification
//! plays a fixed set so it means the same thing on every machine.
//!
//! ## Two rules this crate holds itself to
//!
//! **A sound must never break a notification.** No speaker, no audio device, a
//! machine with its output muted at the OS level — every one of those logs and
//! returns. There is no error type here on purpose: the caller has nothing useful
//! to do with an audio failure, and a missing sound must never be the reason a nudge
//! doesn't appear on screen.
//!
//! **Nothing renders on the caller's thread.** Building a sound takes roughly 20 ms.
//! That is small but not free, and the nudge path already warns that window work
//! marshals onto the main thread. [`SoundPlayer::play`] hands the request to a
//! dedicated audio thread and returns immediately.

mod chord;
mod synth;
mod tuning;
mod voices;

use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::time::Duration;

use rodio::buffer::SamplesBuffer;

pub use chord::{DEFAULT_CHORD, chord_for, semis, stable_hash};
pub use synth::SR;
pub use tuning::Tuning;

/// What happened. One per sound the app can make.
///
/// Deliberately *not* a mirror of `NudgeKind`: several nudge kinds can share a
/// sound, and the mapping between them is policy that belongs in the app. Keeping
/// this list short is the point — a person can learn three sounds, not eight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    /// A teammate shared a Radar update relevant to you. Their name picks the chord.
    Radar,
    /// Your own Radar draft is ready to share. Your name picks the chord.
    MyRadarReady,
    /// Something needs attention. The same for everybody — see
    /// [`chord::NOTIFICATION_CHORD`].
    Notification,
}

impl SoundKind {
    /// Whether a name changes what this sound plays.
    ///
    /// [`SoundKind::Notification`] is about a session rather than a person, so it
    /// ignores any name passed to it — worth checking before going to the trouble of
    /// looking one up.
    pub fn uses_actor(self) -> bool {
        !matches!(self, SoundKind::Notification)
    }
}

/// Render one sound to an interleaved stereo buffer.
///
/// `actor` is whoever caused it. Their name picks the three notes, so the same
/// person always sounds the same on every machine. `None` falls back to the voice
/// at its own root — a reserved unison meaning "nobody in particular".
///
/// [`SoundKind::Notification`] ignores `actor` entirely and plays a fixed set of
/// notes, because it is about a session rather than a person and has to mean the
/// same thing to everybody who hears it.
pub fn render(kind: SoundKind, actor: Option<&str>) -> Vec<f32> {
    render_tuned(kind, actor, Tuning::default_for(kind))
}

/// Render with tuning other than the settled values.
///
/// Exists for the dev sound bench, so the knobs can be turned while listening
/// instead of edited and rebuilt. Whatever settings win there get written into
/// [`Tuning::default_for`], which is the record of what was decided by ear.
pub fn render_tuned(kind: SoundKind, actor: Option<&str>, tuning: Tuning) -> Vec<f32> {
    let mut voice = voices::SOFT_UPDATE;
    voice.root *= 2.0_f32.powf(tuning.transpose / 12.0);
    voice.cutoff = (voice.cutoff * tuning.brightness.max(0.05)).clamp(60.0, 18_000.0);
    voice.detune_track = tuning.detune_track;

    let shape = chord::Chord {
        spread: (tuning.spread_ms.max(0.0) / 1000.0) as f64,
        high_tilt: tuning.high_tilt,
        descending: tuning.descending,
        ..chord::DEFAULT_CHORD
    };

    let named = actor.map(str::trim).filter(|a| !a.is_empty());
    let wave = match (kind, named) {
        // The same three notes for everybody, whoever is passed in.
        (SoundKind::Notification, _) => {
            chord::render_degrees(&chord::NOTIFICATION_CHORD, &voice, shape)
        }
        (_, Some(name)) => chord::render_chord(name, &voice, shape),
        // No name to work with: the reserved unison, meaning nobody in particular.
        (_, None) => synth::render_voice(&voice),
    };

    let mut out = Vec::with_capacity(wave.len() * 2);
    for i in 0..wave.len() {
        out.push(wave.at(0, i));
        out.push(wave.at(1, i));
    }
    out
}

/// A request handed to the audio thread.
struct Request {
    kind: SoundKind,
    actor: Option<String>,
    /// `None` means the settled tuning for this sound. Only the dev bench sends
    /// anything else.
    tuning: Option<Tuning>,
}

/// How long the output device stays open after the last sound.
///
/// The device is opened on demand and released once things go quiet, rather than
/// held for the life of the app. rodio starts the output stream the moment the
/// sink opens and never pauses it, so a sink kept forever means an audio callback
/// firing tens of times a second, for the whole time the app is running, mixing
/// silence. On a menu-bar app that is open all day that is a battery cost for
/// nothing, it keeps the audio hardware from idling, and on Linux it holds the
/// default ALSA device against anything that wants it exclusively.
///
/// Comfortably longer than the longest sound (~1.2 s). Handing a buffer to the
/// mixer returns before it has finished playing, so closing too eagerly would cut
/// the sound off mid-way.
///
/// The cost is that a sound arriving after a quiet spell pays for opening the
/// device first — tens of milliseconds. Against a notification window that is
/// itself being drawn, that is not a delay anybody can hear, and it buys back
/// every idle wakeup in between.
const IDLE_CLOSE: Duration = Duration::from_secs(5);

/// Owns the audio thread and, through it, the output device.
///
/// Create one at app setup and keep it in Tauri managed state. Dropping it closes
/// the audio thread and releases the device.
pub struct SoundPlayer {
    tx: Option<SyncSender<Request>>,
}

impl SoundPlayer {
    /// Start the audio thread. Never fails — if the device can't be opened, that is
    /// discovered on the audio thread and logged there, because a machine with no
    /// working speaker must still run the app.
    ///
    /// Starting the thread does **not** open the device; see [`IDLE_CLOSE`].
    pub fn new() -> Self {
        // Bounded, and deliberately shallow. If sounds are arriving faster than they
        // can play, the right behaviour is to drop the newest rather than queue a
        // backlog that plays long after whatever caused it left the screen.
        let (tx, rx) = sync_channel::<Request>(4);

        let spawned = std::thread::Builder::new()
            .name("antiburn-sound".into())
            .spawn(move || {
                // Held across requests so a burst doesn't reopen the device each
                // time, and dropped when things go quiet. rodio stops playing the
                // moment this handle drops, which is why closing waits out
                // `IDLE_CLOSE` rather than happening after each sound.
                let mut sink = None;
                // A machine with no output device is a normal machine, not a broken
                // install, so this is said once rather than on every sound. Reset on
                // success, because the reason can go away — plugging in headphones
                // or starting a sound server mid-session.
                let mut reported_no_device = false;

                loop {
                    // With nothing open there is nothing to time out for, so wait
                    // indefinitely: an idle app should cost no wakeups at all, which
                    // is the whole point of closing the device in the first place.
                    let request = match &sink {
                        None => match rx.recv() {
                            Ok(request) => request,
                            Err(_) => return,
                        },
                        Some(_) => match rx.recv_timeout(IDLE_CLOSE) {
                            Ok(request) => request,
                            // Quiet for a while: let the device go.
                            Err(RecvTimeoutError::Timeout) => {
                                sink = None;
                                continue;
                            }
                            // The player was dropped; the app is going away.
                            Err(RecvTimeoutError::Disconnected) => return,
                        },
                    };

                    if sink.is_none() {
                        match rodio::DeviceSinkBuilder::open_default_sink() {
                            Ok(opened) => {
                                sink = Some(opened);
                                reported_no_device = false;
                            }
                            Err(error) => {
                                if !reported_no_device {
                                    reported_no_device = true;
                                    tracing::info!(%error, "no audio output device; sounds disabled");
                                }
                                continue;
                            }
                        }
                    }
                    let Some(sink) = &sink else {
                        continue;
                    };

                    let Request {
                        kind,
                        actor,
                        tuning,
                    } = request;
                    let tuning = tuning.unwrap_or_else(|| Tuning::default_for(kind));
                    let samples = render_tuned(kind, actor.as_deref(), tuning);
                    let Some(channels) = std::num::NonZero::new(2u16) else {
                        continue;
                    };
                    let Some(rate) = std::num::NonZero::new(SR as u32) else {
                        continue;
                    };
                    sink.mixer()
                        .add(SamplesBuffer::new(channels, rate, samples));
                }
            });

        match spawned {
            Ok(_) => Self { tx: Some(tx) },
            Err(error) => {
                tracing::warn!(%error, "could not start the audio thread; sounds disabled");
                Self { tx: None }
            }
        }
    }

    /// Play `kind`, optionally in `actor`'s voice.
    ///
    /// Returns immediately: rendering and playback both happen on the audio thread.
    /// Silently does nothing if audio is unavailable or a sound is already queued —
    /// see the type-level note on why there is no error to handle.
    pub fn play(&self, kind: SoundKind, actor: Option<&str>) {
        self.play_tuned(kind, actor, None);
    }

    /// Play with tuning other than the settled values. For the dev bench.
    pub fn play_tuned(&self, kind: SoundKind, actor: Option<&str>, tuning: Option<Tuning>) {
        let Some(tx) = &self.tx else {
            return;
        };
        let request = Request {
            kind,
            actor: actor.map(str::to_owned),
            tuning,
        };
        // `try_send` rather than `send`: a full queue means sounds are already
        // backing up, and blocking here would stall whichever thread is trying to
        // show a notification.
        if tx.try_send(request).is_err() {
            tracing::debug!(?kind, "dropped a sound; queue full or audio thread gone");
        }
    }
}

impl Default for SoundPlayer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write every sound to `/tmp/antiburn-sounds/` so a render can be listened to
    /// and measured outside the app.
    ///
    /// `#[ignore]` because it writes files; run it deliberately:
    /// `cargo test -p antiburn-sound -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_sounds_for_listening() {
        let dir = std::path::Path::new("/tmp/antiburn-sounds");
        std::fs::create_dir_all(dir).expect("create dump dir");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: SR as u32,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        for (kind, name) in [
            (SoundKind::Radar, "radar"),
            (SoundKind::MyRadarReady, "my-radar-ready"),
            (SoundKind::Notification, "notification"),
        ] {
            for actor in [Some("keith"), Some("Marty"), None] {
                let samples = render(kind, actor);
                let label = actor.unwrap_or("nobody");
                let path = dir.join(format!("{name}--{label}.wav"));
                let mut w = hound::WavWriter::create(&path, spec).expect("create wav");
                for s in &samples {
                    w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
                        .expect("write");
                }
                w.finalize().expect("finalize");
                println!("wrote {}", path.display());
            }
        }
    }

    /// Every sound must produce audio, with and without a name. This is the check
    /// that a voice or chord constant has not been left in a state that renders
    /// silence — which no compiler catches.
    #[test]
    fn every_sound_renders_something_audible() {
        for kind in [
            SoundKind::Radar,
            SoundKind::MyRadarReady,
            SoundKind::Notification,
        ] {
            for actor in [Some("keith"), None] {
                let samples = render(kind, actor);
                assert!(!samples.is_empty(), "{kind:?} / {actor:?} rendered nothing");
                let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
                assert!(peak > 0.01, "{kind:?} / {actor:?} is silent (peak {peak})");
                assert!(peak <= 1.0, "{kind:?} / {actor:?} clips (peak {peak})");
            }
        }
    }

    /// Two different people must not get the same audio out of the same sound,
    /// otherwise the whole per-person idea is decoration.
    #[test]
    fn different_people_sound_different() {
        let a = render(SoundKind::Radar, Some("keith"));
        let b = render(SoundKind::Radar, Some("Marty"));
        assert_ne!(a, b);
    }

    /// A blank or whitespace name is treated as no name rather than hashed as one,
    /// so an empty string from a payload can't invent a bogus chord.
    #[test]
    fn blank_actors_fall_back_to_the_unison() {
        let nobody = render(SoundKind::Radar, None);
        for blank in ["", "   "] {
            assert_eq!(render(SoundKind::Radar, Some(blank)), nobody, "{blank:?}");
        }
    }

    /// Notification is about a session, not a person, so a name must not change it.
    #[test]
    fn notification_ignores_the_actor() {
        let plain = render(SoundKind::Notification, None);
        assert_eq!(render(SoundKind::Notification, Some("keith")), plain);
        assert!(!SoundKind::Notification.uses_actor());
    }

    /// Notification plays the same three notes for everybody, so it is the one sound
    /// a person's chord can collide with. Whoever hashes to those degrees hears their
    /// own Radar and every Notification as the same sound.
    ///
    /// That is one name in 255 and accepted rather than designed out — removing the
    /// chord from the per-person pool would renumber every other person's, and the
    /// lab would have to be changed in lockstep to keep matching. This test records
    /// the exposure and fails loudly if the fixed chord is ever changed to one that
    /// somebody on the current roster already owns.
    #[test]
    fn notification_does_not_collide_with_a_known_teammate() {
        for name in ["keith", "Marty", "chris", "jordan", "sam", "alex"] {
            assert_ne!(
                chord_for(name, DEFAULT_CHORD),
                chord::NOTIFICATION_CHORD.to_vec(),
                "{name} would hear their own Radar as every Notification"
            );
        }
    }

    /// Where the fixed Notification chord came from: it is what `"dave"` hashes to,
    /// picked by ear. Held as literal degrees so the sound cannot drift, with this
    /// test as the record of its provenance — if the hash ever changes, this fails
    /// and says so rather than the sound silently becoming somebody else's.
    #[test]
    fn notification_chord_is_daves() {
        assert_eq!(
            chord_for("dave", DEFAULT_CHORD),
            chord::NOTIFICATION_CHORD.to_vec()
        );
    }
}
