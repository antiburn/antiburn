import { act, render } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"

import { BurnCheckDetailBody } from "./BurnCheckDetailBody"

vi.mock("../../../components/ui/ScrollPane", () => ({
  ScrollPane: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))

afterEach(() => vi.unstubAllGlobals())

it("hides the watermark when content grows and stops observing hidden details", () => {
  let resize = () => {}
  const disconnect = vi.fn()
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        resize = callback
      }
      observe() {}
      disconnect = disconnect
    },
  )
  const content = <div className="burn-checks-detail-content">Samples</div>
  const { container, rerender } = render(
    <BurnCheckDetailBody visible>{content}</BurnCheckDetailBody>,
  )
  const body = container.querySelector<HTMLElement>(".burn-check-detail-body")!
  const samples = container.querySelector<HTMLElement>(".burn-checks-detail-content")!
  const mark = container.querySelector<HTMLElement>(".burn-check-background-mark")!
  Object.defineProperty(body, "clientHeight", { value: 800 })
  Object.defineProperty(mark, "offsetHeight", { value: 280 })
  Object.defineProperty(samples, "offsetHeight", { value: 300, configurable: true })
  act(() => resize())
  expect(body.dataset.watermarkRoom).toBe("true")
  Object.defineProperty(samples, "offsetHeight", { value: 600 })
  act(() => resize())
  expect(body.dataset.watermarkRoom).toBe("false")
  rerender(<BurnCheckDetailBody visible={false}>{content}</BurnCheckDetailBody>)
  expect(disconnect).toHaveBeenCalledOnce()
  expect(body.dataset.watermarkRoom).toBeUndefined()
})
