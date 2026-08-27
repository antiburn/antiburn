import type { ReactNode, Ref } from "react"

import { cn } from "../../lib/cn"

/** Title bar of a content pane: an optional leading control (a back arrow, say),
 *  the pane title, and an optional trailing action.
 *
 *  The title is the only `type-title-2` in a pane, and the bottom margin matches
 *  the gap `Pane` puts between groups, so the title reads as a peer break rather
 *  than as part of the first group.
 *
 *  `type-title-2` (17px/600) rather than the larger `type-title-1`: at 22px the
 *  title dwarfed a pane whose tallest element is a 58px row, and the step down
 *  still clears the 15px `SectionGroup` headers under it on both size and
 *  weight — the rest of the pane is regular weight throughout. */
export function PaneHeader({
  title,
  leading,
  trailing,
  headingRef,
  className = "",
}: {
  title: string
  leading?: ReactNode
  trailing?: ReactNode
  /** Set by a pane that pushes a subview, so it can move focus to the new
   *  title when the surface changes. Taking the ref is what makes the heading
   *  programmatically focusable — `tabIndex={-1}` follows it rather than
   *  sitting on every pane title, since a pane nobody navigates *into* has no
   *  reason to be a focus target. */
  headingRef?: Ref<HTMLHeadingElement>
  className?: string
}) {
  return (
    <div className={cn("mb-6 flex items-center gap-2", className)}>
      {leading}
      <h1
        ref={headingRef}
        data-view-heading={headingRef ? "" : undefined}
        tabIndex={headingRef ? -1 : undefined}
        className="type-title-2 min-w-0 flex-1 text-balance text-label outline-none"
      >
        {title}
      </h1>
      {trailing}
    </div>
  )
}

/** A content pane: a `PaneHeader` over a vertical stack of groups.
 *
 *  The `space-y-6` (24px, `--space-2xl`) stack is the one definition of how far
 *  apart two `SectionGroup`s sit — 3× a group's own header-to-card gap, so a
 *  group break never reads as tighter than the break inside one. A pane that
 *  owns its body layout (a full-height list, a subview) composes `PaneHeader`
 *  directly instead. */
export function Pane({
  title,
  trailing,
  children,
  className = "",
}: {
  title: string
  trailing?: ReactNode
  children: ReactNode
  className?: string
}) {
  return (
    <>
      <PaneHeader title={title} trailing={trailing} />
      <div className={cn("space-y-6", className)}>{children}</div>
    </>
  )
}
