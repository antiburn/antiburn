# Rooted flame activity heuristic

The Rooted flame is a cross-surface signal for recent generated-output activity.
It does not report subscription quota use, provider throttling, or exact
instantaneous token throughput. The implementation is in `hooks/burn.ts`;
`hooks/register.tsx` collects observations and controls presentation on desktop
and terminal surfaces.

## Signal and rate estimate

- Count characters from streamed text, thinking, and tool-input JSON. Other stream
  chunk types do not count.
- For each request, sample the clock on its first non-empty chunk and after each
  further 128 accumulated characters. Flush remaining characters when the request
  completes. The hook does not poll or call a model to measure activity.
- Keep counts in a rolling 5-second window with 500ms buckets. Require at least
  2 seconds of observations before using the stream estimate to enter the state.
- Estimate output tokens by dividing character rate by a chars-per-token ratio.
  Start with 4 characters/token. A completed request with a positive
  `output_tokens` value calibrates the ratio from its observed characters,
  clamped to 1–12 characters/token. Use the arithmetic mean of up to the latest
  eight calibrations. Streamed characters are not token counts.
- Enter high activity at an estimated rate of at least 60 tokens/s. Once active,
  a rate of at least 36 tokens/s can renew the hold.
- A completed request also provides `output_tokens / request duration` as a
  request-average rate. Its duration must be at least 2 seconds. This measurement
  can enter the state only while another request remains in flight; a lone
  completed request does not keep the animation active.

## Hold, visibility, and animation

Qualifying activity renews an 8-second hold. The latest chunk or usable completion
measurement must also be no more than 8 seconds old, and at least one request must
remain in flight. The mark renders only while a visible Above Prompt usage strip
has space and is not covered by the survey. `/antiburn-dev debug` provides separate
15-second active and reduced-motion previews; previews do not change usage data.

On desktop, the mark uses a finite 8-second SVG playback with a 620ms Ember catch.
Continued activity renews playback without replaying ignition. Reduced motion
keeps a static warm mark. On terminal, the same signal drives the fixed two-row
wordmark animation described in the [terminal rendering decision](terminal-renderers.md).
The logo opens the antiburn website during both resting and active states.
It has no activity tooltip. Reset and pace tooltips remain available at their
existing targets.

This is a heuristic. It can miss short bursts, and character-to-token conversion
varies by content. It indicates recent output activity only; it does not infer
subscription burn, provider throttling, or a user's plan.
