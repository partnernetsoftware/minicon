# Enroll a company Azure tenant for Artifact Signing (redacted)

Status: company Artifact Signing account created; Public organization identity
validation **Completed** and the MiniCon Public Trust profile became Active on
2026-09-03. GitHub OIDC/profile-scoped signer configuration exists; live
non-promotable signing qualification passed in all six native cells. No credentials,
tenant/subscription/object IDs, mailboxes, addresses, validation IDs, or
payment details are recorded here. Publisher identity and SignPath remain as in
`CODE_SIGNING_POLICY.md`.

Goal: a **work** Microsoft Entra tenant plus an Azure subscription, then
**Azure Artifact Signing** (portal still often labelled Trusted Signing) for
company Authenticode. Not Microsoft 365 as a product we intend to keep.

## Do not do these

- Do not finish `signup.live.com` / “创建你的 Microsoft 帐户”. That is a
  **personal** MSA (`identity provider live.com`), even if the address looks
  like a company domain.
- On Azure login, **创建一个** usually dumps into `signup.live.com`. Same trap.
- `signup.microsoft.com` → **Sign up for Azure** currently lands on the Azure
  marketing site, then the same personal funnel. Skip it.
- Left column **For Personal use** on `signup.microsoft.com`: skip.
- Closing an MSA to free an address: Microsoft holds the address until the
  stated close date (often ~60 days). Logging in during that window **cancels**
  the close. The address cannot become a work user until close completes.

`AADSTS50020` with `identity provider 'live.com'` and tenant `Microsoft
Services` means the address is MSA, not Entra work. Switching directory will
not fix it.

## What actually created the company tenant

1. Open `https://signup.microsoft.com/` (must stay on `signup.microsoft.com`,
   not `signup.live.com`).
2. **For work or school** → **Sign up for Microsoft 365** (this is the path
   that creates an Entra **organization** tenant).
3. Trial page showed **Business Standard – Trial**: unit price was
   **per user per year**, not per month; **Payment due today $0**; one-month
   trial then yearly if not cancelled. Quantity 1.
4. Sign-in email: a **fresh** company mailbox that is **not** already an MSA
   (example placeholder: `business@example.com`). Do not pick a pending-close
   personal account (placeholder: `user@example.com`).
5. Complete MFA at `mysignins.microsoft.com` if prompted.

M365 here is only the tenant factory. Cancel the Office trial before it
converts to paid if the only need is Azure signing.

## After the tenant exists: signing pages

Log into Azure with the **work** account (not the MSA):

1. `https://portal.azure.com`
   Confirm the account is work/school. If the portal asks for a personal
   account or shows `live.com`, you are in the wrong identity.
2. Subscriptions (create PAYG if the new tenant has zero Azure
   subscriptions — an M365 trial does **not** by itself create Azure spend
   credit):
   `https://portal.azure.com/#view/Microsoft_Azure_Billing/SubscriptionsBladeV2`
   Use **+ 添加** → **Pay as you go / 即用即付**. Do not pick Enterprise
   Agreement unless this tenant is already an EA Account Owner.
3. Create the signing resource (search either name):
   **Trusted Signing Account** / **Artifact Signing**
   Product page: `https://azure.microsoft.com/en-us/products/artifact-signing/`
   Quickstart: `https://learn.microsoft.com/azure/trusted-signing/quickstart`
4. Identity validation: **Organization** (company legal name). Not Individual.
5. After validation: **Certificate profile** (Public Trust for Authenticode).
6. Feed Account URI + profile name into MiniCon/AgenTerm signing workflows.
   Keys stay in Azure; do not export to a USB EV token.

## Proven enrollment path

The working order matters:

```text
work Entra identity + Azure subscription
└── Artifact Signing account · East US · Basic
    ├── accept current Artifact Signing terms
    ├── account-scope RBAC
    │   └── Artifact Signing Identity Verifier → human verifier
    ├── Public → Organization identity validation
    │   ├── vetting email to the primary mailbox → click within 7 days
    │   └── Completed when every vetting sub-service is Pass
    ├── [x] Public Trust certificate profile (one per publisher identity)
    ├── [x] signer workload identity (GitHub OIDC) gets only profile-signer RBAC
    ├── [x] company-signing.yml adapter + release-signing Environment
    └── [x] exact unsigned SHA → signed SHA → timestamp → six execute-only courts
```

- The Chinese portal warning translated the role name, but role search returned
  zero results for the translated strings. Search the exact English built-in
  role name `Artifact Signing Identity Verifier`.
- Assign identity-verifier RBAC at the signing-account scope, not the whole
  subscription. Role propagation can take several minutes; refresh the identity
  validation blade until **New identity** becomes enabled.
- Choose **Public**, then **Organization**. Treat the legal name, business
  identifier, registered address, contact mailboxes and requester identity as
  sensitive input. Never copy their values into logs, receipts, PRD, prompts or
  screenshots committed to Git.
- Submission success is only `In progress`; it is not certificate approval.
  Do not create or claim a usable Public Trust profile until Microsoft marks the
  organization validation successful.
- Public Organization validation can move from `In progress` to
  `Action required` so the named representative can complete Microsoft Verified
  ID. After that human flow reports success, Azure can still show
  `Action required` / `Please complete your verification here` until the result
  propagates. Do not repeat identity capture immediately; refresh later and
  require the request to return to `In progress` before treating the personal
  step as acknowledged. Microsoft documents an overall organization-validation
  window of 1–20 business days, possibly longer when it requests documents.
- After approval, record only stable non-secret coordinates through protected
  deployment configuration. Keep all managed key material inside Azure.

The same company account is intended for MiniCon and AgenTerm. Do not create a
second publisher identity per repository.

SignPath Foundation declined the OSS application in early September 2026
(project not well known enough). The company publisher path is the only one.

## Observation log

Only the status field and dates are recorded. The validation ID, subscription,
tenant, mailbox and requester identity are deliberately absent, per the rule at
the top of this file.

| Date | Status in portal | Reading |
|------|------------------|---------|
| 2026-08-30 | submitted | Submission success only; not approval. |
| 2026-08-30 | not recorded | Same day, commit `eac8925` recorded that the named representative had completed Microsoft Verified ID and the portal had not yet acknowledged it. The portal status at that moment was not written down, so it is left blank here rather than reconstructed. |
| 2026-09-01 | **`In progress`** (`正在进行`), expiry empty | The request is back at In progress, which by the rule above is what makes the personal step count as acknowledged. **Not** certificate approval. |
| 2026-09-02 | `In progress`, banner `请完成电子邮件验证` | The vetting email ("Action needed: Verify your email account with Microsoft", sender `<MICROSOFT_VETTING_SENDER>`) had reached the primary mailbox and sat unread among CI notifications. Unrelated "Microsoft account team" one-time-code mails in the same inbox are **not** this email. |
| 2026-09-03 | `In progress` → **`Completed`** | Link clicked; the portal banner persisted for a while because the backend `EV` sub-service had not yet flipped (portal reads the vetting gateway with `X-Cache: CONFIG_NOCACHE`, so it is never browser cache). Refreshed later: Completed. Profile created the same day. |

### Validation waiting schedule (historical; closed)

The request completed on 2026-09-03. No further routine vetting check is
required; the exact non-promotable signed court later passed through GitHub
OIDC. A signed public Release remains a separate policy and Promotion choice.

Microsoft documents 1–20 business days for organization validation. Submission
was Sunday 2026-08-30, so business-day counting starts Monday 2026-08-31.

| Milestone | Date | Action |
|-----------|------|--------|
| +5 business days | **2026-09-04 (Fri)** | Routine re-check. Still In progress is normal; do nothing else. |
| +10 business days | **2026-09-11 (Fri)** | Re-check. Still In progress is within the documented window but worth noting in the log. |
| +20 business days | **2026-09-25 (Fri)** | Documented ceiling. If still In progress, open a Microsoft support request rather than resubmitting — resubmission restarts the queue. |

Do not poll daily: this file already records that repeating identity capture
immediately is counter-productive, and the portal lags behind the human flow.
The email link is the one step that does expire (7 days, not resendable), so
search the primary mailbox by subject on day 1 rather than waiting for the
portal to say more.

The only state that unblocks signing is the request reporting success **and**
the Public Trust certificate profile becoming creatable. Status text alone is
not the gate; the operational test is whether the certificate-profile blade
will create one.

### How the status was read (2026-09-01)

Read out of the page's accessibility tree, not a screenshot and not
coordinates: `mcu unlock <window>` to bring up Brave's renderer accessibility,
then `mcu tree` and read the `Identity validations` table row.

Worth recording for the MCU/cu boundary: `agenterm-cu unlock` reported the poke
delivered but the tree never grew (`poked=true, grew=false`, 55 chrome-only
nodes), while `mcu unlock` did bring the renderer tree up on the same window.
On Brave Origin, cu's unlock is currently weaker than MCU's.

### How the status was read (2026-09-03)

The portal blade's banner is derived from one JSON record. To see the real
state without guessing: DevTools → Network → Fetch/XHR, filter `vet`, open
`GetVettingRequestsBySubscription` → Preview, right-click `vettingRequests` →
Copy value, then `pbpaste`. `vettingResult[]` lists sub-services `DNE`, `TSS`,
`VC_Ind` (Verified ID), `BV` (business), `DV` (domain), `EV` (email) with
`serviceStatus`; the banner text maps to whichever is not `Pass`.

## From Completed to a wired signing profile (2026-09-03, redacted)

Everything below ran from the Mac with `az` (Homebrew `azure-cli` +
`trustedsigning` extension, preview) and `gh`; no portal clicking was needed.
Replace the placeholders; never write the real values into a repository.

```text
az login --tenant <TENANT_ID>                     # browser flow, see note
└── az trustedsigning certificate-profile create
    -g <RG> --account-name <ACCOUNT> -n <PROFILE>
    --profile-type PublicTrust
    --identity-validation-id <VALIDATION_ID>
    --include-street-address false --include-postal-code false
    └── az ad app create --display-name <APP> --sign-in-audience AzureADMyOrg
        ├── az ad sp create --id <APP_ID>
        ├── az ad app federated-credential create --id <APP_ID> --parameters
        │   {issuer: https://token.actions.githubusercontent.com,
        │    subject: repo:<ORG>@<OWNER_ID>/<REPO>@<REPO_ID>:environment:release-signing,
        │    audiences: [api://AzureADTokenExchange]}
        └── az role assignment create --assignee-object-id <SP_OBJECT_ID>
            --assignee-principal-type ServicePrincipal
            --role "Artifact Signing Certificate Profile Signer"
            --scope <PROFILE_RESOURCE_ID>          # profile scope, nothing wider
            └── gh secret set AZURE_CLIENT_ID|AZURE_TENANT_ID|AZURE_SUBSCRIPTION_ID
                gh variable set ARTIFACT_SIGNING_ENDPOINT|ACCOUNT|PROFILE
                --repo <ORG>/<REPO> --env release-signing
```

- `az login --use-device-code` fails in this tenant with error `530035`
  ("登录已成功，但没有访问此资源的权限"): a Microsoft-managed Conditional
  Access policy blocks the device-code flow. Use the browser flow; when the
  terminal cannot be copied from, point `BROWSER=` at a script that runs
  `open -a "Brave Origin" "$1"` so the page lands in the company profile.
- The profile subject is fixed by the validation: CN and O are the validated
  legal name; L/S/C follow the validated address. Street and postal code are
  the only optional subject parts. The workflow verifies `O=` only.
- Role propagation takes minutes; a `role assignment create` right after
  `sp create` can need a short wait.
- New GitHub repositories use the immutable OIDC subject shown above. Derive
  both numeric IDs from the GitHub organization/repository REST resources and
  compare the resulting subject as an opaque exact string. The legacy
  name-only subject, a GraphQL node id, or an issuer with a trailing slash
  causes Azure `AADSTS700213` before signing begins.
- Local mechanism rehearsal that worked: `jsign --storetype TRUSTEDSIGNING
  --keystore <ENDPOINT_HOST> --storepass "$(az account get-access-token
  --resource https://codesigning.azure.net --query accessToken -o tsv)"
  --alias <ACCOUNT>/<PROFILE> --alg SHA-256 --tsaurl
  http://timestamp.acs.microsoft.com --tsmode RFC3161 file.com`. It needs the
  human to hold the profile-signer role temporarily; remove that assignment
  afterwards so only the GitHub identity can sign. `osslsigncode verify
  -CAfile <Microsoft Identity Verification Root CA 2020 PEM>` reports the
  signer chain ok but cannot complete the timestamp chain from its bundle;
  Windows `Get-AuthenticodeSignature` is authoritative.
- The signed APE kept ZipOS readable and still executed on Darwin; the
  Security Directory grew the file by about 12 KiB.

## Live GitHub qualification (2026-09-03)

The final non-promotable court was run `33737286265`, sourced from exact
one-pack run `33736787946` at source `e37e686`. It proved three
Windows-authoritative `Valid` Authenticode signatures, company publisher,
RFC 3161 timestamp, unchanged VERSIONINFO, before→after SHA binding, and the
same signed `minicon.com` after-SHA executing on all six native OS/ISA cells.
Its signing and aggregate receipts both record `release_eligible=false`; the
public-receipt audit rejected protected provider/OIDC coordinate keys and
passed this receipt. `release-policy.json` remained `signing.mode=off`.

The preceding run supplied two durable failure lessons. Azure OIDC trust must
use GitHub's exact immutable subject, not a hand-shortened legacy subject. A
Windows checkout may also materialize tracked `.sh` files with CRLF; copying
those files into a cross-platform signed artifact produced `/bin/bash^M` or
“required file not found” on macOS/Linux after signing itself succeeded. The
workflow now normalizes shipped shell helpers to UTF-8 without BOM and LF, and
a policy test prevents that boundary from disappearing.

## MCU control (related, not signing)

Chromium page clicks via screenshot/coordinates are insufficient. MCU browser
bridge currently enumerates `Brave-Browser`, not `Brave Origin` profiles.
Follow-up: Native Messaging + DOM/a11y for Origin profiles; keep
background/no-focus-steal. See brain `N-639b53d5`.
