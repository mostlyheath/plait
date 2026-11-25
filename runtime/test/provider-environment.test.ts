import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ProviderEnvironment, referencedNames, templateNames } from '../src/provider-environment.ts';

test('template names follow Pi\'s key grammar', () => {
  assert.deepEqual(templateNames('$NOUS_KEY'), ['NOUS_KEY']);
  assert.deepEqual(templateNames('${NOUS_KEY}'), ['NOUS_KEY']);
  assert.deepEqual(templateNames('Bearer ${A}-$B_2.x'), ['A', 'B_2']);
  // Escapes, commands, and malformed references name nothing.
  assert.deepEqual(templateNames('sk-$$A-$!B'), []);
  assert.deepEqual(templateNames('!pass show key $A'), []);
  assert.deepEqual(templateNames('${not valid} ${A $9 $'), []);
  assert.deepEqual(templateNames('literal'), []);
});

test('models.json names come from keys and headers only', () => {
  const file = JSON.stringify({ providers: {
    a: { apiKey: '$A_KEY', headers: { x: '${A_HEADER}', n: 5 }, baseUrl: '$NOT_READ', models: [{ id: 'm', headers: { y: '$M_HEADER' } }, null] },
    b: { apiKey: 'literal' }, c: null,
  } });
  assert.deepEqual(referencedNames(file).sort(), ['A_HEADER', 'A_KEY', 'M_HEADER']);
  assert.deepEqual(referencedNames('{ broken'), []);
  assert.deepEqual(referencedNames(undefined), []);
});

test('copied variables follow the login shell and never replace the host\'s own', () => {
  const target: NodeJS.ProcessEnv = { OWN: 'host' };
  const variables = new ProviderEnvironment(target);
  const names = new Set(['KEY', 'OWN', 'PLAIT_LOCK_FD']);
  assert.equal(variables.apply({ KEY: 'one', OWN: 'shell', PLAIT_LOCK_FD: '9', OTHER: 'x' }, names), true);
  assert.deepEqual(target, { OWN: 'host', KEY: 'one' });
  assert.equal(variables.apply({ KEY: 'one', OWN: 'shell' }, names), false);
  assert.equal(variables.apply({ KEY: 'two' }, names), true);
  assert.equal(target.KEY, 'two');
  assert.equal(variables.apply({}, names), true);
  assert.deepEqual(target, { OWN: 'host' });
});
