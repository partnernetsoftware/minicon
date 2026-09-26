# Azure Artifact Signing setup (redacted)

Entry → branch → gate → evidence → next. Real identifiers stay out of every
repository; `<PLACEHOLDER>` marks them.

## 0. Tooling on the Mac

- `brew install azure-cli jsign` and `az extension add --name trustedsigning`
  (preview; the command group is `az trustedsigning`).
- `az login --use-device-code` is blocked in the company tenant by a
  Microsoft-managed Conditional Access policy (error `530035`, "登录已成功，但
  没有访问此资源的权限"). Use the browser flow:
  `BROWSER=<script running open -a "Brave Origin" "$1"> az login --tenant <TENANT_ID>`
  so the page opens in `<COMPANY_BROWSER_PROFILE>`; the owner handles any
  password/MFA and selects the work account. Gate: a redacted
  `az account show` check confirms the company subscription without printing
  its identifiers.

## 1. Account (exists since 2026-08-30)

Work Entra tenant → pay-as-you-go subscription → Artifact Signing account,
Basic tier, East US, endpoint `https://eus.codesigning.azure.net/`. The human
verifier holds `Artifact Signing Identity Verifier` at account scope only.
Basic tier limits the number of Public Trust profiles; check the quota before
creating a second one and prefer sharing one profile across products, because
the certificate subject is the company, not the product.

## 2. Public Organization identity validation (Completed 2026-09-03)

1. Submit Public → Organization with the legal name and registered address.
2. Microsoft emails the **primary** address from
   `<MICROSOFT_VETTING_SENDER>`,
   subject "Action needed: Verify your email account with Microsoft". The link
   lives 7 days and cannot be resent. Search the mailbox by subject on day 1;
   it hides among CI mail, and "Microsoft account team" one-time codes are a
   different thing.
3. The named representative completes Microsoft Verified ID.
4. Portal banner text is derived from one JSON record; when it looks stuck,
   read the record instead of guessing: DevTools → Network → Fetch/XHR →
   filter `vet` → `GetVettingRequestsBySubscription` → Preview → right-click
   `vettingRequests` → Copy value. `vettingResult[]` holds `DNE`, `TSS`,
   `VC_Ind`, `BV`, `DV`, `EV`; the banner names whichever is not `Pass`. The
   gateway answers with `X-Cache: CONFIG_NOCACHE`, so it is never browser
   cache. Email clicks propagate to `EV` with a delay of minutes to hours.
5. Gate: status `Completed`. Open a support request only after the documented
   20-business-day ceiling; resubmitting restarts the queue.

## 3. Certificate profile

```text
az trustedsigning certificate-profile create -g <RG> --account-name <ACCOUNT>
  -n <PROFILE> --profile-type PublicTrust
  --identity-validation-id <VALIDATION_ID>
  --include-street-address false --include-postal-code false
```

Subject becomes `CN=<legal name>, O=<legal name>, L=…, ST=…, C=…`; only street
and postal code are optional. Gate: `status: Active`, `provisioningState:
Succeeded`. The service manages certificate issuance and renewal; its Public
Trust signing certificates have a three-day validity, which is why timestamping
is mandatory. Do not infer or promise a per-file certificate issuance pattern.

## 4. One GitHub identity per repository

First determine which subject format GitHub actually issues. Repositories
created after GitHub's immutable-subject rollout use both numeric IDs:

```text
OWNER_ID=$(gh api orgs/<ORG> --jq '.id|tostring')
REPO_ID=$(gh api repos/<ORG>/<REPO> --jq '.id|tostring')
SUBJECT="repo:<ORG>@${OWNER_ID}/<REPO>@${REPO_ID}:environment:release-signing"
```

For an organization-owned repository, validate the constructed value against
the documented shape before sending it to Azure. Do not use a GraphQL
`node_id`, an Entra object ID, or the legacy
`repo:<ORG>/<REPO>:environment:release-signing` form as a guess. An Azure
`AADSTS700213` failure means the stored subject did not exactly match the
token's `sub`; compare them as opaque strings and keep the identifiers out of
public logs and receipts. The issuer is exactly
`https://token.actions.githubusercontent.com` with no trailing slash.

```text
az ad app create --display-name <REPO>-release-signing --sign-in-audience AzureADMyOrg
az ad sp create --id <APP_ID>
az ad app federated-credential create --id <APP_ID> --parameters '{
  "name": "github-<REPO>-release-signing",
  "issuer": "https://token.actions.githubusercontent.com",
  "subject": "<EXACT_GITHUB_OIDC_SUBJECT>",
  "audiences": ["api://AzureADTokenExchange"]}'
az role assignment create --assignee-object-id <SP_OBJECT_ID>
  --assignee-principal-type ServicePrincipal
  --role "Artifact Signing Certificate Profile Signer"
  --scope <PROFILE_RESOURCE_ID>
```

Then, with `gh` on the repository, environment `release-signing`: secrets
`AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`; variables
`ARTIFACT_SIGNING_ENDPOINT`, `ARTIFACT_SIGNING_ACCOUNT`,
`ARTIFACT_SIGNING_PROFILE`. Gate: `az role assignment list --scope
<PROFILE_RESOURCE_ID>` shows exactly the service principals that should sign
and no human. Role propagation can take a few minutes.

Do not copy one repository's `AZURE_CLIENT_ID` into another. Products share the
company signing account/profile but own distinct Entra applications and exact
GitHub Environment subjects. List only Environment secret/variable **names**
when verifying setup; never print their values.

## 5. Local rehearsal (mechanism evidence, never a release)

Temporarily grant the owner's user the profile-signer role, then:

```text
TOKEN=$(az account get-access-token --resource https://codesigning.azure.net --query accessToken -o tsv)
jsign --storetype TRUSTEDSIGNING --keystore <ENDPOINT_HOST> --storepass "$TOKEN"
      --alias <ACCOUNT>/<PROFILE> --alg SHA-256
      --tsaurl http://timestamp.acs.microsoft.com --tsmode RFC3161 <copy-of-file>
osslsigncode verify -CAfile <Microsoft Identity Verification Root CA 2020 .pem> -in <copy-of-file>
```

Expect signer issuer `Microsoft ID Verified CS EOC CA nn`, "Signature
verification: ok", a one-byte mutation failing, and for APE files a still
readable ZIP plus Darwin execution. osslsigncode cannot complete the
timestamp-server chain from its own bundle; Windows
`Get-AuthenticodeSignature` is authoritative. Remove the human role assignment
afterwards. Each rehearsal consumes one signature of the monthly quota.

## 6. Reconstructing the setup when nobody remembers

This chain was provisioned on **2026-09-03** from the dev Mac with `az` and
`gh`, not by clicking through the portal. The human performed only the steps
that cannot be automated: Entra signup and MFA, payment, clicking Microsoft's
vetting email, and Microsoft Verified ID. Everything else — certificate
profile, Entra application, federated credential, profile-scoped RBAC,
`gh secret set` / `gh variable set` — ran as commands. Whoever runs them later
will find **nothing to download**: the key is non-exportable inside the
service, so there is no PFX, token or certificate file on any machine. Asking
"where is the certificate I downloaded" is a signpost that this section is
needed, not a sign that something is missing.

State last verified **2026-09-13** (read-only, no changes made):

```text
az trustedsigning list                                  # account: eastus, rg-signing
az trustedsigning certificate-profile list -g <RG> --account-name <ACCOUNT>
    # → one PublicTrust profile, status Active, provisioningState Succeeded
PID=/subscriptions/<SUB>/resourceGroups/<RG>/providers/Microsoft.CodeSigning/
    codeSigningAccounts/<ACCOUNT>/certificateProfiles/<PROFILE>
az role assignment list --scope "$PID"
    # → expect one ServicePrincipal per product, and no human
az ad app list --query "[?contains(displayName,'release-signing')].[displayName,appId]" -o json
az ad app federated-credential list --id <APP_ID> --query "[].[issuer,subject,audiences]" -o json
OID=$(gh api orgs/<ORG> --jq '.id|tostring'); RID=$(gh api repos/<ORG>/<REPO> --jq '.id|tostring')
echo "repo:<ORG>@$OID/<REPO>@$RID:environment:release-signing"   # must equal subject byte for byte
gh api repos/<ORG>/<REPO>/environments/release-signing/secrets   --jq '.secrets[].name'
gh api repos/<ORG>/<REPO>/environments/release-signing/variables --jq '.variables[].name'
    # → names only; GitHub never returns secret values
```

Two additional forensic handles, both local and both identifier-bearing, so
read them without copying values into any repository:

- `~/.azure/azureProfile.json` and `~/.azure/telemetry/*/cache` timestamps show
  when `az` was installed and which afternoon the commands ran; on the dev Mac
  these date to 2026-09-03 and bracket the commits that wired the workflow.
- `git log --since=<date> --until=<date> --pretty='%h %ad %an %s'` in both the
  product repository and this hub: the wiring commits are attributed to the
  `MiniCon Automation` / `PartnerNet Software Automation` identities.

On the dev Mac, `az` (Homebrew, with the preview `trustedsigning` extension),
`jsign`, `osslsigncode` and `gh` are installed and `az` is already signed in.
A checked-in login is not release evidence: signing that matters happens in
GitHub Actions through OIDC, and a local `az` session is only for creating
resources or rehearsing. If `az login` is ever needed again, remember
§0: `--use-device-code` fails in this tenant with `530035`.

Adding another Windows product (`agenterm.exe`, `agenterm.com`, and so on) does
not repeat §1–§3. Reuse the company account and the existing Public Trust
profile, then do only §4 for that repository: its own Entra application, its own
exact immutable subject, its own profile-scoped signer role, its own
`release-signing` Environment. Start from
[new-product-checklist.md](new-product-checklist.md).
