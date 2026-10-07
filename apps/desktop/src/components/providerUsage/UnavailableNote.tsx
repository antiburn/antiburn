import { cn } from "../../lib/cn"
import {
  liveErrorHasDocs,
  liveErrorNote,
  type UnavailableLiveProvider,
} from "../../lib/presentation/liveUsage"
import { LimitsDocsLink } from "./LimitsDocsLink"

/**
 * Why a provider has no limits to show, with the docs link when the reason
 * is one antiburn cannot fix. The popover bar and Overview both use it.
 */
export function UnavailableNote({
  entry,
  className,
}: {
  entry: UnavailableLiveProvider
  className?: string
}) {
  return (
    <p className={cn("type-footnote text-label-secondary", className)}>
      {liveErrorNote(entry.category, entry.provider, entry.detail)}
      {liveErrorHasDocs(entry.detail) && (
        <>
          {" "}
          <LimitsDocsLink />
        </>
      )}
    </p>
  )
}
