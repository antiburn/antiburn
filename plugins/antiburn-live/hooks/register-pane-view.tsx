import type {
  EngineInterface,
  RenderElement,
  RenderInput,
  RenderNode,
} from "claude-code";
import type { LiveCheckId, LiveCheckView, LiveStage } from "../types";
import {
  GROUPS,
  markSize,
  renderListView,
  renderPaneView,
} from "./register-visuals";
import { TOUR } from "./tour";

type PaneEvent = RenderInput<"Pane">;
type PaneUi = ReturnType<EngineInterface["ui"]["resolve"]>;

export function needsPublish(
  hasChanged: boolean,
  hasResolvedLine: boolean,
): boolean {
  return hasChanged || hasResolvedLine;
}

export function enteredStages(
  before: readonly LiveCheckView[],
  after: readonly LiveCheckView[],
): LiveStage[] {
  const isNew = (check: LiveCheckView) =>
    !before.some((old) => old.id === check.id && old.stage === check.stage);
  return [
    ...new Set(
      after
        .filter((check) => !check.isHidden && isNew(check))
        .map((check) => check.stage),
    ),
  ];
}

type PaneActions = {
  centredLine: (event: PaneEvent, key: string, line: string) => RenderNode;
  toggleGroup: (title: string) => Promise<void>;
  toggleCard: (id: LiveCheckId) => Promise<void>;
  toggleRow: (id: LiveCheckId) => Promise<void>;
  act: (id: LiveCheckId, from: "card") => Promise<void>;
  hide: (id: LiveCheckId) => Promise<void>;
  toggleFullPrompt: (id: LiveCheckId) => Promise<void>;
  nextTourStep: () => Promise<string | null>;
  endTour: () => Promise<void>;
};

type PaneViewInput = {
  ui: PaneUi;
  event: PaneEvent;
  checks: readonly LiveCheckView[];
  entered: readonly LiveStage[];
  selectedId: LiveCheckId | null;
  openGroups: readonly string[];
  turned: string | null;
  fullPromptId: LiveCheckId | null;
  armedId: LiveCheckId | null;
  tourBeat: number | null;
  connectionText?: string;
  actions: PaneActions;
};

export function renderChecksPaneView({
  ui,
  event,
  checks,
  entered,
  selectedId,
  openGroups,
  turned,
  fullPromptId,
  armedId,
  tourBeat,
  connectionText,
  actions,
}: PaneViewInput): RenderElement {
  const shown = checks.filter((check) => !check.isHidden);
  const selected = shown.find((check) => check.id === selectedId);
  const hasCard = GROUPS.some(
    (group) =>
      (!group.isFolded || openGroups.includes(group.title)) &&
      shown.some((check) => group.stages.includes(check.stage)),
  );
  const body = renderListView(
    ui,
    event,
    shown,
    shown.length < checks.length,
    openGroups,
    armedId,
    selected?.id ?? null,
    turned,
    entered,
    connectionText,
    fullPromptId,
    actions,
  );
  return renderPaneView(
    ui,
    event,
    body,
    hasCard,
    markSize(event.props.bodyColumns),
    tourBeat,
    TOUR.length,
    {
      nextTourStep: actions.nextTourStep,
      endTour: actions.endTour,
    },
  );
}
