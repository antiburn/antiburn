import { useState, useSyncExternalStore } from "react"
import type { PromotionDefinition } from "../../promotions/types"

type Media = NonNullable<PromotionDefinition["screens"]["announcement"]["media"]>

function canAnimateImage(): boolean {
  return !window.matchMedia("(prefers-reduced-motion: reduce)").matches
}

function subscribeImageMotion(listener: () => void): () => void {
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)")
  reduced.addEventListener("change", listener)
  return () => {
    reduced.removeEventListener("change", listener)
  }
}

function lightTheme(): boolean {
  const theme = document.documentElement.dataset.theme
  return (
    theme === "light" ||
    (theme !== "dark" && !window.matchMedia("(prefers-color-scheme: dark)").matches)
  )
}

function subscribeTheme(listener: () => void): () => void {
  const system = window.matchMedia("(prefers-color-scheme: dark)")
  const observer = new MutationObserver(listener)
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  })
  system.addEventListener("change", listener)
  return () => {
    observer.disconnect()
    system.removeEventListener("change", listener)
  }
}

export function PromotionMedia({ media }: { media: Media }) {
  const light = useSyncExternalStore(subscribeTheme, lightTheme, () => false)
  const source = light && media.light ? { ...media.light, alt: media.alt } : media
  return <PromotionMediaContent key={`${source.src}:${source.poster ?? ""}`} media={source} />
}

function PromotionMediaContent({ media }: { media: Media }) {
  const [failed, setFailed] = useState(false)
  const animateImage = useSyncExternalStore(subscribeImageMotion, canAnimateImage, () => false)
  // An animated source loops from its own file data. The poster is the still
  // frame for a reader who requests less motion, and for a load failure.
  const imageSource = media.poster && (failed || !animateImage) ? media.poster : media.src
  return (
    <div className="promotion-media" role="img" aria-label={media.alt}>
      <img
        src={imageSource}
        // A broken poster has no further fallback, so it must not set the flag.
        onError={media.poster && !failed && animateImage ? () => setFailed(true) : undefined}
        alt=""
        aria-hidden="true"
      />
    </div>
  )
}
