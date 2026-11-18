/** Evaluate trusted harness code, passing runtime values as protocol data. */
export function createCdpEvaluator(send) {
  let objectId;
  return async function evaluate(code, input = {}) {
    // Startup can navigate after the first poll, invalidating the global handle.
    for (let attempt = 0; attempt < 2; attempt++) {
      if (!objectId) {
        const global = await send('Runtime.evaluate', { expression: 'globalThis' });
        if (global.error || global.result?.exceptionDetails) {
          throw new Error(JSON.stringify(global.error ?? global.result.exceptionDetails));
        }
        objectId = global.result?.result?.objectId;
        if (!objectId) throw new Error('CDP global object is unavailable');
      }
      const response = await send('Runtime.callFunctionOn', {
        objectId,
        functionDeclaration: `async function(input) {${code}\n}`,
        arguments: [{ value: input }],
        awaitPromise: true,
        returnByValue: true,
      });
      if (attempt === 0 && response.error?.code === -32000 &&
          /Cannot find context with specified id|Execution context was destroyed|Could not find object with given id/.test(response.error.message)) {
        objectId = undefined;
        continue;
      }
      if (response.error || response.result?.exceptionDetails) {
        throw new Error(JSON.stringify(response.error ?? response.result.exceptionDetails));
      }
      return response.result?.result?.value;
    }
  };
}

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
