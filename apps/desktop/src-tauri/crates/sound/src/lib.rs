//! `antiburn-sound` — programmatic notification sounds for the antiburn desktop app.
//!
//! This crate is the **mechanism**: it knows how to create the notification sound
//! and get that audio to the speaker. It
//! knows nothing about *why* a sound plays, about nudges, or about user
//! preferences — that is the app's **policy**, the same mechanism/policy split the
//! rest of the shell already uses.
//!
//! The seam is one call:
//!
//! ```no_run
//! # use antiburn_sound::SoundPlayer;
//! let player = SoundPlayer::new();
//! player.play();
//! ```
//!
//! The sound uses three fixed notes so it means the same thing on every machine.
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

use synth::SR;
use tuning::Tuning;

/// Render the notification sound to an interleaved stereo buffer.
fn render() -> Vec<f32> {
    let tuning = Tuning::default();
    let mut voice = voices::SOFT_UPDATE;
    voice.root *= 2.0_f32.powf(tuning.transpose / 12.0);
    voice.cutoff = (voice.cutoff * tuning.brightness.max(0.05)).clamp(60.0, 18_000.0);
    voice.detune_track = tuning.detune_track;

    let shape = chord::Chord {
        spread: (tuning.spread_ms.max(0.0) / 1000.0) as f64,
        high_tilt: tuning.high_tilt,
        descending: tuning.descending,
    };

    let wave = chord::render_degrees(&chord::NOTIFICATION_CHORD, &voice, shape);

    let mut out = Vec::with_capacity(wave.len() * 2);
    for i in 0..wave.len() {
        out.push(wave.at(0, i));
        out.push(wave.at(1, i));
    }
    out
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
    tx: Option<SyncSender<()>>,
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
        let (tx, rx) = sync_channel::<()>(4);

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
                    match &sink {
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

                    let samples = render();
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

    /// Returns immediately: rendering and playback both happen on the audio thread.
    /// Silently does nothing if audio is unavailable or a sound is already queued —
    /// see the type-level note on why there is no error to handle.
    pub fn play(&self) {
        let Some(tx) = &self.tx else {
            return;
        };
        // `try_send` rather than `send`: a full queue means sounds are already
        // backing up, and blocking here would stall whichever thread is trying to
        // show a notification.
        if tx.try_send(()).is_err() {
            tracing::debug!("dropped a sound; queue full or audio thread gone");
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
        let samples = render();
        let path = dir.join("notification.wav");
        let mut w = hound::WavWriter::create(&path, spec).expect("create wav");
        for s in &samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
                .expect("write");
        }
        w.finalize().expect("finalize");
        println!("wrote {}", path.display());
    }

    /// The notification must produce finite, audible stereo samples.
    #[test]
    fn notification_renders_something_audible() {
        let samples = render();
        assert_eq!(samples.len(), 112_762);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        let peak = samples.iter().fold(0.0f32, |a, sample| a.max(sample.abs()));
        assert!(peak > 0.01, "the notification is silent (peak {peak})");
        assert!(peak <= 1.0, "the notification clips (peak {peak})");
    }
}
