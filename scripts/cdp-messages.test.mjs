import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dispatchCdpMessage } from './cdp-messages.mjs';

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
