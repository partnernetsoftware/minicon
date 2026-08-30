# Enroll a company Azure tenant for Artifact Signing (redacted)

Status: company Artifact Signing account created; Public organization identity
validation submitted and pending Microsoft review on 2026-08-30. No
credentials, tenant/subscription/object IDs, mailboxes, addresses, validation
IDs, or payment details are recorded here. Publisher identity and SignPath
remain as in `CODE_SIGNING_POLICY.md`.

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
    │   └── pending Microsoft review
    ├── after approval: Public Trust certificate profile
    ├── signer workload identity gets only profile-signer RBAC
    └── exact unsigned SHA → signed SHA → timestamp → six execute-only courts
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

SignPath Foundation remains the OSS publisher path. Company-owned publisher
must never be labelled as SignPath.

## MCU control (related, not signing)

Chromium page clicks via screenshot/coordinates are insufficient. MCU browser
bridge currently enumerates `Brave-Browser`, not `Brave Origin` profiles.
Follow-up: Native Messaging + DOM/a11y for Origin profiles; keep
background/no-focus-steal. See brain `N-639b53d5`.
