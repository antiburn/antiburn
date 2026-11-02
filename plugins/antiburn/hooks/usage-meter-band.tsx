import type { EngineInterface, RenderNode } from "claude-code";
import { PRESENTATION, type usageLayout } from "./usage-presentation";
import type { UsageWindow } from "./usage";

type UsageUi = ReturnType<EngineInterface["ui"]["resolve"]>;

export function renderUsageMeterBand(input: {
  ui: UsageUi;
  layout: ReturnType<typeof usageLayout>;
  windows: readonly UsageWindow[];
  mark: RenderNode | null;
  renderMeter: (window: UsageWindow, index: number) => RenderNode;
}) {
  const { Box } = input.ui;
  const { inline, stackedGap } = input.layout;
  return (
    <Box
      key="usage-band"
      position="relative"
      width="100%"
      flexDirection="row"
      alignItems="center"
    >
      <Box width={PRESENTATION.brandInset / 8} flexShrink={0} />
      {input.mark}
      <Box width={PRESENTATION.brandGap / 8} flexShrink={0} />
      <Box
        key="usage-meters"
        flexDirection={inline ? "row" : "column"}
        flexGrow={1}
        flexShrink={1}
        minWidth={0}
        width={0}
        gap={inline ? PRESENTATION.inlineGap / 8 : stackedGap / 16}
      >
        {input.windows.map((window, index) => (
          <Box
            key={`meter-${window.kind}`}
            position="relative"
            width={inline ? 0 : "100%"}
            minWidth={0}
            flexGrow={1}
            flexShrink={1}
            alignItems="center"
          >
            {input.renderMeter(window, index)}
          </Box>
        ))}
      </Box>
      <Box width={PRESENTATION.brandInset / 8} flexShrink={0} />
    </Box>
  );
}
