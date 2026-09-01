import "@testing-library/jest-dom/vitest"
import { cleanup } from "@testing-library/react"
import { afterEach } from "vitest"

const nativeOffsetHeight = Object.getOwnPropertyDescriptor(
  HTMLElement.prototype,
  "offsetHeight",
)?.get
const nativeOffsetWidth = Object.getOwnPropertyDescriptor(
  HTMLElement.prototype,
  "offsetWidth",
)?.get

// jsdom has no layout. Give virtual lists stable DOM geometry for component tests.
Object.defineProperties(HTMLElement.prototype, {
  offsetHeight: {
    configurable: true,
    get(this: HTMLElement) {
      if (this.classList.contains("ui-scroll-viewport")) return 600
      if (this.dataset.virtualKind === "heading") {
        return this.querySelector(".sr-only") ? 0 : 28
      }
      if (this.dataset.virtualKind === "row") return 72
      return nativeOffsetHeight?.call(this) ?? 0
    },
  },
  offsetWidth: {
    configurable: true,
    get(this: HTMLElement) {
      if (this.classList.contains("ui-scroll-viewport")) return 356
      if (this.dataset.virtualKind) return 340
      return nativeOffsetWidth?.call(this) ?? 0
    },
  },
})

afterEach(() => {
  cleanup()
})
