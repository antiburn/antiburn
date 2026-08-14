// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The signal path, ported from `prototypes/sound-lab`.
//!
//! [`Voice`] mirrors the browser lab's parameter panel one-to-one: three detuned
//! copies per note panned left/centre/right, into a resonant low-pass with an
//! exponential sweep, into a tanh drive stage; a sine sub an octave down and a
//! band-passed noise layer alongside, each on its own envelope. No reverb — the
//! width comes only from the detuned copies being panned apart.
//!
//! **Keep this in step with `prototypes/sound-lab/src/signature.rs`.** The lab is
//! where sounds get chosen by ear; this is where they get played. Two copies of the
//! same maths is a real cost, accepted because the alternative is either shipping
//! the lab's tooling in the app or tuning sound by editing constants and rebuilding.
//! The chord test in [`crate::chord`] is what stops the two drifting silently.

use fundsp::prelude32::*;

/// 48 kHz throughout. Matches the lab, and every modern output device resamples
/// from it cleanly.
pub const SR: f64 = 48_000.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Wave4 {
    Sine,
    Triangle,
}

/// One sound's parameters, one-to-one with the lab's panel.
#[derive(Clone, Copy)]
pub struct Voice {
    pub dur: f64,
    pub attack: f32,
    pub release: f32,
    pub root: f32,
    /// Semitone offsets played together. `UNISON` is a single note.
    pub chord: &'static [f32],
    pub wave: Wave4,
    /// Spread between the three copies, in cents.
    pub detune: f32,
    /// How much the detune narrows as notes get higher, 0 to 1.
    ///
    /// Cents are a *ratio*, so a fixed detune beats faster the higher the note: the
    /// 8.1 cents that give the 196 Hz root a warm 0.9 Hz chorus give a 1319 Hz note
    /// a 6.2 Hz warble, which is heard as a trill. At 1 the detune is scaled by
    /// `root / freq` so every note beats at the rate the root was tuned to; at 0 it
    /// is left in cents and high notes warble.
    pub detune_track: f32,
    pub cutoff: f32,
    pub q: f32,
    /// Filter sweep multiplier across the sound. 1.0 is static; below 1.0 the
    /// filter closes as the sound decays.
    pub sweep: f32,
    pub fm_amount: f32,
    pub fm_ratio: f32,
    pub drive: f32,
    pub sub_gain: f32,
    pub sub_att: f32,
    pub air_gain: f32,
    pub air_band: f32,
    pub air_att: f32,
    /// Target loudness: the windowed RMS this sound is normalised to. Every
    /// person's chord lands here regardless of which notes their name hashed to,
    /// so nobody's alert is randomly louder than anybody else's.
    pub level: f32,
}

pub const UNISON: &[f32] = &[0.0];

/// Linear rise, then exponential decay — matching Web Audio's
/// `linearRampToValueAtTime` followed by `setTargetAtTime`, where the time
/// constant is the reciprocal of the release value.
fn shape(t: f32, attack: f32, release: f32) -> f32 {
    if t < attack {
        t / attack.max(1e-5)
    } else {
        (-(t - attack) * release).exp()
    }
}

fn cents(f: f32, c: f32) -> f32 {
    f * 2.0_f32.powf(c / 1200.0)
}

/// `tanh(x·k) / tanh(k)`, the same curve the lab's WaveShaper is built from.
fn drive_k(drive: f32) -> f32 {
    1.0 + drive * 12.0
}

fn render(dur: f64, node: &mut dyn AudioUnit) -> Wave {
    Wave::render(SR, dur, node)
}

/// Render the three detuned copies of one note, panned apart.
fn note_layer(v: &Voice, f: f32, gain: f32) -> Wave {
    let (att, rel, dur) = (v.attack, v.release, v.dur);
    let env = move |t: f32| shape(t, att, rel);
    let span = dur as f32;
    let (c0, sweep, q) = (v.cutoff, v.sweep, v.q);
    let k = drive_k(v.drive);
    let cut = move |t: f32| {
        let x = (t / span).min(1.0);
        (c0 * sweep.powf(x)).clamp(60.0, 18000.0)
    };

    // FM modulates the carrier frequency, with the index decaying faster than the
    // amplitude — the asymmetry that makes it read as struck rather than blown.
    let (fm_a, fm_r) = (v.fm_amount, v.fm_ratio);
    let wave = v.wave;

    // Type-erased so both waveforms and the FM branch share one chain.
    let osc = |fd: f32| -> Box<dyn AudioUnit> {
        if fm_a > 0.0 {
            // The modulator sums into the carrier frequency, then drives whichever
            // waveform the patch selected. Hardcoding a sine here was a bug once:
            // the warning specifies a triangle, and a sine carrier produces far
            // fewer sidebands, so the port was not the same instrument.
            let m = sine_hz(fd * fm_r) * envelope(move |t: f32| fm_a * (-t * rel * 2.2).exp()) + fd;
            let e = envelope(move |t: f32| shape(t, att, rel));
            match wave {
                Wave4::Sine => Box::new((m >> sine()) * e * gain),
                Wave4::Triangle => Box::new((m >> triangle()) * e * gain),
            }
        } else {
            match wave {
                Wave4::Sine => Box::new(sine_hz(fd) * envelope(env) * gain),
                Wave4::Triangle => Box::new(triangle_hz(fd) * envelope(env) * gain),
            }
        }
    };

    // Narrow the detune as the note rises, so every note beats at the rate the root
    // was tuned to instead of warbling faster the higher it goes.
    let track = v.detune_track.clamp(0.0, 1.0);
    let spread = if f > 1.0 && v.root > 1.0 {
        v.detune * (v.root / f).powf(track)
    } else {
        v.detune
    };

    let voices = [
        (cents(f, -spread), -0.55),
        (cents(f, 0.0), 0.0),
        (cents(f, spread), 0.55),
    ];

    let mut acc: Option<Wave> = None;
    for (fd, pos) in voices {
        let mut chain = unit::<U0, U1>(osc(fd))
            >> (pass() | lfo(move |t: f32| (cut(t), q)))
            >> lowpass()
            // Oversampled to match the browser's `WaveShaper.oversample = '4x'`.
            // Without it the drive stage aliases, folding false high frequencies
            // back down — measured at nearly double the reference brightness on
            // the warning, which is the most heavily driven of the sounds.
            >> oversample(shape_fn(move |x: f32| (x * k).tanh() / k.tanh()))
            >> pan(pos);
        let w = render(v.dur + 0.3, &mut chain);
        acc = Some(match acc {
            None => w,
            Some(mut a) => {
                for ch in 0..2 {
                    a.mix_channel(ch, 0, w.channel(ch));
                }
                a
            }
        });
    }
    acc.unwrap()
}

/// Render one voice at its own root — the "nobody in particular" sound.
pub fn render_voice(v: &Voice) -> Wave {
    finish_to(v.dur + 0.35, voice_parts(v), v.level)
}

/// One pitched note at `freq`, with no sub and no air.
///
/// Exposed so a chord can place its notes itself — at their own start times for an
/// arpeggio — while still weighting them exactly the way a voice weights the notes
/// of its own chord. Reusing that weighting is the point: the balance between the
/// notes and the foundation under them was set by ear, and rebuilding a chord out
/// of separately-normalised notes throws it away.
pub fn note_part(v: &Voice, freq: f32, gain: f32) -> Wave {
    note_layer(v, freq, gain)
}

/// How much one note of an `n`-note chord contributes.
///
/// Every note weighs the same, and the total is held constant as notes are added —
/// matching the lab, which gives each note `0.5 / n` however many there are. The
/// `/ 3` accounts for the three detuned copies [`note_part`] sums internally.
///
/// Backing off the upper notes was tried and is wrong for this: it tilts a chord
/// toward its bass, which is the opposite of what a chord needs when there is
/// already a sub underneath it.
pub fn note_gain(n: usize, _i: usize) -> f32 {
    (1.0 / (n * 3) as f32) * 0.5
}

/// The layers one voice is made of, before any mixing or finishing decision.
fn voice_parts(v: &Voice) -> Vec<(Wave, f64, f32)> {
    let mut parts = tone_parts(v);
    parts.extend(foundation_parts(v, v.root));
    parts
}

/// Just the pitched layers — the notes themselves, with no sub and no air.
///
/// Separated out because a chord needs these **per note** while it needs the
/// foundation below only **once**. See [`foundation_parts`].
pub fn tone_parts(v: &Voice) -> Vec<(Wave, f64, f32)> {
    let n_notes = v.chord.len();
    let mut parts: Vec<(Wave, f64, f32)> = Vec::new();
    for (i, semi) in v.chord.iter().enumerate() {
        let f = v.root * 2.0_f32.powf(semi / 12.0);
        parts.push((note_layer(v, f, note_gain(n_notes, i)), 0.0, 1.0));
    }
    parts
}

/// The sub an octave below `root`, plus the band-passed air on top.
///
/// These belong to the **sound**, not to each note in it, and that distinction is
/// what makes a chord read as a chord. Giving every note its own sub was measured
/// doing real damage: a three-note chord came out as six tones, and the two loudest
/// partials were both subs rather than notes — a thick low drone with the actual
/// harmony buried under it, which is exactly what "it sounds like a unison" means.
///
/// One sub, one air layer, pitched from the chord's lowest note so the bass
/// reinforces the harmony instead of adding a seventh pitch to it.
pub fn foundation_parts(v: &Voice, root: f32) -> Vec<(Wave, f64, f32)> {
    let mut parts: Vec<(Wave, f64, f32)> = Vec::new();

    if v.sub_gain > 0.001 {
        let (att, rel) = (v.sub_att, v.release * 0.85);
        let g = v.sub_gain * 0.6;
        let mut n =
            (sine_hz(root / 2.0) * envelope(move |t: f32| shape(t, att, rel) * g)) >> pan(0.0);
        parts.push((render(v.dur + 0.3, &mut n), 0.0, 1.0));
    }

    if v.air_gain > 0.001 {
        let (att, rel) = (v.air_att, v.release * 0.9);
        let g = v.air_gain * 0.32;
        let mut n = ((noise().seed(31) >> bandpass_hz(v.air_band, 1.1))
            * envelope(move |t: f32| shape(t, att, rel) * g))
            >> pan(0.25);
        parts.push((render(v.dur + 0.3, &mut n), 0.0, 1.0));
    }

    parts
}

/// RMS of the loudest 250 ms, mono-summed.
///
/// Full-length RMS would rank a long sound with a short body quiet just for its
/// trailing silence; loudness is heard from the loud part, so the window follows it.
pub fn windowed_rms(w: &Wave) -> f32 {
    let win = Ord::min((0.25 * SR) as usize, w.len());
    if win == 0 {
        return 0.0;
    }
    let mono = |i: usize| ((w.at(0, i) + w.at(1, i)) * 0.5) as f64;
    let mut run: f64 = (0..win).map(|i| mono(i) * mono(i)).sum();
    let mut best = run;
    for i in win..w.len() {
        run += mono(i) * mono(i) - mono(i - win) * mono(i - win);
        if run > best {
            best = run;
        }
    }
    (best / win as f64).sqrt() as f32
}

/// Soft-clip rather than hard-limit: percussive sounds have peaks far above their
/// average, and tanh rounds those off at the cost of a little harmonic distortion
/// on the transient, inaudible at these levels. The 25 ms fade kills any click at
/// the very end where an envelope hasn't quite reached zero.
fn soft_clip_fade(mut w: Wave) -> Wave {
    for ch in 0..w.channels() {
        for s in w.channel_mut(ch).iter_mut() {
            *s = (*s * 1.05).tanh() * 0.92;
        }
    }
    w.fade_out(0.025);
    w
}

/// Mix, normalise to a target loudness, then soft-clip and fade.
///
/// Normalisation rather than a fixed gain is what makes every person's chord land
/// at the same loudness whatever notes their name hashed to. It is per-class, not
/// global: the difference in loudness *between* sounds is the design, and is set by
/// giving each one a different `target`.
pub fn finish_to(total: f64, parts: Vec<(Wave, f64, f32)>, target: f32) -> Wave {
    let mut w = mix(total, parts);
    normalise_to(&mut w, target);
    soft_clip_fade(w)
}

/// Scale a wave so its windowed RMS lands on `target`. Pure gain — no shaping, so
/// unlike [`soft_clip_fade`] this is safe to apply to a part that will be mixed
/// with others afterwards.
fn normalise_to(w: &mut Wave, target: f32) {
    let cur = windowed_rms(w);
    if cur <= 1e-6 {
        return;
    }
    let g = target / cur;
    for ch in 0..w.channels() {
        for s in w.channel_mut(ch).iter_mut() {
            *s *= g;
        }
    }
}

/// The raw mixdown: independently-rendered elements placed at time offsets.
///
/// Components are deliberately **not** level-matched individually — the balance
/// between transient and body is the design, so normalising each part separately
/// would erase it.
fn mix(total: f64, parts: Vec<(Wave, f64, f32)>) -> Wave {
    let mut out = Wave::zero(2, SR, total);
    let cap = out.len();
    for (w, offset, gain) in parts {
        let off = (offset * SR) as usize;
        if off >= cap {
            continue;
        }
        let take = Ord::min(cap - off, w.len());
        for ch in 0..2 {
            let src = w.channel(Ord::min(ch, w.channels() - 1));
            let scaled: Vec<f32> = src[..take].iter().map(|s| s * gain).collect();
            out.mix_channel(ch, off as isize, &scaled);
        }
    }
    out
}
