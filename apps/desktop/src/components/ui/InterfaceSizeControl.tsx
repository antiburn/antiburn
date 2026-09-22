import { PushButton } from "./PushButton"

export function InterfaceSizeControl({
  percent,
  presets,
  disabled,
  onChange,
  onReset,
}: {
  percent: number
  presets: readonly number[]
  disabled: boolean
  onChange: (percent: number) => void
  onReset: () => void
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <select
        className="ui-push-button tabular-nums"
        aria-label="Interface size"
        value={percent}
        disabled={disabled}
        onChange={(event) => onChange(Number(event.currentTarget.value))}
      >
        {presets.map((value) => (
          <option key={value} value={value}>
            {value}%
          </option>
        ))}
      </select>
      <PushButton disabled={disabled || percent === 100} onClick={onReset}>
        Reset to 100%
      </PushButton>
    </div>
  )
}
