# Connectors, phase 2: services that need a sign-in

Status: **built 2026-11-16** on Huan's "Go to next phase", as phase 2 of
the approved `docs/design/done/connectors-plan.md`. Checked against the
four real services up to their own sign-in pages; finishing a sign-in
needs the user's accounts.

## What exists
Checked on 2026-11-16. Each service answers 401 with MCP's standard
sign-in description (OAuth 2.1, RFC 9728 resource metadata):

| Service | Self-registration (no client id needed) | In Sandhill's list |
|---|---|---|
| Consensus (research papers) | yes | yes |
| Synapse (Sage Bionetworks data platform) | yes | yes |
| Wiley Scholar Gateway (Wiley journals and books) | yes | yes |
| BioRender (figure icons and templates) | yes | yes |
| Owkin | no: Amazon Cognito, needs a client id Owkin issues | no; addable as a web address with a client id |
| Benchling | each company's own address | no; addable as a web address |

## How signing in works
1. Adding one of these (from the list, or a web address that answers 401
   with a sign-in description) opens a card: "Sign in to Consensus".
2. The Mac app listens on `http://127.0.0.1:<free port>/callback` and asks
   the device's daemon to begin. The daemon reads the service's resource
   and authorization-server descriptions, registers Sandhill as a client
   (or uses the client id the user typed), makes the PKCE pair and returns
   the address to open.
3. The Mac opens it in the browser; the user signs in there with their own
   account. The browser comes back to the Mac's loopback address with a
   code; the page says "Signed in. You can close this tab."
4. The app hands the code to the device's daemon, which exchanges it for
   tokens, connects, lists the tools and saves the connector. The device
   does the exchange itself, so this works for servers over ssh too: only
   the browser and the loopback address are on the Mac.
5. The daemon refreshes the token when it is about to expire, and once on
   a 401. If refreshing fails, the row says so and offers Sign In Again.

Tokens live with the other secrets (`connectors/secrets.json`, 0600, on
the device); they never reach agents, logs, exports or the Mac app. Copy
to… adds the connector on the other device and runs the sign-in there,
as tokens are not copied. A sign-in started but not finished is
forgotten after 15 minutes or when the daemon restarts.

## Agents
Unchanged tools. These services are not marked read-only as a whole:
`query_connector` also runs a tool of a connector from Sandhill's list
when the service marks that tool read-only (`readOnlyHint`); other tools
go through `call_connector` and the agent's approval.

## Not building
- Client ID Metadata Documents (Synapse and Wiley offer them; self-
  registration covers all four).
- Signing in without a browser on the Mac.

## Tests
A stand-in service in `crates/daemon/tests/connectors.rs`: 401 with
resource metadata, authorization-server metadata, registration, an
authorize step the test drives, token and refresh endpoints. Sign in,
call, expire and refresh, refresh failing → "sign in again", tokens never
in replies, wrong `state` refused, unfinished sign-in expiring. The app's
loopback listener and card are checked on screen with a real service.
