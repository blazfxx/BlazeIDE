/** BlazeAPI: the account, billing and every model behind the IDE. */
export const SITE_BASE = 'https://blazeapi.org';
export const BILLING_URL = `${SITE_BASE}/dashboard/billing`;
export const MODELS_URL = `${SITE_BASE}/models`;

export const PROVIDER_COLORS: Record<string, string> = {
  Anthropic: '#ffffff',
  OpenAI: '#c8c8c8',
  Google: '#a8a8a8',
  Meta: '#8a8a8a',
  DeepSeek: '#e6e6e6',
};
