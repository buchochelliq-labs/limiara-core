# iOS builds for TestFlight

The `testflight` workflow builds the iOS app (the Tauri 2 shell in `apps/device-shell`, bundle identifier `app.limiara.shell`) in release mode, signs it for App Store distribution and uploads it to App Store Connect, where it appears in TestFlight. This runbook is for the Apple Developer account holder: it lists what to set up once in Apple's portals and in GitHub, and how to run a build.

::: danger Keep signing material private
The API key, the certificate and their passwords are credentials for the Apple Developer account. Never paste them into a chat, an issue, a pull request or a commit, and never send them by email. Enter them only in the GitHub secret form described below. If one is exposed, revoke it in App Store Connect or the Developer portal straight away and create a new one.
:::

## How the workflow works

- **When it runs:** only when asked, from **Actions → testflight → Run workflow**, or when a tag such as `ios-v0.1.0` is pushed. It never runs for pull requests or ordinary pushes, so code from a fork can never reach the signing secrets.
- **Where the secrets live:** in the GitHub environment `testflight`. The job names that environment, so any protection on it, such as required reviewers, applies before the job receives a secret.
- **Missing secrets:** the first step fails at once with the names of any missing secrets. It never prints a value.
- **Signing:** by default the workflow uses automatic signing with the App Store Connect API key: Xcode creates or reuses the distribution certificate and the App Store profile itself, so no certificate or profile secret is needed. If that is not possible for the account, three optional secrets switch it to manual signing with a distribution certificate and profile (see [Manual signing](#manual-signing)).
- **Versions:** the marketing version (`CFBundleShortVersionString`) is the `version` in `apps/device-shell/src-tauri/tauri.conf.json`. The build number (`CFBundleVersion`) is the workflow's run number, so each run uploads a new build; a re-run of the same run uses `<run number>.<attempt>`. A pushed tag must match the marketing version: `ios-v0.1.0` builds `0.1.0`.
- **Release checks:** before uploading, the workflow checks that the web build and the app contain no debug-only code (the `/__ui` showcase, the forced-state probes, the Contacts fixture client and the debug simulator's push hook), that the app is signed for distribution with production push notifications, and that its versions and export compliance key are as expected. The script is `scripts/check-ios-release-build.sh`.
- **Clean-up:** the key file and any decoded certificate sit in the runner's temporary directory with owner-only permissions. With manual signing, the Tauri CLI imports the certificate into a temporary keychain and deletes it when it exits. The last step deletes the files, any keychain left behind and the installed provisioning profiles, even when the build fails.

## 1. Register the App ID

In the [Apple Developer portal](https://developer.apple.com/account/resources/identifiers/list), under **Certificates, Identifiers & Profiles → Identifiers**:

1. Add an identifier of type **App IDs**, then **App**.
2. Description: `Limiara`. Bundle ID: **Explicit**, `app.limiara.shell`.
3. Under **Capabilities**, enable **Push Notifications**. This matches the app's only entitlement, `aps-environment` (`apps/device-shell/src-tauri/Entitlements.ios.plist`); the workflow sets it to `production` for distribution builds.
4. Leave every other capability off. Background pushes (`remote-notification`) are declared in the app's `Info.plist` and need no App ID capability, and the app's keychain items use the app's default access group.

The APNs authentication key the server uses to send pushes is separate from this workflow; see [ADR 0030](../adr/0030-push-notifications.md).

## 2. Create the app record

In [App Store Connect](https://appstoreconnect.apple.com/apps), choose **Apps → + → New App**:

- Platform: **iOS**.
- Name: `Limiara` (or the name the app will have in the App Store).
- Primary language as appropriate.
- Bundle ID: `app.limiara.shell`, as registered in step 1.
- SKU: any unique internal reference, for example `limiara-ios`.
- User access: **Full Access**, unless only some team members should see the app.

Once it exists, open **App Information** and note the **Apple ID**, a number. It is not secret. Save it as the `testflight` environment's variable `APP_STORE_APP_ID` (see step 4): the upload tool needs it on accounts where it cannot tell the app from its bundle identifier alone.

## 3. Create the App Store Connect API key

In App Store Connect, open **Users and Access → Integrations → App Store Connect API**, under **Team Keys**:

1. Choose **Generate API Key** (or **+**). Name it, for example, `GitHub TestFlight`.
2. Choose the access role:
   - **Admin** for automatic signing, the default. Xcode creates the cloud-managed distribution certificate and the App Store profile with this key. Tauri's iOS signing guide asks for Admin access for this, and keys with lower roles are refused cloud-managed distribution signing.
   - **App Manager** is enough for uploading builds. Use it only together with [manual signing](#manual-signing), where the certificate and the profile come from secrets instead.
3. Note the **Issuer ID** shown above the list of keys, and the new key's **Key ID**.
4. Download the private key, `AuthKey_<Key ID>.p8`. Apple lets you download it **only once**. Keep it in a password manager or delete it after creating the secret; it can always be revoked and replaced.

## 4. Create the GitHub environment and its secrets

In the GitHub repository, open **Settings → Environments → New environment** and name it exactly `testflight`. Then:

1. Under **Deployment protection rules**, add **Required reviewers** (for example yourself), so every run waits for approval before it receives the secrets.
2. Under **Deployment branches and tags**, choose **Selected branches and tags** and allow `main` and the tag pattern `ios-v*`.
3. Add these **environment secrets**. Each value is entered only in GitHub's secret form.

| Secret | Value |
| --- | --- |
| `APPLE_TEAM_ID` | The 10-character Team ID, shown under **Membership details** in the Developer account. |
| `APP_STORE_CONNECT_API_KEY_ID` | The key's **Key ID** from step 3. |
| `APP_STORE_CONNECT_API_ISSUER_ID` | The **Issuer ID** from step 3. |
| `APP_STORE_CONNECT_API_KEY_P8_BASE64` | The `.p8` file, base64-encoded (below). |

4. Optionally, add the environment **variable** (not a secret) `APP_STORE_APP_ID` with the Apple ID from step 2.

To base64-encode the key on a Mac and copy it to the clipboard, run this in Terminal, in the folder holding the file, and paste the clipboard into the secret's value:

```sh
base64 -i AuthKey_XXXXXXXXXX.p8 | pbcopy
```

On Windows, in PowerShell in the folder holding the file:

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("AuthKey_XXXXXXXXXX.p8")) | Set-Clipboard
```

On Linux or WSL, `base64 -w0 AuthKey_XXXXXXXXXX.p8` prints it on one line.

Clear the clipboard afterwards by copying something else. Encode the file exactly as downloaded, without opening and saving it in an editor. The workflow ignores line breaks in the secret, removes a byte-order mark and Windows line endings from the decoded key, and stops with a message if it is not an App Store Connect key (an EC P-256 private key).

### Manual signing

Use manual signing only if automatic signing cannot be used, for example because the API key must not have the Admin role. Add all three of these secrets; the workflow rejects a partial set and names what is missing:

| Secret | Value |
| --- | --- |
| `IOS_DISTRIBUTION_CERTIFICATE_P12_BASE64` | An **Apple Distribution** certificate with its private key, exported from Keychain Access as a `.p12`, base64-encoded. |
| `IOS_DISTRIBUTION_CERTIFICATE_PASSWORD` | The password chosen when exporting the `.p12`. |
| `IOS_APP_STORE_PROFILE_BASE64` | An **App Store Connect** provisioning profile for `app.limiara.shell` that includes that certificate, base64-encoded. |

To create them:

1. In the Developer portal, **Certificates → +**, choose **Apple Distribution**, upload a certificate signing request made with Keychain Access (**Certificate Assistant → Request a Certificate From a Certificate Authority**), download the certificate and open it to add it to the login keychain.
2. In Keychain Access, expand the certificate, select it with its private key, choose **File → Export Items**, save it as a `.p12` and set a strong password.
3. In the Developer portal, **Profiles → +**, choose **App Store Connect**, select the App ID `app.limiara.shell` and the certificate from step 1, and download the `.mobileprovision` file.
4. Encode both files in the same way as the key:

```sh
base64 -i distribution.p12 | pbcopy
base64 -i Limiara_App_Store.mobileprovision | pbcopy
```

Before building, the workflow checks that the certificate opens with its password, is an Apple Distribution certificate and belongs to `APPLE_TEAM_ID`; it logs only the certificate's expiry date. The Tauri CLI then imports it into a temporary keychain for the build and deletes it afterwards, and the workflow's clean-up step deletes any keychain that is left. A profile expires with its certificate, after a year; renew both and replace the secrets before then.

## 5. Run a build and find it in TestFlight

- **Manually:** **Actions → testflight → Run workflow** on `main`. The optional `build_number` input replaces the run number, for example to continue a numbering started elsewhere; it must be higher than every build already uploaded for that version. `keep_ipa` keeps the signed `.ipa` as a workflow artifact for 7 days, which is off by default.
- **From a tag:** after raising `version` in `tauri.conf.json` on `main`, push a matching tag:

```sh
git tag ios-v0.1.0
git push origin ios-v0.1.0
```

With required reviewers, approve the run under **Review deployments**. Expect a run to take 20 to 40 minutes. When it succeeds, its summary names the uploaded version and build.

App Store Connect then processes the build, which usually takes 5 to 30 minutes, and emails the account holder when it is ready or if it was rejected. Find it in App Store Connect under **Apps → Limiara → TestFlight → iOS Builds**. Add internal testers under **Internal Testing**; external testers also need the build to pass TestFlight review. Testers install it with the TestFlight app on their iPhone.

## 6. Export compliance

Every app uploaded to App Store Connect must declare whether it uses encryption, because the upload counts as an export from the United States. Apple's `ITSAppUsesNonExemptEncryption` key in `Info.plist` answers this in advance: `NO` means the app uses no encryption or only encryption that is exempt from export documentation, typically the encryption built into iOS, such as HTTPS through Apple's networking APIs; `YES` means anything else. Apple's guidance is in [Complying with encryption export regulations](https://developer.apple.com/documentation/security/complying-with-encryption-export-regulations).

The app sets the key to **`YES`** (`apps/device-shell/src-tauri/Info.ios.plist`), because it brings its own encryption rather than relying only on the encryption in iOS:

- the on-device copy of the workspace is encrypted with SQLCipher, built with its own copy of OpenSSL (ADR 0024, "Encryption at rest");
- its connections to the server use TLS through rustls, not Apple's networking stack.

Both use standard, published algorithms (AES and the TLS cipher suites); nothing is proprietary. Declaring `NO` would claim an exemption, and Apple makes the developer responsible for an inaccurate claim, so the accurate declaration is `YES`.

What this means in practice:

1. In App Store Connect, under **Apps → Limiara → App Information → App Encryption Documentation**, choose **+** and answer the questions once. For this app the type of encryption is **standard encryption algorithms instead of, or in addition to, using or accessing the encryption within Apple's operating system**. App Store Connect then says whether documentation must be uploaded, which depends on where the app is distributed: for example, distribution in France needs a French encryption declaration.
2. Until that is answered, each new build shows **Missing Compliance** in TestFlight; choose **Manage** next to the build to answer for it.
3. If Apple approves uploaded documentation, it gives a compliance code. Add it to `Info.ios.plist` as the string `ITSEncryptionExportComplianceCode`, and builds no longer ask.
4. Using standard encryption can also require a yearly self-classification report to the US Bureau of Industry and Security. Apple's guidance covers when; the account holder, with legal advice if needed, decides whether it applies.

If the questionnaire concludes that the app needs no documentation and its use is exempt, the key may be changed to `NO`. That is a compliance decision for the account holder, not a build setting.

## What only a real run can confirm

The workflow cannot be exercised without the Apple account, so the first run confirms:

- that automatic signing can create the distribution certificate and profile with the API key, or that the manual secrets match each other;
- that the exported app carries `aps-environment` set to `production` and the expected versions, which `scripts/check-ios-release-build.sh` checks before the upload;
- that `altool` accepts the upload and App Store Connect processes the build.

If a run fails, its log shows the failing step; no secret value appears in it.
