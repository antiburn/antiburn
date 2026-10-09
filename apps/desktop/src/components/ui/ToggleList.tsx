import type { ReactNode } from "react"

/** The first line of a toggle list in a `Card`. It says what the switches do. */
export function ToggleListIntro({ children }: { children: ReactNode }) {
  return <p className="px-4 py-3 type-footnote text-label-secondary">{children}</p>
}

/**
 * The rows of a toggle list. The list owns the columns and each row is a
 * subgrid, so the facts and the controls line up from row to row.
 */
export function ToggleList({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-[auto_minmax(0,1fr)_auto_auto] gap-x-2.5 divide-y divide-separator">
      {children}
    </div>
  )
}

/**
 * One row of a `ToggleList`: an icon, a name with an optional `detail` after
 * it, short `facts`, and the controls on the right. `children` shows under the
 * name. `status` is a short text that takes the place of both `facts` and
 * `controls`. It spans both columns and aligns to the right edge.
 */
export function ToggleListRow({
  icon,
  name,
  detail,
  facts,
  controls,
  status,
  children,
}: {
  icon: ReactNode
  name: ReactNode
  detail?: ReactNode
  facts?: ReactNode
  controls?: ReactNode
  status?: ReactNode
  children?: ReactNode
}) {
  return (
    <div className="col-span-full grid grid-cols-subgrid items-center px-4 py-2">
      <span className="flex w-5 justify-center">{icon}</span>

      <span className="flex min-w-0 items-center gap-x-4">
        <span className="flex min-w-0 items-center gap-1.5 type-callout text-label">
          {name}
        </span>
        {detail}
      </span>

      {status ? (
        <span className="col-span-2 justify-self-end type-footnote text-label-tertiary">
          {status}
        </span>
      ) : (
        <>
          <span className="type-footnote text-label-tertiary">{facts}</span>
          <span className="flex items-center gap-1.5">{controls}</span>
        </>
      )}

      {children && <div className="col-span-2 col-start-2 min-w-0">{children}</div>}
    </div>
  )
}
