# Google OAuth Setup (Desktop client) — Cloud Connect

SundayRec backs up recordings to **Google Drive** and (later) publishes to
**YouTube** / sends mail via **Gmail**, all through a **single Google OAuth
client** using the **Desktop / installed-app loopback** flow with PKCE. This doc
walks through creating that client so the cloud features can be smoke-tested.

> The Drive/YouTube/Gmail backends are **NETWORK-UNVERIFIED** in code — only the
> pure decision logic is unit-tested. This setup unlocks the first real wire
> test (see [`docs/SMOKE-TEST.md`](SMOKE-TEST.md) §7).

---

## 1. Create a Google Cloud project

1. Go to <https://console.cloud.google.com/> and create (or pick) a project.
2. Note the project — everything below lives inside it.

## 2. Enable the APIs you intend to test

**APIs & Services → Library**, enable as needed:

| Feature in SundayRec | API to enable           |
| -------------------- | ----------------------- |
| Drive backup         | **Google Drive API**    |
| YouTube publish      | **YouTube Data API v3** |
| Gmail notifications  | **Gmail API**           |

For a first smoke test, **Google Drive API** alone is enough.

## 3. Configure the OAuth consent screen

**APIs & Services → OAuth consent screen**:

1. User type: **External** (or Internal if you're in a Workspace org).
2. Fill app name, support email, developer email.
3. **Scopes** — add the ones matching the services you enabled. SundayRec
   requests exactly these (each service also requests `openid email profile`):

   | Service | Scope string requested by the app                |
   | ------- | ------------------------------------------------ |
   | Drive   | `https://www.googleapis.com/auth/drive.file`     |
   | YouTube | `https://www.googleapis.com/auth/youtube.upload` |
   | Gmail   | `https://www.googleapis.com/auth/gmail.send`     |

   `drive.file` is the **narrow** scope — the app can only see/manage files it
   created itself, never your whole Drive.

4. **Test users** — while the app is in "Testing", add the Google account(s)
   you'll smoke-test with, or consent will be blocked.

## 4. Create the OAuth client credentials

**APIs & Services → Credentials → Create credentials → OAuth client ID**:

1. **Application type: Desktop app** (this is the important part — it enables the
   loopback redirect and means the client "secret" is non-confidential, per
   Google's installed-app guidance).
2. Name it (e.g. "SundayRec Desktop").
3. Create. Copy the **Client ID** (`…apps.googleusercontent.com`) and the
   **Client secret**.

You do **not** register a fixed redirect URI for a Desktop client — SundayRec
binds an **ephemeral loopback port** at connect time and uses
`http://127.0.0.1:<port>` as the redirect (`src-tauri/src/cloud/oauth_flow.rs`
binds `127.0.0.1:0`). Google permits any `127.0.0.1`/`localhost` port for Desktop
clients automatically.

## 5. Provide the credentials to the app

SundayRec resolves the client from env vars at launch (falling back to values
baked in at build time via the same names — see
`src-tauri/src/cloud/config.rs`):

```bash
export SUNDAYREC_GOOGLE_CLIENT_ID="123-abc.apps.googleusercontent.com"
export SUNDAYREC_GOOGLE_CLIENT_SECRET="GOCSPX-…"   # optional but Google still
                                                   # wants it for the code exchange
npm run tauri dev
```

- If `SUNDAYREC_GOOGLE_CLIENT_ID` is missing/blank, cloud is "not configured":
  the upload worker idles silently and **cloud connect surfaces a clear error**
  instead of starting a doomed flow.
- A blank secret is dropped (never sent as an empty `client_secret`).

## 6. Smoke-test the connect flow

With the env set and `npm run tauri dev` running:

1. Trigger **cloud connect** (Drive) in the UI.
2. The **system browser** opens Google's consent screen → approve as a test user.
3. The loopback redirect completes and the service shows **connected**; the
   refresh token is stored in the OS keychain (not in the database).
4. Enqueue a backup and watch the upload — with `RUST_LOG=sundayrec=debug` the
   worker logs each resumable chunk.

### Troubleshooting

| Symptom                              | Likely cause                                        |
| ------------------------------------ | --------------------------------------------------- |
| "client not configured" on connect   | env var didn't reach the process (export, relaunch) |
| `access_denied` / blocked on consent | account not added under **Test users** (§3.4)       |
| `redirect_uri_mismatch`              | client created as Web, not **Desktop app** (§4.1)   |
| Connects, upload never starts        | the right API not enabled (§2), or no network       |
| `invalid_grant` later                | refresh token revoked — reconnect to re-consent     |

## 7. Security notes

- The Desktop client id + secret are **not** confidential (Google's own docs).
- `drive.file` keeps the app out of the rest of your Drive.
- Tokens live in the **OS keychain** via `keyring`, never in plaintext config or
  the SQLite database.
