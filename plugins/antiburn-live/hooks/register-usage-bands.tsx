import type {
  EngineInterface,
  RenderElement,
  RenderInput,
  RenderNode,
} from "claude-code";
import type { UsageWindow } from "./usage";
import {
  meterDescription,
  meterLayer,
  secondaryTextSvg,
} from "./usage-graphics";
import { PRESENTATION, type usageLayout } from "./usage-presentation";
import { renderUsageMeterBand } from "./usage-meter-band";
import { RESPONSIVE_FRAME_WIDTH } from "./usage-graphics";

type UsageUi = ReturnType<EngineInterface["ui"]["resolve"]>;
type AbovePromptEvent = RenderInput<"AbovePrompt">;

type EmptyBandInput = {
  ui: UsageUi;
  event: AbovePromptEvent;
  isDesktop: boolean;
  status: string;
  hint?: string;
  retryHint?: string;
  canRefresh: boolean;
  statusGraphic: {
    source: string;
    description: string;
    width: number;
    height: number;
  };
  renderMark: () => RenderNode | null;
  renderTerminalMark: () => RenderNode;
  onRefresh: () => void;
};

export function renderEmptyUsageBand({
  ui,
  event,
  isDesktop,
  status,
  hint,
  retryHint,
  canRefresh,
  statusGraphic,
  renderMark,
  renderTerminalMark,
  onRefresh,
}: EmptyBandInput): RenderElement {
  const { Box, Button, Text } = ui;
  const band = (
    <Box
      key="usage-band"
      position="relative"
      flexDirection="row"
      alignItems={isDesktop ? "center" : "flex-end"}
      gap={isDesktop ? 0 : 1}
      height={
        isDesktop && event.props.bodyColumns >= 24
          ? statusGraphic.height / 16
          : undefined
      }
    >
      {isDesktop && <Box width={PRESENTATION.brandInset / 8} flexShrink={0} />}
      {isDesktop ? renderMark() : renderTerminalMark()}
      {isDesktop && <Box width={PRESENTATION.brandGap / 8} flexShrink={0} />}
      {isDesktop && "Svg" in ui ? (
        <ui.Svg
          key="usage-status"
          source={statusGraphic.source}
          alt={statusGraphic.description}
          width={Math.min(
            statusGraphic.width,
            Math.max(
              1,
              event.props.bodyColumns * 8 -
                PRESENTATION.brandInset -
                14 -
                PRESENTATION.brandGap -
                (canRefresh ? 32 : 0),
            ),
          )}
          height={statusGraphic.height}
        />
      ) : (
        <Box
          width={Math.max(1, event.props.bodyColumns - (canRefresh ? 14 : 12))}
          flexDirection="row"
          gap={1}
        >
          {event.surface === "terminal" &&
          "Client" in ui &&
          status === "Checking usage..." ? (
            <ui.Client
              key="usage-checking"
              module="./checking-status.ts"
              props={null}
            />
          ) : (
            <Text key="usage-status">{status}</Text>
          )}
          {hint && <Text dimColor>{hint}</Text>}
        </Box>
      )}
      {canRefresh && (
        <Button
          key="usage-state-refresh"
          plain
          dimColor
          label={isDesktop ? "↻" : "[↻]"}
          onPress={onRefresh}
        />
      )}
    </Box>
  );
  if (!retryHint) return band;
  const rows = Math.max(2, Math.floor(event.props.maxRows - 2));
  const secondary = secondaryTextSvg(
    [retryHint],
    event.props.bodyColumns * 8,
    rows,
  );
  return (
    <Box flexDirection="column" width={event.props.bodyColumns}>
      {band}
      {isDesktop && "Svg" in ui ? (
        <ui.Svg
          source={secondary.source}
          alt={secondary.description}
          width={secondary.width}
          height={secondary.height}
        />
      ) : (
        <Box
          key="usage-retry-copy"
          width={event.props.bodyColumns}
          overflow="hidden"
        >
          <Text dimColor wrap="wrap">
            {retryHint}
          </Text>
        </Box>
      )}
    </Box>
  );
}

type DesktopBandInput = {
  ui: Extract<UsageUi, { Svg: unknown }>;
  windows: readonly UsageWindow[];
  layout: ReturnType<typeof usageLayout>;
  responsiveSources: readonly string[];
  now: number;
  status: "ready" | "error" | "empty";
  usageMark: string;
  markAlt: string;
  renderMark: (source: string, alt: string) => RenderNode | null;
};

export function renderDesktopUsageBand({
  ui,
  windows,
  layout,
  responsiveSources,
  now,
  status,
  usageMark,
  markAlt,
  renderMark,
}: DesktopBandInput): RenderElement {
  const { Box } = ui;
  return renderUsageMeterBand({
    ui,
    layout,
    windows,
    mark: renderMark(usageMark, markAlt),
    renderMeter: (window, index) => (
      <Box position="relative" width="100%" minWidth={0} alignItems="center">
        <ui.Svg
          key={`meter-base-${window.kind}`}
          source={meterLayer(responsiveSources[index]!, "base")}
          alt={`${meterDescription(window, now, true)}${status === "error" ? ". Usage unavailable · Last known" : ""}`}
          width={RESPONSIVE_FRAME_WIDTH}
          height={layout.meterHeight}
        />
        <Box
          position="absolute"
          top={0}
          bottom={0}
          left={0}
          right={0}
          alignItems="center"
        >
          <ui.Svg
            key={`meter-interaction-${window.kind}`}
            source={meterLayer(responsiveSources[index]!, "interaction")}
            alt={`${window.label} reset and pace controls`}
            width={RESPONSIVE_FRAME_WIDTH}
            height={layout.meterHeight}
            isInteractive
          />
        </Box>
      </Box>
    ),
  });
}

type CompactBandInput = {
  ui: UsageUi;
  isDesktop: boolean;
  isInline: boolean;
  snapshotStatus: "ready" | "error" | "empty";
  renderMark: () => RenderNode | null;
  windows: readonly UsageWindow[];
  renderMeter: (window: UsageWindow) => RenderNode;
};

export function renderCompactUsageBand({
  ui,
  isDesktop,
  isInline,
  snapshotStatus,
  windows,
  renderMark,
  renderMeter,
}: CompactBandInput): RenderElement {
  const { Box, Text } = ui;
  const mark = renderMark();
  const meters = (
    <Box
      key="usage-meters"
      flexDirection={isInline ? "row" : "column"}
      gap={isInline ? 3 : 0}
    >
      {windows.map(renderMeter)}
    </Box>
  );
  const brandGap = isDesktop && (
    <Box width={PRESENTATION.brandGap / 8} flexShrink={0} />
  );
  if (snapshotStatus === "error") {
    return (
      <Box key="usage-band" flexDirection="column">
        <Box
          flexDirection="row"
          alignItems={isDesktop ? "center" : "flex-end"}
          gap={isDesktop ? 0 : 1}
        >
          {isDesktop && (
            <Box width={PRESENTATION.brandInset / 8} flexShrink={0} />
          )}
          {mark}
          {brandGap}
          <Text key="usage-status">Usage unavailable · Last known</Text>
        </Box>
        <Box marginLeft={isDesktop ? 4 : 3}>{meters}</Box>
      </Box>
    );
  }
  return (
    <Box
      key="usage-band"
      position="relative"
      flexDirection="row"
      alignItems={isDesktop ? "center" : "flex-end"}
      gap={isDesktop ? 0 : 1}
    >
      {isDesktop && <Box width={PRESENTATION.brandInset / 8} flexShrink={0} />}
      {mark}
      {brandGap}
      {meters}
    </Box>
  );
}

type UsageCompositionInput = {
  ui: UsageUi;
  event: AbovePromptEvent;
  checkStrip: RenderNode | null;
  syntheticLabel?: string;
  usage: RenderElement;
};

export function renderUsageComposition({
  ui,
  event,
  checkStrip,
  syntheticLabel,
  usage,
}: UsageCompositionInput): RenderElement {
  const { Box, Text } = ui;
  const separatorWidth = Math.max(1, event.props.bodyColumns * 8 - 16);
  return (
    <Box
      key="usage-spacing"
      flexDirection="column"
      marginTop={
        event.props.maxRows >= 8 && event.props.bodyColumns >= 24
          ? event.surface === "desktop"
            ? 0.25
            : 1
          : 0
      }
      gap={checkStrip && event.surface === "desktop" ? 0.5 : 1}
    >
      {checkStrip}
      {checkStrip && (
        <Box key="checks-usage-separator" marginX={1}>
          {event.surface === "desktop" && "Svg" in ui ? (
            <ui.Svg
              source={`<svg xmlns="http://www.w3.org/2000/svg" width="${separatorWidth}" height="16" viewBox="0 0 ${separatorWidth} 16"><style>path{stroke:#000;stroke-opacity:.06}@media(prefers-color-scheme:dark){path{stroke:#fff;stroke-opacity:.08}}</style><path d="M0 8H${separatorWidth}" stroke-width="1"/></svg>`}
              width={separatorWidth}
              height={16}
              alt="Checks and usage separator"
            />
          ) : (
            <Text dimColor>
              {"─".repeat(Math.max(1, event.props.bodyColumns - 4))}
            </Text>
          )}
        </Box>
      )}
      {syntheticLabel && <Text dimColor>{syntheticLabel}</Text>}
      {usage}
    </Box>
  );
}

type SessionModeInput = {
  ui: UsageUi;
  event: RenderInput<"SessionMode">;
  label: string;
  mark?: { color: string; glyph: string };
  openPane: () => Promise<void>;
};

export function renderSessionModeView({
  ui,
  event,
  label,
  mark: chipMark,
  openPane,
}: SessionModeInput): RenderElement {
  const { Box, Button, Text } = ui;
  const mark = chipMark !== undefined && (
    <Text key="chip-mark" color={chipMark.color}>
      {chipMark.glyph}
    </Text>
  );
  const isTerminal = event.surface === "terminal";
  return (
    <Box flexDirection="row" gap={1} alignItems="center">
      {event.props.modes.length > 0 && (
        <Text dimColor>{event.props.modes.join(" & ")}</Text>
      )}
      {!isTerminal && mark}
      <Button key="chip" plain dimColor label={label} onPress={openPane} />
      {isTerminal && mark}
    </Box>
  );
}
