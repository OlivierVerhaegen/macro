import { openExternalUrl } from '@core/util/url';
import CaretDownIcon from '@phosphor/caret-down.svg';
import CheckIcon from '@phosphor/check.svg';
import { fetchEmailConnectionProviders } from '@queries/auth/outlook-link';
import {
  Button,
  Dialog,
  type ManagedDialogProps,
  openDialog,
  Panel,
  Select,
} from '@ui';
import { createSignal, type Owner, Show } from 'solid-js';
import {
  ZOHO_DATA_CENTERS,
  ZOHO_SERVER_CHOICES,
  ZOHO_SERVER_DOCS_URL,
  type ZohoDataCenter,
  type ZohoServerChoice,
} from './zoho';

export type EmailProvider = 'GMAIL' | 'OUTLOOK';

/** A provider picked in the dialog; Zoho accounts live in one data center. */
export type ProviderChoice =
  | { provider: EmailProvider }
  | {
      provider: 'ZOHO';
      dataCenter: ZohoDataCenter;
      servers: ZohoServerChoice;
    };

type ZohoOption = { readonly value: string; readonly label: string };

/** One of the Zoho connection choices, shown before consent. */
function ZohoSelect<Option extends ZohoOption>(props: {
  label: string;
  options: readonly Option[];
  value: Option;
  onChange: (option: Option) => void;
  disabled?: boolean;
}) {
  return (
    <Select<Option>
      options={[...props.options]}
      value={props.value}
      onChange={(option) => option && props.onChange(option)}
      optionValue="value"
      optionTextValue="label"
      gutter={4}
      disabled={props.disabled}
      itemComponent={(item) => (
        <Select.Item
          item={item.item}
          class="flex items-center justify-between gap-3 rounded-md px-2 py-1.5 text-sm outline-none data-highlighted:bg-hover"
        >
          <Select.ItemLabel>{item.item.rawValue.label}</Select.ItemLabel>
          <Select.ItemIndicator>
            <CheckIcon class="size-3" />
          </Select.ItemIndicator>
        </Select.Item>
      )}
    >
      <Select.Trigger
        class="h-8 min-w-0 flex-1 rounded-md border border-edge-muted px-2 text-sm text-ink-muted"
        aria-label={props.label}
      >
        <Select.Value<Option>>
          {(state) => state.selectedOption().label}
        </Select.Value>
        <CaretDownIcon class="size-3 shrink-0 text-ink-subtle" />
      </Select.Trigger>
      <Select.Content>
        <Select.Listbox />
      </Select.Content>
    </Select>
  );
}

/** Shared provider choice for onboarding, empty states and additional inboxes. */
export function EmailProviderDialog(
  props: ManagedDialogProps & {
    onSelect: (choice: ProviderChoice) => void;
    disabled?: boolean;
    outlookAvailable: boolean;
    zohoAvailable?: boolean;
  }
) {
  // Europe and the automatic server choice are preselected.
  const [dataCenter, setDataCenter] = createSignal<
    (typeof ZOHO_DATA_CENTERS)[number]
  >(ZOHO_DATA_CENTERS[0]);
  const [servers, setServers] = createSignal<
    (typeof ZOHO_SERVER_CHOICES)[number]
  >(ZOHO_SERVER_CHOICES[0]);
  return (
    <Dialog
      open={props.open}
      onOpenChange={props.onOpenChange}
      position="center"
      class="w-120 max-w-[calc(100vw-2rem)]"
    >
      <Panel depth={2} class="rounded-xl">
        <Panel.Header class="px-6">
          <Dialog.Title class="text-ink text-sm font-semibold">
            Connect email
          </Dialog.Title>
        </Panel.Header>
        <Panel.Body class="p-6 font-sans flex flex-col gap-3">
          <Dialog.Description class="text-ink-muted text-sm/tight">
            <Show
              when={props.zohoAvailable}
              fallback="Connect Gmail, Outlook.com, or an individual Microsoft 365 account."
            >
              Connect Gmail, Outlook.com, an individual Microsoft 365 account,
              or Zoho Mail.
            </Show>
          </Dialog.Description>
          <Button
            variant="strong"
            depth={3}
            disabled={props.disabled}
            onClick={() => props.onSelect({ provider: 'GMAIL' })}
          >
            Gmail
          </Button>
          <Button
            variant="strong"
            depth={3}
            disabled={props.disabled || !props.outlookAvailable}
            onClick={() => props.onSelect({ provider: 'OUTLOOK' })}
          >
            Outlook / Microsoft 365
          </Button>
          <Show when={!props.outlookAvailable}>
            <p class="text-ink-muted text-sm">
              New Outlook connections are temporarily unavailable.
            </p>
          </Show>
          <Show when={props.zohoAvailable}>
            <div class="flex flex-wrap items-center gap-2">
              <ZohoSelect
                label="Zoho data center"
                options={ZOHO_DATA_CENTERS}
                value={dataCenter()}
                onChange={setDataCenter}
                disabled={props.disabled}
              />
              <Button
                variant="strong"
                depth={3}
                class="min-w-0 flex-1"
                disabled={props.disabled}
                onClick={() =>
                  props.onSelect({
                    provider: 'ZOHO',
                    dataCenter: dataCenter().value,
                    servers: servers().value,
                  })
                }
              >
                Zoho Mail
              </Button>
            </div>
            <div class="flex flex-col gap-1">
              <ZohoSelect
                label="Zoho mail servers"
                options={ZOHO_SERVER_CHOICES}
                value={servers()}
                onChange={setServers}
                disabled={props.disabled}
              />
              <p class="text-xs text-ink-muted">
                Paid organizations on their own domain use imappro/smtppro;
                personal and free accounts use imap/smtp.{' '}
                <a
                  href={ZOHO_SERVER_DOCS_URL}
                  class="underline"
                  onClick={(event) => {
                    event.preventDefault();
                    openExternalUrl(ZOHO_SERVER_DOCS_URL);
                  }}
                >
                  Check which servers your account uses
                </a>
                .
              </p>
            </div>
          </Show>
          <Button
            variant="ghost"
            depth={3}
            disabled={props.disabled}
            onClick={() => props.onOpenChange(false)}
          >
            Cancel
          </Button>
        </Panel.Body>
      </Panel>
    </Dialog>
  );
}

/** Cancellation and owner disposal resolve without starting an OAuth attempt. */
export async function selectEmailProvider(
  owner: Owner | null
): Promise<ProviderChoice | undefined> {
  let outlookAvailable = false;
  let zohoAvailable = false;
  try {
    const providers = await fetchEmailConnectionProviders();
    outlookAvailable = providers.outlook;
    zohoAvailable = providers.zoho;
  } catch {
    /* Gmail remains available when the availability read fails. */
  }
  let selected: ProviderChoice | undefined;
  const handle = openDialog(
    EmailProviderDialog,
    {
      outlookAvailable,
      zohoAvailable,
      onSelect: (choice) => {
        selected = choice;
        handle.close();
      },
    },
    { owner }
  );
  await handle.closed;
  return selected;
}
