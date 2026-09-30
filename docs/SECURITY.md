# Security notes (OTP / Termii)

This document covers the Aframp → Termii OTP data flow and the constraints of
Termii's public API. It supplements any project-wide vulnerability reporting
process maintainers may publish separately.

## OTP data flow

1. A client calls `POST /signup` or `POST /login` with a phone number.
2. Aframp generates a 6-digit OTP, stores only an HMAC of the code in
   `otp_challenges`, and formats a plain-text SMS:
   `Your Aframp verification code is {code}. It expires in 10 minutes.`
3. When `OTP_PROVIDER=termii`, `TermiiProvider::send_sms` POSTs that message to
   Termii's Messaging API over **HTTPS** (`https://api.ng.termii.com/api/sms/send`).
4. Termii delivers the SMS to the handset. Delivery-status callbacks arrive at
   `POST /webhooks/termii` and are verified with `X-Termii-Signature`
   (HMAC-SHA512 over the raw body using `TERMII_API_KEY`).

Local/dev never needs a live Termii account: set `OTP_PROVIDER=mock` and the
code is logged via `tracing` instead of sent.

## API key placement (issues #1120 / #1122)

Termii's Messaging API and Token API both require the account `api_key` as a
**JSON body field**. Their published docs do **not** document Bearer / 
`Authorization` header authentication for these endpoints.

Consequence:

- Aframp must include `"api_key"` in the POST body on every SMS send.
- Proxies, WAFs, or APM tools that log **request bodies** between Aframp and
  Termii could capture the API key (and the OTP text) in plain text.
- Aframp **never** places the API key in the request URL or query string.
  Unit tests in `src/otp/termii.rs` assert the send URL contains no secret.

Mitigations in place / recommended:

| Control | Status |
|---|---|
| HTTPS to Termii | Required (hard-coded `https://` base URL) |
| API key never in URL/query | Enforced + tested |
| Prefer `OTP_PROVIDER=mock` outside production | Documented in README / `.env.example` |
| Disable body logging on egress proxies for `api.ng.termii.com` | Operator responsibility |
| Migrate to Termii Token API (`/api/sms/otp/send`) | Recommended follow-up — see below |

## Termii data retention / logging

Termii's public developer docs describe request/response shapes but do **not**
publish a customer-facing retention schedule for SMS body contents or API
request logs. Operators should:

1. Review Termii's current privacy / DPA terms in the dashboard account area
   before enabling production SMS.
2. Assume SMS bodies (including OTPs) may be retained by the carrier and by
   Termii for delivery troubleshooting.
3. Keep OTP TTLs short (Aframp uses 10 minutes) and never log the plaintext
   code in Aframp application logs when using the Termii provider.

## Token API consideration

Termii's [Token API](https://developers.termii.com/token) (`POST /api/sms/otp/send`
+ verify) generates and checks OTPs on Termii's side. Benefits:

- Purpose-built OTP lifecycle (attempts, TTL, pin placeholder).
- Aframp would no longer need to format the raw code into an SMS body itself.

Caveats:

- The Token API **still** requires `api_key` in the JSON body — it does not
  solve the body-logging concern for the API key.
- Migrating means storing Termii's `pin_id` and calling their verify endpoint
  instead of local HMAC comparison. Tracked as a follow-up; not required to
  close the immediate key-in-URL / documentation gaps.

## Reporting

If you discover a vulnerability in how Aframp handles OTP secrets or provider
credentials, contact the maintainers privately rather than opening a public
issue with exploit details.
