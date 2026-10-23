import { match } from 'ts-pattern';

export const MACRO_EMAIL_SIGNATURE = '-- Sent with Macro';

export const MAX_ATTACHMENTS_BYTES_SIZE = 18_000_000;
// Raw bytes the provider will still deliver once base64 adds about a third:
// Gmail caps a message's attachments at 25 MB; Exchange Online sends at most
// 35 MB by default and Outlook.com attachments stop at 25 MB. Graph accepts
// uploads up to 150 MB, but the mailbox bounces anything over its send size.
// IMAP (Zoho Mail) messages travel over SMTP, which caps them at 20 MB.
export function attachmentLimitBytes(
  provider?: 'GMAIL' | 'OUTLOOK' | 'IMAP'
): number {
  return match(provider)
    .with('OUTLOOK', () => 25_000_000)
    .with('IMAP', () => 14_000_000)
    .with('GMAIL', undefined, () => MAX_ATTACHMENTS_BYTES_SIZE)
    .exhaustive();
}

// Stable id for the compose "To" input so the create gesture can focus it
// synchronously on iOS (the virtual keyboard only opens within a user gesture).
export const EMAIL_COMPOSE_TO_INPUT_ID = 'email-compose-to-input';
