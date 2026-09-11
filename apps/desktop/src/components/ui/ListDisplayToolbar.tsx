import { cn } from "../../lib/cn"
import { SegmentedControl, type SegmentedOption } from "./SegmentedControl"

export function ListDisplayToolbar<T extends string>({
  label,
  options,
  value,
  onChange,
  ariaLabel,
  dragRegion = false,
  className,
}: {
  label?: string
  options: ReadonlyArray<SegmentedOption<T>>
  value: T
  onChange: (next: T) => void
  ariaLabel: string
  dragRegion?: boolean
  className?: string
}) {
  return (
    <div
      data-list-display-toolbar=""
      data-tauri-drag-region={dragRegion ? "deep" : undefined}
      className={cn(
        "mb-1 flex h-8 shrink-0 items-center px-3",
        label ? "justify-between" : "justify-end",
        className,
      )}
    >
      {label && <span className="type-caption font-medium text-label-tertiary">{label}</span>}
      <SegmentedControl
        options={options}
        value={value}
        onChange={onChange}
        ariaLabel={ariaLabel}
        className="normal-case"
        variant="text-tabs"
      />
    </div>
  )
}
