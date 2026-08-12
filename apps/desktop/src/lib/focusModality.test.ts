// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { afterEach, describe, expect, it } from 'vitest';

import { installFocusModality, isKeyboardModality } from './focusModality';

let teardown: (() => void) | null = null;

function install() {
  teardown = installFocusModality();
}

afterEach(() => {
  teardown?.();
  teardown = null;
  document.documentElement.removeAttribute('data-keyboard');
});

describe('focus modality', () => {
  it('starts in pointer modality', () => {
    install();

    expect(isKeyboardModality()).toBe(false);
    expect(document.documentElement.hasAttribute('data-keyboard')).toBe(false);
  });

  it('arms on Tab and disarms on a pointer press', () => {
    install();

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab' }));
    expect(isKeyboardModality()).toBe(true);

    window.dispatchEvent(new Event('pointerdown'));
    expect(isKeyboardModality()).toBe(false);
  });

  it('ignores keys that are pressed while operating a control', () => {
    install();

    for (const key of ['ArrowDown', 'Enter', ' ', 'a']) {
      window.dispatchEvent(new KeyboardEvent('keydown', { key }));
      expect(isKeyboardModality()).toBe(false);
    }
  });

  it('disarms on a mouse or touch press that never produced a pointerdown', () => {
    install();

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab' }));
    window.dispatchEvent(new Event('mousedown'));
    expect(isKeyboardModality()).toBe(false);

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab' }));
    window.dispatchEvent(new Event('touchstart'));
    expect(isKeyboardModality()).toBe(false);
  });

  it('stops tracking once torn down', () => {
    install();
    teardown?.();
    teardown = null;

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab' }));
    expect(isKeyboardModality()).toBe(false);
  });

  it('tracks whichever root it was given', () => {
    const root = document.createElement('div');
    teardown = installFocusModality(root);

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab' }));
    expect(isKeyboardModality(root)).toBe(true);
    expect(isKeyboardModality()).toBe(false);
  });
});
