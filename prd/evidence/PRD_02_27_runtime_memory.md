# Evidence appendix: Runtime host memory

Owning PRD: `prd/PRD_02_27_con_delivery.md`, "Runtime host memory" section.
This file holds the long forensic/investigative narrative behind that
section's short `[v]`/`[-]` conclusion lines — exact byte accounting,
multi-round debugging trails, receipt/log paths, PE hashes. The conclusions
themselves (what is true, what evidence proves it) live in the owning PRD;
this file is detail, not an independent fact — do not restate a conclusion
here differently than the PRD states it.

## macOS idle ~300 MiB baseline attribution (2026-09-06)

`vmmap` + `heap -s` on an idle debug GUI (80×24): physical footprint
~243–252 MiB; `MALLOC_LARGE` dirty ~214 MiB. `heap` attributes that dirty
heap to three `std::fs::read` allocations inside MiniCon's linked font path.
On disk: `Apple Color Emoji.ttc` is 183 MiB, `Hiragino Sans GB.ttc` is
22 MiB, `SFNSMono.ttf` is 219 KiB. `PingFang.ttc` is listed as a fallback but
is absent at the recorded path on this host. The Unix portable raster
(`agenterm-platform` `adapters/unix/font_raster.rs`) `fs::read`s each
candidate and `Box::leak`s the whole file for process lifetime, and it loads
every fallback at first glyph — including the 183 MiB color-emoji collection
— before any emoji is drawn. Extra tabs adding ~1 MiB matches this: the leak
is process-global, not per session. The earlier ~19 MiB `gimli`/`RawVec`
attribution from inferred `heap -s` type names is not established: generic
allocator symbols can be coalesced, and release also had two ~9 MiB frame
allocations. Full allocation stacks and controlled removal own attribution.
AppKit/Metal mapped files are large in virtual size and mostly not dirty.
Owner of the leak is the shared Unix font raster, not MiniCon tab state. One
macOS heap is not a Linux or Windows claim (Windows GDI does not load whole
TTC files this way).

## Unix whole-font leak repair (shared revision `bb309e79`)

Shared revision `bb309e79bc351b314cec65ec24ba1bab0e74c1f4` (branch
`fix/lazy-font-mapping`). The raster owns read-only file mappings, borrows
font views for each lookup, and opens fallback candidates only after a glyph
miss. Failed opens are cached; ASCII leaves all fallbacks unopened, and CJK
produces visible pixels without opening emoji. Shared font-only tests: 39
PASS; isolated Clippy PASS. MiniCon pins the full shared SHA.

On 2026-09-06, osx-aarch64 debug, the same named host RSS court passed at
**94.53 MiB** idle / **109.67 MiB** after load. A second run with
`MINICON_RSS_DIAGNOSTICS_DIR=target/font-memory-evidence` captured vmmap and
heap from that court's exact idle GUI: **103.31 MiB** idle RSS, **28.9 MiB**
physical footprint, **9024 KiB** `MALLOC_LARGE` dirty, largest heap
allocation **9008 KiB**. No `Apple Color Emoji.ttc` mapping; Hiragino TTC
mapping: 22.4 MiB virtual, 224 KiB resident, zero dirty. The second run
loaded to 109.92 MiB, added at most 1.34 MiB for a tab, and grew 11.11 MiB
over four cycles. GUI multitab black box also PASS. Evidence and artifact
identity: `plan/archive/plan-lazy-font-memory.md`. The 384 MiB regression
ceiling is unchanged; 10 MiB remains unmet. These measurements supersede the
macOS baseline above, not Windows/Linux measurements or runtime
qualification of any other cell.

## macOS frame duplication and allocator retention (shared pin `8d8de88c`)

Shared pin `8d8de88c9ab62709327a6358609e9a09e8c363fd` owns anonymous mapped
softbuffer frames, released by the final CoreGraphics provider callback.
MiniCon rasterizes directly into transient frames, removing a second
8.79 MiB Retina canvas. Three alternating release comparisons isolated the
mapped frame change at 101.17 → 92.47 MiB median idle RSS; independent
product canvas removal measured 92.92 → 84.17 MiB. Host-copy count is now
zero. The one-file Mach-O embeds `assets/macos-info.plist`: macOS 26 design
compatibility preserves standard controls and measured 84.05 → 78.67 MiB
median settled idle RSS. This key is temporary and ignored by SDK 27+
builds; it does not resolve the long-term target.

Final native osx-aarch64 release court at the new pin passed at **86.89 MiB**
early idle / **85.08 MiB** after load, maximum tab delta **1.41 MiB**, four
cycles **2.33 MiB** growth; earlier integrated court idle was **77.75 MiB**.
Short startup samples vary by about one frame's size; retain both results
rather than selecting the favorable one or assigning an unproven cause.
Final idle vmmap: no `MALLOC_LARGE`, physical footprint 18.1 MiB (not the
product metric). GUI control black boxes, 131 MiniCon unit tests, mapped-
frame lifetime tests, Clippy and 32 MiB output qualification passed; final
throughput 21.18 MB/s, zero host copies/present failures. Exact artifacts,
commands, caveats and experiments: `plan/archive/plan-runtime-memory-next.md`.
The 10 MiB RSS target stays open, with the 384 MiB regression ceiling
unchanged.

## Two independent baseline/increment investigations

Native Cocoa-linked process 8.14 MiB, NSApplication init 26.72 MiB, standard
Hello window 49.53 MiB; editable control 83.22 MiB, Hello plus main menu
76.45 MiB, editable control plus menu 87.33 MiB (three-run medians, same
macOS host). Control construction itself causes the editable increment, not
proof of a single IME call's cost. Native Hello already has roughly 11 MiB
ColorSync tables; do not charge all of these to the terminal. MiniCon ASCII
plus CJK adds about 0.3 MiB, an extra tab about 1.3 MiB. This narrows the
remaining work to OS UI initialization and presentation while preserving
input/menu behavior; it is not an irreducible lower-bound claim or an excuse
to change the budget. Owners: `plan/archive/research-hello-memory.md`,
`plan/archive/research-minicon-memory.md`. Follow-up custom-input/pixel and
shared-host checkerboards, including application-policy controls, narrow the
remaining host attribution in `plan/archive/research-pixel-host.md`; these
are research probes, not product gates.

## Screenshot allocator-cache growth repair (shared main revision `745f52b2`)

Shared main revision `745f52b2e169d5b41b51377a82cf8a93a9b00c8b`, now pinned
by MiniCon. Real-product frame/provider tracing found bounded display-frame
ownership, while screenshot preparation and Unix PNG conversion created two
full-frame malloc buffers. After free, the original same-window receipt
retained two 9008 KiB regions, despite only 2320 B live-heap growth. A
mapped immutable worker snapshot plus one-row RGBA encoding eliminates those
large temporary malloc blocks. Two alternating baseline/fixed release
prototype journeys measured screenshot RSS increments 17.891/26.516 MiB
versus 0.469/0.297 MiB; fixed processes had no `MALLOC_LARGE` after
screenshot or final-tab close. Four PNGs decoded at 1920×1200. Exact
prototype hashes and full caveats: `plan/archive/research-frame-lifetime.md`.
Final pinned release
`8ce6e879898843f21a2d0a228a14e767e67025bd961a7448bbb909fe62a07480` passes
132 unit tests, public GUI/RSS courts, sustained output and Clippy. Named
final RSS court idle is 86.39 MiB, load 84.52 MiB; the final screenshot
journey has no large malloc regions and decodes 1920×1200. Commands and
exact receipts: `archive/research/screenshot-memory/`. This is not a startup
or 10 MiB PASS.

## Remaining RSS accounting (macOS)

A frozen macOS release receipt closes 79.265625 MiB resident =
18.046875 internal + 60.718750 external + 0.5 reusable. External is a
kernel pager classification, not proof that every page is a normal file or
can be freed. Another run's roughly 79→68 MiB decline coincides with
compression, not an accepted source optimization. Controls, native-stage
comparisons and observer limitations: `plan/archive/research-rss-ledger.md`.
All resident components remain in the product RSS criterion; OS
initialization and display residency remain open owners.

## AppKit Writing Tools startup load

Native `finishLaunching` probes Writing Tools support while customizing the
main menu, then loads WritingToolsUI. Public context-menu and text-view
opt-outs leave the 22.578 MiB external increment. An isolated private-method
negative control removes it. Actual frozen MiniCon with the same observer in
both arms measures idle 78.141 versus 60.500 MiB; both public GUI/RSS tests
pass. This one-pair private experiment is not enabled in production and
still exceeds 10 MiB. The compatibility choice is pending while independent
initialization/allocation work continues. Owner and exact receipts:
`plan/archive/research-external-residency-next.md`.

## osx/lnx/win same black-box court, per-cell detail

Native osx-aarch64 runs on the build host; Linux and Windows UTM guests
execute the exact host-linked debug artifacts via `scripts/rss-os-court.sh`
(`rss` mode on the existing `*-utm-runner.sh` / `*-runtime-qualify` pair).
One platform's idle figure still cannot become a six-cell claim.

Named so far: osx-aarch64 debug host idle 94.53–103.31 MiB after repair
(previously ~309 MiB; see above); win-aarch64 UTM debug GUI idle
**22.47 MiB** (`idle_bytes=23564288`, 2026-09-06,
`target-six/logs/rss-win-aarch64-utm.log`) — still above the 10 MiB intent,
an order of magnitude below macOS, consistent with GDI not `Box::leak`ing
whole TTC files. lnx-aarch64 UTM remains `BLOCKED` (QGA `file push` EXIT 124
/ transfer timeout); that is not a MiniCon RSS PASS.

Separately, this is **not** the lnx-aarch64 UTM cell above, still BLOCKED: a
lnx-x86_64 cloud container (Xvfb virtual framebuffer, no real display/GPU,
`cargo test` debug build, not six-cell's cross-compiled artifact) ran the
same court directly, 2026-09-26 — idle **21.46 MiB** (22,507,520 B), load
27.03 MiB, extra-tab delta 1.38 MiB, four-cycle growth 2.03 MiB
(`MINICON_HOST_RSS_RECEIPT` in the test's own stdout). This corrects a
2026-09-06 assumption in `plan/archive/plan-v0.2.1.md`/`plan/plan-carried-debt.md`
that this court "already fails here" for lack of a display: it runs and
passes cleanly under `xvfb-run`, and is close to but still above the
10 MiB intent (a debug build, not release-optimized).

**Release-build measurement, same lnx-x86_64 cloud container, 2026-09-26:**
`cargo build --release --bin minicon` then the same court against that
binary (`MINICON_TEST_BINARY` pointed at `target/release/minicon`) under
`xvfb-run` — idle **12.01 MiB** (12,591,104 B), load 17.55 MiB, extra-tab
delta 0.68 MiB, four-cycle growth 1.96 MiB (`MINICON_HOST_RSS_RECEIPT` in the
test's own stdout). This is within ~2 MiB of the 10 MiB intent on this cell
— closer than any other named cell's release figure — but still not a
six-cell lnx-aarch64 UTM receipt.

A separate **win-aarch64 release** court now names source `e6cd7b0`, shared
pin `745f52b2`, and PE SHA-256
`d2d08ce7600dfcc73b6b1002f73aef38bc9c4c28545198d403bafb96b4f47ecd`: idle
**21.46 MiB** (22,507,520 B), load 22.02 MiB, extra-tab delta 1.54 MiB,
four-cycle growth 9.13 MiB. Test body and job wrapper both return success
after keeping the harness process handle and reading its actual exit code;
null exit codes fail explicitly. Earlier coerced-null wrapper results are
not wrapper-success evidence. A hung job's 33,620 K working set is not this
idle measurement. Exact final log:
`target/windows-memory/rss-exitmeta.host.log` includes explicit
`HasExited=True` and `ExitCode_raw=0`; durable owner:
`plan/archive/research-windows-memory.md`. This release remains above
10 MiB, and neither this Windows cell nor macOS fills any unavailable Linux
cell.

### Windows working-set attribution detail

Latest same-PE attribution reads K32GetProcessMemoryInfo WS at 11:04:10.816Z,
.818Z and .829Z, before/after the page query and after classification: all
three are **22,495,232 B**. The page walk is 5484 × 4096 B =
**22,462,464 B**, partitioned as MEM_IMAGE 16,343,040 B, MEM_MAPPED
2,908,160 B and MEM_PRIVATE 3,211,264 B. Its sharing-axis split is sharable
18,186,240 B plus nonsharable 4,276,224 B. Those partitions close
internally; **32,768 B / 8 pages remain unexplained against PMC**. Earlier
erroneous claims of exact WS closure are withdrawn; historical 7/8-page
residuals and receipts remain in `plan/archive/research-windows-memory.md`.
All resident pages remain in the product WS budget.

The unnamed mapped bucket is 2,736,128 B. Its largest allocation base has
2,306,048 B resident (2.20 MiB), of which 2,154,496 B has ShareCount≥2.
GetMappedFileNameW returns ERROR_FILE_INVALID (1006) for these unnamed
mappings; that failure does not establish their owner. The largest block is
not yet a proven framebuffer. Exact receipt:
`target/windows-memory/idle-regions-5b71f8aacb33e7d0e15f63dc54e73814c8f487f9-20260906T110320Z.log.hostout`.
Mapping type and shareability are separate axes. `Shared` means sharable;
ShareCount reports sharing, capped at 7. COW protection flags alone do not
count already-private image/mapped pages, whose mapping type is retained.
References: [PSAPI_WORKING_SET_BLOCK](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-psapi_working_set_block),
[VirtualQueryEx](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualqueryex).

Frozen-PE size controls now establish a size-dependent mapped allocation:
with IME enabled, DPI 96, no screenshots and successful first presents,
fresh 480×300 and 960×600 clients have largest unnamed R/W mapped resident
blocks of **614,400 B** and **2,379,776 B** (3.87× for 4× pixel area).
Shrinking one process from 960×600 to 480×300 reduces that block to
**675,840 B**, releasing 1,703,936 B; the excess over fresh-small is only
61,440 B. This is evidence of size dependence and substantial reclamation,
not a proven DIB owner or a 2 MiB leak. MEM_PRIVATE residency increases only
491,520 B between the fresh sizes; aggregate privatized image pages remain
1,069,056 B. Their numerical equality to TextInputFramework's total
residency does not assign all those private pages to that module. Evidence
and controls: `archive/research/windows-memory/live-owners.md`,
`target/windows-memory/size-compare-91752f59296e6e2e4b2d718c2a97df69f83746bc-20260906T111300Z.log.hostout`.

Per-module counts now resolve that ambiguity: TextInputFramework has
**24,576 B** of private resident pages; the 1,069,056 B aggregate spans all
33 modules. External initialization sampling starts after an HWND already
exists, with TIF/MSCTF/CoreMessaging/imm32 already present. From that first
observation to first-frame confirmation, WS rises only **57,344 B**
(22,458,368 → 22,515,712 B). This does not measure process entry or
identify which earlier call loaded those modules, and does not prove that
supported deferral is impossible. Receipt:
`target/windows-memory/init-phases-25635c4f39397fa5151d4ef1376c1ba348924bbd-20260906T112057Z.log.hostout`.

Research-only in-process hooks (PE SHA-256
`4e7c330176c9c5a764e4a860724468aac2fe5bc1c10acafa853a0673fad056e0`, pin
`745f52b2` source copy plus tracing, IME enabled) narrow the interval:
host-run entry WS is 10,866,688 B; CreateWindowExW spans
10,928,128 → 11,882,496 B, with synchronous NCCREATE but no observed
WM_PAINT during creation. MSCTF appears there, TIF does not. IME
association adds 20,480 B without TIF. ApplicationOpened ends at
12,877,824 B without TIF; the first instrumented post-StretchDIBits sample
is 22,151,168 B with TIF/CoreMessaging/CoreUI present. The **9,273,344 B**
interval includes unmeasured show/focus/render work; one other
StretchDIBits path is uninstrumented. It does not yet attribute that growth
or module loading to StretchDIBits itself. Host-run entry is not process
entry. Research receipt:
`target/windows-memory/init-hooks-93a3dad769f8d41c48ec27e05ffbe9df76c4a468-20260906T113432Z.log`.

A subsequent research PE
(`f71f095ffcf7991be100f6b9338808577a6fc9d5142537959caa76b8b48021d9`) records
the earliest TIF presence transition at WM_IME_SETCONTEXT during
SetForegroundWindow; TIF is already loaded before show and pixel copy.
Skipping the explicit Focus command retains a presented window with TIF
absent and first-recorded-present WS 17,637,376 B versus 22,142,976 B
(difference **4,505,600 B / 4.296875 MiB**). Both variants, however, run
with `--no-activate` and `AGENTERM_NO_ACTIVATE=1`. This establishes a
nonactivated-startup opportunity, not a normal foreground idle saving; IME
enabled alone does not qualify actual Chinese input. Receipt:
`target/windows-memory/init-hooks-b7651f9da8d3dfd97311700a6168e59e64556c30-20260906T114058Z.log`.

Activation follow-up rejects this as a foreground idle optimization: the
research skip-focus process rises from 17,694,720 to 22,138,880 B on
successful activation, restoring **4,444,160 B** with TIF/CoreMessaging/
CoreUI. Physical-key injection delivers three WM_KEYDOWN and WM_CHAR events;
Chinese composition remains **BLOCKED** because the guest has only keyboard
layout 0x409. The normal-start control did not establish foreground
ownership and is not a foreground idle receipt. Research PE:
`c941be928c4ac38050aa6a4d345387962ee12cbd2185a2f9cd902a1c5d73580d`; log
`target/windows-memory/init-hooks-7a1453b9362739bb7f2255d6eb7685dad4ce9f1d-20260906T115253Z.log`.

A separate correctness fix makes the application startup focus request
honor `--no-activate` / `AGENTERM_NO_ACTIVATE`; normal startup and later
explicit user focus requests remain. This is not a foreground RSS saving.
Windows behavior validation of that fix passes for production-source PE
`083bcc802099be5d385460c151f043cef21af49ef6a8a7b1faf182269bfb3897`
(746,496 B, source `20e501b` containing `56207cb`, pin `745f52b2`, no
research skip variables). After first present, the window does not own
foreground; explicit activation succeeds, and real SendInput produces `abc`
in the terminal, read back through capture-pane. WS goes from 17,723,392 to
22,515,712 B on activation, confirming no foreground idle saving. Chinese
composition remains BLOCKED by the absent guest layout. Receipt:
`target/windows-memory/no-activate-behavior-20e501b8c90c5545b0aa0d5d036f27dce9def4cb-20260906T120633Z.log`.

The separately built `461b6cad…` PE has build evidence only; this runtime
receipt applies to `083bcc80…`. The production platform pin is unchanged;
the 10 MiB RSS target remains unmet. Continue with avoidable initialization
in the activated process.
