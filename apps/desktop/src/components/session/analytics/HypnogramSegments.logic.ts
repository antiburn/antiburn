// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { SessionPhase, PhaseSegment } from '../../../lib/types/session';

/**
 * Share of total active time below which a segment is a speckle rather than a
 * run, and gets folded into a neighbour.
 */
const DESPECKLE_FRAC = 0.012;

function mergeSamePhase(segments: PhaseSegment[]): PhaseSegment[] {
  const out: PhaseSegment[] = [];
  for (const segment of segments) {
    const last = out[out.length - 1];
    if (last && last.phase === segment.phase) {
      last.activeMs += segment.activeMs;
      last.tokensIn += segment.tokensIn;
      last.tokensOut += segment.tokensOut;
      last.contextTokens = Math.max(last.contextTokens, segment.contextTokens);
    } else {
      out.push({ ...segment });
    }
  }
  return out;
}

/**
 * Fold sub-threshold segments into their neighbours so a busy session reads as
 * sustained runs rather than as static.
 *
 * The one rule that keeps this honest: a phase's *last surviving* segment is
 * never despeckled. A session with a single brief error band still shows that
 * band, however short it was — otherwise the ribbon would silently disagree
 * with the mode breakdown, which counts it. Total active time is conserved:
 * a folded segment's time and tokens move to the neighbour that absorbs it.
 */
export function coalesce(segments: PhaseSegment[]): PhaseSegment[] {
  let arr = mergeSamePhase(segments);
  const total = arr.reduce((sum, s) => sum + Math.max(s.activeMs, 0), 0) || 1;
  const threshold = total * DESPECKLE_FRAC;

  while (arr.length > 1) {
    const phaseCount = new Map<SessionPhase, number>();
    for (const s of arr) phaseCount.set(s.phase, (phaseCount.get(s.phase) ?? 0) + 1);

    let smallestIndex = -1;
    let smallestMs = Infinity;
    for (let i = 0; i < arr.length; i++) {
      const candidate = arr[i];
      if (!candidate) continue;
      if (phaseCount.get(candidate.phase) === 1) continue;
      if (candidate.activeMs < threshold && candidate.activeMs < smallestMs) {
        smallestMs = candidate.activeMs;
        smallestIndex = i;
      }
    }
    if (smallestIndex < 0) break;

    const speckle = arr[smallestIndex];
    if (!speckle) break;
    const left = arr[smallestIndex - 1];
    const right = arr[smallestIndex + 1];
    const target = !left
      ? right
      : !right
        ? left
        : left.activeMs >= right.activeMs
          ? left
          : right;
    if (!target) break;

    target.activeMs += speckle.activeMs;
    target.tokensIn += speckle.tokensIn;
    target.tokensOut += speckle.tokensOut;
    target.contextTokens = Math.max(target.contextTokens, speckle.contextTokens);
    arr.splice(smallestIndex, 1);
    arr = mergeSamePhase(arr);
  }
  return arr;
}
