import { BITCOUNT_PIXEL, captionSvg, captionWidth } from './usage-font'
import {
  INLINE_METER_HEIGHT,
  METER_HEIGHT,
  PRESENTATION,
  desktopPercentage,
  meterGeometry,
  rollingColumns,
  type UsageLayout,
} from './usage-presentation'
import { SVG_NAMESPACE } from './brand'
import {
  pace,
  paceLabel,
  percentage,
  resetText,
  type UsageWindow,
} from './usage'

export { METER_HEIGHT }

export function xml(text: string): string {
  return text.replace(
    /[&<>"']/g,
    (c) =>
      ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&apos;',
      })[c]!,
  )
}

function fittedStatus(text: string, width: number): string {
  if (captionWidth(text, 12, false, 0.025) <= width) return text
  if (captionWidth('...', 12, false, 0.025) > width) return ''
  let shortened = text
  while (
    shortened.length &&
    captionWidth(shortened + '...', 12, false, 0.025) > width
  )
    shortened = shortened.slice(0, -1)
  return shortened + '...'
}

export function usageStatusSvg(text: string, hint?: string, compact = false) {
  const hintX = compact ? 0 : captionWidth(text, PRESENTATION.captionSize) + 12
  const width = Math.ceil(
    hint
      ? hintX + captionWidth(hint, 12)
      : captionWidth(text, PRESENTATION.captionSize),
  )
  const description = hint ? `${text}. ${hint}` : text
  const checking = text === 'Checking usage...'
  const caption = checking
    ? captionSvg('Checking usage', 0, 13.2, PRESENTATION.captionSize) +
      [0, 1, 2]
        .map(
          (index) =>
            `<g class="checking-dot" style="animation-delay:${index * 80}ms">${captionSvg('.', captionWidth('Checking usage' + '.'.repeat(index), PRESENTATION.captionSize), 13.2, PRESENTATION.captionSize)}</g>`,
        )
        .join('')
    : captionSvg(text, 0, 13.2, PRESENTATION.captionSize)
  const motion = checking
    ? '.checking-dot{animation:checking-dot 1200ms linear infinite}@keyframes checking-dot{0%,100%{opacity:.35}50%{opacity:1}}@media(prefers-reduced-motion:reduce){.checking-dot{animation:none;opacity:1}}'
    : ''
  return {
    width,
    height: 18,
    description,
    source: `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="18" viewBox="0 0 ${width} 18" preserveAspectRatio="xMinYMid meet"><title>${xml(description)}</title><defs><path id="bitcount-pixel" d="${BITCOUNT_PIXEL}"/></defs><style>.ink{fill:#242424}.hint{fill:#66615b}@media(prefers-color-scheme:dark){.ink{fill:#ececea}.hint{fill:#b8b5b0}}${motion}</style>${!compact || !hint ? `<g class="ink">${caption}</g>` : ''}${hint ? `<g class="hint">${captionSvg(hint, hintX, 13, 12)}</g>` : ''}</svg>`,
  }
}

export function secondaryTextSvg(
  lines: readonly string[],
  availableWidth: number,
  maxLines = 2,
) {
  const width = Math.max(1, Math.floor(availableWidth))
  const wrapped = lines.flatMap((text) => {
    const rows: string[] = []
    let row = ''
    for (const word of text.split(/\s+/)) {
      const next = row ? `${row} ${word}` : word
      if (row && captionWidth(next, 12, false, 0.025) > width) {
        rows.push(fittedStatus(row, width))
        row = word
      } else row = next
    }
    if (row) rows.push(fittedStatus(row, width))
    return rows
  })
  const limit = Math.max(1, Math.floor(maxLines))
  const rows =
    wrapped.length <= limit
      ? wrapped
      : [
          ...wrapped.slice(0, limit - 1),
          fittedStatus(wrapped.slice(limit - 1).join(' '), width),
        ]
  const height = Math.max(1, rows.length) * 18
  const description = lines.join(' ')
  return {
    width,
    height,
    description,
    source: `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><title>${xml(description)}</title><defs><path id="bitcount-pixel" d="${BITCOUNT_PIXEL}"/></defs><style>.hint{fill:#66615b}@media(prefers-color-scheme:dark){.hint{fill:#b8b5b0}}</style><g class="hint">${rows.map((line, index) => captionSvg(line, 0, index * 18 + 13, 12, false, 0.025)).join('')}</g></svg>`,
  }
}

export function meterDescription(
  window: UsageWindow,
  now: number,
  desktop = false,
): string {
  const value = pace(window, now)
  return `${window.label}: ${window.percent === null ? 'Unavailable' : desktop ? desktopPercentage(window) : percentage(window)}. ${resetText(window, now)}${value === null ? '' : `. ${paceLabel(value)}`}`
}

export type MeterMotion = {
  layout?: UsageLayout
  snapMarker?: boolean
  warning?: 'red' | 'pulse'
  resetPreview?: 'show' | 'hide' | 'pace-show' | 'pace-hide'
  previousFillFraction?: number
  previousFillEnd?: number
  fillElapsed?: number
  entranceElapsed?: number
  meterIndex?: 0 | 1
  previousPercent?: string
  rollElapsed?: number
}

function fittedReset(text: string, width: number): string {
  if (captionWidth(text, 12, false, 0.025) <= width) return text
  const suffix = '...'
  if (captionWidth(suffix, 12, false, 0.025) > width) return ''
  let shortened = text
  while (
    shortened.length > 0 &&
    captionWidth(shortened + suffix, 12, false, 0.025) > width
  )
    shortened = shortened.slice(0, -1)
  return shortened + suffix
}

function percentSvg(
  current: string,
  x: number,
  motion: MeterMotion,
  baseline: number = PRESENTATION.captionBaseline,
): string {
  if (
    motion.previousPercent === undefined ||
    motion.rollElapsed === undefined ||
    motion.rollElapsed >= 284
  ) {
    return captionSvg(current, x, baseline, PRESENTATION.captionSize, true)
  }
  let left = x
  return rollingColumns(motion.previousPercent, current)
    .map((column, index) => {
      const at = left
      const width = captionWidth(
        column.character,
        PRESENTATION.captionSize,
        true,
      )
      left += width
      const next = captionSvg(
        column.character,
        at,
        baseline,
        PRESENTATION.captionSize,
        true,
      )
      if (!column.changed) return next
      const delay = column.delay - motion.rollElapsed!
      return (
        `<clipPath id="roll-${index}"><rect x="${at}" width="${width}" height="${motion.layout === 'inline' ? INLINE_METER_HEIGHT : PRESENTATION.captionHeight}"/></clipPath>` +
        `<g clip-path="url(#roll-${index})"><g class="roll-old" style="animation-delay:${delay}ms">${captionSvg(column.before, at, baseline, PRESENTATION.captionSize, true)}</g><g class="roll-new" style="animation-delay:${delay}ms">${next}</g></g>`
      )
    })
    .join('')
}

function baseMeterStyles(fill: string): string {
  return `<style>
.ink{fill:#252525}.off{fill:#d1cec9}.used{fill:${fill}}.sweep{opacity:${PRESENTATION.fillOpacity}}.marker{color:#666;opacity:.4;transition:opacity 125ms cubic-bezier(.23,1,.32,1)}.detail-reveal{fill:color-mix(in srgb,#64645f 80%,#252525);opacity:0;clip-path:inset(0 100% 0 0);transition:opacity 180ms cubic-bezier(.32,.72,0,1),clip-path 180ms cubic-bezier(.32,.72,0,1)}
@media(hover:hover) and (pointer:fine){.meter:has(.label:hover) .reset,.meter:has(.pace:hover) .pace-detail{opacity:1;clip-path:inset(0);transition-duration:280ms;transition-delay:300ms}.meter:hover .marker{opacity:1;transition-delay:300ms}.meter:has(.pace:hover) .reset.detail-reveal{opacity:0;clip-path:inset(0 100% 0 0);transition-duration:280ms;transition-delay:300ms;animation:none}}
.meter:has(.label:focus) .reset,.meter:has(.pace:focus) .pace-detail{opacity:1;clip-path:inset(0);transition:none}.meter:has(.pace:focus) .reset{opacity:0;clip-path:inset(0 100% 0 0);transition:none;animation:none}.pace:focus .marker{opacity:1;transition:none}.hit{fill:transparent}.reset,.pace-detail{pointer-events:none}.reveal-background{fill:#f0efec;fill-opacity:.82}.focus-ring{fill:none;stroke:#ff5914;stroke-width:1;opacity:0}.label:focus .focus-ring,.pace:focus .focus-ring{opacity:1}
@media(prefers-color-scheme:dark){.ink{fill:#ececea}.off{fill:#4b4843}.marker{color:#b8b5b0}.reset,.pace-detail{fill:color-mix(in srgb,#b8b5b0 80%,#ececea)}.reveal-background{fill:#212121}}
@media(prefers-reduced-motion:reduce){.detail-reveal{clip-path:none;transition:opacity 80ms cubic-bezier(.32,.72,0,1)}.meter:has(.label:hover) .reset,.meter:has(.pace:hover) .pace-detail{clip-path:none;transition-duration:80ms}.marker{transition-duration:80ms}.meter:has(.pace:hover) .reset{clip-path:none;transition:opacity 80ms cubic-bezier(.32,.72,0,1);transition-delay:0ms}.meter:has(.label:focus) .reset,.meter:has(.pace:focus) .pace-detail,.pace:focus .marker{transition:none}}
</style>`
}

function meterStyles(
  window: UsageWindow,
  width: number,
  now: number,
  revealed: boolean,
  motion: MeterMotion,
): string {
  const exhausted = window.percent !== null && window.percent >= 100
  const warning = exhausted ? undefined : motion.warning
  const inline = motion.layout === 'inline'
  const { trackX, trackWidth, fillEnd } = meterGeometry(
    window,
    width,
    now,
    motion.layout,
  )
  const fillWidth = inline ? trackWidth : width
  const elapsed = motion.entranceElapsed
  const entering = elapsed !== undefined && elapsed < PRESENTATION.entranceEnd
  const delay = (motion.meterIndex === 1 ? 240 : 100) - (elapsed ?? 0)
  const animateFill =
    motion.previousFillEnd !== undefined &&
    motion.fillElapsed !== undefined &&
    motion.fillElapsed < 360 &&
    !entering
  const base = baseMeterStyles(
    exhausted || warning === 'pulse' ? '#ed4545' : '#ff5914',
  )
  const moving =
    entering ||
    animateFill ||
    motion.previousPercent !== undefined ||
    motion.resetPreview !== undefined ||
    warning !== undefined
  if (!moving && !revealed)
    return (
      base +
      (inline
        ? ''
        : '<style>@media(hover:hover) and (pointer:fine){.meter:hover .reset{opacity:1;clip-path:inset(0);transition-duration:280ms;transition-delay:300ms}}</style>')
    )
  return (
    base +
    `<style>
.over-used{fill:#ed4545}
.reset{opacity:${revealed ? 1 : 0};clip-path:inset(0 ${revealed ? 0 : 100}% 0 0)}
${inline ? '' : '@media(hover:hover) and (pointer:fine){.meter:hover .reset{opacity:1;clip-path:inset(0);transition-duration:280ms;transition-delay:300ms}}'}
${warning === 'pulse' ? '.debug-glow{opacity:.12;fill:#ed4545}.debug-fill{animation:debug-pulse 1600ms cubic-bezier(.77,0,.175,1) 6}@keyframes debug-pulse{0%,100%{opacity:1}50%{opacity:.65}}' : ''}
${motion.resetPreview ? `.${motion.resetPreview.startsWith('pace-') ? 'pace-detail' : 'reset'}{animation:debug-reset ${motion.resetPreview.endsWith('show') ? 280 : 180}ms cubic-bezier(.32,.72,0,1) ${motion.resetPreview.endsWith('hide') ? 'reverse' : 'normal'} both}@keyframes debug-reset{from{opacity:0;clip-path:inset(0 100% 0 0)}to{opacity:1;clip-path:inset(0)}}.meter:has(.label:focus) .reset{animation:none}` : ''}
${animateFill ? `.fill-front{transform-origin:${trackX}px 0;animation:preview-fill 360ms cubic-bezier(.23,1,.32,1) ${-motion.fillElapsed!}ms both}@keyframes preview-fill{from{transform:scaleX(${fillWidth > 0 ? Math.max(0, motion.previousFillEnd! - trackX) / fillWidth : 0})}to{transform:scaleX(${fillWidth > 0 ? Math.max(0, fillEnd - trackX) / fillWidth : 0})}}` : ''}
@keyframes meter-in{from{opacity:0;transform:translateY(var(--meter-rise,6px))}to{opacity:1;transform:none}}
@keyframes fill-in{from{transform:scaleX(0)}to{transform:scaleX(1)}}
@keyframes unmask-meter{from{clip-path:url(#meter-mask)}to{clip-path:none}}
@keyframes unmask-fill{from{clip-path:url(#fill-front)}to{clip-path:none}}
@keyframes roll-in{from{transform:translateY(18.75px)}to{transform:none}}
@keyframes roll-out{from{transform:translateY(0);visibility:visible}to{transform:translateY(-18.75px);visibility:hidden}}
.roll-old{animation:roll-out 260ms cubic-bezier(.32,.72,0,1) both}.roll-new{animation:roll-in 260ms cubic-bezier(.32,.72,0,1) both}
${entering ? `.meter{animation:unmask-meter 960ms steps(1,end) ${-elapsed}ms both}.sweep{animation:unmask-fill 960ms steps(1,end) ${-elapsed}ms both}.composition{animation:meter-in 360ms cubic-bezier(.32,.72,0,1) ${delay}ms both}.fill-front{animation:fill-in 360ms cubic-bezier(.23,1,.32,1) ${600 - elapsed}ms both;transform-origin:${trackX}px 0}` : ''}
@media(prefers-reduced-motion:reduce){.debug-fill,.debug-glow,.detail-reveal{animation:none!important}.meter,.sweep{animation:none;clip-path:none}${animateFill ? ' .sweep{clip-path:url(#fill-front)}' : ''}.composition{--meter-rise:0px;animation-duration:80ms;animation-delay:${-(elapsed ?? 0)}ms;transform:none}.fill-front{animation:none;transform:${animateFill ? `scaleX(${fillWidth > 0 ? Math.max(0, fillEnd - trackX) / fillWidth : 0})` : 'none'}}.roll-old{display:none}.roll-new{animation:none}.detail-reveal{clip-path:none;transition:opacity 80ms cubic-bezier(.32,.72,0,1)}${inline ? '.meter:has(.label:hover)' : '.meter:hover'} .reset,.meter:has(.pace:hover) .pace-detail{clip-path:none;transition-duration:80ms}.marker{transition-duration:80ms}.meter:has(.pace:hover) .reset.detail-reveal{clip-path:none;transition:opacity 80ms cubic-bezier(.32,.72,0,1);transition-delay:0ms}.meter:has(.label:focus) .reset,.meter:has(.pace:focus) .pace-detail,.pace:focus .marker{transition:none}${motion.resetPreview ? ` .${motion.resetPreview.startsWith('pace-') ? 'pace-detail' : 'reset'}{opacity:${motion.resetPreview.endsWith('show') ? 1 : 0};clip-path:none}` : ''}}
</style>`
  )
}

export function meterSvg(
  window: UsageWindow,
  width: number,
  now: number,
  revealed = false,
  caption = true,
  motion: MeterMotion = {},
): string {
  const exhausted = window.percent !== null && window.percent >= 100
  const warning = exhausted ? undefined : motion.warning
  const inline = motion.layout === 'inline'
  const geometry = meterGeometry(window, width, now, motion.layout)
  const {
    dots,
    percent,
    percentX,
    resetX,
    resetWidth,
    fillEnd,
    height,
    baseline,
    trackX,
    trackWidth,
    labelWidth,
  } = geometry
  const marker =
    geometry.marker === null || !motion.snapMarker
      ? geometry.marker
      : { ...geometry.marker, x: Math.round(geometry.marker.x) }
  const resetBaseline = inline ? height / 2 + 3.6 : PRESENTATION.resetBaseline
  const labelHitWidth = inline ? labelWidth + 3 : Math.max(0, percentX - 8)
  const labelHitHeight = inline ? height : PRESENTATION.captionHeight
  const fillWidth = inline ? trackWidth : width
  const resetCopy =
    inline && resetWidth < 110 && window.resetsAt !== null
      ? resetText(window, now).replace(/^RESETS IN /, '')
      : resetText(window, now)
  const paceText =
    marker === null ? '' : `Even pace: ${Math.round(marker.value * 100)}%`
  const paceRight =
    marker === null ? 0 : Math.max(0, trackX + trackWidth - marker.x - 6)
  const paceLeft = marker === null ? 0 : Math.max(0, marker.x - trackX - 6)
  const paceOnRight = paceRight >= paceLeft
  const paceCopy = fittedReset(paceText, paceOnRight ? paceRight : paceLeft)
  const paceCopyWidth = captionWidth(paceCopy, 12, false, 0.025)
  const paceCopyX =
    marker === null
      ? trackX
      : paceOnRight
        ? marker.x + 6
        : marker.x - 6 - paceCopyWidth
  const trackTop = PRESENTATION.captionHeight
  const elapsed = motion.entranceElapsed
  const entering = elapsed !== undefined && elapsed < PRESENTATION.entranceEnd
  const animateFill =
    motion.previousFillEnd !== undefined &&
    motion.fillElapsed !== undefined &&
    motion.fillElapsed < 360 &&
    !entering
  const neutral = dots
    .map(
      (dot) =>
        `<circle class="off" cx="${dot.x}" cy="${dot.y}" r="${PRESENTATION.dotSize / 2}"/>`,
    )
    .join('')
  const usedCircle = (dot: (typeof dots)[number], red = false) =>
    `<circle class="${red ? 'over-used' : 'used'}" cx="${dot.x}" cy="${dot.y}" r="${PRESENTATION.dotSize / 2}"/>`
  const filled = dots
    .filter((dot) => animateFill || dot.fraction === 1)
    .map((dot) => usedCircle(dot))
    .join('')
  const redAfterPace =
    warning === 'red' && marker?.reached
      ? `<g id="over-pace">${dots
          .filter(
            (dot) =>
              (animateFill || dot.fraction === 1) &&
              dot.x - PRESENTATION.dotSize / 2 >= marker.x,
          )
          .map((dot) => usedCircle(dot, true))
          .join('')}</g>`
      : ''
  const description = meterDescription(window, now, true)
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${caption ? height : 12}" viewBox="0 ${caption ? 0 : trackTop} ${width} ${caption ? height : 12}" aria-label="${xml(description)}">` +
    `<defs><path id="bitcount-pixel" d="${BITCOUNT_PIXEL}"/>` +
    `<clipPath id="meter-mask"><rect width="${width}" height="${height}"/></clipPath>` +
    `<clipPath id="reset-bounds"><rect x="${resetX}" width="${resetWidth}" height="${inline ? height : trackTop}"/></clipPath>` +
    `<clipPath id="fill-front"><rect class="fill-front" x="${trackX}" width="${animateFill ? fillWidth : Math.max(0, fillEnd - trackX)}" height="${height}"/></clipPath></defs>` +
    meterStyles(window, width, now, revealed, motion) +
    `<g class="meter" clip-path="url(#meter-mask)"><rect class="meter-hit" width="${width}" height="${height}" fill="transparent" pointer-events="all"/><g class="composition">` +
    `<g class="label ink" tabindex="0" aria-label="${xml(`${window.label}, ${resetText(window, now)}`)}"><rect class="hit" width="${labelHitWidth}" height="${labelHitHeight}"/>` +
    `<g class="caption-ink">${captionSvg(window.label, 0, baseline, PRESENTATION.captionSize)}</g>` +
    `<rect class="focus-ring" x=".5" y=".5" width="${Math.max(0, labelHitWidth - 1)}" height="${labelHitHeight - 1}"/></g>` +
    `<g class="ink data" aria-label="${xml(percent)}">${percentSvg(percent, percentX, motion, baseline)}</g>` +
    '<g class="data">' +
    neutral +
    (warning === 'pulse'
      ? `<g class="debug-glow"><g class="debug-fill">${dots
          .filter((dot) => dot.fraction > 0)
          .map(
            (dot) =>
              `<circle cx="${dot.x}" cy="${dot.y}" r="${PRESENTATION.dotSize / 2 + 1.5}"/>`,
          )
          .join('')}</g></g>`
      : '') +
    `<g class="sweep${warning === 'pulse' ? ' debug-fill' : ''}"${entering || animateFill ? ' clip-path="url(#fill-front)"' : ''}>${filled}${redAfterPace}</g>` +
    '</g>' +
    (marker === null
      ? ''
      : `<g class="pace" tabindex="0" aria-label="${xml(paceLabel(marker.value))}"><rect class="hit" x="${Math.max(0, Math.min(width - 20, marker.x - 10))}" y="${inline ? 0 : Math.max(0, geometry.trackY - 12)}" width="20" height="${inline ? height : 24}"/><path class="marker-shadow" d="M${marker.x + 1.5} ${marker.y - marker.height / 2}v${marker.height}" stroke="#000" stroke-opacity=".22" stroke-width="1" pointer-events="none"/><path class="marker" d="M${marker.x} ${marker.y - marker.height / 2}v${marker.height}" stroke="${exhausted ? '#ed4545' : marker.reached ? '#ff5914' : 'currentColor'}" stroke-width="2"/><rect class="focus-ring" x="${Math.max(0, Math.min(width - 20, marker.x - 10)) + 0.5}" y="${inline ? 0.5 : Math.max(0, geometry.trackY - 12) + 0.5}" width="19" height="${inline ? height - 1 : 23}"/></g>`) +
    `<g clip-path="url(#reset-bounds)"><g class="reset detail-reveal">${inline ? `<rect class="reveal-background" x="${resetX}" width="${resetWidth}" height="${height}"/>` : ''}${captionSvg(fittedReset(resetCopy, resetWidth), resetX, resetBaseline, 12, false, 0.025)}</g></g>` +
    (inline && marker !== null
      ? `<g class="pace-detail detail-reveal"><rect class="reveal-background" x="${trackX}" width="${trackWidth}" height="${height}"/>${captionSvg(paceCopy, paceCopyX, resetBaseline, 12, false, 0.025)}</g>`
      : '') +
    '</g></g></svg>'
  )
}

export function markSvg(source: string, elapsed?: number): string {
  if (elapsed === undefined || elapsed >= 960) return source
  const open = source.indexOf('>') + 1
  return (
    source.slice(0, open) +
    `<style>@keyframes mark-in{from{opacity:0;transform:scale(var(--mark-start,.92))}to{opacity:1;transform:none}}.mark{transform-origin:22.4px 22.1px;animation:mark-in 240ms cubic-bezier(.32,.72,0,1) ${-elapsed}ms both}@media(prefers-reduced-motion:reduce){.mark{--mark-start:1;animation-duration:80ms}}</style><g class="mark">` +
    source.slice(open).replace('</svg>', '</g></svg>')
  )
}

export function meterLayer(
  source: string,
  layer: 'base' | 'interaction',
): string {
  const rules =
    layer === 'base'
      ? '.reset,.pace-detail,.hit,.focus-ring{display:none}'
      : '.data,.caption-ink,.marker-shadow{display:none}.marker{opacity:0}'
  return source.replace('</style>', `${rules}</style>`)
}

// The host clamps the frame to its flex slot. SVG coordinates remain CSS pixels.
export const RESPONSIVE_FRAME_WIDTH = 4096

export function responsiveMeterSvg(
  window: UsageWindow,
  now: number,
  motion: MeterMotion = {},
  revealed = false,
): string {
  const variant = motion.layout ?? 'inline'
  const inline = variant === 'inline'
  const scene = meterGeometry(window, 256, now, variant)
  const { trackX, baseline, height, labelWidth } = scene
  const percent = desktopPercentage(window)
  const percentWidth = captionWidth(percent, PRESENTATION.captionSize, true)
  const fixed = trackX + (inline ? 7 + percentWidth : 0)
  const exhausted = window.percent !== null && window.percent >= 100
  const fill = exhausted || motion.warning === 'pulse' ? '#ed4545' : '#ff5914'
  const fraction = Math.max(0, Math.min(1, (window.percent ?? 0) / 100))
  const filled =
    fraction === 0
      ? '0px'
      : fraction === 1
        ? 'var(--track)'
        : `clamp(7px, calc(round(nearest, calc((var(--span) + 11px) * ${fraction}), 11px) - 4px), calc(var(--span) - 4px))`
  const marker = scene.marker
  const description = meterDescription(window, now, true)
  const reset = resetText(window, now)
  const paceCopy =
    marker === null ? '' : `Even pace: ${Math.round(marker.value * 100)}%`
  const resetX = inline ? 0 : scene.resetX
  const copyInset = inline ? fixed + 10 : resetX + percentWidth + 8
  const copy = (text: string, name: string) => {
    const widths = [
      32,
      64,
      110,
      Math.max(111, Math.ceil(captionWidth(text, 12, false, 0.025))),
    ]
    return widths
      .map((width, index) => {
        const rule =
          index === 0
            ? ''
            : `@media(min-width:${copyInset + width}px){.${name}-${index - 1}{display:none}.${name}-${index}{display:inline}}`
        return `<style>.${name}-${index}{display:${index === 0 ? 'inline' : 'none'}}${rule}</style><g class="${name}-${index}">${captionSvg(fittedReset(text, width), 0, inline ? height / 2 + 3.6 : PRESENTATION.resetBaseline, 12, false, 0.025)}</g>`
      })
      .join('')
  }
  const styles = meterStyles(window, 256, now, revealed, {
    ...motion,
    layout: variant,
  })
  const trackY = scene.trackY
  const resetWidth = inline
    ? 'var(--track)'
    : `max(0px, calc(100vw - ${resetX + percentWidth + 8}px))`
  // The visible caption uses the widest copy that fits beside the marker.
  const paceCopies =
    marker === null
      ? []
      : [
          ...new Set(
            [
              16,
              32,
              64,
              110,
              Math.ceil(captionWidth(paceCopy, 12, false, 0.025)),
            ].map((width) => fittedReset(paceCopy, width)),
          ),
        ]
  const paceWidths = paceCopies.map((text) =>
    captionWidth(text, 12, false, 0.025),
  )
  const fitsPace = (index: number) =>
    index === paceWidths.length
      ? '0'
      : `clamp(0, sign(calc(var(--pace-room) - ${paceWidths[index]}px + .001px)), 1)`
  const paceDetails = paceCopies
    .map((text, index) => {
      const width = paceWidths[index]!
      return `<g class="pace-copy-${index}" style="opacity:calc(${fitsPace(index)} - ${fitsPace(index + 1)});transform:translateX(calc(var(--pace-end) - ${width}px + var(--pace-on-right) * (var(--pace-start) - var(--pace-end) + ${width}px)))"><rect class="reveal-background" width="${width}" height="${inline ? height : PRESENTATION.captionHeight}"/>${captionSvg(text, 0, inline ? height / 2 + 3.6 : PRESENTATION.resetBaseline, 12, false, 0.025)}</g>`
    })
    .join('')
  const previousFraction =
    motion.previousFillEnd === undefined
      ? undefined
      : (motion.previousFillFraction ??
        Math.max(
          0,
          Math.min(
            1,
            (motion.previousFillEnd - scene.trackX) /
              Math.max(1, scene.trackWidth),
          ),
        ))
  const fillMotion =
    previousFraction !== undefined &&
    motion.fillElapsed !== undefined &&
    motion.fillElapsed < 360
  const fillPaintClass = fillMotion ? 'track-width' : 'fill-width'
  const entering =
    motion.entranceElapsed !== undefined &&
    motion.entranceElapsed < PRESENTATION.entranceEnd
  return (
    `<svg xmlns="${SVG_NAMESPACE}" width="100%" height="${height}" aria-label="${xml(description)}">` +
    `<defs><path id="bitcount-pixel" d="${BITCOUNT_PIXEL}"/><pattern id="off-dots" patternUnits="userSpaceOnUse" y="${trackY - 9}" width="11" height="18"><circle class="off" cx="3.5" cy="9" r="3.5"/></pattern><pattern id="used-dots" patternUnits="userSpaceOnUse" y="${trackY - 9}" width="11" height="18"><circle class="used" cx="3.5" cy="9" r="3.5"/></pattern><pattern id="over-dots" patternUnits="userSpaceOnUse" y="${trackY - 9}" width="11" height="18"><circle class="over-used" cx="3.5" cy="9" r="3.5"/></pattern><clipPath id="reset-bounds"><rect x="${resetX}" style="width:${resetWidth}" height="${height}"/></clipPath><clipPath id="fill-front"><rect class="fill-front fill-width" height="${height}"/></clipPath><clipPath id="meter-mask"><rect width="100%" height="${height}"/></clipPath></defs>` +
    styles +
    `<style>
:root{--span:max(0px,round(down,calc(100vw - ${fixed + 7}px),11px));--track:calc(var(--span) + 7px);--filled:${filled};--pace-x:${marker === null ? '0px' : `round(nearest,calc(3.5px + var(--span) * ${marker.value}),1px)`};--pace-start:max(${resetX}px,calc(var(--pace-x) + 6px));--pace-end:min(calc(${resetX}px + ${resetWidth}),calc(var(--pace-x) - 6px));--pace-left:max(0px,calc(var(--pace-end) - ${resetX}px));--pace-right:max(0px,calc(${resetX}px + ${resetWidth} - var(--pace-start)));--pace-room:max(var(--pace-left),var(--pace-right));--pace-on-right:clamp(0,sign(calc(var(--pace-right) - var(--pace-left) + .001px)),1)}
.percent{transform:translateX(calc(100vw - ${percentWidth}px))}.track{transform:translateX(${trackX}px)}.track-width{width:var(--track)}.fill-width{width:var(--filled)}.track{display:none}.used{fill:${fill}}.percent{transform-box:view-box}.fill-front{transform-origin:0px 0;transform-box:fill-box}
${fillMotion && !entering ? `.fill-front{animation:responsive-fill 360ms cubic-bezier(.23,1,.32,1) ${-motion.fillElapsed!}ms both}@keyframes responsive-fill{from{width:calc(var(--track) * ${previousFraction})}to{width:var(--filled)}}` : ''}
@media(prefers-reduced-motion:reduce){.fill-front{animation:none!important;transform:none!important;width:var(--filled)}}
@media(min-width:${fixed + 18}px){.track{display:inline}}
${marker === null ? '' : '.pace{transform:translateX(var(--pace-x))}'}
</style>` +
    `<g class="meter">${inline ? '' : `<rect class="meter-hit" width="100%" height="${height}" fill="transparent" pointer-events="all"/>`}<g class="composition"><g class="label ink" tabindex="0" aria-label="${xml(`${window.label}, ${reset}`)}"><rect class="hit" width="${labelWidth + 3}" height="${inline ? 18 : PRESENTATION.captionHeight}"/><g class="caption-ink">${captionSvg(window.label, 0, baseline, PRESENTATION.captionSize)}</g><rect class="focus-ring" x=".5" y=".5" width="${labelWidth + 2}" height="17"/></g>` +
    `<g class="percent ink data">${percentSvg(percent, 0, { ...motion, layout: variant }, baseline)}</g>` +
    `<g class="track"><g class="data"><rect class="track-width" y="${trackY - 3.5}" height="7" fill="url(#off-dots)"/>${motion.warning === 'pulse' && !exhausted ? `<g class="debug-glow"><rect class="debug-fill fill-width" y="${trackY - 5}" height="10"/></g>` : ''}<g class="sweep${motion.warning === 'pulse' && !exhausted ? ' debug-fill' : ''}"${entering || fillMotion ? ' clip-path="url(#fill-front)"' : ''}><rect class="${fillPaintClass}" y="${trackY - 3.5}" height="7" fill="url(#used-dots)"/>${motion.warning === 'red' && !exhausted && marker?.reached ? `<rect id="over-pace" class="${fillPaintClass}" y="${trackY - 3.5}" height="7" fill="url(#over-dots)" style="clip-path:inset(0 0 0 round(up,calc(3.5px + var(--span) * ${marker.value}),11px))"/>` : ''}</g></g>` +
    (marker === null
      ? ''
      : `<g class="pace" tabindex="0" aria-label="${xml(paceLabel(marker.value))}"><rect class="hit" x="-10" width="20" y="${trackY - 9}" height="18"/><path class="marker-shadow" d="M1.5 ${trackY - 9}v18" stroke="#000" stroke-opacity=".22" stroke-width="1" pointer-events="none"/><path class="marker" d="M0 ${trackY - 9}v18" stroke="${exhausted ? '#ed4545' : marker.reached ? '#ff5914' : 'currentColor'}" stroke-width="2"/><rect class="focus-ring" x="-9.5" y="${trackY - 8.5}" width="19" height="17"/></g>`) +
    `</g><g transform="translate(${inline ? trackX : 0} 0)" clip-path="url(#reset-bounds)"><g class="reset detail-reveal"><rect class="reveal-background" x="${resetX}" style="width:${resetWidth}" height="${inline ? height : PRESENTATION.captionHeight}"/><g transform="translate(${resetX} 0)">${copy(reset, 'reset-copy')}</g></g>` +
    (marker === null
      ? ''
      : `<g class="pace-detail detail-reveal">${paceDetails}</g>`) +
    '</g></g></g></svg>'
  )
}

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
]

export const BRAND_MARK_SVG =
  `<svg xmlns="${SVG_NAMESPACE}" viewBox="6.2 5.9 32.4 32.4">` +
  MARK_DOTS.map(
    ([cx, cy]) => `<circle cx="${cx}" cy="${cy}" r="3.2" fill="#FF5914"/>`,
  ).join('') +
  '</svg>'
