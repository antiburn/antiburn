import { renderAgentIcon } from "../../src/lib/agentIcon"
import { AGENT_SLUGS, agentDisplayName } from "../../src/lib/presentation/agents"

export function VendorIconGallery() {
  return (
    <main
      className="h-full overflow-auto bg-surface-window p-6 text-label"
      aria-label="Vendor icon gallery"
    >
      <h1 className="type-title-1 mb-4">Vendor artwork and neutral marks</h1>
      <table className="w-full type-body">
        <thead>
          <tr>
            <th className="text-left">Agent</th>
            {["default", "neutral"].flatMap((appearance) =>
              [16, 28, 40].map((size) => (
                <th key={`${appearance}-${size}`}>
                  {appearance} {size}
                </th>
              )),
            )}
          </tr>
        </thead>
        <tbody>
          {AGENT_SLUGS.map((slug) => (
            <tr key={slug} data-vendor={slug}>
              <th className="py-2 text-left">{agentDisplayName(slug)}</th>
              {(["default", "neutral"] as const).flatMap((appearance) =>
                [16, 28, 40].map((size) => (
                  <td
                    className="py-2 text-center"
                    key={`${appearance}-${size}`}
                    data-appearance={appearance}
                    data-size={size}
                  >
                    {renderAgentIcon(slug, size, undefined, appearance)}
                  </td>
                )),
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </main>
  )
}
