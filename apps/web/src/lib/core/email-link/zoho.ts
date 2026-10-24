/** Zoho data centers, as the authentication service's `data_center` names them. */
export const ZOHO_DATA_CENTERS = [
  { value: 'eu', label: 'Europe (zoho.eu)' },
  { value: 'com', label: 'United States (zoho.com)' },
  { value: 'in', label: 'India (zoho.in)' },
  { value: 'com.au', label: 'Australia (zoho.com.au)' },
  { value: 'jp', label: 'Japan (zoho.jp)' },
  { value: 'ca', label: 'Canada (zohocloud.ca)' },
  { value: 'sa', label: 'Saudi Arabia (zoho.sa)' },
  { value: 'com.cn', label: 'China (zoho.com.cn)' },
] as const;

export type ZohoDataCenter = (typeof ZOHO_DATA_CENTERS)[number]['value'];

/** Zoho's IMAP/SMTP server pairs, as the authentication service's `servers` names them. */
export const ZOHO_SERVER_CHOICES = [
  { value: 'auto', label: 'Automatic: pro for your own domain' },
  { value: 'pro', label: 'imappro / smtppro (paid organization)' },
  { value: 'standard', label: 'imap / smtp (personal or free)' },
] as const;

export type ZohoServerChoice = (typeof ZOHO_SERVER_CHOICES)[number]['value'];

/** Zoho's list of which servers each kind of account uses. */
export const ZOHO_SERVER_DOCS_URL =
  'https://www.zoho.com/mail/help/imap-access.html#server-details';
