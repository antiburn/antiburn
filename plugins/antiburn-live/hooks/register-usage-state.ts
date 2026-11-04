import {
  pace,
  percentage,
  primaryWindow,
  resetText,
  PRIMARY_WINDOWS,
  type UsageSnapshot,
} from "./usage";
import { meterDescription } from "./usage-graphics";

export function sameUsagePresentation(
  previous: UsageSnapshot | undefined,
  snapshot: UsageSnapshot,
): boolean {
  return (
    previous?.status === snapshot.status &&
    JSON.stringify(previous.windows) === JSON.stringify(snapshot.windows) &&
    PRIMARY_WINDOWS.every((kind) => {
      const window = primaryWindow(snapshot, kind);
      return (
        resetText(window, previous.capturedAt) ===
          resetText(window, snapshot.capturedAt) &&
        pace(window, previous.capturedAt) === pace(window, snapshot.capturedAt)
      );
    })
  );
}

export function usagePercentagesChanged(
  previous: UsageSnapshot | undefined,
  snapshot: UsageSnapshot,
): previous is UsageSnapshot {
  return (
    previous !== undefined &&
    snapshot.status === "ready" &&
    previous.status === "ready" &&
    PRIMARY_WINDOWS.some(
      (kind) =>
        percentage(primaryWindow(previous, kind)) !==
        percentage(primaryWindow(snapshot, kind)),
    )
  );
}

export function usageReplyText(snapshot: UsageSnapshot, now: number): string {
  if (!snapshot.windows.some((window) => window.percent !== null))
    return snapshot.status === "error"
      ? "Usage is unavailable."
      : "Usage has not been reported yet.";
  return PRIMARY_WINDOWS.map((kind) =>
    meterDescription(primaryWindow(snapshot, kind), now),
  ).join("\n");
}
