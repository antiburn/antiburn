// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { FolderPermissionNotice, joinFolders } from './FolderPermissionNotice';
import type { DeferredPermissionDir } from '../../lib/types/repository';

const deferred: DeferredPermissionDir[] = [
  { dir: 'Documents', pathCount: 3 },
  { dir: 'Desktop', pathCount: 1 },
];

function renderNotice(overrides: Partial<Parameters<typeof FolderPermissionNotice>[0]> = {}) {
  const props = {
    deferred,
    phase: 'idle' as const,
    current: null,
    position: 0,
    total: 0,
    recordedDenials: [] as string[],
    onRequest: vi.fn(),
    onOpenSettings: vi.fn(),
    onRecheck: vi.fn(),
    onCopyDiagnostics: vi.fn(),
    ...overrides,
  };
  render(<FolderPermissionNotice {...props} />);
  return props;
}

describe('FolderPermissionNotice', () => {
  it('renders nothing when every folder is readable', () => {
    const { container } = render(
      <FolderPermissionNotice
        deferred={[]}
        phase="idle"
        current={null}
        position={0}
        total={0}
        recordedDenials={[]}
        onRequest={vi.fn()}
        onOpenSettings={vi.fn()}
        onRecheck={vi.fn()}
        onCopyDiagnostics={vi.fn()}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('names the folders and says what antiburn wants them for', () => {
    renderNotice();
    expect(screen.getByText(/Documents and Desktop/)).toBeInTheDocument();
    expect(screen.getByText(/nothing leaves this Mac/i)).toBeInTheDocument();
  });

  it('asks only when the reader presses the button', () => {
    const props = renderNotice();
    // Nothing may prompt on render: the explanation comes first.
    expect(props.onRequest).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: /grant access/i }));
    expect(props.onRequest).toHaveBeenCalledTimes(1);
  });

  it('says which folder it is waiting on, and where the dialog might be', () => {
    renderNotice({ phase: 'asking', current: 'Documents', position: 1, total: 2 });

    expect(screen.getByText(/Waiting for macOS about Documents/)).toBeInTheDocument();
    expect(screen.getByText(/check your other windows and displays/i)).toBeInTheDocument();
    expect(screen.getByText(/\(1 of 2\)/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /waiting for macOS/i })).toBeDisabled();
  });

  it('drops the futile ask when the system will not prompt again', () => {
    renderNotice({ recordedDenials: ['Documents', 'Desktop'] });

    // Pressing "Grant access" would show nothing at all, so it is not offered.
    expect(screen.queryByRole('button', { name: /grant access/i })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /open system settings/i })).toBeInTheDocument();
    expect(screen.getByText(/remembers an earlier/i)).toBeInTheDocument();
  });

  it('offers the whole recovery path once macOS will not ask again', () => {
    const props = renderNotice({ recordedDenials: ['Documents', 'Desktop'] });

    // Re-checking is how access granted in System Settings gets noticed. There
    // is deliberately no full-disk-access shortcut here: every blocked state
    // this app can reach is recoverable through the per-folder control.
    fireEvent.click(screen.getByRole('button', { name: /check again/i }));
    expect(props.onRecheck).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: /copy diagnostics/i }));
    expect(props.onCopyDiagnostics).toHaveBeenCalledTimes(1);
  });

  it('can re-check before anything has been refused', () => {
    const props = renderNotice();
    fireEvent.click(screen.getByRole('button', { name: /check again/i }));
    expect(props.onRecheck).toHaveBeenCalledTimes(1);
  });

  it('keeps offering to ask while any folder can still be prompted for', () => {
    renderNotice({ recordedDenials: ['Documents'] });
    expect(screen.getByRole('button', { name: /grant access/i })).toBeInTheDocument();
  });
});

describe('joinFolders', () => {
  it('reads as a sentence at every length', () => {
    expect(joinFolders([])).toBe('');
    expect(joinFolders(['Documents'])).toBe('Documents');
    expect(joinFolders(['Documents', 'Desktop'])).toBe('Documents and Desktop');
    expect(joinFolders(['Documents', 'Desktop', 'Downloads'])).toBe(
      'Documents, Desktop and Downloads',
    );
  });
});
