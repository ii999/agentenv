// Self-test for the fixture server: every page renders on both origins with
// template substitution applied, the receipt endpoint round-trips, and the
// scenario catalog references only existing pages, values and reasons.
import { readFile, readdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { startFixtureServer } from './server.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const failures = [];
const check = (condition, message) => {
  if (!condition) failures.push(message);
};

const server = await startFixtureServer();
try {
  const pages = (await readdir(join(here, 'pages'))).filter((name) => name.endsWith('.html'));
  for (const origin of [server.originA, server.originB]) {
    const health = await (await fetch(`${origin}/health`)).json();
    check(health.ok === true, `${origin}/health`);
    for (const page of pages) {
      const response = await fetch(`${origin}/pages/${page}`);
      const body = await response.text();
      check(response.status === 200, `${origin}/pages/${page} status ${response.status}`);
      check(!body.includes('{{'), `${origin}/pages/${page} has unreplaced template markers`);
      if (body.includes('{{OTHER}}') || page === 'frames.html') {
        const other = origin === server.originA ? server.originB : server.originA;
        check(body.includes(other), `${origin}/pages/${page} does not reference the other origin`);
      }
    }
    const script = await fetch(`${origin}/fixture.js`);
    check(script.status === 200, `${origin}/fixture.js`);
  }
  for (const vendor of ['/vendor/react.js', '/vendor/react-dom.js']) {
    const response = await fetch(`${server.originA}${vendor}`);
    check(response.status === 200, `${vendor} status ${response.status}`);
  }
  const traversal = await fetch(`${server.originA}/pages/..%2Fserver.mjs`);
  check(traversal.status === 404, 'path traversal must be rejected');

  await fetch(`${server.originA}/receipt`, { method: 'DELETE' });
  const posted = await fetch(`${server.originB}/receipt`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ instance: 'selftest:1', field: 'password', value: 'v', events: ['input'] }),
  });
  check(posted.status === 204, `receipt post status ${posted.status}`);
  const listed = await (await fetch(`${server.originA}/receipt`)).json();
  check(listed.receipts.length === 1, 'one receipt after post');
  check(listed.receipts[0]?.origin === server.originB, 'receipt records the posting origin');
  await fetch(`${server.originA}/receipt`, { method: 'DELETE' });
  const cleared = await (await fetch(`${server.originA}/receipt`)).json();
  check(cleared.receipts.length === 0, 'receipts cleared');

  const catalog = JSON.parse(await readFile(join(here, '..', 'scenarios.json'), 'utf8'));
  const ids = new Set();
  for (const scenario of catalog.scenarios) {
    check(!ids.has(scenario.id), `duplicate scenario id ${scenario.id}`);
    ids.add(scenario.id);
    check(scenario.value in catalog.values, `${scenario.id}: unknown value ${scenario.value}`);
    check(['filled', 'error'].includes(scenario.expect.outcome), `${scenario.id}: bad outcome`);
    if (scenario.expect.outcome === 'error') {
      check(catalog.reasons.includes(scenario.expect.reason), `${scenario.id}: unknown reason ${scenario.expect.reason}`);
      check(scenario.expect.receipt === null, `${scenario.id}: error scenarios expect no receipt`);
    } else {
      check(scenario.expect.receipt && scenario.expect.receipt.value in catalog.values, `${scenario.id}: receipt value`);
    }
    const refs = new Set();
    for (const open of scenario.open) {
      check(pages.includes(open.page), `${scenario.id}: unknown page ${open.page}`);
      check(['A', 'B'].includes(open.origin), `${scenario.id}: origin`);
      check(!refs.has(open.ref), `${scenario.id}: duplicate ref ${open.ref}`);
      refs.add(open.ref);
    }
    if (scenario.expect.receipt) {
      check(refs.has(scenario.expect.receipt.ref), `${scenario.id}: receipt ref not opened`);
    }
    check(typeof scenario.request.selector === 'string', `${scenario.id}: selector`);
    check(/^\{[AB]\}\//.test(scenario.request.pageUrl), `${scenario.id}: pageUrl must start with an origin placeholder`);
  }
} finally {
  await server.close();
}

if (failures.length > 0) {
  console.error(`fixture selftest failed:\n${failures.map((line) => `  - ${line}`).join('\n')}`);
  process.exit(1);
}
console.log(`fixture selftest passed (${server.ipv6 ? 'ipv4+ipv6' : 'ipv4 only'})`);
