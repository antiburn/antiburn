// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { lazy, Suspense } from 'react';

import { useRoute } from './lib/route';
import { NudgeView } from './views/NudgeView';

// Keep the nudge view in the entry chunk. The shell prewarms this webview and
// can deliver a pending nudge immediately after startup; making the listener
// wait for a route chunk would add an avoidable readiness race. The larger,
// user-facing windows are loaded only by the webview that owns their route.
const OnboardingView = lazy(() =>
  import('./views/OnboardingView').then(({ OnboardingView: view }) => ({ default: view })),
);
const PopoverView = lazy(() =>
  import('./views/PopoverView').then(({ PopoverView: view }) => ({ default: view })),
);
const SettingsView = lazy(() =>
  import('./views/SettingsView').then(({ SettingsView: view }) => ({ default: view })),
);

function RouteLoading() {
  return <div className="h-full" aria-busy="true" data-testid="route-loading" />;
}

export function App() {
  const route = useRoute();
  if (route === 'nudge') return <NudgeView />;

  const view = (() => {
    switch (route) {
      case 'settings':
        return <SettingsView />;
      case 'onboarding':
        return <OnboardingView />;
      default:
        return <PopoverView />;
    }
  })();

  return <Suspense fallback={<RouteLoading />}>{view}</Suspense>;
}
