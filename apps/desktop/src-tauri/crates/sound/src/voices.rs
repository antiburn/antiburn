//! The two voices, exactly as they were settled by ear in the lab.
//!
//! **These numbers are the output of a tuning session, not a design.** They were
//! arrived at by listening, and no individual value here has a justification beyond
//! "that is what sounded right". Changing one to a rounder number, or because it
//! looks like a typo, will change the sound. If a voice needs to change, change it
//! in `prototypes/sound-lab/lab.html`, listen, and paste the result back — that
//! loop is the entire reason the lab exists.

use crate::synth::{UNISON, Voice};

/// Warm and low, opening rather than striking. Used for ActorUpdate, Own-update ready and
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
