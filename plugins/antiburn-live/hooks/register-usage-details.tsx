import type { EngineInterface, RenderNode } from 'claude-code';
import type { UsageSnapshot } from './usage';
import {
  pace,
  paceLabel,
  percentage,
  primaryWindow,
  resetText,
  PRIMARY_WINDOWS,
} from './usage';

type DetailsUi = ReturnType<EngineInterface['ui']['resolve']>;

export function renderUsageDetailsView(
  ui: DetailsUi,
  snapshot: UsageSnapshot,
  now: number,
  fixture: boolean,
  refresh: () => Promise<void>,
): RenderNode {
  const { Box, Button, Text } = ui;
  const primary = PRIMARY_WINDOWS.map((kind) => primaryWindow(snapshot, kind));
  const extra = snapshot.windows.filter(
    (window) => !PRIMARY_WINDOWS.some((kind) => kind === window.kind),
  );
  return (
    <Box flexDirection="column" gap={1}>
      <Text dimColor>
        {fixture
          ? 'Synthetic data · native readings continue in the background'
          : 'Last reported by Claude Code'}
      </Text>
      {snapshot.status !== 'ready' && (
        <Text>
          {snapshot.status === 'empty' ? 'Usage not available yet' : 'Usage unavailable'}
        </Text>
      )}
      {snapshot.status === 'empty' && !fixture && (
        <Text dimColor>
          Claude has not reported usage for this session yet. Continue with your next message
          normally; the meters update when Claude reports limits. If they stay unavailable, try
          Refresh or check Usage in Claude. Refresh reads existing data and does not send a
          message.
        </Text>
      )}
      {snapshot.status === 'error' && snapshot.windows.length > 0 && (
        <Text dimColor>Last known</Text>
      )}
      {[...primary, ...extra].map((window) => (
        <Box key={window.kind} flexDirection="column">
          <Text>{`${window.label}: ${percentage(window)}`}</Text>
          <Text dimColor>{resetText(window, now)}</Text>
          {window.resetsAt !== null && <Text dimColor>{`Reset: ${window.resetsAt}`}</Text>}
          {pace(window, now) !== null && <Text dimColor>{paceLabel(pace(window, now)!)}</Text>}
        </Box>
      ))}
      <Button key="usage-refresh" label="Refresh" onPress={refresh} />
    </Box>
  );
}
