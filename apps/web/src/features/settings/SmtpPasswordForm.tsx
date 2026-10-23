import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import {
  isSmtpPasswordRejected,
  useSetSmtpPasswordMutation,
} from '@queries/email/link';
import { Button } from '@ui';
import { createSignal } from 'solid-js';
import { SetupField } from './components/runtime-setup-steps';

/**
 * Zoho's SMTP server takes no OAuth, so an IMAP inbox sends with an app
 * password. The server checks it before it is stored; it is never shown again.
 */
export function SmtpPasswordForm(props: {
  linkId: string;
  emailAddress: string;
}) {
  const [password, setPassword] = createSignal('');
  const save = useSetSmtpPasswordMutation();
  const inputId = () => `smtp-password-${props.linkId}`;

  const submit = async () => {
    const value = password().trim();
    if (!value || save.isPending) return;
    try {
      await save.mutateAsync({ linkId: props.linkId, password: value });
      setPassword('');
      toast.success('App password saved');
    } catch (error) {
      toast.failure(
        isSmtpPasswordRejected(error)
          ? 'Zoho rejected this app password'
          : 'Failed to save the app password'
      );
    }
  };

  return (
    <form
      class="flex flex-col gap-1.5 px-4 pb-3"
      onSubmit={(event) => {
        event.preventDefault();
        fireAndForget(submit(), 'smtp-password');
      }}
    >
      <label for={inputId()} class="text-xs font-medium text-ink">
        App password for sending
      </label>
      <div class="flex flex-wrap items-center gap-2">
        <SetupField class="min-w-48 flex-1">
          <input
            id={inputId()}
            type="password"
            autocomplete="off"
            spellcheck={false}
            aria-describedby={`${inputId()}-help`}
            class="ph-no-capture min-w-0 flex-1 bg-transparent font-mono text-sm text-ink outline-none"
            value={password()}
            disabled={save.isPending}
            onInput={(event) => setPassword(event.currentTarget.value)}
          />
        </SetupField>
        <Button
          type="submit"
          variant="outline"
          size="md"
          depth={3}
          disabled={save.isPending || !password().trim()}
          aria-label={`Save app password for ${props.emailAddress}`}
        >
          Save
        </Button>
      </div>
      <p id={`${inputId()}-help`} class="text-xs text-ink-extra-muted">
        Zoho needs an app password to send mail. Create one in Zoho Accounts
        under Security, App Passwords. Macro stores it encrypted; saving a new
        one replaces it.
      </p>
    </form>
  );
}
