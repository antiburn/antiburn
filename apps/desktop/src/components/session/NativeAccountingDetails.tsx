import { nativeAccountingRows } from "../../lib/presentation/nativeAccounting"
import type { PresentableModelRun } from "../../lib/presentation/models"
import type {
  NativeCalculatedCost,
  NativeSourceAccounting,
  NativeTokenClasses,
  NativeUsageReconciliation,
} from "../../lib/types/session"

export function NativeAccountingDetails({
  accounting,
  modelRuns,
  tokenReconciliation,
  costReconciliation,
}: {
  accounting: NativeSourceAccounting | null | undefined
  modelRuns: readonly PresentableModelRun[]
  tokenReconciliation?: NativeUsageReconciliation<NativeTokenClasses> | null | undefined
  costReconciliation?: NativeUsageReconciliation<NativeCalculatedCost> | null | undefined
}) {
  if (!accounting) return null
  const variants = modelRuns.filter((run) => run.nativeVariant !== undefined)
  return (
    <section className="shrink-0" aria-label="Native accounting">
      <h3 className="type-callout text-label">Native accounting</h3>
      <dl className="mt-3 flex flex-col gap-2 type-callout">
        {nativeAccountingRows(accounting, tokenReconciliation, costReconciliation).map(
          ([label, value]) => (
            <div key={label}>
              <dt className="text-label-secondary">{label}</dt>
              <dd className="text-label">{value}</dd>
            </div>
          ),
        )}
        {variants.length > 0 && (
          <div>
            <dt className="text-label-secondary">Native model variants</dt>
            <dd className="break-words text-label">
              {variants.map((run) => `${run.model}#${run.nativeVariant}`).join(" · ")}
            </dd>
          </div>
        )}
      </dl>
      <p className="mt-3 type-callout text-label-secondary">
        Recorded costs are calculated estimates, not billed amounts. Zero does not prove free
        work. Antiburn does not assign unallocated usage to a step.
      </p>
    </section>
  )
}
