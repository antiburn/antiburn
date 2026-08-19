// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from 'vitest';

import { sessionKey } from './PopoverSession';

/**
 * `sessionKey` tags the analytics load a subject's payload belongs to. Get it
 * wrong and a subject that moves between environments — or a sub-agent whose
 * id happens to collide with one from a different parent — shows another
 * session's cached (or in-flight) analytics instead of its own.
 */

describe('sessionKey', () => {
  it('scopes by environment: the same agent and session id in different WSL distros are distinct', () => {
    const native = sessionKey({ agent: 'claude-code', sessionId: 'same-id', wslDistro: null });
    const ubuntu = sessionKey({
      agent: 'claude-code',
      sessionId: 'same-id',
      wslDistro: 'Ubuntu',
    });
    const debian = sessionKey({
      agent: 'claude-code',
      sessionId: 'same-id',
      wslDistro: 'Debian',
    });

    expect(native).not.toBe(ubuntu);
    expect(ubuntu).not.toBe(debian);
  });

  it('is case-insensitive on the WSL distribution name', () => {
    const lower = sessionKey({
      agent: 'claude-code',
      sessionId: 'same-id',
      wslDistro: 'ubuntu',
    });
    const upper = sessionKey({
      agent: 'claude-code',
      sessionId: 'same-id',
      wslDistro: 'UBUNTU',
    });

    expect(lower).toBe(upper);
  });

  it('scopes a sub-agent key by its parent session, not just the sub-agent id', () => {
    const parentOne = sessionKey({
      agent: 'claude-code',
      sessionId: 'same-subagent-id',
      wslDistro: null,
      subagent: { parentSessionId: 'parent-one', subagentId: 'same-subagent-id' },
    });
    const parentTwo = sessionKey({
      agent: 'claude-code',
      sessionId: 'same-subagent-id',
      wslDistro: null,
      subagent: { parentSessionId: 'parent-two', subagentId: 'same-subagent-id' },
    });

    expect(parentOne).not.toBe(parentTwo);
  });

  it('does not collide a sub-agent key with a top-level session of the same id', () => {
    const topLevel = sessionKey({
      agent: 'claude-code',
      sessionId: 'shared-id',
      wslDistro: null,
    });
    const subagent = sessionKey({
      agent: 'claude-code',
      sessionId: 'shared-id',
      wslDistro: null,
      subagent: { parentSessionId: 'shared-id', subagentId: 'sub-1' },
    });

    expect(topLevel).not.toBe(subagent);
  });
});
