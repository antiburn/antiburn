// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The two voices, exactly as they were settled by ear in the lab.
//!
//! **These numbers are the output of a tuning session, not a design.** They were
//! arrived at by listening, and no individual value here has a justification beyond
//! "that is what sounded right". Changing one to a rounder number, or because it
//! looks like a typo, will change the sound. If a voice needs to change, change it
//! in `prototypes/sound-lab/lab.html`, listen, and paste the result back — that
//! loop is the entire reason the lab exists.

use crate::synth::{UNISON, Voice, Wave4};

/// Warm and low, opening rather than striking. Used for Radar, My Radar ready and
/// Tune up — the three that are informational rather than a problem.
///
/// The long 239 ms attack is what keeps it from reading as an alert: it arrives
/// gradually instead of hitting, so it registers without demanding that you stop.
pub const SOFT_UPDATE: Voice = Voice {
    dur: 0.83,
    attack: 0.239,
    release: 5.8,
    root: 196.0,
    chord: UNISON,
    wave: Wave4::Sine,
    detune: 8.1,
    detune_track: 0.0,
    cutoff: 4924.0,
    q: 1.0,
    sweep: 3.5,
    fm_amount: 219.0,
    fm_ratio: 2.7,
    drive: 0.09,
    sub_gain: 0.41,
    sub_att: 0.145,
    air_gain: 0.04,
    air_band: 8689.0,
    air_att: 0.19,
    level: 0.14,
};

/// **Not currently used.** Notification was going to be the harsh voice below, and
/// on a listen it wasn't wanted: it now plays the same warm arpeggio as everything
/// else, just on fixed notes. Kept rather than deleted because it represents real
/// tuning work and is the obvious starting point if a sound ever does need to read
/// as bad news. Delete it if that stops being plausible.
///
/// Short and hard: 400 ms with a fast attack and a filter that *closes*
/// (`sweep` below 1) rather than opening. Darkening as it decays is what gives it
/// the falling, deflating shape that reads as something being wrong.
#[allow(dead_code)]
///
/// Inharmonic FM at ratio 2.7 supplies the metallic edge — an integer ratio would
/// add harmonics that fold into the note and sound musical instead of urgent.
///
/// **Brightness is not calibrated against the lab.** The Rust render measures
/// 1708 Hz against the browser's 662, a 2.6x gap that three separate hypotheses
/// failed to explain (drive-stage oversampling, the FM carrier waveform, and the
/// filter settings — a 14x6 grid search could get no closer than 1294 Hz). Web
/// Audio's biquad and fundsp's state-variable filter simply do not interpret
/// resonance the same way. Left at the browser's own values pending a listen,
/// rather than tuned to a number nobody understands. Loudness *is* matched, via the
/// per-sound normalisation in `finish_to`.
pub const WARNING: Voice = Voice {
    dur: 0.4,
    attack: 0.035,
    release: 10.7,
    root: 1049.0,
    chord: UNISON,
    wave: Wave4::Triangle,
    detune: 2.0,
    detune_track: 0.0,
    cutoff: 530.0,
    q: 0.9,
    sweep: 0.5,
    fm_amount: 265.0,
    fm_ratio: 2.7,
    drive: 0.16,
    sub_gain: 0.12,
    sub_att: 0.323,
    air_gain: 0.1,
    air_band: 1650.0,
    air_att: 0.047,
    level: 0.14,
};
