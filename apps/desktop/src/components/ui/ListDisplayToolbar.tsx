import { CollectionToolbar } from "./CollectionToolbar"
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
    <CollectionToolbar
      dragRegion={dragRegion}
      className={[label ? "justify-between" : "justify-end", className]
        .filter(Boolean)
        .join(" ")}
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
    </CollectionToolbar>
  )
}
