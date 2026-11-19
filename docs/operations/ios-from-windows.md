# iOS on Windows with pippwin

[pippwin](https://github.com/buchochelliq-labs/pippwin) builds the Limiara iOS shell on a Windows PC, through the Windows Subsystem for Linux (WSL), and installs it on an iPhone connected by USB. You need no Mac and no Xcode. It wraps [xtool](https://github.com/xtool-org/xtool), an open-source replacement for Xcode's build and signing steps.

This is for developers who want the app on their own phone. It is experimental, and it does not replace the supported iOS builds, which run on macOS: the `ios` workflow (simulator) and the [`testflight` workflow](testflight.md).

## What you get

- The iOS shell from `apps/device-shell`, built with the web shell embedded, as the Xcode build does.
- Installed as **Limiara (xtool)**, with its own bundle identifier (`app.limiara.shell.xtool`). It sits next to a TestFlight install and never replaces it.
- **No push notifications**, unless your Apple ID belongs to a paid developer team and you add the entitlement.
- **No add-ons** that need Xcode's App Intents step.
- **A limited lifetime:** with a free Apple ID the app stops opening after 7 days and must be installed again; with a paid developer account it lasts a year.

## What you need

- Windows 11, or Windows 10 with WSL 2, and an administrator account for the first run.
- About 40 GB free disk: the Xcode download alone is over 7 GB.
- An Apple ID. A paid Apple Developer Program membership is recommended, because it allows the API-key login.
- The iPhone and a USB cable.

## Set it up

1. **Download pippwin's Windows script.** It is `scripts/setup-windows.ps1` in the [pippwin repository](https://github.com/buchochelliq-labs/pippwin). Read it before you run it: it installs software and asks for administrator rights.
2. **Run it in PowerShell as administrator,** pointing it at this repository:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\setup-windows.ps1 -Project https://github.com/buchochelliq-labs/limiara-core
   ```

   It installs WSL with Ubuntu and the USB passthrough tool `usbipd`, then builds pippwin inside Ubuntu. If Windows needs a restart to finish installing WSL, the script says so: restart and run it again.
3. **Follow `pippwin setup`.** It clones this repository and reads `pippwin.toml` at its root. It then works through the steps one by one, explaining each before it runs:
   - Ubuntu packages, after showing the `sudo` commands and asking first;
   - Swift, xtool, Rust with the `aarch64-apple-ios` target, Node 22 and pnpm;
   - the iOS SDK from your Xcode download;
   - your Apple login;
   - connecting the phone;
   - building and installing the app.

   Every step can be run again safely: one that is already done is skipped.

Already in Ubuntu on WSL? Clone pippwin and run its `scripts/bootstrap.sh`, then:

```sh
pippwin setup --project https://github.com/buchochelliq-labs/limiara-core
```

## Logging in to Apple

pippwin asks which login to use and shows xtool's own descriptions and warnings word for word:

- **API key** (needs a paid membership, recommended). In App Store Connect, create a **Team Key** with the **App Manager** role, and download its `.p8` file. Apple lets you download it only once, so keep a backup in a password manager. pippwin asks for the Key ID and Issuer ID and finds the file in your Downloads folder. It moves the key into a private folder in Ubuntu that only your account can read, and offers to delete the copy left in Windows.
- **Password** (works with any Apple ID, but uses private Apple interfaces). pippwin asks for your Apple ID. You then type the password and the two-factor code into xtool itself, which does not show them. xtool's advice is to use a secondary Apple ID for this.

pippwin never reads, stores or logs your password, your two-factor code or the contents of the key file. On Linux xtool saves its login token readable by other users; pippwin makes that file private after every login, and `pippwin doctor` flags it if it is not.

::: warning Never put the key in this repository
Keep the `.p8` file out of this repository and any other git folder: pippwin refuses to store it inside one, or on the Windows drive. If a key is ever exposed, revoke it in App Store Connect and create a new one.
:::

## Build again after a change

Inside Ubuntu, in your clone of this repository:

```sh
pippwin run
```

This runs `apps/device-shell/ios-xtool/prepare.sh` and then `xtool dev`, which builds, signs and installs the app.

- **Phone disconnected?** Reconnect it with `pippwin phone`. Windows hands the USB device back to itself each time it is unplugged.
- **Something wrong?** `pippwin doctor` shows each step as done, missing or a problem.
- **First run on the phone:** trust the developer profile under **Settings → General → VPN & Device Management**.

## When the build fails

The iOS build in this repository is documented in `apps/device-shell/ios-xtool/README.md`: what `prepare.sh` builds, how to run it by hand, and the likely first failures with what to try. Problems with pippwin itself belong in [pippwin's issues](https://github.com/buchochelliq-labs/pippwin/issues); problems with the app or its build belong in this repository.

## Keeping `pippwin.toml` current

`pippwin.toml` names the app folder, the command to run before `xtool dev`, and what the build needs: Rust with the `aarch64-apple-ios` target, Node 22 and pnpm 9.12.0. Update it in the same change as anything that moves the app folder, renames `prepare.sh`, or changes the Node or pnpm version.
