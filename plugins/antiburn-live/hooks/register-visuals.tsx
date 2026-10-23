import type { EngineInterface, RenderInput, RenderNode } from 'claude-code';
import type { LiveCheckId, LiveCheckView, LiveStage } from '../types';
type PaneEvent = RenderInput<'Pane'>;
import { liveChecks } from './checks';
import { APP_URL } from './brand';
import { PRESENTATION } from './usage-presentation';
import { terminalWordmark } from './terminal-flame';

// A theme has no muted warning background, and the warning key fills the band
// too brightly to read. Each band sets its background and its text color as one
// pair, so the contrast stays the same in a light or a dark theme.
// The antiburn brand token (apps/desktop/design.md), dark value. A mod cannot
// read the terminal theme, so the terminal uses one value.
export const BRAND = '#FF5914';
// The antiburn app's check language (apps/desktop/design.md): failure wording takes
// burn-check-failure-text, light hsl(18 100% 36.4%) and dark hsl(18 100% 68%), in semibold monospace.
// A tint takes burn-check-failure-fill, which is the brand value.
// A Text color cannot follow the theme, so the desktop draws this wording as SVG.
export const FAILURE_TEXT_DARK = '#FF8D5C';
export const FAILURE_THEMED =
  '<style>.f{fill:#BA3800}.t{fill:#FF5914;fill-opacity:.08}' +
  `@media (prefers-color-scheme:dark){.f{fill:${FAILURE_TEXT_DARK}}.t{fill-opacity:.12}}</style>`;
export const MONO = 'ui-monospace, SFMono-Regular, Menlo, monospace';
// A resolved outcome takes burn-check-pass-fill, the cyan of the Resolved icon:
// light hsl(191.5 83% 36.8%), dark hsl(192 63% 47.6%). Passed counts use it at regular weight.
// The terminal cannot read the theme, so it uses the dark value, as it does for the brand.
export const PASS_FILL_DARK = '#2DA7C6';
// Terminal text has no band behind it and the theme can be light, so it takes the light value,
// 3.8:1 on white and 4.9:1 on a dark terminal. A box keeps the dark value, with its own ink.
export const PASS_TEXT_TERMINAL = '#108EAC';
// The count tint: 12% (light) or 16% (dark) of the brand, mixed onto the strip surface as one solid value.
// Light #9A3412 on #F2DCCD is 5.5:1. Dark #FF8D5C on #51372A is 4.8:1. The primary button stays the strongest mark.
export const COUNT_TINT_TERMINAL = '#4D2F23';
export const PASS_THEMED = `<style>.p{fill:#108EAC}@media (prefers-color-scheme:dark){.p{fill:${PASS_FILL_DARK}}}</style>`;

// The warning band is a neutral grey, so the brand and the check orange are its only colours.
// The name takes the brand in bold, 4.8:1 on the band. The count sits in a quiet box: the dark failure
// text on an 18% brand tint mixed onto the band, 5.3:1. A terminal cannot blend, so the tint is a solid value.
// A mod cannot read the terminal theme, so the band is the same on a light theme.
// A 256-colour terminal rounds a tinted colour to its colour cube, which turns a dark warm band into
// #5F5F5F. The band greys are on the 256-colour grey ramp, so they stay the same there.
export const BAND = {
  warning: {
    background: '#262626',
    text: '#E4E4E4',
    // The metric is one step quieter than the check name, 6.4:1 on the band.
    metric: '#A8A8A8',
    mark: FAILURE_TEXT_DARK,
    label: BRAND,
    box: COUNT_TINT_TERMINAL,
    ink: FAILURE_TEXT_DARK,
  },
  // The resolved band is the same grey. Its outcome takes the pass cyan of the Resolved icon, 5.4:1 on the band.
  resolved: {
    background: '#262626',
    text: '#E4E4E4',
    mark: PASS_FILL_DARK,
    label: BRAND,
  },
} as const;

// The desktop draws the strip on its own surface, so a fill looks like an alert
// there. The desktop band colors only the status dot and its label, as the
// sidebar status rows do, and uses theme keys so a light theme stays readable.
export function bandStyle(surface: string, kind: 'warning' | 'resolved') {
  if (surface !== 'terminal') {
    const mark = kind === 'warning' ? 'warning' : 'success';
    return {
      box: {},
      text: undefined,
      mark,
      label: mark,
      count: { color: mark },
      metric: undefined,
    };
  }
  const band = BAND[kind];
  const label = band.label;
  const count =
    'box' in band
      ? { backgroundColor: band.box, color: band.ink }
      : { color: band.mark };
  const metric = 'metric' in band ? band.metric : band.text;
  return {
    box: { backgroundColor: band.background },
    text: band.text,
    mark: band.mark,
    label,
    count,
    metric,
  };
}

// The wordmark is a logo, so it keeps the brand value on a light and a dark theme.
// The antiburn dot-matrix wordmark, as the installer draws it (install.sh, `core`): 40 columns, 7 rows.
// Rows 0 and 1 hold only the ascenders, so the letter bodies centre on row 4.5. Two empty rows
// below move the box centre to row 4.5, so the letters align with the text beside them.
export const WORDMARK_ROWS = [
  '.................O......................',
  '.............O.....O....................',
  '.OOO..O.OO..OOOO.O.O.OO..O...O.O.O.O.OO.',
  '....O.OO..O..O...O.OO..O.O...O.OO..OO..O',
  '.OOOO.O...O..O...O.O...O.O...O.O...O...O',
  'O...O.O...O..O...O.O...O.O..OO.O...O...O',
  '.OO.O.O...O...OO.O.OOOO...OO.O.O...O...O',
] as const;
export const WORDMARK_BOX_ROWS = 9;
export const WORDMARK_HEIGHT = 17;
export const WORDMARK_WIDTH = Math.round((WORDMARK_HEIGHT * 40) / WORDMARK_BOX_ROWS);
export const WORDMARK_SVG =
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 ${WORDMARK_BOX_ROWS}">` +
  WORDMARK_ROWS.flatMap((row, y) =>
    [...row].flatMap((cell, x) =>
      cell === 'O'
        ? [`<circle cx="${x + 0.5}" cy="${y + 0.5}" r="0.45" fill="${BRAND}"/>`]
        : [],
    ),
  ).join('') +
  '</svg>';

// The count pill is a tint, as the terminal count box is. One pair cannot pass on a light and a dark
// theme, so the SVG reads the theme and changes both the fill and the ink.
export const PILL_THEMED =
  '<style>.r{fill:#F2DCCD}.i{fill:#9A3412}' +
  `@media (prefers-color-scheme:dark){.r{fill:#51372A}.i{fill:${FAILURE_TEXT_DARK}}}</style>`;
// A monospace character is about 0.6 of the font size wide, so the pill width follows the text.
export const COUNT_PILL_HEIGHT = 18;

export function countPillWidth(text: string): number {
  return Math.ceil(text.length * 12 * 0.6) + 16;
}

export function countPillSvg(text: string): string {
  const width = countPillWidth(text);
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${COUNT_PILL_HEIGHT}" viewBox="0 0 ${width} ${COUNT_PILL_HEIGHT}">` +
    PILL_THEMED +
    `<rect class="r" width="${width}" height="${COUNT_PILL_HEIGHT}" rx="${COUNT_PILL_HEIGHT / 2}"/>` +
    `<text class="i" x="${width / 2}" y="13" text-anchor="middle" font-family="${MONO}" font-size="12" font-weight="600">${escapeXml(text)}</text>` +
    '</svg>'
  );
}

// design.md: group headings take a 14px icon with a 2.5px stroke,
// CircleAlert in burn-check-failure-fill, CircleCheck in burn-check-pass-fill (dark value).
export const GROUP_ICON: Readonly<Record<string, string>> = {
  Signals:
    '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#FF5914" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/></svg>',
  // Lucide hourglass in the app's system orange, as the app's Awaiting verification badge.
  'Awaiting verification':
    '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#FF9F0A" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 22h14"/><path d="M5 2h14"/><path d="M17 22v-4.172a2 2 0 0 0-.586-1.414L12 12l-4.414 4.414A2 2 0 0 0 7 17.828V22"/><path d="M7 2v4.172a2 2 0 0 0 .586 1.414L12 12l4.414-4.414A2 2 0 0 0 17 6.172V2"/></svg>',
  '✓ Resolved':
    '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#2DA7C6" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/></svg>',
};

// A section icon draws in once when its section takes a new check. Reduced motion keeps the fade only.
export const ICON_ENTER =
  '<style>svg{animation:enter 200ms cubic-bezier(.23,1,.32,1) both}' +
  '@keyframes enter{from{opacity:0;transform:scale(.95)}to{opacity:1;transform:scale(1)}}' +
  '@media (prefers-reduced-motion:reduce){svg{animation:fade 120ms ease-out both}@keyframes fade{from{opacity:0}to{opacity:1}}}</style>';

export function sectionIcon(source: string, isEntering: boolean): string {
  return isEntering ? source.replace(/(<svg[^>]*>)/, `$1${ICON_ENTER}`) : source;
}

// The desktop heading names: the app says "Failed checks" and "Passed checks".
export const DESKTOP_TITLE: Readonly<Record<string, string>> = {
  Signals: 'Active checks',
  '✓ Resolved': 'Resolved',
};

export type ChipView = {
  readonly label: string;
  readonly mark?: { readonly glyph: string; readonly color: string };
};

// The chip puts the state mark in its own tinted Text, because a dimmed
// Button greys every glyph in its label.
export function chipView(checks: readonly LiveCheckView[]): ChipView {
  const shown = checks.filter((check) => !check.isHidden);
  const active = shown.filter((check) => check.stage === 'active').length;
  const watching = shown.filter((check) => check.stage === 'watching').length;
  // A building check is not in the pane, so the chip does not count it.
  if (active > 0)
    return {
      label: `antiburn · ${active}`,
      mark: { glyph: '●', color: METRIC },
    };
  if (watching > 0)
    return {
      label: `antiburn · ${watching}`,
      mark: { glyph: '⧗', color: METRIC },
    };
  return { label: 'antiburn', mark: { glyph: '✓', color: PASS_TEXT_TERMINAL } };
}

export function chipLabel(checks: readonly LiveCheckView[]): string {
  const shown = checks.filter((check) => !check.isHidden);
  const active = shown.filter((check) => check.stage === 'active').length;
  const watching = shown.filter((check) => check.stage === 'watching').length;
  // The count matches the strip: only the checks that still need a fix.
  if (active > 0) return `antiburn · ${active} ●`;
  if (watching > 0) return `antiburn · ${watching} ⧗`;
  return 'antiburn ✓';
}

// A click on the strip gives the strip the keys, so the host refuses the pane's own focus request.
// The press then moves the focus ring to the first row in the pane.

export const GROUPS: readonly {
  title: string;
  stages: readonly LiveStage[];
  isFolded: boolean;
}[] = [
  { title: 'Signals', stages: ['active'], isFolded: false },
  { title: 'Awaiting verification', stages: ['watching'], isFolded: false },
  { title: '✓ Resolved', stages: ['resolved'], isFolded: true },
];

// A theme key. Claude Code 2.1.289 does not export its Color type.
export const AREAS: Readonly<Record<string, { label: string; color: string }>> = {
  tools: { label: 'Tool calls', color: 'warning' },
  context: { label: 'Context', color: 'planMode' },
  model: { label: 'Model', color: 'merged' },
  skills: { label: 'Skills', color: 'warning' },
  scope: { label: 'Scope', color: 'planMode' },
};

export const METRIC = 'warning';

// The desktop draws each check as a card with a tinted icon tile, as the
// antiburn app's checks list does. An Svg draws as an image, so each tile
// holds the dark value of its design.md check token (check-tools,
// check-depth, check-old-model, check-overthinking) and its lucide paths.
export const TILE: Readonly<
  Record<LiveCheckId, { tint: string; paths: readonly string[] }>
> = {
  X1: {
    tint: '#FF922E',
    paths: [
      'm17 2 4 4-4 4',
      'M3 11v-1a4 4 0 0 1 4-4h14',
      'm7 22-4-4 4-4',
      'M21 13v1a4 4 0 0 1-4 4H3',
    ],
  },
  D: {
    tint: '#1ED38A',
    paths: [
      'm12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z',
      'm22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65',
      'm22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65',
    ],
  },
  // E1 has no check token of its own yet, so it borrows the check-mcp dark value.
  E1: {
    tint: '#22B8EF',
    paths: ['M3 11a8 8 0 1 0 16 0a8 8 0 1 0-16 0', 'm21 21-4.3-4.3'],
  },
  // K1 and K2 use the check-skills dark value. C1 has no check token of its own yet, so it borrows check-subagents.
  K1: {
    tint: '#FFD32A',
    paths: [
      'M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z',
    ],
  },
  C1: {
    tint: '#FF4D80',
    paths: [
      'M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0',
      'M6 12a6 6 0 1 0 12 0a6 6 0 1 0-12 0',
      'M10 12a2 2 0 1 0 4 0a2 2 0 1 0-4 0',
    ],
  },
  K2: {
    tint: '#FFD32A',
    paths: [
      'M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8',
      'M21 3v5h-5',
      'M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16',
      'M8 16H3v5',
    ],
  },
  O: {
    tint: '#FFB32C',
    paths: [
      'M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8',
      'M3 3v5h5',
      'M12 7v5l4 2',
    ],
  },
  T: {
    tint: '#A477FF',
    paths: [
      'M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588A4 4 0 1 0 12 18Z',
      'M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588A4 4 0 1 1 12 18Z',
      'M15 13a4.5 4.5 0 0 1-3-4 4.5 4.5 0 0 1-3 4',
      'M17.599 6.5a3 3 0 0 0 .399-1.375',
      'M6.003 5.125A3 3 0 0 0 6.401 6.5',
      'M3.477 10.896a4 4 0 0 1 .585-.396',
      'M19.938 10.5a4 4 0 0 1 .585.396',
      'M6 18a4 4 0 0 1-1.967-.516',
      'M19.967 17.484A4 4 0 0 1 18 18',
    ],
  },
};

// The burn-check-neutral dark value, for checks that are not live.
// The antiburn desktop app, which runs the checks across sessions.

export const TILE_NEUTRAL = '#7D8185';

// A K1 metric starts with the skill name, such as `/verify named · not run`.
export function leadingSkill(metric: string): { name: string; rest: string } | null {
  const match = /^(\/\S+) (.+)$/.exec(metric);
  return match === null ? null : { name: match[1] ?? '', rest: match[2] ?? '' };
}

// A monospace character is about 0.6 of the font size wide, so the pill width follows the name.
export const SKILL_PILL_FONT = 12;
export const SKILL_PILL_HEIGHT = 20;
export const SKILL_PILL_PAD = 6;

export function skillPillWidth(name: string): number {
  return Math.ceil(name.length * SKILL_PILL_FONT * 0.6) + SKILL_PILL_PAD * 2;
}

export function escapeXml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// The skill name is a quiet badge: failure text on a light failure tint, in a rounded rectangle,
// so it names the skill without the weight of the count pill. Contrast: about 5.3:1 on light, 5.9:1 on dark.
export const SKILL_BADGE_RADIUS = 4;
export function skillPillSvg(name: string): string {
  const width = skillPillWidth(name);
  const h = SKILL_PILL_HEIGHT;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${h}" viewBox="0 0 ${width} ${h}">` +
    FAILURE_THEMED +
    `<rect class="t" width="${width}" height="${h}" rx="${SKILL_BADGE_RADIUS}" fill="${BRAND}" fill-opacity="0.1"/>` +
    `<text class="f" x="${width / 2}" y="${(h / 2 + (SKILL_PILL_FONT * CAP_HEIGHT) / 2).toFixed(2)}" text-anchor="middle" fill="#D9541A" ` +
    `font-family="${MONO}" font-size="${SKILL_PILL_FONT}" font-weight="500">${escapeXml(name)}</text>` +
    '</svg>'
  );
}

// A live metric reads as the app's failed count: semibold monospace failure text.
// Each part between the dots is one SVG, so a narrow card wraps the metric between parts.
export const METRIC_FONT = 12;
export const METRIC_HEIGHT = 16;

export function metricParts(metric: string): string[] {
  const parts = metric.split(' · ');
  return parts.map((part, index) => (index < parts.length - 1 ? `${part} ·` : part));
}

export function metricPartWidth(part: string): number {
  return Math.ceil(part.length * METRIC_FONT * 0.6) + 1;
}

export function metricPartSvg(
  part: string,
  tone: 'failure' | 'pass' = 'failure',
): string {
  const width = metricPartWidth(part);
  const isPass = tone === 'pass';
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${METRIC_HEIGHT}" viewBox="0 0 ${width} ${METRIC_HEIGHT}">` +
    (isPass ? PASS_THEMED : FAILURE_THEMED) +
    `<text class="${isPass ? 'p' : 'f'}" x="0" y="12" fill="${isPass ? PASS_FILL_DARK : '#D9541A'}" font-family="${MONO}" ` +
    `font-size="${METRIC_FONT}" font-weight="${isPass ? 400 : 600}" xml:space="preserve">${escapeXml(part)}</text>` +
    '</svg>'
  );
}

// The empty pane icon: the lucide activity line in a muted round tile. The SVG reads the theme itself.
export const EMPTY_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">' +
  '<style>circle{fill:#000;fill-opacity:.05}path{stroke:#000;stroke-opacity:.35}' +
  '@media (prefers-color-scheme:dark){circle{fill:#fff;fill-opacity:.06}path{stroke:#fff;stroke-opacity:.4}}</style>' +
  '<circle cx="20" cy="20" r="20"/>' +
  '<g transform="translate(10 10) scale(0.8333)"><path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></g>' +
  '</svg>';

// A light grey with alpha reads as a subtle card on a light or a dark pane. A mid grey
// darkens a white pane more than it lightens a dark one, so the grey is lighter than the middle.
export const CARD_BACKGROUND = '#AAAAAA12';
export const CARD_OPEN_BACKGROUND = '#AAAAAA21';
// One step stronger than the card fill, as the app's hovered card.

// The antiburn mark from apps/desktop/src/assets/antiburn-mark.svg. The app fills it
// with the label colour at 2% to 7.5%. The mod cannot read the theme, so a mid grey
// with alpha lightens a dark pane and darkens a light pane by about the same amount.
// Each dot gets its own opacity along the diagonal, as the app's gradient does, so
// the markup needs no gradient reference. A radius of 3.2 (not 3.4) keeps the dots
// from overlapping, because overlapping alpha draws darker seams.
export const MARK_DOTS: readonly (readonly [number, number])[] = [
  [16, 9.3],
  [22.4, 9.3],
  [28.8, 9.3],
  [35.2, 15.7],
  [16, 22.1],
  [22.4, 22.1],
  [28.8, 22.1],
  [35.2, 22.1],
  [9.6, 28.5],
  [35.2, 28.5],
  [16, 34.9],
  [22.4, 34.9],
  [35.2, 34.9],
];
// About 45% of the pane width, from 160 to 280 CSS pixels (280 is the app's mark).
// A desktop column is about 8 pixels wide (an assumption, not an engine value).
export function markSize(columns: number): number {
  return Math.round(Math.min(280, Math.max(160, columns * 8 * 0.45)));
}

// The stops copy the app's .burn-check-background-mark: clear to 10% of the
// diagonal, 2% at 45%, 7.5% at the corner, in the label colour. The mark is
// smaller than the app's, so the low stops are raised to keep the top left visible.
export function markOpacity(cx: number, cy: number): number {
  const t = (cx - 6.2 + (cy - 5.9)) / 64.8;
  if (t <= 0.05) return 0;
  if (t <= 0.45) return (0.03 * (t - 0.05)) / 0.4;
  return 0.03 + (0.045 * (t - 0.45)) / 0.55;
}

// The mod cannot read the theme, so the SVG reads it: black dots on a light pane,
// white dots at half strength on a dark one. If a host drops the style element,
// the mid-grey fill attribute still draws the mark at about half strength.
export const WATERMARK_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="6.2 5.9 32.4 32.4">' +
  '<style>circle{fill:#000}@media (prefers-color-scheme:dark){circle{fill:#fff}g{opacity:.5}}</style><g>' +
  MARK_DOTS.map(
    ([cx, cy]) =>
      `<circle cx="${cx}" cy="${cy}" r="3.2" fill="#808080" fill-opacity="${markOpacity(cx, cy).toFixed(4)}"/>`,
  ).join('') +
  '</g></svg>';

// The antiburn mark at full strength in the brand orange, as a logo beside the footer link.
// The shape marks it as the brand, so the orange does not read as an alert.
export const BRAND_MARK_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="6.2 5.9 32.4 32.4">' +
  MARK_DOTS.map(
    ([cx, cy]) => `<circle cx="${cx}" cy="${cy}" r="3.2" fill="${BRAND}"/>`,
  ).join('') +
  '</svg>';

// The cap height of the system font, as a part of the font size. Digits are as tall as capitals.
export const CAP_HEIGHT = 0.7;

// A round count badge. The SVG reads the theme itself, as the watermark does.
export function badgeSvg(count: number): string {
  const label = count > 99 ? '99+' : String(count);
  const size = label.length > 1 ? 9 : 11;
  return (
    '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">' +
    '<style>text{fill:#000;fill-opacity:.6}@media (prefers-color-scheme:dark){text{fill:#fff;fill-opacity:.7}}</style>' +
    '<circle cx="10" cy="10" r="10" fill="#808080" fill-opacity="0.2"/>' +
    // "central" centres the em box, not the digits, so the digits sit low. The baseline drops by half the cap height instead.
    `<text x="10" y="${(10 + (size * CAP_HEIGHT) / 2).toFixed(2)}" text-anchor="middle" font-family="-apple-system, system-ui, sans-serif" font-size="${size}" style="font-variant-numeric:tabular-nums">${label}</text>` +
    '</svg>'
  );
}

// A watched check names its next step.
export const WATCHING_STATUS = 'Checking the next turns.';

// A prompt block shows about three lines of a card. Longer text cuts at a word, and Show all opens the rest.
export const PROMPT_PREVIEW_CHARS = 180;
// A quiet well inside the card. A round border in the same colour gives it round corners.
export const PROMPT_BLOCK_BACKGROUND = '#80808017';

export function promptPreview(text: string): { text: string; isCut: boolean } {
  if (text.length <= PROMPT_PREVIEW_CHARS) return { text, isCut: false };
  const cut = text.lastIndexOf(' ', PROMPT_PREVIEW_CHARS);
  return {
    text: `${text.slice(0, cut > 0 ? cut : PROMPT_PREVIEW_CHARS).trimEnd()}…`,
    isCut: true,
  };
}

/**
 * The short form of a fix that starts with a command. The prompt box shows the full text
 * before the person sends it, so the card names only the command.
 */
export function commandSummary(
  check: Pick<LiveCheckView, 'id' | 'prompt'>,
): { name: string } | null {
  const match = /^(\/\S+)\s*(.*)$/s.exec(check.prompt);
  if (match === null) return null;
  const command = match[1] ?? '';
  const rest = match[2] ?? '';
  if (check.id !== 'K1') return { name: rest === '' ? command : `${command} ${rest}` };
  // A plugin skill runs by its full name; the card shows the short name after the colon.
  return {
    name: `/${command.slice(command.lastIndexOf(':') + 1).replace(/^\//, '')}`,
  };
}

// The label of a fix button. An armed button asks for the second press itself, so no note is necessary.
export function fixLabel(check: LiveCheckView, armedId: LiveCheckId | null): string {
  if (armedId !== check.id) return check.actionLabel;
  return check.actionKind === 'compact' ? 'Confirm compact' : 'Confirm send';
}

// The blank label of the press target over a chevron.
export const CHEVRON_HIT = '\u2003';

// The strip's close glyph on the desktop: the lucide x. A side-by-side capture of the host's own
// close control set its thinner stroke, its dimmer ink and its position. The Svg is as wide as the
// hover, and the viewBox centers the glyph in it. A Box moves in whole cells only, so a negative
// right margin of one cell puts the glyph center where the host's close control is.

export function tileSvg(id: LiveCheckId, isLive: boolean): string {
  const tile = TILE[id] ?? { tint: '#A8A8A8', paths: ['M12 3v18', 'M3 12h18'] };
  const tint = isLive ? tile.tint : TILE_NEUTRAL;
  const icon = tile.paths.map((d) => `<path d="${d}"/>`).join('');
  return (
    '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32">' +
    `<circle cx="16" cy="16" r="16" fill="${tint}" fill-opacity="0.16"/>` +
    `<g transform="translate(7 7) scale(0.75)" fill="none" stroke="${tint}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${icon}</g>` +
    '</svg>'
  );
}

// Each check has its own dot colour, the tint of its desktop tile, so two checks
// in one area do not look the same in the terminal.
export function dotColor(check: LiveCheckView): string {
  return check.stage === 'active' || check.stage === 'watching'
    ? (TILE[check.id]?.tint ?? 'subtle')
    : 'subtle';
}

export function areaOf(check: LiveCheckView): { label: string; color: string } {
  return AREAS[check.area] ?? { label: check.area, color: 'subtle' };
}

export function metricText(check: LiveCheckView): string {
  if (check.source === 'session') return check.metric;
  if (check.stage === 'building') return check.progress ?? '';
  // A resolved row names the outcome in place of the metric. The pane shows this session only, so it does not say so.
  if (check.stage === 'resolved')
    return check.isStoppedAfterFix ? '✓ Fixed' : '✓ Stopped by itself';
  return check.metric;
}

// The rows the pane draws, in order: the checks of each open group.
export function visibleRowIds(
  shown: readonly LiveCheckView[],
  openGroups: readonly string[],
): LiveCheckId[] {
  return GROUPS.flatMap((group) =>
    !group.isFolded || openGroups.includes(group.title)
      ? shown.filter((check) => group.stages.includes(check.stage))
      : [],
  ).map((check) => check.id);
}

export function toggleGroup(openGroups: readonly string[], title: string): string[] {
  return openGroups.includes(title)
    ? openGroups.filter((open) => open !== title)
    : [...openGroups, title];
}

export function renderStripView(
  ui: PaneUi,
  e: RenderInput<'AbovePrompt'>,
  checks: readonly LiveCheckView[],
  resolvedLine: readonly LiveCheckId[],
  isDismissed: boolean,
  newCount: number,
  armedId: LiveCheckId | null,
  actions: StripActions,
  withBrand = true,
) {
  const { Box, Button, Text } = ui;
  // The strip shows only the checks that still need a fix. A watched check has its fix in flight,
  // so the footer chip counts it and the strip goes away when only watched checks remain.
  const active = liveChecks({
    checks: [...checks],
    newIds: [],
    resolvedNow: [],
  }).filter((check) => check.stage === 'active');
  const columns = e.props.bodyColumns;
  const top = active[0];

  if (top === undefined) {
    const [firstId] = resolvedLine;
    if (firstId === undefined) return null;
    // The strip names the first check that resolved, and counts the others.
    const firstLabel = checks.find((check) => check.id === firstId)?.label ?? 'Check';
    const others = resolvedLine.length > 1 ? ` +${resolvedLine.length - 1}` : '';
    const resolved = `${firstLabel}${others} resolved`;
    const style = bandStyle(e.surface, 'resolved');
    // The resolved strip is quieter than the warning strip: the antiburn name, then the outcome
    // in the pass cyan of the Resolved icon, as text and not in a box.
    if (e.surface !== 'terminal' && 'Svg' in ui) {
      return (
        <Box flexDirection="row" alignItems="center" gap={1} paddingX={1}>
          {withBrand &&
            (columns < 80 ? (
              <ui.Svg
                key="brand-mark"
                source={BRAND_MARK_SVG}
                alt="antiburn"
                width={14}
                height={14}
              />
            ) : (
              <ui.Svg
                key="brand"
                source={WORDMARK_SVG}
                alt="antiburn"
                width={WORDMARK_WIDTH}
                height={WORDMARK_HEIGHT}
              />
            ))}
          <ui.Svg
            key="tick"
            source={GROUP_ICON['✓ Resolved'] ?? ''}
            alt="Resolved"
            width={14}
            height={14}
          />
          <ui.Svg
            key="resolved"
            source={metricPartSvg(resolved, 'pass')}
            alt={resolved}
            width={metricPartWidth(resolved)}
            height={METRIC_HEIGHT}
          />
        </Box>
      );
    }
    return (
      <Box {...style.box} paddingX={1}>
        <Text color={style.text}>
          {withBrand && (
            <Text bold color={style.label}>
              antiburn{' '}
            </Text>
          )}
          <Text color={style.mark}>✓ {resolved}</Text>
        </Text>
      </Box>
    );
  }

  const count = `${active.length}${top.source === 'session' ? ' checks' : ' to fix'}`;
  // A hotkey presses a strip Button only from the terminal: from a focused band, or as a bare
  // digit in an empty prompt. The desktop prompt sends a digit to Claude, so the desktop gets no hotkeys.
  const hotkeyFor = (digit: string) => (e.surface === 'terminal' ? digit : undefined);
  // A dismissed strip stays away until a new check arrives. The footer chip still counts the open checks.
  if (isDismissed) return null;

  const fresh = newCount > 0 ? ` (${newCount} new)` : '';
  const isTerminal = e.surface === 'terminal';
  // The terminal does not draw a Button's hotkey, so the label shows the digit.
  const keyed = (digit: string, label: string) =>
    isTerminal ? `${digit} ${label}` : label;
  const viewLabel = columns < 80 ? 'View' : 'View checks';
  const buttons = `${top.actionLabel} ${viewLabel} x`.length + 22;
  // The strip has four forms, the same on each surface. Each form keeps the antiburn name and the
  // top check name, because the fix button acts on that check.
  //   100 columns and more: the wordmark, the count with "(N new)", the check name and its metric.
  //   80 to 99 columns: the metric goes, because the desktop wraps a Text that does not fit.
  //   "(N new)" stays from 80 columns, and on the terminal while it fits beside the check name.
  //   56 to 79 columns: the desktop shows the mark in place of the wordmark, and the buttons get shorter.
  //   Less than 56 columns: the buttons move to a second row, so the check name keeps the full width.
  //   The terminal counts its cells, so it also stacks when the check name does not fit beside the buttons.
  // The check name is bold and its metric is quieter: the fix acts on the check, and the metric only sizes it.
  const hasMetric = columns >= 100;
  const isStacked =
    columns < 56 ||
    (isTerminal &&
      columns - buttons - (withBrand ? 'antiburn'.length : 0) < top.label.length + 3);
  const metric = ` · ${metricText(top)}`;
  const total = columns >= 80 ? `${count}${fresh}` : count;
  const style = bandStyle(e.surface, 'warning');
  const stack = (head: RenderNode) =>
    isStacked ? (
      <Box flexDirection="column" gap={isTerminal ? 0 : 1} {...style.box} paddingX={1}>
        {head}
        <Box key="actions" flexDirection="row" justifyContent="flex-end">
          {stripButtons(top)}
        </Box>
      </Box>
    ) : (
      <Box
        flexDirection="row"
        justifyContent="space-between"
        alignItems="center"
        gap={1}
        {...style.box}
        paddingX={1}
      >
        {head}
        {stripButtons(top)}
      </Box>
    );
  if (!isTerminal) {
    // A Text that holds other Texts wraps on the desktop, but a plain Text truncates in a shrinking Box.
    // So the check name and the metric are two plain Texts.
    const Svg = 'Svg' in ui ? ui.Svg : undefined;
    return stack(
      <Box
        key="brand-head"
        flexDirection="row"
        alignItems="center"
        gap={1}
        flexShrink={1}
        minWidth={0}
      >
        {withBrand &&
          Svg !== undefined &&
          (columns < 80 ? (
            <Svg
              key="brand-mark"
              source={BRAND_MARK_SVG}
              alt="antiburn"
              width={14}
              height={14}
            />
          ) : (
            <Svg
              key="brand"
              source={WORDMARK_SVG}
              alt="antiburn"
              width={WORDMARK_WIDTH}
              height={WORDMARK_HEIGHT}
            />
          ))}
        {Svg !== undefined && (
          <Svg
            key="count"
            source={countPillSvg(total)}
            alt={total}
            width={countPillWidth(total)}
            height={COUNT_PILL_HEIGHT}
          />
        )}
        <Box key="headline" flexDirection="row" flexShrink={1} minWidth={0}>
          <Text bold wrap="truncate-end">
            {top.label}
          </Text>
          {hasMetric && (
            <Text dimColor wrap="truncate-end">
              {metric}
            </Text>
          )}
        </Box>
      </Box>,
    );
  }
  // The count sits in a box, as the desktop pill does. A space on each side gives the box its margin.
  // The box shows only when the check name still fits beside it.
  const room =
    (isStacked ? columns - 2 : columns - buttons) - (withBrand ? 'antiburn'.length : 0);
  const rest = hasMetric ? `${top.label}${metric}` : top.label;
  const fits = (text: string) => room - text.length - 3 >= top.label.length + 1;
  const boxText = [`${count}${fresh}`, count].find(fits);
  const bracket = boxText === undefined ? '' : ` ${boxText} `;
  const lead = bracket === '' ? ' · ' : ' ';
  const tail = truncate(
    `${lead}${rest}`,
    room - (bracket === '' ? 0 : bracket.length + 1),
  );
  const labelEnd = lead.length + top.label.length;
  return stack(
    <Box key="brand-head" flexShrink={1} minWidth={0}>
      <Text wrap="truncate-end" color={style.text}>
        {withBrand && (
          <Text bold color={style.label}>
            antiburn
          </Text>
        )}
        {bracket !== '' && ' '}
        {bracket !== '' && <Text {...style.count}>{bracket}</Text>}
        {tail.slice(0, lead.length)}
        <Text bold>{tail.slice(lead.length, labelEnd)}</Text>
        {tail.length > labelEnd && (
          <Text color={style.metric}>{tail.slice(labelEnd)}</Text>
        )}
      </Text>
    </Box>,
  );

  function stripButtons(top: LiveCheckView) {
    return (
      <Box flexDirection="row" gap={1} flexShrink={0}>
        {top.source !== 'session' && (
          <Button
            key="fix"
            hotkey={hotkeyFor('1')}
            variant="primary"
            label={keyed('1', fixLabel(top, armedId))}
            onPress={() => actions.act(top.id)}
          />
        )}
        <Button
          key="view"
          hotkey={hotkeyFor('2')}
          label={keyed('2', viewLabel)}
          onPress={() => actions.view()}
        />
        {renderDismiss()}
      </Box>
    );
  }

  // The desktop does not draw its native close control for role="dismiss" yet (2.1.296), and it draws
  // a Button label as text. So the desktop draws the close glyph as an Svg, and a blank plain Button
  // lies on top of it to take the press, as on a card chevron. The terminal draws a dim plain ×.
  function renderDismiss() {
    const onPress = () => actions.dismiss();
    if (isTerminal || !('Svg' in ui))
      return <Button key="dismiss" plain dimColor label="×" onPress={onPress} />;
    const { Svg } = ui;
    return (
      <Box
        key="dismiss-box"
        position="relative"
        flexDirection="row"
        alignItems="center"
        justifyContent="center"
        marginRight={-1}
      >
        <Svg
          source={DISMISS_SVG}
          alt="Dismiss"
          width={DISMISS_WIDTH}
          height={DISMISS_SIZE}
        />
        {renderOverlayButton(ui, 'dismiss', DISMISS_HIT, onPress)}
      </Box>
    );
  }
}

type PaneUi = ReturnType<EngineInterface['ui']['resolve']>;
type StripActions = {
  act: (id: LiveCheckId) => Promise<void>;
  view: () => Promise<void>;
  dismiss: () => Promise<void>;
};
type PaneActions = {
  centredLine: (e: PaneEvent, key: string, line: string) => RenderNode;
  toggleGroup: (title: string) => Promise<void>;
  toggleCard: (id: LiveCheckId) => Promise<void>;
  toggleRow: (id: LiveCheckId) => Promise<void>;
  act: (id: LiveCheckId, from: 'card') => Promise<void>;
  hide: (id: LiveCheckId) => Promise<void>;
  toggleFullPrompt: (id: LiveCheckId) => Promise<void>;
};

function renderCardDetailView(
  ui: PaneUi,
  e: PaneEvent,
  check: LiveCheckView,
  fullPromptId: LiveCheckId | null,
  toggleFullPrompt: (id: LiveCheckId) => Promise<void>,
) {
  const { Box, Text } = ui;
  const isLive = check.stage === 'active' || check.stage === 'watching';
  return (
    <Box key={`more-${check.id}`} flexDirection="column" gap={1}>
      {check.cause !== null && (
        <Text>
          <Text bold>Cause</Text>
          <Text dimColor> · </Text>
          {check.cause}
        </Text>
      )}
      {isLive &&
        renderFixView(ui, e, check, fullPromptId === check.id, toggleFullPrompt)}
    </Box>
  );
}

function renderFixView(
  ui: PaneUi,
  e: PaneEvent,
  check: LiveCheckView,
  isFull: boolean,
  toggleFullPrompt: (id: LiveCheckId) => Promise<void>,
) {
  const { Box, Button, Text } = ui;
  const Svg = e.surface === 'desktop' && 'Svg' in ui ? ui.Svg : undefined;
  const isFill = check.actionKind === 'fill';
  const isAdvice = check.actionKind === 'advise';
  const summary = isFill || isAdvice ? commandSummary(check) : null;
  if (summary !== null) {
    const lead = isAdvice ? 'Next time, start with' : 'Fills';
    const end = isAdvice ? 'It may take fewer steps.' : 'You send it.';
    if (Svg === undefined)
      return (
        <Text key={`fix-${check.id}`}>
          {lead} <Text bold>{summary.name}</Text>. <Text dimColor>{end}</Text>
        </Text>
      );
    return (
      <Box
        key={`fix-${check.id}`}
        flexDirection="row"
        alignItems="center"
        columnGap={1}
        flexWrap="wrap"
      >
        <Text>{lead}</Text>
        <Svg
          key={`command-pill-${check.id}-${summary.name}`}
          source={skillPillSvg(summary.name)}
          alt={check.prompt.split(/\s/)[0] ?? summary.name}
          width={skillPillWidth(summary.name)}
          height={SKILL_PILL_HEIGHT}
        />
        <Text dimColor>{end}</Text>
      </Box>
    );
  }
  const label =
    check.actionKind === 'compact'
      ? 'Compact instructions'
      : isFill
        ? 'Prompt'
        : 'Fix prompt';
  const preview = promptPreview(check.prompt);
  const shown = isFull ? check.prompt : preview.text;
  const isDesktop = e.surface !== 'terminal';
  return (
    <Box key={`fix-${check.id}`} flexDirection="column" gap={isDesktop ? 1 : 0}>
      <Text bold>{label}</Text>
      {isDesktop ? (
        <Box
          paddingX={1}
          backgroundColor={PROMPT_BLOCK_BACKGROUND}
          borderStyle="round"
          borderColor={PROMPT_BLOCK_BACKGROUND}
        >
          <Text>{shown}</Text>
        </Box>
      ) : (
        <Box paddingLeft={2}>
          <Text>{shown}</Text>
        </Box>
      )}
      {preview.isCut && (
        <Button
          key={`full-${check.id}`}
          plain
          label={isFull ? 'Show less' : 'Show all'}
          onPress={() => toggleFullPrompt(check.id)}
        />
      )}
      {isFill && <Text dimColor>This fills the prompt box. You send it.</Text>}
    </Box>
  );
}

function renderOverlayButton(
  ui: PaneUi,
  key: string,
  label: string,
  onPress: () => Promise<void>,
) {
  const { Box, Button } = ui;
  return (
    <Box
      position="absolute"
      top={0}
      left={0}
      right={0}
      bottom={0}
      flexDirection="row"
      alignItems="center"
      justifyContent="center"
    >
      <Button key={key} plain label={label} onPress={onPress} />
    </Box>
  );
}

function renderChevronView(
  ui: PaneUi,
  e: PaneEvent,
  key: string,
  alt: string,
  isOpen: boolean,
  isTurning: boolean,
  onPress: () => Promise<void>,
  size = 20,
) {
  const { Box } = ui;
  if (!('Svg' in ui)) return null;
  const { Svg } = ui;
  return (
    <Box
      key={`chevron-${key}`}
      position="relative"
      flexDirection="row"
      alignItems="center"
      justifyContent="center"
    >
      <Svg
        key={`chevron-svg-${key}-${isOpen ? 'open' : 'closed'}`}
        source={chevronSvg(isOpen, isTurning, size)}
        alt={alt}
        width={size}
        height={size}
      />
      {renderOverlayButton(ui, key, CHEVRON_HIT, onPress)}
    </Box>
  );
}

function terminalMetricView(ui: PaneUi, check: LiveCheckView): RenderNode {
  const { Text } = ui;
  const text = metricText(check);
  const skill = check.stage === 'resolved' ? null : leadingSkill(text);
  return skill === null ? (
    text
  ) : (
    <Text>
      <Text bold>{skill.name}</Text> {skill.rest}
    </Text>
  );
}

export function renderListView(
  ui: PaneUi,
  e: PaneEvent,
  shown: readonly LiveCheckView[],
  hasHidden: boolean,
  openGroups: readonly string[],
  armedId: LiveCheckId | null,
  openId: LiveCheckId | null,
  turned: string | null,
  entered: readonly LiveStage[],
  connectionText: string | undefined,
  fullPromptId: LiveCheckId | null,
  actions: PaneActions,
) {
  const { Box, Button, Text } = ui;
  // Only the desktop draws an Svg, so only the desktop draws cards.
  const Svg = e.surface === 'desktop' && 'Svg' in ui ? ui.Svg : undefined;

  // The Checks heading counts the active checks. Watched checks have their own section and count.
  const live = shown.filter((check) => check.stage === 'active');
  // design.md: counts sit in muted badges beside section headings.
  const badge = (count: number) =>
    Svg !== undefined ? (
      <Svg
        key={`badge-${count}`}
        source={badgeSvg(count)}
        alt={`${count}`}
        width={20}
        height={20}
      />
    ) : (
      <Text dimColor>{count}</Text>
    );
  // The focus ring starts on the first row, so Enter opens it without a move first.
  const firstId = visibleRowIds(shown, openGroups)[0];
  // The terminal mock is a bare list: no Signals title and no area before the metric.
  const isTerminal = e.surface === 'terminal';
  return (
    <Box flexDirection="column" gap={1} flexGrow={1}>
      {Svg !== undefined ? (
        // The heading and its count badge. The pane shows this session only, so it names no period, as the terminal does not.
        <Box key="checks-title" flexDirection="row" gap={1} alignItems="center">
          {live.length > 0 && (
            <Svg
              key="checks-icon"
              source={GROUP_ICON.Signals ?? ''}
              alt="Active checks"
              width={14}
              height={14}
            />
          )}
          <Text bold>Checks</Text>
          {(shown.length > 0 || hasHidden) && badge(live.length)}
        </Box>
      ) : (
        // One heading style for each section: a glyph, a bold title, then a dim count.
        // As on the desktop, the Checks glyph shows only while a check is active.
        <Text>
          {live.length > 0 && <Text color={METRIC}>● </Text>}
          <Text bold>Checks</Text>
          {(shown.length > 0 || hasHidden) && <Text dimColor> {live.length}</Text>}
        </Text>
      )}
      {connectionText && <Text key="session-checks-connection" dimColor>{connectionText}</Text>}
      {shown.length === 0 && !hasHidden ? (
        // The empty pane says what the pane is for, a little above the middle of the free space:
        // a block at the exact middle looks low. Text has no alignment, so each line is its own centred Text.
        <Box
          key="empty"
          flexDirection="column"
          flexGrow={1}
          alignItems="center"
          justifyContent="center"
          gap={2}
          marginBottom={emptyLift(e)}
        >
          {Svg !== undefined && (
            <Svg
              key="empty-icon"
              source={EMPTY_ICON}
              alt="No checks yet"
              width={40}
              height={40}
            />
          )}
          <Box flexDirection="column" alignItems="center">
            <Text bold>No checks yet</Text>
            {actions.centredLine(e, 'watching', 'antiburn is watching this session.')}
            {actions.centredLine(
              e,
              'appear',
              "Checks appear when there's a better way to work.",
            )}
          </Box>
        </Box>
      ) : (
        live.length === 0 && (
          <Text key="empty" dimColor>
            Nothing to fix right now.
          </Text>
        )
      )}
      {GROUPS.map((group) => {
        const rows = shown.filter((check) => group.stages.includes(check.stage));
        if (rows.length === 0) return null;
        const isOpen = !group.isFolded || openGroups.includes(group.title);
        const baseIcon = Svg !== undefined ? GROUP_ICON[group.title] : undefined;
        const isEntering = group.stages.some((stage) => entered.includes(stage));
        const icon =
          baseIcon === undefined ? undefined : sectionIcon(baseIcon, isEntering);
        const title =
          Svg !== undefined ? (DESKTOP_TITLE[group.title] ?? group.title) : group.title;
        return (
          // A later section starts one row lower than the gap between cards, so it reads as a new section.
          // On the terminal a heading sits on its cards: the gap goes between the cards only.
          <Box
            key={group.title}
            flexDirection="column"
            gap={isTerminal ? 0 : 1}
            marginTop={group.title === 'Signals' ? undefined : 1}
          >
            {Svg !== undefined && group.title === 'Signals' ? null : Svg !==
              undefined ? (
              <Box
                key={`heading-${group.stages[0] ?? ''}`}
                flexDirection="row"
                justifyContent="space-between"
                alignItems="center"
              >
                <Box flexDirection="row" gap={1} alignItems="center">
                  {icon !== undefined && (
                    <Svg
                      key={`icon-${group.stages[0] ?? ''}`}
                      source={icon}
                      alt={title}
                      width={14}
                      height={14}
                    />
                  )}
                  {/* The title is Text, as the Checks title is: a native button adds padding and moves the baseline.
                      The chevron opens and closes the section. */}
                  <Text bold>{title}</Text>
                  {badge(rows.length)}
                  {/* The chevron sits beside the title it opens, not at the far edge. */}
                  {group.isFolded &&
                    renderChevronView(
                      ui,
                      e,
                      `group-chevron-${group.stages[0] ?? ''}`,
                      isOpen ? `Hide ${title}` : `Show ${title}`,
                      isOpen,
                      turned === group.title,
                      () => actions.toggleGroup(group.title),
                    )}
                </Box>
              </Box>
            ) : group.isFolded ? (
              <Button
                key={`group-${group.stages[0] ?? ''}`}
                plain
                label={`${group.title} ${rows.length} ${isOpen ? '▾' : '▸'}`}
                onPress={() => actions.toggleGroup(group.title)}
              />
            ) : (
              // The terminal names the watched section in the chip's symbol. The Checks title heads the active checks.
              group.title !== 'Signals' && (
                <Text key={`heading-${group.stages[0] ?? ''}`}>
                  <Text color="warning">⧗</Text> <Text bold>{group.title}</Text>
                  <Text dimColor> {rows.length}</Text>
                </Text>
              )
            )}
            {isOpen &&
              Svg !== undefined &&
              rows.map((check) => {
                const isLive = !group.isFolded;
                // The open card takes a stronger fill, so the reader sees which card is open.
                const fill =
                  openId === check.id ? CARD_OPEN_BACKGROUND : CARD_BACKGROUND;
                return (
                  <Box
                    key={`card-${check.id}`}
                    flexDirection="row"
                    alignItems="flex-start"
                    gap={1}
                    padding={1}
                    backgroundColor={fill}
                    // A round border in the card colour gives the card round corners.
                    borderStyle="round"
                    borderColor={fill}
                  >
                    <Svg
                      key={`tile-${check.id}`}
                      source={tileSvg(check.id, isLive)}
                      alt={`${check.label} icon`}
                      width={32}
                      height={32}
                    />
                    {(() => {
                      const isOpenCard = openId === check.id;
                      // No hotkeys on cards: four cards would share one F key.
                      const fix =
                        check.stage === 'active' && check.source !== 'session' ? (
                          <Button
                            key={`card-fix-${check.id}`}
                            // The armed button takes the primary style, because its press sends the prompt.
                            variant={armedId === check.id ? 'primary' : undefined}
                            label={fixLabel(check, armedId)}
                            onPress={() => actions.act(check.id, 'card')}
                          />
                        ) : null;
                      const hideButton =
                        check.stage === 'active' && isOpenCard ? (
                          <Button
                            key={`hide-${check.id}`}
                            plain
                            dimColor
                            label={check.source === 'session' ? 'Dismiss this check' : 'Ignore this session'}
                            onPress={() => actions.hide(check.id)}
                          />
                        ) : null;
                      // The chevron alone opens the card. It sits at the top right, on the title line, so it has a
                      // fixed width at every pane width and stays in place when the card opens.
                      const toggle = renderChevronView(
                        ui,
                        e,
                        `row-${check.id}`,
                        isOpenCard ? 'Show less' : 'Show more',
                        isOpenCard,
                        turned === check.id,
                        () => actions.toggleCard(check.id),
                        14,
                      );
                      // The reason takes one line. Show more gives the full sentence.
                      const reason = isOpenCard || check.source === 'session' ? (
                        <Text dimColor>{check.sentence}</Text>
                      ) : (
                        <Text dimColor wrap="truncate-end">
                          {splitSentence(check.sentence).reason}
                        </Text>
                      );
                      // A watched card has no fix, so its status takes the place of the buttons.
                      const status =
                        check.stage === 'watching' ? (
                          <Text key={`note-${check.id}`} dimColor>
                            {check.checking ? 'Checking this assessment' : check.source === 'session' ? 'Waiting for an assessment' : WATCHING_STATUS}
                          </Text>
                        ) : check.checking ? (
                          <Text key={`note-${check.id}`} dimColor>Checking for an updated result</Text>
                        ) : null;
                      const title = (
                        <Text bold dimColor={group.isFolded}>
                          {check.label}
                        </Text>
                      );
                      const metricLine = metricText(check);
                      const skill =
                        check.id === 'K1' && isLive ? leadingSkill(metricLine) : null;
                      const metric =
                        // A watched check has no fix, so its metric is neutral and only the hourglass gives its state.
                        skill === null &&
                        (check.stage === 'active' || check.stage === 'resolved') ? (
                          <Box
                            key={`metric-${check.id}`}
                            flexDirection="row"
                            alignItems="center"
                            columnGap={1}
                            flexWrap="wrap"
                          >
                            {check.stage === 'resolved' && (
                              // A resolved outcome takes the CircleCheck of the Resolved heading in place of the ✓ glyph.
                              <Svg
                                key={`tick-${check.id}`}
                                source={GROUP_ICON['✓ Resolved'] ?? ''}
                                alt="Resolved"
                                width={12}
                                height={12}
                              />
                            )}
                            {metricParts(
                              check.stage === 'active'
                                ? metricLine
                                : metricLine.replace(/^✓ /, ''),
                            ).map((part, index) => (
                              <Svg
                                key={`metric-${check.id}-${index}`}
                                source={metricPartSvg(
                                  part,
                                  check.stage === 'active' ? 'failure' : 'pass',
                                )}
                                alt={part}
                                width={metricPartWidth(part)}
                                height={METRIC_HEIGHT}
                              />
                            ))}
                          </Box>
                        ) : skill === null ? (
                          <Text color="subtle">{metricLine}</Text>
                        ) : (
                          // The skill name is code, so it draws in a monospace pill.
                          <Box
                            key={`skill-metric-${check.id}`}
                            flexDirection="row"
                            alignItems="center"
                            columnGap={1}
                            flexWrap="wrap"
                          >
                            <Svg
                              key={`skill-${check.id}-${skill.name}`}
                              source={skillPillSvg(skill.name)}
                              alt={skill.name}
                              width={skillPillWidth(skill.name)}
                              height={SKILL_PILL_HEIGHT}
                            />
                            {metricParts(skill.rest).map((part, index) => (
                              <Svg
                                key={`metric-${check.id}-${index}`}
                                source={metricPartSvg(part)}
                                alt={part}
                                width={metricPartWidth(part)}
                                height={METRIC_HEIGHT}
                              />
                            ))}
                          </Box>
                        );
                      const more = check.source !== 'session' && isOpenCard
                        ? renderCardDetailView(
                            ui,
                            e,
                            check,
                            fullPromptId,
                            actions.toggleFullPrompt,
                          )
                        : null;
                      // One layout at every width: the title with its chevron, the metric, the reason, the details,
                      // and the actions last.
                      return (
                        // minWidth 0 lets the column shrink below its widest row, so a long reason truncates inside the card.
                        <Box
                          flexDirection="column"
                          flexGrow={1}
                          flexShrink={1}
                          minWidth={0}
                          gap={1}
                        >
                          <Box
                            flexDirection="row"
                            justifyContent="space-between"
                            alignItems="flex-start"
                            gap={1}
                          >
                            <Box flexDirection="column" flexShrink={1} minWidth={0}>
                              {title}
                              {metric}
                            </Box>
                            <Box flexShrink={0}>{toggle}</Box>
                          </Box>
                          {reason}
                          {more}
                          {/* A narrow pane wraps Ignore this session under the fix. */}
                          {(fix !== null || hideButton !== null || status !== null) && (
                            <Box
                              flexDirection="row"
                              flexWrap="wrap"
                              alignItems="center"
                              gap={1}
                            >
                              {fix}
                              {hideButton}
                              {status}
                            </Box>
                          )}
                        </Box>
                      );
                    })()}
                  </Box>
                );
              })}
            {isOpen &&
              Svg === undefined &&
              rows.map((check, index) => {
                const area = areaOf(check);
                const isOpenCard = openId === check.id;
                // The open card takes the f and h keys. Only one card is open, so the keys do not clash.
                return (
                  <Box
                    key={`item-${check.id}`}
                    flexDirection="column"
                    marginTop={isTerminal && index > 0 ? 1 : undefined}
                  >
                    {/* The chevron alone opens the card, as on the desktop. A one-glyph plain Button is a control by itself. */}
                    {/* One cell of right padding puts the chevron under the pane's close button, off the edge. */}
                    <Box
                      flexDirection="row"
                      justifyContent="space-between"
                      gap={1}
                      paddingRight={1}
                    >
                      <Text wrap="truncate-end">
                        <Text color={dotColor(check)}>●</Text>{' '}
                        <Text bold dimColor={group.isFolded}>
                          {check.label}
                        </Text>
                      </Text>
                      <Button
                        key={`row-${check.id}`}
                        plain
                        autoFocus={check.id === firstId ? true : undefined}
                        dimColor
                        label={isOpenCard ? '▾' : '▸'}
                        onPress={() => actions.toggleRow(check.id)}
                      />
                    </Box>
                    <Text
                      color={
                        check.stage === 'resolved'
                          ? PASS_TEXT_TERMINAL
                          : group.isFolded || check.stage === 'watching'
                            ? 'subtle'
                            : METRIC
                      }
                    >
                      {isTerminal
                        ? terminalMetricView(ui, check)
                        : `${area.label} · ${metricText(check)}`}
                    </Text>
                    {isOpenCard ? (
                      <Box flexDirection="column" gap={1}>
                        <Box flexDirection="column">
                          <Text dimColor>{check.sentence}</Text>
                          {check.cause !== null && (
                            <Text>
                              <Text bold>Cause</Text>
                              <Text dimColor> · {check.cause}</Text>
                            </Text>
                          )}
                        </Box>
                        {(check.stage === 'active' || check.stage === 'watching') && check.source !== 'session' &&
                          renderFixView(
                            ui,
                            e,
                            check,
                            fullPromptId === check.id,
                            actions.toggleFullPrompt,
                          )}
                        {check.stage === 'active' && check.source !== 'session' && (
                          <Box flexDirection="row" gap={1}>
                            <Button
                              key={`card-fix-${check.id}`}
                              hotkey="f"
                              // The armed button takes the primary style, because its press sends the prompt.
                              variant={armedId === check.id ? 'primary' : undefined}
                              label={`f ${fixLabel(check, armedId)}`}
                              onPress={() => actions.act(check.id, 'card')}
                            />
                            <Button
                              key={`hide-${check.id}`}
                              hotkey="h"
                              label="h Ignore this session"
                              onPress={() => actions.hide(check.id)}
                            />
                          </Box>
                        )}
                      </Box>
                    ) : check.source === 'session' ? (
                      <Text dimColor wrap="truncate-end">{check.sentence}</Text>
                    ) : (
                      <Text dimColor wrap="truncate-end">
                        {splitSentence(check.sentence).reason}
                      </Text>
                    )}
                        {check.stage === 'watching' && (
                          <Text dimColor>{check.checking ? 'Checking this assessment' : check.source === 'session' ? 'Waiting for an assessment' : WATCHING_STATUS}</Text>
                        )}
                        {check.stage === 'active' && check.checking && <Text dimColor>Checking for an updated result</Text>}
                        {check.source === 'session' && isOpenCard && (
                          <Button
                            key={`hide-${check.id}`}
                            hotkey="h"
                            label="h Dismiss this check"
                            onPress={() => actions.hide(check.id)}
                          />
                        )}
                  </Box>
                );
              })}
          </Box>
        );
      })}
    </Box>
  );
}

function truncate(text: string, room: number): string {
  if (text.length <= room) return text;
  return `${text.slice(0, Math.max(0, room - 1))}…`;
}

export function splitSentence(sentence: string): {
  reason: string;
  rest: string;
} {
  const end = sentence.indexOf('. ');
  if (end < 0) return { reason: sentence, rest: '' };
  return { reason: sentence.slice(0, end + 1), rest: sentence.slice(end + 2) };
}

function chevronSvg(isOpen: boolean, isTurning: boolean, size: number): string {
  const [from, to] = isOpen ? [0, 180] : [180, 0];
  const turn = isTurning
    ? `g{animation:turn 180ms cubic-bezier(.2,0,0,1) both}@keyframes turn{from{transform:rotate(${from}deg)}to{transform:rotate(${to}deg)}}` +
      '@media (prefers-reduced-motion:reduce){g{animation:none}}'
    : '';
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 24 24">` +
    `<style>g{transform-origin:12px 12px;transform:rotate(${to}deg)}path{stroke:#000;stroke-opacity:.55}` +
    `@media (prefers-color-scheme:dark){path{stroke:#fff;stroke-opacity:.6}}${turn}</style>` +
    '<g><path d="m6 9 6 6 6-6" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></g>' +
    '</svg>'
  );
}

const DISMISS_SIZE = 14;
const DISMISS_WIDTH = 24;
// The blank label of the press target over the close glyph. The host sets the width of the hover
// to the width of the box, so one em is enough and the box is 24 px wide, as the host's close control is.
const DISMISS_HIT = '\u2003';
const DISMISS_SVG =
  `<svg xmlns="http://www.w3.org/2000/svg" width="${DISMISS_WIDTH}" height="${DISMISS_SIZE}" viewBox="-8.58 0 41.16 24">` +
  '<style>path{stroke:#000;stroke-opacity:.4}@media (prefers-color-scheme:dark){path{stroke:#fff;stroke-opacity:.45}}</style>' +
  '<path d="M18 6 6 18M6 6l12 12" fill="none" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>' +
  '</svg>';

function emptyLift(e: PaneEvent): number {
  return e.props.placement === 'dock' ? Math.floor(e.props.scroll.bodyRows / 10) : 0;
}

/** Fields of a tool call input that do not tell two calls apart. The model writes a new `description` for each retry. */
const CALL_NOISE = new Set([
  'tool',
  'tool_use_id',
  'agentId',
  'consent',
  'description',
]);

/** Builds a bounded key for one call, so a large file body does not cost a full copy. */
export function callKey(input: Readonly<Record<string, unknown>>): string {
  const parts: string[] = [];
  for (const [name, value] of Object.entries(input)) {
    if (CALL_NOISE.has(name)) continue;
    const text = typeof value === 'string' ? value : (JSON.stringify(value) ?? '');
    parts.push(
      `${name}=${text.length > 400 ? `${text.length}:${text.slice(0, 200)}` : text}`,
    );
  }
  return parts.join('|');
}

export function centredLineView(ui: PaneUi, e: PaneEvent, key: string, line: string) {
  const { Box, Text } = ui;
  const isTerminal = e.surface === 'terminal';
  const words = line.split(' ');
  return (
    <Box
      key={key}
      flexDirection="row"
      flexWrap="wrap"
      justifyContent="center"
      columnGap={isTerminal ? 1 : 0}
    >
      {words.map((word, index) => (
        <Text key={`${key}-${index}`} dimColor>
          {isTerminal || index === words.length - 1 ? word : `${word}\u00A0`}
        </Text>
      ))}
    </Box>
  );
}

// A docked pane fills the body rows, so the footer reaches the bottom edge. The desktop
// pane body grows with its content, so a percent height does not fill it. On the
// terminal, a fixed height makes Ink shrink the rows of an open card, so the terminal
// takes it as a minimum and a taller tree scrolls in the pane window. On the desktop,
// a minimum lets the footer move below the visible bottom, so the desktop keeps a fixed
// height. An inline pane keeps its own height.
function paneHeight(e: PaneEvent): { height?: number; minHeight?: number } {
  if (e.props.placement !== 'dock') return {};
  const rows = e.props.scroll.bodyRows;
  return e.surface === 'terminal' ? { minHeight: rows } : { height: rows };
}

type PaneViewActions = {
  nextTourStep: () => Promise<string | null>;
  endTour: () => Promise<void>;
};
export function renderPaneView(
  ui: PaneUi,
  e: PaneEvent,
  body: RenderNode,
  hasCard: boolean,
  mark: number,
  tourBeat: number | null,
  tourStepCount: number,
  actions: PaneViewActions,
) {
  const { Box, Button, Link, Text } = ui;
  const Svg = e.surface === 'desktop' && 'Svg' in ui ? ui.Svg : undefined;
  const isTerminal = e.surface === 'terminal';
  // The footer sits at the bottom of the pane and names antiburn on every surface.
  return (
    <Box
      flexDirection="column"
      {...paneHeight(e)}
      justifyContent="space-between"
      gap={1}
    >
      {/* The watermark comes first, so the content paints over it. Only the desktop draws an Svg.
          It stops one row above the bottom: an absolute Svg over the footer link takes its clicks.
          While the demo tour runs, it also stops above the tour row, so the Next step button takes clicks. */}
      {Svg !== undefined && !hasCard && (
        <Box
          key="watermark"
          position="absolute"
          right={0}
          bottom={tourBeat === null ? 1 : 2}
        >
          <Svg source={WATERMARK_SVG} alt="antiburn" width={mark} height={mark} />
        </Box>
      )}
      {/* An absolute Box paints over normal siblings, so the content is positioned too and paints over the mark. */}
      <Box
        key="content"
        position="relative"
        flexDirection="column"
        flexGrow={hasCard ? 0 : 1}
      >
        {body}
      </Box>
      {/* The free rows under the cards hold the mark. The Box shrinks to zero when the cards fill the pane,
          and it clips the top of the mark when the rows are fewer than the mark. */}
      {Svg !== undefined && hasCard && (
        <Box
          key="watermark-space"
          flexDirection="row"
          flexGrow={1}
          flexShrink={1}
          minHeight={0}
          overflow="hidden"
          justifyContent="flex-end"
          alignItems="flex-end"
        >
          <Svg source={WATERMARK_SVG} alt="antiburn" width={mark} height={mark} />
        </Box>
      )}
      <Box key="footer" position="relative" flexDirection="column">
        {tourBeat !== null && (
          <Box key="tour" flexDirection="row" alignItems="center" columnGap={1}>
            <Text dimColor>{`Demo tour · step ${tourBeat} of ${tourStepCount}`}</Text>
            {tourBeat! < tourStepCount ? (
              <Button
                key="tour-next"
                label="Next step"
                onPress={() => actions.nextTourStep()}
              />
            ) : (
              <Button
                key="tour-end"
                label="End tour"
                onPress={() => actions.endTour()}
              />
            )}
          </Box>
        )}
        {isTerminal && (
          <Text dimColor>{'─'.repeat(Math.max(0, e.props.bodyColumns))}</Text>
        )}
        {/* Upsell option A: one passive link. The lead-in stays muted. The mark and the link name use the
            brand orange, not the metric colour, so they read as the brand and not as an alert.
            The mark is 14 px so that it is near the text cap height and does not pull focus from the cards. */}
        <Box key="app-link" flexDirection="row" alignItems="center" columnGap={1}>
          {Svg !== undefined && (
            <Svg
              key="brand-mark"
              source={BRAND_MARK_SVG}
              alt="antiburn"
              width={14}
              height={14}
            />
          )}
          <Text>
            <Text dimColor>{'Get more from every session with '}</Text>
            {/* A terminal without OSC 8 prints the URL after the link text, and the URL is the part that opens.
                So on the terminal the arrow comes after the Link, and it follows the URL when one prints.
                The arrow is dim, as the terminal draws the URL. */}
            <Link href={APP_URL}>
              <Text color={BRAND}>
                {isTerminal ? 'antiburn app' : 'antiburn app ↗'}
              </Text>
            </Link>
            {isTerminal && <Text dimColor> ↗</Text>}
          </Text>
        </Box>
      </Box>
    </Box>
  );
}

export function renderTerminalMarkView(
  ui: PaneUi,
  cells: string | undefined,
  openWebsite: () => Promise<void>,
): RenderNode {
  return (
    <ui.Box
      key="usage-brand-text-slot"
      position="relative"
      width={11}
      paddingLeft={1}
      flexDirection="row"
      flexShrink={0}
      height={2}
      alignItems="center"
      justifyContent="center"
    >
      <ui.Box flexDirection="column" width={10} height={2} flexShrink={0}>
        {'Raster' in ui && cells ? (
          <ui.Raster
            key="usage-terminal-flame"
            columns={10}
            rows={1}
            cells={cells.slice(0, 160)}
          />
        ) : (
          <ui.Text> </ui.Text>
        )}
        <ui.Box width={10} height={1} flexShrink={0} overflow="hidden">
          <ui.Button
            key="usage-brand-open"
            plain
            label="Open antiburn website"
            onPress={openWebsite}
          >
            {cells ? (
              terminalWordmark(cells).map((cell, index) => (
                <ui.Text
                  key={`brand-letter-${index}`}
                  color={cell.color}
                  backgroundColor={cell.backgroundColor}
                >
                  {cell.glyph}
                </ui.Text>
              ))
            ) : (
              <ui.Text color={BRAND} backgroundColor={COUNT_TINT_TERMINAL}>
                {' antiburn '}
              </ui.Text>
            )}
          </ui.Button>
        </ui.Box>
      </ui.Box>
    </ui.Box>
  );
}

export function renderDesktopBrandLinkView(ui: PaneUi): RenderNode {
  const { Box, Link, Text } = ui;
  return (
    <Box
      key="usage-brand-link"
      position="absolute"
      left={0}
      top={0}
      width={14 / 8}
      height={14 / 16}
      overflow="hidden"
    >
      <Link href={APP_URL} label="Open antiburn website">
        <Text>{'\u2003'}</Text>
      </Link>
    </Box>
  );
}

export function renderUsageMarkView(
  ui: PaneUi,
  resting: string,
  alt: string,
  burnSource: string,
  burnHeight: number,
  brandLink: () => RenderNode,
): RenderNode | null {
  if (!('Svg' in ui)) return null;
  const { Box } = ui;
  return (
    <Box
      key="usage-mark-slot"
      alignItems="center"
      width={PRESENTATION.markSlotSize / 8}
      position="relative"
      overflow="visible"
      flexShrink={0}
    >
      <ui.Svg
        key="usage-mark-size"
        source={resting}
        alt="antiburn"
        width={PRESENTATION.markSize}
        height={PRESENTATION.markSize}
        isInteractive={!burnSource}
      />
      {burnSource && (
        <Box
          position="absolute"
          bottom={0}
          left={0}
          width={PRESENTATION.markSlotSize / 8}
          alignItems="center"
        >
          <ui.Svg
            key="usage-mark"
            source={burnSource}
            alt={alt}
            width={PRESENTATION.markSize}
            height={(burnHeight * PRESENTATION.markSize) / 14}
            isInteractive
          />
        </Box>
      )}
      {brandLink()}
    </Box>
  );
}
