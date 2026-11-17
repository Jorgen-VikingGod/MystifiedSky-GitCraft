/** Dispatch only well-formed responses to callbacks registered by this process. */
export function dispatchCdpMessage(data, pending, errors) {
  if (typeof data !== 'string') return;
  let message;
  try { message = JSON.parse(data); } catch { return; }
  if (!message || typeof message !== 'object' || Array.isArray(message)) return;
  if (Number.isSafeInteger(message.id) && message.id > 0) {
    const callback = pending.get(message.id);
    if (typeof callback !== 'function') return;
    pending.delete(message.id);
    callback(message);
  } else if (message.method === 'Runtime.exceptionThrown' && message.params?.exceptionDetails) {
    errors.push(message.params.exceptionDetails);
  }
}
