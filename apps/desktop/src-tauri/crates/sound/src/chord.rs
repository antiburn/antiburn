use crate::synth::{UNISON, Voice, finish_to, foundation_parts, note_gain, note_part};
use fundsp::prelude32::Wave;

const PENTATONIC: [f32; 5] = [0.0, 2.0, 4.0, 7.0, 9.0];
pub(crate) const NOTIFICATION_CHORD: [usize; 3] = [1, 4, 9];

#[cfg(test)]
mod tests {
    #[test]
    fn notification_chord_stays_fixed() {
        assert_eq!(super::NOTIFICATION_CHORD, [1, 4, 9]);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Chord {
    pub(crate) spread: f64,
    pub(crate) high_tilt: f32,
    pub(crate) descending: bool,
}

fn semitones(degree: usize) -> f32 {
    PENTATONIC[degree % 5] + 12.0 * (degree / 5) as f32
}

pub(crate) fn render_degrees(degrees: &[usize], base: &Voice, chord: Chord) -> Wave {
    let together = chord.spread == 0.0;
    let note_count = degrees.len();
    let note_length = if together { base.dur } else { base.dur * 0.62 };
    let mut note_voice = *base;
    note_voice.chord = UNISON;
    note_voice.dur = note_length;
    let lowest = base.root * 2.0_f32.powf(semitones(degrees[0]) / 12.0);
    let order: Vec<usize> = if chord.descending {
        (0..note_count).rev().collect()
    } else {
        (0..note_count).collect()
    };
    let mut parts = Vec::new();
    for (step, &index) in order.iter().enumerate() {
        let frequency = base.root * 2.0_f32.powf(semitones(degrees[index]) / 12.0);
        let octaves_up = if lowest > 1.0 {
            (frequency / lowest).log2()
        } else {
            0.0
        };
        let tilt = chord.high_tilt.clamp(0.05, 1.0).powf(octaves_up);
        let lean = if together {
            1.0
        } else {
            1.0 - step as f32 * 0.12
        };
        parts.push((
            note_part(&note_voice, frequency, note_gain(note_count, index) * tilt),
            step as f64 * chord.spread,
            lean,
        ));
    }
    parts.extend(
        foundation_parts(&note_voice, base.root)
            .into_iter()
            .map(|(wave, at, gain)| (wave, at, gain / note_count as f32)),
    );
    let total = chord.spread * (note_count - 1) as f64 + note_length + 0.3;
    finish_to(total, parts, base.level)
}
