// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Whether this machine looks like it is in a jurisdiction where analytics need
 * consent *before* they start, rather than a control the reader can find.
 *
 * antiburn ships usage analytics on by default (deviations register D-28). In
 * the EU, the EEA, and the UK that default is the wrong way round: ePrivacy
 * Article 5(3) and the GDPR treat non-essential analytics as something a reader
 * opts *into*. So in those places the first-run control still appears in
 * exactly the same spot — it just starts off.
 *
 * Two deliberate choices about how this guesses:
 *
 * - **It errs toward "yes".** A false positive costs a little data. A false
 *   negative means analytics running on somebody who had to be asked first,
 *   which is the failure that actually matters.
 * - **It reads only what the browser already offers** — the locale the reader
 *   configured, and their time zone. No IP lookup, no geolocation prompt, and
 *   nothing about either is ever sent: the answer only picks a default. Using
 *   a geo-IP service to decide whether to respect privacy law would be its own
 *   small joke.
 */

/** EU member states, the rest of the EEA, and the UK. */
const CONSENT_FIRST_REGIONS = new Set([
  // EU
  'AT',
  'BE',
  'BG',
  'HR',
  'CY',
  'CZ',
  'DK',
  'EE',
  'FI',
  'FR',
  'DE',
  'GR',
  'HU',
  'IE',
  'IT',
  'LV',
  'LT',
  'LU',
  'MT',
  'NL',
  'PL',
  'PT',
  'RO',
  'SK',
  'SI',
  'ES',
  'SE',
  // EEA
  'IS',
  'LI',
  'NO',
  // UK GDPR
  'GB',
]);

/** The region subtag of a BCP 47 tag, upper-cased. `de-DE` → `DE`. */
function region(tag: string): string | null {
  const parts = tag.split('-');
  // Skip the language, and any script subtag (four letters, e.g. `zh-Hant-TW`).
  for (const part of parts.slice(1)) {
    if (part.length === 2 && /^[A-Za-z]{2}$/.test(part)) return part.toUpperCase();
  }
  return null;
}

/**
 * Whether analytics should start switched off on this machine.
 *
 * Answers from the reader's configured locales first, because a region subtag
 * is an explicit statement. Time zone is the fallback for a bare language tag
 * like `de`, and is intentionally coarse: any `Europe/` zone counts, which
 * over-reaches into a few non-EU countries and is the right direction to
 * over-reach in.
 */
export function analyticsDefaultsOff(): boolean {
  const tags: string[] =
    typeof navigator !== 'undefined'
      ? [...(navigator.languages ?? []), navigator.language].filter(Boolean)
      : [];

  for (const tag of tags) {
    const found = region(tag);
    if (found && CONSENT_FIRST_REGIONS.has(found)) return true;
  }

  try {
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone ?? '';
    if (zone.startsWith('Europe/')) return true;
  } catch {
    // A runtime without a resolvable time zone tells us nothing; fall through
    // rather than treating the absence of evidence as evidence.
  }

  return false;
}
