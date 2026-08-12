// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useRoute } from './lib/route';
import { PopoverView } from './views/PopoverView';
import { SettingsView } from './views/SettingsView';

export function App() {
  const route = useRoute();
  return route === 'settings' ? <SettingsView /> : <PopoverView />;
}
