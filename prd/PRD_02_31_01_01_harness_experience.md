# Harness connection experience

Parent: [provider gateway](PRD_02_31_01_provider_gateway.md). Sequence:
[0.2.23 plan](../plan/plan-v0.2.23.md#harness-product-redesign-before-publication).
Status: `[-]` product redesign specified after owner feedback, 2026-10-25;
presentation has scoped native development checkpoints below; full implementation
and acceptance remain open. Existing gateway receipts do not
qualify this experience.

This module owns Harness information architecture, visible state, first use,
recovery and connection handoff. [Provider pool](PRD_02_31_01_02_provider_pool.md)
owns account/network/profile allocation; [monitoring](PRD_02_31_01_03_usage_monitoring.md)
owns statistics, balances and quota. The parent owns provider contracts, storage,
API semantics, limits and binding authority. The worker's tools and execution
remain with [PRD 31](PRD_02_31_v0_2_horizon.md#harness--detail).
The new [Harness workspace](PRD_02_36_harness_workspace.md) owns the robot entry
and operational large-screen view. This module owns its configuration forms;
both surfaces use the same account/profile/measurement state.

## Outcome tree DAG

```text
Outcome: manage an account pool and give each local tool a usable, observable connection
├── [-] ORIENT — one visible next action and a persistent current connection
│   invariant: authorization, model validation, local service and client use are distinct
│   evidence: HU-01 empty/partial/returning journeys without operator explanations
│   safe failure: name the incomplete step; preserve the last working connection
│   dependency: existing Harness route and typed runtime observations
│   non-goal: another settings window or duplicating the PRD 36 operation view
├── [_] AUTHORIZE — leave for the provider and return with an unambiguous result
│   invariant: only the current grant and successful private save establish connection
│   evidence: HU-02 success/expiry/cancel/retry/stale-result public journeys
│   safe failure: fresh retry; existing accounts and browser/terminal drafts survive
│   dependency: parent auth coordinator and existing Browser root
│   non-goal: reading cookies, DOM login detection or modifying provider pages
├── [_] CHOOSE — select an account/model and verify before applying a connection
│   invariant: draft, verified candidate and applied binding remain distinct
│   evidence: HU-03/04 paid-probe consent, failed save and active-binding change
│   safe failure: failed verification/save leaves the previous binding usable
│   dependency: pool client-profile allocation, catalogue and parent binding generation
│   non-goal: automatic model selection, fallback or account rotation
├── [_] CONNECT — enable the local endpoint and hand off a complete client recipe
│   invariant: explicit start, actual endpoint and separate key setup; no implied client success
│   evidence: HU-05/06 stopped/conflict/restart/key and real-client journeys
│   safe failure: retain the chosen port and give the failing step's recovery action
│   dependency: parent listener/key contracts and a versioned qualified client
│   non-goal: installing clients, editing their files or running tasks from Settings
└── [_] RECOVER — find and repair the affected connection without losing context
    invariant: errors belong to their operation; a stale error cannot overwrite later success
    evidence: HU-07/08/09 account lifecycle, narrow/native input and safe diagnostics
    safe failure: retain drafts, mask secrets and keep terminal/page input independent
    dependency: shared workspace controls and additive PRD 26 observations
    non-goal: invoice administration, transcript storage or changing provider billing settings
```

## Memory palace

```mermaid
flowchart TD
    H[Harness: pool summary and selected client next action] --> A[Connect account]
    A --> B[Existing Browser root: provider-controlled authorization]
    B --> G{Current grant received and privately saved?}
    G -->|yes| C[Account connected; choose client and model allocation]
    G -->|expired / cancelled / failed| E[Attempt-local recovery; keep existing accounts]
    E --> A
    OLD[Late result / old page / old timer] --> DROP[Cannot change current attempt or erase success]
    C --> D[Draft account and model]
    D --> V[Explicit small inference check]
    V -->|pass| SAVE[Apply connection atomically]
    V -->|fail| KEEP[Keep applied connection; repair draft]
    SAVE --> START[Explicitly enable local connection]
    START --> S{Listener started on chosen port?}
    S -->|no| PORT[Show conflict and chosen port; no silent migration]
    S -->|yes| R[Client recipe and separate local-key setup]
    R --> LC[Local service check: no paid inference]
    LC --> TASK[User runs built-in worker or qualified client]
    TASK --> OBS[Observed request result; client success only when actually proven]
    CUR[Selected client applied binding] --> KEEP
    CUR --> H
    POOL[PRD 31.01.02: account networks and keyed clients] --> CUR
    MET[PRD 31.01.03: usage / balance / allowance] --> H
    W[PRD 36: robot / operational workspace] -->|typed configuration deep link| H
    H -->|return with entity and filter preserved| W
    ADV[Advanced: port / protocols / key lifecycle / diagnostics] --> START
    STOP[Stop / revoke / exit] --> END[Parent bounded cancel and lease release]
```

## Problem and product decisions

Before the orientation delivery slice, `src/harness_settings_ui.rs` presented listener/port controls before
accounts, mixed a manually typed model ID with a long catalogue, and separated
the key from a command template that alone cannot connect a client. A shared
`busy`/status presentation does not tell the user which operation completed.
`provider_runtime.rs` can open a provider completion URL after credentials have
already been stored, making the host's separate successful result easy to miss.
The owner also observed a DeepSeek login-page timeout while a subsequent
account inference succeeded. That observation proves conflicting feedback,
not the cause of the provider page's error.

Use a guided connection experience in the existing Harness settings route.
Keep Console / Web / Harness as the top-level routes. Inside Harness use a
pool summary and expandable sections, rather than another row of navigation tabs
or a modal wizard. Settings remains useful after first use.

| Area, in display order | Product behavior |
| --- | --- |
| Pool overview | Account availability, local service status, attention states and selected client connection; distinguish pending setup from applied allocations |
| Account pool | Multiple accounts from the same provider, local labels, account-specific login/service network, authorization/model status and authoritative balance/allowance summaries |
| Clients and allocations | Each harness profile has its own key and model/account mappings; guided built-in/qualified-external/manual recipes use the selected profile |
| Usage and allowance | Local account/client/model totals, provider snapshot freshness and bounded monitoring controls; read-only metrics do not imply account rotation |
| Advanced | Port, exact protocol URLs, local-key lifecycle and bounded diagnostic details; opening this section performs no network or clipboard action |

On first use the summary shows three steps: **Connect an account -> Allocate and
verify a model for a client -> Connect that tool**. Resume at the first incomplete step after
returning from the browser or reopening Settings. After application, show the
connection summary and operational actions instead of restarting onboarding.
Closing Settings is never cancellation, sign-out or Stop.

```text
Console | Web | Harness

Model connections                                  Local API: Stopped
Accounts: 3 configured   Clients: 2 configured        [Enable local connection]
Next: verify the selected client allocation          [Review allocation]

Account pool
  DeepSeek A      Connected   Login: Direct   Balance: not refreshed
  DeepSeek B      Connected   Login: Proxy    Balance: not refreshed
  OpenCode Go C   Connected   Allowance: not refreshed
  [Connect account]                         [Account details / network]

Clients and allocations
  Built-in Harness -> DeepSeek A / chosen model        [Configure]
  Pi               -> DeepSeek B / chosen model        [Configure]
  [Add client profile]

Usage and allowance
  Today · local requests/tokens by account or client; missing usage labeled
  [Details]  [Refresh provider readings]  Monitoring: Off

> Advanced: port / protocol addresses / diagnostics
[Open Harness workspace]
```

This is an information-layout wireframe, not pixel acceptance. Interface text
follows the existing selected UI language, including errors and action labels;
provider names and model identifiers stay unchanged. Narrow layouts stack
actions with their labels and use the existing readable scrolling mechanism.
The pool summary, selected client connection and recovery action must be reachable
without scrolling through every model. Keyboard focus follows visible order;
Tab, Shift+Tab, Enter, Escape and native IME keep their workspace semantics.

## Settings layout and form design

### Orientation delivery slice (2026-10-26)

Implement ORIENT first in the existing scrolling form: show pool counts,
listener state, the selected **applied** connection and one next-step link
before account forms. Keep operation feedback visible in the overview as well
as at the local service controls. Provide direct links to accounts, allocations, monitoring
and Local API. Put port/start controls after the product forms. Navigation only
reveals existing controls; it never authorizes, probes, creates keys, starts the
listener or copies secrets. Loading/unavailable observations remain explicit.
A dirty draft cannot replace the applied summary. Cached account readiness is
not fresh inference evidence. Keyboard focus follows the visible links and
Enter/Space uses the same navigation action as pointer input.

This slice does not complete the expandable-section design or HU-01 through
HU-11 acceptance. Shared prerequisites are the existing runtime View, selected
form identity and row geometry; hot files are `harness_settings_ui.rs` and its
product-private orientation module. No shared platform mechanism changes.

Windows development evidence:
`harness_orientation_links_do_not_start_login_listener_or_key_creation`
drives the public CLI against a fresh-config, no-activate owned GUI. It checks
empty first use, keyboard next-step navigation, reverse monitoring/port traversal,
wrap to the overview, account/allocation/service links and reopen without login,
key creation or listener start. The empty built-in Default placeholder counts
as zero configured clients; the actual rendered PNG was visually reviewed.
`orientation_preserves_applied_identity_and_requires_verified_enabled_accounts`
checks the returning applied/draft distinction, account disable, cached readiness
versus model evidence and separate key/service/handoff stages. These add one
binary subject (870 total) and one control integration subject (35 total).

`target-six/v0.2.23-provider/orientation-controls/receipt.json` records both
passes, two actual production faults rejected by the original assertions,
byte-exact restoration and both restored passes. This is scoped development
feedback, not real-provider, physical-input, all-language/scale or final-artifact
acceptance. The following focused-form and cached-pool checkpoints record the
subsequent presentation joins; full native and real-account acceptance remains
open under ORIENT.

The global gear opens the existing anchored chrome panel. Keep the top-level
Console / Web / Harness routes. Harness begins with a compact setup/service
summary and four section links: **Account pool, Client allocations, Usage
monitoring, Local API**. These select a form section inside the same panel;
they do not add windows or a row of full application tabs. Diagnostics and
low-level identifiers are expandable details in their relevant section.

### Focused form sections (2026-10-26)

```text
Outcome: reach the intended Harness form without paging through unrelated controls
├── [-] Select one form: Accounts (initial), Clients, Monitoring or Local API
│   invariant: overview stays reachable; only the selected form owns presented rows and hits
│   evidence: public CLI first use, each section link and hidden-control refusal
│   safe failure: loading keeps the chosen section, with explicit unavailable state
│   dependency: existing row geometry and typed View; non-goal: another window or full-app tab bar
└── [-] Preserve working context across form changes and dashboard deep links
    invariant: retain account/client drafts, applied binding and service state; no implicit runtime action
    evidence: edit/leave/return plus delayed hydration and exact-entity deep links
    safe failure: unsaved edits remain editable; hidden editors relinquish input ownership
    dependency: existing form buffers and workspace targets; non-goal: saving or authorizing by navigation
```

The four overview links select a form body and reveal its heading. The Next
link opens the form containing the incomplete step. Console/Web/Harness route
return retains that selection. Runtime hydration must not change it or focus
an editor in a hidden section. A dashboard deep link explicitly selects the
destination form before revealing the account, allocation, monitor or service.
Changing sections blurs the previous input using existing editor semantics;
it does not discard committed draft text. Key-copy intent is invalidated by
leaving the client form. Shared prerequisites/hot files remain those of the
orientation slice; native courts must select the real form through visible
links, without weakening physical-input or cleanup guards.
Account label/login-proxy/service-proxy fields use a localized caption followed
by a full-width editor. Paint, hit testing, drag selection and native IME cursor
geometry use that same one-column row, rather than duplicating a clipped label
beside a half-width input. Public connect-draft observations expose label byte
count, caret and focus only, never the label or proxy value.

Windows development checkpoint: `harness_sections_preserve_drafts_and_hide_unselected_controls`
traverses every page of all four forms through the public CLI and rejects
controls from other sections. It edits model/port/account-name drafts, switches
forms and closes/reopens Settings, then checks retained values (the account
label is observed by byte count only), unchanged applied configuration and no
implicit login/start/key creation. The actual narrow-window editor uses the
published full-width cell; its synthetic PNG was visually reviewed.
`section_selection_survives_hydration_without_focusing_hidden_client_form`
checks delayed client publication, hidden account arrival/removal and exact
private draft buffers without a native window. Both are new subjects: default
Windows binary inventory 870 -> 871, control integration inventory 35 -> 36.

`target-six/v0.2.23-provider/section-controls/receipt.json` records two real
production faults (present unrelated rows; allow hidden client hydration to
take the viewport), both rejected by original assertions, exact restoration,
28 passing form-component subjects and 7 passing public Harness journeys,
all with zero ignored. Existing geometry subjects now explicitly select their
form before asserting the original paint/hit/visibility contracts. The native
fixture also selects the actual visible form before its unchanged guarded
physical input; only its Python syntax is checked here. This checkpoint does
not qualify a new physical, provider, language/scale or final artifact run.

```mermaid
flowchart LR
    O[Overview and Next action] --> S{Selected form}
    D[Dashboard entity deep link] --> S
    S --> A[Account pool]
    S --> C[Client allocations and key]
    S --> M[Usage monitoring]
    S --> L[Local API]
    A & C & M & L --> B[Existing private draft buffers]
    V[Asynchronous View hydration] --> B
    V -. cannot change selected form .-> S
    B --> X[Explicit login / verify / save / key / start controls]
```

Use a readable expanded panel up to 760 DIP, bounded by available host width
and height and anchored to the Settings entry. At least 640 available DIP
supports a section rail and form/detail area; below that use a section selector
and single stacked form. If the panel fills a short/narrow host, retain explicit
Back/Close and scrollable content; avoid horizontally clipped fixed-height rows.
These are proposed layout targets requiring actual native evidence.

#### Responsive panel and close control (2026-10-26)

```text
Outcome: read the Harness form at an appropriate width and leave it without losing work
├── [-] Window-bounded panel up to 760 DIP
│   invariant: draw, input hit targets and published rectangles share one layout
│   evidence: actual wide/narrow GUI dimensions, body input and route return
│   safe failure: clamp to available host area; retain readable rows and paid-action fit guard
│   dependency: existing Layout and fixed-row editor; non-goal: completing the section rail in this slice
└── [-] Persistent Close action
    invariant: closing the form preserves drafts and applied listener/account state
    evidence: explicit close/reopen with edited model/port and no runtime start/login
    safe failure: release the local pointer/edit owner; keep the existing underlying-view return contract
    dependency: same panel header geometry; non-goal: Stop, sign-out or cancel authorization
```

The panel stays below the existing host toolbar and expands to the available
window width up to760 DIP. Its header has the same Console/Web/Harness routes
and an explicit localized Close action, including short windows. Reserve its
space before computing route rectangles so small hosts cannot assign one hit
to both Close and a route. Paint, route dispatch and public observations consume
those rectangles. Existing row height, editor focus, scroll and paid-verification
caption fit checks stay authoritative. The proposed section rail and complete
multiline explanation layout remain selected follow-up work.
When a port draft is retained across explicit Start, the service line names the
actual bound address and the locked draft is labeled not active. A remembered
input value must not masquerade as the currently listening port.

```mermaid
flowchart LR
    W[Actual host size and scale] --> G[Harness panel/header/body geometry]
    G --> P[Paint]
    G --> H[Pointer hit and input anchor]
    G --> O[Public snapshot]
    H --> C[Explicit Close]
    C --> B[Blur/release local input; retain draft and runtime state]
    B --> R[Existing underlying workspace return]
```

Hot files: product `ui.rs`, Harness painter/input/snapshot and the existing
shared settings-route dispatcher/observation. No platform mechanism changes.

Windows development evidence:
`harness_panel_and_close_remain_bounded_and_do_not_share_route_hits` checks
window bounds and header-hit separation at300/420/640/1000 DIP and1/1.25/2
geometry scales. This is pure layout evidence, not native scale qualification.
`browser_windows::host_harness_responsive_panel_close_preserves_drafts_and_running_service`
uses a fresh synthetic account and local key with a no-activate GUI. It observes
the actual wide panel and narrow host, edits model/port drafts, explicitly
starts the loopback listener, then closes/reopens Settings. Authenticated
`GET /v1/models` succeeds while the form is closed, and explicit Stop/owned host
cleanup finish the journey. No upstream inference/metadata read is requested.
Its wide PNG was visually reviewed. The first local HTTP assertion used an
invalid Host and received the existing403 refusal; the corrected exact loopback
Host is used for the pass without changing origin validation.

`target-six/v0.2.23-provider/responsive-panel-controls/receipt.json` records
the two new subjects (binary 874 -> 875; control 38 -> 39), actual production
faults returning to 480-DIP width and making Close a no-op, both rejected by
original assertions, exact restoration, and 1 geometry /29 form /10 public
Harness passes with zero ignored. Final fmt/strict Clippy and the three-case
smoke are recorded in `responsive-panel-controls/final-receipt.json` under the
same local evidence root. Complete section navigation/reflow and original
native, real-account and final-artifact obligations remain open.
The existing development-court Harness selection retains this exact public
subject and log; YAML/shell syntax is checked locally without dispatching jobs.

#### Read-only explanation reflow (2026-10-26)

```text
Outcome: read the complete explanation before operating a narrow Harness form
└── [-] Width-aware read-only text rows
    invariant: one localized text owner; paint, paging, focus reveal and hits share the expanded row list
    evidence: complete bounded line coverage, public narrow/wide paging and unchanged editor/action state
    safe failure: unavailable glyph width remains explicit; continuation rows never dispatch actions
    dependency: existing font measurement and panel geometry; non-goal: changing editor or paid-action fit rules
```

```mermaid
flowchart LR
    T[Owned localized explanation] --> W[Actual font and available text width]
    W --> L[Expanded read-only line rows]
    L --> P[Paint and paging]
    L --> H[Hit and focus reveal]
    L --> O[Whitelisted line geometry; no private text]
    H -. continuation is inert .-> A[Existing explicit actions only]
```

Hot files are the product-private Harness text projection, row list and existing
paint/input/snapshot consumers. Reuse font measurement and the fixed row pager;
do not introduce a shared platform API or alter provider authority. Interactive
monitoring controls and mixed action rows retain their existing fit contracts.

Windows development subjects:
`explanation_lines_preserve_unicode_and_fit_the_actual_font_width` checks
English, simplified/traditional Chinese, mixed combining/emoji text, long URLs,
spaces and explicit line breaks against actual host-font measurement at three
widths. `browser_windows::host_harness_explanations_reflow_through_the_same_pager_and_inert_hits`
starts at the first page of the same Accounts form in 300-DIP and wide hosts,
collects every declared explanation line through the public pager, rejects
horizontal clipping, and clicks a continuation without starting an action or
changing applied state. Wide layout needs fewer lines. A model editor remains
visible and accepts further committed text after shrinking the window.
These add one binary subject (875 -> 876) and one control subject (40 -> 41).
Line observations contain source IDs, line numbers/counts and fit verdicts;
they never copy the private label, account/network text, code or credential.

The narrow Accounts PNG was visually reviewed. Provider sign-in buttons now
lead with the provider identity in short localized captions, so both columns
remain distinguishable instead of showing the same clipped "Continue with".
The initial public test incorrectly required wrapping in a wide form, then
compared different pager starting positions; those fixture assumptions were
corrected without weakening line completeness or inert-hit assertions.

`target-six/v0.2.23-provider/explanation-reflow-controls/receipt.json` records
two real faults rejected by the original subjects: replacing non-ASCII scalars
with question marks, and suppressing continuation expansion for usable widths.
Source bytes were restored exactly. Final restored execution and selected
checks are recorded by `final-receipt.json` in that directory. This is synthetic
Windows development evidence; other native OS/ISA, physical input, real
providers and signed-artifact qualification remain selected. The existing
development workflow retains the exact public subject for integration.

#### Persistent section navigation (2026-10-26)

```text
Outcome: change Harness forms from any scroll position without paging through unrelated controls
├── [-] Wide section rail and compact picker
│   invariant: navigation is outside the form scroll; only visible navigation owns hits
│   evidence: actual wide/narrow public journey, selected form and navigation/body geometry
│   safe failure: short/narrow hosts use the bounded picker; dismissing cannot click through
│   dependency: existing panel geometry and typed section actions; non-goal: new windows or application tabs
└── [-] Keyboard and resize continuity
    invariant: one input owner; rail/picker/fields/monitoring share visible focus order; committed drafts survive
    evidence: keyboard selection, hidden-input refusal, resize and exact section return
    safe failure: Escape dismisses the picker before closing Settings; unavailable targets stay explicit
    dependency: existing field editor and monitoring navigation; non-goal: implicit save/login/Refresh/Start
```

At 640 available panel DIP and sufficient height, show four persistent section
buttons in a 176-DIP rail. Other layouts show a persistent section picker above
the form. Its four options overlay the form only while open, and cannot own
hidden hits. Choosing an option uses the same section/deep-link action as before.
Body rows no longer repeat section links; their index/order is independent of
wide/narrow mode. Tab/Shift+Tab traverse visible navigation then editable fields
and monitoring controls. Arrow keys and Enter operate the compact picker;
Escape dismisses it and preserves drafts. Opening it relinquishes text/IME/key
copy ownership. Pointer dismissal consumes its press without triggering a form
control underneath. Resizing changes navigation presentation, not the selected
section or committed editor buffers.

```mermaid
flowchart LR
    G[Same panel geometry for paint / input / observations] --> M{Available width and height}
    M --> R[Persistent rail]
    M --> P[Compact picker]
    R & P --> S[Existing typed section action]
    S --> F[One selected scrolling form]
    K[Visible focus target order] --> R & P & F
    P --> D[Dismiss and consume; no hidden form action]
    RESIZE[Resize] --> G
    RESIZE -. retain section / draft buffers .-> F
```

Hot files: product panel geometry, Harness overview/painter/input/snapshot,
monitoring focus seam and the existing public/native fixture navigation.
No shared platform, provider or release authority changes. Full multiline
explanation reflow and native/language/real-account qualification remain selected.

Windows development evidence:
`browser_windows::host_harness_persistent_navigation_keeps_picker_input_and_drafts_local`
uses one fresh no-activate GUI with isolated configuration. It switches from
the wide rail to the compact selector, keeps four-option Tab traversal inside
the menu, refuses IME text to the hidden model field, selects Monitoring by
Down/Enter, dismisses with Escape while retaining Settings, and reopens by
keyboard. Clicking over the pager while the menu is open dismisses without
scrolling. Resizing an open menu back to the rail preserves the selected
section and committed model draft; listener, authorization and key creation
remain inactive. Wide rail and compact menu PNGs were visually reviewed.
This adds one control subject (39 -> 40), rather than a new suite.

`target-six/v0.2.23-provider/section-navigation-controls/receipt.json` records
two real production faults rejected by that original public assertion: Escape
leaves the menu open, and pointer dismissal falls through to the pager.
Both faults were restored byte-for-byte and the public journey passed again.
The subsequent selector disclosure marker is presentation-only; final affected
subjects and smoke are recorded separately in `final-receipt.json` in that
directory. This is local synthetic development evidence; native physical input,
other OS/ISA/language/scale qualification and real account flows remain open.
The existing native fixture uses published navigation rectangles; its safety
guards, deadlines and cleanup are unchanged. The development workflow retains
the exact public subject for its next deliberate integration checkpoint.

```text
Settings                                       [Open Harness] [Close]
Console | Web | Harness
Account setup 3 configured · Client setup 2 configured · Local API Stopped
┌───────────────────┬───────────────────────────────────────────────┐
│ Account pool      │ Accounts                         [Connect +] │
│ Client allocations│ DeepSeek A  Connected  Direct     [Details]  │
│ Usage monitoring  │ DeepSeek B  Connected  Proxy      [Details]  │
│ Local API         │ OpenCode C  Connected  Go         [Details]  │
│                   ├───────────────────────────────────────────────┤
│                   │ DeepSeek B                                    │
│                   │ Label: [Personal / Google account          ] │
│                   │ Login network:   [Saved proxy v]             │
│                   │ Service network: [Direct v]                  │
│                   │ Authorization: Connected   Last check: time  │
│                   │ Balance: last known value · freshness         │
│                   │ [Refresh reading] [Reconnect] [More actions]  │
├───────────────────┴───────────────────────────────────────────────┤
│ Unapplied network draft · affects future work [Revert] [Save]     │
└───────────────────────────────────────────────────────────────────┘
```

### Account pool form

#### Cached pool status and account readings (2026-10-26)

```text
Outcome: identify account setup and resource issues without mistaking cached evidence for live health
├── [-] Pool summary and account state
│   invariant: disabled/unknown policy, stored credentials and previously tested accounts remain distinct
│   evidence: mixed three-account projection and public isolated Settings journey
│   safe failure: loading/unavailable is not zero; cached tests never imply fresh inference
│   dependency: existing View/account preferences; non-goal: automatic account rotation
└── [-] Selected account readings and recovery link
    invariant: latest attempt cannot erase a labeled old reading; different providers/currencies are not summed
    evidence: stale success plus denied/new read, exact decimals, missing measurements and exact-account navigation
    safe failure: unknown is not zero; metadata failure does not revoke inference readiness
    dependency: monitoring View and existing report labels; non-goal: new provider requests or implicit Refresh
```

The overview shows configured account states and the number of accounts with
monitoring attention. Missing monitoring observations are counted separately.
Each account row says **Credentials stored**, **Previously tested**, **Disabled**
or **Policy unavailable**; none is a live-health claim. A selected account shows
applied login/service network modes, affected applied-client count, cached
resource freshness, latest attempt and retained readings with source/time.
Money keeps wallet kind and currency; Go shows consumed allowance percentage
and provider reset, never a cash balance. Empty/missing values remain unknown.
The account's monitoring link reveals that exact account in the existing form;
only the existing explicit Refresh/opt-in controls may start a metadata request.
No configuration draft, account label, proxy address or credential is copied
into the new public pool observation; it exposes aggregate counts and enums.

```mermaid
flowchart LR
    V[Existing account and preference View] --> P[Cached pool projection]
    M[Monitoring last success / latest attempt / freshness] --> P
    P --> O[Overview counts and account states]
    P --> D[Selected account network / client / resource details]
    D --> N[Exact account monitoring navigation]
    N --> E[Existing explicit Refresh or opt-in control]
    E --> R[Monitoring owner request admission]
    M -. never grants inference readiness .-> I[Independent applied model binding]
```

Product-private hot files are `harness_settings_ui.rs`, its overview/monitoring
projections, shared `harness_account_ui.rs` and the existing report timestamp helper. Reuse the
monitoring owner's freshness/attempt/attention observations; no new transport,
timer, persistence schema or shared OS/ISA mechanism is introduced. Full
real-account and native/language acceptance remains selected.

Windows development evidence:
`cached_pool_keeps_disabled_and_old_readings_separate_from_inference` covers a
mixed three-account pool, missing policy, provider-mismatched observations,
stale successful wallets retained after a denied metadata read, exact decimal
currency values and Go consumption above100% without cash conversion. It also
rejects loading-as-zero and fresh metadata-as-model-verification.
`browser_windows::host_harness_pool_states_and_exact_account_monitoring_preserve_bindings`
loads three isolated synthetic account records, observes typed aggregate states,
selects the second account and follows its monitoring link through the public
CLI. It retains the applied binding/service state and rejects label/proxy/token
disclosure in the new public observation. The account details PNG was visually
reviewed. These add one binary subject (871 -> 872) and one control subject
(36 -> 37); synthetic grants are not real provider evidence.

`target-six/v0.2.23-provider/pool-presentation-controls/receipt.json` records
two production faults rejected by original assertions (ignore disabled policy;
misdirect monitoring to the first account), exact restoration and passing
1 pool /28 form /9 monitoring /8 public Harness subjects, with zero ignored.
An initial unqualified `--exact` selector executed zero cases and is explicitly
not evidence; the corrected qualified subject and eight-case public run execute.
Final fmt/strict Clippy and daily smoke are recorded separately in
`pool-presentation-controls/final-receipt.json` under the same local evidence root.
This checkpoint does not close real DeepSeek/Go reads, physical input,
all-language/scale, dashboard projection or final artifact acceptance.
The subsequent Dashboard projection join, including the move of the original
Settings `pool.rs` to the common binary-private projection, is recorded by
[PRD36](PRD_02_36_harness_workspace.md#cached-account-and-client-projection-2026-10-26).

The list distinguishes same-provider accounts using local labels, provider,
short identity and connected/needs-login/disabled states. Selecting a row opens
its detail; clicking Details never starts a login. Show the applied login and
service network separately, with plain-language status and the account's affected
client count. Balance/allowance is a read-only snapshot with a labeled explicit
Refresh action; it is not edited as account configuration.

Connect account asks for provider and local label, then shows login-network
choice before Continue. The supported provider page owns the identity method,
including Google; MiniCon does not invent a separate Google OAuth adapter.
The owner's second DeepSeek account can choose a login proxy without changing
the first account's browser storage or ordinary Web proxy preferences. Auth
pending/success/expiry presentation follows the existing handoff section below.

Keep destructive/rare actions in an account-specific menu: Disable, Reconnect,
Sign out and remove. Selecting one names affected client allocations and active
work. A network edit shows Draft vs Applied values and can be reverted without
reconnecting. Saving a changed policy does not reroute active work or silently
start a new authorization attempt. Link failed login/network state directly to
the relevant setting; do not ask the user to diagnose a generic busy status.

### Account action menu (2026-10-26)

```text
Outcome: find rare account actions without accidentally applying them
└── [v] Account-scoped inline action menu (Windows x64 development evidence below)
    invariant: opening/dismissing is read-only; every action belongs to the named account
    evidence: public two-account menu switching, keyboard cancellation, draft-only toggle and removal-review join
    safe failure: missing account closes; busy/save/login refuses mutation; dirty reconnect requires save/revert
    dependency: saved account projection, applied clients, existing account draft/reconnect/removal owners
    non-goal: new logout transport, automatic save, reassignment or inference
```

```mermaid
flowchart LR
    A[Account Actions] --> M[Named account / affected applied clients]
    M --> D[Enable or disable draft]
    D --> S[Existing explicit Save]
    M --> R[Reconnect with saved settings]
    R --> G{No unapplied edits or pending save?}
    G -->|yes| L[Existing login owner]
    G -->|no| K[Keep draft; explain recovery]
    M --> V[Existing removal review]
    M --> C[Close or Escape; retain drafts]
```

Use an inline menu with full-width actions in the same readable pager. Default
focus is Close; Tab traverses only the open menu, and Escape closes it. Account
switches, section changes and settings close discard the menu. Show affected
applied client names privately. The available active-request count is explicitly
service-wide; account-specific live counts are unavailable and must not be
inferred from completed usage records. Public observations expose the opaque
menu account, not private labels or networks. The account list's Actions button
replaces its direct removal-review button; removal still needs its own explicit
review. Remove duplicate rare-action buttons from the normal detail form.

Development evidence (2026-10-26):
`account_menu_targets_named_draft_and_refuses_pending_or_dirty_reconnect`
exercises named-account targeting, pending-save refusal, unapplied enable/disable,
dirty reconnect, separate drafts and missing-account dismissal. It adds one
binary subject (877 -> 878). The existing public
`host_harness_account_removal_reviews_cancel_and_exact_confirmation` journey now
enters through Actions and additionally proves default Close, Escape/Enter
return, keyboard draft toggle, unchanged applied pool, cross-account draft
isolation, dirty reconnect refusal and explicit revert before removal review.
The first run used an incorrect fixture row ID for Revert; it failed before
removal and was corrected to the existing published `account_edit_actions` ID.
No product condition or deadline was relaxed.

Two production faults were rejected: allowing a draft toggle during pending
save fails the component assertion; binding the menu to the first account fails
the public exact-account observation before any provider action. Exact source
bytes were restored. Final evidence in
`target-six/v0.2.23-provider/account-action-menu/` contains both RED logs and
passing 2 component / 1 public menu-removal / 1 navigation / 3 daily smoke
subjects, plus fmt and strict all-target Clippy. The English menu screenshot
was visually inspected. This establishes Windows x64 synthetic interaction;
native platform/scale/language coverage, per-account live activity observation,
real-provider reconnect and the full experience acceptance remain open.

### Account removal review (2026-10-26)

```text
Outcome: deliberately remove the intended account after reviewing connection effects
└── [v] Account-scoped review before existing sign-out/remove action (Windows x64 development evidence below)
    invariant: first click never removes; confirmation belongs to the exact displayed account and affected applied profiles
    evidence: public cancel/close/section return and exact-account removal; component stale-review refusal
    safe failure: changed account/allocation requires a new review; busy/save-in-flight cannot confirm; Cancel is the default keyboard action
    dependency: existing RemoveAccount, account summaries/preferences and applied-profile projection
    non-goal: new remote logout API, paid verification, automatic reassignment or claiming remote revocation from local deletion
```

```mermaid
flowchart LR
    A[Account review action] --> R[Saved identity / affected applied profiles / cancellation effects]
    R --> C{Explicit confirm; identity and applied bindings unchanged?}
    C -->|yes| E[Existing bounded sign-out and removal]
    C -->|no or busy| K[Keep account; review again or wait]
    X[Cancel / Close / leave form] --> K
    V[Account / applied profile publication] --> C
    E --> O[Existing separate local / remote outcome]
```

Use readable inert explanation rows and separate full-width Cancel/Sign out and
remove actions. Retain applied profile names in the private rendered review;
public observations expose only opaque identity, counts and state. Revalidate
the account summary, saved preferences and affected applied allocation contents
at confirmation; unrelated usage updates do not invalidate it. Close/section
changes discard review intent and preserve ordinary drafts. Existing removal
and cancellation semantics stay with the pool/gateway owners. The broader
account-specific menu now delegates to this review, with its evidence owned
by the preceding section. Hot files are Harness presentation/input,
the shared account projection consumer and its existing public fixtures.

Development evidence (2026-10-26):
`harness_settings_ui::tests::account_removal_requires_current_applied_bindings_and_defaults_to_cancel`
rejects changed applied bindings/identity and pending operations/saves while
allowing unrelated status and unapplied draft changes. The public no-activate
`host_harness_account_removal_reviews_cancel_and_exact_confirmation` observes
three synthetic accounts, default Enter cancellation, Escape, section return,
Close/reopen, the exact affected-client count and explicit removal of only the
OpenCode fixture. OpenCode local removal uses no remote logout endpoint.
The rendered English review was inspected: wrapped account/effect explanations,
named affected client, distinct full-width actions and highlighted Cancel.

Both subjects passed after two production fault controls: removing the current
applied-binding comparison fails its assertion; dispatching removal on the first
click fails the public unchanged-account assertion. An earlier direct-replacement
fault failed compilation (`dead_code`) and is retained as invalid fault evidence.
Exact source bytes were restored before the successful final run. Logs and
receipts: `target-six/v0.2.23-provider/account-removal-review/`.
The affected persistent-navigation journey, three-case daily smoke, formatting
and strict all-target Clippy also passed. This is Windows x64 synthetic
development evidence; other native targets, real-account logout and signed
release qualification are not established by these checks. The full product
experience remains open; the subsequent account-menu integration has its own
evidence above.

### Client allocation form

The left list is client profiles, not provider accounts. New profile asks for a
local name and a qualified recipe or Manual configuration. It gets its own key;
a visual **Copy configuration** action never copies a different profile's key.

Profile detail contains **Connection**, **Model allocations**, and **Local key**:

- Connection: actual service status and the client-specific protocol/base URL;
  selected shell and a complete handoff recipe. A stopped URL is not presented
  as currently available.
- Allocations: editable rows for local alias, account, upstream catalogue model,
  relevant capabilities and exact-route verification state. One alias is marked
  default for the built-in recipe. **Add model**, **Verify**, **Apply allocation**
  are explicit; remove affects this profile and explains continuation rules.
- Local key lifecycle authority: [applied-profile keys](PRD_02_31_01_02_02_profile_keys.md).
  Created / Not created / Revoked plus explicit Create, Copy and
  Replace/Revoke actions. The key value stays masked. Show that sharing a key
  also shares route allocation and local usage attribution.

The action footer names the profile being edited and shows pending changes.
Selecting another account row must not mutate it. Failed check/save leaves the
previous applied connection visible. No automatic paid verification while typing,
scrolling, switching rows or reopening Settings.

### Monitoring form

Separate **Local usage recording** from **Provider balance/allowance refresh**.
Explain that local history covers MiniCon traffic only, and show retained coverage
and Clear local history as an explicit action. Account provider cards show which
measurements are supported, the last success, current freshness and any unavailable
authorization capability. Do not present unknown as a disabled-looking zero.

Expose the monitoring owner's planned opt-in switch, refresh interval and per-account
attention thresholds with their actual units; show any minimum interval validation
beside the field. Balance thresholds are currency-specific, quota thresholds
require a known allowance limit. **Refresh now** is a metadata action, not Test
model. **View usage** opens PRD 36 with the same account/profile/time filter.
All polling, retention and measurement facts remain owned by PRD 31.01.03.

### Local API form

Display actual stopped/starting/running/stopping/error state, configured port,
actual bound address and **Start / Stop**. While running, explain that port edits
require Stop. Below, show the protocol-specific addresses, links to each profile's
key/recipe, and expandable diagnostics. Do not place every profile's key controls
in this global service form. Stop names that it affects all local API profiles;
it is not confused with disabling one account or stopping one future task.

### Drafts, focus and feedback

Keep drafts per selected account/profile/section while switching Console/Web/
Harness or moving between Settings and the operational workspace. Returning
restores the entity, caret/selection and scroll where meaningful; IME preedit
follows the shared blur rules and is not silently submitted. The footer offers
Revert / Save for non-inference edits and Verify and apply for unverified model
allocations. It names the affected entity instead of offering an ambiguous global
Save all. Closing Settings retains unapplied in-memory drafts with a small
indicator; process exit does not auto-save them.

An asynchronous draft acknowledgement must preserve edits made after dispatch,
including clearing and retyping the same value. Track edit intent per field;
string equality alone cannot distinguish an untouched field from later input.
Changing an allocation's account also owns clearing its model selection, even
when the model text was already empty. Revert and model-choice results may
reset only fields that have not been edited since that request.

Returning to an existing connection form restores its last legal editable
field and stable allocation slot, with the retained caret/selection. Leaving
clears preedit and pointer selection gestures under the shared blur rules.
A removed slot cannot redirect focus to a different allocation. Keyboard
highlight in the profile chooser remains distinct from entering that form.

Keep applied alias/account/upstream-model rows visible beside the selected
draft's changes. A label or allocation count alone cannot explain which
connection is currently used. Account metadata failure must retain existing
forms and their applied references. A matching successful Discard result still
removes that unapplied form and restores a legal selection while metadata is
unavailable; an unrelated failure cannot clear other forms. These are required
draft-form behaviors, with implementation and public/native acceptance open.

Status feedback is inline and operation-scoped. Use a non-focus-stealing success
notice for Save/Copy and preserve a persistent result where it changes the setup
step. Copy failures say that copying failed, never Copied. Disable a control
only for its real dependency and show why. Group errors with the relevant field
and keep unrelated Stop/Back/Cancel controls usable. Account balance reads and
model verification must not share an unlabeled spinner.

## Visible state and primary action

Do not compress the following into one green `ready` badge. User labels are
projections of authoritative operations, not a replacement for their contracts.

| Dimension | Visible states | What completion establishes |
| --- | --- | --- |
| Authorization attempt | Waiting for authorization / Saving account / Connected / Cancelled / Expired / Could not connect | Connected means the intended grant was received and privately stored; page navigation alone establishes nothing |
| Selected client model | Not selected / Not checked / Checking / Check passed at a displayed time / Check failed | A successful bounded inference for that exact account, credential generation, route and relevant protocol; not every model on the account |
| Local service | Stopped / Starting / Running / Stopping / Could not start | The listener is actually bound at the displayed address; not that an external tool works |
| Client handoff | Setup incomplete / Configuration available / Local service check passed / Request observed | Configuration or an authenticated local check does not certify that the external client's task succeeded |

A check is historical evidence, not a promise of current quota or uptime.
Changed credentials, model or route invalidate that candidate's check. A
transient network failure does not itself invalidate the authorization grant.
Keep the parent's stricter usable-account requirement: authorization alone
does not admit inference before the selected model has passed its check.

| Current situation | Primary next action |
| --- | --- |
| No connected account | Connect account |
| Authorization pending | Continue authorization; Cancel remains available |
| Connected account, no model | Choose model |
| Unverified candidate | Verify and apply, with the inference/quota notice adjacent |
| Previously verified exact candidate, unapplied | Apply connection; no automatic repeat paid request |
| Applied usable connection, stopped service | Enable local connection |
| Service running, handoff incomplete | Configure the selected tool |
| Configured connection | Show connection details; Test again is a secondary explicit action |
| Failure | Repair the affected step; do not reset unrelated completed steps |

Busy state is scoped to its operation. Authorizing another account must not
hide Stop, block unrelated terminal input or replace a running connection's
status. A second simultaneous login is refused with Continue/Cancel guidance;
clicking another provider never silently replaces the current attempt.

## Authorization handoff and recovery

1. Connect account opens a small provider choice in the same route. Explain
   which hosted account is being connected. Only supported methods are offered.
2. Start a fresh bounded attempt and open its official authorization page in
   the existing Browser root. Show a host-owned progress strip and **Back to
   Harness** entry while that attempt is pending. Device-code flows also expose
   their effective countdown and explicit Copy code / Open authorization.
3. A grant received and successfully saved produces a persistent account result
   in Harness and a dismissible host notification. Do not steal focus, inject
   content into the provider page or force a return to Settings. If the provider
   contract requires final navigation, complete it in the original authorization
   surface; its page result cannot erase an already stored grant.
4. Return to Harness reveals the connected account and the model step. A paid
   model check is a separate deliberate action; receiving a grant does not
   silently spend inference quota.

The catalogue may load once after account storage as non-inference metadata
work, with a scoped loading/error state. It must not imply a paid model check,
hide the successful authorization result or repeatedly retry in the background.

The latest authoritative result for the current attempt wins. A timeout timer
or browser error after successful grant storage cannot change Connected into
Expired. A failed private save shows **Authorization received; could not save
account**, with a fresh-retry action and no usable-account claim. Do not retain
an unbounded secret-bearing attempt merely to offer a retry button.

An actual expiry says that **this authorization attempt expired** and offers
Retry; an existing connected account remains separately visible and unchanged.
Retry creates fresh state and callback/device ownership under the parent's
contract. Old timers, callbacks and results cannot complete the retry. Cancel
ends only the pending attempt. No automatic login retry, page-cookie import,
clipboard mutation or inference probe is needed to resolve a timeout message.

When a provider page and host result disagree, show the verifiable host result
and its next action: **Account connected; choose a model** or **Model check
passed** where proved. Do not explain the provider page's cause without evidence,
and do not tell the user to log in again just to clear a stale page message.

## Account and model selection

The [connection-selection owner](PRD_02_31_01_02_01_connection_selection.md)
defines catalogue/authority identity, draft revisions, exact verification,
atomic application and legacy migration. This section owns their presentation.

Account cards show provider, provider-supplied label, stable short ID, connection
state and the client allocations using that account. Only the expanded account shows
its catalogue, network, balance and secondary actions. Keep Connect another
account available. Custom local labels and multiple keyed client profiles are
selected scope under the pool owner; they are not deferred.

Use a bounded picker over that account's actual catalogue. Show a friendly name
where supplied and capabilities relevant to the selected client. Never invent
tool/streaming support or a recommended default from a model's name. Remember
a previously applied matching route as a draft; a new account does not inherit
another provider's model. Manual provider-qualified IDs remain an advanced
option and receive the same validation. An unsupported model is a route problem,
not an instruction to sign out every account.

Select the client profile being edited before choosing its account/model aliases.
Selecting an account or model changes that profile's draft only.
**Verify and apply** is one explicit operation: disclose the small inference request, validate the exact
candidate, then atomically save and apply the complete binding. A matching
successful check can be reused while its identity remains unchanged. Apply
failure retains both the user's draft and the previous working connection;
it must not partially switch account, model or local client profile, and must
not modify another profile's working allocation.

Before applying a different binding, explain the parent's conversation rule:
new conversations use the new connection, and old conversations cannot silently
continue on it. Do not cancel an already admitted stream merely because a
model row was highlighted. Account reconnection stages fresh credentials before
replacing stored credentials; failure preserves the existing record. Reuse an
account ID only when provider identity proves it is the same account, never
by display label alone.

## Local connection and client handoff

**Enable local connection** validates at least one usable applied profile and
starts the shared listener on the configured port. An explicit profile setup
action creates that profile's independent local key if missing; reuse its key
on later setup. Explain key creation before the first enable if combined in
one guided action. Other incomplete profiles stay unavailable individually.
Merely opening Settings, applying a model or copying a recipe never starts it.
If binding or save fails, do not start. If port binding fails after key creation,
show **Key created; service not running**, preserve the key and chosen port,
and offer Retry / Change port. Never choose another port or create another key
silently. Restart still starts stopped under the parent contract.

Client handoff cards select or create an independent keyed profile under the
pool owner. Show its aliases and actual account allocations; warn that copying
one profile key to another application shares both allocation and statistics.
Offer only clients with actual scoped compatibility evidence
as qualified choices; the manual card states that other clients are unverified.
Protocol compatibility alone does not qualify Codex, Cursor or an unnamed CLI.

Each card must supply a complete, versioned handoff:

- Required account/model capabilities and supported client version/protocol.
- The exact actual endpoint in the format that client accepts, including the
  correct base-path convention; a stopped endpoint is labeled as such.
- Separate local-key setup in the user's selected shell. Copy key is explicit,
  reports clipboard failure, and never inserts an upstream credential. Do not
  put key values in copied command templates, history or diagnostic exports.
- A copyable invocation/configuration with clearly marked user inputs. The
  built-in worker needs a project root, task and explicit allowed commands;
  no task execution or allow-list expansion occurs while configuring it.
- A **Check local service** action using authenticated local metadata/model
  routing only, with no provider inference or paid quota. This checks the service
  using the stored local key, not the key/environment/configuration of another
  client process. A separate real task verifies the client; do not present a copied template as successful use.
- Recovery for stopped listener, wrong key, unsupported model/protocol and
  changed conversation binding. Shared-key and restart limits stay visible
  where they affect the client recipe.

Settings does not install clients, write their configuration files, launch tasks
or introduce a background daemon. Future visual task operation is separately
owned and staged in PRD 36.
Advanced reveals editable port, both protocol URLs, explicit key revoke/replace
and diagnostic details. Stop remains readily available while running. Explain
actual active-request/conversation effects before destructive account removal
or key revocation, using the parent's bounded cancellation semantics.

```text
Connect a tool: Built-in Harness
Profile: Built-in Harness (independent local key)
Uses: alias work -> DeepSeek A / selected model
Local connection: Running · http://127.0.0.1:<actual-port>/v1

1  Shell: [PowerShell / bash]  Local key: Created       [Copy key]
   Set the named variable in this shell using the shown private-input steps.
2  Project: [path]  Task: [text]  Allowed commands: [explicit list]
   [Copy command]  (uses the key variable; never contains its value)
3  [Check local service]  Checks the service; no provider inference.
   Run the displayed task in that terminal to check model and tool use.

Other tool: [qualified client/version]   [Manual protocol configuration]
```

The wireframe's endpoint placeholder is replaced with the actual bound address
only when running. The shell recipe explains its process-local environment and
cleanup; it must not suggest that setting a variable in one shell configures
every terminal or GUI client. Missing project/task inputs remain explicit
placeholders and cannot be labeled as a runnable, complete task.

## Errors, diagnostics and acceptance

Put errors beside the affected action, with a plain-language result and one
useful recovery action. Keep optional typed details expandable. Do not leave
an old global footer error above a successfully recovered connection. Sign-out
reports local credential removal separately from provider revocation; unsupported
remote revocation is not described as successful remote logout. Rate limits
stay on the same account, expose retry guidance and never silently replay work.
Diagnostics exclude keys, authorization/device codes, callback query strings,
provider bodies, prompts and tool transcripts. Copying diagnostics is explicit.

Every row below is `[_]` until named evidence exists. Fixtures can prove state
and recovery; real grants, physical input and final artifact claims retain their
separate parent owners and may remain BLOCKED.

| Journey | Observable acceptance |
| --- | --- |
| HU-01 First use and return | Empty/partial/applied states expose the correct next action; no unexpected paid request, listener start, key creation or clipboard write on open/reopen |
| HU-02 Authorization result | Real grant/save success visible after browser return; expiry/cancel/save failure have distinct recovery; old timer/page/result cannot overwrite success or another attempt |
| HU-03 Model choice | Actual catalogue, manual-ID refusal, per-client capability display and explicit probe consent; an account-level success does not certify untested routes |
| HU-04 Apply and recover | Draft differs visibly from applied binding; failed probe/save preserves old connection; changed binding gives the specified conversation guidance |
| HU-05 Endpoint lifecycle | Explicit enable, missing/existing key, occupied port, key-copy failure, Stop and restart produce truthful states and bounded cleanup |
| HU-06 Complete client handoff | Fresh profile with real account -> built-in file/exec task and named external client's tool/SSE/cancel journey, using only the displayed recipe and no hidden operator configuration |
| HU-07 Account lifecycle | Same-provider labels/network contexts and per-client allocations remain distinguishable; reconnect/remove races and local/remote logout outcomes shown accurately; PP-01..06 own the pool proofs |
| HU-08 Native usability | Selected languages, normal/narrow/short viewports, keyboard traversal, native IME and focus return; unchanged Console/Web/terminal drafts; actual claimed scales and platforms named |
| HU-09 Feedback and redaction | Errors clear only when their operation recovers; unrelated failures do not hide running status; snapshots/diagnostics/copies preserve the documented secret boundaries |
| HU-10 Settings forms | Account/profile/monitor/service sections retain entity-scoped drafts, keyboard order, readable narrow forms and correct save/verify/copy feedback; no hidden network or inference on navigation |
| HU-11 Operational handoff | Robot workspace <-> Settings deep links preserve entity/filter/return focus and show the same applied configuration, active counts and measurement freshness |

Pool and metric acceptance stay with PP-01..06 and UM-01..06 in their owners.
The product gate includes a continuous journey:
connect -> verify/apply -> explicitly enable -> configure a tool -> complete a
real tool task -> stop -> reopen and recover. Observe state, actual effects and
pixels. Prove new critical guards with runtime counterexamples; a passing layout
fixture alone does not certify usability or authorization.

### Scoped profile-verification visual observation

The [whole-profile checkpoint](PRD_02_31_01_02_01_01_profile_verification.md#public-zero-call-and-unknown-plan-checkpoint)
records complete N=0/unavailable action captions. Separate actual image
inspection at English/scale1000 also observes the surrounding long verification
explanation truncated in the narrow Settings pane. Receipt:
`target-six/v0.2.23-provider/cs05-final-gates/visual-review.json`.
HU-08 remains open: the complete explanatory text must be readable through a
bounded layout/wrapping treatment before claiming overall narrow-view usability.
The button observation is not human-owner acceptance or physical-input evidence.

### Selected-profile key consumer candidate

The [key lifecycle owner](PRD_02_31_01_02_02_profile_keys.md#k2-selected-applied-profile-consumer-candidate)
owns K2 applied targeting, immediate Copy-intent invalidation, actual clipboard
feedback and worker/Pi handoff semantics. Its integrated controlled-consumer
checkpoint is recorded there; source controls and public/native handoff remain
open. It does not complete HU-05/06/08/09 or native/client qualification.

### Independent service editing during asynchronous publication

Service port editing has its own input owner. Initial or changed client-profile
metadata must not steal that focus, move its caret/selection, cancel its preedit
or jump its viewport. An unrelated View publication with the same applied port
must not reset a selected port value merely because its bytes still equal the
applied value. A changed applied port may update an untouched, unfocused field;
an active or dirty draft remains the user's draft. Failed Save preserves both
the previous applied port and the editable correction; retry remains explicit.
The existing typed View, field editor and connection forms are prerequisites;
the sole product hot file is `src/harness_settings_ui.rs`. Deterministic actual
consumer regressions precede the fix, followed by the original public failed-
save/retry journey. This does not change storage publication, provider authority,
model verification or accepted profile state.

The two actual consumer subjects are
`independent_port_focus_survives_profile_hydration` and
`independent_port_selection_survives_unrelated_view_publication`.
`target-six/v0.2.23-provider/port-publication-controls/receipt.json` records
their two baseline passes, each intended original assertion failure after its
production guard is removed, byte-exact source restoration and two restored
passes. These add two subjects to the Windows binary suite (869 total).
The original public failed-save/draft/retry journey also passes both Windows
architectures in [run40243448527](https://github.com/partnernetsoftware/minicon/actions/runs/40243448527)
at `45a5165be999e3aed40cf806fe48f2a55f1e731c`, with Loaded-state setup and
the actual corrected port checked before Submit. That earlier-source result
does not qualify the subsequent focus/selection fix or physical input.

The integrated fix is qualified for development on both Windows architectures
by [run40244714076](https://github.com/partnernetsoftware/minicon/actions/runs/40244714076)
at `fc26f165396b10b3b733c2b84dc863d213c75e57`. Each target's23 selected
Harness suites reports success. A log/receipt audit verifies both new consumer
subjects, the existing profile-return/navigation subject, the original public
failed-save/draft/retry journey and the public H0 destination journey actually
execute and pass on each target; repeated subjects in separate selected suites
retain their actual execution counts. Audit:
`target-six/v0.2.23-provider/harness-ci-fc26f16/verification-index.json`.
This closes these two editing regressions at that source; it does not claim
physical clipboard/IME input, another scale or final signed-artifact acceptance.

### Hosted physical Harness fields and redaction checkpoint

The exact `d2a67eb` Windows x86_64 physical court passes actual pointer/keyboard
port and model draft editing, Console/Web/Harness route return with those
drafts retained, temporary local-key creation/revocation and public snapshot
redaction. It applies no model binding, performs no provider login/inference
and does not claim a physical Copy-key action. The model-field and key-status
PNGs have been visually inspected; no key bytes are shown. Source/image,
actual1000-permille geometry, exact clipboard/GUI/thread/profile cleanup and the
still-blocked ARM64 scope belong to
[PRD32](PRD_02_32_dev_loop_crosscheck.md#hosted-native-input-completion-and-remaining-desktop-allocation).
Clipped explanation rows and technical draft-status wording observed in that
fixture remain open presentation work; native field success does not close
the complete first-use/readability acceptance gate.

### Native development join for panel, navigation and explanations (2026-10-26)

The original failed canonical-fixture run and its repaired consumers belong to
[PRD32](PRD_02_32_dev_loop_crosscheck.md#harness-presentation-integration-checkpoint-2026-10-26).
The following development evidence extends the local Windows checkpoints above;
it does not complete HU-08 or the continuous real-provider first-use journey.

| Source / run | Native execution scope | Observed result |
| --- | --- | --- |
| `f60e216`, run `40277134811` | Linux x86_64 / ARM64, Windows ARM64, macOS ARM64 | Four successful jobs;31 /31 /29 /27 selected suite results respectively |
| Same source / run | macOS x86_64 | All27 selected suites pass in emitted stdout; job fails only at CreateArtifact upload. Original nested artifact files/PNGs are `BLOCKED`, not retrieved or visually reviewed |
| `189b74c`, run `40278242091` | Windows x86_64 after its separate key-fixture correction | Successful job;29 selected suites and the original12 isolated-clipboard key-handoff checks pass |

Each platform actually executes the65 Settings-component subjects with zero
failed/ignored, the Unicode/font-width subject, and the named public responsive
panel, persistent navigation and explanation-reflow journeys. Those three public
journeys execute twice in the current schedule; repeated execution is not a count
of distinct tests. Canonical browser host checks pass22 cases on Windows/Linux
and23 on macOS. Receipts and raw job logs were inspected, rather than inferring
test execution from compilation or job colour.

The four archived-artifact cell audits are
`target-six/v0.2.23-provider/harness-ci-f60e216/selected-verification-index.json`.
The macOS x86_64 stdout payload recovery and its explicit unavailable-file scope
are `harness-ci-f60e216/osx-x86_64-stdout-recovered/recovery-receipt.json` under
the same evidence root. The corrected Windows x86_64 audit is
`harness-ci-189b74c/verification-index.json`. Source IDs remain separate: the
repair changes a fixture, its log upload list and documentation; product Rust
does not change. These receipts are not one exact-source signed Candidate.

Public logical pointer/keyboard/IME injection is not native physical input.
These default-language development journeys do not qualify every selected
language/scale, real DeepSeek/Go grants or reads, Pi/tool use, the blocked ARM64
physical Pinyin path, or final signed/downloaded artifacts. Those obligations
remain selected in their original owners and the 0.2.23 plan.
