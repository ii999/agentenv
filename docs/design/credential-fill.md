# Credential filling for browsers and desktop applications

Status: planned; no filling commands or adapters are implemented.

## Outcome and scope

An agent supplies a credential name and a destination. Local execution
resolves the credential and fills the destination without putting the value
in the agent's tool arguments, results, diagnostics, or generated artifacts.
The agent continues using its existing browser or desktop automation tools
to navigate and perform the surrounding task.

The first browser release supports both Playwright protocol connections and
Chrome DevTools Protocol (CDP) connections. Desktop delivery starts with
macOS, Windows, and Linux X11 through Enigo, then adds native control setters
and separately validated Wayland support.

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

- `src/credential/mod.rs` owns provider selection and resolution for env,
  keychain, and command credentials. Reuse this ownership.
- `src/credential/secret.rs` keeps values out of Display, Serialize, and
  Debug output. Preserve that contract; do not make `Secret` generally
  serializable to implement browser IPC.
- `src/credential/command.rs` currently inherits stdin and stderr. That is
  appropriate to the existing transparent execution contract, but insufficient
  for filling: a provider could print a candidate value on failure.
- `src/runner.rs` intentionally forwards child output. Leave `run` semantics
  intact; a filling operation must own and normalize its helper output.
- `src/cli/mod.rs`, `src/cli/credential.rs`, and `src/error.rs` own the CLI
  additions and failure mapping.
- Release packaging currently distributes the Rust executable and skills.
  The browser helper adds a runtime and distribution dependency that must
  ship together with browser support.

No credential-store format, reference syntax, or project-file trust change
is needed. A name identifies an existing credential definition, as with
`credential check`. Existing file selection and configuration validation
continue to apply; `inject_as` is irrelevant to direct filling.
If the sudo design's credential usages are present, reject authentication
credentials before resolution. Reuse its confidential resolver policy and
cancellation mechanism when available; filling does not depend on sudo delivery.

## Architecture decision

Three approaches were considered:

| Approach | Benefit | Cost / limitation |
| --- | --- | --- |
| `run` plus arbitrary user scripts | Smallest implementation; already supplies environment injection | Every script must independently handle output, targeting, and failures; no consistent filling contract |
| Unified fill command with owned adapters | Centralizes credential and output handling; retains existing providers | Requires a browser helper and platform dependencies |
| Persistent credential broker integrated into every automation host | Can reuse host-owned page and control handles | Requires host-specific integration and service lifecycle beyond the normal-workflow requirement |

Use the unified command with owned adapters. Keep browser connections inside
one Node.js helper using `playwright-core`; select `browserType.connect()`
or `chromium.connectOverCDP()` according to the requested backend. This
shares targeting, filling, and error mapping without implementing the
Playwright wire protocol in Rust or maintaining a second DOM implementation
for CDP. The Playwright endpoint must expose the host's existing contexts to
another client; protocol compatibility alone does not establish that capability.

Keep desktop input in the Rust process. Start with Enigo's text-input API,
with small native adapters for checking the foreground target and later
setting accessible controls. A fill coordinator owns preflight, secret
resolution, execution, and safe results. Platform APIs and browser transport
details stay behind the filling seam; credential storage stays behind the
existing provider seam.

## Proposed CLI and result contract

These examples describe future commands, not current functionality.

```text
agentenv credential fill portal_password --backend playwright \
  --endpoint ws://127.0.0.1:3000/session --browser chromium \
  --page-url https://portal.example/login --selector '#password' --json

agentenv credential fill portal_password --backend cdp \
  --endpoint http://127.0.0.1:9222 \
  --page-url https://portal.example/login --selector '#password' --json

agentenv credential fill portal_password --backend desktop \
  --expect-pid 12345 --json

agentenv credential fill --capabilities --json
```

- A fill request names exactly one credential and an explicit backend.
  There is no inline-value, print-value, or arbitrary JavaScript option.
- The capabilities form requires no credential and resolves none. It reports
  compiled backends, helper/runtime availability, and known permission state.
  Unknown permission or desktop capability remains unknown, not available.
- Playwright requires an explicit browser family: chromium, firefox, or
  webkit. CDP is Chromium-only. An incompatible option is a usage error.
  `--endpoint` for Playwright denotes a supported shared-browser endpoint
  prepared by the automation host, not an ordinary isolated `launchServer` endpoint.
- Browser target selection uses an exact page URL, with optional
  `--context-index` to disambiguate contexts. The index is scoped to the
  current connection, not a durable identity. Zero or multiple matching pages
  fail. Never select the first tab implicitly.
- A repeatable `--frame-selector` describes a strict chain of iframe
  elements; omission selects the main frame. `--selector` is a CSS selector
  that must identify exactly one visible, enabled, editable target.
- Desktop input requires the expected foreground PID and an already focused,
  empty input field prepared by the caller. The adapter does not read the
  value to establish emptiness. It inserts text; it does not clear, select all, click,
  switch applications, press Enter, or submit. Where accessible focus
  metadata exists, capture and recheck it immediately before input.
- A future native desktop setter uses `--target-file` with a TOML target
  description and explicit `--method control`. It replaces that control's
  value. This is a later interface, not an initial desktop promise.
- Browser filling replaces the field using Playwright `fill`. Focused
  desktop insertion returns a different effect so callers cannot confuse
  event delivery with a verified field value or successful authentication.
- Use one monotonic operation deadline, initially 10 seconds with a positive
  `--timeout-ms` override capped at 300,000 milliseconds. It starts before
  preflight and includes helper startup, connection, target checks, credential
  lookup, permission waits, and input delivery. Every phase receives the
  remaining budget; lookup and IPC do not reset it. Expiry closes the operation
  to new secret delivery or mutation. Cleanup has a separate maximum two-second
  grace period and cannot resume the operation. Do not retry after mutation may
  have started; report possible partial/completed mutation and any unconfirmed
  cleanup explicitly.

Successful JSON contains only a version, backend, and effect, for example:

```json
{"version":1,"backend":"cdp","effect":"field-filled"}
```

The desktop input effect is `input-sent`; a native setter returns
`field-filled` after its API reports success. Neither effect claims readback
verification, persistence, submission, or login success.

Preserve existing exit codes 1 (usage), 2 (configuration), 3 (unknown
credential), and 4 (credential resolution). Allocate 8 to fill failures,
including missing runtime/backend, permissions, connection/version mismatch,
target absence/ambiguity/change, unsupported input, operation deadline expiry,
and uncertain mutation. Expiry uses code 8 even during credential lookup;
a provider failure observed before expiry remains code 4.
Failed JSON commands leave stdout empty, matching the existing CLI convention.
Stderr carries a stable reason and actionable safe message, including whether
mutation might have occurred. No raw helper exception or provider stderr is
forwarded. Existing codes for unrelated commands remain unchanged.

## Browser execution

1. Establish the deadline; validate arguments, the supported endpoint contract,
   and helper availability without resolving credentials.
2. Start the owned helper with pipes, no shell, no inherited terminal input,
   and explicit operation limits. Send non-secret connection and target data.
3. Connect, resolve exactly one page/frame/element, check editability and
   recording preconditions, then acknowledge readiness.
4. Resolve the credential through the cancellable Rust provider boundary within
   the remaining budget. Send its bytes through private framed IPC to the already
   prepared helper only while this operation remains active. Do not put the
   credential in argv, an environment variable, a temporary file, or agent-visible
   messages.
5. Check the remaining deadline and revalidate the prepared page/frame/document
   and target. A navigation, detached target, or newly ambiguous selector fails
   rather than silently retargeting. Fill once and return a closed result code.
6. Disconnect only this helper's connection and terminate the helper. Leave
   the caller's browser, pages, and existing sessions usable.

The helper's stdout is an internal protocol, not CLI stdout. Rust validates
bounded framed responses and builds its own user-visible messages. Drain
stderr without forwarding or persisting it; handle output floods, malformed
frames, process termination, and unhandled rejection without reflecting raw
content. Disable inherited Playwright/Node debug and preload settings for
the helper. Allow only the runtime environment needed for connection and
execution; do not supply all configured credentials.

Real values necessarily reach browser transport messages and the target
application. Only these intended internal channels may carry them. The
helper must not start traces, video, HAR, console forwarding, screenshots,
or action recording. The controlling automation host must keep recordings
that capture filling disabled during the operation; stopping recording
afterwards can persist the sensitive action and is not a solution. Preflight
must document which externally enabled recordings can be detected and which
require host configuration. Detectable conflicts fail before resolution.

The first release targets locally hosted browser endpoints. Authenticated
remote endpoint transport and credential-bearing endpoint URLs need a separate
connection-credential design. Do not embed authentication in CLI arguments.
An existing browser tool's page reference is not automatically a CDP or
Playwright endpoint. Hosts that expose neither are unsupported until they
provide an integration path; no hidden browser launch or GUI fallback occurs.

### Shared Playwright endpoint contract

An ordinary `browserType.launchServer()` endpoint isolates contexts by client
connection. Connecting a new helper does not expose pages created by the
automation host's separate client. Creating a replacement context would lose
the caller's page/session and is not a valid workaround. The server's
[context isolation implementation](https://github.com/microsoft/playwright/blob/main/packages/playwright-core/src/server/dispatchers/playwrightDispatcher.ts)
distinguishes isolated and shared connections.

Use a host-owned shared-browser endpoint. The initial integration candidate is
the public `browser.bind()` API (documented from Playwright 1.59), configured by
the host for a loopback WebSocket and passed to `browserType.connect()`.
[Browser binding API](https://playwright.dev/docs/api/class-browser#browser-bind)
The release baseline is a specific tested client/server version pair and host
setup, frozen in phase A; the API's introduction version alone is not a support
claim. Do not depend on undocumented launch flags. If this public path cannot
meet sharing and disconnect requirements for a browser family, that family
remains blocked pending a design revision, not silently supported through CDP.

Phase A must demonstrate two independent clients: the host opens a page first,
the helper finds and fills that same page without creating a context, and the
host continues using it after helper success, failure, timeout, and disconnect.
Publish the endpoint setup and exact tested versions for each advertised family.
Unsupported endpoint types or an inaccessible existing page fail before lookup;
zero visible matches alone cannot distinguish isolation from an absent page,
so diagnostics must not claim to have detected the endpoint's server mode.
Capabilities report installed protocol support separately from unverified host
sharing capability. CDP retains its separately tested attachment contract.

Playwright protocol clients and servers require compatible major/minor
versions. CDP does not have that Playwright server requirement, but supports
Chromium only and has lower fidelity than Playwright protocol connections.
Both transport paths require real connection tests. The Playwright browser
family matrix includes Chromium, Firefox, and WebKit; stock Safari or an
arbitrary running Firefox instance is not implied by WebKit/Firefox support.
[Playwright connection documentation](https://playwright.dev/docs/api/class-browsertype#browser-type-connect)
describes these constraints.

## Desktop adapters

| Delivery | Backend | Target and behavior | Required evidence |
| --- | --- | --- | --- |
| Initial macOS | Enigo text input / Quartz | Expected foreground PID; focused field | Granted/denied Accessibility permission, Unicode text, wrong PID, interrupted input |
| Initial Windows | Enigo text input / SendInput | Expected foreground PID; focused field | Unicode input, foreground change, normal/elevated integrity boundary |
| Initial Linux X11 | Enigo X11 input | Expected foreground PID where supported; focused field | Live X server, focus checks, text/layout compatibility, unsupported target identity |
| Native setters | macOS AXValue; Windows UIA ValuePattern; Linux AT-SPI EditableText | Explicit process and unique accessible control | Writable/read-only/custom controls, target replacement, permission failure |
| Wayland | AT-SPI when supported; RemoteDesktop Portal + libei for input | Accessible control, or authorized keyboard session with supported target checking | GNOME and KDE tested separately; permission denial, missing portal, session revocation |

For native setters, inspect whether the control supports writing before
resolving the credential. Set its value without reading it back. If a setter
is unsupported, report that explicitly; the caller may request focused input
after positioning the cursor. Do not silently switch to simulated input.

The initial fill feature accepts single-line text. Reject control characters
such as CR, LF, TAB, and escape before any input rather than interpreting them
as keys or silently stripping them. Preserve spaces, punctuation, and supported
Unicode exactly. Desktop implementations must preflight known unsupported
characters/layout combinations and never substitute question marks. A
mid-input failure reports possible partial input and never automatically retries.
Multi-line credentials remain supported by existing storage/run behavior but
are outside initial filling support.

Target checks prevent ordinary focus mistakes; they are not an atomic OS
guarantee against focus stealing or a malicious application. If a backend
cannot establish the expected target, report it unavailable for this operation
instead of ignoring `--expect-pid`. Wayland may require a compositor-specific
target check or the native accessible-control path; the portable keyboard
portal alone does not guarantee foreground PID discovery.

Enigo's Wayland/libei support is currently experimental. Use its stable
platform paths first and select direct portal/libei integration if the
Wayland evaluation shows the experimental backend cannot satisfy the contract.
Do not ship an unsupported backend as a successful no-op.
[Enigo support matrix](https://github.com/enigo-rs/enigo)

The preferred Wayland input sequence is CreateSession, SelectDevices,
Start, then ConnectToEIS with libei. Treat restore tokens as opaque local
session capabilities: keep them out of agent output and do not treat them
as a substitute for permission handling. No
automatic installation, permission escalation, or fallback to privileged
`ydotoold` is part of this route.
[RemoteDesktop Portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)

## Credential resolution and diagnostic ownership

Add an explicit resolution I/O and cancellation policy at the existing provider
boundary for the fill caller. Preserve provider selection and value semantics;
all providers are subject to the operation deadline. The command provider uses
no inherited terminal stdin, suppresses raw stderr, captures stdout only as
bounded candidate bytes, and reports failures without candidate output. A
provider requiring terminal interaction fails with instructions to authenticate
it separately. Platform keychain permission dialogs may still occur, but their
wait consumes the same budget and cannot block cancellation.

Keep provider calls off the coordinator's control loop. Run non-interruptible
keychain APIs in an isolated resolver subprocess, reusing the provider adapters
and the sudo design's confidential resolver seam. A detached worker thread is
insufficient: it can outlive cancellation and return a value into a closed
operation. Internal resolver mode requires its private operation channel and
does not expose a standalone print-secret command. Secret IPC and provider
diagnostics remain private and bounded.

On cancellation or expiry, close the operation's delivery gate before stopping
and reaping owned resolver/provider processes and the browser helper within the
cleanup grace period. Discard late replies, including replies arriving alongside
timeout, and never route them into another invocation. A lookup timeout before
delivery means no fill was attempted; once mutation may have started, report
uncertainty. Failure to confirm process cleanup is an explicit error, not success
or permission to accept a late value. OS dialogs may have their own lifecycle;
do not claim that terminating a resolver necessarily dismisses them.

Leave existing `run`, `credential check`, and `credential set` I/O contracts
unchanged. This is a caller-specific policy at the owning boundary, not a
duplicate credential resolver or a global rewrite of subprocess behavior.

Prevent owned and dependency logging from formatting text input, IPC payloads,
or exceptions containing it. Audit the selected Enigo version's logging path
before enabling it; environment debug flags must not turn filling into a
credential transcript. Use fixed error classification rather than relying on
string replacement to remove known secret values.

The browser IPC encoder is a narrow transport boundary allowed to access
secret bytes. Retain the existing lack of general serialization and keep
transport payloads out of Debug/Display. Drop operation buffers promptly;
do not claim secure memory erasure across Rust, JavaScript, and browser copies.

The skill and shipped README will describe reference-based filling, targeting,
permission setup, statuses, and soft prohibitions on reading/revealing values.
They must distinguish this owned filling contract from transparent `run` output.

## Runtime and packaging

Put the browser helper in `adapters/browser/`, with a package manifest,
pnpm lockfile, and pinned `playwright-core` dependency. TypeScript is suitable
for the helper; ship its compiled JavaScript and runtime dependencies.
The helper protocol has an explicit version checked before resolution.

Release archives include a versioned browser-helper asset directory. Extend
installation and `agentenv update` to install compatible assets alongside the
binary/skills and validate their presence/version before a browser fill.
Manual installations document the complete asset layout. A binary-only update
must report incompatible helper assets rather than run a stale helper.

Node.js remains an explicit optional runtime dependency for browser filling.
Missing Node or assets produce a capabilities result and actionable fill
error; ordinary CLI commands and desktop filling remain usable. Filling
does not run npx, download dependencies, install a global runtime, or download
browsers on demand. Development/CI dependencies are installed from the
checked-in manifest and lockfile. Browser test binaries are test dependencies,
not a required download when attaching to a user's existing browser.

Enigo and native desktop bindings use target-specific Cargo dependencies and
feature selection so headless builds and unrelated platforms do not gain
unnecessary GUI linkage. Publish the supported release-feature matrix and
report a compiled-out backend as unavailable. Keep runtime dependencies out
of configuration fields intended for credentials; no new TOML configuration
schema is required for the first release.

## Delivery plan

All phases below are unstarted implementation work.

| Phase | Deliverable | Dependencies | Acceptance |
| --- | --- | --- | --- |
| A | Browser sharing, connection, and packaging feasibility | None | Freeze public shared-endpoint setup and exact tested versions; independent host/helper clients fill the same existing page for each advertised Playwright family; isolated endpoints fail without lookup; real Chromium CDP attachment, compatible assets, and non-destructive disconnect; recording limits documented |
| B | Fill coordinator, CLI contract, cancellable provider policy, result normalization | A | Existing commands retain behavior; no provider/helper values in failures; target preflight precedes resolution; one deadline covers keychain/command lookup, cancellation closes delivery, late replies are discarded, and owned processes are reaped or cleanup failure reported |
| C | Playwright and CDP browser filling together | B | Exact targeting, iframe selection, replacement semantics, timeout/partial-state reporting, and absence of secrets in owned output verified through both transports |
| D | macOS, Windows, Linux X11 focused text input | B | Live desktop fixtures validate exact text, target guards, permissions, Unicode/control-character handling, and honest input-sent results |
| E | Native AX/UIA/AT-SPI control setters | D | Unique explicit control targeting, unsupported-control errors, replacement semantics, and no implicit keyboard fallback |
| F | Wayland implementation and support matrix | D; reuse E where applicable | GNOME/KDE sessions and supported target checks tested; missing/denied/revoked capabilities fail explicitly |
| G | Documentation, install/update validation, release readiness | C and the desktop phases included in that release | Clean-install and update tests cover helper/runtime combinations; skill documents the exact shipped capability matrix |

Ship the first browser milestone only with both requested transports. Desktop
platforms can follow without delaying browser delivery, but a platform is
advertised only after its live verification passes. Integrate documentation
and packaging in each shippable milestone rather than postponing them to the
end of the entire roadmap.

Future implementation changes involve secret handling and public CLI/helper
contracts. Track their execution and verification and obtain substantive
code-review assurance for those contracts before marking implementation
complete. The planning document itself makes no implementation assurance claim.

## Verification and failure cases

Use test-owned credentials and destinations. Production fill never reads back
the value; a test fixture may inspect its own received value to verify exact
delivery. Keep fixture values and diagnostics isolated from real credentials.

- Browser contract tests exercise both actual transports against local forms:
  password/text fields, reactive input events, nested frames, duplicate pages
  and selectors, hidden/disabled fields, navigation and detachment, disconnect,
  timeout, and application-triggered page changes. Confirm the original
  browser remains usable after successful and failed calls.
- Shared-endpoint tests use a page created by a separate host client before
  helper attachment; the helper creates no replacement context/page. Ordinary
  isolated `launchServer` endpoints fail before provider lookup. A successful
  connection or a page created by the helper itself is not sharing evidence.
- Deadline tests cover blocked keychain reads/permission waits, stalled command
  providers, exhausted budget before delivery, and simultaneous expiry/reply.
  Verify no mutation after lookup cancellation, no late reply reused by a new
  invocation, bounded cleanup, and explicit uncertainty after possible mutation.
  Controlled resolver fixtures establish lifecycle behavior; claimed keychain
  platforms also require real isolated-process access/permission evidence.
- Leak regression tests capture CLI stdout/stderr, helper/provider error
  handling, and owned artifact locations. Cover successful fill, malformed
  helper responses, provider stderr containing candidate text, invalid
  credential bytes, and enabled debug environment variables. Include Unicode
  and escaped JSON characters so serialization failures cannot echo payloads.
- Distinguish recording configured by the adapter from recording owned by an
  external browser host. Verify documented preconditions; do not label
  externally recorded credentials as protected by the fill command.
- Desktop tests run against real fixture windows on the claimed OS/session.
  A headless unit test or successful cross-compilation does not establish GUI
  support. Check permission denial, wrong foreground target, supported Unicode,
  rejected controls, and partial-input reporting without reading real fields.
- Packaging tests cover missing Node, incompatible Playwright server version,
  helper protocol mismatch, missing/stale assets after update, and the absence
  of network installation during fill. Verify ordinary CLI commands still
  work with no browser or desktop dependencies available.
- Run existing CI checks when implementation lands: `cargo fmt --check`,
  Clippy for default and test-keychain features, and the existing test-keychain
  suite across supported operating systems. Add helper checks and integration
  jobs appropriate to the new behavior; avoid duplicating internal tests.

## Technical uncertainties to close during implementation

1. **Browser helper packaging:** validate the actual compiled dependency layout,
   supported Node range, and installer/update placement in phase A. The chosen
   outcome is an explicit versioned companion, not a runtime package download.
2. **Connection ownership and recording:** freeze the shared-endpoint public API,
   host setup, and compatible versions in phase A using the independent-client
   test; prove disconnect leaves the owner's session alive. Determine which
   recorder states the adapter can observe. External host cooperation is a
   documented prerequisite where detection is unavailable.
3. **Desktop text fidelity:** inspect Enigo logging and test the selected version
   against supported keyboards and Unicode. Use the native backend where a
   concrete incompatibility warrants it, with an explicit capability result.
4. **Wayland target identity:** validate the portal and AT-SPI capabilities in
   each desktop environment. If reliable target identification is unavailable,
   defer focused input for that environment and expose the reason. Do not
   silently weaken the target contract.
5. **Resolver cancellation:** select the smallest subprocess boundary that can
   interrupt keychain/provider waits without blocking the control loop. Verify
   deadline propagation, private IPC, late-response disposal, and process cleanup
   in phase B; preserve the existing provider semantics and share this mechanism
   with sudo instead of implementing a second confidential resolver.

## Native API references

- [macOS AX attribute setting](https://developer.apple.com/documentation/applicationservices/1460434-axuielementsetattributevalue)
- [Windows UI Automation SetValue](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationvaluepattern-setvalue)
- [Windows SendInput and integrity restrictions](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)
- [Linux AT-SPI EditableText](https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/iface.EditableText.html)
- [Chrome remote-debugging profile constraints](https://developer.chrome.com/blog/remote-debugging-port)
