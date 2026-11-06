# Credential filling for browsers and desktop applications

## Outcome and scope

An agent supplies a credential name and a destination. Local execution
resolves the credential and fills the destination without putting the secret
value in the agent's tool arguments, results, diagnostics, or generated
artifacts. The agent continues using its existing browser or desktop automation
tools to navigate and perform the surrounding task.

`agentenv credential fill` defines three backend families with the following
implementation scope:

| Backend | Platform scope | Status and runtime behavior |
| --- | --- | --- |
| `cdp` | macOS, Linux, Windows | Implemented (`src/fill/cdp/`). Reads `/json/version` from a loopback `http://` remote debugging endpoint, connects in-process to the browser WebSocket it names, and requires CDP protocol version 1.3; replaces the target field (`effect: "field-filled"`). See [ADR 0001](adr/0001-native-rust-cdp-client.md). |
| `desktop` (Windows) | Windows | Implemented (`src/fill/desktop.rs`, `#[cfg(windows)]`). Verifies the foreground PID and focused writable UI Automation `Edit` control, then sends zeroized Unicode key batches through Win32 `SendInput` (`effect: "input-sent"`). See [Windows port](windows-port.md). |
| `desktop` (macOS, Linux) and native control setters | macOS, Linux X11/Wayland | Designed, not implemented. On non-Windows builds, `credential fill --backend desktop` validates arguments and fails before credential lookup with exit code 8 (`credential-fill: backend-unavailable: the desktop backend is not available in this build`). |
| `playwright` | macOS, Linux, Windows | Designed, not implemented. `credential fill --backend playwright` validates arguments (including `--browser <chromium|firefox|webkit>`) and fails before credential lookup with exit code 8 (`credential-fill: backend-unavailable: the playwright backend is not available in this build; run 'agentenv credential fill --capabilities' to see what is`). |

The CDP and Playwright transports serve different browser hosts and do not
overlap: a host offers either a Chromium CDP endpoint or a shared Playwright
endpoint, and the caller names `--backend` explicitly.

This mechanism provides normal-workflow confidentiality. Deliberate reads by
the agent, reads through other applications, and reads of the destination after
filling are outside the protection boundary. Agent instructions prohibit those
reads; the implementation does not sandbox the agent or destination. In
particular, filling a visible API-key input field does not mask it in subsequent
screenshots taken by another tool.

The filling operation returns no screenshot, DOM snapshot, control value, page
content, clipboard content, or automatic readback verification. Browser
extensions, Native Messaging, password-manager integrations, an MCP server,
automatic form submission, browser launching, and session-cookie export are
outside scope.

## Codebase integration and boundaries

- `src/credential/mod.rs` owns provider selection and resolution for `env`,
  `keychain`, and `command` credentials, exposing `Provider::resolve` for
  ordinary environment injection and `Provider::resolve_confidential(line_oriented: bool)`
  with `FILL_VALUE_LIMIT` (`8,192` bytes) for confidential callers.
- `src/credential/secret.rs` keeps secret bytes in zeroizing storage
  (`Zeroizing<String>` and `Zeroizing<Vec<u8>>`), omits `Display` and
  `Serialize`, redacts `Debug`, and validates fill values through
  `Secret::validate_fill`.
- `src/credential/command.rs` captures confidential command-provider output with
  null stdin, discarded stderr, and a fixed zeroized buffer bounded at
  `FILL_VALUE_LIMIT + 2` bytes (`8,194` bytes, allowing a trailing CRLF plus
  1-byte overflow detection). Fill resolution runs with `line_oriented = true`,
  stripping a single trailing LF or CRLF before validation.
- `src/credential/resolver.rs` and `src/credential/resolver/windows.rs` run
  keychain and command resolution in an isolated child process invoked via
  `--agentenv-resolver` (using an unnamed Unix socket pair and process-group
  termination on Unix, or a current-user-only local named pipe and kill-on-close
  Job Object on Windows; see [sudo execution](sudo-execution.md) and
  [Windows port](windows-port.md)). Its `ResolutionStage::Fill` stage:
  - admits only credentials whose `usages` permit `environment` and rejects
    `sudo` and `ssh-password` credentials before provider access;
  - enforces `FILL_VALUE_LIMIT` (`8,192` bytes) and `Secret::validate_fill` in
    both the child and the parent;
  - distinguishes provider failures (`ResolveError::Provider`, exit code 4) from
    unsupported value characters or lengths (`ResolveError::Value`, exit code 8
    with `value-unsupported`) and deadline expiry (`ResolveError::Timeout`,
    exit code 8 with `timeout`).
- `env` credentials resolve in-process inside `src/fill/mod.rs` because reading
  an environment variable does not block, and then pass through the same
  `Secret::validate_fill(FILL_VALUE_LIMIT)` check.
- `src/runner.rs` retains transparent child output forwarding for `agentenv run`;
  filling owns and normalizes all transport output in `src/fill/`.
- `src/cli/fill.rs` and `src/error.rs` own CLI argument validation and exit-code
  mapping (`AppError::Fill` -> exit code 8, `AppError::FillUncertain` -> exit
  code 11).

No credential-store format, reference syntax, or `.agentenv.toml` trust rule
changes for filling. A positional `<NAME>` selects an existing credential
definition in `config.toml`, as in `credential check`; `inject_as` is ignored
during direct filling.

## Architecture decision

Three approaches were evaluated for the filling architecture:

| Approach | Benefit | Cost / limitation |
| --- | --- | --- |
| `run` plus arbitrary user scripts | Smallest implementation; reuses environment injection | Every caller script must handle output suppression, targeting, and error classification; no uniform contract |
| Unified `credential fill` command with owned backend adapters | Centralizes secret resolution, deadline and cancellation handling, and safe diagnostics | Requires transport and platform adapters |
| Persistent credential broker integrated into each automation host | Can reuse host-owned page and control handles directly | Requires custom host integration and daemon lifecycle management |

`agentenv` uses the unified `credential fill` command with owned backend
adapters (`src/fill/mod.rs`). A single coordinator owns preflight, deadline
tracking, signal registration, confidential credential resolution, the mutation
gate (`Delivery::begin_mutation`), cleanup grace enforcement, and structured
results.

### CDP transport: native Rust client

Two implementations were evaluated for the CDP backend:

| Implementation | Benefit | Cost / limitation |
| --- | --- | --- |
| Native Rust CDP client (`src/fill/cdp/`) | No Node.js runtime, no companion JS bundle, no second process handling the secret | Must implement Playwright-equivalent page/frame targeting, visibility/editability checks, and input event dispatch, including out-of-process iframes |
| Node.js helper using `playwright-core` `chromium.connectOverCDP()` | Reuses Playwright's existing client implementation | Adds a Node.js runtime dependency, helper asset distribution, and a second process boundary for the secret |

Filling one element requires a bounded set of CDP operations: target discovery
and attachment, document and frame resolution, one CSS selector query per
frame, fixed read-only checks for visibility and editability, focus, selection
of existing content, text insertion (`Input.insertText`), and closing the
connection.

The native Rust client in `src/fill/cdp/` implements this contract directly.
[ADR 0001](adr/0001-native-rust-cdp-client.md) records the comparison that
selected it: a `playwright-core` 1.63.0 reference client and the native
client on Microsoft Edge 153 across 45 scenarios. The lab in `tests/cdp_lab/`
runs its current catalog of 47 scenarios against the product and the
reference client. The contract covers:

- Exact or `origin-path` page selection across attached targets, including
  pages in non-default browser contexts (`--context-index`), failing on zero or
  multiple matches.
- Strict iframe chain resolution (`--frame-selector`) across same-process and
  out-of-process iframes (attached with flattened target sessions), with no
  implicit frame descent.
- Single-element matching (`--selector`, piercing open shadow roots) using
  the same visibility and editability definitions Playwright applies to
  `Locator.fill()`. An element is visible when it passes `checkVisibility`,
  has a computed `visibility` of `visible`, and has a non-empty bounding box;
  an element styled `display: contents` has no box of its own and is visible
  instead when one of its element children is visible or one of its text
  children has a non-empty rendered range.
- Replacement semantics matching Playwright `fill`: focus, select existing
  content, and insert text through `Input.insertText` so reactive frameworks
  (such as React controlled inputs) observe standard `input` and `change`
  events on `<input>`, `<textarea>`, and `contenteditable` targets.
- Target revalidation after secret resolution: the prepared element must
  still be connected, the page, frame chain, and element selection are
  repeated, and the result must be the same page target, the same frame
  sessions and frame IDs, and the same DOM node, so page replacement, frame
  replacement, or DOM replacement fails before mutation without retargeting.
- Cleanup by closing the WebSocket connection, which detaches every session
  the client attached without closing any target and leaves the host's
  existing CDP or Playwright sessions and pages intact.

All page inspection uses fixed, read-only JavaScript expressions owned by
`src/fill/cdp/scripts.rs`; caller-supplied JavaScript is never executed. Full
protocol details, lab evidence, and residual constraints are recorded in
[ADR 0001](adr/0001-native-rust-cdp-client.md).

### Playwright transport (designed, not implemented)

Playwright protocol connections require the Playwright client library;
reimplementing the custom Playwright wire protocol in Rust is out of scope.
The designed Playwright backend uses an owned Node.js helper with
`playwright-core` and `browserType.connect()` against a host-prepared shared
endpoint. Until that helper and shared-endpoint integration are implemented,
`--backend playwright` validates its CLI arguments and returns exit code 8
(`backend-unavailable`).

### Desktop transport

Desktop input runs in-process in Rust. On Windows (`src/fill/desktop.rs`), the
adapter verifies the foreground window PID (`GetForegroundWindow` and
`GetWindowThreadProcessId`), thread focus state (`GetGUIThreadInfo`), and
active modifier keys (`GetAsyncKeyState`), queries UI Automation (`IUIAutomation`)
on a dedicated metadata-only MTA worker thread to verify that the focused
element is an enabled, writable `UIA_EditControlTypeId` (`IUIAutomationValuePattern`),
revalidates element identity (`CompareElements`) after credential resolution,
and delivers UTF-16 keystrokes in a single synchronous Win32 `SendInput`
(`KEYEVENTF_UNICODE`) batch whose buffer is zeroized on drop. Desktop support on
macOS and Linux and direct native control-value setters remain unimplemented
design scope.

## CLI and result contract

```text
agentenv credential fill portal_password --backend cdp \
  --endpoint http://127.0.0.1:9222 \
  --page-url https://portal.example/login --selector '#password' --json

agentenv credential fill portal_password --backend cdp \
  --endpoint http://127.0.0.1:9222 \
  --page-url https://portal.example/login --page-match origin-path \
  --selector '#password' --json

agentenv credential fill portal_password --backend playwright \
  --endpoint ws://127.0.0.1:3000/session --browser chromium \
  --page-url https://portal.example/login --selector '#password' --json

agentenv credential fill portal_password --backend desktop \
  --expect-pid 12345 --json

agentenv credential fill --capabilities --json
```

- A fill invocation requires `<NAME>` and `--backend <cdp|playwright|desktop>`
  (unless `--capabilities` is passed). There is no inline secret value,
  print-value mode, or caller-supplied script option.
- Only credentials permitting `environment` usage can be filled (`fill::check_fillable`).
  Credentials configured for `sudo` or `ssh-password` are rejected as a
  configuration error (exit code 2) before any transport setup or secret
  resolution.
- `--capabilities` conflicts with `<NAME>` and all target/backend flags. It
  resolves no credentials and prints the compiled and runtime state of the
  confidential resolver and each backend (`cdp`, `playwright`, `desktop`). On
  Windows, `backends.desktop.available` is `null` in JSON (`compiled,
  target/permission-dependent` in text) because UI Automation focus and
  UIPI integrity permissions depend on the target window at execution time. On
  non-Windows builds, `desktop` reports `compiled: false, available: false`.
  `playwright` reports `compiled: false, available: false` on all platforms.
- `--endpoint <URL>`, `--page-url <URL>`, `--page-match <exact|origin-path>`,
  `--context-index <N>`, repeatable `--frame-selector <CSS>`, and
  `--selector <CSS>` apply to browser backends (`cdp` and `playwright`) and are
  rejected as usage errors (exit code 1) with `--backend desktop`.
- For `cdp`, `--endpoint` must be the browser's remote debugging HTTP
  endpoint: `http://<host>:<port>` with an explicit port, where `<host>` is
  `127.0.0.1`, `[::1]`, or `localhost` (which must resolve to loopback
  addresses); any path is ignored. `https://`, `ws://`, `wss://`, a missing
  port, a non-loopback host, or embedded userinfo fails with
  `connect-failed`. The client reads `/json/version` from that endpoint and
  connects to the `webSocketDebuggerUrl` it names, which must be a plain
  `ws://` URL on the same loopback host.
- `--browser <chromium|firefox|webkit>` is required for `--backend playwright`
  and rejected for `cdp` and `desktop`.
- `--page-url <URL>` must contain a scheme (`://`). `--page-match` defaults to
  `exact`; `origin-path` compares scheme, host, port, and path while ignoring
  query string and fragment so login pages with volatile state or OAuth nonce
  parameters still match unambiguously. Both modes require exactly one matching
  page; zero (`page-absent`) or multiple (`page-ambiguous`) matches fail.
  Optional `--context-index <N>` restricts matching to the `N`th browser
  context (`0` is the default browser context).
- Repeatable `--frame-selector <CSS>` specifies an explicit chain of `<iframe>`
  or `<frame>` elements from the main document; omitting it targets the main
  frame. `--selector <CSS>` must match a single visible, enabled, editable
  element in the resolved frame.
- `--expect-pid <PID>` (`1..=2147483647`) is required for `--backend desktop`
  and rejected for `cdp` and `playwright`. The caller must focus an empty text
  field in that process before invoking `credential fill`. The desktop adapter
  verifies both the foreground PID and that the focused accessibility element
  is a writable text-input control before typing; it never clicks, switches
  windows, clears existing text, or presses Enter.
- `--timeout-ms <MS>` sets a single monotonic deadline covering preflight,
  transport connection, target inspection, confidential credential resolution,
  revalidation, and delivery. The default is `30000` (30 seconds) and the valid
  range is `1..=300000`. Cleanup after cancellation or timeout has a separate
  2-second grace window (`CLEANUP_GRACE = 2s`) that cannot resume delivery.

### Fill value contract

Fill resolution accepts single-line UTF-8 text of 1 to 8,192 bytes
(`FILL_VALUE_LIMIT = 8192`). Spaces, punctuation, and printable Unicode scalar
values are preserved literally. `Secret::validate_fill` rejects, before any
mutation:

- all Unicode control characters (general category `Cc`, `U+0000`..`U+001F` and
  `U+007F`..`U+009F`), including CR, LF, TAB, and ESC;
- line and paragraph separators `U+2028` and `U+2029`;
- bidirectional override and isolate controls `U+202A`..`U+202E` and
  `U+2066`..`U+2069`.

For command providers, confidential capture strips at most one trailing LF or
CRLF before validation (`line_oriented = true`). Empty output, NUL bytes, or
invalid UTF-8 from a provider fail with exit code 4 (credential error); values
that are valid UTF-8 but exceed 8,192 bytes or contain forbidden characters for
fill fail with exit code 8 (`credential-fill: value-unsupported: ...`).

### Results, exit codes, and reason codes

On success, `--json` prints a single object containing `version`, `backend`,
and `effect`:

```json
{"version":1,"backend":"cdp","effect":"field-filled"}
```

Without `--json`, stdout prints:

```text
Credential '<name>' delivered through the <backend> backend (<effect>).
```

`cdp` reports `effect: "field-filled"`; `desktop` focused keystroke delivery
reports `effect: "input-sent"`. Neither effect claims readback verification,
form submission, or login success.

Existing CLI exit codes `1` (usage), `2` (configuration, including rejecting a
`sudo` or `ssh-password` credential), `3` (unknown credential), and `4`
(credential provider failure) are preserved. Fill-specific failures use two
exit codes partitioned by the coordinator's mutation gate
(`Delivery::begin_mutation`):

| Exit code | Meaning |
| --- | --- |
| `8` (`AppError::Fill`) | Fill failed before the secret value was sent across the mutation gate: unavailable backend, permission denial, connection or protocol version failure, open DevTools recording conflict, absent/ambiguous/non-editable/changed target, unsupported fill value, or timeout/cancellation (`SIGINT`, `SIGTERM`, `SIGHUP`, `Ctrl+C`) before `begin_mutation`. Focusing the target and selecting existing content in CDP happen before the gate and do not count as secret delivery. |
| `11` (`AppError::FillUncertain`) | The secret value may have reached the destination, or post-delivery cleanup could not be confirmed: any delivery error, transport drop, timeout, or signal cancellation after `begin_mutation`, or unconfirmed backend cleanup (`cleanup-unconfirmed`) after a delivered value. |

Every `AppError::Fill` and `AppError::FillUncertain` diagnostic writes a single
line to stderr in the format `credential-fill: <reason>: <message>` and leaves
stdout empty when `--json` is set. The fixed reason codes in `src/fill/mod.rs`
are:

| Reason code | Condition |
| --- | --- |
| `backend-unavailable` | Backend is not compiled or not implemented in this build (`playwright`, or `desktop` on non-Windows) |
| `permission` | The desktop target could not be inspected: no foreground window, or a UI Automation or thread-focus query failed |
| `connect-failed` | Endpoint that is not a loopback `http://` URL with a port, `/json/version` discovery failure or unusable WebSocket URL, WebSocket handshake failure, or a dropped or failing browser connection |
| `version-unsupported` | `Browser.getVersion` reports a `protocolVersion` other than `1.3` |
| `recording-conflict` | `Target.getDevToolsTarget` reports a DevTools window for the target page, or, on a browser without that method, any `devtools://` target is open anywhere in the browser |
| `context-absent` | Requested `--context-index` does not exist on the browser connection |
| `page-absent` | Zero open page targets match `--page-url` and `--page-match` |
| `page-ambiguous` | Multiple open page targets match `--page-url` and `--page-match` |
| `frame-absent` | A `--frame-selector` step matched zero elements in the parent frame |
| `frame-ambiguous` | A `--frame-selector` step matched multiple elements in the parent frame |
| `frame-invalid` | A `--frame-selector` step matched an element that is not an `<iframe>` or `<frame>` with a navigable content document |
| `target-absent` | `--selector` matched zero elements |
| `target-ambiguous` | `--selector` matched multiple elements |
| `target-hidden` | A target element styled `display: contents` has no visible element child and no text child with a non-empty rendered range; any other target element fails `checkVisibility`, has a computed `visibility` other than `visible`, or has an empty bounding box; or the desktop control is offscreen |
| `target-disabled` | Target element or desktop control is disabled |
| `target-readonly` | Target element or desktop `ValuePattern` is read-only |
| `target-unfillable` | Target element is not a fillable `<input>`, `<textarea>`, or `contenteditable` element; or the desktop window has no focused control, a modifier key is held, or focus is not a `UIA_EditControlTypeId` control supporting `IUIAutomationValuePattern` |
| `target-changed` | The foreground PID differs from `--expect-pid`, or between preparation and delivery the desktop window, focused control, or UI Automation element changed; in a browser, the prepared element was disconnected, a different page now matches, the frame chain's sessions or frame IDs changed, the reselected element is a different node, or the element could not take focus. A page, frame, or element that is now absent or ambiguous reports its own reason instead |
| `value-unsupported` | Resolved secret exceeds 8,192 bytes, contains control/separator/bidi characters, or contains unpaired surrogates |
| `timeout` | Operation deadline expired (`8` before `begin_mutation`, `11` after) |
| `cancelled` | Termination signal received (`8` before `begin_mutation`, `11` after) |
| `delivery-failed` | `Input.insertText` or `SendInput` failed or sent fewer events than expected after `begin_mutation` (exit code `11`) |
| `cleanup-unconfirmed` | After a delivered value, the backend's release step (closing the CDP connection, or stopping the Windows UI Automation worker) failed or did not finish within the 2-second cleanup grace window (exit code `11`). When the operation had already failed, the cleanup problem is appended to that failure's diagnostic, which keeps its own reason and exit code |

## Browser execution

The browser coordinator sequence in `src/fill/mod.rs` and `src/fill/cdp/mod.rs`
runs in six steps. Steps 1 to 5 share a single monotonic deadline, and step 6
has its own cleanup grace period:

1. **Preflight and deadline setup:** Validate CLI arguments, verify that the
   credential definition permits `environment` usage (`fill::check_fillable`),
   and start the monotonic timer (`--timeout-ms`, default 30s).
2. **Connect and verify protocol:** Read `/json/version` from the loopback
   `http://` endpoint, open the `ws://` browser WebSocket it names on the same
   loopback host, query `Browser.getVersion`, and require `protocolVersion`
   `1.3`. The browser product string is not checked; the Chromium-family scope
   is the tested scope recorded below.
3. **Target and recording preflight (before secret lookup):** Enumerate targets
   via `Target.getTargets` and `Target.getBrowserContexts`, and select the
   unique page matching `--page-url`, `--page-match`, and optional
   `--context-index`. Ask `Target.getDevToolsTarget` whether a DevTools window
   inspects that page and fail with `recording-conflict` if one does. When the
   browser does not implement that method, the inspected page cannot be
   determined, so any open `devtools://` target anywhere in the browser fails
   with `recording-conflict`. Attach a flattened page session
   (`Target.attachToTarget`), enable `Target.setAutoAttach` for out-of-process
   iframes and `Emulation.setFocusEmulationEnabled` on the page session and on
   each attached iframe session, walk `--frame-selector`, and verify that
   `--selector` resolves to a single visible, enabled, editable target. Keep
   the page target ID, the session and frame ID of each frame in the chain,
   and the element's `Runtime` object ID.
4. **Confidential credential resolution:** Resolve the credential within the
   remaining operation budget (`env` in-process, `keychain` and `command` via
   the cancellable `--agentenv-resolver` subprocess in `ResolutionStage::Fill`)
   and validate it with `Secret::validate_fill(8192)`.
5. **Revalidate and deliver:** Verify that no cancellation or timeout occurred
   and repeat the DevTools check. Confirm that the prepared element is still
   connected, then repeat page selection, frame-chain resolution, and element
   selection: the page must be the same target, every frame in the chain must
   have the same session and frame ID, and the reselected element must be the
   same node as the prepared one (`target-changed` otherwise). A page, frame,
   or element that is now absent, ambiguous, hidden, disabled, read-only, or
   unfillable fails with that condition's own reason. Frame URLs are not
   compared, so a same-document URL change inside an unchanged frame is
   accepted. Select the element's existing contents and focus it, still before
   the mutation gate, then call `delivery.begin_mutation()` to obtain the
   secret slice and immediately issue the single `Input.insertText` command on
   the page session.
6. **Clean up:** Close the WebSocket within the 2-second cleanup grace period.
   Closing the browser connection detaches every session this client attached
   without closing any target, leaving the browser and the caller's own
   automation session intact. No other cleanup command is sent.

Only an open DevTools window can be detected read-only by a second CDP client
(`Target.getDevToolsTarget` and `devtools://` targets). Chromium tracing,
screencast, OS screen recording, screenshots taken by another client, and
Playwright tracing/HAR/video expose no read-only probe (probing `Tracing.start`
is excluded because it would start a trace). Callers must keep external trace,
HAR, video, and screenshot recorders disabled around `credential fill`.

### CDP attachment details

Tested baseline ([ADR 0001](adr/0001-native-rust-cdp-client.md)): Microsoft
Edge `Edg/153.0.4234.48` (Chromium 153), CDP protocol `1.3`, with a concurrent
`playwright-core` 1.63.0 session driving the same page before, during, and
after each fill. The client uses `Browser.getVersion`, `Target.getTargets`,
`Target.getBrowserContexts`, `Target.getDevToolsTarget`,
`Target.attachToTarget`, `Target.setAutoAttach`, `Page.getFrameTree`,
`Page.createIsolatedWorld`, `Runtime.callFunctionOn` (with fixed scripts in
`src/fill/cdp/scripts.rs`), `DOM.describeNode`,
`Emulation.setFocusEmulationEnabled`, and `Input.insertText`, and it never
creates or closes a target, context, or page. Note that Chrome and Chromium 136+ require a
non-default `--user-data-dir` when launching with `--remote-debugging-port`.

### Shared Playwright endpoint design (unimplemented)

An ordinary `browserType.launchServer()` endpoint isolates browser contexts per
client connection, so a helper connecting to it cannot see pages opened by the
host's client.
[Playwright context isolation](https://github.com/microsoft/playwright/blob/main/packages/playwright-core/src/server/dispatchers/playwrightDispatcher.ts)

The designed Playwright integration targets a host-owned shared endpoint
exposed through Playwright's public `browser.bind()` API (introduced in
Playwright 1.59) over a loopback WebSocket and connected via
`browserType.connect()`.
[Browser binding API](https://playwright.dev/docs/api/class-browser#browser-bind),
[Playwright connection documentation](https://playwright.dev/docs/api/class-browsertype#browser-type-connect)
When implemented, the Playwright adapter will run in a version-checked Node.js
companion under `adapters/browser/` using framed stdio pipes, a scrubbed
environment (`NODE_OPTIONS`, `DEBUG`, `PWDEBUG`, and `PLAYWRIGHT_*` removed),
and the same 6-step preflight, resolver, revalidation, and mutation-gate
contract as CDP.

## Desktop adapters

| Adapter | Status | Target verification and delivery mechanism |
| --- | --- | --- |
| Windows focused input (`src/fill/desktop.rs`) | Implemented (`#[cfg(windows)]`) | Verifies foreground window PID (`GetForegroundWindow`, `GetWindowThreadProcessId`), active thread focus (`GetGUIThreadInfo`), and no pressed modifier keys (`VK_SHIFT`, `VK_CONTROL`, `VK_MENU`, `VK_LWIN`, `VK_RWIN`). Queries UI Automation (`CUIAutomation` on a dedicated metadata-only MTA worker thread `agentenv-uia-metadata`) to confirm the focused element is an enabled `UIA_EditControlTypeId` (`30004`) with a writable `IUIAutomationValuePattern` (`CurrentIsReadOnly == FALSE`) without calling `CurrentValue`. After confidential secret resolution, re-verifies foreground window, focused thread/HWND, modifier state, and UIA runtime element identity (`CompareElements`), crosses `Delivery::begin_mutation`, and sends a single synchronous `SendInput` batch of `KEYEVENTF_UNICODE` key-down/key-up pairs stored in a volatile-zeroized `InputBatch`. |
| macOS focused input | Designed, not implemented | Expected foreground PID and focused text/secure-text field verified through macOS Accessibility (`AXUIElement`), followed by Unicode text insertion; returns `backend-unavailable` on macOS builds. |
| Linux X11 / Wayland focused input | Designed, not implemented | Expected foreground PID and toolkit/AT-SPI focus verification (or Wayland `org.freedesktop.portal.RemoteDesktop` + `libei` session); returns `backend-unavailable` on Linux builds. |
| Native control setters (`--target-file`, `--method control`) | Designed, not implemented | Explicit accessible control targeting via macOS `AXValue`, Windows `IUIAutomationValuePattern::SetValue`, or Linux AT-SPI `EditableText` without keyboard simulation; not part of the current CLI. |

On Windows (`src/fill/desktop.rs`), UI Automation calls can block when the
target window stopped pumping messages or belongs to a higher-integrity (UIPI)
process. Running COM/UIA inspection on an isolated MTA worker thread that
receives only target metadata and timeouts—never the resolved `Secret`—keeps the
monotonic operation deadline enforceable before the mutation gate. If
`SendInput` returns `0` or fewer events than the UTF-16 batch length (for
example, when blocked by UIPI against an elevated target window), the adapter
probes `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` to include an elevation
hint when `ERROR_ACCESS_DENIED` is present and returns `delivery-failed` after
the mutation gate (exit code 11).

Target checks prevent ordinary focus mistakes (such as typing into a browser
address bar or terminal when a login box lost focus); they are not an atomic OS
lock against a concurrent focus-stealing application.

## Credential resolution and diagnostic ownership

`credential fill` shares the confidential subprocess resolver
(`src/credential/resolver.rs`, `src/credential/resolver/windows.rs`) with
`sudo` execution using `ResolutionStage::Fill`:

- Only credentials whose `usages` include `environment` are admitted; `sudo`
  and `ssh-password` credentials fail preflight with exit code 2.
- `keychain` and `command` credentials resolve inside the isolated
  `--agentenv-resolver` child process under the remaining `--timeout-ms`
  budget; `env` credentials resolve in-process.
- On timeout or signal cancellation (`SIGINT`, `SIGTERM`, `SIGHUP`, `Ctrl+C`),
  the coordinator drops the operation, which kills the resolver child and
  waits for it (process-group `SIGKILL` on Unix; closing the
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` Job Object and killing the child on
  Windows) before the backend's cleanup grace period starts. Killing the
  resolver does not necessarily dismiss an operating-system dialog that the
  provider opened, such as a keychain access prompt; such dialogs follow their
  own lifecycle.
- Any resolver reply arriving after deadline expiry or cancellation is dropped
  without crossing `Delivery::begin_mutation`.
- All error paths construct fixed reason codes and sanitized messages; secret
  buffers (`Secret`, `CapturedSecret`, and Windows `InputBatch`) zeroize on
  drop and never implement `Display` or `Serialize`.
- Zeroization covers only buffers that `agentenv` owns. It is not secure
  erasure of every copy: the CDP client builds the `Input.insertText` command
  as ordinary JSON values and a serialized string, and the browser's
  JavaScript heap, the destination application, operating-system input
  queues, and the credential provider hold copies outside `agentenv`'s
  control.

## Verification and test suites

Verification uses test-owned credentials and local browser or desktop fixture
targets. Production code never reads back a filled value; test fixtures read
their own DOM or control state to assert exact delivery.

- **Resolver and CLI integration tests (`tests/fill_resolver.rs`, `tests/fill_cli.rs`, `src/fill/mod.rs`):**
  Verify the `ResolutionStage::Fill` usage gate (rejecting `sudo` and
  `ssh-password` credentials with exit code 2), the 8,192-byte fill limit,
  control/separator/bidi character rejection (`value-unsupported`),
  line-oriented command provider capture, deadline expiry and `SIGTERM`
  cancellation before and after `Delivery::begin_mutation` (exit codes 8 vs.
  11), resolver child reaping, `--capabilities` output, and sentinel leak
  checks across stdout and stderr.
- **CDP browser parity and product lab (`tests/cdp_lab/`):**
  `python3 tests/cdp_lab/run.py --implementation both --output-dir
  <evidence-directory>` runs the 47 scenarios of `tests/cdp_lab/scenarios.json`
  against a real Chromium-family browser, comparing `agentenv credential fill
  --backend cdp` with the `playwright-core` reference client, and writes
  `evidence.json` and `summary.md` to the output directory.
  `--implementation` also accepts `reference` or `product`, and repeatable
  `--scenario <id>` selects individual scenarios. The catalog covers exact and
  `origin-path` page matching, `--context-index`, nested same-process and
  out-of-process iframes, open and closed shadow roots, React controlled
  inputs, `contenteditable`, hidden/disabled/readonly/unfillable targets, a
  page with a worker target, and open DevTools refusal
  (`recording-conflict`). It also covers navigation, frame detachment, a newly
  ambiguous selector, and deadline expiry between preparation and insertion.
  The browser on which the comparison was recorded is listed in
  [ADR 0001](adr/0001-native-rust-cdp-client.md).
- **Windows confidential resolver (`tests/windows_credentials.rs`):**
  `cargo test --features test-support` on `windows-latest` verifies native
  Windows Credential Manager and command resolution over the named-pipe / Job
  Object resolver, stage-specific limits, cancellation of provider
  descendants, and resolver timeouts.
- **Windows desktop UIA/SendInput lab (`tests/windows_lab/desktop.ps1`):**
  `pwsh -NoProfile -STA -File tests/windows_lab/desktop.ps1`, run in
  `.github/workflows/ci.yml` on `windows-latest`, opens a disposable WinForms
  fixture window and runs six cases: delivery of a value with leading and
  trailing spaces, quotes, a backslash, CJK text, and an astral-plane
  character into a focused `TextBox`; the same delivery into a password-style
  `TextBox`; and four refusals that must exit with code 8, leave every fixture
  input unchanged, and print nothing on stdout: a read-only `TextBox`, focus
  on a button, a wrong `--expect-pid`, and focus moving to another `TextBox`
  while a command provider is resolving. Every case also checks that the
  synthetic value never appears in CLI output. Disabled controls and
  unresponsive windows are not exercised by this lab
  ([windows-port.md](windows-port.md)).

## Open design questions for unimplemented backends

1. **Playwright shared-endpoint lifecycle and recording signals:** Before
   enabling `--backend playwright`, verify `browser.bind()` +
   `browserType.connect()` across Chromium, Firefox, and WebKit with two
   independent clients (host opens the page, helper attaches and fills without
   creating a context, and the host session remains usable after helper
   disconnect, timeout, and error), and determine which recording states can be
   detected read-only on a shared Playwright connection.
2. **Playwright companion packaging:** Finalize the compiled asset layout under
   `adapters/browser/`, supported Node.js version range, and atomic
   install/rollback integration with `src/update/`.
3. **macOS and Linux X11 desktop focus metadata:** Determine which
   accessibility roles (`AXTextField`, `AXSecureTextField`, `AXTextArea` on
   macOS; AT-SPI / X11 toolkit focus reporting on Linux) can be verified
   reliably before sending keystrokes without reading the control's current
   value.
4. **Wayland target identification:** Evaluate `org.freedesktop.portal.RemoteDesktop`
   + `libei` alongside AT-SPI on GNOME and KDE to determine whether the
   foreground PID and focused text control can be verified before input; keep
   Wayland desktop input disabled where target identity cannot be established.

## Native API references

- [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/)
- [macOS AX attribute setting](https://developer.apple.com/documentation/applicationservices/1460434-axuielementsetattributevalue)
- [Windows UI Automation SetValue](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationvaluepattern-setvalue)
- [Windows SendInput and integrity restrictions](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)
- [Linux AT-SPI EditableText](https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/iface.EditableText.html)
- [Chrome remote-debugging profile constraints](https://developer.chrome.com/blog/remote-debugging-port)
