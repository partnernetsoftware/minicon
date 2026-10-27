# Harness settings and local model gateway

Parent: [0.2.x horizon](PRD_02_31_v0_2_horizon.md). Product boundary:
[charter](PRD_02_23_minicon.md). Sequence:
[0.2.23 plan](../plan/plan-v0.2.23.md). Status: `[-]` implementation active;
existing harness/browser mechanisms are inputs, not qualification of this feature.
Account-auth implementation is now in progress; the owner goal authorizes
development through public 0.2.23. The experimental-release amendment below
now governs the real-provider acceptance boundary.

This module owns provider account identity and authentication, the local
inference API, and the selection seam for a future `llm-gate`. Its
[connection-experience child](PRD_02_31_01_01_harness_experience.md) owns Harness
information architecture, visible state, first use and recovery;
[provider pool](PRD_02_31_01_02_provider_pool.md) owns multiple accounts,
per-account networks and keyed client allocation; [monitoring](PRD_02_31_01_03_usage_monitoring.md)
owns local statistics and provider balance/allowance observations. The released
worker's file/exec authority is recorded in the parent module; its future
independent app ownership and handoff are in [PRD 40](PRD_02_40_plugins_apps.md).
This gateway never acquires tool authority.

## Outcome tree DAG

```text
Outcome: connect a model account once and use it from MiniCon's worker or a compatible local harness
├── [-] SETTINGS — Console / Web / Harness in the existing settings panel
│   invariant: one Harness route guides account -> verified model -> local tool, with truthful independent states
│   evidence: PRD 31.01.01 HU-01..09 plus public route/focus/state/pixel journeys
│   safe failure: keep the prior configuration and terminal draft; show a local actionable error
│   dependency: workspace controls and additive PRD 26 observations
│   non-goal: another settings window or duplicating PRD 36 task operation in Settings
│   owner: PRD_02_31_01_01_harness_experience.md (redesign specified; implementation open)
├── [-] AUTH — DeepSeek and OpenCode provider identities and usable credentials
│   invariant: flow availability follows the verified initialization contract; AccountReady requires successful real inference
│   evidence: dated official contract plus real login/inference/logout and expiry/refresh where the provider supplies them
│   safe failure: unavailable authentication is BLOCKED; no fabricated OAuth endpoint or successful-login badge
│   dependency: provider feasibility probe, private platform storage and browser presentation when required
│   non-goal: calling API-key entry, website cookies or Console MCP authorization model-inference OAuth
├── [-] POOL — account registry; multi-account/client allocation redesign open
│   invariant: separate account/network/profile identities; conversation binding stays fixed
│   evidence: PRD 31.01.02 PP-01..06; two DeepSeek accounts plus OpenCode Go
│   safe failure: explain which binding needs repair; never silently use another account or provider
│   dependency: AUTH and a typed selection contract
│   non-goal: automatic rotation, load balancing, fallback chains or quota scheduling
│   owner: PRD_02_31_01_02_provider_pool.md
├── [_] OBSERVE — local client/account usage and provider balance/allowance
│   invariant: measured local usage and authoritative remote snapshots retain source and coverage
│   evidence: PRD 31.01.03 UM-01..06, including stream/cancel and unavailable metadata
│   safe failure: unknown/stale/partial instead of fabricated zero balance or usage
│   dependency: POOL identities, response metadata and verified provider read contracts
│   non-goal: invoices, auto-top-up, scraping or changing a provider's paid overage policy
│   owner: PRD_02_31_01_03_usage_monitoring.md
├── [-] API — explicit process-lifetime loopback model endpoint
│   invariant: OpenAI Chat Completions and Anthropic Messages expose model inference only
│   evidence: authenticated text/tool/SSE/cancel round trips and unauthorized/port-conflict/stop black boxes
│   safe failure: bounded protocol-shaped error, cancellation and release of the account lease
│   dependency: POOL, protocol adapters and bounded native HTTP streaming mechanisms
│   non-goal: file/exec endpoints, remote tasks, LAN access, daemon or protocol-completeness claims
├── [-] WORKER CLIENT — connect the optional minicon-harness app through the model API
│   invariant: local root/command policy and task lifetime stay with the separate worker; gateway returns model results only
│   evidence: exact host/app pair: real model -> tool description -> app execution -> tool result -> final answer; negative tool courts
│   safe failure: missing app, account or API cannot start a tool; app owns cancellation and descendant cleanup
│   dependency: API, PRD 40 app seam and worker migration acceptance
│   non-goal: a second agent loop in MiniCon or treating exec's cwd as a filesystem sandbox
└── [-] CLIENT / FUTURE SEAM — one named external client and replaceable selection
    invariant: compatibility names client/version/protocol/model; selection returns a binding, not execution authority
    evidence: one real external tool/SSE/cancel journey and a fake selector substitutable without codec changes
    safe failure: unsupported client/capability is explicit and unqualified
    dependency: API and frozen selection seam
    non-goal: all CLI/TUI compatibility, implementing llm-gate, or embedding ujs/tinyvm in 0.2.23
```

## Memory palace

```mermaid
flowchart LR
    S[Settings: Console / Web / Harness] --> UX[PRD 31.01.01: guided connection and recovery]
    UX --> A[Account registry and login]
    A --> POOL[PRD 31.01.02: account networks / client keys / allocations]
    A --> V{Verified inference authorization?}
    V -->|no| B[BLOCKED provider auth claim]
    V -->|yes| P[Manual provider/model/account selector]
    H[Released embedded worker / future separate app] --> G[Loopback inference API]
    E[Named external harness] --> G
    G --> C[Protocol adapters and capability validation]
    C --> P
    POOL --> P
    G --> OBS[PRD 31.01.03: local request statistics]
    U --> BAL[Verified provider balance / allowance reads]
    BAL --> OBS
    P --> U[Provider adapter and private credentials]
    U --> M[Upstream model]
    M -->|text or tool description| G
    G -->|tool description only| H
    G -->|tool description only| E
    H --> T[Worker-owned local tools]
    E --> ET[Client-owned tools]
    F[Future llm-gate / ujs + tinyvm wasm] -. replace selector through the same binding contract .-> P
    X[Stop / disconnect / deadline] --> K[Cancel upstream, release lease, bounded shutdown]
    K --> G
```

## Decisions and first delivery

### 0.2.23 experimental release amendment (2026-10-26)

The owner accepts shipping the rapidly evolving Harness/provider pool as an
experimental feature without waiting for complete real-account acceptance.
The two-DeepSeek-plus-OpenCode scenario and complete real-client journeys remain
open acceptance work carried into 0.2.24; their absence no longer blocks 0.2.23.
They are not marked passed. Synthetic tests do not establish broad OAuth,
entitlement, balance or external-client compatibility. Public release notes must
disclose this scope. Native/build correctness, credential isolation, required
artifact qualification, signing, Defender and no-rebuild Promotion remain
required; missing native evidence is not converted to PASS.

The owner also selects [independently versioned dynamic logic](PRD_02_31_01_04_dynamic_logic.md)
after unisacc/ujs maturity, so fast Harness policy changes need not always ship
a new native client. That module owns host boundaries, updates, rollback and
dependency gates. No engine is embedded in 0.2.23.

Owner direction, 2026-10-24: add Harness beside Console and Web, configure a
local API port, support OpenAI and Anthropic model protocols, and start a
provider-pool with DeepSeek and OpenCode account login. Subsequent narrowing:
**first make the path work**; a future `llm-gate` may schedule through the
owner's ujs + tinyvm (wasm) dynamic engine.

Owner clarification, 2026-10-25: the product is a provider pool serving several
local harnesses. Two DeepSeek accounts (one needing a proxy for Google sign-in)
and one OpenCode Go account are the reference case. Multiple keyed client
allocations, account-specific network policy and statistics/balance/allowance
monitoring are selected design scope under the children above. This supersedes
the earlier single-default-profile and no-monitoring cut. Initial allocation is
manual; automatic scheduling and the dynamic engine remain later work.

**Transport decision:** HTTP is the model interchange for external clients.
CLI commands manage or run that same product capability; they do not spawn
another agent CLI for each inference request. Reusable Rust request/event and
selection interfaces sit below HTTP. Initially the built-in worker consumes
HTTP too, making the shared path observable; later an in-process consumer can
reuse the interfaces without a different credential or selection policy.

The provider gateway is a new, narrowly admitted model-listener capability.
It does not inherit the GUI control endpoint's tab authority and cannot call
the worker's tools. MiniCon still has no agent manager or remote execution
service. An inference response describing a tool never authorizes its execution.

### Planned CLI connection and legacy behavior

The first implementation adds `minicon provider serve [--port <port>]` for
foreground headless serving. GUI Start and this command use the same service
composition; startup does not discover/kill another instance or auto-select a
different port. Provider management initially stays in Harness settings rather
than adding a second account editor to the CLI.

The existing worker gains an explicit gateway mode:

```sh
minicon harness --root <path> --task "<task>" \
  --gateway http://127.0.0.1:28789/v1 \
  --client-key-env MINICON_LOCAL_API_KEY --allow-cmd <name>
```

Gateway worker flags, foreground serving and settings/server composition are implemented; account-backed and final native qualification remain in progress. The real CLI fixture `gateway_worker_runs_its_own_tools_and_refuses_changed_resume_binding` verifies native HTTP, both worker-owned tools, key exclusion from exec, persisted binding and changed-binding refusal against a controlled model peer. This does not establish a real OAuth account or hosted inference. Gateway mode validates a
literal IPv4 loopback URL, requires the named client-key environment variable,
and resolves model/account through that key's client profile. No credential
value belongs in argv. Explicit `--backend` and `--gateway` are mutually
exclusive; absent `--gateway`, legacy backend/env behavior stays unchanged.
`--root`, `--allow-cmd`, `--session` and `--continue` retain their worker meaning.
There is no implicit listener start or fallback to a legacy upstream when the
gateway is unavailable. Bind the worker's session to the client profile and
provider/account/model identity; continuing it with a different binding is a
typed refusal, never an unnoticed provider change. The child exec environment
does not inherit the local client key.

## Settings and first-use journey

Owner: [Harness connection experience](PRD_02_31_01_01_harness_experience.md).
The owner's 2026-10-25 product feedback reopens the experience before public
0.2.23. That module replaces the earlier listener-first section layout and
owns the proposed journey, account/model draft and apply behavior, independent
visible states, authorization result handling and complete client handoff.
Its `[_]` rows are not implemented merely because the current API works.

The API, auth and resource contracts below remain authoritative. In particular,
the UI distinction between authorization received and model verified must not
weaken account admission. Existing first-connection receipts remain scoped to
their actual source and behavior; they do not qualify the redesigned product.

## Listener lifecycle and local client credentials

- Bind **`127.0.0.1` only**, using port **28789** as the proposed initial
  editable default. Valid ports are 1024..65535; the displayed URL uses the
  actual bound port. This is a design default, not a claim that the port is
  free. No wildcard bind, LAN mode, tunnel, browser CORS grant or port search.
- States: `stopped -> starting -> listening -> draining -> stopped`; startup
  and runtime failure enter `error` with a retry action. Start validates the
  configuration and at least one usable client binding before accepting inference;
  another incomplete profile cannot disable an unrelated working profile.
- Changing the port requires Stop first; while listening the field explains
  that step and is not editable. Save the validated port atomically, then
  explicitly Start. A failed save retains the previous configuration; a bind
  conflict stays stopped/error on the chosen port. No live port migration or
  silent endpoint change is needed for the first delivery.
- The GUI owns the listener until its process exits. Closing Settings does
  not stop it; detach/reattach follows process lifetime. A new GUI launch
  always starts stopped, even when the port/accounts are persisted.
- Headless `minicon provider serve` is the same server in an explicitly run
  foreground process. It uses stored accounts without creating or initializing
  a WebView or GUI; it
  neither attaches to nor starts a second GUI-owned server. A duplicate bind
  fails clearly. Ctrl+C, Stop and process shutdown cancel active work.
- Every inference and model-list request requires a locally generated client
  profile key, independent of upstream credentials. Accept Bearer authorization on
  OpenAI routes and `x-api-key` on Anthropic routes. Keys identify a local
  client profile and its selected binding; compare without logging the secret.
  Revocation rejects that profile's new requests; its active requests drain or
  cancel within the same stop bound. Other profiles keep working. An invalid
  key never reaches an upstream or any tool.
- Check Host against the bound loopback authority, reject browser-origin
  requests, and do not serve credentials/configuration over model HTTP. Local
  management remains a separate private CLI/GUI control path.

Initial engineering ceilings, to be measured before release: 8 concurrent
requests globally, at most 2 per account, a 16-entry admission queue with a
15-second wait, 16 MiB JSON request bodies, 1 MiB individual stream events,
32 MiB aggregate response bytes, 120-second upstream inactivity and a
10-minute total request deadline. Stop/drain is at most 5 seconds, then cancel.
Admission rejects before allocating beyond its bound. These are model-service
limits, not the worker's shorter exec timeout. Terminal rendering must not
wait on network, refresh, JSON parsing or admission work.

## Provider-pool v0: identity, login and selection

### Provider contract

Each registered provider declares its stable ID, service identity, upstream
base/path rules, available auth methods, model IDs and per-model capabilities.
Credentials are bound to that provider/service, not generic interchangeable
bearer strings. First delivery uses built-in typed adapters, not downloadable
plugins or executable configuration. Zhipu/GLM and Kimi are future adapters;
their OAuth methods are not presumed from another vendor's login UI.

An account has an opaque ID, provider/service ID, display label, credential
reference and state: `disconnected`, `authorizing`, `ready`, `refreshing`,
`needs_login`, `error`, or `disabled`. `ready` requires the intended inference
grant, not merely successful page navigation. Model/protocol incompatibility
is a route error, not a reason to mark every credential invalid.

For OAuth, verify authorize/token endpoints, actual client identity rules, scopes,
redirect or device-code rules, token audience, expiry and refresh/revocation
behavior according to the provider's real contract. Use state, PKCE S256 where the provider supports
the code flow, one-shot callbacks and a ten-minute cancellable login ceiling.
Each HTTP exchange retains its separate short timeout; the provider expiry can
shorten the absolute interactive deadline and never restart or extend it.
The device prompt shows the remaining effective deadline, capped by both the
local attempt and the provider expiry. The initial three-minute local ceiling
expired during the owner login journey; it was shorter than the official
DeepSeek client's [ten-minute default](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/credentials/deepseek-account-platform/src/index.ts#L37-L62).
Anonymous MiniCon `auth_init`/`auth_cancel` on 2026-10-25 independently observed
`expires_in=600`, HTTP 200 and successful cancellation; the local three-minute
cap had discarded a still-valid service window. The bounded local receipt is
`target-six/v0.2.23-provider/anonymous-auth-ttl-20261025.json`; no account grant
was obtained by that probe. The provider-filter gate passed 60 tests, with
one external-Pi court ignored. Removing the absolute-deadline minimum produced
a runtime assertion failure; byte-exact restoration passed again.
A retry creates new state/PKCE/callback ownership and reuses the existing browser
profile where the native adapter persists it. Proxy/incognito environments may
require signing in again. An expired callback never revives the old attempt.
An expired browser wait distinguishes no loopback request, an incomplete
request and rejected requests, using bounded counts without recording their
URL, state, code or provider body. Expiry/cancellation during a read or state
check remains a lifecycle failure, not a malformed-request count. Each expiry
offers a fresh-login instruction; none proves a successful grant. A generic
expiry alone cannot establish that no callback arrived.
The real-loopback regression
`provider_login::tests::callback_expiry_distinguishes_absent_incomplete_and_rejected_returns`
passed within the 13-test login-coordinator gate. Removing in-flight expiry
propagation made that regression fail at runtime; byte-exact restoration passed
again (`target-six/v0.2.23-provider/callback-expiry-guard-receipt.json`). These
are local failure-classification proofs, not provider authorization evidence.
The callback listener is separate from the model port and closes on timeout,
cancel or exit. Browser presentation stays in MiniCon's browser tab; an
unsupported embedded-browser flow is explicitly unavailable. Existing WebView
tests do not substitute for a provider-supported authorization contract.

For expiring grants, refresh only for an active authorized operation, coalesced per account, and
retry an expired credential once if the provider permits. Failed refresh
requires reconnect; no background refresh timer or automatic account switch.
DeepSeek's documented account grant is opaque until rejected and has no
expiry/refresh operation; do not invent one. OpenCode device grants have an
expiry and refresh token. A public client ID is an identifier, not a secret or
evidence that independent registration is required.
Late login/refresh completion cannot recreate a removed account. Disconnect
removes local credentials and attempts provider revocation where supported;
report local removal and remote revocation separately. A cancelled or failed
local removal releases its temporary admission block only if the account still
exists; it cannot recreate credentials or change the selected conversation.

Persist non-secret configuration separately from a private credential store,
using the platform's protected directory, bounded no-follow read and atomic
private-write mechanisms. This first delivery does not claim hardware-backed
storage. Logout, save failure, restart and migration must preserve isolation;
never import other agents' credential files automatically. Existing API-key
environment overrides remain explicit legacy inputs and are not silently
added as pooled OAuth accounts.

### Pool ownership and conversation binding

Account/profile ceilings, key lifecycle, account-specific proxies, aliases and
explicit allocations belong to [provider pool](PRD_02_31_01_02_provider_pool.md).
Each selected profile/model alias resolves to one `{provider, account, model}` binding.
Its generation also pins the selected catalogue route identity, native protocol,
endpoint, configured non-secret headers and account network-policy generation.
Changing that route invalidates old
conversations even when its wire model ID is unchanged; unrelated catalogue
edits do not invalidate the current route.
The model listing shows only compatible configured routes for that profile;
unknown models fail instead of choosing a default that sends data elsewhere.
Changing a profile affects new conversations. Existing conversations never silently migrate: the first connection rejects continuation after a changed binding; disabling/removing that account makes continuation fail with
reconnect/new-conversation guidance.

Recipes for qualified clients supply a stable session identifier, mapped to
a generated upstream session ID without forwarding arbitrary client headers.
For hosted OpenCode Go, send the provider-required session header through
the adapter. Never reuse the existing one-global-session constant for every
client. A client without a stable session signal is limited to a dedicated
fixed account profile; do not claim arbitrary continuation or live switching.
Routing metadata is process-local and capped at 256 entries globally, including
each profile's dedicated no-session slot. A full table refuses new conversations; it does not
evict old bindings and silently recreate their upstream sessions. Mappings
expire on process exit or that profile's client-key replacement, whose generation also changes.
No prompts or tool transcripts are persisted by the gateway. A restart loses
gateway session mappings and requires a new conversation for session-dependent
providers. The qualification recipe must expose this limit.

No automatic failover in v0: rate limits preserve the same account and return
retry guidance; ambiguous delivery, partial SSE output and tool results are
never transparently replayed. This keeps the first path deterministic while
leaving scheduling to the later llm-gate.

### Future llm-gate seam

Separate `ModelRequest`, `ModelEvent`, `ProviderAdapter` and a `RouteSelector`
contract. `RouteSelector` receives client/session identity, requested model,
required capabilities and redacted account availability; it returns a validated
account/model binding or a typed refusal. The current implementation is manual.
The host validates every returned binding against the client's allowed routes,
account state and capability matrix, including after future engine decisions.

Future ujs + tinyvm (wasm) can implement selection through a bounded host-call
adapter. The engine receives opaque account handles and metadata, not raw
tokens, arbitrary sockets, filesystem access or tool execution. Cancellation,
credential use, HTTP and leases remain host-owned. No runtime dependency,
wasm ABI, scheduler, engine packaging or external engine API is added now.

## Protocol scope and client qualification

| Surface | First delivery | Follow-on / explicit limit |
| --- | --- | --- |
| `GET /v1/models` | Authenticated configured model IDs with capability filtering | No claim that all upstream catalog entries work with both protocols |
| `POST /v1/chat/completions` | OpenAI text, role history, function tools/results, non-stream and SSE | Reject unsupported modalities/fields that change meaning |
| `POST /v1/messages` | Anthropic text blocks, system input, tool_use/tool_result, non-stream and SSE | Preserve the supported version/error/event shape; no fabricated token counts |
| `POST /v1/responses` | Not required for the initial connection slice | Required before claiming Codex compatibility; separate typed items, function and streaming evidence |

Translate only the advertised common capability subset. Preserve tool call IDs,
argument JSON, result linkage, ordering, stop/finish reasons, supported usage
and incremental SSE event framing. Unsupported semantic fields fail before
upstream dispatch; documented harmless metadata may be ignored. Usage absent
from the provider is absent/unsupported, not an invented exact count. Native
same-protocol forwarding is preferred where semantics match; translation must
not manufacture hidden reasoning or omit required continuation blocks.

When the provider/model cannot preserve required tool or reasoning state for
the requested client protocol, that route is unavailable. The initial scope is
text plus function tools, with bounded parallel tool-call descriptions; the
caller executes them. No multimodal, server-side conversation storage,
embeddings, audio, web search, batch or generic API proxy is promised.

The server propagates disconnect/cancel and bounds backpressure. Before SSE
headers it returns a protocol-shaped error; afterward it uses that protocol's
error/termination rules and never appends an unrelated JSON success. It must
not buffer the entire response to fake streaming. No automatic request replay.

Qualify **the built-in worker plus one named external client**, choosing a
local client with an actual configurable compatible endpoint (Pi is the first
candidate). Record the client version, configuration, provider/model, auth
method, protocol, text/tool cycle, streaming and cancellation observations.
Both protocol adapters need black-box evidence even if the first client uses
only one. Codex requires Responses evidence, and is a follow-on connection
target rather than a Chat Completions compatibility claim. Cursor IDE's
documented backend routing prevents assuming that its requests can reach this
machine's loopback listener; keep it unqualified. Identify a concrete Grok CLI
implementation before planning its adapter. Do not expose the server publicly
to make an unqualified client appear compatible.

## Authentication research and current implementation gaps

Research date: 2026-10-24. Working product interpretation is **DeepSeek model
API** and **hosted OpenCode Console/Go inference**, matching the existing
backends' intended role. The OpenCode agent's headless server is a different
agent/session API and is not a raw provider substitute. This interpretation
does not establish either vendor's OAuth support.

| Subject | Confirmed input | Consequence |
| --- | --- | --- |
| DeepSeek | [Official account implementation](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/credentials/deepseek-account-platform/src/index.ts) supplies auth_init/auth_exchange/cancel with PKCE; the separate API-key path uses Bearer | Real owner development GUI completed the supported grant, official catalogue and inference probe; final-artifact and worker qualification remain open |
| OpenCode | [Official device flow](https://github.com/anomalyco/opencode/blob/20a6cc72917586eb5314be075f96dbb21d16c480/packages/core/src/plugin/provider/opencode.ts) supplies device authorization and refresh; external API-key and MCP paths are distinct | Anonymous device initialization accepted client_id=minicon and returned a challenge; user grant and inference are still unqualified |
| OpenCode service identity | The same guide distinguishes hosted inference plans, native model protocols and stable Go session headers; free chat models are limited to OpenCode | Verify the chosen hosted endpoint, account entitlement and model, not a guessed local agent server |
| Codex | [Official custom provider configuration](https://developers.openai.com/codex/config-advanced/) includes Responses wire configuration | Chat Completions alone does not qualify this client |
| Cursor IDE | [Official API-key routing](https://cursor.com/help/models-and-usage/api-keys) sends requests through its backend | Loopback reachability is unproven; no first-delivery compatibility claim |
| Pi | [Official custom model configuration](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md) provides configurable provider APIs | A candidate for first external qualification, not completed evidence |

Anonymous probes created no logged-in account and did not exchange a token or
call inference. The DeepSeek anonymous attempt was cancelled. These observations
remove the earlier assumption that MiniCon must first obtain a separate
registration before even starting login; no official restriction supporting
that requirement was found. A client-name denylist would be an invented gate.

The DeepSeek account adapter must use its documented account inference header
and Anthropic endpoint, rather than relabeling the legacy Bearer codec as OAuth.
[Official account inference implementation](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/llm/llm-deepseek-account/src/index.ts)
owns the upstream protocol input; adapters still need real qualification here.

OAuth remains the requested account experience. Feasibility work must either
produce supported inference authorization or return a documented unavailable
result. Existing explicit API-key mode can validate transport while this is
open, but it does **not** complete the OAuth leaf or silently replace that
requirement. If both requested OAuth paths remain unavailable, report that
scope before choosing a release; do not label the desired connection delivered.

Source audit, not a new runtime receipt:

- `src/harness.rs` already runs file/exec and bounded persisted sessions;
  `harness_wire.rs` and `harness_opencode.rs` already contain provider codecs.
  The gateway composes `provider_protocol.rs` single-turn conversion and the
  shared streaming HTTP transport, while file/exec stays in the worker. Calling
  `run_task` inside the gateway would incorrectly transfer tool authority.
- The previous GUI DeepSeek prototype used unverified generic authorize/token endpoints and an independent callback/exchange lifetime. The product shortcut `Ctrl+Shift+D` now invokes the same Harness account-login coordinator as Settings; the old product bridge and detached exchange are retired. Shared minicon-web APIs remain available to their existing consumers. The direct legacy worker may still read an already-existing private bearer file (parent owner); new account grants are isolated in the provider store and never silently fed into that legacy codec.
- The legacy OpenCode worker codec retains its own direct-backend contract.
  The gateway instead resolves model identity and endpoint from the authorized
  official catalogue; historical direct API-key receipts cannot certify that
  separate hosted endpoint or OAuth.
- Worker source findings, limits and required repairs are owned by the
  [parent's worker requirements](PRD_02_31_v0_2_horizon.md#worker-requirements-for-the-first-gateway-connection).
  The shared API path depends on those cancellation, output, credential
  isolation and descendant-cleanup proofs, without moving tool authority here.

### Public observation of authorization pages

Actual authorization navigation and the interactive address bar retain their
necessary query values. Public CLI snapshots, tab listings, host notices and
OS window titles expose a fixed authorization title and only the known page
origin/path, with an explicit redaction marker. This covers current DeepSeek
and OpenCode authorization/completion pages, GitHub's `/login` authentication
namespace and the observed OpenCode GitHub callback, the new loopback callback
and known historical MiniCon callback paths. Nested SSO `return_to` and
`redirect_uri` parameters are not exposed through those observations.
Ordinary page query strings retain
their existing browser semantics. Device codes stay in the expiring interactive
prompt and explicit user clipboard action; neither codes nor grant/client keys
are included in public observation metadata.

### Scoped first-connection development evidence

- [v] The gateway worker preserves its own file/exec authority and refuses a changed resume binding against a controlled model peer.
  `minicon_harness::gateway_worker_runs_its_own_tools_and_refuses_changed_resume_binding`
  runs the actual native CLI through loopback HTTP, real file and exec tools,
  tool-result delivery, key exclusion from exec and persisted resume identity.
  Removing the binding guard made the changed-account invocation exit 0 rather
  than 2; the real CLI journey passed again after exact source restoration.
  This is a model-peer fixture, not OAuth or hosted-model inference evidence.
- The pinned real **Pi 1.4.0** CLI passed
  `provider_server::tests::external_pi_1_4_0_uses_real_gateway_sse_and_usage`:
  actual client -> actual Gateway -> controlled loopback upstream, final text
  `MINICON_PI_OK`, stop, input/output usage 7/3, total 10, exit 0. Package tag
  and npm provenance bind source `4a080252f06caf63a525d7873c5d94d3c23887e0`.
  This names client/protocol compatibility on Windows x86_64; its upstream was
  synthetic and it does not establish account authorization or the separate
  live tool/cancel courts. The explicitly ignored court fails BLOCKED when its
  pinned local dependency is unavailable.
- A Windows x86_64 private-console foreground probe drove the actual
  `provider serve` executable with synthetic stored-ready data, authenticated
  `/v1/models`, verified attachment to its isolated parent console, and sent
  native `CTRL_C_EVENT`. The process exited 0 in 0.063 seconds and its port
  could be rebound. Receipt `native-windows-private-console-CTRL_C_EVENT`
  identifies development bytes `a88240ae42accae661e9222cee62ac59cfe8b1274842aaafd24cd441ddb1def4`.
  This proves foreground stop/cleanup on that host, not a provider grant or
  upstream inference cancellation; the latter retains its separate courts.
- Actual DeepSeek GUI initialization reached the official authorization page
  after fixing host BCP47 locale conversion at the provider boundary. The prior
  build failed before transport with InvalidInput; the corrected native build
  initialized the flow and rendered the official origin/path. Public observations
  redact its query and fragment. The later account observation below owns the
  grant and inference result.

### Real account checkpoint and worker defects (2026-10-25)

- [v] DeepSeek grant, official model catalogue and actual inference probe passed
  in the owner development GUI. Named receipt:
  `target-six/v0.2.23-provider/real-deepseek-account.json`, source
  `ca51a04c4638395d1308962bdbf924b458dd8ec2`, Windows x86_64 binary SHA-256
  `1399e9e0d84836fda5b71a108376130ebb0254d195f8eda7b5915b497c00e952`.
  The normal official login button reused the signed-in browser session;
  no password, OTP or cookie import was involved. The account became ready
  after probing `deepseek/deepseek-flash`; the explicit local listener started.
  This is development evidence, not qualification of a final signed Candidate.
- [v] The pinned Pi 1.4.0 CLI also completed an actual DeepSeek inference
  through that GUI's local gateway, OpenAI Chat Completions SSE, exit 0 and
  final `MINICON_PI_REAL_OK`. Receipt:
  `target-six/v0.2.23-provider/real-deepseek-pi-1/receipt.json` records eight
  text deltas, stop and 69 input / 8 output / 77 total tokens. The local key
  was supplied by environment and absent from captured output. This extends
  the controlled-peer compatibility evidence to account-backed streaming;
  it does not qualify live tools/cancellation or final Candidate bytes.
- [v] Pi 1.4.0 subsequently completed real account-backed `read` and `write`
  calls: a fresh input nonce was written unchanged to the result file, both
  tool events reported success, and the client consumed SSE text and usage.
  The tool result is recorded in
  `target-six/v0.2.23-provider/pi-real-tool-cancel-20261025-034122/receipt.json`.
  That first combined court retained a `BLOCKED` cancellation-recovery result:
  its auxiliary stream ended during the recovery observation and could have
  released the available lease. A separate bounded cancel-only court kept the
  auxiliary stream active: Pi received real text, a third request established
  `429/account_busy`, RPC abort reached `aborted` in 15 ms, and a short request
  succeeded after 844 ms while the auxiliary stream was still active. The
  latter was then closed and all readers/client processes joined. Receipt:
  `target-six/v0.2.23-provider/pi-real-tool-cancel-20261025-034250/receipt.json`.
  Both courts bind to development GUI SHA-256
  `e830e773e5cb0acd3e86e98ef2c026854828ff8d221dd72a3e4167465824b474`,
  not final Candidate bytes. They establish the named client's real tool/SSE
  and local cancellation path; remote compute/billing termination is unproven.
- [v] An actual DeepSeek Anthropic Messages SSE tool round trip passed through
  that gateway: `tool_use`, a controlled local tool-result nonce, then exact
  nonce text with `end_turn`, two requests and preserved input/output usage.
  Receipt: `target-six/v0.2.23-provider/real-deepseek-anthropic-tool-1/receipt.json`.
  The gateway performed no tool execution; this is development account/protocol
  evidence, not final artifact or worker cleanup qualification.
- [v] Actual Chat Completions and Anthropic Messages streams occupied the two
  account leases, and a third request returned `429/account_busy`. After both
  real client sockets closed, short requests succeeded 797 ms and 1703 ms after
  disconnect, within the five-second observation window. Receipt:
  `target-six/v0.2.23-provider/real-stream-disconnect-20261025-032147/receipt.json`.
  This used development GUI binary `e830e773e5cb0acd3e86e98ef2c026854828ff8d221dd72a3e4167465824b474`;
  it proves local I/O/lease recovery, not remote compute/billing termination
  or final Candidate behavior. No request key or response body was recorded.
- [-] Actual worker tasks through that account did not qualify file+exec.
  `real-deepseek-worker-1` exhausted the model-turn budget after file access;
  `real-deepseek-worker-2` isolated a CMD working-directory failure. Both
  directories are under `target-six/v0.2.23-provider/`. Exit zero in the second
  invocation did not prove task success: the model reported inability to run
  the requested command and no result file was produced. The subsequent repair
  and successful ordinary-task checkpoint are owned by
  [the worker module](PRD_02_31_v0_2_horizon.md#account-backed-ordinary-worker-checkpoint-2026-10-25).
  Final-source/artifact qualification remains open.
- [-] OpenCode grant/inference and authorization-completion-page presentation
  remain unqualified. A bounded same-page Retry confirmed completion-page
  failure while the stored account stayed ready; the public generic reason
  cannot identify its cause. Login transport errors retain only the platform
  failure kind for diagnosis, never its potentially sensitive free-text detail.
  The OpenCode native initialization failure was traced by a real differential
  to the shared [Windows HTTP trust-root policy](PRD_02_28_shared_core.md);
  fixing that mechanism does not itself establish an OpenCode account grant.
  Preserve those boundaries while independent repairs run.
  The successful source `7694388` pack/signing checkpoint must not be promoted
  after these defects require a new source. PRD 27 owns matching final gates.

## Evidence and documentation closeout

Provider delivery rows remain `[-]` until account-backed and selected native
courts qualify them. Windows x86_64 development checks: the `provider_` filter
ran 60 passing tests and reported the separately invoked Pi court ignored;
the worker filter ran 36 passing tests; the HTTP filter ran 40 passing tests.
Four public Harness settings journeys and the public authorization-URL
redaction journey also passed.

Eleven guard removals each produced a real runtime assertion failure, followed
by exact byte restoration: model/tool budgets, exec environment, SSE error,
usage/content preservation, typed cancellation, CLI port-save confirmation,
failed/cancelled account-removal recovery, full selected-route identity and
effective device expiry. The latter four use
`provider_cli::tests::failed_port_save_never_starts_old_listener_and_same_port_needs_no_write`,
`provider_runtime::tests::removal_cancel_and_failure_restore_only_the_still_stored_account`,
`provider_server::tests::selected_full_route_identity_invalidates_binding_but_unselected_changes_do_not`
and `provider_login::tests::device_challenge_uses_the_actual_attempt_deadline_and_can_cancel_immediately`.
These are controlled mechanism/input fixtures, not real account authorization.
Unavailable provider authorization or native courts are `BLOCKED` for their exact claim. Register evidence and public
command identities only when implemented; design prose does not change machine
truth or advertise nonexistent commands.

Required acceptance: failed saves preserve state; listener conflict/stop/restart;
unauthorized requests have no upstream/tool effect; OAuth invalid state, timeout,
cancel, refresh and removal races; account isolation/manual binding; actual text,
tools, SSE and cancel for both protocols; worker global limits and descendant
cleanup; one real external client; native settings focus/secret redaction on
each claimed OS/ISA; bounded idle/active cost and artifact size. For new guards,
prove the corresponding public assertion fails when the guard is removed.
Use isolated synthetic credentials for mechanism courts and real supported
accounts only for separately identified provider receipts; never commit secrets
or prompt/tool transcripts. Qualification and publication remain with PRD 27/32.

### Native Harness input checkpoint

The unsigned Windows x86_64 pack/input checkpoint indexed by
[the controls owner](PRD_02_25_02_browser_controls.md#exact-unsigned-windows-native-input-checkpoint)
also observed physical Console/Web/Harness route changes, independent native
port/model drafts, unchanged saved binding before Submit, and explicit local
key creation/revocation with no key in public snapshots or receipts. Its
`harness-routes-fields-and-redaction` row passed; the overall court remains
BLOCKED on fixture clipboard restoration. It attempted no account login,
provider inference or listener activation, so the real-grant frontier above
is unchanged. Pixel review and final-source qualification remain separate.
