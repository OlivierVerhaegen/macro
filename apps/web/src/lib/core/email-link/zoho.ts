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
