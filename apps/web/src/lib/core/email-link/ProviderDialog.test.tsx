/**
 * @vitest-environment jsdom
 */

import { fireEvent, render, screen } from '@solidjs/testing-library';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { EmailProviderDialog } from './ProviderDialog';

vi.mock('@queries/auth/outlook-link', () => ({
  fetchEmailConnectionProviders: vi.fn(),
}));

beforeAll(() => {
  vi.stubGlobal('scrollTo', vi.fn());
});

function renderDialog(zohoAvailable: boolean) {
  const onSelect = vi.fn();
  render(() => (
    <EmailProviderDialog
      open
      onOpenChange={vi.fn()}
      onSelect={onSelect}
      outlookAvailable
      zohoAvailable={zohoAvailable}
    />
  ));
  return onSelect;
}

describe('EmailProviderDialog', () => {
  it('connects Zoho Mail in the preselected European data center', () => {
    const onSelect = renderDialog(true);

    fireEvent.click(screen.getByRole('button', { name: 'Zoho Mail' }));

    expect(onSelect).toHaveBeenCalledExactlyOnceWith({
      provider: 'ZOHO',
      dataCenter: 'eu',
      servers: 'auto',
    });
  });

  it('offers no Zoho Mail where the server has not configured it', () => {
    renderDialog(false);

    expect(screen.queryByRole('button', { name: 'Zoho Mail' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Gmail' })).toBeTruthy();
  });
});
