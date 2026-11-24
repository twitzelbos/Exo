# Exo security audit

An independent review of Exo (`m1guelpf/Exo`, this fork `twitzelbos/Exo`) done before
connecting a real Google account, since the app requests full Gmail + Calendar +
Contacts access and its own README notes it is AI-generated and not human-reviewed.

- **Date:** 2026-11-24
- **Commit reviewed:** `b4a6605` (upstream `main`), ~282k lines across 10 crates
- **Method:** five parallel reviewers, one per risk area, each finding then
  spot-verified against the source by hand. This is a code review, not a
  penetration test or a formal proof. It does not cover the forked `blitz`/`parley`
  rendering engine's internals (see the one open item below) or macOS itself.

## Bottom line

**Safe to connect a real Google account, with two caveats to act on.** For a
from-scratch, AI-generated client the security posture is unusually disciplined.
No critical or high-severity issue was found in any area, and the serious-sounding
surfaces (token storage, email HTML, native `unsafe`, the dependency forks) each
held up under direct inspection. The two things worth doing before and after sign-in
are both easy and are described under "Act on these."

## Findings by area

| Area | Result | Worst item |
|---|---|---|
| Credentials at rest + OAuth | Solid | Sign-out doesn't revoke at Google (Low–Med, by design) |
| Network egress | Tightly scoped | Auto-updater has no independent signature (Medium) |
| Untrusted email rendering | Strong | Remote images load by default (Medium, privacy) |
| Supply chain (forked deps) | Low–moderate | Same updater item; add `cargo audit` to CI (Low) |
| Memory safety / `unsafe` | Clean | None (maintenance fragility only) |

### Credentials & OAuth — solid

- **Refresh tokens go to the macOS Keychain** (`exo-auth/src/secrets.rs`, via
  `security_framework`), one generic-password item per account. **Access tokens are
  never written to disk** (in memory only, refreshed ~5 min before expiry).
  `accounts.json` holds only metadata (email, name, colour), no secrets, written
  atomically.
- **Textbook desktop OAuth:** PKCE (S256), a random 128-bit `state` generated per
  attempt and **verified** on the callback, and a redirect listener bound to
  **127.0.0.1 only** on an ephemeral port that validates the request. An attacker
  can't reach it off-box, and PKCE blocks a stolen code.
- **OAuth endpoints are hard-pinned to Google** regardless of the client-secret
  file — there's a test feeding a malicious client file with attacker-controlled
  `auth_uri`/`token_uri` and confirming Google's real endpoints are still used.
- **No token or secret is ever logged** (manual `Debug` impls omit them, enforced by
  tests). Log files are written `0600`.
- **Scopes:** `gmail.modify`, `gmail.settings.basic`, `calendar` (read-write),
  contacts **read-only**, directory read-only, userinfo. Correctly minimised — *not*
  the permanent-delete `mail.google.com` scope, and contacts can't be modified.
- Verified by hand: Keychain storage, no access-token persistence, endpoint pinning.

### Network egress — tightly scoped

- The running app contacts only: **Google** (the user's own account + `lh3`
  contact avatars), **GitHub** (auto-update only), and **sender-controlled public
  hosts** for email images and one-click unsubscribe. Nothing else.
- **No telemetry, analytics, crash reporting, or author callback** of any kind. (A
  grep that looked alarming — 13 "segment"/"sentry" hits — turned out to be
  `icu_segmenter` / `unicode-segmentation`, text crates.)
- **No email content or metadata leaves the user's Google account.** The Microsoft
  and Zoom/Meet strings are links opened in the browser, not requests the app makes.
- **TLS:** rustls only; certificate validation is never disabled anywhere; sensitive
  paths force HTTPS and `no_proxy()`. Verified by hand (no `danger_accept_invalid*`
  in the tree).

### Untrusted email rendering — strong

- Email HTML runs through a **DOMPurify-grade sanitizer** (`exo-mail/src/sanitize.rs`,
  ~1250 lines) *before* reaching `blitz`, which has **no JavaScript engine** at all.
  Scripts, event handlers, iframes and `javascript:`/`vbscript:`/`data:text/html`
  URLs are stripped — including `javascript:` hidden behind control characters — with
  a hostile-HTML fuzz test asserting nothing dangerous survives. Bodies render in an
  isolated shadow root with a non-resolving base URL.
- **SSRF protection is genuinely strong** (`exo/src/remote_content.rs`): only
  http/https, userinfo rejected, DNS resolved and then the connection made only to
  addresses that pass a public-IP check (defeats DNS rebinding), redirects
  re-checked, proxies disabled. A crafted `<img src>` cannot reach loopback, the LAN,
  or `169.254.169.254`. No bypass found.
- **Clicked links are scheme-restricted to http/https** (`format/urls.rs`); `file:`,
  `javascript:`, `smb:`, custom schemes are dropped, never handed to the OS opener.
  `mailto:` goes to the internal composer.
- **Attachments:** filenames are sanitised (path separators, control/formatting chars
  incl. the RTL-override range U+202A–202E that disguises `invoice<RLO>fdp.exe`,
  `..` → `__`), written with `O_EXCL` (never overwrite or follow a symlink) and
  **quarantined** so Gatekeeper vets mailed executables. A `../../.zshrc` test
  confirms no traversal.
- Verified by hand: sanitizer behaviour + tests, link scheme filter, attachment
  path defenses.

### Supply chain — low to moderate

- The four non-crates.io dependencies (`crmne/egui`, `crmne/winit`,
  `m1guelpf-forks/blitz`, `m1guelpf-forks/fastframe`) are **first-party** (same author
  as the app) and **pinned to immutable commit revs**. The vendored checkouts match
  the pinned revs.
- Scanned for smuggled behaviour: no exfiltration, secret-harvesting, obfuscation, or
  unexpected network/process calls in what Exo actually compiles. The scary-looking
  networking/debug-server code that exists in `blitz` and `egui` (a resource loader, a
  CDP debug server, an inspection TCP listener) is **not in the lockfile** — only the
  JS-less HTML/layout/paint engine is. `fastframe`'s heavy `Command` use is ordinary
  desktop-shell glue (font matching, dialogs, the updater's `codesign`/`spctl`).
  Build scripts are codegen only; none touch the network or write outside `OUT_DIR`.
- ~912 packages total (heavy but expected: wgpu + egui + winit + servo's stylo via
  blitz). No OpenSSL. No known-vulnerable versions spotted, **but** `cargo audit`
  isn't installed, so that's a manual spot-check, not an authoritative RUSTSEC scan.

### Memory safety — clean

- Every `unsafe` block in Exo's own code was read. **No use-after-free, invalid
  deref, bad transmute, out-of-bounds length, or unchecked FFI return.** The `unsafe`
  is routine objc2/AppKit UI glue plus small libc/CoreFoundation FFI, with accurate
  `// SAFETY:` comments and null-checked FFI returns.
- The one `unsafe` whose length comes from the network (`brands/dns.rs`, a DNS TXT
  lookup) takes its length from `dns_sd`'s own validated record length, not raw wire
  bytes, and re-validates downstream. No `set_len` anywhere; every `transmute` is
  Objective-C method-pointer glue, not data punning. Verified by hand.
- Only liability is maintenance fragility: the drag/menu/text-input code swizzles
  `winit`'s private classes, so it's worth re-testing on each `winit` upgrade (it
  fails safe with a log if the internals change).

## Act on these

1. **Turn on "Block all remote images"** (⌘K → Images, or Settings). *Medium,
   privacy.* The default, "Reduce tracking", still auto-loads normal-sized remote
   images when you open a message, so a sender learns you opened it, with your IP,
   timestamp and User-Agent, sent straight to their server (no image proxy). The
   automatic stripper only catches a hardcoded tracker list and ≤4×4px pixels, and it
   only scans `<img>` — a CSS `background-image` or SVG image tracker still loads.
   "Block all remote images" closes this; you reveal images per-message when you
   trust the sender. This leaks no mailbox content either way — it's an open-tracking
   issue, not a compromise.
2. **Prefer the official signed/notarized DMG, and know what secures updates.**
   *Medium, supply chain.* Exo auto-updates from its GitHub releases. It verifies the
   download's SHA-256, but `checksums.txt` comes from the same release, so that only
   catches corruption, not tampering — the app does **not** set an independent
   Ed25519 publisher key. The real anti-tamper guarantee is **macOS code-signing**:
   an update must carry the author's Apple Developer Team signature and pass
   Gatekeeper, and only a team-signed install auto-updates at all. That's a strong
   barrier (an attacker would need the author's Apple signing identity), but all
   update trust rests on Apple's chain, not on a key Exo itself checks. A locally
   built (ad-hoc-signed) Exo — like the one in `target/release` — is **excluded from
   auto-update**, so this doesn't apply to it.

## Also worth knowing

- **"Sign Out" does not revoke access at Google** (by design, so other copies of Exo
  aren't signed out). It deletes the local tokens, but the grant stays valid until you
  remove Exo at <https://myaccount.google.com/permissions>. Do that if you want the
  authorization truly killed.
- **The requested scopes are broad** (`gmail.modify` can read/send/label/trash all
  mail; `calendar` is read-write), but they match the app's features and are minimised
  where possible. This is the same access the official `op`-style desktop clients ask
  for; the point is that you're granting it to unofficial, unreviewed code.

## Open item for a deeper pass

The highest-value follow-up is a dedicated review of the **forked `blitz`/`parley`
engine's decoder limits**. Exo caps remote downloads at 25 MB and animated images at
4096×4096, but a static image or a `@font-face` remote font (loadable under the
default remote setting) is decoded by blitz/parley, and no Exo-side pixel/size cap was
found for those — bounded only by the 25 MB fetch cap and Gmail's message size. A
decompression bomb there would be a crash/DoS, not code execution. Reviewing the fork
was out of scope here.

## Reproducing the safe demo

The app can be run against a local fake mailbox, with no real account or credentials
and a throwaway data dir, to try the UI safely:

```sh
cargo build --release --bin mock-servers
./target/release/mock-servers --port 8787 &
EXO_MOCK_SERVERS=http://127.0.0.1:8787/ ./target/release/Exo.app/Contents/MacOS/Exo
```
