# Credential filling for browsers and desktop applications

Status: CDP browser filling is implemented. Phase A (CDP feasibility) is
recorded in [ADR 0001](adr/0001-native-rust-cdp-client.md); Phases B and C
ship `agentenv credential fill --backend cdp` with the native Rust client.
Desktop and Playwright filling (Phases D–G) are not implemented.

## Outcome and scope

An agent supplies a credential name and a destination. Local execution
resolves the credential and fills the destination without putting the value
in the agent's tool arguments, results, diagnostics, or generated artifacts.
The agent continues using its existing browser or desktop automation tools
to navigate and perform the surrounding task.

The first browser release supports Chrome DevTools Protocol (CDP) connections
to an existing Chromium-family browser. Playwright protocol connections are a
separate, later milestone for automation hosts that own a Playwright session
and expose it as a shared endpoint; that path also covers Firefox and WebKit,
which CDP cannot reach. The two transports serve different hosts and do not
overlap: a host offers a CDP endpoint or a shared Playwright endpoint, and
the caller names the backend explicitly. Desktop delivery starts with macOS,
Windows, and Linux X11 through Enigo, then adds native control setters and
separately validated Wayland support.

This is normal-workflow confidentiality. Deliberate reads by the agent,
reads through other applications, and reads of the destination after filling
are outside the protection boundary. Agent instructions prohibit those reads;
the implementation does not attempt to sandbox the agent or destination.
In particular, filling an ordinary visible API-key field does not mask it
in subsequent screenshots taken by another tool.

The filling operation itself returns no screenshot, DOM snapshot, control
value, page content, clipboard content, or automatic input-value verification.
Browser extensions, Native Messaging, password-manager integrations, an MCP
server, automatic sign-in/submission, browser launching, and session-cookie
export are outside the initial implementation.

## Existing seams and required changes

The sudo execution work has landed; the seams below describe the current
code rather than the pre-sudo state.

- `src/credential/mod.rs` owns provider selection and resolution for env,
  keychain, and command credentials, and already carries a caller-specific
  `ResolutionIo` policy (`Ordinary` or `Confidential { max_bytes }`). Reuse
  this ownership; do not add a second provider seam.
- `src/credential/secret.rs` keeps values out of Display, Serialize, and
  Debug output and zeroizes owned buffers. Preserve that contract; do not make
  `Secret` generally serializable to implement transport IPC.
- `src/credential/command.rs` already implements the confidential policy:
  null stdin, discarded stderr, and a fixed zeroized capture buffer. That
  buffer is sized for authentication values (255 bytes) and must be
  generalized to the fill value limit below.
- `src/credential/resolver.rs` is the cancellable confidential resolver:
  an owned subprocess in its own process group, a private inherited Unix
  socket, process-group termination on drop, and late-reply disposal. Filling
  reuses it rather than adding a second resolver, but its current contract
  is authentication-specific and needs these changes:
  - It admits only credentials whose `usages` permit sudo or SSH password.
    Filling requires the opposite gate: only credentials permitting
    `environment` usage may be filled, and authentication credentials are
    rejected before resolution. Add a fill resolution stage whose gate is
    `environment` usage; keep the existing stages unchanged.
  - Its byte limit is 1 to 255 and both the client and the served process
    apply authentication validation (no CR or LF, at most 255 bytes).
    Parameterize the stage so the fill stage applies fill validation instead
    (see the value contract below). The wire format's length field already
    allows the larger fill limit.
  - The served process constructs a sudo-usage definition. The fill stage
    must construct an environment-usage definition so provider adapters see
    the correct purpose.
  - Env credentials are rejected by the resolver. The fill coordinator
    resolves env credentials in-process, since reading an environment
    variable cannot block; only keychain and command credentials use the
    subprocess.
  - The resolver is Unix-only. Windows filling of keychain and command
    credentials depends on a Windows resolver, which the sudo plan defers.
    Windows desktop filling is advertised only after that dependency lands,
    or with env credentials only, stated explicitly in capabilities.
- `src/runner.rs` intentionally forwards child output. Leave `run` semantics
  intact; a filling operation owns and normalizes its transport output.
- `src/cli/mod.rs`, `src/cli/credential.rs`, and `src/error.rs` own the CLI
  additions and failure mapping. Codes 1 to 5, 7, 9, 10, and 127 are taken;
  8 is reserved for filling by the sudo design.
- `src/update/` already installs companion executables together with the
  main binary and rolls both back when the main replacement fails. A browser
  helper, when one ships, extends that bundle rather than adding a separate
  installation path.

No credential-store format, reference syntax, or project-file trust change
is needed. A name identifies an existing credential definition, as with
`credential check`. Existing file selection and configuration validation
continue to apply; `inject_as` is irrelevant to direct filling.

## Architecture decision

Three approaches were considered for the overall shape:

| Approach | Benefit | Cost / limitation |
| --- | --- | --- |
| `run` plus arbitrary user scripts | Smallest implementation; already supplies environment injection | Every script must independently handle output, targeting, and failures; no consistent filling contract |
| Unified fill command with owned adapters | Centralizes credential and output handling; retains existing providers | Requires transport adapters and platform dependencies |
| Persistent credential broker integrated into every automation host | Can reuse host-owned page and control handles | Requires host-specific integration and service lifecycle beyond the normal-workflow requirement |

Use the unified command with owned adapters. A fill coordinator owns
preflight, secret resolution, execution, and safe results. Platform APIs and
browser transport details stay behind the filling seam; credential storage
stays behind the existing provider seam.

### CDP transport: native Rust client, adopted only at full parity

Two implementations were considered for the CDP backend:

| Implementation | Benefit | Cost / limitation |
| --- | --- | --- |
| Native Rust CDP client | No Node.js runtime, no helper asset bundle, no helper protocol or version matrix; one process owns the secret | Must reproduce Playwright's targeting and fill semantics, including out-of-process iframes |
| Node.js helper using `playwright-core` `chromium.connectOverCDP()` | Proven targeting and fill semantics | Adds a runtime and distribution dependency and a second process that handles the secret |

Filling one element needs a small, fixed set of CDP operations: target
enumeration and attachment, document and frame resolution, one selector
query per frame, fixed read-only checks for visibility and editability,
focus, selection of existing content, text insertion, and detachment. The
native client is the preferred implementation because it removes the runtime
dependency from the transport most hosts can actually expose.

It is adopted only if phase A demonstrates full parity with the contract in
this document. A partial native implementation does not ship. Parity means
every item below passes against the same fixtures the Node helper passes:

- Exact page selection across all attached targets, including pages in
  additional browser contexts, with zero and multiple matches failing.
- Frame chain resolution for same-process and out-of-process iframes
  (separate CDP targets attached with flattened sessions), with a strict
  chain and no implicit descent.
- Exactly one visible, enabled, editable match, using the same visibility
  and editability definitions Playwright applies to `fill`.
- Replacement semantics equal to Playwright `fill`: focus, select existing
  content, insert text through the input path so reactive frameworks observe
  the same input and change events, for text, password, email, and
  contenteditable targets.
- Revalidation after resolution: navigation, detachment, or a newly
  ambiguous selector fails without retargeting.
- Detachment that leaves the host's sessions, pages, and its own CDP
  connection usable.
- Protocol and browser version checks with fixed safe diagnostics.

The client uses fixed, owned read-only scripts for the visibility and
editability checks. There is no caller-supplied JavaScript. If any parity
item fails and cannot be closed in phase A, the CDP backend ships on the Node
helper described below, and the native client is not a fallback for part of
the contract.

Phase A outcome: the native client is adopted. The lab in `tests/cdp_lab/`
ran 45 scenarios covering every parity item through a native Rust prototype
and a `playwright-core` 1.63.0 reference against Microsoft Edge 153; all 45
matched the catalog and all 45 were identical between the two clients in
outcome, reason, receipts, delivered value, and event sequence. The Node
helper is therefore required only for the Playwright milestone. Evidence,
protocol surface, contract narrowings, and residual risks are recorded in
[ADR 0001](adr/0001-native-rust-cdp-client.md).

### Playwright transport: Node.js helper on a shared endpoint

Playwright protocol connections require the Playwright client library.
Implementing the Playwright wire protocol in Rust is not proposed. The
Playwright backend uses one owned Node.js helper with `playwright-core` and
`browserType.connect()`. This backend is a separate milestone; its helper,
packaging, and shared-endpoint contract are described in later sections and
do not gate the CDP release.

### Desktop transport

Keep desktop input in the Rust process. Start with Enigo's text-input API,
with small native adapters for checking the foreground target and focused
control and later setting accessible controls.

## Proposed CLI and result contract

The CDP form below is implemented; the Playwright and desktop forms are the
planned contract for later phases.

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

- A fill request names exactly one credential and an explicit backend.
  There is no inline-value, print-value, or arbitrary JavaScript option.
- Only credentials permitting `environment` usage can be filled.
  Authentication credentials (sudo, SSH password) are rejected as a
  configuration error before any resolution.
- The capabilities form requires no credential and resolves none. It reports
  compiled backends, helper/runtime availability, resolver availability for
  the platform, and known permission state. Unknown permission or desktop
  capability remains unknown, not available.
- CDP is Chromium-only. Playwright requires an explicit browser family:
  chromium, firefox, or webkit. An incompatible option is a usage error.
  `--endpoint` for Playwright denotes a supported shared-browser endpoint
  prepared by the automation host, not an ordinary isolated `launchServer`
  endpoint.
- Browser target selection uses `--page-url`. The default match is the
  exact URL. `--page-match origin-path` compares scheme, host, port, and
  path and ignores query and fragment, because login and single-sign-on
  pages carry volatile state, nonce, and redirect parameters that can change
  between the agent reading the URL and the fill call. Both modes require
  exactly one matching page; zero or multiple matches fail. Optional
  `--context-index` disambiguates contexts; the index is scoped to the
  current connection, not a durable identity. Never select the first tab
  implicitly.
- A repeatable `--frame-selector` describes a strict chain of iframe
  elements; omission selects the main frame. `--selector` is a CSS selector
  that must identify exactly one visible, enabled, editable target.
- Desktop input requires the expected foreground PID and an already focused,
  empty input field prepared by the caller. On macOS and Windows the adapter
  also requires that the focused element, read through the accessibility API,
  is a text-input control (for example a text or secure text field on macOS,
  an Edit control on Windows). Typing a secret into the wrong control of the
  right process, such as a browser address bar that submits to a search
  engine, is the most likely leak path, so this check is a precondition, not
  an optional refinement: if accessibility access is denied or the focused
  element cannot be read, the operation fails with the permission or
  capability reason instead of proceeding on the PID alone. Linux X11
  applies the check where a toolkit exposes focus metadata and otherwise
  reports the weaker PID-only guarantee in its diagnostics. The adapter does
  not read the value to establish emptiness. It inserts text; it does not
  clear, select all, click, switch applications, press Enter, or submit.
  Capture focus metadata during preflight and recheck it immediately before
  input.
- A future native desktop setter uses `--target-file` with a TOML target
  description and explicit `--method control`. It replaces that control's
  value. This is a later interface, not an initial desktop promise.
- Browser filling replaces the field. Focused desktop insertion returns a
  different effect so callers cannot confuse event delivery with a verified
  field value or successful authentication.
- Use one monotonic operation deadline, with a positive `--timeout-ms`
  override capped at 300,000 milliseconds. The default is 30 seconds. The
  deadline starts before preflight and includes transport startup,
  connection, target checks, credential lookup, permission waits, and input
  delivery. A platform keychain may show an authorization dialog on first
  access and wait for the user; the default leaves room for that click, and
  the skill documents that keychain users who expect dialogs should raise the
  override. Every phase receives the remaining budget; lookup and IPC do not
  reset it. Expiry closes the operation to new secret delivery or mutation.
  Cleanup has a separate maximum two-second grace period and cannot resume
  the operation. Do not retry after mutation may have started; report
  possible partial/completed mutation and any unconfirmed cleanup explicitly.

### Fill value contract

The initial fill feature accepts single-line text of at most 8,192 bytes of
UTF-8. Preserve spaces, punctuation, and supported Unicode exactly. Reject,
before any input, rather than interpret as keys or silently strip:

- every Unicode control character (category Cc), including CR, LF, TAB, and
  escape;
- the line and paragraph separators U+2028 and U+2029;
- the bidirectional overrides and isolates U+202A–U+202E and U+2066–U+2069,
  which can make a filled value render differently from what was stored.

Command providers are line-oriented under fill, as they are under ordinary
environment resolution: exactly one trailing LF or CRLF is removed from the
command's output before validation, and any further line ending is rejected.
The fill capture is confidential and noninteractive like the authentication
stages (no stdin, stderr discarded, bounded stdout), so a provider that
prompts on the terminal fails under fill. Empty output, NUL, or invalid UTF-8
is a provider failure (exit 4) for every provider, as it is for `run`.

This limit replaces the resolver's authentication limit for the fill stage;
API keys, personal access tokens, and signed tokens routinely exceed 255
bytes and are the primary browser filling case. Multi-line credentials
remain supported by existing storage/run behavior but are outside initial
filling support.

### Results and exit codes

Successful JSON contains only a version, backend, and effect, for example:

```json
{"version":1,"backend":"cdp","effect":"field-filled"}
```

The desktop input effect is `input-sent`; a native setter returns
`field-filled` after its API reports success. Neither effect claims readback
verification, persistence, submission, or login success.

Preserve existing exit codes 1 (usage), 2 (configuration), 3 (unknown
credential), and 4 (credential resolution). Allocate two fill codes so a
caller can tell from the status alone whether the destination may have
changed, mirroring the sudo design's split between execution failure and
unconfirmed completion:

| Code | Meaning |
| --- | --- |
| 8 | Fill failed before the value was sent: missing runtime/backend/resolver, permissions, connection/version mismatch, target absence/ambiguity/change, unsupported input, or deadline expiry or cancellation (SIGINT, SIGTERM, SIGHUP) before the mutation gate. The gate is the backend's single value-bearing send; revalidation, focusing the target, and selecting its content happen before it and are not counted as a change |
| 11 | The value may have reached the destination but the result is uncertain: any failure, expiry, or cancellation after the mutation gate, transport loss after the fill was sent, or cleanup that could not be confirmed after a delivered value. A failure before the gate keeps its own code (4 or 8) and appends the cleanup detail to its message |

Expiry uses code 8 even during credential lookup; a provider failure
observed before expiry remains code 4. Rejecting an authentication credential
is code 2. Failed JSON commands leave stdout empty, matching the existing CLI
convention.

Stderr carries one line with the prefix `credential-fill:`, a stable reason
code, and an actionable safe message, in the same shape as the sudo
`sudo-execution:` diagnostics:

```text
credential-fill: target-ambiguous: 3 elements match '#password'; refine the selector
credential-fill: delivery-failed: the browser rejected Input.insertText (protocol error -32000); the destination may have changed, inspect it before retrying
```

The reason code set is fixed and documented in the skill. No raw helper
exception, browser message, or provider stderr is forwarded. Existing codes
for unrelated commands remain unchanged.

## Browser execution

The sequence is the same for both transports; "transport" is the native CDP
client in-process or the Node helper subprocess.

1. Establish the deadline; validate arguments, the endpoint contract, and
   transport availability without resolving credentials.
2. Prepare the transport with explicit operation limits. For the helper:
   pipes, no shell, no inherited terminal input. Send non-secret connection
   and target data.
3. Connect, resolve exactly one page/frame/element, check editability and
   recording preconditions, then acknowledge readiness.
4. Resolve the credential through the cancellable resolver within the
   remaining budget. Deliver its bytes to the already prepared transport only
   while this operation remains active; for the helper, through private
   framed IPC. Do not put the credential in argv, an environment variable, a
   temporary file, or agent-visible messages.
5. Check the remaining deadline and revalidate the prepared page/frame/document
   and target. A navigation, detached target, or newly ambiguous selector fails
   rather than silently retargeting. Fill once and return a closed result code.
6. Detach only this operation's connection and terminate any helper. Leave
   the caller's browser, pages, and existing sessions usable.

Transport output is an internal protocol, not CLI stdout. Rust validates
bounded framed responses and builds its own user-visible messages. For the
helper, drain stderr without forwarding or persisting it; handle output
floods, malformed frames, process termination, and unhandled rejection
without reflecting raw content. Remove inherited Node and Playwright debug
and preload settings from the helper environment, including `NODE_OPTIONS`,
`DEBUG`, `PWDEBUG`, and `PLAYWRIGHT_*` variables, together with the
preload and backtrace variables the resolver already removes. Allow only the
runtime environment needed for connection and execution; do not supply all
configured credentials.

Real values necessarily reach browser transport messages and the target
application. Only these intended internal channels may carry them. The
transport must not start traces, video, HAR, console forwarding, screenshots,
or action recording. The controlling automation host must keep recordings
that capture filling disabled during the operation; stopping recording
afterwards can persist the sensitive action and is not a solution. Preflight
must document which externally enabled recordings can be detected and which
require host configuration. Detectable conflicts fail before resolution.

Phase A established what a second CDP client can observe without starting a
recorder itself: only an open DevTools window on the target page, through
`Target.getDevToolsTarget` and `devtools://` targets. Chromium tracing,
screencast, screen recording, screenshots by another session, Playwright
tracing, HAR, and video leave no read-only signal. CDP preflight therefore
fails on an open DevTools window and treats every other recorder as a host
prerequisite documented in the skill; the fill result never claims those
recorders were absent. Probing by starting a trace and checking for an
"already started" error is excluded because it starts a trace.

The first release targets locally hosted browser endpoints. Authenticated
remote endpoint transport and credential-bearing endpoint URLs need a separate
connection-credential design. Do not embed authentication in CLI arguments.
An existing browser tool's page reference is not automatically a CDP or
Playwright endpoint. Hosts that expose neither are unsupported until they
provide an integration path; no hidden browser launch or GUI fallback occurs.

### CDP attachment contract

CDP attaches to a Chromium-family browser started with a remote debugging
port or exposing an equivalent endpoint. The adapter connects to the browser
target, enumerates page targets without creating one, and attaches to the
selected page and its frame targets for the duration of the operation. The
host's own CDP or Playwright session continues to work; the adapter must not
close targets, contexts, or the browser.

Tested in Phase A: Microsoft Edge `Edg/153.0.4234.48` (Chromium 153),
protocol 1.3, headless, with a separate `playwright-core` 1.63.0 host session
driving the same pages before, during, and after each fill. The adapter uses
`Browser.getVersion`; the `Target` domain for context and target enumeration,
flattened attach and auto-attach, and detach; `Emulation.setFocusEmulationEnabled`;
`Page.getFrameTree` and `Page.createIsolatedWorld`; `Runtime.callFunctionOn`
with fixed scripts; `DOM.describeNode`; and `Input.insertText`. Out-of-process
iframes are separate targets reached through auto-attach on each owned
session; text insertion on the page session reaches the focused frame.
Chrome and Chromium 136 and later require a non-default `--user-data-dir`
for remote debugging; the lab's browser launch documents that setup.

### Shared Playwright endpoint contract

An ordinary `browserType.launchServer()` endpoint isolates contexts by client
connection. Connecting a new helper does not expose pages created by the
automation host's separate client. Creating a replacement context would lose
the caller's page/session and is not a valid workaround. The server's
[context isolation implementation](https://github.com/microsoft/playwright/blob/main/packages/playwright-core/src/server/dispatchers/playwrightDispatcher.ts)
distinguishes isolated and shared connections.

Use a host-owned shared-browser endpoint. The integration candidate is the
public `browser.bind()` API (documented from Playwright 1.59), configured by
the host for a loopback WebSocket and passed to `browserType.connect()`.
[Browser binding API](https://playwright.dev/docs/api/class-browser#browser-bind)
The release baseline is a specific tested client/server version pair and host
setup, frozen in the Playwright milestone; the API's introduction version
alone is not a support claim. Do not depend on undocumented launch flags. If
this public path cannot meet sharing and disconnect requirements for a browser
family, that family remains blocked pending a design revision, not silently
supported through CDP.

The milestone must demonstrate two independent clients: the host opens a page
first, the helper finds and fills that same page without creating a context,
and the host continues using it after helper success, failure, timeout, and
disconnect. Publish the endpoint setup and exact tested versions for each
advertised family. Unsupported endpoint types or an inaccessible existing
page fail before lookup; zero visible matches alone cannot distinguish
isolation from an absent page, so diagnostics must not claim to have detected
the endpoint's server mode. Capabilities report installed protocol support
separately from unverified host sharing capability.

Playwright protocol clients and servers require compatible major/minor
versions. CDP does not have that Playwright server requirement, but supports
Chromium only. Both transport paths require real connection tests. The
Playwright browser family matrix includes Chromium, Firefox, and WebKit;
stock Safari or an arbitrary running Firefox instance is not implied by
WebKit/Firefox support.
[Playwright connection documentation](https://playwright.dev/docs/api/class-browsertype#browser-type-connect)
describes these constraints.

## Desktop adapters

| Delivery | Backend | Target and behavior | Required evidence |
| --- | --- | --- | --- |
| Initial macOS | Enigo text input / Quartz | Expected foreground PID; focused text-input control verified through AX | Granted/denied Accessibility permission, Unicode text, wrong PID, wrong focused control, interrupted input |
| Initial Windows | Enigo text input / SendInput | Expected foreground PID; focused Edit control verified through UIA | Unicode input, foreground change, wrong focused control, normal/elevated integrity boundary |
| Initial Linux X11 | Enigo X11 input | Expected foreground PID where supported; focused field | Live X server, focus checks, text/layout compatibility, unsupported target identity |
| Native setters | macOS AXValue; Windows UIA ValuePattern; Linux AT-SPI EditableText | Explicit process and unique accessible control | Writable/read-only/custom controls, target replacement, permission failure |
| Wayland | AT-SPI when supported; RemoteDesktop Portal + libei for input | Accessible control, or authorized keyboard session with supported target checking | GNOME and KDE tested separately; permission denial, missing portal, session revocation |

For native setters, inspect whether the control supports writing before
resolving the credential. Set its value without reading it back. If a setter
is unsupported, report that explicitly; the caller may request focused input
after positioning the cursor. Do not silently switch to simulated input.

Desktop implementations must preflight known unsupported characters/layout
combinations and never substitute question marks. A mid-input failure reports
possible partial input with code 11 and never automatically retries.

Target checks prevent ordinary focus mistakes; they are not an atomic OS
guarantee against focus stealing or a malicious application. If a backend
cannot establish the expected target, report it unavailable for this operation
instead of ignoring `--expect-pid`. Wayland may require a compositor-specific
target check or the native accessible-control path; the portable keyboard
portal alone does not guarantee foreground PID discovery.

Enigo's Wayland/libei support is currently experimental and disabled by
default. Use its stable platform paths first and select direct portal/libei
integration if the Wayland evaluation shows the experimental backend cannot
satisfy the contract. Do not ship an unsupported backend as a successful no-op.
[Enigo support matrix](https://github.com/enigo-rs/enigo)

The preferred Wayland input sequence is CreateSession, SelectDevices,
Start, then ConnectToEIS with libei. Treat restore tokens as opaque local
session capabilities: keep them out of agent output and do not treat them
as a substitute for permission handling. No automatic installation,
permission escalation, or fallback to privileged `ydotoold` is part of this
route.
[RemoteDesktop Portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)

## Credential resolution and diagnostic ownership

Add a fill resolution stage to the existing confidential resolver with its
own gate, limit, and validation, as listed in the seams section. Preserve
provider selection and value semantics; all providers are subject to the
operation deadline. Keychain and command credentials resolve in the owned
resolver subprocess with the remaining budget as the deadline; env
credentials resolve in-process. The command provider uses no inherited
terminal stdin, suppresses raw stderr, captures stdout only as bounded
candidate bytes, and reports failures without candidate output. A provider
requiring terminal interaction fails with instructions to authenticate it
separately. Platform keychain permission dialogs may still occur, but their
wait consumes the same budget and cannot block cancellation.

Provider calls stay off the coordinator's control loop. The existing resolver
already runs non-interruptible keychain APIs in an isolated subprocess in its
own process group and kills and reaps it on drop; a detached worker thread
would be insufficient because it can outlive cancellation and return a value
into a closed operation. The internal resolver mode requires its private
session channel and does not expose a standalone print-secret command.
Secret IPC and provider diagnostics remain private and bounded.

On cancellation or expiry, close the operation's delivery gate before stopping
and reaping the resolver and any browser helper within the cleanup grace
period. Discard late replies, including replies arriving alongside timeout,
and never route them into another invocation. The coordinator owns the
resolved value and hands it to the backend only through the mutation gate,
immediately before the single value-bearing send; expiry or cancellation
before the gate, including during lookup and revalidation, means no fill was
attempted (code 8). Once the gate has passed, every failure is reported as
uncertain with code 11, whether or not the backend says so. Failure to
confirm process cleanup is an explicit error, not success or permission to
accept a late value. OS dialogs may have
their own lifecycle; do not claim that terminating a resolver necessarily
dismisses them.

Leave existing `run`, `credential check`, `credential set`, and sudo I/O
contracts unchanged. This is a caller-specific stage at the owning boundary,
not a duplicate credential resolver or a global rewrite of subprocess behavior.

Prevent owned and dependency logging from formatting text input, IPC payloads,
or exceptions containing it. Audit the selected Enigo version's logging path
before enabling it; environment debug flags must not turn filling into a
credential transcript. Use fixed error classification rather than relying on
string replacement to remove known secret values.

The transport encoder is a narrow boundary allowed to access secret bytes:
the CDP message builder in-process, or the helper IPC encoder. Retain the
existing lack of general serialization and keep transport payloads out of
Debug/Display. Drop operation buffers promptly; do not claim secure memory
erasure across Rust, JavaScript, and browser copies.

The skill and shipped README will describe reference-based filling, targeting,
permission setup, statuses, reason codes, and soft prohibitions on
reading/revealing values. They must distinguish this owned filling contract
from transparent `run` output.

## Runtime and packaging

The CDP release on the native client adds no runtime dependency beyond the
Rust binary. The Node helper and its packaging below are required for the
Playwright milestone only; Phase A adopted the native client for CDP.

Put the browser helper in `adapters/browser/`, with a package manifest,
pnpm lockfile, and pinned `playwright-core` dependency. TypeScript is suitable
for the helper; ship its compiled JavaScript and runtime dependencies.
The helper protocol has an explicit version checked before resolution.

Release archives include a versioned browser-helper asset directory. Extend
the existing companion installation and `agentenv update` bundle to install
compatible assets alongside the binary/skills, with the same rollback
behavior, and validate their presence/version before a helper-backed fill.
Manual installations document the complete asset layout. A binary-only update
must report incompatible helper assets rather than run a stale helper.

Node.js remains an explicit optional runtime dependency for helper-backed
filling. Missing Node or assets produce a capabilities result and actionable
fill error; ordinary CLI commands, native CDP filling, and desktop filling
remain usable. Filling does not run npx, download dependencies, install a
global runtime, or download browsers on demand. Development/CI dependencies
are installed from the checked-in manifest and lockfile. Browser test
binaries are test dependencies, not a required download when attaching to a
user's existing browser.

Enigo and native desktop bindings use target-specific Cargo dependencies and
feature selection so headless builds and unrelated platforms do not gain
unnecessary GUI linkage. Publish the supported release-feature matrix and
report a compiled-out backend as unavailable. Keep runtime dependencies out
of configuration fields intended for credentials; no new TOML configuration
schema is required for the first release.

## Delivery plan

Phases A–C are complete; the phases after them are unstarted implementation
work.

| Phase | Deliverable | Dependencies | Acceptance |
| --- | --- | --- | --- |
| A (complete) | CDP feasibility and implementation choice | None | Done: native Rust client evaluated against every parity item with the same fixtures as a `connectOverCDP` reference, 45/45 identical; decision, real Chromium attachment, non-destructive detachment, tested versions, and recording detection recorded in [ADR 0001](adr/0001-native-rust-cdp-client.md) and `tests/cdp_lab/` |
| B (complete) | Fill coordinator, CLI contract, resolver fill stage, result normalization | A | Done: `src/fill` owns the coordinator (one deadline from preflight through delivery, two-second cleanup grace, signal cancellation shared with sudo, fixed reason codes, exit 8/11 split); `src/credential/resolver.rs` gains the `fill` stage with the environment-usage gate, 8,192-byte limit, line-oriented command capture, and typed provider-versus-value failures; `src/cli/fill.rs` carries the CLI contract. Verified by `tests/fill_resolver.rs`, `tests/fill_cli.rs` (including SIGTERM cancellation with resolver reaping and sentinel leak checks), and the `src/fill` unit tests; sudo tests unchanged |
| C (complete) | CDP browser filling (first browser release) | B | Done: `src/fill/cdp` is the native client from ADR 0001 moved into the product behind the `Backend` seam. The lab harness runs the product binary as a third implementation (`python3 tests/cdp_lab/run.py --implementation all`); all 47 scenarios pass on Microsoft Edge 153, the 46 shared scenarios with outcomes identical to the prototype and the reference client, plus a product-only DevTools-open scenario that must fail with `recording-conflict`. The run is recorded under `.dev/work/credential-fill-browser/rounds/001/evidence/` |
| D | macOS, Windows, Linux X11 focused text input | B; Windows keychain/command filling additionally needs the Windows resolver | Live desktop fixtures validate exact text, PID and focused-control guards, permissions, Unicode/control-character handling, and honest input-sent results |
| E | Native AX/UIA/AT-SPI control setters | D | Unique explicit control targeting, unsupported-control errors, replacement semantics, and no implicit keyboard fallback |
| F | Wayland implementation and support matrix | D; reuse E where applicable | GNOME/KDE sessions and supported target checks tested; missing/denied/revoked capabilities fail explicitly |
| G | Playwright shared-endpoint filling and helper packaging | C | Frozen public shared-endpoint setup and exact tested versions; independent host/helper clients fill the same existing page for each advertised family; isolated endpoints fail without lookup; helper assets installed, updated, rolled back, and version-checked with the companion bundle |
| H | Documentation, install/update validation, release readiness | Each shippable milestone | Clean-install and update tests cover the milestone's runtime combinations; skill documents the exact shipped capability matrix and reason codes |

Ship the first browser milestone with CDP alone. Desktop platforms can
follow without delaying browser delivery, but a platform is advertised only
after its live verification passes. The Playwright milestone is advertised
per browser family after its independent-client demonstration. Integrate
documentation and packaging in each shippable milestone rather than
postponing them to the end of the entire roadmap.

Future implementation changes involve secret handling and public CLI/helper
contracts. Track their execution and verification and obtain substantive
code-review assurance for those contracts before marking implementation
complete. The planning document itself makes no implementation assurance claim.

## Verification and failure cases

Use test-owned credentials and destinations. Production fill never reads back
the value; a test fixture may inspect its own received value to verify exact
delivery. Keep fixture values and diagnostics isolated from real credentials.

- Parity tests in phase A run the native CDP client and the `connectOverCDP`
  reference against identical local forms and assert identical outcomes for
  every parity item, including out-of-process iframes and reactive input
  events.
- Browser contract tests exercise each shipped transport against local forms:
  password/text fields, reactive input events, nested frames, duplicate pages
  and selectors, origin-path matching with changing query strings,
  hidden/disabled fields, navigation and detachment, disconnect, timeout, and
  application-triggered page changes. Confirm the original browser and the
  host's own session remain usable after successful and failed calls.
- Shared-endpoint tests in phase G use a page created by a separate host
  client before helper attachment; the helper creates no replacement
  context/page. Ordinary isolated `launchServer` endpoints fail before
  provider lookup. A successful connection or a page created by the helper
  itself is not sharing evidence.
- Resolver stage tests confirm that authentication credentials are rejected
  with code 2 before resolution, that environment-usage credentials resolve,
  that values above 8,192 bytes or containing control characters fail before
  delivery, and that sudo and SSH stages keep their existing limits.
- Deadline tests cover blocked keychain reads/permission waits, stalled command
  providers, exhausted budget before delivery, and simultaneous expiry/reply.
  Verify no mutation after lookup cancellation, no late reply reused by a new
  invocation, bounded cleanup, and explicit uncertainty after possible mutation.
  Controlled resolver fixtures establish lifecycle behavior; claimed keychain
  platforms also require real isolated-process access/permission evidence.
- Leak regression tests capture CLI stdout/stderr, transport/provider error
  handling, and owned artifact locations. Cover successful fill, malformed
  transport responses, provider stderr containing candidate text, invalid
  credential bytes, and enabled debug environment variables. Include Unicode
  and escaped JSON characters so serialization failures cannot echo payloads.
- Distinguish recording configured by the adapter from recording owned by an
  external browser host. Verify documented preconditions; do not label
  externally recorded credentials as protected by the fill command.
- Desktop tests run against real fixture windows on the claimed OS/session.
  A headless unit test or successful cross-compilation does not establish GUI
  support. Check permission denial, wrong foreground target, wrong focused
  control in the right process, supported Unicode, rejected controls, and
  partial-input reporting without reading real fields.
- Packaging tests for helper-backed milestones cover missing Node,
  incompatible Playwright server version, helper protocol mismatch,
  missing/stale assets after update, rollback with the companion bundle, and
  the absence of network installation during fill. Verify ordinary CLI
  commands still work with no browser or desktop dependencies available.
- Run existing CI checks when implementation lands: `cargo fmt --check`,
  Clippy for default and test-keychain features, and the existing test-keychain
  suite across supported operating systems. Add transport checks and
  integration jobs appropriate to the new behavior; avoid duplicating
  internal tests.

## Technical uncertainties to close during implementation

1. **Native CDP parity (closed):** Phase A proved every parity item,
   including out-of-process iframe attachment and event fidelity for a React
   controlled input, on Edge 153. Remaining CDP risks are listed in ADR 0001:
   only Edge was available on the lab machine, and the prototype is
   synchronous.
2. **Windows resolver:** keychain and command filling on Windows needs the
   deferred Windows confidential resolver. Decide in phase B whether Windows
   desktop filling ships env-only first or waits; capabilities must state the
   result.
3. **Browser helper packaging:** for the Playwright milestone, validate the
   compiled dependency layout, supported Node range, and installer/update
   placement within the companion bundle. The chosen outcome is an explicit
   versioned companion, not a runtime package download.
4. **Connection ownership and recording:** freeze the shared-endpoint public
   API, host setup, and compatible versions in phase G using the
   independent-client test; prove disconnect leaves the owner's session alive.
   Determine which recorder states each transport can observe. External host
   cooperation is a documented prerequisite where detection is unavailable.
5. **Desktop text fidelity and focus metadata:** inspect Enigo logging and test
   the selected version against supported keyboards and Unicode. Establish
   which focused-control roles each platform can read reliably; use the native
   backend where a concrete incompatibility warrants it, with an explicit
   capability result.
6. **Wayland target identity:** validate the portal and AT-SPI capabilities in
   each desktop environment. If reliable target identification is unavailable,
   defer focused input for that environment and expose the reason. Do not
   silently weaken the target contract.

## Native API references

- [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/)
- [macOS AX attribute setting](https://developer.apple.com/documentation/applicationservices/1460434-axuielementsetattributevalue)
- [Windows UI Automation SetValue](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationvaluepattern-setvalue)
- [Windows SendInput and integrity restrictions](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)
- [Linux AT-SPI EditableText](https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/iface.EditableText.html)
- [Chrome remote-debugging profile constraints](https://developer.chrome.com/blog/remote-debugging-port)
