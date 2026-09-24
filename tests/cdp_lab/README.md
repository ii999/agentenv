# CDP filling parity lab

This lab produces the Phase A and Phase C evidence for
`docs/design/credential-fill.md`: whether a native Rust Chrome DevTools
Protocol (CDP) client reaches full parity with a `playwright-core`
`connectOverCDP()` reference for the credential filling contract, against a
real Chromium-family browser, and whether the product's
`agentenv credential fill --backend cdp` keeps that parity.

It is a test fixture, not product code. Values it fills are test strings from
`scenarios.json`; no real credential is read or stored. The product run
reads each scenario value from an `env` credential in a generated
configuration file, never from arguments.

## Layout

| Path | Purpose |
| --- | --- |
| `fixtures/` | Fixture server, page instrumentation, and scenario pages |
| `scenarios.json` | Scenario catalog shared by both clients and the harness |
| `reference/` | Node reference client using `playwright-core` |
| `native/` | Standalone Rust CDP client prototype |
| `harness/`, `run.py` | Browser launch, host session, parity comparison, evidence |
| `recording_probe/` | Probe for which recorder states another CDP client can observe |

## Fixture server

`node fixtures/server.mjs [--port N]` prints one JSON line with the chosen
port and both origins, then serves until stdin closes. Pages are served on
`http://127.0.0.1:<port>` (origin A) and `http://localhost:<port>` (origin B).
Chromium treats these as different sites, so an iframe from the other origin
runs out of process. Templates replace `{{SELF}}` and `{{OTHER}}` with the
serving origin and the opposite origin.

Every field with an `id` is instrumented by `fixture.js`. It records the
trusted event sequence and posts the field's value to `POST /receipt` on each
`input` or `change` event. `GET /receipt` lists receipts; `DELETE /receipt`
clears them. The React page reports React state instead of the DOM value.
Each document has a random `instance` so receipts can be attributed to the
page and frame the harness opened.

Run the self-test with:

```sh
node tests/cdp_lab/fixtures/selftest.mjs
```

Fixture dependencies (React 18 UMD builds) are declared in
`fixtures/package.json`; install them with `pnpm install` in that directory.

## Client contract

Both clients accept the same arguments and produce the same output so the
harness can compare them directly.

```text
<client> --endpoint http://127.0.0.1:<port> \
  --page-url <url> [--page-match exact|origin-path] [--context-index N] \
  [--frame-selector <css>]... --selector <css> \
  [--timeout-ms N] [--delay-before-insert-ms N]
```

- The value is read from stdin as UTF-8, exactly as given.
- `--page-match exact` (default) compares the whole URL. `origin-path`
  compares scheme, host, port, and path only.
- `--context-index` selects a browser context: 0 is the default context;
  created contexts follow in `Target.getBrowserContexts` order. Omitted means
  all contexts, and the page must still be unique.
- `--frame-selector` values form a strict chain of iframe elements from the
  main frame. Each must match exactly one iframe element in its parent frame.
- `--selector` must match exactly one element in the resolved frame. Open
  shadow roots are pierced; closed ones are not. The element must be visible,
  enabled, and editable under Playwright's definitions, and must itself be a
  fillable control: an `input` of type text, password, email, search, tel,
  url, number, or empty; a `textarea`; or a contenteditable element. Labels
  are not retargeted and date-like inputs are not set directly.
- `--delay-before-insert-ms` is a lab-only hook. It sleeps after the target
  has been prepared and before revalidation and insertion, so the harness can
  create navigation, detachment, ambiguity, and timeout windows. The delay
  is bounded by the deadline.
- Order of operations: connect, check versions, select page, resolve frame
  chain, find and check the element, optional delay, check the deadline,
  revalidate page, frames, and element (including match count), focus and
  select existing content, insert the value once, detach.

Output is one JSON line on stdout:

```json
{"version":1,"implementation":"native","outcome":"filled","reason":null,"message":"","details":{},"versions":{"browser":"...","protocol":"...","library":"..."}}
```

`outcome` is `filled` or `error`. `reason` is one of the codes listed in
`scenarios.json` under `reasons`. `details` never contains the value.
Exit status is 0 for `filled`, 8 for `error`, and 1 for a usage error.

## Scenario catalog

Each scenario lists the pages the host session opens (`open`, with `ref`,
`origin`, `page`, optional `query`, and `context` index), the client request
with `{A}`/`{B}` origin placeholders, the value key, and the expectation:
either `filled` with the receipt the fixture must record (page `ref`, frame,
field, and value key) or `error` with a reason code and no receipt at all.
`filledEvents` is the trusted event sequence every filled field must record.

## Harness

`python3 tests/cdp_lab/run.py --output-dir <dir>` launches a Chromium-family
browser with a temporary profile and remote debugging, starts the fixture
server, and runs every scenario through both clients from a separate host
session. It writes `evidence.json` and `summary.md` to the output directory.
See `run.py --help` for browser selection.

`--implementation native|reference|product|both|all` selects the clients.
`product` builds `agentenv` with `--features test-keychain`, writes a
configuration file whose `lab_value` env credential reads
`AGENTENV_LAB_VALUE`, and runs `agentenv --json credential fill lab_value
--backend cdp ...` for each scenario with the value only in that variable.
The product's success JSON and `credential-fill: <reason>: <message>` stderr
line are mapped onto the shared outcome schema; error scenarios must exit 8.
Race-window scenarios set the debug-only `AGENTENV_FILL_TEST_DELAY_MS`
variable instead of the prototype's `--delay-before-insert-ms` option.

A scenario may name `implementations` to restrict which clients run it; the
others are recorded as skipped and parity is compared only among those that
ran. `hostSetup` runs a host-side step after the pages open: `await-workers`
waits for the workers fixture's dedicated and service workers, and
`open-devtools` opens DevTools on the main page through `Target.openDevTools`
and closes it again afterwards.
