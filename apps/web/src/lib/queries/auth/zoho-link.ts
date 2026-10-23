import type { ZohoDataCenter } from '@core/email-link/zoho';
import { authServiceClient } from '@service-auth/client';
import { useMutation } from '@tanstack/solid-query';

/** OAuth transport stays in the auth client, shared query orchestration here. */
export function useInitZohoLink() {
  return useMutation(() => ({
    mutationFn: (params: {
      originalUrl: string;
      dataCenter: ZohoDataCenter;
      reconnectLinkId?: string;
    }) => authServiceClient.initZohoLink(params.originalUrl, params),
  }));
}
