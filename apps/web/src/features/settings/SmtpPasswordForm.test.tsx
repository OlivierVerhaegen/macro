/**
 * @vitest-environment jsdom
 */

import { ThrownResultError } from '@core/util/result';
import { fireEvent, render, screen, waitFor } from '@solidjs/testing-library';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SmtpPasswordForm } from './SmtpPasswordForm';

const mocks = vi.hoisted(() => ({
  save: vi.fn(),
  toastSuccess: vi.fn(),
  toastFailure: vi.fn(),
}));

vi.mock('@queries/email/link', () => ({
  useSetSmtpPasswordMutation: () => ({
    mutateAsync: mocks.save,
    isPending: false,
  }),
  isSmtpPasswordRejected: (error: unknown) =>
    error instanceof ThrownResultError &&
    error.errors.some((e) => e.code === 'SMTP_PASSWORD_REJECTED'),
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: { success: mocks.toastSuccess, failure: mocks.toastFailure },
}));

beforeEach(() => {
  vi.clearAllMocks();
  mocks.save.mockResolvedValue(undefined);
});

function renderForm() {
  render(() => (
    <SmtpPasswordForm linkId="zoho-link" emailAddress="contact@example.be" />
  ));
  return {
    input: screen.getByLabelText<HTMLInputElement>('App password for sending'),
    save: screen.getByRole<HTMLButtonElement>('button', {
      name: 'Save app password for contact@example.be',
    }),
  };
}

describe('SmtpPasswordForm', () => {
  it('saves the app password and clears the field', async () => {
    const { input, save } = renderForm();
    expect(save.disabled).toBe(true);

    fireEvent.input(input, { target: { value: 'app-password' } });
    fireEvent.click(save);

    await waitFor(() =>
      expect(mocks.toastSuccess).toHaveBeenCalledWith('App password saved')
    );
    expect(mocks.save).toHaveBeenCalledExactlyOnceWith({
      linkId: 'zoho-link',
      password: 'app-password',
    });
    expect(input.value).toBe('');
  });

  it('says when Zoho refuses the password and keeps it for correction', async () => {
    mocks.save.mockRejectedValue(
      new ThrownResultError([
        { code: 'SMTP_PASSWORD_REJECTED', message: 'rejected' },
      ])
    );
    const { input, save } = renderForm();

    fireEvent.input(input, { target: { value: 'wrong' } });
    fireEvent.click(save);

    await waitFor(() =>
      expect(mocks.toastFailure).toHaveBeenCalledWith(
        'Zoho rejected this app password'
      )
    );
    expect(input.value).toBe('wrong');
  });
});
