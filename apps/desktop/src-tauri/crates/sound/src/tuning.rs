//! The knobs, so sounds can be settled by ear rather than by argument.
//!
//! Everything here is a *modification* to a tuned voice, not a replacement for one.
//! The voices in [`crate::voices`] came out of a listening session and are the
//! baseline; these adjust how a set of notes is laid out over one, which is the part
//! that only shows up once real chords are playing through real speakers.
//!
//! [`Tuning::default`] holds the current settled values. When a number here
//! changes, that is the record of a decision made by listening.

/// How a sound's notes are laid out over its voice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Tuning {
    /// Milliseconds between note onsets. Zero plays them together as a chord.
    ///
    /// Every sound is stepped rather than simultaneous. Played together, three
    /// notes arrive as one event and the chord's identity has to land in a single
    /// instant; stepped through, the order carries information too and each note
    /// gets its own moment. Chosen by ear over the simultaneous version, which read
    /// as one thick chord rather than as somebody's signature.
    pub spread_ms: f32,
    /// Gain applied per octave above the lowest note, 0 to 1.
    ///
    /// At 1 every note is equal. Below that the top of a chord is pulled back, which
    /// matters because the chords range over three octaves: a note two octaves up
    /// carries far more perceived loudness than the same gain down low, and it is
    /// the note that makes a chord sound shrill rather than the one that gives it
    /// its identity.
    pub high_tilt: f32,
    /// How much the detune narrows as notes rise, 0 to 1. See
    /// [`crate::synth::Voice::detune_track`] — this is what stops high notes
    /// warbling.
    pub detune_track: f32,
    /// Whole-sound transpose, in semitones.
    pub transpose: f32,
    /// Filter cutoff multiplier. Above 1 opens the sound up, below 1 darkens it.
    pub brightness: f32,
    /// Play the notes highest-first. The notes are the same either way — only the
    /// order in time changes.
    pub descending: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            spread_ms: 180.0,
            high_tilt: 0.45,
            detune_track: 0.85,
            transpose: -1.0,
            brightness: 1.5,
            descending: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settled values, written down.
    ///
    /// Not a tautology test: these came out of a listening session and there is no
    /// way to derive them, so the only protection against one being changed by
    /// accident — a refactor, a plausible-looking tidy-up — is a test that says what
    /// was decided. If this fails, either the tuning changed on purpose and this
    /// should be updated with it, or something got edited that shouldn't have been.
    #[test]
    fn the_settled_tuning_is_what_was_chosen_by_ear() {
        let notification = Tuning::default();
        assert_eq!(notification.spread_ms, 180.0);
        assert_eq!(notification.high_tilt, 0.45);
        assert_eq!(notification.detune_track, 0.85);
        assert_eq!(notification.transpose, -1.0);
        assert_eq!(notification.brightness, 1.5);
        assert!(notification.descending);
    }
}
