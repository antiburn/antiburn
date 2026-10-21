import { renderAgentIcon } from "../../lib/agentIcon"
import { CLAUDE_PLUGIN_TAGLINE, CLAUDE_PLUGIN_TITLE } from "../../lib/claudePluginOffer"
import { useClaudePlugin } from "../../lib/claudePluginStore"
import {
  ClaudePluginInstallDetail,
  PluginPageButton,
  claudePluginInstalledLine,
} from "../../components/claudePlugin/ClaudePluginOffer"
import { mainClaudePluginStore } from "./mainClaudePluginStore"

/**
 * The plugin's permanent home on the Agents page. It shows whenever Claude
 * Code is here: the install detail until the plugin is found, then its
 * version. It ignores the sidebar card's dismissal because it is status, not
 * a nudge.
 */
export function ClaudePluginAgentsPanel({ active }: { active: boolean }) {
  const { snapshot, offer } = useClaudePlugin(mainClaudePluginStore, "agents", active)
  if (offer === "hidden") return null
  const installed = offer === "installed" && snapshot.status ? snapshot.status : null
  return (
    <section
      aria-labelledby="claude-plugin-agents-title"
      className="flex flex-col gap-3 rounded-control bg-surface-card/75 p-4"
    >
      <header className="flex items-start gap-3">
        {renderAgentIcon("claude-code", 28)}
        <div className="min-w-0 flex-1">
          <h2 id="claude-plugin-agents-title" className="type-headline text-label">
            {CLAUDE_PLUGIN_TITLE}
          </h2>
          <p className="type-callout text-pretty text-label-secondary">
            {installed ? claudePluginInstalledLine(installed) : CLAUDE_PLUGIN_TAGLINE}
          </p>
        </div>
        {installed && <PluginPageButton placement="agents" />}
      </header>
      {!installed && <ClaudePluginInstallDetail placement="agents" />}
    </section>
  )
}
