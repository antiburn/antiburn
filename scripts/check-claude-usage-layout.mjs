import assert from 'node:assert/strict'
import { checkPairedUsageHover, checkPaceCaptionAnchoring, checkStripPaceResize } from './check-claude-usage-hover.mjs'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const require = createRequire(join(root, 'apps/desktop/package.json'))
const ts = require('typescript')
const { chromium } = require('@playwright/test')
const temporary = mkdtempSync(join(tmpdir(), 'antiburn-usage-layout-'))
const modules = ['usage', 'usage-presentation', 'usage-font', 'usage-font-data', 'brand', 'usage-meter-graphics']
let browser
try {
  for (const name of modules) {
    const folder = 'hooks'
    const source = readFileSync(join(root, `plugins/antiburn-live/${folder}/${name}.ts`), 'utf8')
    const output = ts
      .transpileModule(source, {
        compilerOptions: {
          target: ts.ScriptTarget.ES2023,
          module: ts.ModuleKind.ESNext,
        },
      })
      .outputText.replaceAll(/from (["'])(\.\/[^"']+)\1/g, 'from "$2.mjs"')
    writeFileSync(join(temporary, `${name}.mjs`), output)
  }
  const { responsiveMeterSvg, meterSvg, meterLayer, RESPONSIVE_FRAME_WIDTH } = await import(
    pathToFileURL(join(temporary, 'usage-meter-graphics.mjs'))
  )
  const { meterGeometry, PRESENTATION, usageLayout } = await import(
    pathToFileURL(join(temporary, 'usage-presentation.mjs'))
  )
  const { filledDotCount } = await import(pathToFileURL(join(temporary, 'usage.mjs')))
  const now = Date.parse('2026-01-01T00:00:00Z')
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({
    viewport: { width: 1800, height: 400 },
  })
  let cases = 0
  for (const variant of ['inline', 'current']) {
    for (const cell of [6, 8, 8.85, 10, 12]) {
      for (const columns of [24, 40, 59, 60, 61, 80, 120]) {
        for (const percent of [null, 0, 1, 2.5, 27, 55, 99.5, 100, 105]) {
          const window = {
            kind: 'five_hour',
            label: '5-hour',
            percent,
            resetsAt: '2026-01-01T02:48:00Z',
          }
          const svg = responsiveMeterSvg(window, now, { layout: variant })
          assert.ok(svg.length < 131072)
          const width = columns * cell
          const inline = columns >= 60
          await page.setContent(
            `<style>*{box-sizing:border-box}body{margin:0}.row{display:flex;align-items:center;width:${width}px}.meters{display:flex;flex-direction:${inline ? 'row' : 'column'};gap:${inline ? 3 * cell : 1}px;flex:1 1 0;min-width:0}.slot{display:flex;flex:1 1 0;min-width:0;width:${inline ? '0' : '100%'}}iframe{border:0;display:block;max-width:100%;height:${variant === 'inline' ? 18 : 33.75}px}</style><div class="row"><div style="flex:none;width:${(PRESENTATION.brandInset / 8) * cell}px"></div><div style="flex:none;width:${((PRESENTATION.markSlotSize + PRESENTATION.brandGap) / 8) * cell}px"></div><div class="meters"><div class="slot"><iframe width="${RESPONSIVE_FRAME_WIDTH}"></iframe></div><div class="slot"><iframe width="${RESPONSIVE_FRAME_WIDTH}"></iframe></div></div><div style="flex:none;width:${(PRESENTATION.brandInset / 8) * cell}px"></div></div>`,
          )
          await page.locator('iframe').evaluateAll((frames, source) => {
            for (const frame of frames) frame.srcdoc = `<style>html,body{margin:0}</style>${source}`
          }, svg)
          for (const frame of page.frames().slice(1)) {
            await frame.waitForSelector('.percent')
            const geometry = await frame.evaluate(() => {
              const svg = document.querySelector('svg')
              const percent = document.querySelector('.percent')
              const track = document.querySelector('.track-width')
              const fill = document.querySelector('.fill-width')
              return {
                width: innerWidth,
                scale: svg.getScreenCTM().a,
                percentX: percent.getCTM().e,
                percentRight: percent.getBoundingClientRect().right,
                track: parseFloat(getComputedStyle(track).width),
                fill: parseFloat(getComputedStyle(fill).width),
              }
            })
            const expected = meterGeometry(window, geometry.width, now, variant)
            assert.equal(geometry.scale, 1, 'Glyphs and dots must retain their pixel size')
            assert.ok(geometry.percentRight <= geometry.width + 0.02, 'Percentage must fit the visible frame')
            assert.ok(
              Math.abs(geometry.percentX - expected.percentX) < 0.02,
              'Percentage must reach the actual slot edge',
            )
            if (expected.dots.length >= 2) {
              const trackWidth = (expected.dots.length - 1) * 11 + 7
              assert.ok(Math.abs(geometry.track - trackWidth) < 0.02, 'Track must contain whole dots')
              const used = filledDotCount(percent, expected.dots.length)
              assert.ok(
                Math.abs(geometry.fill - (used === 0 ? 0 : (used - 1) * 11 + 7)) < 0.02,
                'Fill must match whole-dot rounding',
              )
            }
          }
          const bounds = await page.locator('.slot').evaluateAll((slots) =>
            slots.map((slot) => {
              const r = slot.getBoundingClientRect()
              return { x: r.x, right: r.right, y: r.y }
            }),
          )
          assert.ok(
            Math.abs(width - bounds[1].right - (PRESENTATION.brandInset / 8) * cell) < 0.02,
            'No trailing cell-to-pixel gap',
          )
          assert.equal(bounds[0].y === bounds[1].y, inline)
          cases++
        }
      }
    }
  }
  let motionCases = 0
  for (const colorScheme of ['light', 'dark']) {
    for (const reducedMotion of ['no-preference', 'reduce']) {
      await page.emulateMedia({ colorScheme, reducedMotion })
      for (const variant of ['inline', 'current']) {
        for (const width of [160, 288, 440]) {
          const reading = {
            kind: 'five_hour',
            label: '5-hour',
            percent: 68,
            resetsAt: '2026-01-01T02:15:00Z',
          }
          for (const motion of [
            {},
            { warning: 'red' },
            { warning: 'pulse' },
            { entranceElapsed: 0 },
            { previousPercent: '9%', rollElapsed: 0 },
            { previousFillEnd: 0, previousFillFraction: 0, fillElapsed: 0 },
            { resetPreview: 'show' },
          ]) {
            const source = responsiveMeterSvg(reading, now, {
              ...motion,
              layout: variant,
            })
            await page.setContent(`<iframe style="border:0;width:${width}px;height:40px"></iframe>`)
            await page.locator('iframe').evaluate((frame, source) => {
              frame.srcdoc = `<style>html,body{margin:0}</style>${source}`
            }, source)
            const frame = page.frames()[1]
            await frame.waitForSelector('.percent')
            const result = await frame.evaluate((source) => {
              const parsed = new DOMParser().parseFromString(source, 'image/svg+xml')
              return {
                error: parsed.querySelector('parsererror')?.textContent,
                overflow: !!document.querySelector('#over-pace'),
                fill: getComputedStyle(document.querySelector('.used')).fill,
                animations: document.getAnimations().map((animation) => ({
                  name: animation.animationName,
                  iterations: animation.effect.getTiming().iterations,
                })),
              }
            }, source)
            assert.equal(result.error, undefined, 'Shared SVG must remain well-formed')
            assert.equal(result.overflow, motion.warning === 'red')
            assert.equal(result.fill, motion.warning === 'pulse' ? 'rgb(237, 69, 69)' : 'rgb(255, 89, 20)')
            if (reducedMotion === 'reduce')
              assert.ok(
                result.animations.every(
                  (animation) => !/debug-pulse|roll-|fill-in|responsive-fill/.test(animation.name),
                ),
                'Reduced motion must suppress pulses, rolls and fill sweeps',
              )
            if (motion.warning === 'pulse' && reducedMotion === 'no-preference')
              assert.ok(
                result.animations.some((animation) => animation.name === 'debug-pulse' && animation.iterations === 6),
                'Near-limit pulse must stop after six cycles',
              )
            if (!motion.entranceElapsed && !motion.resetPreview) {
              await frame.locator('.label').focus()
              assert.equal(
                await frame.locator('.reset').evaluate((node) => getComputedStyle(node).opacity),
                '1',
                'Keyboard focus must reveal reset details',
              )
              await frame.locator('.pace').focus()
              assert.equal(
                await frame.locator('.pace-detail').evaluate((node) => getComputedStyle(node).opacity),
                '1',
                'Keyboard focus must reveal pace details',
              )
              assert.equal(
                await frame.locator('.reset').evaluate((node) => getComputedStyle(node).opacity),
                '0',
                'Pace focus must hide reset details',
              )
            }
            motionCases++
          }
        }
      }
    }
  }
  let fillCases = 0
  for (const reducedMotion of ['no-preference', 'reduce']) {
    await page.emulateMedia({ reducedMotion })
    for (const variant of ['inline', 'current']) {
      for (const width of [160, 440]) {
        for (const previousFillFraction of [0.05, 0.9]) {
          for (const warning of [undefined, 'red']) {
            const source = responsiveMeterSvg(
              { kind: 'five_hour', label: '5-hour', percent: 68, resetsAt: '2026-01-01T02:15:00Z' },
              now,
              { layout: variant, warning, previousFillFraction, previousFillEnd: 0, fillElapsed: 0 },
            )
            await page.setContent(`<iframe style="border:0;width:${width}px;height:40px"></iframe>`)
            await page.locator('iframe').evaluate((frame, source) => {
              frame.srcdoc = `<style>html,body{margin:0}</style>${source}`
            }, source)
            const frame = page.frames()[1]
            await frame.waitForSelector('.sweep')
            const samples = await frame.evaluate(() => {
              const clip = document.querySelector('#fill-front rect')
              const paint = document.querySelector('.sweep > rect')
              const track = document.querySelector('.track-width')
              const animation = document.getAnimations().find((item) => item.animationName === 'responsive-fill')
              animation?.pause()
              return [0, 90, 180, 360].map((time) => {
                if (animation) animation.currentTime = time
                const front = parseFloat(getComputedStyle(clip).width)
                const painted = parseFloat(getComputedStyle(paint).width)
                const overflow = document.querySelector('#over-pace')
                return {
                  visible: Math.min(front, painted),
                  front,
                  track: parseFloat(getComputedStyle(track).width),
                  overflow: overflow ? parseFloat(getComputedStyle(overflow).width) : undefined,
                }
              })
            })
            const start = samples[0]
            const end = samples.at(-1)
            for (const sample of samples) {
              assert.ok(Math.abs(sample.visible - sample.front) < 0.02, 'Paint must cover the animated clipping front')
              if (sample.overflow !== undefined)
                assert.ok(sample.overflow >= sample.front - 0.02, 'Red paint must cover the animated clipping front')
            }
            if (reducedMotion === 'reduce') {
              for (const sample of samples) assert.equal(sample.visible, end.visible)
            } else {
              assert.ok(Math.abs(start.visible - start.track * previousFillFraction) < 0.02)
              const direction = previousFillFraction === 0.9 ? -1 : 1
              for (let index = 1; index < samples.length; index++)
                assert.ok(
                  direction * (samples[index].visible - samples[index - 1].visible) > 0,
                  'Visible fill must progress toward the target in both directions',
                )
            }
            fillCases++
          }
        }
      }
    }
  }
  console.log(`Passed ${fillCases} rendered increasing/decreasing fill and reduced-motion cases.`)
  const auditArgument = process.argv.indexOf('--audit-dir')
  const auditDirectory = auditArgument < 0 ? undefined : resolve(process.argv[auditArgument + 1])
  if (auditDirectory) mkdirSync(auditDirectory, { recursive: true })
  let interactionCases = 0
  for (const colorScheme of ['light', 'dark']) {
    await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
    for (const variant of ['inline', 'current']) {
      for (const width of [160, 288, 440]) {
        for (const layer of ['single', 'interaction']) {
          const reading = {
            kind: 'five_hour',
            label: '5-hour',
            percent: 68,
            resetsAt: '2026-01-01T02:15:00Z',
          }
          const source = responsiveMeterSvg(reading, now, { layout: variant })
          await page.setContent(
            `<style>body{margin:0;background:${colorScheme === 'dark' ? '#212121' : '#f0efec'}}</style><iframe style="border:0;width:${width}px;height:40px"></iframe>`,
          )
          await page.locator('iframe').evaluate(
            (frame, source) => {
              frame.srcdoc = `<style>html,body{margin:0}</style>${source}`
            },
            layer === 'single' ? source : meterLayer(source, 'interaction'),
          )
          const frame = page.frames()[1]
          await frame.waitForSelector('.pace')
          await frame.locator('.pace').focus()
          const caption = await frame.evaluate(() => {
            const bounds = document.querySelector('#reset-bounds rect')
            const group = [...document.querySelectorAll('.pace-detail g')].find(
              (node) =>
                /pace-copy-/.test(node.getAttribute('class') ?? '') &&
                getComputedStyle(node).display !== 'none' &&
                Number(getComputedStyle(node).opacity) === 1,
            )
            const box = group.getBoundingClientRect()
            const container = document.querySelector('.pace-detail').parentElement
            const clipLeft = container.getCTM().e + bounds.x.baseVal.value
            const clipRight = clipLeft + parseFloat(getComputedStyle(bounds).width)
            return { left: box.left, right: box.right, clipLeft, clipRight }
          })
          if (auditDirectory && layer === 'single')
            await page
              .locator('iframe')
              .screenshot({ path: join(auditDirectory, `${colorScheme}-${variant}-${width}-pace.png`), scale: 'css' })
          assert.ok(caption.left >= caption.clipLeft - 0.02, 'Pace caption must start inside its clipping bounds')
          assert.ok(caption.right <= caption.clipRight + 0.02, 'Pace caption must fit its clipping bounds')
          await frame.locator('.pace').evaluate((node) => node.blur())
          if (variant === 'current') {
            await page.mouse.move(width - 60, 12)
            await frame.waitForFunction(() => getComputedStyle(document.querySelector('.reset')).opacity === '1')
            if (auditDirectory && layer === 'single')
              await page.locator('iframe').screenshot({
                path: join(auditDirectory, `${colorScheme}-${variant}-${width}-hover.png`),
                scale: 'css',
              })
          }
          interactionCases++
        }
      }
    }
  }
  console.log(
    `Passed ${await checkPaceCaptionAnchoring({ browser, responsiveMeterSvg, auditDirectory })} pace caption anchoring cases.`,
  )
  console.log(
    `Passed ${await checkStripPaceResize({ browser, responsiveMeterSvg, meterLayer, usageLayout, PRESENTATION, auditDirectory })} full-strip breakpoint hover cases.`,
  )
  const paired = await checkPairedUsageHover({ browser, responsiveMeterSvg, meterSvg, meterLayer, auditDirectory })
  console.log(
    `Passed ${paired.cases} paired image/frame resize cases; old overlay offset ${paired.negativeOffset.toFixed(2)}px rejected.`,
  )
  console.log(`Passed ${interactionCases} rendered caption and pointer-target regression cases.`)
  console.log(`Passed ${motionCases} shared SVG motion, theme, focus and reduced-motion cases.`)
  console.log(`Passed ${cases} synthetic Desktop layout cases across cell scales, breakpoints and readings.`)
} finally {
  await browser?.close()
  rmSync(temporary, { recursive: true, force: true })
}
