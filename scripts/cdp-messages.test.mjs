import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Session } from 'node:inspector/promises';
import { createCdpEvaluator, dispatchCdpMessage } from './cdp-messages.mjs';

test('passes hostile paths as data through the real runtime protocol', async t => {
  const session = new Session();
  session.connect();
  t.after(() => session.disconnect());
  const calls = [];
  const evaluate = createCdpEvaluator(async (method, params) => {
    // Exercise the same JSON transport boundary as the WebSocket harness.
    const wire = JSON.parse(JSON.stringify(params));
    calls.push({ method, params: wire });
    return { result: await session.post(method, wire) };
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
  assert.deepEqual(await evaluate('await Promise.resolve(); return input;', input), input);
  assert.equal(await evaluate('return typeof globalThis.__gitcraftInjected;'), 'undefined');
  assert.equal(calls.filter(c => c.method === 'Runtime.evaluate').length, 1);
  const call = calls.find(c => c.method === 'Runtime.callFunctionOn');
  assert.deepEqual(call.params.arguments, [{ value: input }]);
  for (const path of paths) assert.equal(call.params.functionDeclaration.includes(path), false);
  // The harness relies on global state surviving successive calls.
  t.after(() => { delete globalThis.__gitcraftCdpFixture; });
  await evaluate('globalThis.__gitcraftCdpFixture = input.paths;', { paths });
  assert.deepEqual(await evaluate('return globalThis.__gitcraftCdpFixture;'), paths);
  await assert.rejects(evaluate('throw new Error("fixture exception");'), /fixture exception/);
});

test('reports protocol errors and missing runtime objects', async () => {
  for (const response of [
    { error: { message: 'protocol failure' } },
    { result: { exceptionDetails: { text: 'runtime failure' } } },
    { result: { result: {} } },
  ]) {
    await assert.rejects(createCdpEvaluator(async () => response)('return 1;'));
  }
  const evaluate = createCdpEvaluator(async method => method === 'Runtime.evaluate'
    ? { result: { result: { objectId: 'global' } } }
    : { error: { message: 'function call failed' } });
  await assert.rejects(evaluate('return 1;'), /function call failed/);
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
