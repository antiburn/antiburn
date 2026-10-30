import { useCallback, useState, useSyncExternalStore } from "react"
import type { PromotionDefinition } from "../../promotions/types"

type Media = NonNullable<PromotionDefinition["screens"]["announcement"]["media"]>

/** Show the poster when the reader requests less motion. */
class VideoPlayback {
  attach = (video: HTMLVideoElement): (() => void) => {
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)")
    const sync = () => {
      const still = reduced.matches
      video.dataset.still = String(still)
      if (still) {
        video.pause()
        video.currentTime = 0
      } else {
        void video.play().catch(() => undefined)
      }
    }
    reduced.addEventListener("change", sync)
    video.addEventListener("canplay", sync)
    sync()
    return () => {
      reduced.removeEventListener("change", sync)
      video.removeEventListener("canplay", sync)
      video.pause()
    }
  }
}

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
  return (
    <PromotionMediaContent
      key={`${source.kind}:${source.src}:${source.poster ?? ""}`}
      media={source}
    />
  )
}

function PromotionMediaContent({ media }: { media: Media }) {
  const [playback] = useState(() => new VideoPlayback())
  const attach = useCallback(
    (node: HTMLVideoElement | null) => (node ? playback.attach(node) : undefined),
    [playback],
  )
  const [failed, setFailed] = useState(false)
  const animateImage = useSyncExternalStore(subscribeImageMotion, canAnimateImage, () => false)
  const imageSource = media.poster && (failed || !animateImage) ? media.poster : media.src
  return (
    <div className="promotion-media" role="img" aria-label={media.alt}>
      {media.kind === "video" && !failed ? (
        <>
          <img className="promotion-poster" src={media.poster} alt="" aria-hidden="true" />
          <video
            ref={attach}
            src={media.src}
            poster={media.poster}
            aria-hidden="true"
            muted
            loop
            playsInline
            preload="metadata"
            onError={() => setFailed(true)}
          />
        </>
      ) : (
        <img
          src={media.kind === "video" ? media.poster : imageSource}
          onError={media.poster && !failed ? () => setFailed(true) : undefined}
          alt=""
          aria-hidden="true"
        />
      )}
    </div>
  )
}
