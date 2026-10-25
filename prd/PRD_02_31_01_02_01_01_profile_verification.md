# Whole-profile verification and application

Parent: [connection selection](PRD_02_31_01_02_01_connection_selection.md).
Sequence: [0.2.23 plan](../plan/plan-v0.2.23.md#harness-product-redesign-before-publication).
Status: `[-]` accepted CS-05 design enters production integration, 2026-10-25.
The complete preview is integrated and its initial controlled runtime gate
passes. Original-assertion controls and the zero-call/unknown-plan public consumer are
recorded below; paid-positive public and selected native qualification remain open.
Plain Apply is an independently verified prerequisite, not completion of this workflow.

This module owns the complete-profile paid-check quote, sequential execution,
progress/cancellation and the transition into existing atomic Apply. The parent
retains raw candidate/wire/proof identity and plain publication; the pool owner
retains account/profile/key lifecycle. Public schema belongs to PRD 26.

## Outcome tree DAG

```text
Outcome: verify exactly the displayed missing models, then apply the complete profile once
├── [-] QUOTE — one bounded Owner quote for current raw input and reusable evidence
│   invariant: visible N/order/scope equals the paid work authorized by the explicit click
│   evidence: BV-01 stale quote and same-N changed-authority refusal; zero-call plan
│   safe failure: show unavailable/stale and perform no inference
│   dependency: parent raw resolver and exact proofs; non-goal: deriving proof from account Ready
├── [-] RUN — one parent operation, one sequential existing check worker
│   invariant: accepted proofs survive a later failure and cannot add work to the original plan
│   evidence: BV-02 sparse two-account sequence, actual POST counts and deliberate retry
│   safe failure: stop on failure/expiry and retain drafts/proofs/old applied routes
│   dependency: QUOTE and existing transport/permit; non-goal: parallel paid checks or automatic retry
├── [-] INTENT — invalidate pending or active work before queue delivery
│   invariant: exact Cancel/Stop/affected data change prevents further unadmitted child requests
│   evidence: BV-03 pending-start, queue-full, between-child and value-ABA controls
│   safe failure: no revived permit or stale/future request cancellation
│   dependency: one bounded shared parent gate; non-goal: cancelling for caret-only activity
├── [-] COMMIT — enter one explicit cancellation cutoff, then reuse atomic Apply
│   invariant: CommitAdmitted differs from Published; complete state follows the typed write result
│   evidence: BV-04 pre-admission cancellation, write failure and published-warning matrix
│   safe failure: unpublished failure keeps old routing; published state is never called rolled back
│   dependency: RUN and parent Gateway-to-Store publication; non-goal: a second publisher or rename-time cancellation hook
├── [-] MERGE / HEALTH — retain later input and bounded local health-write warnings
│   invariant: final result has an applied revision; health retry cannot undo a published result or pay again
│   evidence: BV-05 later typing/selection, two warning accounts and stale local retry
│   safe failure: preserve local edits and distinguish stale authority from persistence failure
│   dependency: existing field-activity merge and readiness separation; non-goal: a tenth edit receipt/history
└── [-] PRESENT / QUALIFY — actual cost caption, progress, controls and public observations
    invariant: readable painted N/output limit and pointer admission describe the same quote
    evidence: BV-06 localized short/narrow viewports, public N=0 path and controlled/native journeys
    safe failure: unreadable/stale/pending input cannot start the batch; missing selected evidence is BLOCKED
    dependency: all preceding leaves and PRD 26; non-goal: claiming real entitlement from controlled HTTP
```

## Memory palace

```mermaid
flowchart TD
    RAW[Current raw form / exact authority / existing proofs] --> Q[Stable Owner quote and ordered missing slots]
    Q --> UI[Readable actual caption: N calls / possible charge / per-call output limit]
    UI --> P[Register pending parent before try_send]
    P --> G{Owner matches quote / revision / scope?}
    G -->|no| FAIL[Refuse / retain draft and prior proof / fresh quote]
    G -->|N=0| A[Existing plain Apply]
    G -->|N greater than 0| CHILD[Register one child permit under same parent gate]
    CHILD --> HTTP[Existing single-check admission / native HTTP / lease]
    HTTP --> PROOF[Accept real observation under current authority]
    PROOF -->|more originally planned slots| CHILD
    PROOF -->|all accepted| LOCK[Gateway then Store authority lock]
    LOCK --> ADMIT{Same gate: Active to CommitAdmitted?}
    ADMIT -->|cancelled / stale / expired| FAIL
    ADMIT -->|yes| WRITE[Existing preencoded private publisher]
    WRITE -->|not published| FAIL
    WRITE -->|published / durability warning| DONE[Applied revision / complete live state / local input merge]
    CHANGE[Exact Cancel / Stop / affected data intent] --> INVALID[Invalidate parent network cancel and registered permit]
    INVALID -->|before CommitAdmitted| FAIL
    INVALID -. after CommitAdmitted: refuse cancellation .-> WRITE
    PROOF --> HEALTH[Bounded parent-bound health retry set]
    HEALTH -. local persistence only .-> DONE
```

## Quote and cost contract

Each profile has at most one bounded private quote and a finite public plan
projection: an opaque plan ID, draft revision and ordered distinct paid slots.
It covers the complete current raw references, full candidate authorities/wires,
unchanged legacy references and reusable proof identities. IDs remain stable
until those meaningful inputs change; labels, focus and polling do not churn
quotes. The public plan exposes no grants, URLs, headers or raw organization.

The actual full-width localized action displays the exact number of calls,
possible charge and the parent's per-call output limit. Painter and pointer
routing share the same complete-caption fit test. Flush and acknowledge staged
input before accepting that click; pending/invalid input never defers an
automatic batch submission. The action carries the displayed plan ID, revision
and exact ordered paid-slot list. Owner rereads authority and rejects a changed
scope even when its call count stays equal. It never silently expands the quote.

A zero-call quote invokes existing plain Apply without inventing a check or
timestamp. Unchanged legacy provenance is governed solely by the parent; a
missing legacy account does not by itself manufacture new paid work or proof.
A new alias over a retained wire uses the parent's planned shared resolver
extension and requires its own actual inference evidence before Apply.

## Sequential execution and budgets

One explicit parent request/profile/revision/Stop epoch pins the initial plan,
its ordered sparse slots, accepted cursor and one 300-second total deadline.
There are at most 32 paid model checks, subject to the parent's allocation
bound. Each child reuses the parent's single-check deadline, decoded-response
budget including refresh, output cap, native transport, lease and one-shot
permit; the total deadline may shorten that child's allowance.

Only one worker exists at a time. Successful acceptance retains that exact proof
and advances a private cursor; it neither invalidates its own authorization nor
adds a slot when the current reusable-proof set changes. Any failure, expiry or
pre-admission cancellation stops subsequent work without partial profile Apply.
A later explicit retry obtains a new quote and pays only for still-missing
proofs. The logical batch remains active between children, excluding another
check/catalogue/login/Start. No new HTTP engine or request queue is introduced.

## Immediate intent and cancellation cutoff

Use one bounded shared gate for the current pending/active parent, its exact
identity, parent network-cancellation handle and currently registered child
permit. Runtime registers pending identity before enqueueing the batch; a failed
send withdraws only that same parent. Owner can activate only the matching,
unrevoked registration. This covers edits or Cancel between enqueue and Owner
start, before an active View has reached the GUI.

Exact Cancel must match this pending/active parent; stale and future IDs cannot
poison it. Stop and structurally valid affected-profile Edit/Choose/Remove/
Revert/Discard invalidate it before trying the existing queue, including on a
full queue. UI also invalidates the same gate as soon as actual raw input data
changes, before successful flush is possible. An A-to-B-to-A edit is revoked on
its first data change. Caret, selection, focus and another profile's edits do not
revoke it. Do not use the editor's broader activity counter as a content-change
signal.

Next-child registration and producer invalidation share the same gate lock.
Invalidation cancels the parent native network handle and the registered
one-shot child permit. A request already admitted by the existing transport CAS
may have incurred cost; cancellation cannot retract those admitted bytes.

After all checks succeed, existing Apply holds Gateway then Store authority.
Inside its prewrite callback, acquire the same parent gate, revalidate epoch,
cancel, revision, authority and deadline, and atomically change Active to
CommitAdmitted. This is the explicit cancellation cutoff, before invoking the
existing publisher; it is not a promise of a hook immediately before rename.
The visible Applying phase is not Published and no longer offers cancellation.
A late queued/stale-view Cancel is refused without setting the parent flag or
changing the eventual write result. An unpublished write failure remains Failed
with draft/proofs retained; a published durability warning advances complete
state. Never report rollback or Cancelled for the committed operation.

## Result merge and health persistence

A positive-call batch is a check operation, not a form command held in the nine
unresolved edit-receipt slots for its lifetime. Maintain one parent-bound,
nonblocking publication baseline containing the original input and field
activity sequence. A successful profile terminal carries an explicit
`applied_revision`; consume that exact parent/revision through existing later-
field merge before ordinary form synchronization. A rejected unrelated start
cannot clear someone else's baseline. Preserve later text, caret/selection and
all other profiles, including while the final save is being admitted.

Retain the existing active/last-terminal observation slots with finite scope,
progress and Applying/Applied state. Do not add child request histories or a
tenth edit receipt. Zero-call Apply uses the existing short Apply receipt.

Keep one parent-bound deduplicated pending health-write set, bounded by the
pool's account ceiling. Successful inference proof is independent of these
writes; retry performs local persistence only, with no new timestamp or paid
request. Authority failure is stale/failed health work, not a storage-success
warning. After profile publication, such a retry cannot rewrite the historical
Applied result into Failed or remove its applied revision. The published
connection and its current availability are distinct observations.

## Evidence frontier and joins

BV-01..06 remain open beyond the initial controlled checkpoint below. Required
consumers include exact
same-N changed-scope rejection, zero POST for N=0, sparse two-account ordering,
partial failure/proof reuse, pending-start and queued Cancel/Stop/data-edit
races, first value-ABA change, caret-only continuation, stored deadline expiry,
multiple readiness failures, final authority race, and publication failure versus
published-warning outcomes. Use the actual Owner/transport/private publisher;
newly introduced guards need their original assertion RED and exact restoration.

Public UI must observe the real button, caption/geometry, progress, cancellation,
final applied revision and unchanged unrelated drafts/keys. Controlled HTTP does
not establish real provider entitlement, physical input or release artifacts.
The current plain-Apply source is `eca1f1f4d282a31354e8bdfbe2148205c54aff30`;
its independent evidence remains in the parent. It cannot qualify changed CS-05 source.


### Initial implementation and controlled checkpoint

The frozen v1 preview changes selection, Runtime, Settings and connection-form
consumers and adds the product-private `provider_runtime/profile_batch.rs`. It
reuses the unchanged server/account seams. Root verifies all six code preimages
against both CS04 `eca1f1f` and current `0ac018e`, applies the exact five-path
patch and checks every postimage. Evidence:
`target-six/v0.2.23-provider/cs05-source-integration.json`. The preview retains
all 163 prior test names including the named opt-in ignore and adds 21
consumer subjects. Root fixed three serializer scope references and one test
initializer found by the actual compiler/Clippy gates; the failed logs remain
retained. An original test wrongly required a v2 file to remain byte-identical
after explicit Apply. Its corrected assertions preserve effective routing and
the complete Legacy evidence set, require no check/timestamp, and inspect raw
disk JSON before the migration decoder so omitted publication cannot pass.
These narrow repairs do not change paid-work or publication policy.

Actual current-source receipt:
`target-six/v0.2.23-provider/cs05-initial-gates-v5/receipt.json`
(SHA-256 `f34afdd852af554628f0313049006e80e2f7ac100e60b33e7d7e82cc5a767cd9`). It records format,
default and native-auth-feature all-target strict Clippy, 18 alignment tests,
all 21 new controlled subjects, and the broader 332-pass/one named opt-in Pi
ignore gate. Its seven code-path hashes remained stable. Earlier compiler and
fixture failures are retained under `cs05-initial-gates*`, not counted as guard
counterexamples. These are Windows developer tests with controlled HTTP/Owner,
not real provider entitlement, public GUI, physical input or release evidence.
Original-assertion guard removal with byte-exact restoration, public presentation
and the selected native/release joins remain required.

### Original-assertion controls and exact restoration

Current developer code is bound by
`target-six/v0.2.23-provider/cs05-source-controls-v2/receipt.json` and
`cs05-source-controls-adjudication.json` beside it. All 22 scoped controls cover
21 new subjects: 21 are single intended original-assertion failures. The remaining
Applying observation hits its exact original assertion before one source-bound
Disconnected cleanup-thread panic. Independent raw review is
`cs05-c09-adjudication-v2/receipt.json`; root accepts that primary assertion
separately without rewriting the strict receipt's INCOMPLETE result or claiming
22 single-panic failures or successful writer completion in the mutant.
Original, untagged compound and byte-exact restored suites each execute
183 PASS and one named opt-in Pi ignore. All seven original code paths restore
exactly and the index remains unchanged through this execution.

The earlier quote-ID-only mutant actually passed because stale account authority
was rejected by another guard. That passing mutant remains NOT RED in
`cs05-source-controls/c16/`. The same test now also supplies valid same-count
changed candidates and a current revision with the old quote identity; bypassing
the actual ID comparison fails its no-paid-work-reservation assertion. The raw
disk observer also fails when a writer falsely reports success without writing.
These test-only refinements preserve the original authority-refusal case and
production policy. Selected public GUI/native/provider and exact release joins
remain separate and open.

### Public zero-call and unknown-plan checkpoint

`target-six/v0.2.23-provider/cs05-final-gates/receipt.json`
(SHA-256 `cc231fb09dede08b707e3ddbfe8225e45ca427122a20c29ce200ca016bed7d21`) records current
format/default+native-auth strict Clippy, 18 alignment + 332 broader + two
public-worker Rust executions (352 PASS, one named Pi ignore), then an explicitly
built and pinned default development image. On those same bytes, the unchanged
CS04 public script passes 15 checks, the additional CS05 script passes six, and
the ordinary browser replay passes 22. Both profile replays close their owned
windows gracefully, remove fresh synthetic roots and retain expected private
state/key identity. This is base `0ac018e` plus the recorded dirty CS05 source,
not a clean-commit or signed release artifact claim.

The additional public consumer observes an exact N=0 quote across actual
760x420 and 420x300 frames, clicks the real whole-profile button and receives
plain Apply's exact acknowledgement without creating a check. It verifies raw
v3 publication with Legacy provenance and unchanged key, then edits to an
unverified missing model, clicks the unavailable plan and observes refusal with
no new receipt/publication; bounded idle polls do not retry. No network/billing
counter, real grant or paid-positive inference is claimed. The separate
`cs05-final-gates/visual-review.json` records actual image inspection of complete
N=0/unavailable button captions in English at scale1000. Broader readability
and the observed explanatory-row limitation remain with [HU-08](PRD_02_31_01_01_harness_experience.md#errors-diagnostics-and-acceptance).
Paid-positive public progress/cancellation, other languages/scales and physical
native/provider/release qualifications remain open.
