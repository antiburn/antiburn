import assert from 'node:assert/strict'
import { writeFile } from 'node:fs/promises'
import { join } from 'node:path'

async function markerPixels(page, image, y, bounds) {
  return page.evaluate(
    async ({ encoded, y, bounds }) => {
      const bitmap = new Image()
      bitmap.src = `data:image/png;base64,${encoded}`
      await bitmap.decode()
      const canvas = document.createElement('canvas')
      canvas.width = bitmap.width
      canvas.height = bitmap.height
      const context = canvas.getContext('2d')
      context.drawImage(bitmap, 0, 0)
      const { data } = context.getImageData(0, 0, canvas.width, canvas.height)
      const start = bounds ? Math.floor(bounds.left * devicePixelRatio) : 0
      const end = bounds ? Math.ceil((bounds.left + bounds.width) * devicePixelRatio) : canvas.width
      const scores = Array.from({ length: end - start }, (_, index) => {
        const x = start + index
        let score = 0
        for (let row = y; row < y + 3; row++)
          for (let channel = 0; channel < 3; channel++)
            score += Math.abs(
              data[(row * canvas.width + x) * 4 + channel] - data[(row * canvas.width + start) * 4 + channel],
            )
        return score
      })
      const peak = Math.max(...scores)
      if (peak < 20) throw new Error('The base image has no visible pace marker')
      // Use the leading half-contrast edge and the two-pixel marker width.
      const first = scores.findIndex((score) => score >= peak * 0.5)
      const previous = scores[first - 1] ?? 0
      const edge = first - 0.5 + (peak * 0.5 - previous) / (scores[first] - previous)
      return (start + edge) / devicePixelRatio + 1 - (bounds?.left ?? 0)
    },
    { encoded: image.toString('base64'), y, bounds },
  )
}

export async function checkPairedUsageHover({ browser, responsiveMeterSvg, meterSvg, meterLayer, auditDirectory }) {
  const context = await browser.newContext({
    viewport: { width: 1100, height: 150 },
    deviceScaleFactor: 2,
  })
  const page = await context.newPage()
  const now = Date.parse('2026-01-01T00:00:00Z')
  const readings = [
    { kind: 'five_hour', label: '5-hour', percent: 9, resetsAt: '2026-01-01T01:06:00Z' },
    { kind: 'seven_day', label: 'Weekly', percent: 63, resetsAt: '2026-01-03T12:28:48Z' },
  ]
  let cases = 0
  let negativeOffset
  const measurements = []
  try {
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
      for (const layout of ['inline', 'current']) {
        const height = layout === 'inline' ? 18 : 33.75
        for (const reading of readings) {
          const source = responsiveMeterSvg(reading, now, { layout })
          await page.setContent(
            `<style>body{margin:0;background:${colorScheme === 'dark' ? '#212121' : '#f0efec'}}.slot{position:relative;display:flex;align-items:center;width:440px;height:${height}px}.overlay{position:absolute;inset:0;display:flex;align-items:center}img,iframe{display:block;border:0;max-width:100%;height:${height}px}</style><div class="slot"><img width="4096"><div class="overlay"><iframe width="4096"></iframe></div></div>`,
          )
          await page.locator('img').evaluate(
            (image, source) => {
              image.src = `data:image/svg+xml,${encodeURIComponent(source)}`
              return image.decode()
            },
            meterLayer(source, 'base'),
          )
          await page.locator('iframe').evaluate(
            (frame, source) => {
              frame.srcdoc = `<style>html,body{margin:0}</style>${source}`
            },
            meterLayer(source, 'interaction'),
          )
          const frame = page.frames()[1]
          await frame.waitForSelector('.pace')
          const frameIdentity = await frame.evaluate(() => {
            window.resizeIdentity = Math.random()
            return window.resizeIdentity
          })
          for (const width of [440, 400, 340, 302.49, 302.5, 288, 255.49, 255.5, 200, 160, 288, 440]) {
            await page.mouse.move(1000, 100)
            await page.locator('.slot').evaluate((slot, width) => {
              slot.style.width = `${width}px`
            }, width)
            await page.locator('.overlay').evaluate((overlay) => {
              overlay.style.visibility = 'hidden'
            })
            const baseImage = await page.locator('.slot').screenshot()
            const baseMarker = await markerPixels(page, baseImage, layout === 'inline' ? 1 : 63)
            await page.locator('.overlay').evaluate((overlay) => {
              overlay.style.visibility = 'visible'
            })
            const geometry = await frame.evaluate(() => {
              const marker = document.querySelector('.marker').getBoundingClientRect()
              const hit = document.querySelector('.pace .hit').getBoundingClientRect()
              return {
                marker: marker.x,
                y: marker.y + marker.height / 2,
                hit: { left: hit.left, right: hit.right, top: hit.top, bottom: hit.bottom },
                identity: window.resizeIdentity,
              }
            })
            const bounds = await page.locator('img,iframe').evaluateAll((nodes) =>
              nodes.map((node) => {
                const box = node.getBoundingClientRect()
                return { left: box.left, width: box.width, height: box.height }
              }),
            )
            assert.deepEqual(bounds[0], bounds[1], 'The image and interaction frame must fill the same slot')
            assert.equal(geometry.identity, frameIdentity, 'Resize must preserve the interaction document')
            assert.ok(
              Math.abs(baseMarker - geometry.marker) <= 0.5,
              `${colorScheme}/${layout}/${reading.kind}/${width}: visible marker ${baseMarker}, interaction ${geometry.marker}`,
            )
            assert.ok(
              baseMarker >= geometry.hit.left && baseMarker <= geometry.hit.right,
              'The hit region must contain the visible base marker',
            )
            measurements.push({
              colorScheme,
              layout,
              meter: reading.kind,
              width,
              baseMarker,
              interactionMarker: geometry.marker,
            })
            const capture = async (state) => {
              if (auditDirectory)
                await page.locator('.slot').screenshot({
                  path: join(auditDirectory, `paired-${colorScheme}-${layout}-${reading.kind}-${width}-${state}.png`),
                  scale: 'css',
                })
            }
            if ([160, 288, 340, 400, 440].includes(width)) {
              await capture('rest')
              await page.mouse.move(baseMarker, geometry.y)
              await frame.waitForFunction(
                () => getComputedStyle(document.querySelector('.pace-detail')).opacity === '1',
              )
              assert.equal(await frame.locator('.pace').evaluate((node) => node.matches(':hover')), true)
              const markerAfterHover = await frame.locator('.marker').evaluate((node) => node.getBoundingClientRect().x)
              assert.equal(markerAfterHover, geometry.marker, 'Pace reveal must not move the marker')
              await capture('pace')
              const label = await frame.locator('.label .hit').boundingBox()
              await page.mouse.move(label.x + label.width / 2, label.y + label.height / 2)
              await frame.waitForFunction(() => getComputedStyle(document.querySelector('.reset')).opacity === '1')
              assert.equal(
                await frame.locator('.marker').evaluate((node) => node.getBoundingClientRect().x),
                geometry.marker,
                'Reset reveal must not move the marker',
              )
              await capture('reset')
              await frame.locator('.pace').focus()
              await frame.waitForFunction(
                () => getComputedStyle(document.querySelector('.pace-detail')).opacity === '1',
              )
              await frame.locator('.pace').evaluate((node) => node.blur())
              await page.mouse.move(1000, 100)
              await frame.waitForFunction(() =>
                ['.reset', '.pace-detail'].every(
                  (selector) => getComputedStyle(document.querySelector(selector)).opacity === '0',
                ),
              )
              await capture('dismissed')
            }
            cases++
          }
        }
      }
    }
    await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' })
    const source = meterSvg(readings[1], 300, now, false, true, { layout: 'inline' })
    await page.setContent(
      '<style>body{margin:0;background:#f0efec}.slot{position:relative;width:288px;height:18px}img{max-width:100%;height:18px}.overlay{position:absolute;left:0;top:0;width:300px}iframe{border:0;height:18px}</style><div class="slot"><img width="300"><div class="overlay"><iframe width="300"></iframe></div></div>',
    )
    await page.locator('img').evaluate(
      (image, source) => {
        image.src = `data:image/svg+xml,${encodeURIComponent(source)}`
        return image.decode()
      },
      meterLayer(source, 'base'),
    )
    await page.locator('iframe').evaluate(
      (frame, source) => {
        frame.srcdoc = `<style>html,body{margin:0}</style>${source}`
      },
      meterLayer(source, 'interaction'),
    )
    const frame = page.frames()[1]
    await frame.waitForSelector('.pace')
    await page.locator('.overlay').evaluate((overlay) => {
      overlay.style.visibility = 'hidden'
    })
    const base = await page.locator('.slot').screenshot()
    const visible = await markerPixels(page, base, 3)
    await page.locator('.overlay').evaluate((overlay) => {
      overlay.style.visibility = 'visible'
    })
    const nominal = await frame.locator('.marker').evaluate((node) => node.getBoundingClientRect().x)
    negativeOffset = Math.abs(nominal - visible)
    assert.ok(negativeOffset > 1, 'The old nominal-width overlay must fail the paired alignment check')
    if (auditDirectory) {
      await page.mouse.move(nominal, 9)
      await frame.waitForFunction(() => getComputedStyle(document.querySelector('.pace-detail')).opacity === '1')
      await page
        .locator('.slot')
        .screenshot({ path: join(auditDirectory, 'paired-old-layout-failure.png'), scale: 'css' })
      await writeFile(
        join(auditDirectory, 'paired-hover-measurements.json'),
        JSON.stringify({ cases, negativeOffset, measurements }, null, 2) + '\n',
      )
    }
    return { cases, negativeOffset }
  } finally {
    await context.close()
  }
}

export async function checkPaceCaptionAnchoring({ browser, responsiveMeterSvg, auditDirectory }) {
  const page = await browser.newPage({ viewport: { width: 600, height: 70 } })
  const now = Date.parse('2026-01-01T00:00:00Z')
  let cases = 0
  try {
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
      for (const layout of ['inline', 'current']) {
        for (const value of [0.05, 0.4, 0.5, 0.55, 0.9, 0.99]) {
          const reading = {
            kind: 'five_hour',
            label: '5-hour',
            percent: 9,
            resetsAt: new Date(now + (1 - value) * 5 * 3600000).toISOString(),
          }
          await page.setContent(
            '<style>body{margin:0}</style><iframe style="border:0;width:440px;height:40px"></iframe>',
          )
          await page.locator('iframe').evaluate(
            (node, source) => {
              node.srcdoc = `<style>html,body{margin:0}</style>${source}`
            },
            responsiveMeterSvg(reading, now, { layout }),
          )
          const frame = page.frames()[1]
          await frame.waitForSelector('.pace')
          for (const width of [440, 400, 340, 302.49, 302.5, 288, 255.49, 255.5, 160, 440]) {
            await page.locator('iframe').evaluate((node, width) => {
              node.style.width = `${width}px`
            }, width)
            await frame.locator('.pace').focus()
            await assertPaceCaption(frame, `${colorScheme}/${layout}/${value}/${width}`)
            if (auditDirectory && [340, 400].includes(width))
              await page.locator('iframe').screenshot({
                path: join(auditDirectory, `anchored-${colorScheme}-${layout}-${value}-${width}.png`),
                scale: 'css',
              })
            cases++
          }
        }
      }
    }
    return cases
  } finally {
    await page.close()
  }
}

async function assertPaceCaption(frame, tag) {
  const geometry = await frame.evaluate(() => {
    const copies = [...document.querySelectorAll('.pace-detail [class^="pace-copy-"]')].filter(
      (node) => getComputedStyle(node).display !== 'none' && Number(getComputedStyle(node).opacity) === 1,
    )
    const copy = copies[0]?.getBoundingClientRect()
    const marker = document.querySelector('.marker').getBoundingClientRect().x
    const bounds = document.querySelector('#reset-bounds rect')
    const clipLeft = document.querySelector('.pace-detail').parentElement.getCTM().e + bounds.x.baseVal.value
    const clipRight = clipLeft + parseFloat(getComputedStyle(bounds).width)
    return { copies: copies.length, left: copy?.left, right: copy?.right, marker, clipLeft, clipRight }
  })
  assert.equal(geometry.copies, 1, `${tag}: exactly one pace caption must be visible`)
  const leftRoom = Math.max(0, Math.min(geometry.clipRight, geometry.marker - 6) - geometry.clipLeft)
  const rightRoom = Math.max(0, geometry.clipRight - Math.max(geometry.clipLeft, geometry.marker + 6))
  const onRight = rightRoom >= leftRoom - 0.001
  assert.ok(
    geometry.left >= geometry.clipLeft - 0.1 && geometry.right <= geometry.clipRight + 0.1,
    `${tag}: caption must fit its track`,
  )
  if (onRight) {
    assert.ok(geometry.left >= geometry.marker + 5, `${tag}: caption must sit to the right of the marker`)
    assert.ok(
      geometry.left <= Math.max(geometry.clipLeft, geometry.marker + 6) + 2,
      `${tag}: caption must pin its left edge beside the marker`,
    )
  } else {
    assert.ok(geometry.right <= geometry.marker - 5, `${tag}: caption must sit to the left of the marker`)
    assert.ok(
      geometry.right >= Math.min(geometry.clipRight, geometry.marker - 6) - 3,
      `${tag}: caption must pin its right edge beside the marker`,
    )
  }
  return geometry
}

export async function checkStripPaceResize({
  browser,
  responsiveMeterSvg,
  meterLayer,
  usageLayout,
  PRESENTATION,
  auditDirectory,
}) {
  const context = await browser.newContext({ viewport: { width: 1100, height: 150 }, deviceScaleFactor: 2 })
  const page = await context.newPage()
  const now = Date.parse('2026-01-01T00:00:00Z')
  const readings = [
    { kind: 'five_hour', label: '5-hour', percent: 9, resetsAt: '2026-01-01T03:54:00Z' },
    { kind: 'seven_day', label: 'Weekly', percent: 63, resetsAt: '2026-01-03T12:28:48Z' },
  ]
  let cases = 0
  try {
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme, reducedMotion: 'no-preference' })
      for (const layout of ['inline', 'current']) {
        for (const cell of [8, 8.85]) {
          const height = layout === 'inline' ? 18 : 33.75
          await page.setContent(
            `<style>body{margin:0;background:${colorScheme === 'dark' ? '#212121' : '#f0efec'}}.band,.meters,.slot,.overlay{display:flex;align-items:center}.band{width:768px}.meters{flex:1;min-width:0}.slot{position:relative;flex:1;min-width:0;height:${height}px}.overlay{position:absolute;inset:0}img,iframe{display:block;border:0;max-width:100%;height:${height}px}</style><div class="band"><div style="flex:none;width:${((PRESENTATION.brandInset + PRESENTATION.markSlotSize + PRESENTATION.brandGap) * cell) / 8}px"></div><div class="meters">${readings.map(() => '<div class="slot"><img width="4096"><div class="overlay"><iframe width="4096"></iframe></div></div>').join('')}</div><div style="flex:none;width:${(PRESENTATION.brandInset * cell) / 8}px"></div></div>`,
          )
          for (const [index, reading] of readings.entries()) {
            const source = responsiveMeterSvg(reading, now, { layout })
            await page
              .locator('img')
              .nth(index)
              .evaluate(
                (image, source) => {
                  image.src = `data:image/svg+xml,${encodeURIComponent(source)}`
                  return image.decode()
                },
                meterLayer(source, 'base'),
              )
            await page
              .locator('iframe')
              .nth(index)
              .evaluate(
                (frame, source) => {
                  frame.srcdoc = `<style>html,body{margin:0}</style>${source}`
                },
                meterLayer(source, 'interaction'),
              )
          }
          const frames = page.frames().slice(1)
          for (const frame of frames) await frame.waitForSelector('.pace')
          const identities = await Promise.all(
            frames.map((frame) =>
              frame.evaluate(() => {
                window.resizeIdentity = Math.random()
                return window.resizeIdentity
              }),
            ),
          )
          const breakpoint = 60 * cell
          for (const width of [
            768,
            641,
            640,
            639,
            breakpoint + 1,
            breakpoint + 0.01,
            breakpoint,
            breakpoint - 0.01,
            breakpoint - 1,
            400,
            340,
            768,
          ]) {
            await page.mouse.move(1050, 130)
            const scene = usageLayout((width * 8) / cell, layout)
            await page.locator('.band').evaluate((band, width) => {
              band.style.width = `${width}px`
            }, width)
            await page.locator('.meters').evaluate(
              (meters, scene) => {
                meters.style.flexDirection = scene.inline ? 'row' : 'column'
                meters.style.alignItems = 'stretch'
                meters.style.gap = `${scene.gap}px`
              },
              {
                inline: scene.inline,
                gap: scene.inline ? (PRESENTATION.inlineGap * cell) / 8 : (scene.stackedGap * cell) / 16,
              },
            )
            const boxes = await page.locator('.slot').evaluateAll((slots) =>
              slots.map((slot) => {
                const box = slot.getBoundingClientRect()
                return { x: box.x, y: box.y, width: box.width }
              }),
            )
            assert.equal(
              Math.abs(boxes[0].y - boxes[1].y) < 0.01,
              scene.inline,
              'The strip must cross its inline/stacked breakpoint',
            )
            for (const [index, frame] of frames.entries()) {
              const overlay = page.locator('.overlay').nth(index)
              await overlay.evaluate((node) => {
                node.style.visibility = 'hidden'
              })
              const visible = await markerPixels(
                page,
                await page.screenshot(),
                Math.floor(boxes[index].y * 2) + (layout === 'inline' ? 1 : 63),
                { left: boxes[index].x, width: boxes[index].width },
              )
              await overlay.evaluate((node) => {
                node.style.visibility = 'visible'
              })
              const geometry = await frame.locator('.marker').evaluate((node) => {
                const box = node.getBoundingClientRect()
                return { x: box.x, y: box.y + box.height / 2 }
              })
              assert.ok(
                Math.abs(visible - geometry.x) <= 0.5,
                `The strip base and hover marker must align after resize: ${JSON.stringify({ colorScheme, layout, cell, width, index, visible, geometry, box: boxes[index] })}`,
              )
              assert.equal(
                await frame.evaluate(() => window.resizeIdentity),
                identities[index],
                'Breakpoint resize must preserve the interaction document',
              )
              await page.mouse.move(boxes[index].x + visible, boxes[index].y + geometry.y)
              await frame.waitForFunction(
                () => getComputedStyle(document.querySelector('.pace-detail')).opacity === '1',
              )
              assert.equal(await frame.locator('.pace').evaluate((node) => node.matches(':hover')), true)
              await assertPaceCaption(frame, `strip/${colorScheme}/${layout}/${cell}/${width}/${index}`)
              if (auditDirectory && [breakpoint - 0.01, breakpoint, breakpoint + 0.01, 340, 400].includes(width))
                await page.locator('.band').screenshot({
                  path: join(auditDirectory, `strip-${colorScheme}-${layout}-${cell}-${width}-${index}-pace.png`),
                  scale: 'css',
                })
              await page.mouse.move(1050, 130)
              await frame.waitForFunction(
                () => getComputedStyle(document.querySelector('.pace-detail')).opacity === '0',
              )
              cases++
            }
          }
        }
      }
    }
    return cases
  } finally {
    await context.close()
  }
}
