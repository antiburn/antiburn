// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Turning a person's name into the notes you hear.
//!
//! Single pitches don't work: an earlier test put fourteen people into a 30-slot
//! space and got eleven distinct sounds, and enlarging the space doesn't help
//! because people can only hold five to nine absolute pitches. Several notes escape
//! that limit, because a listener identifies a short figure by its **shape** rather
//! than its absolute pitches — which is why ringtones are recognisable and single
//! beeps are not.

use crate::synth::{UNISON, Voice, finish_to, foundation_parts, note_gain, note_part};
use fundsp::prelude32::Wave;

/// Major pentatonic degrees within one octave. Positions past the fourth wrap into
/// successive octaves, so the scale runs as far as [`Chord::octaves`] allows and no
/// interval among the notes can clash. Every chord is consonant by construction,
/// whatever the hash produces.
pub const PENT: [f32; 5] = [0.0, 2.0, 4.0, 7.0, 9.0];

/// Scale position to semitones above the root.
pub fn semis(i: usize) -> f32 {
    PENT[i % 5] + 12.0 * (i / 5) as f32
}

/// FNV-1a, hand-rolled on purpose. `DefaultHasher` is stable across neither
/// platforms nor releases, and a person has to sound the same on every machine.
///
/// Both constants are written to a full 16 hex digits. Grouped any shorter, the
/// prime is one keystroke away from `0x1000_0000_01b3`, which is a different number
/// with an extra zero — it still scatters, so nothing looks broken until it is
/// compared against another implementation. That is exactly how it was caught.
pub fn stable_hash(s: &str) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET_BASIS;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// How the person axis is laid out.
///
/// `octaves` says where chords may sit; `max_span` says how wide any one of them
/// gets. Those are separate on purpose — a generous range gives variety in register
/// while a tight span keeps each individual chord from sounding hollow.
#[derive(Clone, Copy)]
pub struct Chord {
    pub notes: usize,
    pub octaves: usize,
    /// Fewest scale steps between adjacent notes.
    pub min_gap: usize,
    /// Most scale steps from a chord's lowest note to its highest.
    pub max_span: usize,
    /// Force the lowest note into the first octave.
    pub anchor_low: bool,
    /// Seconds between note onsets. Zero plays them together as a chord.
    pub spread: f64,
    /// Gain per octave above the lowest note. 1 leaves the chord untilted.
    pub high_tilt: f32,
    /// Play the notes highest-first.
    pub descending: bool,
}

/// Three notes played together, over three octaves.
///
/// `min_gap: 2` is the tightest spacing that still rings. At 1 the hash is free to
/// place two notes a whole tone apart, and high up that warbles like a trill rather
/// than sounding like a chord — which is exactly what it did.
///
/// `max_span: 12` does double duty. It stops the opposite failure, three notes
/// legally spaced but strung so far apart the middle falls out. And against a
/// 15-position scale it also pins the register for free: a 12-wide chord cannot
/// start above the third position, so every chord ends up with a low note under it
/// without needing `anchor_low`.
///
/// The constraints cost address space: 455 chords unconstrained against 255 here.
/// That takes the chance of two people in eight colliding from 6% to 10%, which is
/// the price of every chord being one you would choose to hear.
pub const DEFAULT_CHORD: Chord = Chord {
    notes: 3,
    octaves: 3,
    min_gap: 2,
    max_span: 12,
    anchor_low: false,
    spread: 0.0,
    high_tilt: 1.0,
    descending: false,
};

/// The chord every Notification plays, for everybody.
///
/// These are the degrees `"dave"` hashes to, picked by ear from the per-person
/// renders. Held as literal degrees rather than by hashing the name, because a
/// sound every user hears must not depend on the spelling of somebody's login, nor
/// change if the hash or the chord constraints ever do. `notification_chord_is_daves`
/// records where it came from.
pub const NOTIFICATION_CHORD: [usize; 3] = [1, 4, 9];

/// Every chord the constraints allow, ascending.
///
/// Enumerating and then indexing beats picking notes from the hash and repairing
/// the result. Free picking let three notes clump at the top of the range a whole
/// tone apart; repairing that afterwards biases the outcome in ways that are hard
/// to reason about. Here a chord is either legal or absent, and the length of this
/// list is the exact address space rather than an estimate.
///
/// At most C(20,4) = 4845 candidates, so brute force costs nothing.
pub fn valid_chords(c: Chord) -> Vec<Vec<usize>> {
    let span = c.octaves * 5;
    let n = Ord::min(c.notes, span);
    let gap = Ord::max(1, c.min_gap);
    let mut out = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn walk(
        start: usize,
        acc: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
        span: usize,
        n: usize,
        gap: usize,
        width: usize,
        anchor: bool,
    ) {
        if acc.len() == n {
            out.push(acc.clone());
            return;
        }
        let mut i = start;
        while i < span {
            if acc.is_empty() && anchor && i >= 5 {
                break;
            }
            if let Some(&lowest) = acc.first()
                && i - lowest > width
            {
                break;
            }
            acc.push(i);
            walk(i + gap, acc, out, span, n, gap, width, anchor);
            acc.pop();
            i += 1;
        }
    }
    walk(
        0,
        &mut Vec::new(),
        &mut out,
        span,
        n,
        gap,
        c.max_span,
        c.anchor_low,
    );
    out
}

/// One chord per person, drawn from the legal set.
pub fn chord_for(id: &str, c: Chord) -> Vec<usize> {
    let list = valid_chords(c);
    if list.is_empty() {
        return (0..c.notes).collect();
    }
    list[(stable_hash(id) % list.len() as u64) as usize].clone()
}

/// Render a person's chord in one of the voices.
pub fn render_chord(id: &str, base: &Voice, c: Chord) -> Wave {
    render_degrees(&chord_for(id, c), base, c)
}

/// Render specific scale degrees, rather than whichever ones a name hashes to.
///
/// Used for the sounds that must be the same for everybody. A fixed sound holds
/// its degrees directly instead of hashing a chosen name, so it can never drift if
/// the hash or the chord constraints change — a sound every user hears should not
/// depend on the spelling of somebody's login.
///
/// At `spread` zero every note starts together, so the result is one gesture of the
/// same length however many notes it holds. Above roughly 60 ms it becomes an
/// arpeggio instead: more distinguishable, because order starts carrying
/// information, at the cost of taking longer to play.
pub fn render_degrees(degrees: &[usize], base: &Voice, c: Chord) -> Wave {
    let together = c.spread == 0.0;
    let n = degrees.len();
    // Arpeggio notes are shortened so they do not smear into each other; a chord
    // has nothing to smear into and keeps its full length.
    let note_len = if together { base.dur } else { base.dur * 0.62 };

    let mut note_voice = *base;
    note_voice.chord = UNISON;
    note_voice.dur = note_len;

    // `degrees` is always ascending, so the lowest note is the reference the tilt
    // measures from whichever direction the phrase is played in.
    let lowest = base.root * 2.0_f32.powf(semis(degrees[0]) / 12.0);

    // Descending reverses the order notes are *played* in, not which notes they are.
    // The chord is the person's identity; the direction is the sound's character.
    let order: Vec<usize> = if c.descending {
        (0..n).rev().collect()
    } else {
        (0..n).collect()
    };

    let mut parts = Vec::new();
    for (step, &idx) in order.iter().enumerate() {
        let freq = base.root * 2.0_f32.powf(semis(degrees[idx]) / 12.0);
        // Pull back the top of the chord. These chords span three octaves, and a
        // note two octaves up reads far louder than the same gain does down low —
        // it is what makes a chord sound shrill rather than what gives it identity.
        let octaves_up = if lowest > 1.0 {
            (freq / lowest).log2()
        } else {
            0.0
        };
        let tilt = c.high_tilt.clamp(0.05, 1.0).powf(octaves_up);
        // Later notes of an arpeggio back off slightly so the phrase leans forward.
        // A chord has no order, so nothing to lean.
        let lean = if together {
            1.0
        } else {
            1.0 - step as f32 * 0.12
        };
        parts.push((
            note_part(&note_voice, freq, note_gain(n, idx) * tilt),
            step as f64 * c.spread,
            lean,
        ));
    }

    // One sub and one air layer for the whole chord, pitched an octave below the
    // voice's own root rather than below any particular note — matching the lab,
    // which builds these outside its note loop at `root / 2`. Every chord is drawn
    // from a pentatonic scale on that root, so the root's octave is the correct
    // bass under all of them.
    //
    // Giving every note its own sub instead put six tones in a three-note chord and
    // made the two loudest partials both subs, burying the harmony under its own
    // bass — which is what "it sounds like a unison" turned out to mean.
    //
    // Divided by the note count for the same reason the notes are. A voice's
    // tone-to-bass balance was set by ear on a single note; spreading that same
    // tone across three notes without touching the sub leaves the harmony at a
    // third of its weight against an unchanged bass, and the bass wins. Scaling
    // both together keeps a chord in the balance the voice was tuned to.
    parts.extend(
        foundation_parts(&note_voice, base.root)
            .into_iter()
            .map(|(w, at, gain)| (w, at, gain / n as f32)),
    );

    let total = c.spread * (n - 1) as f64 + note_len + 0.3;
    finish_to(total, parts, base.level)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Energy at one frequency, measured over the sustain. Goertzel rather than a
    /// full FFT — a handful of known frequencies is all this needs.
    fn energy_at(w: &Wave, freq: f32) -> f32 {
        let sr = crate::synth::SR as f32;
        let from = Ord::min((0.25 * sr) as usize, w.len());
        let to = Ord::min((0.65 * sr) as usize, w.len());
        if to <= from {
            return 0.0;
        }
        let k = 2.0 * std::f32::consts::PI * freq / sr;
        let (c, s) = (k.cos(), k.sin());
        let (mut q1, mut q2) = (0.0f32, 0.0f32);
        for i in from..to {
            let v = (w.at(0, i) + w.at(1, i)) * 0.5;
            let q0 = 2.0 * c * q1 - q2 + v;
            q2 = q1;
            q1 = q0;
        }
        (q1 - c * q2).hypot(s * q2) / (to - from) as f32
    }

    /// A chord has to be audible *as a chord* — its notes must carry more energy
    /// than the bass underneath them.
    ///
    /// This exists because of a real bug that shipped past every other test here.
    /// The port gave each note its own sub an octave below, so a three-note chord
    /// came out as six tones and the two loudest partials were both subs. Every
    /// assertion still passed: it rendered, it was audible, it did not clip, and two
    /// people still differed. It just sounded like one low drone rather than a
    /// chord, which is the only thing about it that mattered.
    ///
    /// Measuring the notes is the difference between "it makes a sound" and "it
    /// makes the right sound".
    #[test]
    fn a_chord_is_louder_than_the_bass_under_it() {
        for name in ["keith", "Marty", "jordan", "dave"] {
            let w = render_chord(name, &crate::voices::SOFT_UPDATE, DEFAULT_CHORD);
            let root = crate::voices::SOFT_UPDATE.root;
            let sub = energy_at(&w, root / 2.0);
            let notes: Vec<f32> = chord_for(name, DEFAULT_CHORD)
                .iter()
                .map(|&d| energy_at(&w, root * 2.0_f32.powf(semis(d) / 12.0)))
                .collect();
            let total: f32 = notes.iter().sum();
            let loudest = notes.iter().fold(0.0f32, |a, &b| a.max(b));

            assert!(
                total > sub * 2.0,
                "{name}: notes {total:.5} vs sub {sub:.5} — the bass is drowning the chord"
            );
            assert!(
                loudest > sub,
                "{name}: loudest partial is the sub ({sub:.5}), not a note ({loudest:.5})"
            );
        }
    }

    // Deliberately not asserted here: that each individual note clears some
    // threshold. These voices carry 219 Hz of FM deviation, which spreads a note's
    // energy across sidebands rather than leaving it on the carrier — measured on
    // dave's chord, the bins 10 Hz either side of his 220 Hz note read as strong as
    // the note itself. Energy at one frequency is simply not a measure of whether a
    // note is present in an FM sound, so a per-note assertion would be measuring its
    // own instrument. The totals above survive that smearing because it moves energy
    // around within the notes rather than into the sub.

    /// Turning `high_tilt` down must actually pull the top of the chord back.
    ///
    /// Checked as a ratio between the top and bottom note *within* each render,
    /// never by comparing one render's absolute levels to another's — the finish
    /// normalises every sound to the same loudness, so lowering one note quietly
    /// raises everything else and absolute levels say nothing.
    ///
    /// Comparing the same note across the two renders is also the only way to see
    /// this: energy at a carrier frequency is not comparable *between* notes here,
    /// because the FM index is `fm_amount / freq` and so a low note has far more of
    /// its energy pushed into sidebands than a high one.
    #[test]
    fn pulling_high_tilt_down_quietens_the_top_of_the_chord() {
        // chris — the chord that prompted this, with a note two octaves up.
        let degrees = chord_for("chris", DEFAULT_CHORD);
        let root = crate::voices::SOFT_UPDATE.root;
        let low = root * 2.0_f32.powf(semis(degrees[0]) / 12.0);
        let high = root * 2.0_f32.powf(semis(degrees[2]) / 12.0);

        let ratio_at = |tilt: f32| {
            let c = Chord {
                high_tilt: tilt,
                spread: 0.09,
                ..DEFAULT_CHORD
            };
            let w = render_degrees(&degrees, &crate::voices::SOFT_UPDATE, c);
            energy_at(&w, high) / energy_at(&w, low).max(1e-9)
        };

        let even = ratio_at(1.0);
        let tilted = ratio_at(0.55);
        assert!(
            tilted < even * 0.8,
            "high_tilt 0.55 barely moved the top note: {tilted:.3} against {even:.3} even"
        );
    }

    /// The browser lab and this crate must derive the same chord from the same
    /// name, or a person previewed in the lab sounds like someone else in the app.
    /// These values were read out of the lab (`chordFor`) and pasted here, so the
    /// test fails if either side drifts.
    ///
    /// This is not ceremony. It is how a one-keystroke typo in the FNV prime was
    /// caught in the prototype — "jordan" matched by luck in the low byte while
    /// "keith" did not, which no amount of reading the code had revealed.
    #[test]
    fn chords_match_the_browser_lab() {
        assert_eq!(stable_hash("keith"), 0xfd3f_bdf2_e5ca_7d70);
        for (name, expected) in [
            ("Marty", [0, 5, 8]),
            ("keith", [3, 5, 12]),
            ("jordan", [2, 8, 12]),
            ("sam", [2, 6, 11]),
            ("alex", [0, 4, 12]),
            ("chris", [2, 8, 14]),
            ("dave", [1, 4, 9]),
        ] {
            assert_eq!(chord_for(name, DEFAULT_CHORD), expected.to_vec(), "{name}");
        }
    }

    /// Every chord the enumerator emits must satisfy every constraint. This is the
    /// property the old pick-then-repair approach could not guarantee.
    #[test]
    fn every_valid_chord_obeys_the_constraints() {
        for octaves in 1..=4 {
            for min_gap in 1..=4 {
                for max_span in [4, 8, 10, 20] {
                    for anchor_low in [false, true] {
                        let c = Chord {
                            octaves,
                            min_gap,
                            max_span,
                            anchor_low,
                            ..DEFAULT_CHORD
                        };
                        let span = octaves * 5;
                        for chord in valid_chords(c) {
                            assert_eq!(chord.len(), Ord::min(c.notes, span));
                            assert!(
                                chord.windows(2).all(|w| w[1] - w[0] >= min_gap),
                                "{chord:?}"
                            );
                            assert!(chord[chord.len() - 1] - chord[0] <= max_span, "{chord:?}");
                            assert!(chord.iter().all(|&x| x < span), "{chord:?}");
                            if anchor_low {
                                assert!(chord[0] < 5, "{chord:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// No interval tighter than a minor third at the default spacing. A whole tone
    /// between two high notes is what made the earlier chords warble.
    #[test]
    fn default_spacing_admits_no_trills() {
        for chord in valid_chords(DEFAULT_CHORD) {
            for w in chord.windows(2) {
                let interval = semis(w[1]) - semis(w[0]);
                assert!(interval >= 3.0, "{chord:?} has a {interval} semitone gap");
            }
        }
    }

    /// Constraints tight enough to admit nothing must return an empty list rather
    /// than looping or panicking, and `chord_for` must still hand back something.
    #[test]
    fn impossible_constraints_degrade_gracefully() {
        let c = Chord {
            octaves: 1,
            min_gap: 4,
            max_span: 4,
            ..DEFAULT_CHORD
        };
        assert!(valid_chords(c).is_empty());
        assert_eq!(chord_for("anyone", c).len(), 3);
    }

    /// The arpeggio must draw the *same* notes as the chord — it is the same person,
    /// laid out in time rather than stacked. Spread, tilt and direction change how
    /// a chord is *played*; they must never change which notes it is.
    #[test]
    fn playing_options_do_not_change_which_notes_a_person_gets() {
        let played = Chord {
            spread: 0.09,
            high_tilt: 0.55,
            descending: true,
            ..DEFAULT_CHORD
        };
        for name in ["keith", "Marty", "jordan", "chris"] {
            assert_eq!(
                chord_for(name, played),
                chord_for(name, DEFAULT_CHORD),
                "{name}"
            );
        }
    }
}
