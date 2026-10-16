// User-facing explanations for the Host's stable failure codes.

export const REPLY_FAILURE_TEXT: Record<string, string> = {
  QuotaExceeded: "The provider's quota has been reached.",
  Unauthenticated: "The API key was rejected. Check the connection in Models.",
  DeadlineExceeded: "The reply took too long and was stopped.",
  ProviderUnavailable: "The AI provider could not respond.",
  Forbidden: "The provider blocked the request.",
  InvalidRequest: "The provider rejected the request.",
  Interrupted: "The Host stopped before the reply finished.",
  Cancelled: "Stopped by you. The provider may still have processed the request.",
};

export function replyFailure(failure: { code: string; message: string }): string {
  const known = REPLY_FAILURE_TEXT[failure.code];
  return known ? `${known} (${failure.message})` : `${failure.code}: ${failure.message}`;
}
