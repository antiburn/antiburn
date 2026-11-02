import type { EngineInterface, RenderInput, RenderNode } from "claude-code";
import type { LiveBurnState, LiveUsageState } from "../types";
import type { LiveMode } from "./mode";
import { BURN_ANIMATION_ENABLED, BURN_POLICY } from "./burn";
import { CARD_BACKGROUND } from "./register-visuals";
import { USAGE_FIXTURES, type usageFixture } from "./usage-fixtures";

type DebugUi = ReturnType<EngineInterface["ui"]["resolve"]>;
type DebugEvent = RenderInput<"Pane">;
type DebugFixture = NonNullable<ReturnType<typeof usageFixture>>;
type DebugActions = {
  setDebugMode: (mode: LiveMode) => Promise<void>;
  openPane: () => Promise<void>;
  showCheckDemo: () => Promise<void>;
  startTour: () => Promise<void>;
  endDemo: () => Promise<void>;
  previewBurn: (reduced?: boolean) => Promise<void>;
  stopBurnPreview: () => Promise<void>;
  refreshBurnStats: () => Promise<void>;
  selectUsageLayout: (layout: "inline" | "current") => Promise<void>;
  selectUsagePreview: (id: string | null) => Promise<void>;
  selectUsageState: (
    state: "idle" | "checking" | "retry" | "success" | "stale",
  ) => Promise<void>;
  replayUsageEntrance: () => Promise<void>;
  startUsageSequence: () => Promise<void>;
  stopUsageSequence: () => Promise<void>;
  toggleSyntheticLabels: () => Promise<void>;
  inspectUsageAnimation: (action: "show" | "hide") => Promise<void>;
};

export function renderUsageDebugView(
  ui: DebugUi,
  e: DebugEvent,
  state: LiveUsageState,
  preview: LiveUsageState["preview"],
  fixture: DebugFixture | undefined,
  burnState: LiveBurnState,
  checksEnabled: boolean,
  usageDetails: RenderNode,
  actions: DebugActions,
) {
  const { Box, Button, Text } = ui;
  const cardProps = {
    flexDirection: "column",
    gap: 1,
    padding: 1,
    borderStyle: "round",
    backgroundColor: e.surface === "desktop" ? CARD_BACKGROUND : undefined,
    borderColor: e.surface === "desktop" ? CARD_BACKGROUND : "subtle",
  } as const;
  return (
    <Box flexDirection="column" gap={1}>
      <Text bold>Developer debug</Text>
      <Text>
        {checksEnabled
          ? "Mode 1 · Usage limits + live checks"
          : "Mode 0 · Usage limits"}
      </Text>
      <Box key="debug-checks-card" {...cardProps}>
        <Text bold>Checks</Text>
        <Text dimColor>
          Mode changes apply to this session. Reload restores
          ANTIBURN_LIVE_MODE. Checks start fresh after each change.
        </Text>
        <Button
          key="usage-debug-mode"
          label={
            checksEnabled
              ? "Switch to Mode 0 · Usage limits"
              : "Switch to Mode 1 · Usage limits + live checks"
          }
          onPress={() =>
            actions.setDebugMode(checksEnabled ? "usage" : "checks")
          }
        />
        {checksEnabled && (
          <Button
            key="debug-checks"
            label="View live checks"
            onPress={() => actions.openPane()}
          />
        )}
        {checksEnabled && (
          <Button
            key="debug-checks-demo"
            label="Show sample checks"
            onPress={() => actions.showCheckDemo()}
          />
        )}
        {checksEnabled && (
          <Button
            key="debug-checks-tour"
            label="Start checks tour"
            onPress={() => actions.startTour()}
          />
        )}
        {checksEnabled && (
          <Button
            key="debug-checks-live"
            label="Return to live checks"
            onPress={() => actions.endDemo()}
          />
        )}
      </Box>
      <Box key="debug-burn-card" {...cardProps}>
        <Text bold>Rooted flame · output activity</Text>
        {BURN_ANIMATION_ENABLED ? (
          <Box flexDirection="column" gap={1}>
            <Text dimColor>
              Recent streamed text, thinking and tool arguments estimate output
              activity. Completed responses provide measured request averages.
              Subscription quota does not trigger this effect.
            </Text>
            <Text>{`Signal: ${burnState.source} · estimate ${Math.round(burnState.estimatedTps)} tokens/s · completed response ${burnState.measuredTps === null ? "unavailable" : Math.round(burnState.measuredTps) + " tokens/s"}`}</Text>
            <Text
              dimColor
            >{`Experimental heuristic: ${BURN_POLICY.entryTps} estimated tokens/s. Native terminal shows shaded blocks and a heat ripple behind the wordmark. Preview stops after 15 seconds.`}</Text>
            <Button
              key="burn-preview"
              label="Preview rooted flame · 15s"
              onPress={() => actions.previewBurn()}
            />
            <Button
              key="burn-preview-reduced"
              label="Preview reduced motion · 15s"
              onPress={() => actions.previewBurn(true)}
            />
            <Button
              key="burn-preview-stop"
              label="Return to measured activity"
              onPress={() => actions.stopBurnPreview()}
            />
            <Button
              key="burn-stats"
              label="Refresh activity measurements"
              onPress={() => actions.refreshBurnStats()}
            />
          </Box>
        ) : (
          <Text key="burn-unavailable" dimColor>
            Burn previews and activity tracking are unavailable.
          </Text>
        )}
      </Box>
      <Box key="debug-usage-card" {...cardProps}>
        <Text bold>Usage</Text>
        <Text>
          {state.layout === "current"
            ? "Layout: Current · captions above meters"
            : "Layout: Label reveal · inline (default)"}
        </Text>
        <Button
          key="usage-layout-inline"
          label="inline (default)"
          onPress={() => actions.selectUsageLayout("inline")}
        />
        <Button
          key="usage-layout-current"
          label="labels above meters"
          onPress={() => actions.selectUsageLayout("current")}
        />
        {usageDetails}
        <Text dimColor>
          {fixture
            ? "Synthetic data · native readings continue in the background"
            : "Live usage data"}
        </Text>
        {fixture && <Text>{`Preview: ${preview!.id}`}</Text>}
        {fixture && (
          <Button
            key="usage-preview-live"
            label="Return to live data"
            onPress={() => actions.selectUsagePreview(null)}
          />
        )}
        <Text bold>Usage examples</Text>
        <Text dimColor>Preview account usage states</Text>
        <Box flexDirection="row" gap={1}>
          {(["idle", "checking", "retry", "success", "stale"] as const).map(
            (status) => (
              <Button
                key={`usage-state-${status}`}
                label={status[0]!.toUpperCase() + status.slice(1)}
                onPress={() => actions.selectUsageState(status)}
              />
            ),
          )}
        </Box>
        <Button
          key="usage-preview-entrance"
          label="Replay entrance"
          onPress={() => actions.replayUsageEntrance()}
        />
        <Text dimColor>
          Replays the current reading; uses Low usage when no usable reading
          exists.
        </Text>
        <Box flexDirection="row" gap={1}>
          <Button
            key="usage-sequence-start"
            label="Play all usage examples"
            onPress={() => actions.startUsageSequence()}
          />
          <Button
            key="usage-synthetic-labels"
            plain
            label={`${state.showSyntheticLabels ? "☑" : "☐"} Show synthetic labels`}
            onPress={actions.toggleSyntheticLabels}
          />
        </Box>
        {state.sequence && <Text>{`Playing: ${state.sequence}`}</Text>}
        {state.sequence && (
          <Button
            key="usage-sequence-stop"
            label="Stop sequence"
            onPress={() => actions.stopUsageSequence()}
          />
        )}
        <Text dimColor>
          {BURN_ANIMATION_ENABLED
            ? "Runs once: entrance, all examples, reset and pace reveal/hide, then Ember catch at high usage. Near limit holds for six pulse cycles."
            : "Runs once: entrance, all examples, and reset and pace reveal/hide. Near limit holds for six pulse cycles. Burn previews are unavailable."}
        </Text>
        <Text dimColor>
          Warning styles are previews only. Pulse runs six cycles; reduced
          motion stays static.
        </Text>
        {USAGE_FIXTURES.map((item) => (
          <Button
            key={`usage-preview-${item.id}`}
            label={item.label}
            onPress={() => actions.selectUsagePreview(item.id)}
          />
        ))}
        {fixture && (
          <Box flexDirection="column">
            <Text bold>Animation inspector</Text>
            <Text dimColor>
              Desktop uses SVG motion; terminal red dots pulse in place. Hover a
              label to test delayed reveal. Keyboard/tap reveal stays immediate.
            </Text>
            <Text>
              Mark: 0–240ms · meters: 100–460ms / 240–600ms · fill: 600–960ms
            </Text>
            <Text>
              Example fill: 360ms · cubic-bezier(.23,1,.32,1) · retargets from
              the current frame
            </Text>
            <Text>
              Digits: 260ms + 12ms per column, capped at 24ms · choose 9 → 10 or
              99 → 100
            </Text>
            <Text>
              Reset: 300ms hover dwell · 280ms reveal / 180ms hide ·
              cubic-bezier(.32,.72,0,1)
            </Text>
            <Text>
              Pace: 300ms hover dwell · 125ms contrast ·
              cubic-bezier(.23,1,.32,1)
            </Text>
            <Text>
              Warning: 1600ms × 6 cycles · opacity 65–100% · subtle glow ·
              reduced motion static
            </Text>
            <Text>
              Reduced motion: 80ms entrance/reveal opacity · no rise, fill sweep
              or digit roll
            </Text>
            <Button
              key="usage-preview-reveal"
              label="Replay reset reveal"
              onPress={() => actions.inspectUsageAnimation("show")}
            />
            <Button
              key="usage-preview-hide"
              label="Replay reset hide"
              onPress={() => actions.inspectUsageAnimation("hide")}
            />
          </Box>
        )}
      </Box>
    </Box>
  );
}
