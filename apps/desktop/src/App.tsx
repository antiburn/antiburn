// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useRoute } from './lib/route';
import { NudgeView } from './views/NudgeView';
import { OnboardingView } from './views/OnboardingView';
import { PopoverView } from './views/PopoverView';
import { SettingsView } from './views/SettingsView';

export function App() {
  const route = useRoute();
  switch (route) {
    case 'settings':
      return <SettingsView />;
    case 'nudge':
      return <NudgeView />;
    case 'onboarding':
      return <OnboardingView />;
    default:
      return <PopoverView />;
  }
}
