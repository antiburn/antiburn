// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Tracks whether the user is currently navigating by keyboard.
 *
 * `:focus-visible` alone is not enough here: webviews paint a focus ring for
 * programmatic and window-activation focus too, so a menu-bar window that
 * restores focus when it reopens would show a ring nobody asked for. The
 * design foundation therefore gates its focus ring on `html[data-keyboard]`
 * (see src/styles/focus.css), and this module is what sets and clears it.
 *
 * The rule is deliberately narrow: only Tab arms keyboard modality. Arrow
 * keys, Space, and Enter all occur while driving a control the pointer just
 * clicked, and treating those as keyboard navigation makes a ring appear
 * mid-click.
 */

const ATTRIBUTE = 'data-keyboard';

/** True while the last input that moved focus was a Tab press. */
export function isKeyboardModality(root: HTMLElement = document.documentElement): boolean {
  return root.hasAttribute(ATTRIBUTE);
}

/**
 * Starts tracking focus modality on the document.
 *
 * Listeners are registered in the capture phase so a component that stops
 * propagation cannot desynchronize the attribute from reality. Returns a
 * teardown function; calling it twice is harmless.
 */
export function installFocusModality(root: HTMLElement = document.documentElement): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Tab') {
      root.setAttribute(ATTRIBUTE, '');
    }
  };
  const onPointer = () => {
    root.removeAttribute(ATTRIBUTE);
  };

  window.addEventListener('keydown', onKeyDown, true);
  window.addEventListener('pointerdown', onPointer, true);
  // A pointer press that starts outside the window (a menu-bar click that
  // reveals the popover) never produces a pointerdown here, but it does end in
  // one of these, so clear on them too.
  window.addEventListener('mousedown', onPointer, true);
  window.addEventListener('touchstart', onPointer, true);

  return () => {
    window.removeEventListener('keydown', onKeyDown, true);
    window.removeEventListener('pointerdown', onPointer, true);
    window.removeEventListener('mousedown', onPointer, true);
    window.removeEventListener('touchstart', onPointer, true);
  };
}
