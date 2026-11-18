import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createCdpEvaluator, dispatchCdpMessage } from './cdp-messages.mjs';

test('keeps paths separate from code in browser protocol messages', async () => {
  const calls = [];
  const evaluate = createCdpEvaluator(async (method, params) => {
    const wire = JSON.parse(JSON.stringify(params));
    calls.push({ method, params: wire });
    if (method === 'Runtime.evaluate') {
      return { result: { result: { objectId: 'global' } } };
    }
    return { result: { result: { value: 'ok' } } };
  });
  const paths = [
    'C:\\nested folder\\caf\u00e9 \u65e5\u672c\\selected \u03bb file.txt',
    '\\\\localhost\\C$\\folder\\file.txt',
    '\");globalThis.__gitcraftInjected = true;//',
    "');globalThis.__gitcraftInjected = true;//",
    '</script><script>globalThis.__gitcraftInjected = true</script>',
    '` ${globalThis.__gitcraftInjected = true} \\ \u0000\b\f\n\r\t\u2028\u2029',
  ];
  const input = { paths, closedAt: 123.5 };
  const code = 'await Promise.resolve(); return input;';
  assert.equal(await evaluate(code, input), 'ok');
  assert.equal(await evaluate(code, input), 'ok');

  const [lookup, ...invocations] = calls;
  assert.deepEqual(lookup, {
    method: 'Runtime.evaluate',
    params: { expression: 'globalThis' },
  });
  assert.equal(invocations.length, 2);
  for (const call of invocations) {
    assert.deepEqual(call, {
      method: 'Runtime.callFunctionOn',
      params: {
        objectId: 'global',
        functionDeclaration: `async function(input) {${code}\n}`,
        arguments: [{ value: input }],
        awaitPromise: true,
        returnByValue: true,
      },
    });
  }
});

test('reports protocol errors and missing runtime objects', async () => {
  for (const response of [
    { error: { message: 'protocol failure' } },
    { result: { exceptionDetails: { text: 'runtime failure' } } },
    { result: { result: {} } },
  ]) {
    await assert.rejects(createCdpEvaluator(async () => response)('return 1;'));
  }
  for (const [response, message] of [
    [{ error: { message: 'function call failed' } }, /function call failed/],
    [{ result: { exceptionDetails: { text: 'fixture exception' } } }, /fixture exception/],
  ]) {
    const evaluate = createCdpEvaluator(async method => method === 'Runtime.evaluate'
      ? { result: { result: { objectId: 'global' } } }
      : response);
    await assert.rejects(evaluate('return 1;'), message);
  }
});

test('refreshes a global handle after navigation, with a bounded retry', async () => {
  let globals = 0, calls = 0;
  const input = { path: 'C:\\fixture' };
  const evaluate = createCdpEvaluator(async (method, params) => {
    if (method === 'Runtime.evaluate') return { result: { result: { objectId: `global-${++globals}` } } };
    assert.deepEqual(params.arguments, [{ value: input }]);
    if (++calls === 1) return { error: { code: -32000, message: 'Cannot find context with specified id' } };
    assert.equal(params.objectId, 'global-2');
    return { result: { result: { value: 'ready' } } };
  });
  assert.equal(await evaluate('return input.path;', input), 'ready');
  assert.equal(globals, 2);
  assert.equal(calls, 2);

  let retries = 0;
  const unavailable = createCdpEvaluator(async method => method === 'Runtime.evaluate'
    ? { result: { result: { objectId: 'global' } } }
    : (++retries, { error: { code: -32000, message: 'Cannot find context with specified id' } }));
  await assert.rejects(unavailable('return 1;'), /Cannot find context/);
  assert.equal(retries, 2);
});

test('resolves a registered response once, removing it before callback execution', () => {
  const errors = [], received = [];
  const pending = new Map([[1, message => { assert.equal(pending.has(1), false); received.push(message.result); }]]);
  dispatchCdpMessage('{"id":1,"result":{"value":"ok"}}', pending, errors);
  dispatchCdpMessage('{"id":1,"result":{"value":"duplicate"}}', pending, errors);
  assert.deepEqual(received, [{value:'ok'}]); assert.deepEqual(errors, []);
});

test('rejects malformed, unknown and non-numeric response IDs without invoking other targets', () => {
  let calls = 0;
  const pending = new Map([[1, () => calls++], [2, { unexpected:'object' }]]);
  for (const data of ['{', 'null', '[]', '1', '{"id":"__proto__"}', '{"id":"constructor"}', '{"id":"1"}', '{"id":0}', '{"id":-1}', '{"id":1.5}', '{"id":9007199254740992}', '{"id":99}', '{"id":2}', null]) {
    assert.doesNotThrow(() => dispatchCdpMessage(data, pending, []));
  }
  assert.equal(calls, 0); assert.equal(pending.has(1), true);
});

test('collects exception notifications independently of response dispatch', () => {
  const errors = [];
  dispatchCdpMessage('{"method":"Runtime.exceptionThrown","params":{"exceptionDetails":{"text":"fixture"}}}', new Map(), errors);
  dispatchCdpMessage('{"method":"Runtime.exceptionThrown"}', new Map(), errors);
  dispatchCdpMessage('{"method":"Other.event","params":{"exceptionDetails":{}}}', new Map(), errors);
  assert.deepEqual(errors, [{text:'fixture'}]);
});
