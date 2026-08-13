export interface OAuth2Entity {
  id: number;
  description?: string;
  client_id: string;
  client_secret: string;
  auth_url: string;
  token_url: string;
  redirect_uri: string;
  scopes?: string[];
  extra_params?: Record<string, string>;
  enabled: boolean;
  use_proxy?: number;
  grant_type: 'AuthorizationCode' | 'ClientCredentials';
  created_at: number;
  updated_at: number;
}