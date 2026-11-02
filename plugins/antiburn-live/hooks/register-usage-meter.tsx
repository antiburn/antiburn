import type { EngineInterface, RenderNode } from 'claude-code';
import type { UsageWindow } from './usage';
import { filledDotCount, paceLabel, percentage, resetText } from './usage';
import { meterDescription } from './usage-graphics';
import { desktopPercentage } from './usage-presentation';
import type { PreviewWarning } from './usage-fixtures';

const BRAND = '#FF5914';

type Geometry = { count: number; value: number | null; markerCell: number | null };
type MeterUi = ReturnType<EngineInterface['ui']['resolve']>;

type MeterInput = {
  ui: MeterUi;
  window: UsageWindow;
  now: number;
  isDesktop: boolean;
  revealed: string | null;
  geometry: (window: UsageWindow) => Geometry;
  fixtureWarning?: PreviewWarning;
  pulse: boolean;
  meterColumns: number;
  compact: boolean;
  hoverBinding: string;
  reveal: (kind: string) => Promise<void>;
};

export function renderUsageMeter({
  ui,
  window,
  now,
  isDesktop,
  revealed,
  geometry,
  fixtureWarning,
  pulse,
  meterColumns,
  compact,
  hoverBinding,
  reveal,
}: MeterInput): RenderNode {
  const { Box, Button, Text } = ui;

  const description = meterDescription(window, now, isDesktop);
  const isRevealed = revealed === window.kind;
  const isPaceRevealed = revealed === `pace-${window.kind}`;
  const { count, value, markerCell } = geometry(window);
  const isPaceReached =
    window.percent !== null && value !== null && window.percent / 100 >= value;
  const cells = count * 2 - 1;
  const copy = isRevealed
    ? resetText(window, now)
    : isPaceRevealed
      ? value === null
        ? 'Pace unavailable'
        : paceLabel(value)
      : null;
  const copyOnRight = markerCell !== null && cells - markerCell - 1 >= markerCell;
  const copyOnLeft = isRevealed || (isPaceRevealed && !copyOnRight);
  const segment = (start: number, end: number, showCopy: boolean) => {
    const copyLength =
      showCopy && copy !== null ? Math.min(copy.length, Math.max(0, end - start)) : 0;
    const copyStart = copyOnLeft ? end - copyLength : start;
    const dotCells = Array.from({ length: Math.max(0, end - start) }, (_, index) => {
      const cell = start + index;
      if (copyLength > 0 && cell >= copyStart && cell < copyStart + copyLength) return null;
      if (cell % 2 !== 0) return <Text key={`space-${cell}`}> </Text>;
      const filled = cell / 2 < filledDotCount(window.percent, count);
      const red =
        filled &&
        (fixtureWarning === 'pulse' ||
          (fixtureWarning === 'red' &&
            isPaceReached &&
            value !== null &&
            cell / (count * 2 - 2) > value));
      return (
        <Text key={`dot-${cell}`} color={red ? '#ed4545' : filled ? BRAND : 'subtle'}>
          {red && pulse ? '•' : '●'}
        </Text>
      );
    });
    if (copyLength === 0) return dotCells;
    dotCells.splice(
      copyStart - start,
      copyLength,
      <Text key="usage-segment-copy" dimColor wrap="truncate-end">
        {copy}
      </Text>,
    );
    return dotCells;
  };

  const target = (
    detail: 'reset' | 'pace',
    text: string,
    color?: string,
    dimColor?: boolean,
  ) =>
    !isDesktop && 'Client' in ui ? (
      <ui.Client
        key={`usage-hover-${detail}-${window.kind}`}
        module="./usage-target.tsx"
        width={detail === 'reset' ? window.label.length : 1}
        height={1}
        props={{
          binding: hoverBinding,
          width: detail === 'reset' ? window.label.length : 1,
          text,
          ...(color === undefined ? {} : { color }),
          ...(dimColor === undefined ? {} : { dimColor }),
        }}
      />
    ) : null;
  const markerGlyph = isPaceReached ? '│' : '┊';
  const markerHover = target(
    'pace',
    markerGlyph,
    isPaceReached ? BRAND : 'subtle',
    !isPaceReached && !isPaceRevealed,
  );
  const paceMarker =
    markerCell !== null ? (
      <Box position="relative" width={1} height={1} flexShrink={0}>
        <Button
          key={`usage-pace-${window.kind}`}
          plain
          onPress={() => reveal(`pace-${window.kind}`)}
        >
          <Text
            key="pace-marker"
            color={isPaceReached ? BRAND : 'subtle'}
            dimColor={!isPaceReached && !isPaceRevealed}
          >
            {isPaceReached ? '│' : '┊'}
          </Text>
        </Button>
        {markerHover && (
          <Box position="absolute" left={0} top={0} width={1} height={1}>
            {markerHover}
          </Box>
        )}
      </Box>
    ) : null;
  const dots =
    count >= 2 ? (
      isRevealed && markerCell !== null ? (
        <Box
          key="usage-reset-track"
          position="relative"
          width={cells}
          height={1}
          overflow="hidden"
        >
          <Text>{segment(0, cells, false)}</Text>
          <Box
            key="usage-pace-marker-slot"
            position="absolute"
            left={markerCell}
            top={0}
            width={1}
            height={1}
          >
            <Button
              key={`usage-pace-${window.kind}`}
              plain
              onPress={() => reveal(`pace-${window.kind}`)}
            >
              <Text color={isPaceReached ? BRAND : 'subtle'}>{markerGlyph}</Text>
            </Button>
          </Box>
          <Box
            key="usage-reset-copy-track"
            position="absolute"
            left={0}
            top={0}
            width={cells}
            height={1}
            overflow="hidden"
          >
            <Text key="usage-reset-copy" dimColor wrap="truncate-end">
              {copy}
            </Text>
          </Box>
          {markerHover && (
            <Box position="absolute" left={markerCell} top={0} width={1} height={1}>
              {target(
                'pace',
                copy !== null && markerCell < copy.length ? copy[markerCell]! : markerGlyph,
                copy !== null && markerCell < copy.length
                  ? undefined
                  : isPaceReached
                    ? BRAND
                    : 'subtle',
                copy !== null && markerCell < copy.length ? true : !isPaceReached,
              )}
            </Box>
          )}
        </Box>
      ) : (
        <Box flexDirection="row" width={cells} height={1} flexShrink={0}>
          {markerCell === null && <Text>{segment(0, cells, copy !== null)}</Text>}
          {markerCell !== null && (
            <Box
              key={`usage-pace-left-${window.kind}`}
              width={markerCell}
              flexShrink={0}
              justifyContent={isPaceRevealed && copyOnLeft ? 'flex-end' : undefined}
              height={1}
              flexDirection="row"
              overflow="hidden"
            >
              <Text wrap="truncate-end">
                {segment(0, markerCell, copy !== null && copyOnLeft)}
              </Text>
            </Box>
          )}
          {paceMarker}
          {markerCell !== null && (
            <Box
              key={`usage-pace-right-${window.kind}`}
              width={cells - markerCell - 1}
              flexShrink={0}
              height={1}
              flexDirection="row"
              overflow="hidden"
            >
              <Text wrap="truncate-end">
                {segment(markerCell + 1, cells, copy !== null && isPaceRevealed && copyOnRight)}
              </Text>
            </Box>
          )}
        </Box>
      )
    ) : null;
  return (
    <Box
      key={`usage-${window.kind}`}
      position="relative"
      flexDirection="column"
      width={meterColumns}
      height={compact && !isDesktop ? 1 : undefined}
    >
      <Box flexDirection="row" justifyContent="space-between" gap={1}>
        <Box
          position="relative"
          width={window.label.length}
          height={1}
          flexShrink={0}
          overflow="hidden"
        >
          <Button
            key={`usage-detail-${window.kind}`}
            plain
            label={isDesktop ? window.label : description}
            onPress={() => reveal(window.kind)}
          >
            <Text>{window.label}</Text>
          </Button>
          {!isDesktop && (
            <Box position="absolute" left={0} top={0} width={window.label.length} height={1}>
              {target('reset', window.label)}
            </Box>
          )}
        </Box>
        <Box flexGrow={1} flexShrink={1} minWidth={0}>
          {compact ? (
            dots
          ) : (
            <Text dimColor wrap="truncate-end">
              {isRevealed ? resetText(window, now) : ''}
            </Text>
          )}
        </Box>
        <Text>{isDesktop ? desktopPercentage(window) : percentage(window)}</Text>
      </Box>
      {!compact && dots}
    </Box>
  );
}
