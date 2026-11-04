import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const require = createRequire(join(root, 'apps/desktop/package.json'))
const ts = require('typescript')
const { chromium } = require('@playwright/test')
const temporary = mkdtempSync(join(tmpdir(), 'antiburn-usage-entry-'))
const auditIndex = process.argv.indexOf('--audit-dir')
const auditDirectory =
  auditIndex < 0 ? undefined : resolve(process.argv[auditIndex + 1])
let browser
try {
  for (const name of [
    'usage',
    'usage-presentation',
    'usage-font',
    'usage-font-data',
    'brand',
    'usage-meter-graphics',
    'usage-strip-graphics',
    'usage-entrance',
  ]) {
    const source = readFileSync(
      join(root, `plugins/antiburn-live/hooks/${name}.ts`),
      'utf8',
    )
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
  const load = (name) => import(pathToFileURL(join(temporary, `${name}.mjs`)))
  const {
    responsiveMeterSvg,
    markSvg,
    BRAND_MARK_SVG,
    usageStatusSvg,
    secondaryTextSvg,
  } = await load('usage-meter-graphics')
  const { usageStripSvg, usageStatusStripSvg } = await load('usage-strip-graphics')
  const { fadeUsageSvg, USAGE_ENTRANCE } = await load('usage-entrance')
  const { PRESENTATION, usageLayout, meterGeometry, bezier } =
    await load('usage-presentation')
  const now = Date.parse('2026-01-01T00:00:00Z')
  const readings = [
    {
      kind: 'five_hour',
      label: '5-hour',
      percent: 68,
      resetsAt: '2026-01-01T02:15:00Z',
    },
    {
      kind: 'seven_day',
      label: 'Weekly',
      percent: 55,
      resetsAt: '2026-01-03T02:15:00Z',
    },
  ]
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({
    viewport: { width: 1600, height: 240 },
    deviceScaleFactor: 1,
  })
  const mount = async (source, width, height, colorScheme = 'light') => {
    assert.ok(
      Buffer.byteLength(source) <= 131072,
      'Scene must fit the SDK SVG source limit',
    )
    await page.mouse.move(1500, 200)
    await page.setContent(
      `<style>body{margin:0;background:${colorScheme === 'dark' ? '#212121' : '#f0efec'}}iframe{display:block;border:0;width:${width}px;height:${height}px}</style><iframe></iframe>`,
    )
    await page.locator('iframe').evaluate((frame, svg) => {
      frame.srcdoc = `<style>html,body{margin:0}</style>${svg}`
    }, source)
    const frame = page.frames()[1]
    await frame.waitForSelector('svg')
    return frame
  }
  const scene = (variant, columns, replay = false) => {
    const layout = usageLayout(columns * 8, variant)
    const sources = readings.map((reading, meterIndex) =>
      responsiveMeterSvg(reading, now, {
        layout: variant,
        ...(replay ? { entranceElapsed: 0, meterIndex } : {}),
      }),
    )
    return {
      layout,
      sources,
      source: usageStripSvg(
        replay ? markSvg(BRAND_MARK_SVG, 0) : BRAND_MARK_SVG,
        sources,
        layout,
        columns,
      ),
    }
  }
  let geometryCases = 0
  for (const colorScheme of ['light', 'dark']) {
    await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
    for (const variant of ['inline', 'current'])
      for (const columns of [24, 40, 59, 60, 61, 80, 120])
        for (const nativeCell of [6, 8.85, 12]) {
          const { source, layout } = scene(variant, columns)
          const width = Math.round(columns * nativeCell)
          const cell = width / columns
          const frame = await mount(source, width, layout.height, colorScheme)
          const fixedCells =
            (PRESENTATION.brandInset * 2 +
              PRESENTATION.markSlotSize +
              PRESENTATION.brandGap +
              (layout.inline ? PRESENTATION.inlineGap : 0)) /
            8
          const meterWidth = (width - fixedCells * cell) / (layout.inline ? 2 : 1)
          const metersX =
            ((PRESENTATION.brandInset +
              PRESENTATION.markSlotSize +
              PRESENTATION.brandGap) /
              8) *
            cell
          const geometry = await frame.evaluate(() =>
            [...document.querySelectorAll('.strip-meter')].map((node) => {
              const percent = node.querySelector('.percent')
              return {
                x: node.getCTM().e,
                y: node.getCTM().f,
                percentX: percent.getCTM().e,
                scale: percent.getCTM().a,
                dotScale: node.querySelector('.track-width').getCTM().a,
                dotRadius: node.querySelector('pattern .used').r.baseVal.value,
                dotPitch: node.querySelector('pattern').width.baseVal.value,
              }
            }),
          )
          for (let index = 0; index < 2; index++) {
            const expectedX =
              metersX +
              (index === 1 && layout.inline
                ? meterWidth + (PRESENTATION.inlineGap / 8) * cell
                : 0)
            assert.ok(
              Math.abs(geometry[index].x - expectedX) < 0.1,
              `Meter x: ${variant}/${columns}/${cell}`,
            )
            assert.ok(
              Math.abs(
                geometry[index].percentX -
                  expectedX -
                  meterGeometry(readings[index], meterWidth, now, variant).percentX,
              ) < 0.1,
              `Percentage x: ${variant}/${columns}/${cell}, actual ${geometry[index].percentX}, expected ${expectedX + meterGeometry(readings[index], meterWidth, now, variant).percentX}, meterWidth ${meterWidth}`,
            )
            assert.equal(geometry[index].scale, 1, 'Fonts retain their pixel size')
            assert.equal(geometry[index].dotScale, 1, 'Dots retain their pixel size')
            assert.equal(geometry[index].dotRadius * 2, PRESENTATION.dotSize)
            assert.equal(geometry[index].dotPitch, PRESENTATION.dotPitch)
            assert.equal(
              geometry[index].y,
              index === 1 && !layout.inline
                ? layout.meterHeight + layout.stackedGap
                : 0,
            )
          }
          assert.ok(
            Math.abs(
              width - geometry[1].x - meterWidth - (PRESENTATION.brandInset / 8) * cell,
            ) < 0.1,
            'Only the brand inset remains at the right edge',
          )
          geometryCases++
        }
  }
  let interactionCases = 0
  for (const variant of ['inline', 'current']) {
    const { source, layout } = scene(variant, 80)
    const frame = await mount(source, 640, layout.height)
    for (let index = 0; index < 2; index++) {
      const meter = frame.locator(`.strip-meter-${index}`)
      await meter.locator('.label').focus()
      assert.equal(
        await meter
          .locator('.reset')
          .evaluate((node) => getComputedStyle(node).opacity),
        '1',
      )
      await meter.locator('.pace').focus()
      assert.equal(
        await meter
          .locator('.pace-detail')
          .evaluate((node) => getComputedStyle(node).opacity),
        '1',
      )
      assert.equal(
        await meter
          .locator('.reset')
          .evaluate((node) => getComputedStyle(node).opacity),
        '0',
      )
      await meter.locator('.pace').evaluate((node) => node.blur())
      await meter.locator('.label').hover()
      await frame.waitForFunction(
        (index) =>
          getComputedStyle(document.querySelector(`.strip-meter-${index} .reset`))
            .opacity === '1',
        index,
      )
      await meter.locator('.pace').hover()
      await frame.waitForFunction(
        (index) =>
          getComputedStyle(document.querySelector(`.strip-meter-${index} .pace-detail`))
            .opacity === '1',
        index,
      )
      interactionCases++
    }
  }
  let fadeCases = 0
  for (const colorScheme of ['light', 'dark'])
    for (const reducedMotion of ['no-preference', 'reduce']) {
      await page.emulateMedia({ colorScheme, reducedMotion })
      for (const variant of ['inline', 'current']) {
        const { source, layout } = scene(variant, 80)
        const frame = await mount(
          fadeUsageSvg(source, 0, 'browser'),
          640,
          layout.height,
          colorScheme,
        )
        const duration = reducedMotion === 'reduce' ? 80 : 240
        const timing = await frame.evaluate(() => {
          const animations = document
            .getAnimations()
            .filter((animation) => animation instanceof CSSAnimation)
          for (const animation of animations) animation.pause()
          return animations.map((animation) => ({
            name: animation.animationName,
            ...animation.effect.getTiming(),
            cssEasing: getComputedStyle(animation.effect.target)
              .animationTimingFunction,
          }))
        })
        assert.equal(timing.length, 1, 'One animation fades the complete scene')
        assert.equal(timing[0].duration, duration)
        assert.equal(
          timing[0].cssEasing.replaceAll(' ', '').replaceAll('0.', '.'),
          USAGE_ENTRANCE.easing,
        )
        const samples = []
        for (const fraction of [1, 0, 0.25, 0.5]) {
          await frame.evaluate((time) => {
            document
              .getAnimations()
              .find((animation) =>
                animation.animationName?.startsWith('usage-enter'),
              ).currentTime = time
          }, fraction * duration)
          const screenshot = await page.locator('iframe').screenshot({ scale: 'css' })
          samples.push(
            await page.evaluate(async (data) => {
              const image = new Image()
              image.src = `data:image/png;base64,${data}`
              await image.decode()
              const canvas = document.createElement('canvas')
              canvas.width = image.width
              canvas.height = image.height
              const context = canvas.getContext('2d')
              context.drawImage(image, 0, 0)
              return [...context.getImageData(0, 0, image.width, image.height).data]
            }, screenshot.toString('base64')),
          )
        }
        const regions = await frame.evaluate(() =>
          ['.strip-mark', '.strip-meter-0', '.strip-meter-1'].map((selector) => {
            const bounds = document.querySelector(selector).getBoundingClientRect()
            return {
              x: bounds.x,
              y: bounds.y,
              right: bounds.right,
              bottom: bounds.bottom,
            }
          }),
        )
        const final = samples[0]
        const background = colorScheme === 'dark' ? [33, 33, 33] : [240, 239, 236]
        for (const region of regions) {
          const pixels = []
          for (
            let y = Math.max(0, Math.floor(region.y));
            y < Math.min(layout.height, Math.ceil(region.bottom));
            y++
          )
            for (
              let x = Math.max(0, Math.floor(region.x));
              x < Math.min(640, Math.ceil(region.right));
              x++
            ) {
              const offset = (y * 640 + x) * 4
              if (
                final[offset] - final[offset + 2] > 60 &&
                final[offset] > final[offset + 1] + 30
              )
                pixels.push(offset)
            }
          assert.ok(pixels.length > 4, 'Every region has final orange pixels')
          for (let index = 1; index < samples.length; index++) {
            const opacity =
              pixels.reduce((sum, offset) => {
                const delta = [0, 1, 2].map(
                  (channel) => final[offset + channel] - background[channel],
                )
                return (
                  sum +
                  delta.reduce(
                    (value, change, channel) =>
                      value +
                      change * (samples[index][offset + channel] - background[channel]),
                    0,
                  ) /
                    delta.reduce((value, change) => value + change * change, 0)
                )
              }, 0) / pixels.length
            const expected = bezier([0, 0.25, 0.5][index - 1], 0.23, 1, 0.32, 1)
            assert.ok(
              Math.abs(opacity - expected) < 0.025,
              `Pixel opacity ${opacity} must match ${expected} in every scene region`,
            )
          }
        }
        if (auditDirectory) {
          mkdirSync(auditDirectory, { recursive: true })
          await page.locator('iframe').screenshot({
            path: join(
              auditDirectory,
              `${colorScheme}-${reducedMotion}-${variant}.png`,
            ),
          })
        }
        fadeCases++
      }
    }
  let replayCases = 0
  for (const reducedMotion of ['no-preference', 'reduce'])
    for (const variant of ['inline', 'current']) {
      await page.emulateMedia({ reducedMotion })
      const { source, sources, layout } = scene(variant, 80, true)
      const animations = async (frame, selector) =>
        frame.evaluate(
          (selector) =>
            document
              .getAnimations()
              .filter((animation) => animation.effect.target.closest(selector))
              .map((animation) => {
                animation.pause()
                animation.currentTime = 1200
                const timing = animation.effect.getTiming()
                return {
                  name: animation.animationName.replace(
                    /^strip-(?:brand|meter-\d)-/,
                    '',
                  ),
                  duration: timing.duration,
                  delay: timing.delay,
                  easing: getComputedStyle(animation.effect.target)
                    .animationTimingFunction,
                  iterations: timing.iterations,
                  opacity: getComputedStyle(animation.effect.target).opacity,
                  transform: getComputedStyle(animation.effect.target).transform,
                }
              })
              .sort((a, b) => a.name.localeCompare(b.name)),
          selector,
        )
      const composite = await mount(source, 640, layout.height)
      const actual = []
      for (const selector of ['.strip-brand', '.strip-meter-0', '.strip-meter-1'])
        actual.push(await animations(composite, selector))
      for (let index = 0; index < 3; index++) {
        const original = await mount(
          index === 0 ? markSvg(BRAND_MARK_SVG, 0) : sources[index - 1],
          index === 0 ? 45 : layout.meterWidth,
          index === 0 ? 45 : layout.meterHeight,
        )
        assert.deepEqual(
          actual[index],
          await animations(original, 'svg'),
          'Composition preserves original replay timing and final state',
        )
      }
      replayCases++
    }
  let statusCases = 0
  for (const columns of [24, 80])
    for (const cell of [6, 8.85, 12])
      for (const refresh of [false, true]) {
        const width = Math.round(columns * cell)
        const effectiveCell = width / columns
        const status = usageStatusSvg('Checking usage...')
        const secondary = secondaryTextSvg(['Additional usage information'], 600)
        const frame = await mount(
          usageStatusStripSvg(BRAND_MARK_SVG, status, columns, secondary, refresh),
          width,
          status.height + secondary.height,
        )
        const result = await frame.evaluate(() => {
          const mark = document.querySelector('.strip-mark').getBoundingClientRect()
          const primary = document.querySelector('.strip-status')
          const secondary = document.querySelector('.strip-status-secondary')
          return {
            markCenter: (mark.top + mark.bottom) / 2,
            primaryX: primary.getCTM().e,
            secondaryX: secondary.getCTM().e,
            secondaryY: secondary.getCTM().f,
            clip: parseFloat(
              getComputedStyle(document.querySelector('#strip-status-viewport rect'))
                .width,
            ),
          }
        })
        const inset =
          ((PRESENTATION.brandInset +
            PRESENTATION.markSlotSize +
            PRESENTATION.brandGap) /
            8) *
          effectiveCell
        assert.ok(Math.abs(result.markCenter - status.height / 2) < 0.1)
        assert.ok(Math.abs(result.primaryX - inset) < 0.1)
        assert.equal(result.secondaryX, 0)
        assert.equal(result.secondaryY, status.height)
        assert.ok(
          Math.abs(result.clip - (width - inset - (refresh ? 4 * effectiveCell : 0))) <
            0.1,
        )
        statusCases++
      }
  console.log(
    `Passed ${geometryCases} responsive scene geometry, ${interactionCases} focus/hover, ${fadeCases} pixel fade, ${replayCases} original replay, and ${statusCases} status geometry cases. Browser evidence does not establish native SDK rendering.`,
  )
} finally {
  await browser?.close()
  rmSync(temporary, { recursive: true, force: true })
}
