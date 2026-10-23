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
import { ZOHO_DATA_CENTERS, type ZohoDataCenter } from './zoho';

export type EmailProvider = 'GMAIL' | 'OUTLOOK';

/** A provider picked in the dialog; Zoho accounts live in one data center. */
export type ProviderChoice =
  | { provider: EmailProvider }
  | { provider: 'ZOHO'; dataCenter: ZohoDataCenter };

type DataCenterOption = (typeof ZOHO_DATA_CENTERS)[number];

/** Shared provider choice for onboarding, empty states and additional inboxes. */
export function EmailProviderDialog(
  props: ManagedDialogProps & {
    onSelect: (choice: ProviderChoice) => void;
    disabled?: boolean;
    outlookAvailable: boolean;
    zohoAvailable?: boolean;
  }
) {
  // The first data center (Europe) is preselected.
  const [dataCenter, setDataCenter] = createSignal<DataCenterOption>(
    ZOHO_DATA_CENTERS[0]
  );
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
              <Select<DataCenterOption>
                options={[...ZOHO_DATA_CENTERS]}
                value={dataCenter()}
                onChange={(option) => option && setDataCenter(option)}
                optionValue="value"
                optionTextValue="label"
                gutter={4}
                disabled={props.disabled}
                itemComponent={(item) => (
                  <Select.Item
                    item={item.item}
                    class="flex items-center justify-between gap-3 rounded-md px-2 py-1.5 text-sm outline-none data-highlighted:bg-hover"
                  >
                    <Select.ItemLabel>
                      {item.item.rawValue.label}
                    </Select.ItemLabel>
                    <Select.ItemIndicator>
                      <CheckIcon class="size-3" />
                    </Select.ItemIndicator>
                  </Select.Item>
                )}
              >
                <Select.Trigger
                  class="h-8 w-44 shrink-0 rounded-md border border-edge-muted px-2 text-sm text-ink-muted"
                  aria-label="Zoho data center"
                >
                  <Select.Value<DataCenterOption>>
                    {(state) => state.selectedOption().label}
                  </Select.Value>
                  <CaretDownIcon class="size-3 shrink-0 text-ink-subtle" />
                </Select.Trigger>
                <Select.Content>
                  <Select.Listbox />
                </Select.Content>
              </Select>
              <Button
                variant="strong"
                depth={3}
                class="min-w-0 flex-1"
                disabled={props.disabled}
                onClick={() =>
                  props.onSelect({
                    provider: 'ZOHO',
                    dataCenter: dataCenter().value,
                  })
                }
              >
                Zoho Mail
              </Button>
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
    zohoAvailable = providers.zoho ?? false;
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
